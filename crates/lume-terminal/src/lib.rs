//! Backend terminale nativo: PTY, processo shell ed emulazione VT.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use thiserror::Error;

mod model;
pub use model::*;

/// Chi va svegliato quando la sessione ha qualcosa da drenare: output,
/// uscita del processo, errore.
///
/// Una funzione e non un canale: il crate non deve sapere **come** l'app
/// aspetta. Con un `mpsc::Receiver` l'app parcheggiava un `recv()` bloccante
/// in `spawn_blocking`, che non si può interrompere: chiudendo e riaprendo il
/// dock il `recv()` vecchio restava vivo e si mangiava un risveglio, e il
/// terminale diventava «sordo» (N14).
pub type Wakeup = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl TerminalSize {
    pub const fn new(rows: u16, cols: u16) -> Self {
        Self {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

impl From<TerminalSize> for PtySize {
    fn from(size: TerminalSize) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
            pixel_width: size.pixel_width,
            pixel_height: size.pixel_height,
        }
    }
}

impl From<PtySize> for TerminalSize {
    fn from(size: PtySize) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
            pixel_width: size.pixel_width,
            pixel_height: size.pixel_height,
        }
    }
}

#[derive(Clone, Copy)]
struct GridDimensions {
    lines: usize,
    columns: usize,
}

impl From<TerminalSize> for GridDimensions {
    fn from(size: TerminalSize) -> Self {
        Self {
            lines: usize::from(size.rows).max(1),
            columns: usize::from(size.cols).max(1),
        }
    }
}

impl Dimensions for GridDimensions {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    Output,
    /// Il processo è terminato; il reader PTY può ancora consegnare output.
    /// Conservare la sessione e drenare anche gli eventi Output successivi.
    Exited(u32),
    Error(String),
}

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("errore PTY: {0}")]
    Pty(String),
    #[error("errore I/O terminale: {0}")]
    Io(#[from] std::io::Error),
    #[error("lock terminale avvelenato")]
    LockPoisoned,
}

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

#[derive(Clone)]
struct PtyResponder {
    writer: SharedWriter,
}

impl EventListener for PtyResponder {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(response) = event
            && let Ok(mut writer) = self.writer.lock()
        {
            let _ = writer.write_all(response.as_bytes());
            let _ = writer.flush();
        }
    }
}

struct SpawnGuard {
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
}

impl SpawnGuard {
    fn new(killer: Box<dyn ChildKiller + Send + Sync>) -> Self {
        Self {
            killer: Some(killer),
        }
    }

    #[allow(clippy::expect_used)] // invariante dello spawn guard: killer presente finché non disarmato
    fn disarm(mut self) -> Box<dyn ChildKiller + Send + Sync> {
        self.killer
            .take()
            .expect("spawn guard must own the child killer")
    }
}

impl Drop for SpawnGuard {
    fn drop(&mut self) {
        if let Some(killer) = self.killer.as_mut() {
            let _ = killer.kill();
        }
    }
}

fn process_already_exited(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::NotFound {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        // ERROR_INVALID_HANDLE: ConPTY ha già chiuso l'handle del processo.
        error.raw_os_error() == Some(6)
    }
    #[cfg(not(target_os = "windows"))]
    {
        // ESRCH: il processo non esiste più.
        error.raw_os_error() == Some(3)
    }
}

pub struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: SharedWriter,
    model: TerminalModel,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    events: Receiver<TerminalEvent>,
    output_pending: Arc<AtomicBool>,
    process_id: Option<u32>,
}

impl TerminalSession {
    pub fn spawn(command: CommandBuilder, size: TerminalSize) -> Result<Self, TerminalError> {
        Self::spawn_with_wakeup(command, size, None)
    }

    // L'allow sta qui e non su `spawn`: e' questo il corpo che indicizza il
    // buffer del reader. Restava sul chiamante da quando la slice 2 ha spostato
    // il codice, e clippy lo segnalava.
    #[allow(clippy::indexing_slicing)] // read <= buffer.len() (contratto di Read)
    pub fn spawn_with_wakeup(
        command: CommandBuilder,
        size: TerminalSize,
        wakeup: Option<Wakeup>,
    ) -> Result<Self, TerminalError> {
        #[cfg(unix)]
        let command = {
            let mut command = command;
            // The embedded emulator supports indexed 256-color and RGB SGR.
            // Keep a caller's explicit terminal choice, but provide useful
            // defaults when the parent application did not set them.
            if command.get_env("TERM").is_none_or(|value| value.is_empty()) {
                command.env("TERM", "xterm-256color");
            }
            if command
                .get_env("COLORTERM")
                .is_none_or(|value| value.is_empty())
            {
                command.env("COLORTERM", "truecolor");
            }
            command
        };
        let system = native_pty_system();
        let pair = system
            .openpty(size.into())
            .map_err(|error| TerminalError::Pty(error.to_string()))?;
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| TerminalError::Pty(error.to_string()))?;
        let process_id = child.process_id();
        let spawn_guard = SpawnGuard::new(child.clone_killer());
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| TerminalError::Pty(error.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| TerminalError::Pty(error.to_string()))?;
        let writer = Arc::new(Mutex::new(writer));
        let model = TerminalModel::with_writer(size, writer.clone());
        drop(pair.slave);

        let (events_tx, events) = mpsc::sync_channel(4);
        let output_pending = Arc::new(AtomicBool::new(false));
        let reader_tx = events_tx.clone();
        let reader_model = model.clone();
        let reader_pending = output_pending.clone();
        let reader_wakeup = wakeup.clone();
        thread::Builder::new()
            .name("lume-terminal-reader".into())
            .spawn(move || {
                let mut buffer = vec![0_u8; 16 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            if let Err(error) = reader_model.advance(&buffer[..read]) {
                                let _ = reader_tx.send(TerminalEvent::Error(error.to_string()));
                                break;
                            }
                            // Il wakeup sta **dentro** il guard del flag, come
                            // l'evento: senza, un risveglio per chunk da 16 KB
                            // su un canale illimitato farebbe girare `update()`
                            // una volta per lettura — sotto `cargo build` piu'
                            // del tick a 33 ms che questa via ha sostituito.
                            // Il flag torna `false` quando la UI drena
                            // (`try_recv`/`recv_timeout`), ed e' li' che si
                            // riapre la porta al risveglio successivo.
                            if !reader_pending.swap(true, Ordering::AcqRel) {
                                if reader_tx.try_send(TerminalEvent::Output).is_err() {
                                    reader_pending.store(false, Ordering::Release);
                                } else if let Some(w) = &reader_wakeup {
                                    // Solo a evento accodato: un risveglio
                                    // senza niente da drenare e' un giro a
                                    // vuoto della UI.
                                    w();
                                }
                            }
                        }
                        Err(error) => {
                            let _ = reader_tx.send(TerminalEvent::Error(error.to_string()));
                            break;
                        }
                    }
                }
            })
            .map_err(TerminalError::Io)?;
        let wait_wakeup = wakeup;
        thread::Builder::new()
            .name("lume-terminal-wait".into())
            .spawn(move || {
                let notify = |w: &Option<Wakeup>| {
                    if let Some(w) = w {
                        w();
                    }
                };
                match child.wait() {
                    Ok(status) => {
                        let _ = events_tx.send(TerminalEvent::Exited(status.exit_code()));
                        notify(&wait_wakeup);
                    }
                    Err(error) => {
                        let _ = events_tx.send(TerminalEvent::Error(error.to_string()));
                        notify(&wait_wakeup);
                    }
                }
            })
            .map_err(TerminalError::Io)?;

        let killer = spawn_guard.disarm();
        Ok(Self {
            master: pair.master,
            writer,
            model,
            killer: Mutex::new(killer),
            events,
            process_id,
            output_pending,
        })
    }

    pub fn spawn_default(size: TerminalSize) -> Result<Self, TerminalError> {
        Self::spawn_default_in(size, None)
    }

    pub fn spawn_default_in(size: TerminalSize, cwd: Option<&Path>) -> Result<Self, TerminalError> {
        Self::spawn_default_in_with_wakeup(size, cwd, None)
    }

    pub fn spawn_default_in_with_wakeup(
        size: TerminalSize,
        cwd: Option<&Path>,
        wakeup: Option<Wakeup>,
    ) -> Result<Self, TerminalError> {
        let mut command = CommandBuilder::new_default_prog();
        if let Some(cwd) = cwd {
            command.cwd(cwd);
        }
        Self::spawn_with_wakeup(command, size, wakeup)
    }

    /// Sessione con programma esplicito (es. `wsl.exe -d Ubuntu`) nella cwd data.
    pub fn spawn_command_in(
        program: &str,
        args: &[String],
        size: TerminalSize,
        cwd: Option<&Path>,
    ) -> Result<Self, TerminalError> {
        Self::spawn_command_in_with_wakeup(program, args, size, cwd, None)
    }

    pub fn spawn_command_in_with_wakeup(
        program: &str,
        args: &[String],
        size: TerminalSize,
        cwd: Option<&Path>,
        wakeup: Option<Wakeup>,
    ) -> Result<Self, TerminalError> {
        let mut command = CommandBuilder::new(program);
        command.args(args);
        if let Some(cwd) = cwd {
            command.cwd(cwd);
        }
        Self::spawn_with_wakeup(command, size, wakeup)
    }

    pub fn process_id(&self) -> Option<u32> {
        self.process_id
    }

    pub fn size(&self) -> Result<TerminalSize, TerminalError> {
        self.master
            .get_size()
            .map(Into::into)
            .map_err(|error| TerminalError::Pty(error.to_string()))
    }

    pub fn resize(&self, size: TerminalSize) -> Result<(), TerminalError> {
        self.master
            .resize(size.into())
            .map_err(|error| TerminalError::Pty(error.to_string()))?;
        self.model.resize(size)
    }

    pub fn set_scrollback(&self, lines: usize) -> Result<(), TerminalError> {
        self.model.set_scrollback(lines)
    }

    pub fn history_size(&self) -> Result<usize, TerminalError> {
        self.model.history_size()
    }

    pub fn write(&self, bytes: &[u8]) -> Result<(), TerminalError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    pub fn visible_text(&self) -> Result<Vec<String>, TerminalError> {
        self.model.visible_text()
    }

    pub fn snapshot(&self) -> Result<TerminalSnapshot, TerminalError> {
        self.model.snapshot()
    }

    /// Il frame condiviso e riusato finché nulla cambia (vedi
    /// `TerminalModel::snapshot_cached`).
    pub fn snapshot_cached(&self) -> Result<std::sync::Arc<TerminalSnapshot>, TerminalError> {
        self.model.snapshot_cached()
    }

    /// Rilascia il frame in cache; da chiamare quando il dock si nasconde.
    pub fn release_snapshot_cache(&self) {
        self.model.release_snapshot_cache();
    }

    pub fn snapshot_cache_held(&self) -> bool {
        self.model.snapshot_cache_held()
    }

    pub fn scroll(&self, scroll: TerminalScroll) -> Result<(), TerminalError> {
        self.model.scroll(scroll)
    }

    pub fn start_selection(&self, point: TerminalPoint) -> Result<(), TerminalError> {
        self.model.start_selection(point)
    }

    pub fn update_selection(&self, point: TerminalPoint) -> Result<(), TerminalError> {
        self.model.update_selection(point)
    }

    pub fn clear_selection(&self) -> Result<(), TerminalError> {
        self.model.clear_selection()
    }

    pub fn selected_text(&self) -> Result<Option<String>, TerminalError> {
        self.model.selected_text()
    }

    pub fn hyperlink_at(&self, point: TerminalPoint) -> Result<Option<String>, TerminalError> {
        self.model.hyperlink_at(point)
    }

    pub fn try_recv(&self) -> Result<TerminalEvent, TryRecvError> {
        let event = self.events.try_recv();
        if matches!(&event, Ok(TerminalEvent::Output)) {
            self.output_pending.store(false, Ordering::Release);
        }
        event
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<TerminalEvent, RecvTimeoutError> {
        let event = self.events.recv_timeout(timeout);
        if matches!(&event, Ok(TerminalEvent::Output)) {
            self.output_pending.store(false, Ordering::Release);
        }
        event
    }

    pub fn shutdown(&self) -> Result<(), TerminalError> {
        let result = self
            .killer
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .kill();
        match result {
            Ok(()) => Ok(()),
            Err(error) if process_already_exited(&error) => Ok(()),
            Err(error) => Err(TerminalError::Io(error)),
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
