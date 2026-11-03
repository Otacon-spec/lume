//! Il ponte stdio: `lume --mcp-stdio [--project <cartella>] [--instance <id>]`.
//!
//! È un processo del client: sceglie l'istanza dai record di scoperta
//! (`--instance`, poi `--project` → cwd → unica → errore), apre il trasporto
//! della piattaforma, manda il preambolo col token e poi copia byte per byte
//! nei due versi. Non capisce MCP: la spec la insegue solo il server.
//!
//! Gira su un runtime tokio `current_thread`: non ci sono due thread che si
//! serializzano sullo stesso handle sincrono, e il ponte non possiede lavoro
//! periodico.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::discovery::Token;

pub const EXIT_USAGE: i32 = 64;
pub const EXIT_UNAVAILABLE: i32 = 69;
pub const EXIT_SOFTWARE: i32 = 70;
pub const EXIT_IO: i32 = 74;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProxyArgs {
    pub project: Option<PathBuf>,
    /// Identità pubblica `<pid>-<nonce>`; solo Unix. Il token non entra mai
    /// in argv.
    pub instance: Option<String>,
}

pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<ProxyArgs, String> {
    let mut parsed = ProxyArgs::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--project" {
            let value = args.next().ok_or("--project vuole una cartella")?;
            parsed.project = Some(PathBuf::from(value));
        } else if arg == "--instance" {
            let value = args.next().ok_or("--instance vuole un identificatore")?;
            #[cfg(unix)]
            {
                let value = value
                    .to_str()
                    .ok_or("--instance vuole un identificatore UTF-8")?;
                if value.is_empty() {
                    return Err("--instance è vuoto".to_owned());
                }
                parsed.instance = Some(value.to_owned());
            }
            #[cfg(windows)]
            {
                let _ = value;
                return Err(
                    "--instance è disponibile solo con il trasporto Unix; su Windows usa --project"
                        .to_owned(),
                );
            }
        } else {
            return Err(format!("argomento sconosciuto: {}", arg.to_string_lossy()));
        }
    }
    Ok(parsed)
}

/// Senza console (release) stderr può mancare: la scrittura fallisce in
/// silenzio invece di andare in panic come farebbe `eprintln!`.
fn warn(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr(), "lume --mcp-stdio: {message}");
}

pub fn run_stdio_proxy(
    args: impl IntoIterator<Item = OsString>,
    data_dir: Result<PathBuf, String>,
) -> i32 {
    let parsed = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            warn(&message);
            return EXIT_USAGE;
        }
    };
    let data_dir = match data_dir {
        Ok(dir) => dir,
        Err(message) => {
            warn(&format!("cartella dati non trovata: {message}"));
            return EXIT_SOFTWARE;
        }
    };
    #[cfg(windows)]
    {
        run_windows(parsed, &data_dir)
    }
    #[cfg(unix)]
    {
        run_unix(parsed, &data_dir)
    }
}

#[cfg(windows)]
fn run_windows(parsed: ProxyArgs, data_dir: &Path) -> i32 {
    let dir = crate::discovery::discovery_dir(data_dir);
    let records = crate::discovery::prune_stale(&dir);
    let project = match parsed
        .project
        .as_deref()
        .map(crate::confine::canonical)
        .transpose()
    {
        Ok(project) => project,
        Err(error) => {
            warn(&format!("--project non leggibile: {error}"));
            return EXIT_USAGE;
        }
    };
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|cwd| crate::confine::canonical(&cwd).ok());
    let record = match crate::discovery::choose(&records, project.as_deref(), cwd.as_deref()) {
        Ok(record) => record,
        Err(error) => {
            warn(&error.to_string());
            return EXIT_UNAVAILABLE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            warn(&format!("runtime non avviato: {error}"));
            return EXIT_SOFTWARE;
        }
    };
    let esito = runtime.block_on(bridge(
        tokio::io::stdin(),
        tokio::io::stdout(),
        &record.pipe,
        &record.token,
    ));
    // Vedi `run_unix`: nessuna attesa delle letture bloccanti di stdin.
    runtime.shutdown_background();
    match esito {
        Ok(()) => 0,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // PID riusato da un altro processo nostro: il file dice «vivo» ma
            // la pipe non c'è più.
            remove_stale_record(&dir, &record);
            warn("l'istanza di Lume non risponde più: file di scoperta stantio rimosso");
            EXIT_UNAVAILABLE
        }
        Err(error) => {
            warn(&format!("ponte interrotto: {error}"));
            EXIT_IO
        }
    }
}

#[cfg(unix)]
fn run_unix(parsed: ProxyArgs, data_dir: &Path) -> i32 {
    let dir = crate::unix::discovery_dir(data_dir);
    let dir_messaggio = dir.clone();
    let project = match parsed
        .project
        .as_deref()
        .map(crate::confine::canonical)
        .transpose()
    {
        Ok(project) => project,
        Err(error) => {
            warn(&format!("--project non leggibile: {error}"));
            return EXIT_USAGE;
        }
    };
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|cwd| crate::confine::canonical(&cwd).ok());
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            warn(&format!("runtime non avviato: {error}"));
            return EXIT_SOFTWARE;
        }
    };
    let esito = runtime.block_on(async move {
        let report = crate::unix::select_report(
            &dir,
            project.as_deref(),
            cwd.as_deref(),
            parsed.instance.as_deref(),
        )
        .await;
        for notice in &report.notices {
            warn(&format!(
                "record di scoperta ignorato in {}: {}",
                notice.path.display(),
                notice.reason
            ));
        }
        let selected = report.result.map_err(Esito::Scelta)?;
        // Da qui in poi si parla solo con l'istanza scelta: la nuova
        // connessione è già stata rivalidata dal selettore.
        bridge_socket(
            tokio::io::stdin(),
            tokio::io::stdout(),
            selected.stream,
            &selected.record.token,
        )
        .await
        .map_err(Esito::Io)
    });
    // Il runtime non attende le letture bloccanti di stdin: quando il server
    // chiude, il ponte deve uscire con il suo codice invece di restare appeso
    // finché il client tiene aperto stdin.
    runtime.shutdown_background();
    match esito {
        Ok(()) => 0,
        Err(Esito::Scelta(error)) => {
            warn(&crate::unix::failure_message(&error, &dir_messaggio));
            match error {
                crate::unix::SelectError::NoInstance
                | crate::unix::SelectError::NoMatch(_)
                | crate::unix::SelectError::UnknownInstance(_)
                | crate::unix::SelectError::Ambiguous(_)
                | crate::unix::SelectError::Unreachable(_)
                | crate::unix::SelectError::Busy(_)
                | crate::unix::SelectError::Deadline(_) => EXIT_UNAVAILABLE,
                crate::unix::SelectError::Discovery(_)
                | crate::unix::SelectError::Unsafe(_)
                | crate::unix::SelectError::Inconsistent(_)
                | crate::unix::SelectError::Io(_) => EXIT_IO,
            }
        }
        Err(Esito::Io(error)) if bridge_is_unavailable(&error) => {
            // «Occupato» e «l'istanza ha chiuso prima di rispondere» hanno già
            // un messaggio completo e sono servizio non disponibile.
            warn(&error.to_string());
            EXIT_UNAVAILABLE
        }
        Err(Esito::Io(error)) => {
            warn(&format!("ponte interrotto: {error}"));
            EXIT_IO
        }
    }
}

#[cfg(unix)]
enum Esito {
    Scelta(crate::unix::SelectError),
    Io(std::io::Error),
}

/// Rimuove solo il record scelto dal proxy. Un nuovo server può aver già
/// riutilizzato il PID e sostituito il file mentre il proxy tentava la pipe.
#[cfg(windows)]
fn remove_stale_record(dir: &Path, selected: &crate::discovery::DiscoveryRecord) -> bool {
    let path = crate::discovery::record_path(dir, selected.pid);
    let is_still_selected = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<crate::discovery::DiscoveryRecord>(&bytes).ok())
        .is_some_and(|current| current == *selected);
    is_still_selected && std::fs::remove_file(path).is_ok()
}

/// Ponte Windows: apre la pipe e delega al preambolo e ai due pump.
#[cfg(windows)]
pub async fn bridge<I, O>(
    stdin: I,
    stdout: O,
    pipe_name: &str,
    token: &Token,
) -> std::io::Result<()>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
{
    let pipe = open_pipe(pipe_name).await?;
    handshake_and_pump(pipe, stdin, stdout, token, None).await
}

/// Ponte Unix: lo stream è già verificato dal selettore.
#[cfg(unix)]
pub async fn bridge_socket<I, O>(
    stdin: I,
    stdout: O,
    stream: tokio::net::UnixStream,
    token: &Token,
) -> std::io::Result<()>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
{
    let mut busy = crate::protocol::busy_line().into_bytes();
    busy.push(b'\n');
    handshake_and_pump(stream, stdin, stdout, token, Some(&busy)).await
}

/// Rifiuto esplicito al tetto: il server ha scritto `protocol::busy_line()`
/// dopo il preambolo, e il ponte esce con codice «servizio non disponibile».
fn busy_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::ConnectionRefused,
        crate::protocol::BUSY_MESSAGE,
    )
}

/// L'istanza ha chiuso la connessione prima della prima risposta: token
/// rifiutato, preambolo scaduto o server in chiusura. Non è «occupato» e il
/// messaggio dice cosa è successo; il codice resta «servizio non disponibile».
fn closed_before_reply() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "l'istanza di Lume ha chiuso la connessione prima di rispondere: token rifiutato, preambolo non arrivato in tempo o server in chiusura",
    )
}

/// «Occupato» e «chiuso prima di rispondere» sono servizio non disponibile;
/// ogni altro errore del trasporto resta un errore di I/O.
fn bridge_is_unavailable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::UnexpectedEof
    )
}

/// Primi byte del server: il segnale di tetto, un'altra risposta, o la
/// chiusura prima di qualunque byte.
enum Greeting {
    Busy(Vec<u8>),
    Bytes(Vec<u8>),
    Closed,
}

/// Legge l'inizio della risposta del server fermandosi appena i byte letti non
/// possono più essere la riga di tetto (o quando ne è arrivata una intera):
/// un segnale spezzato su più letture viene riconosciuto e una risposta normale
/// non resta in attesa. Nessun byte viene perso: quello che esce da qui va
/// inoltrato al client.
async fn read_greeting<R: AsyncRead + Unpin>(
    from: &mut R,
    expected: &[u8],
) -> std::io::Result<Greeting> {
    let mut head = Vec::new();
    let mut buffer = vec![0_u8; 8 * 1024];
    loop {
        let read = from.read(&mut buffer).await?;
        if read == 0 {
            return Ok(if head.is_empty() {
                Greeting::Closed
            } else {
                Greeting::Bytes(head)
            });
        }
        head.extend_from_slice(buffer.get(..read).unwrap_or_default());
        if head.starts_with(expected) {
            return Ok(Greeting::Busy(head));
        }
        if !expected.starts_with(&head) {
            return Ok(Greeting::Bytes(head));
        }
    }
}

/// Preambolo col token e poi due pump byte per byte. `busy` è la riga con cui
/// il server rifiuta una connessione al tetto: dove il trasporto non la manda
/// (`None`, pipe Windows) il comportamento resta quello precedente.
async fn handshake_and_pump<S, I, O>(
    stream: S,
    stdin: I,
    stdout: O,
    token: &Token,
    busy: Option<&[u8]>,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
{
    let (mut from_server, mut to_server) = tokio::io::split(stream);
    let mut stdout = stdout;
    let preamble = async {
        to_server
            .write_all(crate::discovery::preamble(token).as_bytes())
            .await?;
        to_server.write_all(b"\n").await?;
        to_server.flush().await
    };
    preamble.await?;
    // Il verso client → server parte subito: nessun byte del client resta
    // fermo mentre si guarda la prima riga del server.
    let up = pump(stdin, to_server);
    tokio::pin!(up);
    if let Some(expected) = busy {
        let greeting = {
            let read = read_greeting(&mut from_server, expected);
            tokio::pin!(read);
            tokio::select! {
                biased;
                result = &mut up => return result,
                greeting = &mut read => greeting?,
            }
        };
        match greeting {
            Greeting::Closed => return Err(closed_before_reply()),
            Greeting::Busy(bytes) => {
                // Il client vede un errore JSON-RPC esplicito; il ponte esce
                // «occupato» senza interpretare il resto del protocollo.
                stdout.write_all(&bytes).await?;
                stdout.flush().await?;
                return Err(busy_error());
            }
            Greeting::Bytes(bytes) => {
                stdout.write_all(&bytes).await?;
                stdout.flush().await?;
            }
        }
    }
    let down = pump_count(from_server, stdout);
    tokio::pin!(down);
    tokio::select! {
        biased;
        result = &mut up => result,
        result = &mut down => result.map(|_| ()),
    }
}

/// Copia a pezzi con `flush` a ogni pezzo: una risposta MCP non resta ferma
/// in un buffer mentre il client la aspetta.
async fn pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(from: R, to: W) -> std::io::Result<()> {
    pump_count(from, to).await.map(|_| ())
}

async fn pump_count<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut from: R,
    mut to: W,
) -> std::io::Result<u64> {
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = from.read(&mut buffer).await?;
        if read == 0 {
            to.flush().await?;
            return Ok(total);
        }
        to.write_all(buffer.get(..read).unwrap_or_default()).await?;
        to.flush().await?;
        total = total.saturating_add(read as u64);
    }
}

#[cfg(windows)]
async fn open_pipe(
    name: &str,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;
    // Tentativi limitati, una volta sola all'avvio del processo ponte: le
    // quattro istanze possono essere tutte occupate per un attimo.
    for _ in 0..40 {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(name) {
            Ok(client) => return Ok(client),
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "tutte le istanze della pipe sono occupate",
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn os(parole: &[&str]) -> Vec<OsString> {
        parole.iter().map(OsString::from).collect()
    }

    #[test]
    fn gli_argomenti_del_ponte() {
        assert_eq!(parse_args(os(&[])).unwrap(), ProxyArgs::default());
        assert_eq!(
            parse_args(os(&["--project", r"C:\lavoro\lume"]))
                .unwrap()
                .project,
            Some(PathBuf::from(r"C:\lavoro\lume"))
        );
        assert!(parse_args(os(&["--project"])).is_err(), "manca la cartella");
        assert!(parse_args(os(&["--verbose"])).is_err());
        #[cfg(unix)]
        {
            assert_eq!(
                parse_args(os(&["--instance", "42-0a1b2c3d"]))
                    .unwrap()
                    .instance,
                Some("42-0a1b2c3d".to_owned())
            );
            assert!(parse_args(os(&["--instance"])).is_err());
            assert!(parse_args(os(&["--instance", ""])).is_err());
        }
        #[cfg(windows)]
        {
            assert!(parse_args(os(&["--instance", "42-0a1b2c3d"])).is_err());
        }
    }

    /// Nessuna istanza: il ponte esce subito con 69 e non tocca stdin.
    #[test]
    fn senza_istanze_il_ponte_esce_con_servizio_non_disponibile() {
        let dati = tempfile::tempdir().unwrap();
        assert_eq!(
            run_stdio_proxy(os(&[]), Ok(dati.path().to_path_buf())),
            EXIT_UNAVAILABLE
        );
        assert_eq!(
            run_stdio_proxy(os(&["--boh"]), Ok(dati.path().to_path_buf())),
            EXIT_USAGE
        );
        assert_eq!(
            run_stdio_proxy(os(&[]), Err("niente".to_owned())),
            EXIT_SOFTWARE
        );
    }

    /// «Occupato» e «chiuso prima di rispondere» sono servizio non
    /// disponibile; un errore di trasporto a metà sessione no.
    #[test]
    fn solo_il_tetto_e_la_chiusura_iniziale_sono_servizio_non_disponibile() {
        assert!(bridge_is_unavailable(&busy_error()));
        assert!(bridge_is_unavailable(&closed_before_reply()));
        for kind in [
            std::io::ErrorKind::BrokenPipe,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::ConnectionAborted,
            std::io::ErrorKind::NotConnected,
            std::io::ErrorKind::TimedOut,
        ] {
            let errore = std::io::Error::from(kind);
            assert!(!bridge_is_unavailable(&errore), "{kind:?}");
        }
        assert!(busy_error().to_string().contains("occupate"));
        assert!(!closed_before_reply().to_string().contains("occupate"));
    }

    #[cfg(windows)]
    #[test]
    fn la_pulizia_non_rimuove_un_record_sostituito_dallo_stesso_pid() {
        let dati = tempfile::tempdir().unwrap();
        let dir = crate::discovery::discovery_dir(dati.path());
        let precedente = crate::discovery::DiscoveryRecord {
            pid: 42,
            pipe: "pipe-vecchia".to_owned(),
            root: PathBuf::from("C:/vecchia"),
            token: Token::generate().unwrap(),
            version: "0.2.1".to_owned(),
            started_at: 1,
        };
        let _vecchia = crate::discovery::DiscoveryFile::write(&dir, &precedente).unwrap();
        let selezionata = precedente.clone();
        let mut nuova = crate::discovery::DiscoveryRecord {
            pid: 42,
            pipe: "pipe-nuova".to_owned(),
            root: PathBuf::from("C:/vecchia"),
            token: Token::generate().unwrap(),
            version: "0.2.1".to_owned(),
            started_at: 2,
        };
        nuova.pipe = "pipe-nuova".to_owned();
        let _nuova = crate::discovery::DiscoveryFile::write(&dir, &nuova).unwrap();
        assert!(!remove_stale_record(&dir, &selezionata));
        let records = crate::discovery::read_records(&dir);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].1.pipe, "pipe-nuova");
    }
}
