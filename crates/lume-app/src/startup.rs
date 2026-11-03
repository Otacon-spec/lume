//! Misura del tempo di avvio, dal `main` alla prima costruzione della vista.
//!
//! Il budget prestazionale del README fissa **< 100 ms alla finestra
//! interattiva**, e ogni modifica deve portare una misura prestazionale.
//! Fino a qui i 100 ms
//! erano un numero in un documento: non esisteva né un benchmark né una
//! guardia, e `metrics.rs` — nonostante il nome — sono costanti geometriche.
//!
//! **Perché sta nel binario e non in un bench.** Il costo d'avvio dipende da
//! quel che c'è sul disco di chi lancia il programma: sessione salvata, numero
//! di schede da riaprire, grandezza dei file. Un `cargo bench` misurerebbe una
//! sessione finta, cioè il caso in cui il difetto non si vede. Questa misura
//! gira sul programma vero, con la sessione vera.
//!
//! **E non costa niente quando è spenta.** Un `Instant::now()` all'ingresso e
//! un confronto atomico al primo frame: nessun thread, nessun timer, nessun
//! lavoro da inattivo. La stampa avviene una volta sola, e solo se
//! `LUME_STARTUP_TRACE` è impostata.
//!
//! Uso, da PowerShell (la build di release non ha console: vedi la sezione
//! «Misurare l'avvio» del README per reindirizzare lo stderr):
//! ```text
//! $env:LUME_STARTUP_TRACE = 1; cargo run --release -p lume-app
//! ```

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use std::{ffi::OsString, path::PathBuf};

/// L'istante d'ingresso in `main`, registrato prima di qualunque altra cosa.
static AVVIO: OnceLock<Instant> = OnceLock::new();
/// Il primo frame si annuncia una volta sola.
static PRIMO_FRAME_VISTO: AtomicBool = AtomicBool::new(false);

/// Segna l'inizio. Va chiamata come **prima** riga di `main`.
pub fn segna_avvio() {
    let _ = AVVIO.set(Instant::now());
}

/// Millisecondi trascorsi dall'ingresso in `main`, se l'avvio è stato segnato.
pub fn millis_da_avvio() -> Option<f64> {
    AVVIO
        .get()
        .map(|inizio| inizio.elapsed().as_secs_f64() * 1000.0)
}

/// Da chiamare in cima a `view()`: al **primo** passaggio riporta il tempo al
/// primo passaggio, poi non fa più nulla.
///
/// `view()` costruisce la vista prima del rendering e della presentazione.
/// Questo marker non prova che un frame sia presentato o che la finestra
/// risponda all'input. Il budget della finestra interattiva richiede una
/// misura separata; il prefisso del log resta compatibile con i runner.
pub fn segna_primo_frame() {
    // `swap` e non `load`+`store`: `view()` può essere chiamata da più di un
    // punto e la riga deve uscire una volta sola.
    if PRIMO_FRAME_VISTO.swap(true, Ordering::Relaxed) {
        return;
    }
    let Some(millis) = millis_da_avvio() else {
        return;
    };
    if !tracciamento_attivo() {
        return;
    }
    let budget = if millis < BUDGET_MS {
        "marker sotto soglia"
    } else {
        "FUORI BUDGET"
    };
    // stderr e non stdout: l'avvio non è output del programma.
    eprintln!(
        "lume: primo frame a {millis:.1} ms (soglia marker {BUDGET_MS:.0} ms; interattività non misurata) — {budget}"
    );
}

/// Il budget di avvio del README: < 100 ms alla finestra interattiva.
pub const BUDGET_MS: f64 = 100.0;

/// Restituisce il file passato dal sistema operativo all'avvio.
///
/// Windows usa il primo argomento dopo l'eseguibile quando Lume e' registrato
/// come editor predefinito. `OsString` conserva percorsi Unicode non
/// rappresentabili come UTF-8 su tutte le piattaforme supportate.
pub fn file_argument(args: impl IntoIterator<Item = OsString>) -> Option<PathBuf> {
    let argument = args.into_iter().nth(1)?;
    if argument.to_string_lossy().starts_with('-') {
        return None;
    }
    Some(PathBuf::from(argument))
}

/// `lume.exe --mcp-stdio …`: il client MCP lancia il ponte solo se il flag
/// occupa la prima posizione dopo l'eseguibile.
pub fn mcp_stdio_requested(args: impl IntoIterator<Item = OsString>) -> bool {
    args.into_iter()
        .nth(1)
        .is_some_and(|arg| arg == "--mcp-stdio")
}

/// Marca una tappa intermedia dell'avvio, con `LUME_STARTUP_TRACE` attiva.
///
/// Serve a **dividere** il tempo al primo frame: senza tappe si sa solo che il
/// budget è mancato, non da dove cominciare. Spenta, è una lettura di variabile
/// d'ambiente e nient'altro.
pub fn traccia(tappa: &str) {
    if !tracciamento_attivo() {
        return;
    }
    if let Some(millis) = millis_da_avvio() {
        eprintln!("lume:   {millis:>8.1} ms  {tappa}");
    }
}

/// Restituisce `true` una sola volta per ogni flag atomico ricevuto.
///
/// Usata nelle funzioni del ciclo eventi, così la misura registra la prima
/// chiamata senza leggere l'ambiente o stampare a ogni evento.
pub fn prima_volta(visto: &AtomicBool) -> bool {
    !visto.swap(true, Ordering::Relaxed)
}

/// Il nome della variabile d'ambiente che accende la traccia d'avvio. Sta in
/// una costante perché un test verifica che il README la documenti.
pub const VARIABILE_TRACCIA: &str = "LUME_STARTUP_TRACE";

fn tracciamento_attivo() -> bool {
    std::env::var_os(VARIABILE_TRACCIA).is_some_and(|valore| valore != "0")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

    /// L'associazione file di Windows avvia `lume.exe "<percorso>"`: se il
    /// primo argomento viene ignorato, la finestra compare ma il documento no.
    #[test]
    fn conserva_il_file_passato_dall_associazione_di_windows() {
        let file = PathBuf::from(r"C:\Progetti con spazi\guida è.md");
        let args = [OsString::from("lume.exe"), file.clone().into_os_string()];

        assert_eq!(file_argument(args), Some(file));
    }

    #[test]
    fn ignora_un_argomento_che_sembra_un_opzione() {
        let args = [OsString::from("lume.exe"), OsString::from("--verbose")];

        assert_eq!(file_argument(args), None);
    }

    #[test]
    fn senza_argomento_non_seleziona_un_file() {
        assert_eq!(file_argument([OsString::from("lume.exe")]), None);
    }

    /// Senza `segna_avvio` la misura non esiste e non si inventa un numero:
    /// meglio nessun dato che un dato falso.
    #[test]
    fn senza_avvio_segnato_non_c_e_misura() {
        // `AVVIO` è globale e altri test potrebbero averlo impostato; il caso
        // che conta è che la funzione non vada mai in panico.
        let _ = millis_da_avvio();
    }

    /// Segnato l'avvio, il tempo trascorso è un numero non negativo e
    /// monotono: se questo non valesse, ogni misura successiva sarebbe rumore.
    #[test]
    fn il_tempo_trascorso_e_monotono() {
        segna_avvio();
        let primo = millis_da_avvio().expect("avvio segnato");
        assert!(primo >= 0.0);
        std::thread::sleep(std::time::Duration::from_millis(5));
        let secondo = millis_da_avvio().expect("avvio segnato");
        assert!(
            secondo >= primo,
            "il tempo trascorso deve crescere: {primo} → {secondo}"
        );
        assert!(
            secondo >= 5.0,
            "dopo una pausa di 5 ms la misura deve accorgersene: {secondo}"
        );
    }

    /// `segna_avvio` chiamata due volte non azzera la misura: l'avvio è uno,
    /// e un reset silenzioso falserebbe il numero mostrato.
    #[test]
    fn segnare_lavvio_due_volte_non_azzera() {
        segna_avvio();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let prima = millis_da_avvio().expect("avvio segnato");
        segna_avvio();
        let dopo = millis_da_avvio().expect("avvio segnato");
        assert!(
            dopo >= prima,
            "il secondo segna_avvio non deve far ripartire il cronometro"
        );
    }

    #[test]
    fn prima_volta_e_vera_una_sola_volta() {
        let visto = AtomicBool::new(false);
        assert!(prima_volta(&visto));
        assert!(!prima_volta(&visto));
        assert!(!prima_volta(&visto));

        let altro = AtomicBool::new(false);
        assert!(prima_volta(&altro));
    }

    /// Il primo frame si annuncia una volta sola: `view()` viene chiamata a
    /// ogni ridisegno, e una riga per frame sarebbe l'opposto di una misura.
    #[test]
    fn il_primo_frame_si_annuncia_una_volta_sola() {
        PRIMO_FRAME_VISTO.store(false, Ordering::Relaxed);
        assert!(!PRIMO_FRAME_VISTO.swap(true, Ordering::Relaxed));
        assert!(PRIMO_FRAME_VISTO.swap(true, Ordering::Relaxed));
        // Chiamate ripetute non devono andare in panico né ristampare.
        segna_primo_frame();
        segna_primo_frame();
    }

    #[test]
    fn riconosce_il_ponte_mcp_solo_in_prima_posizione() {
        let args = |parole: &[&str]| parole.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(mcp_stdio_requested(args(&["lume.exe", "--mcp-stdio"])));
        assert!(mcp_stdio_requested(args(&[
            "lume.exe",
            "--mcp-stdio",
            "--project",
            r"C:\x"
        ])));
        assert!(!mcp_stdio_requested(args(&[
            "lume.exe",
            "file.rs",
            "--mcp-stdio"
        ])));
        assert!(!mcp_stdio_requested(args(&["lume.exe"])));
        assert_eq!(
            file_argument(args(&["lume.exe", "--mcp-stdio"])),
            None,
            "il flag non va interpretato come percorso da aprire"
        );
    }

    #[test]
    fn il_readme_documenta_la_variabile_di_traccia() {
        let readme = include_str!("../../../README.md").replace("\r\n", "\n");
        assert!(
            readme.contains("### Misurare l'avvio"),
            "il README deve avere la sezione «Misurare l'avvio»"
        );
        assert!(
            readme.contains(&format!("`{VARIABILE_TRACCIA}`")),
            "il README deve nominare `{VARIABILE_TRACCIA}`"
        );
    }
}
