//! Driver Azure SQL / SQL Server, su `tiberius-ng`.
//!
//! `tiberius-ng` è **asincrono**, il trait `Driver` è sincrono: la cucitura è
//! un runtime tokio `current_thread` **dentro questo driver**, quindi dentro
//! il thread del worker. Non è un secondo pool — un runtime current_thread da
//! fermo non ha thread propri — e soprattutto **viene droppato insieme al
//! driver**: è esso stesso il teardown dell'ibernazione, non un pezzo di
//! codice da ricordarsi di chiamare.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

#[cfg(test)]
use futures::FutureExt;
use futures::TryStreamExt;
use tiberius::{AuthMethod, Client, ColumnData, Config, EncryptionLevel, QueryItem, Row};
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio::sync::Notify;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use crate::driver::{
    AuthKind, ColumnInfo, ConnectionProfile, Driver, Interrupter, QUERY_INTERROTTA, QueryBatch,
    QueryFlow, QuerySink, ResultSet, Secret, SqlError, Value, accounted_row_bytes,
};

/// Ogni fallimento di login, qualunque ne sia il motivo, SQL Server lo
/// collassa in **18456** («Login failed for user»): lo stato dettagliato
/// resta solo nel log del server e al client non arriva. Quindi questo codice
/// da solo è la condizione giusta, e anche l'unica disponibile.
///
/// 4060 («Cannot open database … requested by the login») **non** è qui
/// apposta: lì la password è buona ed è il database a non andare, quindi
/// buttarla via dalla cassaforte farebbe ridigitare una password corretta.
const LOGIN_FAILED: u32 = 18456;

/// Distingue «credenziali rifiutate» da «non sono arrivato al server». È la
/// sola distinzione che serve fuori di qui: un segreto rifiutato va tolto
/// dalla cassaforte, un host irraggiungibile no.
fn connect_error(error: tiberius::error::Error) -> SqlError {
    match &error {
        tiberius::error::Error::Server(token) if token.code() == LOGIN_FAILED => {
            SqlError::Auth(error.to_string())
        }
        _ => SqlError::Connect(error.to_string()),
    }
}

/// Porta di default di SQL Server. Azure SQL non ne usa altre, ma un'istanza
/// locale sì, quindi `host` può portarsela dietro.
const DEFAULT_PORT: u16 = 1433;

/// Quanto si aspetta ciascuna fase della connessione. Il thread del worker è
/// dedicato e l'ibernazione lo aspetta: senza tetti, un host che scarta i
/// pacchetti o un server che tace lo terrebbe bloccato.
#[derive(Clone, Copy)]
struct Attese {
    /// Connessione TCP.
    tcp: Duration,
    /// Prelogin, TLS e login TDS, cioè `Client::connect`.
    handshake: Duration,
}

const ATTESE: Attese = Attese {
    tcp: Duration::from_secs(15),
    handshake: Duration::from_secs(30),
};

/// Dà un tetto a un'attesa di rete. Allo scadere l'errore dice **cosa**
/// aspettava: «connessione TCP» e «handshake» sono cose diverse per chi deve
/// capire se è la rete o il server.
async fn entro<T>(
    limite: Duration,
    cosa: &str,
    futuro: impl std::future::Future<Output = Result<T, SqlError>>,
) -> Result<T, SqlError> {
    match tokio::time::timeout(limite, futuro).await {
        Ok(esito) => esito,
        Err(_) => Err(SqlError::Connect(format!(
            "{cosa}: nessuna risposta entro {limite:?}"
        ))),
    }
}

/// Quanto si aspetta la conferma `DONE_ATTN` del server dopo l'Attention.
///
/// Se la conferma non arriva (`Conferma::Rifiutata` o `Conferma::Scaduta`,
/// cioè l'errore contiene «non ha confermato») lo stato TDS della connessione
/// è ambiguo: questo driver **non** si scarta da solo. Spetta al chiamante
/// (il worker, Task 12) scartare e ricreare il driver in quel caso.
const ATTESA_ANNULLAMENTO: Duration = Duration::from_secs(5);

/// Il lato «condiviso» dell'interruzione: dietro un `Arc`, lo tengono il
/// driver (nel thread del worker) e l'`Interrupter` (che la UI può azionare).
struct Attention {
    stato: Mutex<StatoAttention>,
    // Segnale di risveglio, NON autorizzazione a interrompere: una wake
    // della query precedente può arrivare dopo l'armamento di quella nuova.
    notify: Notify,
}

#[derive(Default)]
struct StatoAttention {
    attivo: bool,
    richiesta: bool,
}

impl Attention {
    fn new() -> Self {
        Self {
            stato: Mutex::new(StatoAttention::default()),
            notify: Notify::new(),
        }
    }

    fn stato(&self) -> MutexGuard<'_, StatoAttention> {
        self.stato.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Armamento e cancellazione logica del residuo sono un solo passaggio;
    /// nessun drain dopo active=true che possa perdere una richiesta nuova.
    fn inizia(&self) {
        self.inizia_con(|| {});
    }

    // Sincronizzazione controllata delle fasi reali, non un driver/server
    // finto. Callback normale vuota; callback e Notify fuori dal mutex.
    fn inizia_con(&self, dopo_armamento: impl FnOnce()) {
        {
            let mut stato = self.stato();
            stato.attivo = true;
            stato.richiesta = false;
        }
        dopo_armamento();
    }

    fn finisci(&self) {
        let mut stato = self.stato();
        stato.attivo = false;
        stato.richiesta = false;
    }

    /// Registra la richiesta prima della wake. Più richieste già pending
    /// coalescono; finisci/inizia revocano logicamente la richiesta vecchia.
    fn chiedi(&self) {
        self.chiedi_con(|| {});
    }

    fn chiedi_con(&self, prima_notifica: impl FnOnce()) {
        let notifica = {
            let mut stato = self.stato();
            if stato.attivo {
                stato.richiesta = true;
                true
            } else {
                false
            }
        };
        if notifica {
            prima_notifica();
            self.notify.notify_one();
        }
    }

    async fn attesa_richiesta(&self) {
        loop {
            let risveglio = self.notify.notified();
            tokio::pin!(risveglio);
            // Registrazione PRIMA del controllo per evitare lost wake.
            risveglio.as_mut().enable();
            {
                let mut stato = self.stato();
                if stato.attivo && stato.richiesta {
                    stato.richiesta = false;
                    return;
                }
            }
            // Una notifica tardiva senza richiesta attuale è solo wake:
            // ricontrolla e torna ad attendere senza timer/poll da inattivo.
            risveglio.await;
        }
    }
}

/// Esegue `lavoro` finché non arriva l'interruzione. `None` = interrotto: il
/// futuro `lavoro` viene lasciato cadere qui, e con lui il prestito del
/// client (serve, perché `cancel_query` prende il client in `&mut`).
async fn fino_a_interruzione<T>(
    attention: &Attention,
    lavoro: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        esito = lavoro => Some(esito),
        () = attention.attesa_richiesta() => None,
    }
}

/// Com'è andata la richiesta di annullamento al server.
enum Conferma {
    Ricevuta,
    Rifiutata(String),
    Scaduta,
}

fn errore_di_interruzione(conferma: Conferma) -> SqlError {
    match conferma {
        Conferma::Ricevuta => SqlError::Query(QUERY_INTERROTTA.to_owned()),
        Conferma::Rifiutata(motivo) => SqlError::Query(format!(
            "{QUERY_INTERROTTA}; il server non ha confermato: {motivo}"
        )),
        Conferma::Scaduta => SqlError::Query(format!(
            "{QUERY_INTERROTTA}; il server non ha confermato entro {} s",
            ATTESA_ANNULLAMENTO.as_secs()
        )),
    }
}

/// Manda l'Attention TDS e aspetta la conferma, al massimo
/// `ATTESA_ANNULLAMENTO`. Va chiamata **dopo** aver lasciato cadere lo stream
/// della query, cioè dopo `fino_a_interruzione`.
async fn annulla_sul_server(client: &mut Client<Compat<TcpStream>>) -> SqlError {
    let conferma = match tokio::time::timeout(ATTESA_ANNULLAMENTO, client.cancel_query()).await {
        Ok(Ok(())) => Conferma::Ricevuta,
        Ok(Err(errore)) => Conferma::Rifiutata(errore.to_string()),
        Err(_) => Conferma::Scaduta,
    };
    errore_di_interruzione(conferma)
}

pub struct AzureDriver {
    runtime: Runtime,
    client: Client<Compat<TcpStream>>,
    attention: Arc<Attention>,
}

impl AzureDriver {
    pub fn connect(profile: &ConnectionProfile, secret: &Secret) -> Result<Self, SqlError> {
        Self::connect_con(profile, secret, ATTESE)
    }

    fn connect_con(
        profile: &ConnectionProfile,
        secret: &Secret,
        attese: Attese,
    ) -> Result<Self, SqlError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| SqlError::Connect(error.to_string()))?;
        let client = runtime.block_on(open(profile, secret, attese))?;
        Ok(Self {
            runtime,
            client,
            attention: Arc::new(Attention::new()),
        })
    }

    fn fetch(&mut self, sql: &str) -> Result<ResultSet, SqlError> {
        let AzureDriver {
            runtime, client, ..
        } = self;
        runtime.block_on(run(client, sql))
    }

    fn fetch_stream(
        &mut self,
        sql: &str,
        sink: &mut dyn QuerySink,
    ) -> Result<std::time::Duration, SqlError> {
        let AzureDriver {
            runtime,
            client,
            attention,
        } = self;
        attention.inizia();
        let esito = runtime.block_on(fino_a_interruzione(
            attention.as_ref(),
            run_stream(client, sql, sink),
        ));
        // Prima dell'Attention: da qui una nuova richiesta riguarda la query
        // successiva, non questa.
        attention.finisci();
        match esito {
            Some(esito) => esito,
            None => Err(runtime.block_on(annulla_sul_server(client))),
        }
    }
}

async fn open(
    profile: &ConnectionProfile,
    secret: &Secret,
    attese: Attese,
) -> Result<Client<Compat<TcpStream>>, SqlError> {
    let (host, port) = split_host(&profile.host);
    let mut config = Config::new();
    config.host(host);
    config.port(port);
    config.database(&profile.database);
    // Azure SQL **impone** il TLS. Nessun `trust_cert`: i suoi certificati
    // sono validi, e accettarli alla cieca vanificherebbe la cifratura.
    // Le radici sono quelle del sistema operativo, come per reqwest (patch su
    // tiberius-ng con rustls-platform-verifier): una CA aziendale installata
    // nel sistema vale anche qui.
    config.encryption(EncryptionLevel::Required);
    config.application_name("Lume");
    config.authentication(match &profile.auth {
        AuthKind::SqlLogin { user } => AuthMethod::sql_server(user, secret.expose()),
        AuthKind::ServicePrincipal {
            tenant_id,
            client_id,
        } => {
            // Il token vive solo il tempo di costruire `AuthMethod`, che ne fa
            // una copia propria in una `String` non azzerata (limite noto).
            let token = crate::entra::access_token(tenant_id, client_id, secret).await?;
            AuthMethod::aad_token(token.expose())
        }
    });

    let tcp = entro(attese.tcp, "connessione TCP", async {
        TcpStream::connect(config.get_addr())
            .await
            .map_err(|error| SqlError::Connect(error.to_string()))
    })
    .await?;
    // TDS fa molti scambi piccoli: senza questo Nagle aggiunge latenza a ogni
    // andata e ritorno.
    tcp.set_nodelay(true)
        .map_err(|error| SqlError::Connect(error.to_string()))?;
    // L'unico punto in cui il server può rifiutare le credenziali: prima di
    // qui c'è solo la rete.
    entro(attese.handshake, "handshake con il server", async {
        Client::connect(config, tcp.compat_write())
            .await
            .map_err(connect_error)
    })
    .await
}

/// Esegue e raccoglie il primo result set per le letture interne del catalogo.
///
/// Si legge lo stream a mano invece di usare `into_first_result()` perché
/// quello butta via i metadati: una query che torna zero righe mostrerebbe
/// una griglia senza nemmeno le intestazioni, che sembra un guasto.
async fn run(client: &mut Client<Compat<TcpStream>>, sql: &str) -> Result<ResultSet, SqlError> {
    let started = std::time::Instant::now();
    let mut stream = client
        .simple_query(sql.to_owned())
        .await
        .map_err(|error| SqlError::Query(error.to_string()))?;

    let mut columns: Vec<ColumnInfo> = Vec::new();
    let mut rows: Vec<Vec<Value>> = Vec::new();
    while let Some(item) = stream
        .try_next()
        .await
        .map_err(|error| SqlError::Query(error.to_string()))?
    {
        match item {
            QueryItem::Metadata(metadata) if metadata.result_index() == 0 => {
                columns = metadata
                    .columns()
                    .iter()
                    .map(|column| ColumnInfo {
                        name: column.name().to_owned(),
                        type_name: format!("{:?}", column.column_type()).to_lowercase(),
                    })
                    .collect();
            }
            QueryItem::Row(row) if row.result_index() == 0 => rows.push(cells(&row)),
            // Batch con più SELECT: la griglia ne mostra una, ed è la prima.
            QueryItem::Metadata(_) | QueryItem::Row(_) => {}
        }
    }
    Ok(ResultSet {
        columns,
        rows,
        elapsed: started.elapsed(),
    })
}

/// Variante del lettore TDS che mantiene solo il batch in corso. Il `sink`
/// può bloccarsi sulla coda bounded del worker: in quel caso smettiamo di
/// estrarre token dalla rete e non accumuliamo righe nel processo.
async fn run_stream(
    client: &mut Client<Compat<TcpStream>>,
    sql: &str,
    sink: &mut dyn QuerySink,
) -> Result<std::time::Duration, SqlError> {
    let started = std::time::Instant::now();
    let mut stream = client
        .simple_query(sql.to_owned())
        .await
        .map_err(|error| SqlError::Query(error.to_string()))?;
    let mut announced = false;
    let mut active_result_index = 0;
    let mut batch = QueryBatch::new(Vec::new());
    while let Some(item) = stream
        .try_next()
        .await
        .map_err(|error| SqlError::Query(error.to_string()))?
    {
        match item {
            QueryItem::Metadata(metadata) => {
                let result_index = metadata.result_index();
                if announced && !batch.rows.is_empty() {
                    if sink.batch(batch)? == QueryFlow::Stop {
                        return Ok(started.elapsed());
                    }
                    batch = QueryBatch::new(Vec::new());
                }
                let columns = metadata
                    .columns()
                    .iter()
                    .map(|column| ColumnInfo {
                        name: column.name().to_owned(),
                        type_name: format!("{:?}", column.column_type()).to_lowercase(),
                    })
                    .collect();
                announced = true;
                active_result_index = result_index;
                let flow = if result_index == 0 {
                    sink.started(columns)?
                } else {
                    sink.result_set_started(result_index, columns)?
                };
                if flow == QueryFlow::Stop {
                    return Ok(started.elapsed());
                }
            }
            QueryItem::Row(row) if row.result_index() == active_result_index => {
                let cells = cells(&row);
                if !batch.can_accept(&cells) && !batch.rows.is_empty() {
                    if sink.batch(batch)? == QueryFlow::Stop {
                        return Ok(started.elapsed());
                    }
                    batch = QueryBatch::new(Vec::new());
                }
                batch.accounted_bytes = batch
                    .accounted_bytes
                    .saturating_add(accounted_row_bytes(&cells));
                batch.rows.push(cells);
            }
            QueryItem::Row(_) => {}
        }
    }
    if !announced && sink.started(Vec::new())? == QueryFlow::Stop {
        return Ok(started.elapsed());
    }
    if !batch.rows.is_empty() && sink.batch(batch)? == QueryFlow::Stop {
        return Ok(started.elapsed());
    }
    Ok(started.elapsed())
}

impl Driver for AzureDriver {
    fn tables(&mut self) -> Result<Vec<String>, SqlError> {
        let result = self.fetch(
            "SELECT TABLE_SCHEMA + '.' + TABLE_NAME FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_SCHEMA, TABLE_NAME",
        )?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .map(|value| value.to_string())
            .collect())
    }

    fn columns(&mut self, table: &str) -> Result<Vec<ColumnInfo>, SqlError> {
        let (schema, name) = split_table(table);
        let result = self.fetch(&format!(
            "SELECT COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {} ORDER BY ORDINAL_POSITION",
            quote(schema),
            quote(name)
        ))?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|row| {
                let mut cells = row.into_iter();
                Some(ColumnInfo {
                    name: cells.next()?.to_string(),
                    type_name: cells.next()?.to_string(),
                })
            })
            .collect())
    }

    fn query_stream(
        &mut self,
        sql: &str,
        sink: &mut dyn QuerySink,
    ) -> Result<std::time::Duration, SqlError> {
        self.fetch_stream(sql, sink)
    }

    fn interrupter(&self) -> Option<Interrupter> {
        let attention = Arc::clone(&self.attention);
        Some(Interrupter::new(move || attention.chiedi()))
    }
}

/// `host` può portarsi dietro la porta: «srv.database.windows.net,1433» è la
/// forma di Azure, «host:1433» quella abituale altrove.
fn split_host(host: &str) -> (&str, u16) {
    let split = host
        .rsplit_once(',')
        .or_else(|| host.rsplit_once(':'))
        .and_then(|(name, port)| port.trim().parse().ok().map(|port| (name, port)));
    match split {
        Some((name, port)) => (name.trim(), port),
        None => (host.trim(), DEFAULT_PORT),
    }
}

/// «dbo.users» → («dbo», «users»); senza schema si assume `dbo`, come fa
/// SQL Server quando lo si omette.
fn split_table(table: &str) -> (&str, &str) {
    match table.split_once('.') {
        Some((schema, name)) => (schema, name),
        None => ("dbo", table),
    }
}

/// `simple_query` non accetta parametri, quindi il nome della tabella entra
/// nel testo: va **citato**, con l'apice raddoppiato come vuole T-SQL.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn cells(row: &Row) -> Vec<Value> {
    row.cells().map(|(_, data)| value(data)).collect()
}

fn value(data: &ColumnData<'static>) -> Value {
    use tiberius::time::chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};

    match data {
        ColumnData::U8(v) => v.map_or(Value::Null, |v| Value::Int(v.into())),
        ColumnData::I16(v) => v.map_or(Value::Null, |v| Value::Int(v.into())),
        ColumnData::I32(v) => v.map_or(Value::Null, |v| Value::Int(v.into())),
        ColumnData::I64(v) => v.map_or(Value::Null, Value::Int),
        ColumnData::F32(v) => v.map_or(Value::Null, |v| Value::Float(v.into())),
        ColumnData::F64(v) => v.map_or(Value::Null, Value::Float),
        ColumnData::Bit(v) => v.map_or(Value::Null, Value::Bool),
        ColumnData::String(v) => v
            .as_ref()
            .map_or(Value::Null, |text| Value::Text(text.to_string())),
        ColumnData::Guid(v) => v.map_or(Value::Null, |uuid| Value::Text(uuid.to_string())),
        ColumnData::Binary(v) => v
            .as_ref()
            .map_or(Value::Null, |bytes| Value::Bytes(bytes.to_vec())),
        // DECIMAL/NUMERIC è a virgola fissa: vedi `numeric_value`.
        ColumnData::Numeric(v) => v.map_or(Value::Null, |numeric| numeric_value(&numeric)),
        ColumnData::Xml(v) => v
            .as_ref()
            .map_or(Value::Null, |xml| Value::Text(xml.to_string())),
        ColumnData::DateTime(_) | ColumnData::SmallDateTime(_) | ColumnData::DateTime2(_) => {
            from_sql::<NaiveDateTime>(data)
        }
        ColumnData::Date(_) => from_sql::<NaiveDate>(data),
        ColumnData::Time(_) => from_sql::<NaiveTime>(data),
        ColumnData::DateTimeOffset(_) => from_sql::<DateTime<Utc>>(data),
    }
}

/// Le date TDS non hanno una rappresentazione testuale propria: si passa dai
/// tipi `chrono` che `tiberius-ng` già espone con la feature `chrono`, senza
/// aggiungere `chrono` fra le nostre dipendenze dirette.
fn from_sql<'a, T>(data: &'a ColumnData<'static>) -> Value
where
    T: tiberius::FromSql<'a> + std::fmt::Display,
{
    match T::from_sql(data) {
        Ok(Some(value)) => Value::Text(value.to_string()),
        Ok(None) => Value::Null,
        Err(error) => Value::Text(format!("<{error}>")),
    }
}

fn numeric_value(numeric: &tiberius::numeric::Numeric) -> Value {
    if numeric.scale() == 0 {
        // Un DECIMAL senza decimali è un intero e va tenuto tale: nella
        // griglia deve ordinare da numero, non da testo. `Display` non serve
        // — stampa comunque un «.0» — e `int_part()` è esatto.
        return match i64::try_from(numeric.int_part()) {
            Ok(value) => Value::Int(value),
            // DECIMAL(38,0) arriva oltre `i64`: la cifra esatta come testo è
            // meglio di un troncamento silenzioso.
            Err(_) => Value::Text(numeric.int_part().to_string()),
        };
    }
    Value::Float(f64::from(*numeric))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn la_porta_si_legge_dalle_due_forme_e_altrimenti_e_la_1433() {
        assert_eq!(
            split_host("srv.database.windows.net"),
            ("srv.database.windows.net", 1433)
        );
        assert_eq!(
            split_host("srv.database.windows.net,1433"),
            ("srv.database.windows.net", 1433)
        );
        assert_eq!(split_host("localhost:14330"), ("localhost", 14330));
        assert_eq!(
            split_host("srv.example.com:non-numerica"),
            ("srv.example.com:non-numerica", 1433),
            "una porta illeggibile non deve far sparire un pezzo di host"
        );
    }

    #[test]
    fn lo_schema_omesso_e_dbo() {
        assert_eq!(split_table("dbo.users"), ("dbo", "users"));
        assert_eq!(split_table("users"), ("dbo", "users"));
    }

    #[test]
    fn lapice_nel_nome_tabella_viene_raddoppiato() {
        assert_eq!(quote("users"), "'users'");
        assert_eq!(
            quote("o'brien'; DROP TABLE x --"),
            "'o''brien''; DROP TABLE x --'",
            "il nome arriva dall'albero, ma l'albero arriva dal database"
        );
    }

    #[test]
    fn i_valori_nulli_restano_nulli_e_i_tipi_base_si_mappano() {
        assert_eq!(value(&ColumnData::I32(None)), Value::Null);
        assert_eq!(value(&ColumnData::I32(Some(-7))), Value::Int(-7));
        assert_eq!(value(&ColumnData::U8(Some(255))), Value::Int(255));
        assert_eq!(value(&ColumnData::Bit(Some(true))), Value::Bool(true));
        assert_eq!(
            value(&ColumnData::String(Some("ciao".into()))),
            Value::Text("ciao".into())
        );
        assert_eq!(
            value(&ColumnData::Binary(Some(vec![1, 2, 3].into()))),
            Value::Bytes(vec![1, 2, 3])
        );
    }

    #[test]
    fn un_decimale_intero_resta_intero_e_ordina_come_tale() {
        // scale 0: e' un intero, e nella griglia deve ordinare da numero.
        assert_eq!(
            numeric_value(&tiberius::numeric::Numeric::new_with_scale(1234, 0)),
            Value::Int(1234)
        );
        assert_eq!(
            numeric_value(&tiberius::numeric::Numeric::new_with_scale(1234, 2)),
            Value::Float(12.34)
        );
    }

    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn un_attesa_senza_risposta_scade_e_nomina_cosa_aspettava() {
        let errore = runtime()
            .block_on(entro(
                Duration::from_millis(50),
                "connessione TCP",
                std::future::pending::<Result<(), SqlError>>(),
            ))
            .expect_err("doveva scadere");
        assert!(
            matches!(&errore, SqlError::Connect(testo) if testo.contains("connessione TCP")),
            "{errore}"
        );
    }

    #[test]
    fn un_esito_arrivato_in_tempo_passa_intatto() {
        let ok = runtime().block_on(entro(Duration::from_secs(5), "connessione TCP", async {
            Ok::<_, SqlError>(7)
        }));
        assert_eq!(ok.unwrap(), 7);
        let errore = runtime()
            .block_on(entro(Duration::from_secs(5), "connessione TCP", async {
                Err::<(), _>(SqlError::Auth("no".to_owned()))
            }))
            .expect_err("l'errore interno non va mascherato");
        assert!(matches!(errore, SqlError::Auth(_)));
    }

    #[test]
    fn handshake_muto_scade() {
        // Un server vero che accetta la connessione e non risponde mai al
        // prelogin: prima di questo task `connect` non tornava.
        let ascolto = TcpListener::bind("127.0.0.1:0").unwrap();
        let porta = ascolto.local_addr().unwrap().port();
        let (fine, attesa_fine) = mpsc::channel::<()>();
        let filo = std::thread::spawn(move || {
            let (flusso, _) = ascolto.accept().unwrap();
            let _ = attesa_fine.recv_timeout(Duration::from_secs(10));
            drop(flusso);
        });
        let profilo = ConnectionProfile {
            id: "prova".to_owned(),
            label: "prova".to_owned(),
            engine: crate::driver::Engine::AzureSql,
            host: format!("127.0.0.1:{porta}"),
            database: "master".to_owned(),
            auth: AuthKind::SqlLogin {
                user: "sa".to_owned(),
            },
        };
        let attese = Attese {
            tcp: Duration::from_secs(1),
            handshake: Duration::from_millis(500),
        };

        let inizio = Instant::now();
        let errore = AzureDriver::connect_con(&profilo, &Secret::new("p".to_owned()), attese)
            .err()
            .expect("un handshake muto deve dare errore");

        assert!(
            inizio.elapsed() < Duration::from_secs(5),
            "{:?}",
            inizio.elapsed()
        );
        assert!(
            matches!(&errore, SqlError::Connect(testo) if testo.contains("handshake")),
            "{errore}"
        );
        fine.send(()).unwrap();
        filo.join().unwrap();
    }

    /// Vero se c'è una richiesta ATTUALE, non soltanto una wake residua.
    /// Future/Notify/stato di produzione sondati una volta senza attesa.
    fn pronto(attention: &Attention) -> bool {
        attention.attesa_richiesta().now_or_never().is_some()
    }

    /// La query è finita (o non è ancora iniziata) quando arriva l'interruzione:
    /// va persa. Altrimenti il click su «Scarta» di una query già chiusa
    /// ucciderebbe quella dopo.
    #[test]
    fn interrompere_una_query_finita_non_fa_nulla() {
        let attention = Attention::new();
        // Mai iniziata: nessuna richiesta logica né wake da chiedi. Non
        // affidarsi al solo permesso Notify: non identifica la query.
        attention.chiedi();
        assert!(!pronto(&attention), "richiesta fuori da una query");
        attention.inizia();
        assert!(!pronto(&attention));
        attention.finisci();
        attention.chiedi();
        assert!(!pronto(&attention), "richiesta dopo la fine della query");
        attention.inizia();
        assert!(!pronto(&attention));
    }

    /// Il doppio clic su «Scarta» è un'interruzione sola, non due: la seconda
    /// non deve restare in coda e uccidere la query successiva.
    #[test]
    fn due_interruzioni_sono_una() {
        let attention = Attention::new();
        attention.inizia();
        attention.chiedi();
        attention.chiedi();
        assert!(pronto(&attention), "la prima deve arrivare");
        assert!(!pronto(&attention), "la seconda non deve accumularsi");
    }

    /// L'interruzione arriva un istante prima che la query finisca da sola:
    /// il permesso resta nel `Notify`. La query dopo deve partire pulita.
    #[test]
    fn un_permesso_residuo_non_interrompe_la_query_dopo() {
        let attention = Attention::new();
        attention.inizia();
        attention.chiedi();
        attention.finisci();
        attention.inizia();
        assert!(!pronto(&attention));
    }

    /// Un lavoro che non finisce mai (il caso della `SELECT` da dieci minuti)
    /// si libera quando un altro thread interrompe. Runtime vero, `Notify`
    /// vero, thread vero: l'unica cosa non reale è che il «lavoro» è un
    /// futuro sempre pendente, perché senza un server TDS non c'è di meglio.
    #[test]
    fn l_interruzione_libera_un_lavoro_bloccato() {
        let attention = std::sync::Arc::new(Attention::new());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        attention.inizia();
        let da_un_altro_thread = std::sync::Arc::clone(&attention);
        let thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            da_un_altro_thread.chiedi();
        });
        let esito = runtime.block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                fino_a_interruzione(&attention, std::future::pending::<u8>()),
            )
            .await
        });
        thread.join().unwrap();
        assert_eq!(
            esito.expect("l'interruzione doveva liberare il lavoro"),
            None
        );
    }

    #[test]
    fn richiesta_durante_armamento_non_scartata_come_residuo() {
        let attention = Attention::new();
        attention.inizia_con(|| attention.chiedi());
        assert_eq!(
            fino_a_interruzione(&attention, std::future::pending::<u8>()).now_or_never(),
            Some(None),
            "richiesta valida dopo active=true non va drenata"
        );
    }

    #[test]
    fn notifica_vecchia_in_ritardo_non_interrompe_query_successiva() {
        let attention = Attention::new();
        attention.inizia();
        attention.chiedi_con(|| {
            attention.finisci();
            attention.inizia();
        });
        assert!(
            fino_a_interruzione(&attention, std::future::pending::<u8>())
                .now_or_never()
                .is_none(),
            "notifica da richiesta precedente non è richiesta della query nuova"
        );
        attention.chiedi();
        assert_eq!(
            fino_a_interruzione(&attention, std::future::pending::<u8>()).now_or_never(),
            Some(None)
        );
    }

    #[test]
    fn wake_senza_richiesta_attuale_non_basta_e_non_perde_quella_successiva() {
        let attention = Attention::new();
        attention.inizia();
        attention.notify.notify_one();
        assert!(!pronto(&attention), "wake non è consenso a interrompere");
        attention.chiedi();
        assert!(pronto(&attention));
        assert!(!pronto(&attention), "richiesta consumata una volta");
    }

    #[test]
    fn callback_waker_rientra_nello_stato_fuori_mutex() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::task::{Context, Poll, Wake, Waker};
        struct Rientrante {
            attention: Arc<Attention>,
            risvegli: AtomicUsize,
        }
        impl Wake for Rientrante {
            fn wake(self: Arc<Self>) {
                self.wake_by_ref();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.attention.finisci();
                self.risvegli.fetch_add(1, Ordering::SeqCst);
            }
        }
        let attention = Arc::new(Attention::new());
        attention.inizia();
        let observer = Arc::new(Rientrante {
            attention: Arc::clone(&attention),
            risvegli: AtomicUsize::new(0),
        });
        let waker = Waker::from(Arc::clone(&observer));
        let mut cx = Context::from_waker(&waker);
        let mut future = Box::pin(attention.attesa_richiesta());
        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
        attention.chiedi();
        assert_eq!(observer.risvegli.load(Ordering::SeqCst), 1);
        assert!(
            matches!(future.as_mut().poll(&mut cx), Poll::Pending),
            "finisci dal waker ha revocato il pending"
        );
    }

    #[test]
    fn un_lavoro_finito_vince_sull_interruzione_assente() {
        let attention = Attention::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        attention.inizia();
        let esito = runtime.block_on(fino_a_interruzione(&attention, std::future::ready(7_u8)));
        assert_eq!(esito, Some(7));
    }

    /// Cosa vede l'utente quando il server conferma, rifiuta o tace: in ogni
    /// caso «interrotta su richiesta», con il perché se la conferma manca.
    #[test]
    fn l_errore_di_interruzione_dice_sempre_che_e_stata_interrotta() {
        let SqlError::Query(ok) = errore_di_interruzione(Conferma::Ricevuta) else {
            panic!("doveva essere un errore di query");
        };
        assert_eq!(ok, crate::driver::QUERY_INTERROTTA);

        let SqlError::Query(rifiutata) =
            errore_di_interruzione(Conferma::Rifiutata("connessione chiusa".into()))
        else {
            panic!("doveva essere un errore di query");
        };
        assert!(rifiutata.starts_with(crate::driver::QUERY_INTERROTTA));
        assert!(rifiutata.contains("connessione chiusa"));

        let SqlError::Query(scaduta) = errore_di_interruzione(Conferma::Scaduta) else {
            panic!("doveva essere un errore di query");
        };
        assert!(scaduta.starts_with(crate::driver::QUERY_INTERROTTA));
        assert!(scaduta.contains("non ha confermato"));
    }
}
