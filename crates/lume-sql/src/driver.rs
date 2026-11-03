//! Il contratto dei motori. **Sincrono di proposito**: il driver Oracle
//! ufficiale (`oracledb`) espone `pub fn connect(...) -> Result<...>`, non
//! `async fn`, e una chiamata bloccante dentro un `Task::perform` di iced
//! parcheggerebbe un worker dell'executor. Chi è async (`tiberius-ng`) si
//! adatta ospitando un runtime `current_thread` dentro il proprio thread:
//! nessun pool, costo nullo da fermo, e viene droppato insieme al thread —
//! cioè è esso stesso il teardown dell'ibernazione.

use std::fmt;
use std::mem::size_of;
use std::time::Duration;

/// Limiti di un trasferimento dal worker alla UI. Una riga singola più grande
/// è lecita e forma da sola un batch: spezzarla cambierebbe il risultato SQL.
pub const MAX_BATCH_ROWS: usize = 256;
pub const MAX_BATCH_ACCOUNTED_BYTES: usize = 1024 * 1024;

/// Motori realmente supportati. Non è l'elenco del prototipo: sono i due che
/// l'utente usa ogni giorno.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Engine {
    AzureSql,
    Oracle,
}

impl Engine {
    /// Nome mostrato nella barra di stato della console («dialetto»).
    pub const fn dialect(self) -> &'static str {
        match self {
            Self::AzureSql => "SQL Server",
            Self::Oracle => "Oracle",
        }
    }

    /// Come si limita il numero di righe: la sintassi non coincide fra i due.
    pub fn select_top(self, table: &str, limit: usize) -> String {
        match self {
            Self::AzureSql => format!("SELECT TOP {limit} *\nFROM {table};"),
            Self::Oracle => format!("SELECT *\nFROM {table}\nFETCH FIRST {limit} ROWS ONLY;"),
        }
    }
}

/// Parte **non segreta** del profilo: questa sì che va in `lume-store`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionProfile {
    pub id: String,
    /// Etichetta del prototipo: «azure · prod».
    pub label: String,
    pub engine: Engine,
    pub host: String,
    pub database: String,
    pub auth: AuthKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthKind {
    /// Login SQL classico: utente + password.
    SqlLogin { user: String },
    /// Service principal Entra: il segreto è il client secret.
    ServicePrincipal {
        tenant_id: String,
        client_id: String,
    },
}

/// Password, client secret o token. Non implementa `Display`, e `Debug` non
/// lo mostra: un segreto non deve poter finire in un log per distrazione.
///
/// Alla `Drop` viene azzerata **tutta la capacità** del buffer (lo fa
/// `zeroize`, con scritture volatili che il compilatore non può eliminare),
/// non solo la parte in uso. Resta una barriera contro il segreto che
/// sopravvive nella memoria liberata, **non** una garanzia crittografica: un
/// `realloc` avvenuto prima può aver lasciato una copia altrove, e le
/// librerie che ricevono il segreto (`tiberius-ng`, `oracledb`, il magazzino
/// del sistema) ne fanno copie proprie.
pub struct Secret(zeroize::Zeroizing<Vec<u8>>);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(zeroize::Zeroizing::new(value.into_bytes()))
    }

    pub fn expose(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(…)")
    }
}

#[derive(Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
}

/// Debug è diagnostica, non il contenuto della griglia: anche numeri e nomi
/// possono essere dati privati. Display resta esplicito per la UI, non i log.
impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("Null"),
            Self::Bool(_) => f.write_str("Bool(…)"),
            Self::Int(_) => f.write_str("Int(…)"),
            Self::Float(_) => f.write_str("Float(…)"),
            Self::Text(text) => f.debug_struct("Text").field("byte", &text.len()).finish(),
            Self::Bytes(bytes) => f.debug_struct("Bytes").field("byte", &bytes.len()).finish(),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("NULL"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Text(v) => f.write_str(v),
            Self::Bytes(v) => write!(f, "0x{} byte", v.len()),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,
}

impl fmt::Debug for ColumnInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ColumnInfo")
            .field("nome_byte", &self.name.len())
            .field("tipo_byte", &self.type_name.len())
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct ResultSet {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Value>>,
    pub elapsed: Duration,
}

impl fmt::Debug for ResultSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResultSet")
            .field("colonne", &self.columns.len())
            .field("righe", &self.rows.len())
            .field("elapsed", &self.elapsed)
            .finish()
    }
}

impl ResultSet {
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Byte direttamente posseduti dal result set, incluse capacità dei
    /// contenitori e payload testo/BLOB. Non è la RAM totale del processo.
    pub fn accounted_bytes(&self) -> usize {
        accounted_columns_bytes(&self.columns, self.columns.capacity())
            + self.rows.capacity() * size_of::<Vec<Value>>()
            + self
                .rows
                .iter()
                .map(|row| accounted_row_bytes(row))
                .sum::<usize>()
    }
}

/// Byte posseduti dalla lista di metadati della griglia, comprese capacità e
/// stringhe. Viene conteggiata sia nel risultato visibile sia nel budget del
/// worker, che può quindi sospendere anche una query senza righe ma con uno
/// schema insolitamente grande.
pub fn accounted_columns_bytes(columns: &[ColumnInfo], capacity: usize) -> usize {
    capacity * size_of::<ColumnInfo>()
        + columns
            .iter()
            .map(ColumnInfo::accounted_bytes)
            .sum::<usize>()
}

impl ColumnInfo {
    pub fn accounted_bytes(&self) -> usize {
        self.name.capacity() + self.type_name.capacity()
    }
}

pub fn accounted_row_bytes(row: &[Value]) -> usize {
    std::mem::size_of_val(row) + row.iter().map(Value::accounted_bytes).sum::<usize>()
}

impl Value {
    fn accounted_bytes(&self) -> usize {
        match self {
            Self::Text(text) => text.capacity(),
            Self::Bytes(bytes) => bytes.capacity(),
            Self::Null | Self::Bool(_) | Self::Int(_) | Self::Float(_) => 0,
        }
    }
}

/// Un batch trasferibile. Il costruttore non rompe mai una riga per forzare
/// il budget: al massimo il batch contiene un unico record enorme.
#[derive(Clone, PartialEq)]
pub struct QueryBatch {
    pub rows: Vec<Vec<Value>>,
    pub accounted_bytes: usize,
}

impl fmt::Debug for QueryBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryBatch")
            .field("righe", &self.rows.len())
            .field("byte_contabilizzati", &self.accounted_bytes)
            .finish()
    }
}

impl QueryBatch {
    pub fn new(rows: Vec<Vec<Value>>) -> Self {
        let accounted_bytes = rows.capacity() * size_of::<Vec<Value>>()
            + rows
                .iter()
                .map(|row| accounted_row_bytes(row))
                .sum::<usize>();
        Self {
            rows,
            accounted_bytes,
        }
    }

    pub fn can_accept(&self, row: &[Value]) -> bool {
        self.rows.len() < MAX_BATCH_ROWS
            && (self.rows.is_empty()
                || self
                    .accounted_bytes
                    .saturating_add(accounted_row_bytes(row))
                    <= MAX_BATCH_ACCOUNTED_BYTES)
    }
}

/// Trasforma righe già prodotte dal driver in batch con il contratto della
/// UI. I driver streaming lo chiameranno mentre leggono il cursore; la
/// fixture esercita gli stessi limiti senza una rete.
pub fn batch_rows(rows: impl IntoIterator<Item = Vec<Value>>) -> Vec<QueryBatch> {
    let mut batches = Vec::new();
    let mut pending = Vec::new();
    let mut pending_bytes = 0usize;
    for row in rows {
        let row_bytes = accounted_row_bytes(&row);
        if pending.len() >= MAX_BATCH_ROWS
            || (!pending.is_empty()
                && pending_bytes.saturating_add(row_bytes) > MAX_BATCH_ACCOUNTED_BYTES)
        {
            batches.push(QueryBatch::new(pending));
            pending = Vec::new();
            pending_bytes = 0;
        }
        pending_bytes = pending_bytes.saturating_add(row_bytes);
        pending.push(row);
    }
    if !pending.is_empty() {
        batches.push(QueryBatch::new(pending));
    }
    batches
}

#[derive(thiserror::Error)]
pub enum SqlError {
    #[error("connessione fallita: {0}")]
    Connect(String),
    #[error("autenticazione fallita: {0}")]
    Auth(String),
    #[error("query fallita: {0}")]
    Query(String),
    /// Una richiesta più recente ha sostituito questa prima che partisse.
    #[error("richiesta superata")]
    Superseded,
    #[error("connessione non disponibile: {0}")]
    Unavailable(String),
}

/// Un errore remoto può ripetere SQL o parametri. Il Debug non li divulga;
/// il Display per l'utente resta invariato e non è un formatter sicuro per log.
impl fmt::Debug for SqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (tipo, messaggio) = match self {
            Self::Connect(message) => ("Connect", message),
            Self::Auth(message) => ("Auth", message),
            Self::Query(message) => ("Query", message),
            Self::Unavailable(message) => ("Unavailable", message),
            Self::Superseded => return f.write_str("Superseded"),
        };
        f.debug_struct(tipo)
            .field("dettagli_byte", &messaggio.len())
            .finish()
    }
}

/// Testo dell'errore con cui una query si chiude quando la si interrompe.
/// Il worker e l'albero SQL lo mostrano com'è: dice che è stata una scelta,
/// non un guasto.
pub const QUERY_INTERROTTA: &str = "interrotta su richiesta";

/// Il mezzo per interrompere la query **in corso** di un driver da un altro
/// thread. Il driver vive tutto dentro il thread del suo worker e non è
/// `Sync`: quel che si può condividere è solo questo richiamo, che non
/// blocca e non tocca lo stato del driver (per Azure sveglia un `Notify`).
#[derive(Clone)]
pub struct Interrupter(std::sync::Arc<dyn Fn() + Send + Sync>);

impl Interrupter {
    pub fn new(richiamo: impl Fn() + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(richiamo))
    }

    /// Chiede l'interruzione. Se non c'è una query in corso non fa nulla.
    pub fn interrupt(&self) {
        (self.0)();
    }
}

impl std::fmt::Debug for Interrupter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Interrupter")
    }
}

/// Un motore aperto. Vive **solo** dentro il thread del suo worker: nessun
/// metodo è chiamato dal thread della UI.
pub trait Driver: Send {
    fn tables(&mut self) -> Result<Vec<String>, SqlError>;
    fn columns(&mut self, table: &str) -> Result<Vec<ColumnInfo>, SqlError>;

    /// Produce il risultato a incrementi leggendo il cursore e consegnando
    /// i batch al `sink`.
    fn query_stream(&mut self, sql: &str, sink: &mut dyn QuerySink) -> Result<Duration, SqlError>;

    /// Il mezzo per interrompere la query in corso da un altro thread, se il
    /// motore ne ha uno. `None` (il default) vuol dire «non so interrompere»:
    /// il chiamante ripiega su ciò che ha (chiudere la connessione).
    fn interrupter(&self) -> Option<Interrupter> {
        None
    }
}

/// Consumer sincrono dei batch di un cursore. Restare sincroni permette ai
/// driver Oracle e SQL Server di fermarsi sul canale bounded senza creare un
/// pool o polling aggiuntivo.
pub trait QuerySink {
    fn started(&mut self, columns: Vec<ColumnInfo>) -> Result<QueryFlow, SqlError>;
    /// Inizia un altro insieme di risultati della query. Driver single-set
    /// non lo chiamano; il sink predefinito resta compatibile col primo.
    fn result_set_started(
        &mut self,
        _index: usize,
        columns: Vec<ColumnInfo>,
    ) -> Result<QueryFlow, SqlError> {
        self.started(columns)
    }
    fn batch(&mut self, batch: QueryBatch) -> Result<QueryFlow, SqlError>;
}

/// Consegna un risultato già in memoria a un `QuerySink`, a batch. È il modo
/// in cui un driver senza cursore (per esempio un doppione di prova)
/// implementa `query_stream`; i driver nativi leggono il cursore.
pub fn stream_result_set(
    result: ResultSet,
    sink: &mut dyn QuerySink,
) -> Result<Duration, SqlError> {
    if sink.started(result.columns)? == QueryFlow::Stop {
        return Ok(result.elapsed);
    }
    for batch in batch_rows(result.rows) {
        if sink.batch(batch)? == QueryFlow::Stop {
            break;
        }
    }
    Ok(result.elapsed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryFlow {
    Continue,
    Stop,
}

/// Come si apre un motore. Un puntatore a funzione invece di un metodo su
/// `Driver` perché la connessione deve avvenire **dentro** il worker (blocca)
/// e perché il registro dei motori resta senza generici.
pub type DriverFactory = fn(&ConnectionProfile, &Secret) -> Result<Box<dyn Driver>, SqlError>;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn un_interrupter_clonato_chiama_lo_stesso_richiamo() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let chiamate = std::sync::Arc::new(AtomicUsize::new(0));
        let contatore = std::sync::Arc::clone(&chiamate);
        let interrupter = Interrupter::new(move || {
            contatore.fetch_add(1, Ordering::SeqCst);
        });
        let copia = interrupter.clone();
        interrupter.interrupt();
        copia.interrupt();
        assert_eq!(chiamate.load(Ordering::SeqCst), 2);
        assert_eq!(format!("{interrupter:?}"), "Interrupter");
    }

    /// Un driver che non sa interrompere (Oracle, i doppi di ciclo di vita)
    /// non promette niente: il chiamante deve poterlo distinguere.
    #[test]
    fn il_driver_di_default_non_ha_un_interrupter() {
        struct Muto;
        impl Driver for Muto {
            fn tables(&mut self) -> Result<Vec<String>, SqlError> {
                Ok(Vec::new())
            }
            fn columns(&mut self, _: &str) -> Result<Vec<ColumnInfo>, SqlError> {
                Ok(Vec::new())
            }
            fn query_stream(
                &mut self,
                _: &str,
                _: &mut dyn QuerySink,
            ) -> Result<Duration, SqlError> {
                Err(SqlError::Unavailable("non usato".into()))
            }
        }
        assert!(Muto.interrupter().is_none());
    }

    #[test]
    fn stream_result_set_consegna_colonne_batch_e_tempo() {
        #[derive(Default)]
        struct Sink {
            columns: Vec<ColumnInfo>,
            batches: Vec<QueryBatch>,
        }
        impl QuerySink for Sink {
            fn started(&mut self, columns: Vec<ColumnInfo>) -> Result<QueryFlow, SqlError> {
                self.columns = columns;
                Ok(QueryFlow::Continue)
            }
            fn batch(&mut self, batch: QueryBatch) -> Result<QueryFlow, SqlError> {
                self.batches.push(batch);
                Ok(QueryFlow::Continue)
            }
        }

        let result = ResultSet {
            columns: vec![ColumnInfo {
                name: "nome".into(),
                type_name: "TEXT".into(),
            }],
            rows: vec![vec![Value::Text("riga".into())]],
            elapsed: Duration::from_millis(7),
        };
        let mut sink = Sink::default();
        assert_eq!(
            stream_result_set(result, &mut sink).unwrap(),
            Duration::from_millis(7)
        );
        assert_eq!(sink.columns.len(), 1);
        assert_eq!(sink.batches.len(), 1);
        assert_eq!(sink.batches[0].rows, vec![vec![Value::Text("riga".into())]]);
    }

    #[test]
    fn il_limite_di_righe_usa_la_sintassi_del_motore() {
        assert_eq!(
            Engine::AzureSql.select_top("dbo.users", 500),
            "SELECT TOP 500 *\nFROM dbo.users;"
        );
        assert_eq!(
            Engine::Oracle.select_top("USERS", 500),
            "SELECT *\nFROM USERS\nFETCH FIRST 500 ROWS ONLY;"
        );
    }

    #[test]
    fn il_segreto_non_finisce_nel_debug() {
        let secret = Secret::new("p4ssw0rd".into());
        assert_eq!(secret.expose(), "p4ssw0rd");
        assert_eq!(format!("{secret:?}"), "Secret(…)");
    }

    #[test]
    fn il_batch_non_supera_i_limiti_ma_accetta_una_riga_enorme_da_sola() {
        let huge = vec![Value::Bytes(vec![0; MAX_BATCH_ACCOUNTED_BYTES + 1])];
        let empty = QueryBatch::new(Vec::new());
        assert!(empty.can_accept(&huge));
        let batch = QueryBatch::new(vec![huge]);
        assert!(!batch.can_accept(&[Value::Int(1)]));

        let rows = vec![vec![Value::Int(1)]; MAX_BATCH_ROWS];
        let batch = QueryBatch::new(rows);
        assert!(!batch.can_accept(&[Value::Int(2)]));
    }

    #[test]
    fn le_righe_vengono_divise_senza_duplicazioni() {
        let rows: Vec<Vec<Value>> = (0..(MAX_BATCH_ROWS + 1))
            .map(|index| vec![Value::Int(index as i64)])
            .collect();
        let batches = batch_rows(rows);
        assert_eq!(
            batches
                .iter()
                .map(|batch| batch.rows.len())
                .collect::<Vec<_>>(),
            vec![MAX_BATCH_ROWS, 1]
        );
        assert_eq!(batches[1].rows[0], vec![Value::Int(MAX_BATCH_ROWS as i64)]);
    }

    #[test]
    #[allow(unsafe_code)]
    fn l_azzeramento_copre_tutta_la_capacita_e_non_solo_la_lunghezza() {
        use zeroize::Zeroize;

        let mut testo = String::with_capacity(128);
        testo.push_str("segretissimo");
        let mut secret = Secret::new(testo);
        // Questa riga compila solo se il contenuto azzera da solo a `Drop`
        // (`Zeroizing<Vec<u8>>`): è la parte del test che fallisce senza il
        // cambiamento del task. Il resto prova che cosa fa `Vec::zeroize`.
        fn azzera_da_solo<T: zeroize::ZeroizeOnDrop>(_: &T) {}
        azzera_da_solo(&secret.0);
        let capacita = secret.0.capacity();
        assert!(capacita >= 128, "la capacità di partenza si conserva");

        secret.0.zeroize();

        assert!(secret.is_empty(), "azzerare svuota anche il contenitore");
        // SAFETY: `Vec::zeroize` ha scritto zeri su tutta la capacità
        // (lo dice la sua documentazione), quindi i primi `capacita` byte
        // sono inizializzati e `u8` non ha requisiti di validità.
        unsafe { secret.0.set_len(capacita) };
        assert!(
            secret.0.iter().all(|byte| *byte == 0),
            "nessun byte della vecchia password deve restare nella capacità"
        );
        secret.0.clear();
    }

    #[test]
    fn un_segreto_vuoto_e_vuoto_e_non_si_stampa() {
        let secret = Secret::new(String::new());
        assert!(secret.is_empty());
        assert_eq!(secret.expose(), "");
        assert_eq!(format!("{secret:?}"), "Secret(…)");
    }
}
