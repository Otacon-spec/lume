//! Il binario `jj` trovato su `PATH`.
//!
//! È l'unico punto in cui Lume lancia `jj` (lo verifica
//! `tests/eccezione_cli.rs`; decisione 5 del 29/09 e sezione «Eccezione: jj
//! CLI» di CONTRIBUTING.md). Ha la stessa forma di `lume_git::GitCli` (piano
//! Git, Task 2) ed è una copia consapevole: dipendere da `lume-git` porterebbe
//! gix in un crate che non ne ha bisogno (G9 del piano).
//!
//! - `jj` si cerca come si cercano i language server (voci di `PATH` assolute,
//!   in ordine), **senza sonda `--version`**: una sonda sarebbe un processo in
//!   più; un `jj` rotto si vede al primo uso, col suo messaggio;
//! - un comando parte solo da un'azione o da un'apertura del pannello e ha un
//!   flag di annullamento: `run` controlla `try_wait` ogni 25 ms **solo mentre
//!   il comando gira**. Lettori posseduti/stop/join; child diretto raccolto solo
//!   con esito confermato. Child incerti conservati, mai kill/retry dopo errore
//!   di osservazione. Non contenimento dei discendenti o teardown app completo.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::JjError;

/// Ogni quanto `run` controlla se il figlio è finito o se l'utente ha annullato.
const POLL: Duration = Duration::from_millis(25);
/// Quanto stdout/stderr si tiene di un comando. Il resto si legge e si scarta,
/// così il figlio non si blocca mai su una pipe piena.
const MAX_CAPTURE: usize = 64 * 1024;
/// Dopo l'uscita di `jj`, quanto si aspetta che i lettori svuotino le pipe (un
/// processo nipote, per esempio `git` o `ssh` del push, potrebbe tenerle aperte).
const DRAIN_WAIT: Duration = Duration::from_millis(500);
mod capture;
mod child_owner;
use capture::{Capture, abort_all, drain, finish_all};
use child_owner::{Admission, ChildOwner};
pub use child_owner::{JjQuarantineReport, collect_quarantine, quarantine_has_children};
/// Opzioni globali antepposte a ogni comando: mai un pager (bloccherebbe il
/// processo in attesa di un terminale) e mai i codici colore nell'output.
const GLOBAL_ARGS: [&str; 3] = ["--no-pager", "--color", "never"];
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Il `jj` da usare.
#[derive(Clone, Debug)]
pub struct JjCli {
    program: PathBuf,
}

/// Esito di un comando terminato (con successo o no).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JjOutput {
    pub stdout: String,
    pub stderr: String,
    /// `None` se il processo è stato terminato da un segnale.
    pub code: Option<i32>,
}

impl JjOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Il testo da mostrare quando il comando è fallito: prima lo stderr di
    /// `jj`, poi lo stdout, altrimenti il codice di uscita.
    pub fn failure_text(&self) -> String {
        let text = [self.stderr.trim(), self.stdout.trim()]
            .into_iter()
            .find(|text| !text.is_empty());
        match (text, self.code) {
            (Some(text), _) => text.to_owned(),
            (None, Some(code)) => format!("jj è uscito con codice {code}"),
            (None, None) => "jj è stato terminato da un segnale".to_owned(),
        }
    }

    /// Il testo di un comando riuscito: `jj` scrive gli esiti («Working copy
    /// now at: …», «Nothing changed.») su stderr, e i dati su stdout; si
    /// mostrano entrambi, non vuoti, uno per riga.
    pub fn message(&self) -> String {
        [self.stdout.trim(), self.stderr.trim()]
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl JjCli {
    /// Cerca `jj` nelle cartelle di `PATH`.
    pub fn locate() -> Option<Self> {
        let path = std::env::var_os("PATH")?;
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        Self::locate_in(&dirs)
    }

    /// Come `locate`, su un elenco dato (i test lo usano; niente `set_var`).
    /// Le voci vuote o relative si scartano, come fa il discovery dei
    /// language server.
    pub fn locate_in(dirs: &[PathBuf]) -> Option<Self> {
        let name = if cfg!(windows) { "jj.exe" } else { "jj" };
        dirs.iter()
            .filter(|dir| !dir.as_os_str().is_empty() && dir.is_absolute())
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
            .map(|program| Self { program })
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Esegue `jj --no-pager --color never <args>` in `dir` e aspetta la fine.
    ///
    /// Richiesta già revocata: nessuno spawn. Dopo ammissione, Cancelled solo
    /// con stop/reap confermato e raccolta dei lettori.
    /// Cleanup fallito/incerto è Cli, non successo mascherato da Cancelled.
    /// L'azione può aver avuto effetti: nessun rollback promesso. Un comando
    /// che esce con codice diverso da 0
    /// **non** è un errore di `run`: torna `Ok` con `success() == false`, e chi
    /// chiama sa quale messaggio dare.
    pub fn run(&self, dir: &Path, args: &[&str], cancel: &AtomicBool) -> Result<JjOutput, JjError> {
        if cancel.load(Ordering::Acquire) {
            return Err(JjError::Cancelled);
        }
        let admission = Admission::reserve().map_err(|error| JjError::Cli(error.to_string()))?;
        let mut command = Command::new(&self.program);
        command
            .args(GLOBAL_ARGS)
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Nessun editor e nessuna richiesta di password sul terminale che
            // non c'è (G5): `jj` e il `git` che chiama non possono bloccarsi.
            .env("JJ_EDITOR", "true")
            .env("GIT_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command
            .spawn()
            .map_err(|error| JjError::Cli(format!("{}: {error}", self.program.display())))?;
        let mut owner = ChildOwner::new(child, admission);
        let mut stdout = None;
        let mut stderr = None;
        let setup = (|| -> io::Result<()> {
            stdout = owner.child()?.stdout.take().map(drain).transpose()?;
            stderr = owner.child()?.stderr.take().map(drain).transpose()?;
            Ok(())
        })();
        if let Err(error) = setup {
            return Err(cleanup(
                JjError::Cli(error.to_string()),
                &mut owner,
                stdout,
                stderr,
            ));
        }
        let code = loop {
            // Precedenza conservata: uscita già osservata prima di cancel.
            match owner.observe() {
                Ok(Some(status)) => break status.code(),
                Ok(None) => {}
                Err(error) => {
                    return Err(cleanup(
                        JjError::Cli(error.to_string()),
                        &mut owner,
                        stdout,
                        stderr,
                    ));
                }
            }
            if cancel.load(Ordering::Acquire) {
                return Err(cleanup(JjError::Cancelled, &mut owner, stdout, stderr));
            }
            std::thread::sleep(POLL);
        };
        let (stdout, stderr) = finish_all(stdout, stderr, DRAIN_WAIT).map_err(|error| {
            JjError::Cli(format!(
                "raccolta pipe JJ: {error}; esito azione da verificare, nessun rollback"
            ))
        })?;
        Ok(JjOutput {
            stdout,
            stderr,
            code,
        })
    }
}
fn cleanup(
    primary: JjError,
    owner: &mut ChildOwner,
    stdout: Option<Capture>,
    stderr: Option<Capture>,
) -> JjError {
    // Attempt-all, senza nascondere un errore dietro l'altro o dietro Cancelled.
    let child = owner.stop();
    let readers = abort_all(stdout, stderr);
    match (child, readers) {
        (Ok(()), Ok(())) => primary,
        (child, readers) => {
            let mut detail = primary.to_string();
            if let Err(error) = child {
                detail.push_str(&format!("; stop child: {error}"));
            }
            if let Err(error) = readers {
                detail.push_str(&format!("; raccolta lettori: {error}"));
            }
            JjError::Cli(format!(
                "{detail}; esito azione da verificare, nessun rollback"
            ))
        }
    }
}

#[cfg(test)]
#[path = "cli/pipe_tests81.rs"]
mod pipe_tests81;

#[cfg(all(test, target_os = "linux"))]
#[path = "cli/native_cancel82.rs"]
mod native_cancel82;

#[cfg(all(test, unix))]
fn spawn_owner_fixture(script: &str, input: Stdio) -> io::Result<std::process::Child> {
    Command::new("/bin/sh")
        .args(["-c", script])
        .stdin(input)
        .spawn()
}
