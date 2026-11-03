//! Ambienti di sviluppo reali: «Locale» più le distribuzioni WSL rilevate.
//!
//! Senza agent remoto l'unico effetto concreto ed economico della scelta
//! dell'ambiente è la shell con cui partono i nuovi terminali: Locale = shell
//! di default, WSL · <distro> = `wsl.exe -d <distro>`. Nessuna voce SSH finché
//! non esiste il backend.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use lume_store::{EnvironmentKind, EnvironmentProfile, LOCAL_ENVIRONMENT_ID};
use wait_timeout::ChildExt;

/// Prefisso degli id delle distro WSL (`wsl:<nome>`).
pub const WSL_ID_PREFIX: &str = "wsl:";

/// WSL è avviabile solo su Windows.
pub const WSL_SUPPORTED: bool = cfg!(windows);

/// Tempo massimo di `wsl.exe -l -q`.
#[cfg_attr(not(windows), allow(dead_code))]
pub const WSL_TIMEOUT: Duration = Duration::from_secs(2);

/// Un elenco di distro sta in poche centinaia di byte: oltre 64 KiB non è un
/// elenco e non lo si tiene in memoria.
#[cfg_attr(not(windows), allow(dead_code))]
pub const WSL_MAX_OUTPUT: usize = 64 * 1024;

/// Tempo concesso al lettore per consegnare quel che ha letto dopo che il
/// figlio è uscito (un nipote potrebbe aver ereditato la pipe).
const ATTESA_LETTORE: Duration = Duration::from_secs(1);

/// Shell con cui il terminale apre una nuova sessione.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellSpec {
    /// Shell di default del sistema (portable-pty `new_default_prog`).
    Default,
    /// `wsl.exe -d <distro>`.
    Wsl(String),
}

impl ShellSpec {
    /// Programma e argomenti per `CommandBuilder`; None = shell di default.
    pub fn command(&self) -> Option<(String, Vec<String>)> {
        match self {
            ShellSpec::Default => None,
            ShellSpec::Wsl(distro) => Some((
                "wsl.exe".to_string(),
                vec!["-d".to_string(), distro.clone()],
            )),
        }
    }
}

/// Profilo «Locale» (sempre presente, anche senza store).
pub fn local_profile() -> EnvironmentProfile {
    EnvironmentProfile {
        id: LOCAL_ENVIRONMENT_ID.into(),
        label: "Locale".into(),
        kind: EnvironmentKind::Local,
    }
}

/// Shell corrispondente a un profilo.
pub fn shell_for(profile: &EnvironmentProfile) -> ShellSpec {
    shell_for_with(profile, WSL_SUPPORTED)
}

/// Shell corrispondente a un profilo, con supporto WSL esplicito.
pub fn shell_for_with(profile: &EnvironmentProfile, wsl_supported: bool) -> ShellSpec {
    match profile.kind {
        EnvironmentKind::Wsl if wsl_supported => profile
            .id
            .strip_prefix(WSL_ID_PREFIX)
            .map(|distro| ShellSpec::Wsl(distro.to_string()))
            .unwrap_or(ShellSpec::Default),
        EnvironmentKind::Local | EnvironmentKind::Ssh | EnvironmentKind::Wsl => ShellSpec::Default,
    }
}

/// Dettaglio mostrato a destra nel popup: dice onestamente cosa cambia.
pub fn detail_for(profile: &EnvironmentProfile) -> &'static str {
    match profile.kind {
        EnvironmentKind::Local => "questo PC",
        EnvironmentKind::Wsl => "shell del terminale",
        EnvironmentKind::Ssh => "non disponibile",
    }
}

/// Unione «Locale» + profili dello store + distro rilevate, senza duplicati
/// (per id) e con Locale sempre per primo.
pub fn merge_profiles(
    stored: Vec<EnvironmentProfile>,
    detected: Vec<EnvironmentProfile>,
) -> Vec<EnvironmentProfile> {
    merge_profiles_for(stored, detected, WSL_SUPPORTED)
}

/// Unione dei profili con supporto WSL esplicito, utile anche nei test.
pub fn merge_profiles_for(
    stored: Vec<EnvironmentProfile>,
    detected: Vec<EnvironmentProfile>,
    wsl_supported: bool,
) -> Vec<EnvironmentProfile> {
    let mut out = vec![local_profile()];
    for profile in stored.into_iter().chain(detected) {
        if profile.kind == EnvironmentKind::Ssh {
            continue; // nessun backend SSH: non si mostra (zero-mock)
        }
        if profile.kind == EnvironmentKind::Wsl && !wsl_supported {
            continue;
        }
        if !out.iter().any(|p| p.id == profile.id) {
            out.push(profile);
        }
    }
    out
}

/// Decodifica l'output di `wsl.exe -l -q` (UTF-16LE con BOM, una distro per
/// riga, eventuale «(Default)» da ignorare) in profili WSL.
#[cfg(any(windows, test))]
pub fn parse_wsl_list(raw: &[u8]) -> Vec<EnvironmentProfile> {
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .filter_map(|pair| <[u8; 2]>::try_from(pair).ok())
        .map(u16::from_le_bytes)
        .collect();
    let text = String::from_utf16_lossy(&units);
    text.lines()
        .map(|line| line.trim_matches(|c: char| c == '\u{feff}' || c.is_whitespace() || c == '\0'))
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_end_matches(" (Default)").trim())
        .filter(|name| !name.is_empty())
        .map(|name| EnvironmentProfile {
            id: format!("{WSL_ID_PREFIX}{name}"),
            label: format!("WSL · {name}"),
            kind: EnvironmentKind::Wsl,
        })
        .collect()
}

/// Legge `sorgente` fino alla fine tenendo al massimo `max` byte; quel che
/// eccede si legge e si scarta, così chi scrive dall'altra parte non si
/// blocca mai. Il `bool` dice se c'era più di `max`.
pub fn leggi_con_tetto(mut sorgente: impl Read, max: usize) -> (Vec<u8>, bool) {
    let mut dati = Vec::new();
    let mut troppo = false;
    let mut blocco = [0u8; 8192];
    loop {
        let letti = match sorgente.read(&mut blocco) {
            Ok(0) => break,
            Ok(letti) => letti,
            Err(errore) if errore.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let Some(pezzo) = blocco.get(..letti) else {
            break;
        };
        if !troppo && dati.len() + letti <= max {
            dati.extend_from_slice(pezzo);
        } else {
            troppo = true;
        }
    }
    (dati, troppo)
}

/// Esegue `comando` (senza stdin né stderr), attende al massimo `timeout` e
/// restituisce lo `stdout` se il processo esce in tempo con al più
/// `max_bytes` di output. Scaduto il tempo il processo è ucciso e atteso
/// (nessun orfano); un output più grande del tetto, un processo che non parte
/// o un errore di attesa danno `None`.
///
/// Lo `stdout` è svuotato da un thread dedicato mentre si attende: senza, un
/// figlio che scrive più della capienza della pipe resterebbe bloccato e
/// scadrebbe sempre. L'attesa è quella del sistema (`wait_timeout`), non un
/// ciclo con `sleep`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn run_capped(comando: &mut Command, timeout: Duration, max_bytes: usize) -> Option<Vec<u8>> {
    let mut figlio = comando
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let Some(stdout) = figlio.stdout.take() else {
        let _ = figlio.kill();
        let _ = figlio.wait();
        return None;
    };
    let (invia, ricevi) = mpsc::channel();
    let lettore = std::thread::Builder::new()
        .name("lume-lettore-processo".into())
        .spawn(move || {
            let _ = invia.send(leggi_con_tetto(stdout, max_bytes));
        });
    if lettore.is_err() {
        let _ = figlio.kill();
        let _ = figlio.wait();
        return None;
    }
    match figlio.wait_timeout(timeout) {
        Ok(Some(_stato)) => {
            let (dati, troppo) = ricevi.recv_timeout(ATTESA_LETTORE).ok()?;
            (!troppo).then_some(dati)
        }
        Ok(None) | Err(_) => {
            let _ = figlio.kill();
            let _ = figlio.wait();
            None
        }
    }
}

/// Esegue `wsl.exe -l -q` (solo Windows, senza finestra console, con tempo e
/// output limitati: `run_capped`). Da chiamare fuori dal thread UI e solo
/// quando l'utente apre il selettore degli ambienti.
#[cfg(windows)]
pub fn detect_wsl_distros() -> Vec<EnvironmentProfile> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut comando = Command::new("wsl.exe");
    comando.args(["-l", "-q"]).creation_flags(CREATE_NO_WINDOW);
    run_capped(&mut comando, WSL_TIMEOUT, WSL_MAX_OUTPUT)
        .map(|raw| parse_wsl_list(&raw))
        .unwrap_or_default()
}

#[cfg(not(windows))]
pub fn detect_wsl_distros() -> Vec<EnvironmentProfile> {
    Vec::new()
}

/// Stato della rilevazione delle distro WSL: parte alla prima apertura del
/// selettore degli ambienti, mai all'avvio, e una volta sola per sessione.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RilevazioneWsl {
    #[default]
    NonAvviata,
    InCorso,
    Fatta,
}

impl RilevazioneWsl {
    /// `true` solo nel passaggio `NonAvviata -> InCorso`: chi la chiama deve
    /// allora lanciare la rilevazione.
    pub fn avvia(&mut self) -> bool {
        if *self == Self::NonAvviata {
            *self = Self::InCorso;
            true
        } else {
            false
        }
    }

    /// L'esito è arrivato (anche se vuoto).
    pub fn finita(&mut self) {
        *self = Self::Fatta;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn utf16le(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn decodifica_l_output_utf16_di_wsl() {
        let raw = utf16le("\u{feff}Ubuntu-24.04 (Default)\r\nDebian\r\n\r\n");
        let profiles = parse_wsl_list(&raw);
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].id, "wsl:Ubuntu-24.04");
        assert_eq!(profiles[0].label, "WSL · Ubuntu-24.04");
        assert_eq!(profiles[0].kind, EnvironmentKind::Wsl);
        assert_eq!(profiles[1].id, "wsl:Debian");
    }

    #[test]
    fn shell_e_dettaglio_per_profilo() {
        let wsl = parse_wsl_list(&utf16le("Ubuntu\n")).remove(0);
        assert_eq!(
            shell_for_with(&wsl, true).command(),
            Some(("wsl.exe".into(), vec!["-d".into(), "Ubuntu".into()]))
        );
        assert_eq!(shell_for(&local_profile()), ShellSpec::Default);
        assert_eq!(detail_for(&wsl), "shell del terminale");
        assert_eq!(detail_for(&local_profile()), "questo PC");
    }

    #[test]
    fn merge_senza_duplicati_ne_ssh() {
        let stored = vec![
            local_profile(),
            EnvironmentProfile {
                id: "ssh:x".into(),
                label: "SSH · x".into(),
                kind: EnvironmentKind::Ssh,
            },
        ];
        let detected = parse_wsl_list(&utf16le("Ubuntu\nUbuntu\n"));
        let merged = merge_profiles_for(stored, detected, true);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, LOCAL_ENVIRONMENT_ID);
        assert_eq!(merged[1].id, "wsl:Ubuntu");
    }

    fn wsl_profile(name: &str) -> EnvironmentProfile {
        EnvironmentProfile {
            id: format!("{WSL_ID_PREFIX}{name}"),
            label: format!("WSL · {name}"),
            kind: EnvironmentKind::Wsl,
        }
    }

    #[test]
    fn senza_supporto_wsl_filtra_profili_salvati_e_non_avvia_shell_wsl() {
        let stored = vec![wsl_profile("Ubuntu"), local_profile()];
        let detected = vec![wsl_profile("Debian")];
        assert_eq!(
            merge_profiles_for(stored, detected, false),
            vec![local_profile()]
        );
        assert_eq!(
            shell_for_with(&wsl_profile("Ubuntu"), false),
            ShellSpec::Default
        );
    }

    #[test]
    fn con_supporto_wsl_conserva_profili_salvati_e_rilevati() {
        let merged = merge_profiles_for(
            vec![wsl_profile("Ubuntu")],
            vec![wsl_profile("Debian")],
            true,
        );
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[1].id, "wsl:Ubuntu");
        assert_eq!(merged[2].id, "wsl:Debian");
        assert_eq!(
            shell_for_with(&merged[1], true),
            ShellSpec::Wsl("Ubuntu".into())
        );
    }

    #[test]
    fn supporto_wsl_corrisponde_alla_piattforma() {
        assert_eq!(WSL_SUPPORTED, cfg!(windows));
    }

    /// Aiutante di sottoprocesso: non si esegue da solo (`#[ignore]`), lo
    /// lanciano i test qui sotto con `--ignored --exact`. Il comportamento lo
    /// sceglie `LUME_RUN_CAPPED_MODO`.
    #[test]
    #[ignore = "aiutante di sottoprocesso per run_capped"]
    fn aiutante_run_capped() {
        use std::io::Write;
        let modo = std::env::var("LUME_RUN_CAPPED_MODO").unwrap_or_default();
        let mut out = std::io::stdout().lock();
        match modo.as_str() {
            "breve" => {
                let _ = out.write_all(b"ciao\n");
            }
            "enorme" => {
                // 64 MiB: ben oltre la capienza di qualunque pipe
                let blocco = [b'a'; 65536];
                for _ in 0..1024 {
                    if out.write_all(&blocco).is_err() {
                        break;
                    }
                }
            }
            "lento" => {
                let _ = out.flush();
                drop(out);
                std::thread::sleep(Duration::from_millis(1000));
                if let Ok(percorso) = std::env::var("LUME_RUN_CAPPED_MARCATORE") {
                    let _ = std::fs::write(percorso, "sopravvissuto");
                }
            }
            _ => {}
        }
    }

    fn comando_aiutante(modo: &str) -> Command {
        let mut comando = Command::new(std::env::current_exe().unwrap());
        comando
            .args([
                "--ignored",
                "--exact",
                "environments::tests::aiutante_run_capped",
                "--nocapture",
            ])
            .env("LUME_RUN_CAPPED_MODO", modo);
        comando
    }

    #[test]
    fn leggi_con_tetto_non_accumula_oltre_il_tetto() {
        use std::io::Read;
        let dieci_mib = std::io::repeat(b'a').take(10 * 1024 * 1024);
        let (dati, troppo) = leggi_con_tetto(dieci_mib, 64 * 1024);
        assert!(troppo, "10 MiB sono più del tetto");
        assert!(dati.len() <= 64 * 1024, "letti {} byte", dati.len());
        assert!(
            dati.capacity() <= 2 * 64 * 1024 + 8192,
            "la memoria trattenuta ({}) deve restare vicina al tetto",
            dati.capacity()
        );
    }

    #[test]
    fn leggi_con_tetto_lascia_passare_quel_che_sta_nel_tetto() {
        let (dati, troppo) = leggi_con_tetto(&b"Ubuntu\nDebian\n"[..], 64 * 1024);
        assert!(!troppo);
        assert_eq!(dati, b"Ubuntu\nDebian\n");
        let (vuoto, troppo) = leggi_con_tetto(&b""[..], 8);
        assert!(!troppo);
        assert!(vuoto.is_empty());
    }

    #[test]
    fn un_processo_veloce_restituisce_l_output() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        let raw = run_capped(
            &mut comando_aiutante("breve"),
            Duration::from_secs(20),
            64 * 1024,
        )
        .expect("il processo esce in fretta");
        assert!(
            String::from_utf8_lossy(&raw).contains("ciao"),
            "output: {}",
            String::from_utf8_lossy(&raw)
        );
    }

    #[test]
    fn un_output_enorme_non_blocca_ne_si_accumula() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        let inizio = Instant::now();
        let esito = run_capped(
            &mut comando_aiutante("enorme"),
            Duration::from_secs(20),
            64 * 1024,
        );
        assert!(esito.is_none(), "64 MiB non sono un elenco di distro");
        assert!(
            inizio.elapsed() < Duration::from_secs(15),
            "il figlio non deve restare bloccato sulla pipe: {:?}",
            inizio.elapsed()
        );
    }

    #[test]
    fn un_processo_lento_e_fermato() {
        lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
        let cartella = tempfile::tempdir().unwrap();
        let marcatore = cartella.path().join("marcatore.txt");

        // controllo: lasciato in pace, il processo scrive il marcatore. Senza
        // questa prova l'assenza del marcatore più sotto non direbbe nulla.
        let stato = comando_aiutante("lento")
            .env("LUME_RUN_CAPPED_MARCATORE", &marcatore)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(stato.success());
        assert!(
            marcatore.exists(),
            "il processo lasciato finire scrive il marcatore"
        );
        std::fs::remove_file(&marcatore).unwrap();

        let inizio = Instant::now();
        let esito = run_capped(
            comando_aiutante("lento").env("LUME_RUN_CAPPED_MARCATORE", &marcatore),
            Duration::from_millis(200),
            64 * 1024,
        );
        assert!(esito.is_none(), "scaduto il tempo non c'è nessun elenco");
        assert!(
            inizio.elapsed() < Duration::from_secs(5),
            "{:?}",
            inizio.elapsed()
        );

        // se il processo fosse sopravvissuto scriverebbe il marcatore entro 1 s
        std::thread::sleep(Duration::from_millis(1800));
        assert!(
            !marcatore.exists(),
            "il processo lento doveva essere ucciso"
        );
    }

    #[test]
    fn un_programma_inesistente_da_none() {
        let esito = run_capped(
            &mut Command::new("lume-programma-che-non-esiste-7f3a"),
            Duration::from_secs(1),
            64 * 1024,
        );
        assert!(esito.is_none());
    }

    #[test]
    fn parse_wsl_list_regge_byte_dispari_e_surrogati_spaiati() {
        // un byte in più a fine buffer e un surrogato alto senza il basso:
        // nessun panic, nessun profilo inventato
        let mut raw = utf16le("Ubuntu\n");
        raw.push(0x41);
        assert_eq!(parse_wsl_list(&raw).len(), 1);
        let spaiato: Vec<u8> = [0xD800u16, u16::from(b'\n')]
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        for profilo in parse_wsl_list(&spaiato) {
            assert!(profilo.id.starts_with(WSL_ID_PREFIX));
        }
        assert!(parse_wsl_list(&[]).is_empty());
    }

    #[test]
    fn la_rilevazione_parte_una_volta_sola() {
        let mut stato = RilevazioneWsl::default();
        assert_eq!(stato, RilevazioneWsl::NonAvviata);
        assert!(stato.avvia(), "la prima richiesta avvia");
        assert_eq!(stato, RilevazioneWsl::InCorso);
        assert!(!stato.avvia(), "mentre è in corso non se ne avvia un'altra");
        stato.finita();
        assert_eq!(stato, RilevazioneWsl::Fatta);
        assert!(!stato.avvia(), "a rilevazione fatta non si ripete");
    }
}
