//! «Aiuto → Scarica aggiornamento»: scarica lo zip ufficiale e lo verifica.
//!
//! **Solo scaricare.** Lume non estrae il pacchetto, non scrive eseguibili e
//! non sostituisce sé stesso: su un PC con EDR aziendale un processo che
//! riscrive il proprio `.exe` è proprio il comportamento che viene bloccato o
//! segnalato. Il file finisce nella cartella Download dell'utente, come se lo
//! avesse scaricato dal browser; estrarlo e avviarlo resta un suo gesto.
//!
//! Prima di dichiararlo pronto: dimensione uguale a quella dichiarata da
//! GitHub, sha256 uguale al `.sha256` pubblicato accanto, nessuna
//! sovrascrittura di un file diverso già presente. Lo scaricamento passa da un
//! `.part` che viene rinominato solo dopo la verifica.
//!
//! Chiamate bloccanti (`reqwest::blocking`): vanno in `spawn_blocking`.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::redirect::Policy;
use sha2::{Digest, Sha256};

use crate::updates::Pacchetto;

/// Oltre questa dimensione non si scarica: il pacchetto 0.4.0 pesa 25 MiB.
const TETTO_PACCHETTO: u64 = 256 * 1024 * 1024;
/// Il file `.sha256` è una riga: hash, due spazi, nome.
const TETTO_CHECKSUM: u64 = 4 * 1024;
const MAX_REDIRECT: usize = 5;
const PEZZO: usize = 64 * 1024;
/// Tempo massimo dell'intero scaricamento. Largo: 25 MiB su questa rete di
/// prova hanno richiesto più di cinque minuti.
const ATTESA_SCARICAMENTO: Duration = Duration::from_secs(30 * 60);
/// GitHub rimanda `browser_download_url` al proprio storage degli asset.
const HOST_GITHUB: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// Dove si può scaricare: in produzione solo GitHub e solo HTTPS. I test
/// passano un server locale reale in HTTP.
#[derive(Clone, Copy)]
struct Regole {
    host: &'static [&'static str],
    solo_https: bool,
}

const REGOLE_GITHUB: Regole = Regole {
    host: HOST_GITHUB,
    solo_https: true,
};

/// Il pacchetto scaricato e verificato.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scaricato {
    pub percorso: PathBuf,
    pub sha256: String,
}

impl Scaricato {
    pub fn messaggio(&self) -> String {
        format!(
            "Aggiornamento scaricato e verificato (sha256 {}…): {}. Chiudi Lume, estrai l'archivio e avvia la nuova versione.",
            self.sha256.get(..12).unwrap_or(&self.sha256),
            self.percorso.display()
        )
    }
}

/// Esito di «Scarica aggiornamento»: controllo e scaricamento in un gesto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EsitoScarica {
    /// Nessuna versione più recente: il messaggio del controllo.
    Aggiornato(String),
    /// Versione nuova scaricata e verificata.
    Scaricato(Scaricato),
    /// Versione nuova che Lume non scarica da sé (piattaforma o checksum
    /// mancanti): il messaggio con la pagina della release.
    SoloPagina(String),
}

impl EsitoScarica {
    pub fn messaggio(&self) -> String {
        match self {
            Self::Aggiornato(testo) | Self::SoloPagina(testo) => testo.clone(),
            Self::Scaricato(scaricato) => scaricato.messaggio(),
        }
    }
}

/// Chiede a GitHub l'ultima release e, se è più recente e scaricabile, la
/// scarica e la verifica. **Bloccante.**
pub fn controlla_e_scarica() -> Result<EsitoScarica, String> {
    use crate::updates::Esito;
    let esito = crate::updates::controlla()?;
    match &esito {
        Esito::Aggiornato(_) => Ok(EsitoScarica::Aggiornato(esito.messaggio())),
        Esito::Disponibile {
            pacchetto: Some(pacchetto),
            ..
        } if pacchetto.installabile && pacchetto.checksum_url.is_some() => {
            scarica(pacchetto).map(EsitoScarica::Scaricato)
        }
        Esito::Disponibile { .. } => Ok(EsitoScarica::SoloPagina(esito.messaggio())),
    }
}

/// La cartella Download dell'utente, o la home se il sistema non ne indica una.
fn cartella_download() -> Result<PathBuf, String> {
    let utente = directories::UserDirs::new()
        .ok_or_else(|| "cartella dell'utente non trovata".to_owned())?;
    Ok(utente
        .download_dir()
        .map_or_else(|| utente.home_dir().to_path_buf(), Path::to_path_buf))
}

/// Scarica e verifica `pacchetto` nella cartella Download. **Bloccante.**
pub fn scarica(pacchetto: &Pacchetto) -> Result<Scaricato, String> {
    let cartella = cartella_download()?;
    scarica_in(pacchetto, &cartella, REGOLE_GITHUB, ATTESA_SCARICAMENTO)
}

fn scarica_in(
    pacchetto: &Pacchetto,
    cartella: &Path,
    regole: Regole,
    attesa: Duration,
) -> Result<Scaricato, String> {
    if !pacchetto.installabile {
        return Err(
            "su questa piattaforma l'aggiornamento si scarica dalla pagina della release".into(),
        );
    }
    let checksum_url = pacchetto.checksum_url.as_deref().ok_or_else(|| {
        format!(
            "la release non pubblica {}.sha256: niente da verificare",
            pacchetto.nome
        )
    })?;
    if pacchetto.dimensione == 0 || pacchetto.dimensione > TETTO_PACCHETTO {
        return Err(format!(
            "dimensione dichiarata non plausibile: {} byte",
            pacchetto.dimensione
        ));
    }
    // Il nome finisce in un percorso: niente separatori né «..».
    if pacchetto.nome.is_empty()
        || pacchetto.nome.contains(['/', '\\', ':'])
        || pacchetto.nome.starts_with('.')
    {
        return Err(format!("nome del pacchetto non valido: {}", pacchetto.nome));
    }

    lume_language_servers::install_crypto_provider();
    let client = client(regole, attesa)?;

    let atteso = leggi_checksum(&client, regole, checksum_url, &pacchetto.nome)?;
    let finale = cartella.join(&pacchetto.nome);
    if finale.exists() {
        // Già scaricato prima? Si riusa solo se è identico byte per byte.
        let presente = sha256_file(&finale)?;
        if presente == atteso {
            return Ok(Scaricato {
                percorso: finale,
                sha256: presente,
            });
        }
        return Err(format!(
            "{} esiste già e non è il pacchetto pubblicato: non lo sovrascrivo",
            finale.display()
        ));
    }

    let parziale = cartella.join(format!("{}.part", pacchetto.nome));
    // Creato nuovo: un `.part` già presente (un altro scaricamento, un file
    // dell'utente) non è nostro e non va né riusato né cancellato.
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&parziale)
        .map_err(|e| {
            format!(
                "{}: {e} (uno scaricamento è già in corso o il file è rimasto da prima)",
                parziale.display()
            )
        })?;
    // Da qui in poi il `.part` è nostro: ogni errore lo toglie.
    let ottenuto = match scarica_verificando(&client, regole, pacchetto, file) {
        Ok(hash) => hash,
        Err(errore) => {
            let _ = fs::remove_file(&parziale);
            return Err(errore);
        }
    };
    if ottenuto != atteso {
        let _ = fs::remove_file(&parziale);
        return Err(format!(
            "sha256 non corrisponde (atteso {atteso}, ottenuto {ottenuto}): file scartato"
        ));
    }
    fs::rename(&parziale, &finale).map_err(|e| {
        let _ = fs::remove_file(&parziale);
        format!("{}: {e}", finale.display())
    })?;
    Ok(Scaricato {
        percorso: finale,
        sha256: ottenuto,
    })
}

fn client(regole: Regole, attesa: Duration) -> Result<reqwest::blocking::Client, String> {
    let politica = Policy::custom(move |tentativo| {
        if tentativo.previous().len() >= MAX_REDIRECT {
            tentativo.error("troppi rimandi")
        } else if url_ammesso(tentativo.url(), regole) {
            tentativo.follow()
        } else {
            let url = tentativo.url().to_string();
            tentativo.error(format!("rimando verso un indirizzo non ammesso: {url}"))
        }
    });
    reqwest::blocking::Client::builder()
        .redirect(politica)
        .connect_timeout(Duration::from_secs(15))
        .timeout(attesa)
        .user_agent(concat!(
            "lume/",
            env!("CARGO_PKG_VERSION"),
            " aggiornamento"
        ))
        .build()
        .map_err(|e| format!("client HTTP non costruito: {e}"))
}

fn url_ammesso(url: &reqwest::Url, regole: Regole) -> bool {
    let schema_ok = url.scheme() == "https" || (!regole.solo_https && url.scheme() == "http");
    schema_ok
        && url.username().is_empty()
        && url.password().is_none()
        && url
            .host_str()
            .is_some_and(|host| regole.host.contains(&host))
}

fn apri(
    client: &reqwest::blocking::Client,
    regole: Regole,
    url: &str,
) -> Result<reqwest::blocking::Response, String> {
    let url = reqwest::Url::parse(url).map_err(|_| format!("indirizzo non valido: {url}"))?;
    if !url_ammesso(&url, regole) {
        return Err(format!("indirizzo non ammesso: {url}"));
    }
    let risposta = client
        .get(url)
        .send()
        .map_err(|e| format!("scaricamento non riuscito: {e}"))?;
    if !risposta.status().is_success() {
        return Err(format!("GitHub ha risposto {}", risposta.status().as_u16()));
    }
    Ok(risposta)
}

/// Legge `<nome>.sha256` e ne estrae l'hash, controllando che parli di `nome`.
fn leggi_checksum(
    client: &reqwest::blocking::Client,
    regole: Regole,
    url: &str,
    nome: &str,
) -> Result<String, String> {
    let risposta = apri(client, regole, url)?;
    let mut corpo = Vec::new();
    Read::take(risposta, TETTO_CHECKSUM + 1)
        .read_to_end(&mut corpo)
        .map_err(|e| format!("checksum non letto: {e}"))?;
    if corpo.len() as u64 > TETTO_CHECKSUM {
        return Err("file di checksum troppo grande".into());
    }
    let testo = String::from_utf8(corpo).map_err(|_| "checksum non è testo".to_owned())?;
    let mut campi = testo.split_whitespace();
    let hash = campi.next().unwrap_or_default().to_ascii_lowercase();
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("checksum illeggibile".into());
    }
    if let Some(dichiarato) = campi.next()
        && dichiarato.trim_start_matches('*') != nome
    {
        return Err(format!(
            "il checksum si riferisce a {dichiarato}, non a {nome}"
        ));
    }
    Ok(hash)
}

/// Scarica nel `.part` appena creato contando i byte e
/// calcolando lo sha256 mentre scrive.
#[allow(clippy::indexing_slicing)] // letti <= pezzo.len(): contratto di Read
fn scarica_verificando(
    client: &reqwest::blocking::Client,
    regole: Regole,
    pacchetto: &Pacchetto,
    mut file: File,
) -> Result<String, String> {
    let mut risposta = apri(client, regole, &pacchetto.url)?;
    if let Some(lunghezza) = risposta.content_length()
        && lunghezza != pacchetto.dimensione
    {
        return Err(format!(
            "dimensione diversa da quella dichiarata ({lunghezza} invece di {})",
            pacchetto.dimensione
        ));
    }
    let mut hasher = Sha256::new();
    let mut ricevuti: u64 = 0;
    let mut pezzo = vec![0u8; PEZZO];
    loop {
        let letti = risposta
            .read(&mut pezzo)
            .map_err(|e| format!("scaricamento interrotto: {e}"))?;
        if letti == 0 {
            break;
        }
        ricevuti += letti as u64;
        if ricevuti > pacchetto.dimensione {
            return Err("il server manda più byte di quelli dichiarati".into());
        }
        hasher.update(&pezzo[..letti]);
        file.write_all(&pezzo[..letti])
            .map_err(|e| format!("scrittura del pacchetto: {e}"))?;
    }
    if ricevuti != pacchetto.dimensione {
        return Err(format!(
            "scaricamento incompleto: {ricevuti} di {} byte",
            pacchetto.dimensione
        ));
    }
    file.sync_all()
        .map_err(|e| format!("scrittura del pacchetto: {e}"))?;
    Ok(esadecimale(&hasher.finalize()))
}

#[allow(clippy::indexing_slicing)] // letti <= pezzo.len(): contratto di Read
fn sha256_file(percorso: &Path) -> Result<String, String> {
    let mut file = File::open(percorso).map_err(|e| format!("{}: {e}", percorso.display()))?;
    let mut hasher = Sha256::new();
    let mut pezzo = vec![0u8; PEZZO];
    loop {
        let letti = file
            .read(&mut pezzo)
            .map_err(|e| format!("{}: {e}", percorso.display()))?;
        if letti == 0 {
            break;
        }
        hasher.update(&pezzo[..letti]);
    }
    Ok(esadecimale(&hasher.finalize()))
}

fn esadecimale(byte: &[u8]) -> String {
    use std::fmt::Write as _;
    byte.iter()
        .fold(String::with_capacity(byte.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::thread;

    const REGOLE_LOCALI: Regole = Regole {
        host: &["127.0.0.1"],
        solo_https: false,
    };

    /// Un server HTTP vero su 127.0.0.1: risponde a ogni percorso con il
    /// corpo indicato, o con un rimando.
    struct Server {
        base: String,
    }

    enum Risposta {
        Corpo(Vec<u8>),
        Rimando(String),
    }

    fn server(rotte: Vec<(&'static str, Risposta)>) -> Server {
        let ascolto = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", ascolto.local_addr().unwrap());
        let rotte = Arc::new(rotte);
        thread::spawn(move || {
            for flusso in ascolto.incoming() {
                let Ok(mut flusso) = flusso else { break };
                let rotte = Arc::clone(&rotte);
                thread::spawn(move || servi(&mut flusso, &rotte));
            }
        });
        Server { base }
    }

    fn servi(flusso: &mut TcpStream, rotte: &[(&'static str, Risposta)]) {
        let mut dati = Vec::new();
        let mut pezzo = [0u8; 1024];
        while !dati.windows(4).any(|f| f == b"\r\n\r\n") {
            let letti = flusso.read(&mut pezzo).unwrap_or(0);
            if letti == 0 {
                return;
            }
            dati.extend_from_slice(&pezzo[..letti]);
        }
        let testa = String::from_utf8_lossy(&dati);
        let percorso = testa.split_whitespace().nth(1).unwrap_or("/").to_owned();
        match rotte.iter().find(|(p, _)| *p == percorso).map(|(_, r)| r) {
            Some(Risposta::Corpo(corpo)) => {
                let _ = write!(
                    flusso,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    corpo.len()
                );
                let _ = flusso.write_all(corpo);
            }
            Some(Risposta::Rimando(dove)) => {
                let _ = write!(
                    flusso,
                    "HTTP/1.1 302 Found\r\nLocation: {dove}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            }
            None => {
                let _ = write!(
                    flusso,
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            }
        }
    }

    fn sha(dati: &[u8]) -> String {
        esadecimale(&Sha256::digest(dati))
    }

    const NOME: &str = "lume-9.9.9-windows-x86_64-0123456789ab.zip";

    fn pacchetto(base: &str, dimensione: u64) -> Pacchetto {
        Pacchetto {
            nome: NOME.to_owned(),
            url: format!("{base}/zip"),
            dimensione,
            checksum_url: Some(format!("{base}/sha")),
            installabile: true,
        }
    }

    fn scarica_test(p: &Pacchetto, cartella: &Path) -> Result<Scaricato, String> {
        scarica_in(p, cartella, REGOLE_LOCALI, Duration::from_secs(10))
    }

    #[test]
    fn scarica_verifica_e_rinomina_solo_alla_fine() {
        let zip = b"PK finto ma reale come byte".repeat(5000);
        let s = server(vec![
            ("/zip", Risposta::Corpo(zip.clone())),
            (
                "/sha",
                Risposta::Corpo(format!("{}  {NOME}\n", sha(&zip)).into_bytes()),
            ),
        ]);
        let cartella = tempfile::tempdir().unwrap();
        let fatto = scarica_test(&pacchetto(&s.base, zip.len() as u64), cartella.path()).unwrap();
        assert_eq!(fatto.percorso, cartella.path().join(NOME));
        assert_eq!(fs::read(&fatto.percorso).unwrap(), zip);
        assert_eq!(fatto.sha256, sha(&zip));
        assert!(!cartella.path().join(format!("{NOME}.part")).exists());
        // Un secondo giro trova il file identico e lo riusa.
        let ancora = scarica_test(&pacchetto(&s.base, zip.len() as u64), cartella.path()).unwrap();
        assert_eq!(ancora, fatto);
    }

    #[test]
    fn uno_sha_diverso_scarta_il_file() {
        let zip = b"contenuto".to_vec();
        let s = server(vec![
            ("/zip", Risposta::Corpo(zip.clone())),
            (
                "/sha",
                Risposta::Corpo(format!("{}  {NOME}\n", sha(b"altro")).into_bytes()),
            ),
        ]);
        let cartella = tempfile::tempdir().unwrap();
        let errore =
            scarica_test(&pacchetto(&s.base, zip.len() as u64), cartella.path()).unwrap_err();
        assert!(errore.contains("sha256 non corrisponde"), "{errore}");
        assert_eq!(fs::read_dir(cartella.path()).unwrap().count(), 0);
    }

    #[test]
    fn dimensione_diversa_da_quella_dichiarata_e_un_errore() {
        let zip = b"dodici byte!".to_vec();
        let s = server(vec![
            ("/zip", Risposta::Corpo(zip.clone())),
            (
                "/sha",
                Risposta::Corpo(format!("{}  {NOME}\n", sha(&zip)).into_bytes()),
            ),
        ]);
        let cartella = tempfile::tempdir().unwrap();
        let errore = scarica_test(&pacchetto(&s.base, 5), cartella.path()).unwrap_err();
        assert!(errore.contains("dimensione"), "{errore}");
        assert_eq!(fs::read_dir(cartella.path()).unwrap().count(), 0);
    }

    #[test]
    fn un_file_diverso_gia_presente_non_viene_sovrascritto() {
        let zip = b"nuovo".to_vec();
        let s = server(vec![
            ("/zip", Risposta::Corpo(zip.clone())),
            (
                "/sha",
                Risposta::Corpo(format!("{}  {NOME}\n", sha(&zip)).into_bytes()),
            ),
        ]);
        let cartella = tempfile::tempdir().unwrap();
        fs::write(cartella.path().join(NOME), b"mio file").unwrap();
        let errore =
            scarica_test(&pacchetto(&s.base, zip.len() as u64), cartella.path()).unwrap_err();
        assert!(errore.contains("non lo sovrascrivo"), "{errore}");
        assert_eq!(fs::read(cartella.path().join(NOME)).unwrap(), b"mio file");
    }

    #[test]
    fn un_part_gia_presente_non_e_nostro_e_resta_intatto() {
        let zip = b"nuovo".to_vec();
        let s = server(vec![
            ("/zip", Risposta::Corpo(zip.clone())),
            (
                "/sha",
                Risposta::Corpo(format!("{}  {NOME}\n", sha(&zip)).into_bytes()),
            ),
        ]);
        let cartella = tempfile::tempdir().unwrap();
        let altrui = cartella.path().join(format!("{NOME}.part"));
        fs::write(&altrui, b"scaricamento del browser").unwrap();
        let errore =
            scarica_test(&pacchetto(&s.base, zip.len() as u64), cartella.path()).unwrap_err();
        assert!(errore.contains("già in corso"), "{errore}");
        assert_eq!(fs::read(&altrui).unwrap(), b"scaricamento del browser");
        assert!(!cartella.path().join(NOME).exists());
    }

    #[test]
    fn checksum_di_un_altro_file_e_rifiutato() {
        let zip = b"x".to_vec();
        let s = server(vec![
            ("/zip", Risposta::Corpo(zip.clone())),
            (
                "/sha",
                Risposta::Corpo(format!("{}  altro.zip\n", sha(&zip)).into_bytes()),
            ),
        ]);
        let cartella = tempfile::tempdir().unwrap();
        let errore = scarica_test(&pacchetto(&s.base, 1), cartella.path()).unwrap_err();
        assert!(errore.contains("altro.zip"), "{errore}");
    }

    #[test]
    fn i_rimandi_verso_host_non_ammessi_sono_bloccati() {
        let zip = b"x".to_vec();
        let s = server(vec![
            (
                "/zip",
                Risposta::Rimando("http://localhost:1/zip".to_owned()),
            ),
            (
                "/sha",
                Risposta::Corpo(format!("{}  {NOME}\n", sha(&zip)).into_bytes()),
            ),
        ]);
        let cartella = tempfile::tempdir().unwrap();
        let errore = scarica_test(&pacchetto(&s.base, 1), cartella.path()).unwrap_err();
        assert!(errore.contains("scaricamento non riuscito"), "{errore}");
        assert_eq!(fs::read_dir(cartella.path()).unwrap().count(), 0);
    }

    #[test]
    fn un_rimando_ammesso_viene_seguito() {
        let zip = b"dopo il rimando".to_vec();
        let s = server(vec![
            ("/zip", Risposta::Rimando("/vero".to_owned())),
            ("/vero", Risposta::Corpo(zip.clone())),
            (
                "/sha",
                Risposta::Corpo(format!("{}  {NOME}\n", sha(&zip)).into_bytes()),
            ),
        ]);
        let cartella = tempfile::tempdir().unwrap();
        let fatto = scarica_test(&pacchetto(&s.base, zip.len() as u64), cartella.path()).unwrap();
        assert_eq!(fs::read(fatto.percorso).unwrap(), zip);
    }

    #[test]
    fn in_produzione_solo_github_in_https() {
        let ok =
            reqwest::Url::parse("https://github.com/Otacon-spec/lume/releases/download/v1/x.zip")
                .unwrap();
        assert!(url_ammesso(&ok, REGOLE_GITHUB));
        // Credenziali nell'URL composte a runtime: in chiaro sembrerebbero
        // un indirizzo email allo scanner dell'export pubblico.
        let con_credenziali = format!("https://{}:{}@github.com/x.zip", "utente", "segreto");
        for no in [
            "http://github.com/x.zip",
            "https://example.com/x.zip",
            con_credenziali.as_str(),
            "https://127.0.0.1/x.zip",
        ] {
            assert!(
                !url_ammesso(&reqwest::Url::parse(no).unwrap(), REGOLE_GITHUB),
                "{no}"
            );
        }
    }

    /// Prova vera contro GitHub con le regole di produzione: release 0.4.0,
    /// rimando allo storage degli asset, sha256 pubblicato. In rete: si
    /// lancia a mano con `--ignored`.
    #[test]
    #[ignore = "scarica ~25 MiB da GitHub"]
    fn rete_reale_scarica_e_verifica_lo_zip_windows_0_4_0() {
        lume_language_servers::install_crypto_provider();
        let corpo = reqwest::blocking::Client::builder()
            .user_agent("lume-test")
            .build()
            .unwrap()
            .get("https://api.github.com/repos/Otacon-spec/lume/releases/tags/v0.4.0")
            .send()
            .unwrap()
            .text()
            .unwrap();
        let esito = crate::updates::confronta_per_piattaforma_windows(
            &corpo,
            crate::updates::Versione::leggi("0.3.0").unwrap(),
        )
        .unwrap();
        let crate::updates::Esito::Disponibile {
            pacchetto: Some(pacchetto),
            ..
        } = esito
        else {
            panic!("pacchetto Windows non trovato: {esito:?}");
        };
        let cartella = tempfile::tempdir().unwrap();
        let fatto = scarica_in(
            &pacchetto,
            cartella.path(),
            REGOLE_GITHUB,
            ATTESA_SCARICAMENTO,
        )
        .unwrap();
        assert_eq!(
            fatto.sha256,
            "2c9721eae41d7d315960b5e5e6e3805a13fd55a39460dd4d129ae03b5afb045a"
        );
        assert_eq!(
            fs::metadata(&fatto.percorso).unwrap().len(),
            pacchetto.dimensione
        );
    }

    #[test]
    fn senza_checksum_pubblicato_non_si_scarica() {
        let cartella = tempfile::tempdir().unwrap();
        let mut p = pacchetto("http://127.0.0.1:1", 10);
        p.checksum_url = None;
        let errore = scarica_test(&p, cartella.path()).unwrap_err();
        assert!(errore.contains(".sha256"), "{errore}");
    }
}
