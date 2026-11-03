//! Il workspace jj di un progetto: elenco dei cambiamenti, stato e le tre
//! azioni (describe, new, push). Ogni chiamata è uno o due processi `jj`, mai
//! di più, e parte solo quando chi chiama lo chiede.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::{JjCli, JjError, JjLogEntry, LOG_TEMPLATE, parse_log};

/// Ciò che il pannello mostra: i cambiamenti recenti e lo stato.
#[derive(Clone, Debug)]
pub struct JjSnapshot {
    pub entries: Vec<JjLogEntry>,
    /// L'output di `jj status`, senza l'a-capo finale.
    pub status: String,
}

/// Le azioni che l'utente può lanciare.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JjCommand {
    /// `jj describe --message=<testo>` sulla copia di lavoro.
    Describe(String),
    /// `jj new`, con `--message=<testo>` se il testo non è vuoto.
    New(Option<String>),
    /// `jj git push`: i bookmark già tracciati verso il remote di default.
    Push,
}

impl JjCommand {
    /// Gli argomenti dopo le opzioni globali. Il messaggio è **sempre** un solo
    /// argomento `--message=<testo>` (G6): un testo che comincia con `-` non
    /// diventa un'opzione e uno su più righe resta un argomento.
    pub fn args(&self) -> Vec<String> {
        match self {
            JjCommand::Describe(message) => {
                vec!["describe".to_owned(), format!("--message={message}")]
            }
            JjCommand::New(Some(message)) if !message.trim().is_empty() => {
                vec!["new".to_owned(), format!("--message={message}")]
            }
            JjCommand::New(_) => vec!["new".to_owned()],
            JjCommand::Push => vec!["git".to_owned(), "push".to_owned()],
        }
    }
}

/// Un workspace jj: il `jj` da usare e la cartella che contiene `.jj`.
#[derive(Clone, Debug)]
pub struct JjWorkspace {
    cli: JjCli,
    root: PathBuf,
}

impl JjWorkspace {
    pub fn new(cli: JjCli, root: PathBuf) -> Self {
        Self { cli, root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `jj log` (gli ultimi `limit` cambiamenti) e poi `jj status`. Un log
    /// fallito o illeggibile ferma tutto: lo status non parte.
    pub fn snapshot(&self, limit: usize, cancel: &AtomicBool) -> Result<JjSnapshot, JjError> {
        let limit = limit.to_string();
        let log = self.cli.run(
            &self.root,
            &[
                "log",
                "--no-graph",
                "-n",
                &limit,
                "--template",
                LOG_TEMPLATE,
            ],
            cancel,
        )?;
        if !log.success() {
            return Err(JjError::Failed(log.failure_text()));
        }
        let entries = parse_log(&log.stdout)?;
        let status = self.cli.run(&self.root, &["status"], cancel)?;
        if !status.success() {
            return Err(JjError::Failed(status.failure_text()));
        }
        Ok(JjSnapshot {
            entries,
            status: status.stdout.trim_end().to_owned(),
        })
    }

    /// Lancia un'azione. `Ok(testo)` è quello che `jj` ha risposto (o «Fatto.»);
    /// un codice di uscita diverso da 0 è `Err(Failed(testo di jj))`.
    pub fn run_command(&self, command: &JjCommand, cancel: &AtomicBool) -> Result<String, JjError> {
        if matches!(command, JjCommand::Describe(message) if message.trim().is_empty()) {
            return Err(JjError::Failed(
                "Scrivi prima il messaggio della descrizione.".to_owned(),
            ));
        }
        let args = command.args();
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.cli.run(&self.root, &refs, cancel)?;
        if !out.success() {
            return Err(JjError::Failed(out.failure_text()));
        }
        let text = out.message();
        Ok(if text.is_empty() {
            "Fatto.".to_owned()
        } else {
            text
        })
    }
}
