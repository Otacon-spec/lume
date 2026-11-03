//! Stato **vivo** della modalità SQL, posseduto da `Lume`.
//!
//! Sta qui e non dentro il tool Database per una ragione strutturale, la
//! stessa che ferma il push del terminale: `subscription()` legge `&self` e il
//! `Registry` espone solo `&dyn Tool`, senza downcast. Il timer
//! d'ibernazione deve poter leggere quando è stato chiuso tutto, quindi il
//! peso sta sull'app e il tool resta una vista pura.
//!
//! Cosa vive dove, e perché:
//! - i **profili** (host, database, motore) sono leggeri e stanno su `Lume`:
//!   esistono anche da freddo, altrimenti il pannello sarebbe vuoto al primo
//!   avvio;
//! - la **schema cache** sta qui e muore con l'ibernazione (`hibernate_sql` azzera
//!   `Lume::sql`): rende istantanea solo una riapertura prima del timeout;
//! - le **griglie risultati** sono la parte grossa e invisibile a pannello
//!   chiuso: si liberano subito, senza aspettare il timeout;
//! - le **connessioni** muoiono con lo `SqlWorkspace`, e con loro i thread.

#[cfg(test)]
use lume_sql::QueryEvent;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::mpsc::Receiver;
use std::time::Duration;

use futures::channel::oneshot;
use lume_sql::{
    ColumnInfo, Connection, ConnectionProfile, ResultSet, Secret, SqlError, Value, worker::Request,
    worker::Response,
};

/// Quanto una connessione resta viva dopo che console e pannello si sono
/// chiusi entrambi. **Deve stare sotto l'idle timeout del server** (Azure SQL
/// è sui 30 minuti): tenere un socket che il server ha già chiuso farebbe
/// fallire la prima query dopo la riapertura invece di riconnettersi.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Quante righe per pagina nella griglia risultati (misura del prototipo).
pub const PAGE_SIZE: usize = 20;

/// Result set oltre il tetto di sessione: si tiene il testo, non le righe.
pub const RESULT_NOT_RESTORED: &str = "risultato non ripristinato";

/// Nessuna query attesa. Le generazioni vere partono da 1 (`fetch_add`
/// restituisce il valore vecchio, e si somma 1), quindi lo zero è libero.
pub const NO_QUERY: u64 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    pub const fn reversed(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sort {
    pub column: String,
    pub direction: SortDirection,
}

/// Stato del riquadro, mostrato dalla barra in fondo alla console.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PaneStatus {
    /// «Premi Esegui o Ctrl+↵ per lanciare la query».
    #[default]
    Idle,
    /// Query in volo. «Scarta risultato» in toolbar abbandona l'attesa e, su
    /// Azure SQL, interrompe la query sul server (Attention TDS); su Oracle
    /// la query prosegue finché il server non finisce.
    Running,
    Paused,
    Ok,
    Error(String),
}

/// Un riquadro della console. Sono due solo in modalità confronto.
#[derive(Debug, Default)]
pub struct Pane {
    /// Id del profilo selezionato nel menù della connessione.
    pub connection: Option<String>,
    /// Griglia corrente. `None` = mai eseguito, oppure liberata dall'ibernazione.
    pub results: Option<ResultSet>,
    /// Result set non attualmente selezionati. Lo slot selezionato è `None`:
    /// la griglia grossa resta posseduta una sola volta e si scambia al click.
    pub(crate) result_sets: Vec<Option<ResultSet>>,
    pub(crate) selected_result_set: usize,
    pub(crate) streaming_result_set: usize,
    pub page: usize,
    pub sort: Option<Sort>,
    /// Indici ordinati del risultato corrente. `None` significa l'ordine del
    /// server, che si può indicizzare direttamente senza allocare un vettore.
    pub(crate) sorted_rows: Option<Vec<usize>>,
    /// Testo delle sole celle visibili. Evita di formattare numeri e BLOB a
    /// ogni repaint senza duplicare l'intero result set.
    pub(crate) page_text: Vec<Vec<String>>,
    pub(crate) cached_page: Option<usize>,
    pub status: PaneStatus,
    /// Menù «▾» del selettore connessione aperto.
    pub connection_menu_open: bool,
    /// Generazione della query che questo riquadro sta aspettando.
    ///
    /// Per riquadro e **non per connessione**: sulla stessa connessione
    /// passano anche le richieste di schema del pannello Database, e un
    /// contatore condiviso farebbe buttare via il risultato di una query
    /// completata solo perché nel frattempo qualcuno ha aperto una tabella
    /// nell'albero. `NO_QUERY` = non ne aspetto nessuna.
    pub query_gen: u64,
    /// Lease del consumer, separato dal socket/timeout SQL cinque minuti.
    pub(crate) query_origin: Option<lume_sql::QueryOrigin>,
    /// Memoria direttamente posseduta dai batch ricevuti. Non rappresenta la
    /// RAM totale del processo.
    pub accounted_bytes: usize,
    pub incomplete: bool,
}

impl Pane {
    pub fn set_results(&mut self, results: ResultSet) {
        self.abandon_stream();
        self.accounted_bytes = results.accounted_bytes();
        self.incomplete = false;
        self.results = Some(results);
        self.result_sets.clear();
        self.result_sets.push(None);
        self.selected_result_set = 0;
        self.streaming_result_set = 0;
        self.page = 0;
        self.sort = None;
        self.sorted_rows = None;
        self.refresh_page_text();
    }

    pub(crate) fn bind_stream(&mut self, origin: lume_sql::QueryOrigin) {
        self.abandon_stream();
        self.query_origin = Some(origin);
    }
    pub(crate) fn abandon_stream(&mut self) {
        if let Some(origin) = self.query_origin.take() {
            origin.abandon();
        }
    }
    pub(crate) fn finish_stream(&mut self) {
        self.query_origin = None;
    }
    pub fn clear_results(&mut self) {
        self.abandon_stream();
        self.clear_grid();
    }
    fn clear_grid(&mut self) {
        self.results = None;
        self.result_sets.clear();
        self.selected_result_set = 0;
        self.streaming_result_set = 0;
        self.page = 0;
        self.sort = None;
        self.sorted_rows = None;
        self.page_text.clear();
        self.cached_page = None;
        self.accounted_bytes = 0;
        self.incomplete = false;
    }

    pub fn begin_stream(&mut self, columns: Vec<ColumnInfo>) {
        // Started inizializza i dati della LEASE corrente, non la abbandona.
        self.clear_grid();
        self.results = Some(ResultSet {
            columns,
            rows: Vec::new(),
            elapsed: Duration::ZERO,
        });
        self.result_sets.push(None);
        self.selected_result_set = 0;
        self.streaming_result_set = 0;
        self.recount_result_bytes();
        self.cached_page = Some(0);
    }

    pub fn begin_result_set(&mut self, index: usize, columns: Vec<ColumnInfo>) {
        if index != self.result_sets.len() {
            self.incomplete = true;
            return;
        }
        if let Some(current) = self.results.take() {
            let Some(slot) = self.result_sets.get_mut(self.selected_result_set) else {
                self.results = Some(current);
                self.incomplete = true;
                return;
            };
            *slot = Some(current);
        }
        self.result_sets.push(None);
        self.selected_result_set = index;
        self.streaming_result_set = index;
        self.results = Some(ResultSet {
            columns,
            rows: Vec::new(),
            elapsed: Duration::ZERO,
        });
        self.page = 0;
        self.sort = None;
        self.sorted_rows = None;
        self.page_text.clear();
        self.cached_page = Some(0);
        self.recount_result_bytes();
    }

    pub fn result_set_count(&self) -> usize {
        self.result_sets.len()
    }

    pub fn selected_result_set(&self) -> usize {
        self.selected_result_set
    }

    pub(crate) fn result_set(&self, index: usize) -> Option<&ResultSet> {
        if index == self.selected_result_set {
            self.results.as_ref()
        } else {
            self.result_sets.get(index).and_then(Option::as_ref)
        }
    }

    pub fn set_results_elapsed(&mut self, elapsed: Duration) {
        if let Some(results) = self.results.as_mut() {
            results.elapsed = elapsed;
        }
        for result in self.result_sets.iter_mut().flatten() {
            result.elapsed = elapsed;
        }
    }

    pub fn select_result_set(&mut self, index: usize) -> bool {
        if index >= self.result_sets.len() || index == self.selected_result_set {
            return index == self.selected_result_set && index < self.result_sets.len();
        }
        let Some(target_results) = self.result_sets.get_mut(index).and_then(Option::take) else {
            return false;
        };
        let Some(current) = self.results.take() else {
            if let Some(target) = self.result_sets.get_mut(index) {
                *target = Some(target_results);
            }
            return false;
        };
        self.results = Some(target_results);
        if let Some(slot) = self.result_sets.get_mut(self.selected_result_set) {
            *slot = Some(current);
        }
        self.selected_result_set = index;
        self.page = 0;
        self.sort = None;
        self.sorted_rows = None;
        self.refresh_page_text();
        true
    }

    pub(crate) fn restore_result_sets(&mut self, result_sets: Vec<ResultSet>, selected: usize) {
        let Some(first) = result_sets.first().cloned() else {
            self.clear_grid();
            return;
        };
        self.set_results(first);
        for (index, result) in result_sets.into_iter().enumerate().skip(1) {
            self.result_sets.push(Some(result));
            self.streaming_result_set = index;
        }
        let _ = self.select_result_set(selected.min(self.result_sets.len() - 1));
        self.recount_result_bytes();
    }

    pub fn append_batch(&mut self, batch: lume_sql::QueryBatch) {
        let results = if self.streaming_result_set == self.selected_result_set {
            self.results.as_mut()
        } else {
            self.result_sets
                .get_mut(self.streaming_result_set)
                .and_then(Option::as_mut)
        };
        if let Some(results) = results {
            results.rows.extend(batch.rows);
            self.accounted_bytes = self
                .results
                .as_ref()
                .map_or(0, ResultSet::accounted_bytes)
                .saturating_add(
                    self.result_sets
                        .iter()
                        .flatten()
                        .map(ResultSet::accounted_bytes)
                        .sum::<usize>(),
                );
            if self.streaming_result_set == self.selected_result_set {
                self.sorted_rows = None;
                self.refresh_page_text();
            }
        }
    }

    fn recount_result_bytes(&mut self) {
        self.accounted_bytes = self
            .result_sets
            .iter()
            .flatten()
            .map(ResultSet::accounted_bytes)
            .sum::<usize>()
            .saturating_add(self.results.as_ref().map_or(0, ResultSet::accounted_bytes));
    }

    pub fn set_sort(&mut self, sort: Sort) {
        self.sort = Some(sort);
        self.page = 0;
        self.rebuild_order();
        self.refresh_page_text();
    }

    pub fn set_page(&mut self, page: usize) {
        self.page = page;
        self.refresh_page_text();
    }

    pub fn row_index_at(&self, slot: usize) -> Option<usize> {
        let index = self.page.checked_mul(PAGE_SIZE)?.checked_add(slot)?;
        match &self.sorted_rows {
            Some(rows) => rows.get(index).copied(),
            None => self.results.as_ref()?.rows.get(index).map(|_| index),
        }
    }

    pub fn page_len(&self) -> usize {
        let Some(results) = self.results.as_ref() else {
            return 0;
        };
        results
            .row_count()
            .saturating_sub(self.page.saturating_mul(PAGE_SIZE))
            .min(PAGE_SIZE)
    }

    pub fn text_at(&self, slot: usize, column: usize) -> Option<&str> {
        (self.cached_page == Some(self.page))
            .then(|| self.page_text.get(slot)?.get(column).map(String::as_str))
            .flatten()
    }

    fn rebuild_order(&mut self) {
        let Some(results) = self.results.as_ref() else {
            self.sorted_rows = None;
            return;
        };
        let Some(sort) = self.sort.as_ref() else {
            self.sorted_rows = None;
            return;
        };
        let Some(column) = results
            .columns
            .iter()
            .position(|info| info.name == sort.column)
        else {
            self.sorted_rows = None;
            return;
        };
        let mut rows: Vec<usize> = (0..results.rows.len()).collect();
        rows.sort_by(|left, right| {
            let ordering = compare_values(
                results.rows.get(*left).and_then(|row| row.get(column)),
                results.rows.get(*right).and_then(|row| row.get(column)),
            );
            match sort.direction {
                SortDirection::Ascending => ordering,
                SortDirection::Descending => ordering.reverse(),
            }
        });
        self.sorted_rows = Some(rows);
    }

    fn refresh_page_text(&mut self) {
        let rows: Vec<Vec<String>> = self
            .results
            .as_ref()
            .map(|results| {
                (0..self.page_len())
                    .filter_map(|slot| self.row_index_at(slot))
                    .filter_map(|row| results.rows.get(row))
                    .map(|cells| cells.iter().map(ToString::to_string).collect())
                    .collect()
            })
            .unwrap_or_default();
        self.page_text = rows;
        self.cached_page = self.results.as_ref().map(|_| self.page);
    }
}

/// Ordinamento stabile della griglia. I tipi uguali mantengono il loro ordine
/// naturale; i casi misti mantengono la semantica testuale precedente.
pub fn compare_values(left: Option<&Value>, right: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (left, right) {
        (Some(Value::Int(a)), Some(Value::Int(b))) => a.cmp(b),
        (Some(Value::Float(a)), Some(Value::Float(b))) => {
            a.partial_cmp(b).unwrap_or(Ordering::Equal)
        }
        (Some(Value::Bool(a)), Some(Value::Bool(b))) => a.cmp(b),
        (Some(Value::Text(a)), Some(Value::Text(b))) => a.cmp(b),
        (Some(Value::Bytes(a)), Some(Value::Bytes(b))) => a.cmp(b),
        (Some(Value::Null), Some(Value::Null)) | (None, None) => Ordering::Equal,
        (Some(Value::Null) | None, _) => Ordering::Greater,
        (_, Some(Value::Null) | None) => Ordering::Less,
        (Some(a), Some(b)) => a.to_string().cmp(&b.to_string()),
    }
}

/// Tabelle e colonne note di una connessione.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConnectionSchema {
    pub tables: Vec<String>,
    pub columns: HashMap<String, Vec<ColumnInfo>>,
}

/// Una connessione viva più il suo **token di generazione**. Il token sta
/// accanto alla connessione e non sull'app perché è per connessione: in
/// modalità confronto A e B corrono davvero in parallelo, e una risposta
/// tardiva di A non deve poter scartare quella di B.
struct Live {
    connection: Connection,
    generation: Arc<AtomicU64>,
}

pub struct SqlWorkspace {
    connections: Vec<Live>,
    schema: HashMap<String, ConnectionSchema>,
    panes: [Pane; 2],
    /// Modalità «Confronta due database»: il secondo riquadro è visibile.
    pub compare: bool,
    pub sync_scroll: bool,
    /// Offset verticale condiviso tra i riquadri in modalità confronto.
    pub grid_scroll_y: f32,
    /// Evita il loop quando si sincronizza lo scroll programmaticamente.
    pub scroll_echo: Option<usize>,
}

impl SqlWorkspace {
    pub fn new() -> Self {
        Self {
            connections: Vec::new(),
            schema: HashMap::new(),
            panes: [Pane::default(), Pane::default()],
            compare: false,
            sync_scroll: true,
            grid_scroll_y: 0.0,
            scroll_echo: None,
        }
    }

    /// Riquadri realmente attivi: uno, o due in modalità confronto.
    pub fn active_panes(&self) -> &[Pane] {
        if self.compare {
            &self.panes
        } else {
            &self.panes[..1]
        }
    }

    pub fn pane_mut(&mut self, index: usize) -> Option<&mut Pane> {
        self.panes.get_mut(index)
    }

    pub fn connection(&self, id: &str) -> Option<&Connection> {
        self.connections
            .get(self.index_of(id)?)
            .map(|live| &live.connection)
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.connections
            .iter()
            .position(|live| live.connection.profile().id == id)
    }

    /// Accoda una richiesta e apre una **nuova generazione**: la risposta
    /// porta con sé il numero, e chi la riceve scarta quelle superate. Copre
    /// insieme tre casi — riesecuzione mentre una query è in volo, riapertura
    /// dopo l'ibernazione sullo stesso profilo, risposta che arriva quando lo
    /// `SqlWorkspace` non c'è più.
    pub fn submit(
        &self,
        id: &str,
        request: Request,
    ) -> Result<(oneshot::Receiver<Result<Response, SqlError>>, u64), SqlError> {
        let live =
            self.connections
                .get(self.index_of(id).ok_or_else(|| {
                    SqlError::Unavailable(format!("connessione «{id}» non aperta"))
                })?)
                .ok_or_else(|| SqlError::Unavailable(format!("connessione «{id}» non aperta")))?;
        let generation = live.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let receiver = live
            .connection
            .submit(request, live.generation.clone(), generation)?;
        Ok((receiver, generation))
    }

    /// Le prove legacy pilotano l'API std reale del backend; produzione usa
    /// la coda async, non un alias del trasporto nuovo.
    #[cfg(test)]
    pub fn submit_query_stream(
        &self,
        id: &str,
        sql: String,
    ) -> Result<(Receiver<QueryEvent>, u64), SqlError> {
        let live =
            self.connections
                .get(self.index_of(id).ok_or_else(|| {
                    SqlError::Unavailable(format!("connessione «{id}» non aperta"))
                })?)
                .ok_or_else(|| SqlError::Unavailable(format!("connessione «{id}» non aperta")))?;
        let generation = live.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let receiver =
            live.connection
                .submit_query_stream(sql, live.generation.clone(), generation)?;
        Ok((receiver, generation))
    }

    /// Percorso UI senza relay thread o spool unbounded.
    pub fn submit_query_events(
        &self,
        id: &str,
        sql: String,
    ) -> Result<(lume_sql::QueryEvents, u64), SqlError> {
        let live =
            self.connections
                .get(self.index_of(id).ok_or_else(|| {
                    SqlError::Unavailable(format!("connessione «{id}» non aperta"))
                })?)
                .ok_or_else(|| SqlError::Unavailable(format!("connessione «{id}» non aperta")))?;
        let generation = live.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let receiver =
            live.connection
                .submit_query_events(sql, live.generation.clone(), generation)?;
        Ok((receiver, generation))
    }

    pub fn continue_query(&self, pane: usize) -> Result<(), SqlError> {
        let pane = self
            .pane(pane)
            .ok_or_else(|| SqlError::Unavailable("riquadro assente".into()))?;
        let id = pane
            .connection
            .as_deref()
            .ok_or_else(|| SqlError::Unavailable("nessuna connessione".into()))?;
        self.connection(id)
            .ok_or_else(|| SqlError::Unavailable("connessione chiusa".into()))?
            .continue_query(pane.query_gen)
    }

    pub fn stop_query(&self, pane: usize) -> Result<(), SqlError> {
        let pane = self
            .pane(pane)
            .ok_or_else(|| SqlError::Unavailable("riquadro assente".into()))?;
        let id = pane
            .connection
            .as_deref()
            .ok_or_else(|| SqlError::Unavailable("nessuna connessione".into()))?;
        self.connection(id)
            .ok_or_else(|| SqlError::Unavailable("connessione chiusa".into()))?
            .stop_query(pane.query_gen)
    }

    /// Apre una connessione se non c'è già. Il thread parte subito ma la
    /// connessione vera avviene al suo interno: qui non si blocca nulla.
    pub fn open(
        &mut self,
        profile: ConnectionProfile,
        secret: Secret,
        factory: lume_sql::DriverFactory,
    ) -> Result<(), SqlError> {
        if self.index_of(&profile.id).is_some() {
            return Ok(());
        }
        let connection = Connection::open(profile, secret, factory)
            .map_err(|error| SqlError::Connect(error.to_string()))?;
        self.connections.push(Live {
            connection,
            generation: Arc::new(AtomicU64::new(0)),
        });
        Ok(())
    }

    /// Chiude una connessione **subito**, senza aspettare l'ibernazione.
    /// Droppare il `Live` fa cadere il socket e uscire il thread. Serve a due
    /// casi: il profilo eliminato, e la riconnessione dopo un tentativo
    /// fallito (`open` è idempotente sull'id, quindi senza questo un errore
    /// di password resterebbe appiccicato).
    pub fn close(&mut self, id: &str) -> bool {
        match self.index_of(id) {
            Some(index) => {
                self.connections.remove(index);
                self.schema.remove(id);
                true
            }
            None => false,
        }
    }

    /// Riquadro per indice, **compreso quello nascosto**. Il controllo delle
    /// risposte deve usare questo e non `active_panes`: uscire da «Confronta»
    /// mentre B ha una query in volo non deve lasciarla `Running` per sempre.
    pub fn pane(&self, index: usize) -> Option<&Pane> {
        self.panes.get(index)
    }

    /// «Scambia sinistra/destra» del prototipo.
    pub fn swap_panes(&mut self) {
        self.panes.swap(0, 1);
    }

    /// Connessioni vive. Non è più solo per i test: il pannello
    /// Estensioni la mostra all'utente.
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    pub fn schema_for(&self, id: &str) -> Option<&ConnectionSchema> {
        self.schema.get(id)
    }

    pub fn set_tables(&mut self, id: &str, tables: Vec<String>) {
        self.schema.entry(id.to_owned()).or_default().tables = tables;
    }

    pub fn set_columns(&mut self, id: &str, table: String, columns: Vec<ColumnInfo>) {
        self.schema
            .entry(id.to_owned())
            .or_default()
            .columns
            .insert(table, columns);
    }

    /// Fronte «tutto chiuso»: le griglie sono la parte grossa, sono invisibili
    /// e sono ri-fetchabili. Socket e schema cache restano, così chi riapre
    /// entro il timeout non ripaga handshake TLS e autenticazione. Una query
    /// ancora in corso viene interrotta sul server (Azure SQL) oltre che
    /// abbandonata: nessuno leggerà quel risultato.
    pub fn release_results(&mut self) {
        self.cancel_running_queries();
        self.stop_paused_queries();
        for pane in &mut self.panes {
            pane.clear_results();
            // Una query può essere ancora in volo: azzerare l'atteso fa
            // scartare la sua risposta, che altrimenti ripopolerebbe proprio
            // la griglia appena liberata.
            pane.query_gen = NO_QUERY;
            if matches!(
                pane.status,
                PaneStatus::Ok | PaneStatus::Running | PaneStatus::Paused
            ) {
                pane.status = PaneStatus::Idle;
            }
        }
    }

    /// Abbandona l'attesa delle query in volo **e** le interrompe sul server
    /// dove il motore sa farlo (Azure SQL: Attention TDS; Oracle no, e lì la
    /// query prosegue finché il server non finisce). Una query ferma al
    /// budget viene invece risvegliata, così il suo worker non resta in
    /// attesa se l'utente abbandona il risultato.
    pub fn discard_running(&mut self) {
        self.cancel_running_queries();
        self.stop_paused_queries();
        for pane in self.panes.iter_mut() {
            if matches!(pane.status, PaneStatus::Running | PaneStatus::Paused) {
                pane.abandon_stream();
                pane.query_gen = NO_QUERY;
                pane.status = PaneStatus::Idle;
            }
        }
    }

    /// Interrompe le query dei riquadri `Running`. Va chiamata **prima** di
    /// cambiare lo stato dei riquadri: dopo, non si saprebbe più quali
    /// connessioni avevano una query in corso. Le connessioni senza
    /// `Interrupter` (Oracle) ignorano la richiesta.
    fn cancel_running_queries(&self) {
        for pane in &self.panes {
            if pane.status == PaneStatus::Running
                && pane.query_gen != NO_QUERY
                && let Some(id) = pane.connection.as_deref()
                && let Some(connection) = self.connection(id)
            {
                connection.cancel_running();
            }
        }
    }

    /// Risveglia solamente i cursori che sono davvero in pausa. Gli errori
    /// sono innocui: tra la fotografia della UI e il comando il worker può
    /// aver già completato o ricevuto Stop da una nuova query.
    fn stop_paused_queries(&self) {
        let paused = self
            .panes
            .iter()
            .filter(|pane| pane.status == PaneStatus::Paused && pane.query_gen != NO_QUERY)
            .filter_map(|pane| {
                pane.connection
                    .as_deref()
                    .map(|connection| (connection.to_owned(), pane.query_gen))
            })
            .collect::<Vec<_>>();

        for (connection, generation) in paused {
            if let Some(connection) = self.connection(&connection) {
                let _ = connection.stop_query(generation);
            }
        }
    }
}

impl Default for SqlWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use lume_sql::{AuthKind, Driver, Engine, Interrupter, Value};

    use super::*;

    struct FakeDriver;

    impl Driver for FakeDriver {
        fn tables(&mut self) -> Result<Vec<String>, SqlError> {
            Ok(vec!["users".into()])
        }
        fn columns(&mut self, _table: &str) -> Result<Vec<ColumnInfo>, SqlError> {
            Ok(Vec::new())
        }
        fn query_stream(
            &mut self,
            _sql: &str,
            sink: &mut dyn lume_sql::QuerySink,
        ) -> Result<Duration, SqlError> {
            lume_sql::stream_result_set(result_set(), sink)
        }
    }

    fn factory(
        _profile: &ConnectionProfile,
        _secret: &Secret,
    ) -> Result<Box<dyn Driver>, SqlError> {
        Ok(Box::new(FakeDriver))
    }

    fn result_set() -> ResultSet {
        ResultSet {
            columns: vec![ColumnInfo {
                name: "id".into(),
                type_name: "int".into(),
            }],
            rows: vec![vec![Value::Int(1)]],
            elapsed: Duration::from_millis(42),
        }
    }

    #[test]
    fn pane_conserva_e_seleziona_tutti_i_result_set_di_una_query() {
        let mut pane = Pane::default();
        pane.begin_stream(vec![ColumnInfo {
            name: "first".into(),
            type_name: "int".into(),
        }]);
        pane.append_batch(lume_sql::QueryBatch::new(vec![vec![Value::Int(1)]]));
        pane.begin_result_set(
            1,
            vec![ColumnInfo {
                name: "second".into(),
                type_name: "int".into(),
            }],
        );
        assert!(pane.select_result_set(0));
        pane.append_batch(lume_sql::QueryBatch::new(vec![vec![Value::Int(2)]]));

        assert_eq!(pane.result_set_count(), 2);
        assert_eq!(pane.results.as_ref().unwrap().columns[0].name, "first");
        assert_eq!(pane.results.as_ref().unwrap().rows[0][0], Value::Int(1));
        assert_eq!(pane.result_set(1).unwrap().rows[0][0], Value::Int(2));
        assert!(pane.select_result_set(1));
        assert_eq!(pane.results.as_ref().unwrap().rows[0][0], Value::Int(2));
    }

    fn profile(id: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: id.into(),
            label: format!("azure · {id}"),
            engine: Engine::AzureSql,
            host: "acme.database.windows.net".into(),
            database: "prod".into(),
            auth: AuthKind::SqlLogin {
                user: "lume".into(),
            },
        }
    }

    #[test]
    fn liberare_i_risultati_tiene_socket_e_schema() {
        let mut workspace = SqlWorkspace::new();
        workspace
            .open(profile("a"), Secret::new("s".into()), factory)
            .unwrap();
        workspace.set_tables("a", vec!["users".into()]);
        workspace.pane_mut(0).unwrap().set_results(result_set());
        workspace.pane_mut(0).unwrap().status = PaneStatus::Ok;
        workspace.pane_mut(0).unwrap().page = 3;

        workspace.release_results();

        assert!(workspace.active_panes()[0].results.is_none());
        assert_eq!(workspace.active_panes()[0].page, 0);
        assert_eq!(workspace.active_panes()[0].status, PaneStatus::Idle);
        assert_eq!(
            workspace.connection_count(),
            1,
            "l'ibernazione non chiude ancora il socket: e' il timeout a farlo"
        );
        assert_eq!(
            workspace.schema_for("a").unwrap().tables,
            vec!["users".to_string()],
            "la schema cache e' cio' che rende istantanea la riapertura"
        );
    }

    #[test]
    fn un_errore_sopravvive_alla_liberazione_dei_risultati() {
        let mut workspace = SqlWorkspace::new();
        workspace.pane_mut(0).unwrap().status = PaneStatus::Error("ORA-00942".into());
        workspace.release_results();
        assert_eq!(
            workspace.active_panes()[0].status,
            PaneStatus::Error("ORA-00942".into()),
            "l'utente deve poter leggere l'errore anche dopo aver riaperto"
        );
    }

    #[test]
    fn liberare_o_scartare_un_risultato_rimuove_anche_lo_stato_in_pausa() {
        let mut workspace = SqlWorkspace::new();
        let pane = workspace.pane_mut(0).unwrap();
        pane.status = PaneStatus::Paused;
        pane.query_gen = 42;
        workspace.release_results();
        assert_eq!(workspace.active_panes()[0].status, PaneStatus::Idle);
        assert_eq!(workspace.active_panes()[0].query_gen, NO_QUERY);

        let pane = workspace.pane_mut(0).unwrap();
        pane.status = PaneStatus::Paused;
        pane.query_gen = 43;
        workspace.discard_running();
        assert_eq!(workspace.active_panes()[0].status, PaneStatus::Idle);
        assert_eq!(workspace.active_panes()[0].query_gen, NO_QUERY);
    }

    #[test]
    fn memoria_contabilizzata_riflette_il_result_set_dopo_ogni_batch() {
        let mut pane = Pane::default();
        pane.begin_stream(vec![ColumnInfo {
            name: "nota".into(),
            type_name: "nvarchar".into(),
        }]);
        assert_eq!(
            pane.accounted_bytes,
            pane.results.as_ref().unwrap().accounted_bytes()
        );

        pane.append_batch(lume_sql::QueryBatch::new(vec![vec![Value::Text(
            "dato nel batch".into(),
        )]]));
        assert_eq!(
            pane.accounted_bytes,
            pane.results.as_ref().unwrap().accounted_bytes(),
            "la lista del risultato, non il Vec temporaneo del batch, e' la fonte del contatore"
        );
    }

    #[test]
    fn aprire_due_volte_la_stessa_connessione_non_duplica_il_thread() {
        let mut workspace = SqlWorkspace::new();
        workspace
            .open(profile("a"), Secret::new("s".into()), factory)
            .unwrap();
        workspace
            .open(profile("a"), Secret::new("s".into()), factory)
            .unwrap();
        assert_eq!(workspace.connection_count(), 1);
        assert_eq!(workspace.active_panes().len(), 1);
        workspace.compare = true;
        assert_eq!(workspace.active_panes().len(), 2);
    }

    /// Doppio di **ciclo di vita**, non di SQL: una query che dura finché
    /// non la si interrompe (max 30 s). L'`Interrupter` è attivo solo durante
    /// la query, come quello di Azure.
    struct Bloccato {
        started: std::sync::atomic::AtomicBool,
        in_corso: std::sync::atomic::AtomicBool,
        interrotto: AtomicU64,
        flag: std::sync::Mutex<bool>,
        wake: std::sync::Condvar,
    }

    impl Bloccato {
        const fn new() -> Self {
            Self {
                started: std::sync::atomic::AtomicBool::new(false),
                in_corso: std::sync::atomic::AtomicBool::new(false),
                interrotto: AtomicU64::new(0),
                flag: std::sync::Mutex::new(false),
                wake: std::sync::Condvar::new(),
            }
        }

        fn interrupt(&self) {
            if !self.in_corso.load(Ordering::SeqCst) {
                return;
            }
            self.interrotto.fetch_add(1, Ordering::SeqCst);
            *self.flag.lock().unwrap() = true;
            self.wake.notify_all();
        }
    }

    struct BloccatoDriver(&'static Bloccato);

    impl Driver for BloccatoDriver {
        fn tables(&mut self) -> Result<Vec<String>, SqlError> {
            Ok(Vec::new())
        }
        fn columns(&mut self, _table: &str) -> Result<Vec<ColumnInfo>, SqlError> {
            Ok(Vec::new())
        }
        fn query_stream(
            &mut self,
            _sql: &str,
            sink: &mut dyn lume_sql::QuerySink,
        ) -> Result<Duration, SqlError> {
            let gate = self.0;
            gate.in_corso.store(true, Ordering::SeqCst);
            sink.started(Vec::new())?;
            gate.started.store(true, Ordering::SeqCst);
            let guard = gate.flag.lock().unwrap();
            let (_guard, esito) = gate
                .wake
                .wait_timeout_while(guard, Duration::from_secs(30), |interrotto| !*interrotto)
                .unwrap();
            gate.in_corso.store(false, Ordering::SeqCst);
            if esito.timed_out() {
                Err(SqlError::Query("non interrotta entro 30 s".into()))
            } else {
                Err(SqlError::Query(lume_sql::QUERY_INTERROTTA.into()))
            }
        }
        fn interrupter(&self) -> Option<Interrupter> {
            let gate = self.0;
            Some(Interrupter::new(move || gate.interrupt()))
        }
    }

    static SCARTA: Bloccato = Bloccato::new();
    static LIBERA: Bloccato = Bloccato::new();
    static RIPETI: Bloccato = Bloccato::new();

    fn scarta_factory(
        _profile: &ConnectionProfile,
        _secret: &Secret,
    ) -> Result<Box<dyn Driver>, SqlError> {
        Ok(Box::new(BloccatoDriver(&SCARTA)))
    }

    fn libera_factory(
        _profile: &ConnectionProfile,
        _secret: &Secret,
    ) -> Result<Box<dyn Driver>, SqlError> {
        Ok(Box::new(BloccatoDriver(&LIBERA)))
    }

    fn ripeti_factory(
        _profile: &ConnectionProfile,
        _secret: &Secret,
    ) -> Result<Box<dyn Driver>, SqlError> {
        Ok(Box::new(BloccatoDriver(&RIPETI)))
    }

    /// Un workspace con una query lanciata e il primo riquadro che la aspetta,
    /// come dopo un `Run`. Ritorna il canale degli eventi.
    fn con_query_in_corso(
        factory: lume_sql::DriverFactory,
        gate: &'static Bloccato,
    ) -> (
        SqlWorkspace,
        std::sync::mpsc::Receiver<lume_sql::QueryEvent>,
    ) {
        let mut workspace = SqlWorkspace::new();
        workspace
            .open(profile("a"), Secret::new("s".into()), factory)
            .unwrap();
        let (receiver, generation) = workspace
            .submit_query_stream("a", "select lento".into())
            .unwrap();
        let pane = workspace.pane_mut(0).unwrap();
        pane.connection = Some("a".into());
        pane.query_gen = generation;
        pane.status = PaneStatus::Running;
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(5)),
            Ok(lume_sql::QueryEvent::Started { .. })
        ));
        assert!(
            (0..1000).any(|_| {
                std::thread::sleep(Duration::from_millis(5));
                gate.started.load(Ordering::SeqCst)
            }),
            "la query doveva partire"
        );
        (workspace, receiver)
    }

    fn si_e_chiusa_interrotta(receiver: &std::sync::mpsc::Receiver<lume_sql::QueryEvent>) {
        match receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(lume_sql::QueryEvent::Failed(testo)) => {
                assert!(testo.contains(lume_sql::QUERY_INTERROTTA), "{testo}");
            }
            altro => panic!("la query doveva chiudersi «interrotta su richiesta»: {altro:?}"),
        }
    }

    #[test]
    fn scartare_una_query_in_corso_la_interrompe_sul_server() {
        let (mut workspace, receiver) = con_query_in_corso(scarta_factory, &SCARTA);

        workspace.discard_running();

        si_e_chiusa_interrotta(&receiver);
        assert_eq!(SCARTA.interrotto.load(Ordering::SeqCst), 1);
        assert_eq!(workspace.active_panes()[0].status, PaneStatus::Idle);
        assert_eq!(workspace.active_panes()[0].query_gen, NO_QUERY);
    }

    #[test]
    fn liberare_i_risultati_interrompe_anche_la_query_in_corso() {
        let (mut workspace, receiver) = con_query_in_corso(libera_factory, &LIBERA);

        workspace.release_results();

        si_e_chiusa_interrotta(&receiver);
        assert_eq!(LIBERA.interrotto.load(Ordering::SeqCst), 1);
        assert_eq!(workspace.active_panes()[0].status, PaneStatus::Idle);
    }

    /// Idempotenza: scarta, libera (ibernazione) e scarta di nuovo sul
    /// riquadro già `Idle` non interrompono niente una seconda volta e la
    /// connessione, riconnessa dal worker, serve ancora le richieste.
    #[test]
    fn scartare_e_liberare_a_ripetizione_interrompe_una_volta_sola() {
        let (mut workspace, receiver) = con_query_in_corso(ripeti_factory, &RIPETI);

        workspace.discard_running();
        si_e_chiusa_interrotta(&receiver);
        workspace.release_results();
        workspace.discard_running();
        workspace.release_results();

        assert_eq!(
            RIPETI.interrotto.load(Ordering::SeqCst),
            1,
            "il pannello era già Idle: niente seconda interruzione"
        );
        assert_eq!(workspace.active_panes()[0].status, PaneStatus::Idle);
        assert_eq!(workspace.active_panes()[0].query_gen, NO_QUERY);

        let (tables, _generation) = workspace.submit("a", Request::Tables).unwrap();
        assert!(
            futures::executor::block_on(tables).unwrap().is_ok(),
            "dopo l'interruzione la connessione si ricrea e risponde"
        );
    }
}
