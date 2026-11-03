//! Ponte fra il pannello Jujutsu e `lume-jj`.
//!
//! Funzioni sincrone senza stato né iced: `app.rs` le esegue sui worker.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use lume_jj::{JjCli, JjCommand, JjError, JjSnapshot, JjWorkspace};
use lume_ui::tool::JjAction;

pub(crate) mod admission;
pub(crate) mod final_collection;
pub(crate) mod load_admission;

/// L'app possiede la cancellazione; worker e Task conservano solo l'Arc.
/// Drop segnala senza attendere, avviare thread o osservare PID.
pub(crate) struct CancelOnDrop(std::sync::Arc<AtomicBool>);

impl From<std::sync::Arc<AtomicBool>> for CancelOnDrop {
    fn from(cancel: std::sync::Arc<AtomicBool>) -> Self {
        Self(cancel)
    }
}
impl std::ops::Deref for CancelOnDrop {
    type Target = std::sync::Arc<AtomicBool>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

pub const LOG_LIMIT: usize = 30;
pub const NOT_FOUND: &str = "jj non è stato trovato nel PATH.";
pub const CANCELLED: &str = "Annullato.";
const ONE_LINE_MAX: usize = 240;

pub fn command_for(action: &JjAction) -> JjCommand {
    match action {
        JjAction::Describe(message) => JjCommand::Describe(message.clone()),
        JjAction::New(message) => JjCommand::New(message.clone()),
        JjAction::Push => JjCommand::Push,
    }
}

pub fn load(root: &Path, cancel: &AtomicBool) -> Result<JjSnapshot, String> {
    load_with(JjCli::locate(), root, cancel)
}

pub fn load_with(
    cli: Option<JjCli>,
    root: &Path,
    cancel: &AtomicBool,
) -> Result<JjSnapshot, String> {
    let result = cli.ok_or_else(|| NOT_FOUND.to_owned()).and_then(|cli| {
        JjWorkspace::new(cli, root.to_path_buf())
            .snapshot(LOG_LIMIT, cancel)
            .map_err(error_text)
    });
    finish_cleanup(result, lume_jj::collect_quarantine())
}

pub fn execute(root: &Path, action: &JjAction, cancel: &AtomicBool) -> Result<String, String> {
    execute_with(JjCli::locate(), root, action, cancel)
}

pub fn execute_with(
    cli: Option<JjCli>,
    root: &Path,
    action: &JjAction,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let result = cli.ok_or_else(|| NOT_FOUND.to_owned()).and_then(|cli| {
        JjWorkspace::new(cli, root.to_path_buf())
            .run_command(&command_for(action), cancel)
            .map_err(error_text)
    });
    finish_cleanup(result, lume_jj::collect_quarantine())
}

// Chiamato dal worker già posseduto dall'ammissione app, mai dal thread UI.
// Il report riguarda la quarantena globale JJ, comprese radici precedenti.
fn finish_cleanup<T>(
    result: Result<T, String>,
    report: lume_jj::JjQuarantineReport,
) -> Result<T, String> {
    if report.pending == 0 && report.uncertain == 0 {
        return result;
    }
    let status = match result {
        Ok(_) => "Operazione completata".to_owned(),
        Err(error) => error,
    };
    Err(format!(
        "{status}; raccolta risorse JJ incompleta (pendenti: {}, incerti: {}). Verificare l’esito prima di ripetere operazioni; nessun rollback.",
        report.pending, report.uncertain
    ))
}

pub fn is_cancelled(text: &str) -> bool {
    text == CANCELLED
}

fn error_text(error: JjError) -> String {
    match error {
        JjError::Cancelled => CANCELLED.to_owned(),
        other => other.to_string(),
    }
}

pub fn one_line(text: &str) -> String {
    let joined = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if joined.chars().count() <= ONE_LINE_MAX {
        return joined;
    }
    let mut cut: String = joined.chars().take(ONE_LINE_MAX).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;

    use lume_jj::{JjCli, JjCommand};
    use lume_ui::tool::JjAction;

    use super::*;

    fn cli_finto() -> (tempfile::TempDir, JjCli) {
        let dir = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) { "jj.exe" } else { "jj" };
        std::fs::write(dir.path().join(name), b"").unwrap();
        let cli = JjCli::locate_in(&[dir.path().to_path_buf()]).unwrap();
        (dir, cli)
    }

    fn root() -> PathBuf {
        std::env::temp_dir()
    }

    #[test]
    fn raccolta_incompleta_non_diventa_successo_o_cancelled() {
        let report = lume_jj::JjQuarantineReport {
            pending: 1,
            uncertain: 2,
            ..Default::default()
        };
        let success = finish_cleanup(Ok("eseguita".to_owned()), report).unwrap_err();
        assert!(success.contains("Operazione completata"));
        assert!(success.contains("pendenti: 1"));
        assert!(success.contains("incerti: 2"));
        let cancelled = finish_cleanup::<String>(Err(CANCELLED.into()), report).unwrap_err();
        assert!(!is_cancelled(&cancelled));
        assert!(cancelled.contains(CANCELLED));
        assert_eq!(finish_cleanup(Ok("ok"), Default::default()).unwrap(), "ok");
    }

    #[test]
    fn command_for_traduce_ogni_azione() {
        assert_eq!(
            command_for(&JjAction::Describe("m".to_owned())),
            JjCommand::Describe("m".to_owned())
        );
        assert_eq!(command_for(&JjAction::New(None)), JjCommand::New(None));
        assert_eq!(
            command_for(&JjAction::New(Some("a".to_owned()))),
            JjCommand::New(Some("a".to_owned()))
        );
        assert_eq!(command_for(&JjAction::Push), JjCommand::Push);
    }

    #[test]
    fn senza_jj_niente_processi_e_messaggio_chiaro() {
        let cancel = AtomicBool::new(false);
        assert_eq!(load_with(None, &root(), &cancel).unwrap_err(), NOT_FOUND);
        assert_eq!(
            execute_with(None, &root(), &JjAction::Push, &cancel).unwrap_err(),
            NOT_FOUND
        );
    }

    #[test]
    fn annullamento_riconoscibile_non_avvia_il_processo() {
        let (_dir, cli) = cli_finto();
        let cancel = AtomicBool::new(true);
        let load = load_with(Some(cli.clone()), &root(), &cancel).unwrap_err();
        let action = execute_with(Some(cli), &root(), &JjAction::Push, &cancel).unwrap_err();
        assert_eq!(load, CANCELLED);
        assert_eq!(action, CANCELLED);
        assert!(is_cancelled(&load));
        assert!(!is_cancelled("jj è uscito con codice 1"));
    }

    #[test]
    fn descrizione_vuota_rifiutata_prima_di_avviare_jj() {
        let (_dir, cli) = cli_finto();
        let error = execute_with(
            Some(cli),
            &root(),
            &JjAction::Describe("  ".to_owned()),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error, "Scrivi prima il messaggio della descrizione.");
    }

    #[test]
    fn programma_non_eseguibile_diventa_testo_di_errore() {
        let (_dir, cli) = cli_finto();
        let error =
            execute_with(Some(cli), &root(), &JjAction::Push, &AtomicBool::new(false)).unwrap_err();
        assert!(error.starts_with("impossibile eseguire jj:"), "{error}");
    }

    #[test]
    fn one_line_unisce_solo_righe_non_vuote() {
        assert_eq!(one_line(""), "");
        assert_eq!(one_line("  Fatto.  \n"), "Fatto.");
        assert_eq!(
            one_line("Changes to push:\n\n  Move bookmark main\n"),
            "Changes to push: · Move bookmark main"
        );
    }

    #[test]
    fn one_line_tronca_per_caratteri() {
        let result = one_line(&"è".repeat(300));
        assert_eq!(result.chars().count(), 241);
        assert!(result.ends_with('…'));
        assert_eq!(one_line(&"a".repeat(240)), "a".repeat(240));
    }
}
