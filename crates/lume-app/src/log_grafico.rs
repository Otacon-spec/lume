//! Log della grafica su file.
//!
//! iced e wgpu scrivono con la facciata `log` quale GPU e quale backend
//! (Vulkan, DX12, GL) hanno scelto e gli errori nel presentare un frame. Senza
//! un logger installato quei messaggi si perdono, e un difetto di disegno
//! visto solo sul PC di un utente non lascia traccia. Questo logger li scrive
//! in `<dati>/logs/grafica.log`, accanto a `errors.log`.
//!
//! - Solo le librerie della grafica (`PREFISSI`): avvisi ed errori, più le
//!   informazioni di `iced_wgpu`, che all'avvio descrivono adattatori, formato
//!   e modalità di presentazione. Nessun messaggio per frame in condizioni
//!   normali: si scrive quando qualcosa va storto.
//! - Stessa rotazione di `errors.log` (tre file, tetto noto), stessa
//!   anonimizzazione della cartella utente, nessuna riga identica ripetuta.
//! - Un errore di scrittura non diventa mai un secondo errore.

use std::sync::Mutex;

use crate::error_log::ErrorLog;

/// Nome base dei file: `grafica.log`, `grafica.1.log`, `grafica.2.log`.
const NOME: &str = "grafica";

/// L'elenco degli adattatori di iced è un `{:#?}` lungo qualche KiB.
const CORPO_MAX: usize = 16 * 1024;

/// Bersagli `log` della grafica. Un prefisso copre anche i sottomoduli
/// (`wgpu_hal::dx12`) e le varianti (`iced_wgpu`, `iced_winit`).
const PREFISSI: [&str; 6] = ["wgpu", "iced", "naga", "winit", "cryoglyph", "softbuffer"];

/// Variabili d'ambiente che cambiano il renderer: scritte nell'intestazione,
/// così da sapere con quale configurazione è stato ottenuto il log.
const VARIABILI: [&str; 4] = [
    "WGPU_BACKEND",
    "WGPU_POWER_PREF",
    "ICED_BACKEND",
    "ICED_PRESENT_MODE",
];

/// Installa il logger nella cartella dati. Da chiamare in `main`, prima che
/// iced crei la finestra. Se la cartella manca o un logger c'è già, lo dice
/// nella traccia di avvio e prosegue: il log è un aiuto, non un requisito.
pub fn installa() {
    let Some(file) = ErrorLog::predefinito() else {
        crate::startup::traccia("log grafica: cartella dati non disponibile");
        return;
    };
    let logger = LogGrafico::new(file);
    logger.scrivi(&intestazione(|nome| std::env::var(nome).ok()));
    match log::set_boxed_logger(Box::new(logger)) {
        Ok(()) => log::set_max_level(log::LevelFilter::Info),
        Err(errore) => crate::startup::traccia(&format!("log grafica non installato: {errore}")),
    }
}

struct LogGrafico {
    file: Mutex<ErrorLog>,
}

impl LogGrafico {
    fn new(file: ErrorLog) -> Self {
        Self {
            file: Mutex::new(file.con_nome(NOME, CORPO_MAX)),
        }
    }

    fn scrivi(&self, testo: &str) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.registra_ora(testo);
        }
    }
}

impl log::Log for LogGrafico {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        ammesso(metadata.level(), metadata.target())
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            self.scrivi(&format!(
                "{} {}: {}",
                record.level(),
                record.target(),
                record.args()
            ));
        }
    }

    fn flush(&self) {}
}

fn ammesso(livello: log::Level, bersaglio: &str) -> bool {
    let grafica = PREFISSI.iter().any(|p| bersaglio.starts_with(p));
    grafica
        && (livello <= log::Level::Warn
            || (livello <= log::Level::Info && bersaglio.starts_with("iced_wgpu")))
}

/// La prima riga di ogni avvio: versione, sistema e le variabili del
/// renderer impostate.
fn intestazione(leggi: impl Fn(&str) -> Option<String>) -> String {
    let mut riga = format!(
        "--- Lume {} avviato su {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    for nome in VARIABILI {
        if let Some(valore) = leggi(nome) {
            riga.push_str(&format!(" {nome}={valore}"));
        }
    }
    riga
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use log::Log as _;

    #[test]
    fn passano_avvisi_della_grafica_e_info_di_iced_wgpu() {
        assert!(ammesso(log::Level::Error, "wgpu_core::present"));
        assert!(ammesso(log::Level::Warn, "wgpu_hal::dx12"));
        assert!(ammesso(log::Level::Error, "iced_winit"));
        assert!(ammesso(log::Level::Info, "iced_wgpu::window::compositor"));
        assert!(!ammesso(log::Level::Info, "wgpu_core::device"));
        assert!(!ammesso(log::Level::Debug, "iced_wgpu::window::compositor"));
        assert!(!ammesso(log::Level::Error, "reqwest::connect"));
        assert!(!ammesso(log::Level::Warn, "cosmic_text::font"));
    }

    #[test]
    fn un_errore_di_presentazione_finisce_in_grafica_log() {
        let dir = tempfile::tempdir().unwrap();
        let logger = LogGrafico::new(ErrorLog::new(dir.path().to_owned(), None));
        logger.log(
            &log::Record::builder()
                .level(log::Level::Error)
                .target("iced_winit")
                .args(format_args!("Error Timeout when presenting surface."))
                .build(),
        );
        logger.log(
            &log::Record::builder()
                .level(log::Level::Info)
                .target("wgpu_core::device")
                .args(format_args!("scartato"))
                .build(),
        );
        let testo = std::fs::read_to_string(dir.path().join("grafica.log")).unwrap();
        assert!(
            testo.ends_with(" ERROR iced_winit: Error Timeout when presenting surface.\n"),
            "{testo}"
        );
        assert!(!testo.contains("scartato"), "{testo}");
        assert!(!dir.path().join("errors.log").exists());
    }

    #[test]
    fn l_elenco_lungo_degli_adattatori_non_si_tronca_a_2_kib() {
        let dir = tempfile::tempdir().unwrap();
        let logger = LogGrafico::new(ErrorLog::new(dir.path().to_owned(), None));
        let lungo = format!("Available adapters: {}fine", "x".repeat(6000));
        logger.log(
            &log::Record::builder()
                .level(log::Level::Info)
                .target("iced_wgpu::window::compositor")
                .args(format_args!("{lungo}"))
                .build(),
        );
        let testo = std::fs::read_to_string(dir.path().join("grafica.log")).unwrap();
        assert!(testo.contains("xfine\n"), "riga troncata");
    }

    #[test]
    fn l_intestazione_riporta_le_variabili_impostate() {
        let riga = intestazione(|nome| (nome == "WGPU_BACKEND").then(|| "dx12".to_owned()));
        assert!(riga.starts_with("--- Lume "), "{riga}");
        assert!(riga.ends_with(" WGPU_BACKEND=dx12"), "{riga}");
        assert!(!riga.contains("ICED_BACKEND"), "{riga}");
    }
}
