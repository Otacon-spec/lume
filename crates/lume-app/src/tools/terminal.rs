//! Terminale nativo: sessioni PTY reali, lifecycle lazy e modello VT condiviso.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::TryRecvError;

use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Element, Length, Padding};
use lume_terminal::{TerminalEvent, TerminalPoint, TerminalScroll, TerminalSession, TerminalSize};
use lume_ui::icons::Icon;
use lume_ui::styles;
use lume_ui::tool::{Tool, ToolCtx, ToolEvent, ToolId, ToolKind, ToolRequest};

use crate::environments::ShellSpec;
use crate::terminal_widget::TerminalView;

pub const ID: ToolId = ToolId("terminal");
const DEFAULT_SIZE: TerminalSize = TerminalSize::new(24, 80);
/// Il dock senza sessioni. Dice come aprirne una: il terminale non parte da
/// solo, e un «Avvio terminale…» senza avvio era un testo che mentiva.
const DOCK_VUOTO: &str = "Nessun terminale — premi +";
/// Righe di storia tenute a dock chiuso (valore proposto nel piano
/// «Leggerezza on-demand»). ATTENZIONE: È una perdita: la storia oltre il limite non
/// torna alla riapertura. 1.000 perché sotto la cache di alacritty
/// (`MAX_CACHE_SIZE`) la memoria non rientrerebbe comunque.
const SCROLLBACK_DOCK_CHIUSO: usize = 1_000;

#[derive(Debug, Clone)]
pub enum Event {
    /// Una sessione ha qualcosa da drenare. **Non** avvia shell: può essere il
    /// residuo di una sessione già chiusa (`Notify` conserva un permesso).
    Tick,
    NewSession,
    /// Esegue un comando in una scheda nuova (widget «Esegui»).
    Run(RunSpec),
    Select(u64),
    Close(u64),
    Kill(u64),
    /// Ferma la funzionalità: chiude **tutte** le sessioni, PTY comprese.
    ///
    /// È `Close`-all e non `Kill`-all: `Kill` lascia la scheda con «Processo
    /// arrestato» e il tasto Riavvia, che a funzionalità ferma non avrebbe
    /// senso. Qui non deve restare niente.
    StopAll,
    Restart(u64),
    SetRoot(Option<PathBuf>),
    /// Shell (ed etichetta ambiente) per le prossime sessioni.
    SetShell {
        shell: ShellSpec,
        label: String,
    },
    Input(Vec<u8>),
    Resize(TerminalSize),
    Scroll(TerminalScroll),
    StartSelection(TerminalPoint),
    UpdateSelection(TerminalPoint),
    ClearSelection,
    OpenHyperlink(TerminalPoint),
}

/// Un comando da eseguire in una scheda propria (widget «Esegui», piano 17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec {
    pub label: String,
    pub program: std::path::PathBuf,
    pub args: Vec<String>,
    pub cwd: std::path::PathBuf,
    pub env: Vec<(String, String)>,
}

struct SessionState {
    id: u64,
    /// `Some` per una scheda di esecuzione: etichetta e Riavvia la usano.
    run: Option<RunSpec>,
    terminal: Option<TerminalSession>,
    exit_code: Option<u32>,
    stopped: bool,
    error: Option<String>,
}

impl SessionState {
    fn spawn(
        id: u64,
        shell: &ShellSpec,
        cwd: Option<&std::path::Path>,
        wakeup: Option<lume_terminal::Wakeup>,
        scrollback: usize,
    ) -> Self {
        Self::spawn_with(id, shell, None, cwd, wakeup, scrollback)
    }

    fn spawn_with(
        id: u64,
        shell: &ShellSpec,
        run: Option<RunSpec>,
        cwd: Option<&std::path::Path>,
        wakeup: Option<lume_terminal::Wakeup>,
        scrollback: usize,
    ) -> Self {
        let spawned = match (&run, shell.command()) {
            (Some(run), _) => TerminalSession::spawn_program_with_wakeup(
                &run.program,
                &run.args,
                &run.env,
                DEFAULT_SIZE,
                &run.cwd,
                wakeup,
            ),
            (None, None) => {
                TerminalSession::spawn_default_in_with_wakeup(DEFAULT_SIZE, cwd, wakeup)
            }
            (None, Some((program, args))) => TerminalSession::spawn_command_in_with_wakeup(
                &program,
                &args,
                DEFAULT_SIZE,
                cwd,
                wakeup,
            ),
        };
        match spawned {
            Ok(terminal) => {
                // Una sessione nata a dock chiuso parte col limite ridotto.
                // Al limite pieno non si chiama niente: è già il default
                // del modello (`SCROLLBACK_DEFAULT`).
                let error = (scrollback != lume_terminal::SCROLLBACK_DEFAULT)
                    .then(|| terminal.set_scrollback(scrollback).err())
                    .flatten()
                    .map(|error| error.to_string());
                Self {
                    id,
                    run,
                    terminal: Some(terminal),
                    exit_code: None,
                    error,
                    stopped: false,
                }
            }
            Err(error) => Self {
                id,
                run,
                terminal: None,
                exit_code: None,
                error: Some(error.to_string()),
                stopped: false,
            },
        }
    }

    fn status(&self) -> Option<String> {
        if let Some(error) = &self.error {
            Some(format!("Errore terminale: {error}"))
        } else if self.stopped {
            Some("Processo arrestato".into())
        } else {
            self.exit_code
                .map(|code| format!("Processo terminato ({code})"))
        }
    }
}

pub struct TerminalTool {
    sessions: Vec<SessionState>,
    active: Option<u64>,
    next_id: u64,
    cwd: Option<PathBuf>,
    /// Shell dei nuovi terminali (dall'ambiente selezionato).
    shell: ShellSpec,
    /// Etichetta dell'ambiente mostrata nella toolbar («Locale», «WSL · …»).
    shell_label: String,
    /// Segnale push verso l'app: output PTY, uscita processo, errori. Lo
    /// stesso `Notify` lo aspetta il ponte in `app.rs` (`terminal_wakeups`).
    wakeup: Arc<tokio::sync::Notify>,
    /// Limite della storia applicato alle sessioni: pieno a dock aperto,
    /// ridotto a dock chiuso.
    scrollback: usize,
}

impl TerminalTool {
    pub fn new() -> (Self, Arc<tokio::sync::Notify>) {
        let wakeup = Arc::new(tokio::sync::Notify::new());
        (
            Self {
                sessions: Vec::new(),
                active: None,
                next_id: 1,
                cwd: None,
                shell: ShellSpec::Default,
                shell_label: "Locale".into(),
                wakeup: wakeup.clone(),
                scrollback: lume_terminal::SCROLLBACK_DEFAULT,
            },
            wakeup,
        )
    }

    fn wakeup(&self) -> Option<lume_terminal::Wakeup> {
        let notify = self.wakeup.clone();
        Some(Arc::new(move || notify.notify_one()))
    }

    #[cfg(test)]
    pub fn shell_label(&self) -> &str {
        &self.shell_label
    }

    #[cfg(test)]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    #[cfg(test)]
    pub fn active_id(&self) -> Option<u64> {
        self.active
    }
    #[cfg(test)]
    pub fn active_process_id(&self) -> Option<u32> {
        self.active_session()
            .and_then(|session| session.terminal.as_ref())
            .and_then(TerminalSession::process_id)
    }

    /// Righe di storia della sessione attiva (solo per i test).
    #[cfg(test)]
    pub fn active_history_size(&self) -> Option<usize> {
        self.active_session()
            .and_then(|session| session.terminal.as_ref())
            .and_then(|terminal| terminal.history_size().ok())
    }

    /// Sessioni con un processo **ancora vivo**: è ciò che rende «Ferma»
    /// un gesto che può buttare via una build in corso, quindi da confermare.
    pub fn running_sessions(&self) -> usize {
        self.sessions
            .iter()
            .filter(|session| {
                session.exit_code.is_none() && !session.stopped && session.terminal.is_some()
            })
            .count()
    }

    fn active_session(&self) -> Option<&SessionState> {
        let id = self.active?;
        self.sessions.iter().find(|session| session.id == id)
    }

    fn active_session_mut(&mut self) -> Option<&mut SessionState> {
        let id = self.active?;
        self.sessions.iter_mut().find(|session| session.id == id)
    }

    fn spawn_session(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.sessions.push(SessionState::spawn(
            id,
            &self.shell,
            self.cwd.as_deref(),
            self.wakeup(),
            self.scrollback,
        ));
        self.active = Some(id);
        id
    }

    /// Nuova scheda che esegue `run`; restituisce il suo id.
    fn spawn_run(&mut self, run: RunSpec) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.sessions.push(SessionState::spawn_with(
            id,
            &self.shell,
            Some(run),
            self.cwd.as_deref(),
            self.wakeup(),
            self.scrollback,
        ));
        self.active = Some(id);
        id
    }

    fn remove_session(&mut self, id: u64) {
        self.sessions.retain(|session| session.id != id);
        if self.active == Some(id) {
            self.active = self.sessions.last().map(|session| session.id);
        }
    }

    #[allow(clippy::indexing_slicing)] // position viene da iter().position() sullo stesso Vec
    fn close_session(&mut self, id: u64) -> bool {
        let Some(position) = self.sessions.iter().position(|session| session.id == id) else {
            return false;
        };
        let session = &mut self.sessions[position];
        if session.exit_code.is_none()
            && let Some(terminal) = &session.terminal
            && let Err(error) = terminal.shutdown()
        {
            session.error = Some(error.to_string());
            return false;
        }
        self.remove_session(id);
        self.sessions.is_empty()
    }

    fn kill_session(&mut self, id: u64) {
        let Some(session) = self.sessions.iter_mut().find(|session| session.id == id) else {
            return;
        };
        if session.exit_code.is_some() {
            session.terminal = None;
            session.stopped = true;
            return;
        }
        let Some(terminal) = &session.terminal else {
            return;
        };
        match terminal.shutdown() {
            Ok(()) => {
                session.terminal = None;
                session.exit_code = None;
                session.error = None;
                session.stopped = true;
            }
            Err(error) => session.error = Some(error.to_string()),
        }
    }

    #[allow(clippy::indexing_slicing)] // position viene da iter().position() sullo stesso Vec
    fn restart_session(&mut self, id: u64) {
        if let Some(position) = self.sessions.iter().position(|session| session.id == id) {
            let run = self.sessions[position].run.clone();
            self.sessions[position] = SessionState::spawn_with(
                id,
                &self.shell,
                run,
                self.cwd.as_deref(),
                self.wakeup(),
                self.scrollback,
            );
            self.active = Some(id);
        }
    }

    fn drain_events(&mut self) {
        for session in &mut self.sessions {
            let Some(terminal) = &session.terminal else {
                continue;
            };
            for _ in 0..8 {
                match terminal.try_recv() {
                    Ok(TerminalEvent::Output) => {}
                    Ok(TerminalEvent::Exited(code)) => session.exit_code = Some(code),
                    Ok(TerminalEvent::Error(error)) => session.error = Some(error),
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
        }
    }

    fn apply_to_active(
        &mut self,
        operation: impl FnOnce(&TerminalSession) -> Result<(), lume_terminal::TerminalError>,
    ) {
        let Some(session) = self.active_session_mut() else {
            return;
        };
        let Some(terminal) = &session.terminal else {
            return;
        };
        if let Err(error) = operation(terminal) {
            session.error = Some(error.to_string());
        }
    }
}

impl Default for TerminalTool {
    fn default() -> Self {
        Self::new().0
    }
}

impl Tool for TerminalTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Terminale"
    }

    fn icon(&self) -> Icon {
        Icon::Terminal
    }

    fn feature(&self) -> Option<&'static str> {
        Some("terminale")
    }

    /// Le sessioni **vive**, non quelle in elenco: una scheda con «Processo
    /// arrestato» non tiene più niente e non deve far credere il contrario.
    fn live_count(&self) -> Option<usize> {
        Some(self.running_sessions())
    }

    /// Finché una sessione tiene il PTY il suo lettore può ancora consegnare
    /// output, anche dopo `Exited`: con ConPTY l'ultimo messaggio di una shell
    /// che muore (l'errore di `wsl.exe`, per dire) arriva spesso **dopo**
    /// l'uscita. Il PTY si rilascia solo con «Arresta» o chiudendo la scheda;
    /// fino ad allora il ponte resta vivo, e un `notified()` in attesa non
    /// costa niente.
    fn needs_wakeups(&self) -> bool {
        self.sessions
            .iter()
            .any(|session| session.terminal.is_some())
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Dock
    }

    /// L'eccezione dichiarata del modello di attivazione: il terminale
    /// possiede le PTY e le tiene vive a dock chiuso. Qui si riduce solo la
    /// storia, l'unica parte ibernabile senza uccidere la shell.
    fn on_visibility_changed(&mut self, open: bool) -> Vec<ToolRequest> {
        let righe = if open {
            lume_terminal::SCROLLBACK_DEFAULT
        } else {
            SCROLLBACK_DOCK_CHIUSO
        };
        self.scrollback = righe;
        for session in &mut self.sessions {
            if let Some(terminal) = &session.terminal
                && let Err(error) = terminal.set_scrollback(righe)
            {
                session.error = Some(error.to_string());
            }
        }
        if !open {
            // Nessuno guarda il dock: il frame in cache è memoria senza
            // lettori. La prima `view` dopo la riapertura lo ricostruisce.
            for session in &self.sessions {
                if let Some(terminal) = &session.terminal {
                    terminal.release_snapshot_cache();
                }
            }
        }
        Vec::new()
    }

    #[allow(clippy::expect_used)] // active_session() è Some solo se active è Some
    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        let mut tabs = row![
            text(self.shell_label.as_str()).size(11.5).color(t.text),
            Space::new().width(6),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        for session in &self.sessions {
            let nome = session.run.as_ref().map_or_else(
                || format!("Terminale {}", session.id),
                |run| format!("▶ {}", run.label),
            );
            let label = if self.active == Some(session.id) {
                format!("{nome} ●")
            } else {
                nome
            };
            tabs = tabs.push(
                button(text(label).size(11).color(t.ui))
                    .padding(Padding::from([3, 7]))
                    .style(styles::hover_row(t.ui, t.hover, None, 3.0))
                    .on_press(ToolEvent::new(Event::Select(session.id))),
            );
            tabs = tabs.push(
                button(text("×").size(12).color(t.sec))
                    .padding(Padding::from([2, 5]))
                    .style(styles::hover_row(t.sec, t.hover, None, 3.0))
                    .on_press(ToolEvent::new(Event::Close(session.id))),
            );
        }
        if let Some(session) = self
            .active_session()
            .filter(|session| session.terminal.is_some() && session.exit_code.is_none())
        {
            tabs = tabs.push(
                button(text("■ Arresta").size(11).color(t.warn))
                    .padding(Padding::from([2, 7]))
                    .style(styles::hover_row(t.warn, t.hover, None, 3.0))
                    .on_press(ToolEvent::new(Event::Kill(session.id))),
            );
        }
        tabs = tabs.push(Space::new().width(Length::Fill)).push(
            button(text("+").size(14).color(t.accent))
                .padding(Padding::from([2, 8]))
                .style(styles::hover_row(t.accent, t.hover, None, 3.0))
                .on_press(ToolEvent::new(Event::NewSession)),
        );

        let body: Element<'a, ToolEvent> = if let Some(session) = self.active_session() {
            if let Some(terminal) = &session.terminal {
                match terminal.snapshot_cached() {
                    Ok(snapshot) => {
                        let selection = terminal.selected_text().ok().flatten();
                        TerminalView::new(snapshot, selection, t, ToolEvent::new).into()
                    }
                    Err(error) => text(error.to_string()).size(12).color(t.bad).into(),
                }
            } else {
                text(
                    session
                        .status()
                        .unwrap_or_else(|| "Terminale non disponibile".into()),
                )
                .size(12)
                .color(t.bad)
                .into()
            }
        } else {
            text(DOCK_VUOTO).size(12).color(t.muted).into()
        };

        let mut content = column![tabs, body].spacing(4).height(Length::Fill);
        if let Some(status) = self.active_session().and_then(SessionState::status) {
            let id = self.active.expect("sessione attiva");
            content = content.push(
                row![
                    text(status).size(11).color(t.warn),
                    Space::new().width(Length::Fill),
                    button(text("Riavvia").size(11).color(t.accent))
                        .on_press(ToolEvent::new(Event::Restart(id))),
                ]
                .align_y(Alignment::Center),
            );
        }

        // corpo del dock terminale su `inset` (prototipo)
        let inset = t.inset;
        container(content)
            .padding(Padding::from([5, 8]))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| styles::bg(inset))
            .into()
    }

    fn update(&mut self, event: &ToolEvent) -> Vec<ToolRequest> {
        let Some(event) = event.downcast::<Event>() else {
            return Vec::new();
        };
        match event {
            Event::Tick => self.drain_events(),
            Event::NewSession => {
                self.spawn_session();
            }
            Event::Run(run) => {
                self.spawn_run(run.clone());
            }
            Event::Select(id) => {
                if self.sessions.iter().any(|session| session.id == *id) {
                    self.active = Some(*id);
                }
            }
            Event::Close(id) => {
                if self.close_session(*id) {
                    return vec![ToolRequest::CloseSelf];
                }
            }
            Event::Kill(id) => self.kill_session(*id),
            Event::StopAll => {
                for id in self
                    .sessions
                    .iter()
                    .map(|session| session.id)
                    .collect::<Vec<_>>()
                {
                    self.close_session(id);
                }
                // `close_session` fallisce se lo shutdown della PTY dà errore:
                // in quel caso la sessione resta, e il conteggio lo dice.
                self.active = self.sessions.last().map(|session| session.id);
                if self.sessions.is_empty() {
                    return vec![ToolRequest::CloseSelf];
                }
            }
            Event::Restart(id) => self.restart_session(*id),
            Event::SetRoot(root) => self.cwd = root.clone(),
            Event::SetShell { shell, label } => {
                self.shell = shell.clone();
                self.shell_label = label.clone();
            }
            Event::Input(bytes) => self.apply_to_active(|terminal| terminal.write(bytes)),
            Event::Resize(size) => self.apply_to_active(|terminal| terminal.resize(*size)),
            Event::Scroll(scroll) => self.apply_to_active(|terminal| terminal.scroll(*scroll)),
            Event::StartSelection(point) => {
                self.apply_to_active(|terminal| terminal.start_selection(*point));
            }
            Event::UpdateSelection(point) => {
                self.apply_to_active(|terminal| terminal.update_selection(*point));
            }
            Event::ClearSelection => {
                self.apply_to_active(TerminalSession::clear_selection);
            }
            Event::OpenHyperlink(point) => {
                let url = self
                    .active_session()
                    .and_then(|session| session.terminal.as_ref())
                    .and_then(|terminal| terminal.hyperlink_at(*point).ok().flatten());
                if let Some(url) = url {
                    return vec![ToolRequest::OpenExternal(url)];
                }
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{Event, TerminalTool};
    use crate::environments::ShellSpec;
    use lume_ui::tool::{Tool, ToolEvent, ToolRequest};
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};

    fn process_exists(pid: u32) -> bool {
        #[cfg(target_os = "windows")]
        {
            let Ok(output) = Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH"])
                .output()
            else {
                return false;
            };
            let pid = pid.to_string();
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .any(|field| field == pid)
        }
        #[cfg(not(target_os = "windows"))]
        {
            Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .is_ok_and(|status| status.success())
        }
    }

    fn wait_until_process_is_gone(pid: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while process_exists(pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        !process_exists(pid)
    }

    #[test]
    fn set_shell_cambia_ambiente_senza_avviare_processi() {
        let mut tool = TerminalTool::new().0;
        let _ = tool.update(&ToolEvent::new(Event::SetShell {
            shell: ShellSpec::Wsl("Ubuntu".into()),
            label: "WSL · Ubuntu".into(),
        }));
        assert_eq!(tool.shell_label(), "WSL · Ubuntu");
        assert_eq!(tool.shell, ShellSpec::Wsl("Ubuntu".into()));
        assert_eq!(tool.session_count(), 0);
    }

    /// ATTENZIONE: Un risveglio non avvia shell. `Notify` conserva un permesso: il
    /// residuo di una sessione già chiusa arriva come `Tick` alla riapertura
    /// del dock, e se `Tick` creasse la prima sessione partirebbe una shell
    /// che nessuno ha chiesto. La prima shell parte solo col «+».
    #[test]
    fn un_risveglio_non_avvia_shell_e_il_piu_si() {
        let mut tool = TerminalTool::new().0;
        Tool::update(&mut tool, &ToolEvent::new(Event::Tick));
        assert_eq!(tool.session_count(), 0);

        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        assert_eq!(tool.session_count(), 1);

        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        assert_eq!(tool.session_count(), 2);
    }

    /// «Ferma» sulla funzionalità non deve lasciare in piedi niente: né
    /// sessioni, né PTY, né i thread che le leggono.
    #[test]
    fn fermare_il_terminale_chiude_ogni_sessione() {
        let mut tool = TerminalTool::new().0;
        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        assert_eq!(tool.session_count(), 2);
        assert_eq!(tool.running_sessions(), 2);

        let requests = Tool::update(&mut tool, &ToolEvent::new(Event::StopAll));

        assert_eq!(tool.session_count(), 0);
        assert_eq!(tool.live_count(), Some(0));
        assert!(
            matches!(requests.as_slice(), [ToolRequest::CloseSelf]),
            "senza sessioni il dock si chiude da solo: {requests:?}"
        );
    }

    /// La cache del frame serve a una vista che si vede: a dock nascosto non
    /// deve restare in memoria, e riaprire non la ricrea da sola (la ricrea
    /// la prima `view`).
    #[test]
    fn a_dock_nascosto_la_cache_dello_snapshot_si_rilascia() {
        let mut tool = TerminalTool::new().0;
        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));

        let terminal = tool.sessions[0].terminal.as_ref().unwrap();
        let visto = terminal.snapshot_cached().unwrap();
        assert!(terminal.snapshot_cache_held(), "la vista tiene il frame");
        drop(visto);

        let richieste = tool.on_visibility_changed(false);
        assert!(richieste.is_empty());
        let terminal = tool.sessions[0].terminal.as_ref().unwrap();
        assert!(
            !terminal.snapshot_cache_held(),
            "dock nascosto: niente cache"
        );

        let _ = tool.on_visibility_changed(true);
        let terminal = tool.sessions[0].terminal.as_ref().unwrap();
        assert!(
            !terminal.snapshot_cache_held(),
            "riaprire non ricostruisce il frame: lo fa la prima view"
        );
        assert!(terminal.snapshot_cached().is_ok());
        assert!(terminal.snapshot_cache_held());

        let _ = Tool::update(&mut tool, &ToolEvent::new(Event::StopAll));
    }

    /// Una scheda con «Processo arrestato» non tiene più niente: contarla
    /// farebbe chiedere una conferma per buttare via il nulla.
    #[test]
    fn una_sessione_arrestata_non_conta_come_viva() {
        let mut tool = TerminalTool::new().0;
        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        let id = tool.active_id().unwrap();
        assert_eq!(tool.running_sessions(), 1);

        Tool::update(&mut tool, &ToolEvent::new(Event::Kill(id)));

        assert_eq!(tool.session_count(), 1, "la scheda resta, per il Riavvia");
        assert_eq!(tool.running_sessions(), 0);
        assert_eq!(tool.live_count(), Some(0));
    }

    /// Dopo `Exited` il lettore PTY può avere ancora output da consegnare:
    /// il tool deve continuare a chiedere il ponte finché tiene il PTY, e
    /// smettere solo quando lo rilascia. Shell reale.
    #[test]
    fn il_ponte_serve_finche_il_pty_non_e_rilasciato() {
        let mut tool = TerminalTool::new().0;
        assert!(
            !tool.needs_wakeups(),
            "nessuna sessione: niente da svegliare"
        );
        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        let id = tool.active_id().unwrap();
        assert!(tool.needs_wakeups());
        Tool::update(
            &mut tool,
            &ToolEvent::new(Event::Input(b"exit 7\r".to_vec())),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        while tool.running_sessions() > 0 && Instant::now() < deadline {
            Tool::update(&mut tool, &ToolEvent::new(Event::Tick));
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            tool.sessions[0].exit_code,
            Some(7),
            "la shell doveva uscire"
        );
        assert_eq!(tool.running_sessions(), 0);
        assert!(
            tool.needs_wakeups(),
            "processo uscito ma PTY ancora in mano: il lettore può consegnare altro"
        );

        Tool::update(&mut tool, &ToolEvent::new(Event::Kill(id)));
        assert!(
            !tool.needs_wakeups(),
            "PTY rilasciato: il ponte può chiudersi"
        );
    }

    #[test]
    fn kill_preserves_a_restartable_session() {
        let mut tool = TerminalTool::new().0;
        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        let id = tool.active_id().unwrap();
        let pid = tool.active_process_id().unwrap();

        Tool::update(&mut tool, &ToolEvent::new(Event::Kill(id)));

        assert_eq!(tool.session_count(), 1);
        assert_eq!(
            tool.active_process_id(),
            None,
            "status={:?}",
            tool.active_session().and_then(|session| session.status())
        );
        assert!(wait_until_process_is_gone(pid));

        Tool::update(&mut tool, &ToolEvent::new(Event::Restart(id)));
        assert!(tool.active_process_id().is_some());
    }

    #[test]
    fn closing_the_last_session_closes_the_tool() {
        let mut tool = TerminalTool::new().0;
        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        let id = tool.active_id().unwrap();

        let requests = Tool::update(&mut tool, &ToolEvent::new(Event::Close(id)));

        assert_eq!(tool.session_count(), 0);
        assert!(matches!(
            requests.as_slice(),
            [lume_ui::tool::ToolRequest::CloseSelf]
        ));
    }

    #[test]
    fn dropping_multiple_sessions_leaves_no_child_process() {
        let mut tool = TerminalTool::new().0;
        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        let first = tool.active_process_id().unwrap();
        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        let second = tool.active_process_id().unwrap();

        drop(tool);

        assert!(wait_until_process_is_gone(first));
        assert!(wait_until_process_is_gone(second));
    }

    /// Chiudere il dock riduce la storia ma **non** tocca le shell: sono i
    /// processi dell'utente (una build in corso), e la regola delle risorse
    /// pesanti le esclude dallo spegnimento.
    #[test]
    fn chiudere_il_dock_riduce_lo_scrollback_e_lascia_vive_le_shell() {
        let mut tool = TerminalTool::new().0;
        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        let pid = tool.active_process_id().unwrap();

        let requests = tool.on_visibility_changed(false);
        assert!(requests.is_empty());
        assert_eq!(tool.scrollback, super::SCROLLBACK_DOCK_CHIUSO);
        assert_eq!(
            tool.running_sessions(),
            1,
            "la shell resta viva a dock chiuso"
        );
        assert_eq!(tool.active_process_id(), Some(pid));
        assert!(
            tool.sessions.iter().all(|session| session.error.is_none()),
            "ridurre lo scrollback non deve lasciare errori sulle schede"
        );

        // una shell riavviata a dock chiuso nasce col limite ridotto, senza errori
        let id = tool.active.unwrap();
        Tool::update(&mut tool, &ToolEvent::new(Event::Restart(id)));
        assert_eq!(tool.running_sessions(), 1);
        assert!(tool.sessions.iter().all(|session| session.error.is_none()));

        let _ = tool.on_visibility_changed(true);
        assert_eq!(tool.scrollback, lume_terminal::SCROLLBACK_DEFAULT);
    }

    /// `Tick` continua a drenare gli eventi finché la shell vive e anche
    /// quando esce: è così che `exit_code` si aggiorna e `running_sessions()`
    /// scende a zero (e con lui il ponte dei risvegli in `app.rs`).
    /// Shell reale: si chiede alla shell di default di uscire con codice 7.
    #[test]
    fn tick_drena_l_uscita_della_shell_e_aggiorna_il_codice() {
        let mut tool = TerminalTool::new().0;
        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        assert_eq!(tool.running_sessions(), 1);
        Tool::update(
            &mut tool,
            &ToolEvent::new(Event::Input(b"exit 7\r".to_vec())),
        );

        let deadline = Instant::now() + Duration::from_secs(15);
        while tool.running_sessions() > 0 && Instant::now() < deadline {
            Tool::update(&mut tool, &ToolEvent::new(Event::Tick));
            thread::sleep(Duration::from_millis(20));
        }

        assert_eq!(tool.running_sessions(), 0, "la shell doveva uscire");
        assert_eq!(tool.sessions[0].exit_code, Some(7));
        assert_eq!(tool.live_count(), Some(0));
    }

    /// Fa stampare 3.000 righe alla shell **reale** della sessione attiva, la
    /// fa uscire, e aspetta (drenando con `Tick`) che l'output sia arrivato
    /// tutto: shell uscita e storia ferma da un po'. Il thread lettore aggiorna
    /// il modello per conto suo, quindi l'uscita può arrivare prima
    /// dell'ultimo output: da qui la quiete.
    fn stampa_3000_righe_e_aspetta(tool: &mut TerminalTool) {
        #[cfg(target_os = "windows")]
        let comando: &[u8] = b"for /L %i in (1,1,3000) do @echo riga %i\r";
        #[cfg(not(target_os = "windows"))]
        let comando: &[u8] = b"seq 1 3000\r";
        Tool::update(tool, &ToolEvent::new(Event::Input(comando.to_vec())));
        Tool::update(tool, &ToolEvent::new(Event::Input(b"exit\r".to_vec())));

        let deadline = Instant::now() + Duration::from_secs(60);
        let mut ultima = tool.active_history_size();
        let mut da = Instant::now();
        while Instant::now() < deadline {
            Tool::update(tool, &ToolEvent::new(Event::Tick));
            let attuale = tool.active_history_size();
            if attuale != ultima {
                ultima = attuale;
                da = Instant::now();
            }
            if tool.running_sessions() == 0 && da.elapsed() >= Duration::from_millis(700) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("l'output della shell non si è fermato entro il timeout");
    }

    /// Chiudere il dock **taglia davvero** la storia di una sessione con
    /// centinaia di righe: è il test che fallisce se `on_visibility_changed`
    /// non chiama `set_scrollback`.
    #[test]
    fn chiudere_il_dock_taglia_la_storia_di_una_sessione_reale() {
        let mut tool = TerminalTool::new().0;
        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        stampa_3000_righe_e_aspetta(&mut tool);
        let prima = tool.active_history_size().unwrap();
        assert!(
            prima > 1000,
            "a dock aperto la storia supera 1.000 righe: {prima}"
        );

        let _ = tool.on_visibility_changed(false);

        assert_eq!(
            tool.active_history_size(),
            Some(super::SCROLLBACK_DOCK_CHIUSO)
        );
    }

    /// Una shell **nata** a dock chiuso (scheda nuova o «Riavvia») parte col
    /// limite ridotto: senza l'applicazione in `SessionState::spawn` la sua
    /// storia arriverebbe a ~3.000 righe.
    #[test]
    fn una_shell_nata_a_dock_chiuso_ha_lo_scrollback_ridotto() {
        let mut tool = TerminalTool::new().0;
        let _ = tool.on_visibility_changed(false);

        Tool::update(&mut tool, &ToolEvent::new(Event::NewSession));
        stampa_3000_righe_e_aspetta(&mut tool);
        assert_eq!(
            tool.active_history_size(),
            Some(super::SCROLLBACK_DOCK_CHIUSO),
            "scheda nuova a dock chiuso"
        );

        let id = tool.active.unwrap();
        Tool::update(&mut tool, &ToolEvent::new(Event::Restart(id)));
        stampa_3000_righe_e_aspetta(&mut tool);
        assert_eq!(
            tool.active_history_size(),
            Some(super::SCROLLBACK_DOCK_CHIUSO),
            "shell riavviata a dock chiuso"
        );
    }
}
