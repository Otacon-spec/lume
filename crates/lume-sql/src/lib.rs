//! Modalità SQL di Lume: Azure SQL e Oracle, in Rust puro, caricati solo su richiesta.
//!
//! Due motori, quelli che l'utente usa davvero: **Azure SQL** e **Oracle**.
//! Entrambi in Rust puro — nessun Instant Client, nessun ODBC — e **nulla
//! viene caricato finché non si apre una connessione**, perché il budget
//! prestazionale (README) vieta lavoro in background da inattivo.
//!
//! Il crate non conosce iced e non conosce l'ibernazione: espone connessioni
//! che si aprono e si chiudono. È `lume-app` a decidere *quando*, perché è lì
//! che si sa se console e pannello sono aperti.

pub mod azure;
pub mod driver;
pub mod entra;
pub mod oracle;
pub mod vault;
pub mod worker;

pub use driver::{
    AuthKind, ColumnInfo, ConnectionProfile, Driver, DriverFactory, Engine, Interrupter,
    MAX_BATCH_ACCOUNTED_BYTES, MAX_BATCH_ROWS, QUERY_INTERROTTA, QueryBatch, QueryFlow, QuerySink,
    ResultSet, Secret, SqlError, Value, batch_rows, stream_result_set,
};
pub use vault::{OsVault, SecretStore, VaultError};
pub use worker::{
    Connection, QUERY_EVENT_QUEUE_CAPACITY, QueryEvent, RESULT_BUDGET_BYTES, RESULT_BUDGET_ROWS,
    Request, Response, State,
};
pub use worker::{QueryEvents, QueryOrigin};

/// La `DriverFactory` vera. Ha esattamente la firma del tipo, quindi si passa
/// così com'è a `SqlWorkspace::open`.
///
/// Viene chiamata **dentro** il thread del worker: qui si blocca, ed è
/// giusto che sia così.
pub fn open_driver(
    profile: &ConnectionProfile,
    secret: &Secret,
) -> Result<Box<dyn Driver>, SqlError> {
    install_crypto_provider();
    match profile.engine {
        Engine::AzureSql => Ok(Box::new(azure::AzureDriver::connect(profile, secret)?)),
        Engine::Oracle => Ok(Box::new(oracle::OracleDriver::connect(profile, secret)?)),
    }
}

/// Sceglie **esplicitamente** il provider crittografico di rustls.
///
/// Senza questa scelta rustls la deduce dalle feature accese, e se un domani
/// qualcuno riaccendesse `ring` accanto ad `aws-lc-rs` la deduzione
/// fallirebbe **a runtime, in panic, alla prima connessione TLS** — inclusi i
/// download dei language server, che con SQL non c'entrano nulla. Meglio
/// dirlo qui, dove si legge, che scoprirlo là.
///
/// Sta dietro una `Once` e non all'avvio dell'app perché chi non apre mai una
/// connessione non deve pagarne il costo: è la stessa regola dell'ibernazione.
pub fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // `Err` significa «già installato», per esempio da reqwest: va bene
        // uguale, il provider è lo stesso.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}
