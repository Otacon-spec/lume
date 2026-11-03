//! Log dei crash su file.
//!
//! In release Lume è un'app `windows_subsystem = "windows"`: non ha una
//! console, e il messaggio che il panic scrive su stderr non lo legge nessuno.
//! La finestra sparisce e l'utente non ha niente da allegare a una
//! segnalazione. Questo hook scrive il messaggio, il punto del codice e la
//! versione in `<dati>/logs/crash-<data>.log` prima di lasciare il posto
//! all'hook predefinito.
//!
//! Il file contiene **solo** quello: niente contenuto dei documenti. La
//! cartella dell'utente, ovunque compaia (nel messaggio, nel punto, nel
//! backtrace), diventa `~`; la build di release rimappa inoltre i percorsi
//! delle dipendenze (`tools/build_release_bundle.ps1`). È fatto per essere
//! allegato a una issue pubblica.
//!
//! ATTENZIONE: Il profilo release ha `strip = true`: il backtrace arriva senza simboli
//! ed è quasi muto. Il file e la riga del panic invece ci sono sempre, perché
//! `Location` è incisa nel binario a compile time e non dipende dai simboli.

use std::io::Write;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};

/// Oltre questo numero di file i più vecchi si cancellano: un crash in un
/// ciclo non deve riempire il disco del PC aziendale.
const MAX_LOG: usize = 20;

/// L'argomento che provoca un crash di prova (#12): serve a verificare, su
/// una macchina vera e sullo zip di release, che il log si scriva davvero.
pub const FLAG_PROVA: &str = "--prova-crash";
/// Il messaggio del crash di prova, riconoscibile nel file.
pub const MESSAGGIO_PROVA: &str = "crash di prova richiesto con --prova-crash";

/// `true` se il primo argomento dopo l'eseguibile è [`FLAG_PROVA`]. Solo il
/// primo: un file chiamato così più avanti nella riga non conta.
pub fn prova_richiesta(args: impl IntoIterator<Item = std::ffi::OsString>) -> bool {
    args.into_iter()
        .nth(1)
        .is_some_and(|argomento| argomento == FLAG_PROVA)
}

/// La cartella dei log, accanto al database delle impostazioni.
pub fn cartella() -> Option<PathBuf> {
    lume_store::application_data_dir()
        .ok()
        .map(|dati| dati.join("logs"))
}

/// Installa l'hook. Da chiamare come prima cosa in `main`.
pub fn installa() {
    let predefinito = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Un errore qui non deve mai diventare un secondo panic: si prova a
        // scrivere, e se non si riesce si passa comunque all'hook di sistema.
        if let Some(dir) = cartella() {
            let testo = anonimizza(&rapporto(info), cartella_utente().as_deref());
            let _ = scrivi(&dir, &testo, &adesso());
        }
        predefinito(info);
    }));
}

fn adesso() -> String {
    jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string()
}

/// Il testo del file. Separato dall'hook per poterlo provare senza un panic.
fn rapporto(info: &PanicHookInfo<'_>) -> String {
    let messaggio = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(messaggio non testuale)".to_owned());
    let punto = info
        .location()
        .map_or_else(|| "(sconosciuto)".to_owned(), ToString::to_string);
    let thread = std::thread::current()
        .name()
        .unwrap_or("(senza nome)")
        .to_owned();
    componi(&messaggio, &punto, &thread)
}

fn componi(messaggio: &str, punto: &str, thread: &str) -> String {
    format!(
        "Lume {versione} ({os}-{arch})\n\
         Thread: {thread}\n\
         Punto: {punto}\n\
         Messaggio: {messaggio}\n\n\
         Backtrace:\n{backtrace}\n",
        versione = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        backtrace = std::backtrace::Backtrace::force_capture(),
    )
}

/// La cartella dell'utente, da togliere dal rapporto: il file va allegato a
/// una issue pubblica, e `C:\Users\<nome>` dice chi l'ha scritto.
pub(crate) fn cartella_utente() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|valore| !valore.is_empty())
        .map(PathBuf::from)
}

/// Sostituisce `cartella` con `~` in tutto il testo, con le barre di Windows
/// e con quelle in avanti. Senza distinguere maiuscole e minuscole ASCII:
/// Windows non le distingue, e `C:\users\mario` è `C:\Users\Mario`. Una
/// cartella di tre caratteri o meno (`C:\`) non si sostituisce.
pub(crate) fn anonimizza(testo: &str, cartella: Option<&Path>) -> String {
    let Some(cartella) = cartella.and_then(Path::to_str).filter(|c| c.len() > 3) else {
        return testo.to_owned();
    };
    let mut esito = testo.to_owned();
    for forma in [cartella.to_owned(), cartella.replace('\\', "/")] {
        esito = sostituisci_senza_maiuscole(&esito, &forma, "~");
    }
    esito
}

/// `str::replace` che ignora le maiuscole ASCII. `to_ascii_lowercase`
/// conserva la lunghezza in byte, quindi gli indici trovati sul testo
/// abbassato valgono anche sull'originale.
fn sostituisci_senza_maiuscole(testo: &str, cerca: &str, con: &str) -> String {
    if cerca.is_empty() {
        return testo.to_owned();
    }
    let basso = testo.to_ascii_lowercase();
    let cerca_basso = cerca.to_ascii_lowercase();
    let mut esito = String::with_capacity(testo.len());
    let mut da = 0;
    while let Some(trovato) = basso.get(da..).and_then(|resto| resto.find(&cerca_basso)) {
        let inizio = da + trovato;
        esito.push_str(testo.get(da..inizio).unwrap_or_default());
        esito.push_str(con);
        da = inizio + cerca_basso.len();
    }
    esito.push_str(testo.get(da..).unwrap_or_default());
    esito
}

/// Scrive il rapporto e sfoltisce i log vecchi. Restituisce il percorso.
fn scrivi(dir: &Path, testo: &str, quando: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let percorso = dir.join(format!("crash-{quando}-{}.log", std::process::id()));
    let mut file = std::fs::File::create(&percorso)?;
    file.write_all(testo.as_bytes())?;
    file.sync_all()?;
    sfoltisci(dir, MAX_LOG);
    Ok(percorso)
}

/// Tiene i `massimo` log più recenti. I nomi iniziano con la data in forma
/// ordinabile, quindi l'ordine alfabetico è quello cronologico.
fn sfoltisci(dir: &Path, massimo: usize) {
    let Ok(voci) = std::fs::read_dir(dir) else {
        return;
    };
    let mut log: Vec<PathBuf> = voci
        .filter_map(Result::ok)
        .map(|voce| voce.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("crash-") && n.ends_with(".log"))
        })
        .collect();
    if log.len() <= massimo {
        return;
    }
    log.sort();
    for vecchio in log.iter().take(log.len() - massimo) {
        let _ = std::fs::remove_file(vecchio);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn cartella_prova(nome: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lume-crash-log-{nome}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn il_rapporto_dice_versione_punto_e_messaggio() {
        let testo = componi("indice fuori dai limiti", "src/app.rs:42:7", "main");
        assert!(testo.starts_with(&format!("Lume {}", env!("CARGO_PKG_VERSION"))));
        assert!(testo.contains("Punto: src/app.rs:42:7"));
        assert!(testo.contains("Messaggio: indice fuori dai limiti"));
        assert!(testo.contains("Thread: main"));
    }

    #[test]
    fn scrive_il_file_creando_la_cartella() {
        let dir = cartella_prova("scrive");
        let percorso = scrivi(&dir, "contenuto", "20260926-120000").unwrap();
        assert_eq!(std::fs::read_to_string(&percorso).unwrap(), "contenuto");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tiene_solo_i_log_piu_recenti_e_ignora_gli_altri_file() {
        let dir = cartella_prova("sfoltisci");
        std::fs::create_dir_all(&dir).unwrap();
        for giorno in 1..=5 {
            std::fs::write(dir.join(format!("crash-2026090{giorno}-000000-1.log")), "x").unwrap();
        }
        std::fs::write(dir.join("appunti.txt"), "non toccare").unwrap();
        sfoltisci(&dir, 2);
        let mut rimasti: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|v| v.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        rimasti.sort();
        assert_eq!(
            rimasti,
            [
                "appunti.txt",
                "crash-20260904-000000-1.log",
                "crash-20260905-000000-1.log"
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn la_cartella_utente_diventa_una_tilde_anche_con_maiuscole_diverse() {
        let casa = Path::new(r"C:\Users\Mario");
        let testo = "Punto: C:\\users\\mario\\.cargo\\registry\\src\\x.rs:1:2\n\
                     Messaggio: aprendo C:/Users/Mario/progetto/a.txt";
        assert_eq!(
            anonimizza(testo, Some(casa)),
            "Punto: ~\\.cargo\\registry\\src\\x.rs:1:2\n\
             Messaggio: aprendo ~/progetto/a.txt"
        );
    }

    #[test]
    fn senza_cartella_utente_il_testo_resta_com_e() {
        assert_eq!(
            anonimizza("Punto: src/app.rs:1:1", None),
            "Punto: src/app.rs:1:1"
        );
    }

    #[test]
    fn una_cartella_troppo_corta_non_si_sostituisce() {
        // `USERPROFILE=C:\` toglierebbe ogni lettera di unità dal rapporto
        assert_eq!(
            anonimizza(r"C:\progetto\a.rs", Some(Path::new(r"C:\"))),
            r"C:\progetto\a.rs"
        );
    }

    #[test]
    fn lo_sfoltimento_dei_crash_non_tocca_il_log_degli_errori() {
        let dir = cartella_prova("sfoltisci-errori");
        std::fs::create_dir_all(&dir).unwrap();
        for nome in ["errors.log", "errors.1.log", "errors.2.log"] {
            std::fs::write(dir.join(nome), "riga\n").unwrap();
        }
        for n in 0..25 {
            std::fs::write(dir.join(format!("crash-20260929-1200{n:02}-1.log")), "x").unwrap();
        }
        sfoltisci(&dir, MAX_LOG);
        let crash = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|voce| voce.file_name().to_string_lossy().starts_with("crash-"))
            .count();
        assert_eq!(crash, MAX_LOG);
        for nome in ["errors.log", "errors.1.log", "errors.2.log"] {
            assert!(dir.join(nome).exists(), "{nome} è stato cancellato");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn la_prova_parte_solo_col_flag_come_primo_argomento() {
        use std::ffi::OsString;
        let argv = |parti: &[&str]| parti.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(prova_richiesta(argv(&["lume.exe", "--prova-crash"])));
        assert!(!prova_richiesta(argv(&["lume.exe"])));
        assert!(!prova_richiesta(argv(&["lume.exe", "main.rs"])));
        assert!(!prova_richiesta(argv(&[
            "lume.exe",
            "main.rs",
            "--prova-crash"
        ])));
    }
}
