//! Controllo aggiornamenti.
//!
//! **Solo su richiesta.** Il budget prestazionale (README)
//! vieta lavoro da inattivo: niente timer che sveglia il processo ogni tot ore
//! per chiedere a GitHub se c'è una versione nuova. Il controllo parte quando
//! l'utente sceglie «Controlla aggiornamenti…», e non parte mai da solo —
//! nemmeno una volta all'avvio, che allungherebbe il primo frame con una
//! richiesta di rete.
//!
//! `releases/latest` risponde 404 in due casi che da fuori non si distinguono:
//! il repo non ha ancora nessuna release pubblicata, oppure non è raggiungibile.
//! In entrambi il messaggio indica l'assenza di release e la pagina delle
//! release del repository pubblico. Il controllo
//! non legge token o credenziali e non autentica richieste per conto dell'utente.
//!
//! ATTENZIONE: `releases/latest` **ignora le pre-release** di GitHub. Le versioni alfa
//! vanno quindi pubblicate come release normali con un tag `vX.Y.Z` senza
//! suffisso (lo stato «alfa» sta nel titolo e nelle note), altrimenti questo
//! controllo non le vedrebbe mai.
//!
//! La chiamata è sincrona (`reqwest::blocking`); workspace, download dei
//! language server e fetch Git condividono reqwest 0.13. Va
//! quindi passata a `spawn_blocking`, come gix e il vault.

use std::io::Read;
use std::time::Duration;

/// Il repo da cui si leggono le release.
const REPO: &str = "Otacon-spec/lume";
/// GitHub rifiuta le richieste senza User-Agent con 403.
const AGENTE: &str = concat!("lume/", env!("CARGO_PKG_VERSION"));
/// Oltre questo si smette di aspettare: un controllo manuale che pende per
/// mezzo minuto è indistinguibile da uno rotto.
const ATTESA: Duration = Duration::from_secs(10);
/// Byte massimi del corpo di `releases/latest`: una release vera pesa una
/// decina di KiB anche con note lunghe.
const TETTO_CORPO: usize = 256 * 1024;

/// Una versione `major.minor.patch`. Niente pre-release: i tag del progetto
/// non ne hanno, e accettarle senza saperle ordinare darebbe un confronto
/// sbagliato invece di un errore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Versione {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Versione {
    /// Legge `1.2.3` o `v1.2.3`. `None` se non è quella forma.
    pub fn leggi(testo: &str) -> Option<Self> {
        let corpo = testo.trim().trim_start_matches(['v', 'V']);
        let mut pezzi = corpo.split('.');
        let major = pezzi.next()?.parse().ok()?;
        let minor = pezzi.next()?.parse().ok()?;
        let patch = pezzi.next()?.parse().ok()?;
        if pezzi.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

impl std::fmt::Display for Versione {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// La versione con cui questo binario è stato compilato.
pub fn versione_corrente() -> Option<Versione> {
    Versione::leggi(env!("CARGO_PKG_VERSION"))
}

// Alcuni casi sono usati solo dal target nativo corrispondente; il test usa
// esplicitamente i nomi degli asset macOS anche quando gira su Linux.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Piattaforma {
    WindowsX64,
    LinuxX64,
    MacosAarch64,
    MacosX64,
    NonSupportata,
}

impl Piattaforma {
    fn corrente() -> Self {
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            return Self::WindowsX64;
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            return Self::LinuxX64;
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            return Self::MacosAarch64;
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            return Self::MacosX64;
        }
        #[allow(unreachable_code)]
        Self::NonSupportata
    }

    fn nome(self) -> &'static str {
        match self {
            Self::WindowsX64 => "Windows x86_64",
            Self::LinuxX64 => "Linux x86_64",
            Self::MacosAarch64 => "macOS Apple Silicon",
            Self::MacosX64 => "macOS Intel",
            Self::NonSupportata => "questa piattaforma",
        }
    }

    fn nome_pacchetto(self, versione: Versione) -> Option<String> {
        let nome = match self {
            Self::WindowsX64 => "Lume-Bundle-windows-x86_64.zip".to_owned(),
            Self::LinuxX64 => format!("lume-{versione}-linux-x86_64.tar.gz"),
            Self::MacosAarch64 => format!("lume-{versione}-macos-aarch64.tar.gz"),
            Self::MacosX64 => format!("lume-{versione}-macos-x86_64.tar.gz"),
            Self::NonSupportata => return None,
        };
        Some(nome)
    }
}

/// Cosa dire all'utente dopo il controllo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Esito {
    /// Non c'è niente di più recente.
    Aggiornato(Versione),
    /// C'è una versione nuova, con la pagina da cui prenderla.
    Disponibile {
        versione: Versione,
        pagina: String,
        pacchetto: Option<String>,
        piattaforma: &'static str,
    },
}

impl Esito {
    /// Il testo della notifica in status bar.
    pub fn messaggio(&self) -> String {
        match self {
            Self::Aggiornato(v) => format!("Lume {v} è l'ultima versione."),
            Self::Disponibile {
                versione,
                pagina,
                pacchetto: Some(pacchetto),
                ..
            } => format!("Disponibile Lume {versione} ({pacchetto}): {pagina}"),
            Self::Disponibile {
                versione,
                pagina,
                piattaforma,
                pacchetto: None,
            } => format!(
                "Lume {versione} è uscito, ma non risulta un pacchetto per {piattaforma}. Release: {pagina}"
            ),
        }
    }
}

/// Interpreta la risposta di `releases/latest` confrontandola con `corrente`.
///
/// Separata dalla rete apposta: è la parte che può sbagliare (tag strano,
/// campo mancante, versione più vecchia della nostra) ed è l'unica testabile
/// senza chiamare GitHub.
pub fn confronta(corpo: &str, corrente: Versione) -> Result<Esito, String> {
    confronta_per(corpo, corrente, Piattaforma::corrente())
}

fn confronta_per(
    corpo: &str,
    corrente: Versione,
    piattaforma: Piattaforma,
) -> Result<Esito, String> {
    let json: serde_json::Value =
        serde_json::from_str(corpo).map_err(|e| format!("risposta illeggibile: {e}"))?;
    let tag = json
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "la release non ha un tag".to_owned())?;
    let ultima =
        Versione::leggi(tag).ok_or_else(|| format!("tag «{tag}» non è una versione leggibile"))?;

    if ultima <= corrente {
        return Ok(Esito::Aggiornato(corrente));
    }
    let pagina = json
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .map_or_else(
            || format!("https://github.com/{REPO}/releases/tag/{tag}"),
            ToOwned::to_owned,
        );
    let pacchetto_atteso = piattaforma.nome_pacchetto(ultima);
    let pacchetto = json
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .and_then(|assets| {
            assets.iter().find_map(|asset| {
                let nome = asset.get("name").and_then(serde_json::Value::as_str)?;
                (Some(nome) == pacchetto_atteso.as_deref()).then(|| nome.to_owned())
            })
        });
    Ok(Esito::Disponibile {
        versione: ultima,
        pagina,
        pacchetto,
        piattaforma: piattaforma.nome(),
    })
}

/// Chiede a GitHub l'ultima release. **Bloccante**: va in `spawn_blocking`.
pub fn controlla() -> Result<Esito, String> {
    let corrente = versione_corrente()
        .ok_or_else(|| "la versione di questo binario non è leggibile".to_owned())?;
    controlla_da("https://api.github.com", corrente, ATTESA, TETTO_CORPO)
}

/// Il controllo vero, con base URL, attesa e tetto iniettabili: i test
/// parlano con un server locale reale, non con GitHub.
fn controlla_da(
    base: &str,
    corrente: Versione,
    attesa: Duration,
    tetto: usize,
) -> Result<Esito, String> {
    // `reqwest` è compilato senza provider rustls: chi non ha mai aperto una
    // connessione SQL andrebbe in panic qui. La funzione è idempotente.
    #[cfg(feature = "sql")]
    lume_sql::install_crypto_provider();
    let client = reqwest::blocking::Client::builder()
        .timeout(attesa)
        .user_agent(AGENTE)
        .build()
        .map_err(|e| format!("client HTTP non costruito: {e}"))?;
    let risposta = client
        .get(format!("{base}/repos/{REPO}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("controllo non riuscito: {e}"))?;
    let stato = risposta.status();
    if !stato.is_success() {
        return Err(errore_http(stato.as_u16()));
    }
    // `take(tetto + 1)`: se arriva un byte in più del tetto, il corpo è
    // troppo grande e non si legge oltre.
    let mut corpo = Vec::new();
    Read::take(risposta, tetto as u64 + 1)
        .read_to_end(&mut corpo)
        .map_err(|e| format!("risposta non letta: {e}"))?;
    if corpo.len() > tetto {
        return Err(format!(
            "la risposta di GitHub è oltre {} KiB: ignorata",
            tetto / 1024
        ));
    }
    let corpo = String::from_utf8(corpo).map_err(|_| "la risposta non è testo".to_owned())?;
    confronta(&corpo, corrente)
}

/// Il testo di «Informazioni su Lume».
///
/// Contiene l'attribuzione del file `NOTICE`: con Apache-2.0 §4(d) chi
/// ridistribuisce un derivato deve mostrarla dove l'opera mostra i crediti,
/// e questa è la schermata di Lume che fa da crediti.
pub fn informazioni() -> String {
    format!(
        "Lume {} — creato da mak (https://github.com/{REPO}) · Apache-2.0",
        env!("CARGO_PKG_VERSION")
    )
}

/// Cosa dire quando GitHub non risponde 2xx.
fn errore_http(stato: u16) -> String {
    match stato {
        404 => format!("Nessuna release pubblicata: https://github.com/{REPO}/releases"),
        403 | 429 => "GitHub limita le richieste: riprova fra qualche minuto".to_owned(),
        altro => format!("GitHub ha risposto {altro}"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn un_404_indica_la_pagina_delle_release() {
        assert_eq!(
            errore_http(404),
            "Nessuna release pubblicata: https://github.com/Otacon-spec/lume/releases"
        );
        assert!(errore_http(403).contains("limita"));
        assert_eq!(errore_http(500), "GitHub ha risposto 500");
    }

    fn v(major: u32, minor: u32, patch: u32) -> Versione {
        Versione {
            major,
            minor,
            patch,
        }
    }

    #[test]
    fn il_tag_si_legge_con_e_senza_la_v() {
        assert_eq!(Versione::leggi("1.2.3"), Some(v(1, 2, 3)));
        assert_eq!(Versione::leggi("v0.9.10"), Some(v(0, 9, 10)));
        assert_eq!(Versione::leggi(" v2.0.0 "), Some(v(2, 0, 0)));
    }

    /// Un tag che non sappiamo ordinare non deve diventare un confronto
    /// sbagliato: deve diventare un `None`, cioè un errore visibile.
    #[test]
    fn i_tag_che_non_sappiamo_ordinare_sono_rifiutati() {
        for strano in ["1.2", "1.2.3.4", "1.2.3-rc1", "release", ""] {
            assert_eq!(
                Versione::leggi(strano),
                None,
                "«{strano}» doveva essere None"
            );
        }
    }

    #[test]
    fn le_versioni_si_ordinano_per_campo() {
        assert!(v(0, 9, 10) > v(0, 9, 9));
        assert!(v(1, 0, 0) > v(0, 99, 99));
        assert!(v(1, 2, 0) > v(1, 1, 99));
    }

    #[test]
    fn una_release_piu_recente_porta_la_sua_pagina() {
        let corpo = r#"{"tag_name":"v1.4.0","html_url":"https://example.invalid/r/1.4.0","assets":[{"name":"Lume-Bundle-windows-x86_64.zip"}]}"#;
        assert_eq!(
            confronta_per(corpo, v(1, 3, 9), Piattaforma::WindowsX64),
            Ok(Esito::Disponibile {
                versione: v(1, 4, 0),
                pagina: "https://example.invalid/r/1.4.0".to_owned(),
                pacchetto: Some("Lume-Bundle-windows-x86_64.zip".to_owned()),
                piattaforma: "Windows x86_64",
            })
        );
    }

    #[test]
    fn una_release_senza_pacchetto_aarch64_lo_dice_invece_di_fingere_che_sia_scaricabile() {
        let corpo = r#"{"tag_name":"v1.4.0","assets":[{"name":"lume-1.4.0-macos-x86_64.tar.gz"}]}"#;
        let esito = confronta_per(corpo, v(1, 3, 9), Piattaforma::MacosAarch64).unwrap();
        assert_eq!(
            esito,
            Esito::Disponibile {
                versione: v(1, 4, 0),
                pagina: "https://github.com/Otacon-spec/lume/releases/tag/v1.4.0".to_owned(),
                pacchetto: None,
                piattaforma: "macOS Apple Silicon",
            }
        );
        let messaggio = esito.messaggio();
        assert!(messaggio.contains("macOS Apple Silicon"), "{messaggio}");
        assert!(
            messaggio.contains("non risulta un pacchetto"),
            "{messaggio}"
        );
    }

    #[test]
    fn il_pacchetto_aarch64_esatto_risulta_disponibile_su_mac() {
        let corpo =
            r#"{"tag_name":"v1.4.0","assets":[{"name":"lume-1.4.0-macos-aarch64.tar.gz"}]}"#;
        let esito = confronta_per(corpo, v(1, 3, 9), Piattaforma::MacosAarch64).unwrap();
        assert!(matches!(
            esito,
            Esito::Disponibile { pacchetto: Some(nome), .. }
                if nome == "lume-1.4.0-macos-aarch64.tar.gz"
        ));
    }

    #[test]
    fn i_nomi_asset_rispecchiano_tutti_i_pacchetti_previsti() {
        for (piattaforma, nome) in [
            (Piattaforma::LinuxX64, "lume-1.4.0-linux-x86_64.tar.gz"),
            (Piattaforma::MacosX64, "lume-1.4.0-macos-x86_64.tar.gz"),
        ] {
            let corpo = format!(r#"{{"tag_name":"v1.4.0","assets":[{{"name":"{nome}"}}]}}"#);
            let esito = confronta_per(&corpo, v(1, 3, 9), piattaforma).unwrap();
            assert!(matches!(
                esito,
                Esito::Disponibile { pacchetto: Some(uscita), .. } if uscita == nome
            ));
        }
    }

    /// Senza `html_url` si costruisce l'indirizzo dal tag invece di perdere
    /// l'avviso: dire «c'è una versione nuova» senza dire dove prenderla
    /// sarebbe inutile.
    #[test]
    fn senza_html_url_l_indirizzo_si_ricava_dal_tag() {
        let corpo = r#"{"tag_name":"v1.4.0"}"#;
        let Ok(Esito::Disponibile { pagina, .. }) = confronta(corpo, v(1, 0, 0)) else {
            panic!("doveva risultare disponibile");
        };
        assert_eq!(
            pagina,
            format!("https://github.com/{REPO}/releases/tag/v1.4.0")
        );
    }

    /// La release pari o più vecchia della nostra non è un aggiornamento —
    /// il caso «più vecchia» capita davvero quando si gira una build locale
    /// più avanti dell'ultimo tag pubblicato.
    #[test]
    fn una_release_non_piu_recente_non_e_un_aggiornamento() {
        for tag in ["v1.3.9", "v1.0.0"] {
            let corpo = format!("{{\"tag_name\":\"{tag}\"}}");
            assert_eq!(
                confronta_per(&corpo, v(1, 3, 9), Piattaforma::WindowsX64),
                Ok(Esito::Aggiornato(v(1, 3, 9)))
            );
        }
    }

    #[test]
    fn una_risposta_rotta_e_un_errore_non_un_silenzio() {
        assert!(confronta("non json", v(1, 0, 0)).is_err());
        assert!(confronta("{}", v(1, 0, 0)).is_err());
        assert!(confronta(r#"{"tag_name":"latest"}"#, v(1, 0, 0)).is_err());
    }

    /// Se questo fallisce, `Cargo.toml` ha una versione che il controllo non
    /// sa confrontare — e l'aggiornamento non funzionerebbe mai.
    #[test]
    fn informazioni_preservano_versione_e_attribuzione_originale() {
        assert!(versione_corrente().is_some());
        assert_eq!(
            informazioni(),
            format!(
                "Lume {} — creato da mak (https://github.com/Otacon-spec/lume) · Apache-2.0",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    #[test]
    fn il_messaggio_dice_dove_prendere_la_versione_nuova() {
        let esito = Esito::Disponibile {
            versione: v(2, 0, 0),
            pagina: "https://example.invalid/r".to_owned(),
            pacchetto: Some("Lume-Bundle-windows-x86_64.zip".to_owned()),
            piattaforma: "Windows x86_64",
        };
        let testo = esito.messaggio();
        assert!(testo.contains("2.0.0"), "{testo}");
        assert!(testo.contains("https://example.invalid/r"), "{testo}");
    }

    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::time::Instant;

    /// Consuma la richiesta HTTP di un client vero, fino alla riga vuota.
    fn leggi_testa(flusso: &mut TcpStream) -> String {
        flusso
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut dati = Vec::new();
        let mut pezzo = [0u8; 512];
        while !dati.windows(4).any(|finestra| finestra == b"\r\n\r\n") {
            let letti = flusso.read(&mut pezzo).unwrap_or(0);
            if letti == 0 {
                break;
            }
            dati.extend_from_slice(&pezzo[..letti]);
        }
        String::from_utf8(dati).unwrap()
    }

    #[test]
    fn un_corpo_enorme_di_releases_non_si_legge_tutto() {
        let ascolto = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", ascolto.local_addr().unwrap());
        let (uscito, attesa_uscita) = mpsc::channel::<usize>();
        std::thread::spawn(move || {
            let (mut flusso, _) = ascolto.accept().unwrap();
            leggi_testa(&mut flusso);
            // Nessun Content-Length: il corpo dura finché il client non chiude.
            flusso
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n")
                .unwrap();
            let blocco = [b'{'; 8192];
            let mut scritti = 0usize;
            while flusso.write_all(&blocco).is_ok() {
                scritti += blocco.len();
                if scritti > 256 * 1024 * 1024 {
                    break;
                }
            }
            let _ = uscito.send(scritti);
        });

        let errore = controlla_da(&base, v(0, 1, 0), Duration::from_secs(5), 4096)
            .expect_err("un corpo enorme deve dare errore");

        assert!(errore.contains("oltre"), "{errore}");
        let scritti = attesa_uscita
            .recv_timeout(Duration::from_secs(10))
            .expect("il server doveva accorgersi della chiusura");
        assert!(
            scritti < 64 * 1024 * 1024,
            "il client ha letto troppo: {scritti}"
        );
    }

    #[test]
    fn un_server_muto_scade_entro_l_attesa() {
        let ascolto = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", ascolto.local_addr().unwrap());
        let (fine, attesa_fine) = mpsc::channel::<()>();
        let filo = std::thread::spawn(move || {
            let (flusso, _) = ascolto.accept().unwrap();
            let _ = attesa_fine.recv_timeout(Duration::from_secs(10));
            drop(flusso);
        });

        let inizio = Instant::now();
        let errore = controlla_da(&base, v(0, 1, 0), Duration::from_secs(1), 4096)
            .expect_err("un server muto deve dare errore");

        assert!(
            inizio.elapsed() < Duration::from_secs(5),
            "{:?}",
            inizio.elapsed()
        );
        assert!(errore.contains("controllo non riuscito"), "{errore}");
        fine.send(()).unwrap();
        filo.join().unwrap();
    }

    #[test]
    fn una_release_vera_da_un_server_vero_si_confronta() {
        let corpo = r#"{"tag_name":"v9.9.9","html_url":"https://github.com/Otacon-spec/lume/releases/tag/v9.9.9"}"#;
        let ascolto = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", ascolto.local_addr().unwrap());
        let filo = std::thread::spawn(move || {
            let (mut flusso, _) = ascolto.accept().unwrap();
            let richiesta = leggi_testa(&mut flusso);
            assert!(
                richiesta.starts_with("GET /repos/Otacon-spec/lume/releases/latest HTTP/1.1\r\n"),
                "{richiesta}"
            );
            let risposta = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{corpo}",
                corpo.len()
            );
            flusso.write_all(risposta.as_bytes()).unwrap();
        });

        let esito = controlla_da(&base, v(0, 1, 0), Duration::from_secs(5), 4096).unwrap();
        filo.join().unwrap();
        assert!(matches!(
            esito,
            Esito::Disponibile { versione, .. } if versione == v(9, 9, 9)
        ));
    }

    #[test]
    fn un_corpo_non_utf8_e_un_errore_leggibile() {
        let ascolto = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", ascolto.local_addr().unwrap());
        let filo = std::thread::spawn(move || {
            let (mut flusso, _) = ascolto.accept().unwrap();
            leggi_testa(&mut flusso);
            flusso
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n\xff\xfe\xfd",
                )
                .unwrap();
        });
        let errore = controlla_da(&base, v(0, 1, 0), Duration::from_secs(5), 4096)
            .expect_err("un corpo non UTF-8 deve dare errore");
        filo.join().unwrap();
        assert!(errore.contains("non è testo"), "{errore}");
    }
}
