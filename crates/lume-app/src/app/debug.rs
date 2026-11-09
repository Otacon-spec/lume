//! Il debugger nell'app: gesti del pannello, eventi della sessione,
//! fiducia, istantanea per la vista (piano 17).

use super::*;
use crate::debug_mode::{DebugCommand, DebugEvent, Phase, Setup};
use crate::tools::debug::{DebugView, VIEW_CONSOLE_LINES};
use lume_ui::DebugAction;

impl Lume {
    /// La chiave della subscription che possiede la sessione, finché esiste.
    ///
    /// Non dipende dall'interruttore della funzionalità: «Ferma e spegni»
    /// manda `Stop` e spegne nello stesso giro, e se lo stream cadesse subito
    /// gdb morirebbe senza `disconnect`. La chiave sparisce solo con
    /// `Closed`, dopo la raccolta; l'avvio controlla già l'interruttore.
    pub(super) fn debug_spawn(&self) -> Option<crate::debug_mode::DebugSpawn> {
        self.debug.spawn.clone()
    }

    fn debug_root(&self) -> Option<PathBuf> {
        self.project_root.clone()
    }

    /// Rilegge `.lume/launch.toml` e cerca gdb, fuori dal thread della UI.
    pub(super) fn load_debug_setup(&mut self) -> Task<Message> {
        let Some(root) = self.debug_root() else {
            return self.notify_error("Debug: apri prima una cartella di progetto".into());
        };
        Task::perform(
            async move {
                let cartella = root.clone();
                let setup =
                    tokio::task::spawn_blocking(move || crate::debug_mode::load_setup(&cartella))
                        .await
                        .unwrap_or_else(|errore| {
                            Setup::vuoto(Err(lume_dap::LaunchError::Unreadable(errore.to_string())))
                        });
                (root, setup)
            },
            |(root, setup)| Message::DebugSetup(root, setup),
        )
    }

    pub(super) fn debug_setup_loaded(&mut self, root: PathBuf, setup: Setup) -> Task<Message> {
        if self.debug_root().as_deref() != Some(root.as_path()) {
            return Task::none();
        }
        self.debug.setup_root = Some(root);
        let numero = setup.configs.as_ref().map_or(0, Vec::len);
        if self.debug.selected_config >= numero {
            self.debug.selected_config = 0;
        }
        self.debug.setup = Some(setup);
        self.sync_voci_esegui();
        self.sync_maven_tool();
        self.sync_debug_tool();
        Task::none()
    }

    pub(super) fn debug_event(&mut self, epoch: u64, event: DebugEvent) -> Task<Message> {
        let avviso = self.debug.apply(epoch, event);
        self.sync_debug_tool();
        self.refresh_debug_marks();
        match avviso {
            Some(testo) if testo.starts_with("Debug") => self.notify_error(testo),
            Some(testo) => self.notify(NoticeKind::Info, testo),
            None => Task::none(),
        }
    }

    pub(super) fn debug_snippet_loaded(
        &mut self,
        epoch: u64,
        snippet: Option<crate::debug_mode::Snippet>,
        path: PathBuf,
        line: i64,
    ) -> Task<Message> {
        if epoch != self.debug.epoch {
            return Task::none();
        }
        let scelto = self
            .debug
            .history
            .current()
            .and_then(|stop| stop.frames.get(self.debug.selected_frame))
            .is_some_and(|frame| {
                frame.path.as_deref() == Some(path.as_path()) && frame.line == line
            });
        if scelto {
            self.debug.snippet = snippet;
            self.sync_debug_tool();
        }
        Task::none()
    }

    /// Chiude la sessione: `disconnect` con terminazione, poi raccolta.
    pub(super) fn stop_debug(&mut self) -> Task<Message> {
        match self.debug.phase() {
            Phase::Ferma | Phase::Arresto => Task::none(),
            Phase::Avvio if self.debug.commands.is_none() => {
                // Nessun attore a cui parlare ancora: smettere di produrre la
                // subscription fa cadere lo stream e termina l'adattatore.
                self.debug.spawn = None;
                self.debug.phase = Some(Phase::Ferma);
                self.sync_debug_tool();
                self.notify(NoticeKind::Info, "Debug annullato".into())
            }
            _ => {
                if let Err(errore) = self.debug.send(DebugCommand::Stop) {
                    self.debug.spawn = None;
                    self.debug.phase = Some(Phase::Ferma);
                    self.sync_debug_tool();
                    return self.notify_error(format!("Debug: {errore}"));
                }
                self.debug.phase = Some(Phase::Arresto);
                self.sync_debug_tool();
                Task::none()
            }
        }
    }

    fn debug_send(&mut self, command: DebugCommand) -> Task<Message> {
        match self.debug.send(command) {
            Ok(()) => Task::none(),
            Err(errore) => self.notify_error(format!("Debug: {errore}")),
        }
    }

    /// Chiede le righe di sorgente del frame scelto e, se è dell'arresto
    /// corrente, scope e variabili.
    fn debug_request_frame(&mut self) -> Task<Message> {
        let Some(frame) = self
            .debug
            .history
            .current()
            .and_then(|stop| stop.frames.get(self.debug.selected_frame))
            .cloned()
        else {
            return Task::none();
        };
        self.debug.scopes.clear();
        self.debug.children.clear();
        self.debug.expanded.clear();
        self.debug.detail_error = None;
        let mut tasks = Vec::new();
        if let Some(live) = self.debug.selected_live_frame() {
            let id = live.id;
            tasks.push(self.debug_send(DebugCommand::Frame {
                frame_id: Some(id),
                path: None,
                line: frame.line,
            }));
        }
        match frame.path.clone() {
            Some(path) => {
                let epoch = self.debug.epoch;
                let line = frame.line;
                tasks.push(Task::perform(
                    async move {
                        let snippet = crate::debug_mode::read_snippet(&path, line).await;
                        (snippet, path)
                    },
                    move |(snippet, path)| Message::DebugSnippet(epoch, snippet, path, line),
                ));
            }
            None => self.debug.snippet = None,
        }
        self.sync_debug_tool();
        Task::batch(tasks)
    }

    fn debug_start(&mut self, trust_now: bool) -> Task<Message> {
        let Some(root) = self.debug_root() else {
            return self.notify_error("Debug: apri prima una cartella di progetto".into());
        };
        let Some(config) = self.debug.selected_config().cloned() else {
            return self.notify_error("Debug: nessuna configurazione in .lume/launch.toml".into());
        };
        let chiave = (root, config.name.clone());
        if trust_now {
            self.debug.trusted.insert(chiave.clone());
        }
        if !self.debug.trusted.contains(&chiave) {
            // D-DAP-6: avviare esegue codice del progetto, anche con gdb di
            // sistema. La fiducia vale fino alla chiusura di Lume.
            self.debug.asking_trust = true;
            self.sync_debug_tool();
            return Task::none();
        }
        self.debug.asking_trust = false;
        if let Err(errore) = self.debug.prepare_spawn() {
            self.sync_debug_tool();
            return self.notify_error(format!("Debug: {errore}"));
        }
        self.debug.console.clear();
        self.sync_debug_tool();
        self.refresh_debug_marks();
        self.notify(
            NoticeKind::Info,
            format!("Debug: avvio di «{}» con gdb", config.name),
        )
    }

    pub(super) fn debug_action(&mut self, action: DebugAction) -> Task<Message> {
        if matches!(action, DebugAction::Start | DebugAction::TrustAndStart)
            && !self.feature_is_enabled(crate::features::Feature::Debugger)
        {
            return self.notify_error("Il debugger è spento nel pannello Estensioni".into());
        }
        let task = match action {
            DebugAction::ReloadConfigs => self.load_debug_setup(),
            DebugAction::SelectConfig(index) => {
                if index < self.debug.configs().len() {
                    self.debug.selected_config = index;
                    self.debug.override_config = None;
                }
                Task::none()
            }
            DebugAction::Start => self.debug_start(false),
            DebugAction::TrustAndStart => self.debug_start(true),
            DebugAction::CancelTrust => {
                self.debug.asking_trust = false;
                self.notify(NoticeKind::Info, "Debug non avviato".into())
            }
            DebugAction::Continue => self.debug_send(DebugCommand::Continue),
            DebugAction::Pause => self.debug_send(DebugCommand::Pause),
            DebugAction::StepOver => self.debug_send(DebugCommand::Next),
            DebugAction::StepIn => self.debug_send(DebugCommand::StepIn),
            DebugAction::StepOut => self.debug_send(DebugCommand::StepOut),
            DebugAction::Stop => self.stop_debug(),
            DebugAction::HistoryPrevious => {
                if self.debug.history.select_previous() {
                    self.debug.selected_frame = 0;
                    self.debug_request_frame()
                } else {
                    Task::none()
                }
            }
            DebugAction::HistoryNext => {
                if self.debug.history.select_next() {
                    self.debug.selected_frame = 0;
                    self.debug_request_frame()
                } else {
                    Task::none()
                }
            }
            DebugAction::HistoryLatest => {
                let mut mosso = false;
                while self.debug.history.select_next() {
                    mosso = true;
                }
                if mosso {
                    self.debug.selected_frame = 0;
                    self.debug_request_frame()
                } else {
                    Task::none()
                }
            }
            DebugAction::SelectFrame(index) => {
                let esiste = self
                    .debug
                    .history
                    .current()
                    .is_some_and(|stop| index < stop.frames.len());
                if esiste && index != self.debug.selected_frame {
                    self.debug.selected_frame = index;
                    self.debug_request_frame()
                } else {
                    Task::none()
                }
            }
            DebugAction::ToggleVariable(reference) => {
                if self.debug.expanded.remove(&reference) {
                    self.debug.children.remove(&reference);
                    Task::none()
                } else {
                    self.debug.expanded.insert(reference);
                    self.debug_send(DebugCommand::Children(reference))
                }
            }
            DebugAction::Evaluate(expression) => {
                let frame_id = self.debug.selected_live_frame().map(|frame| frame.id);
                if frame_id.is_none() {
                    self.notify_error("Debug: si valuta solo con il programma in pausa".into())
                } else {
                    self.debug_send(DebugCommand::Evaluate {
                        expression,
                        frame_id,
                    })
                }
            }
            DebugAction::OpenSource { path, line } => self.open_file_task(
                path,
                crate::file_read::DopoApertura::Riga(usize::try_from(line).unwrap_or(1)),
            ),
            DebugAction::ClearConsole => {
                self.debug.console.clear();
                Task::none()
            }
            DebugAction::RemoveBreakpoint { path, line } => {
                self.toggle_debug_breakpoint(&path, i64::from(line))
            }
            DebugAction::MavenEsegui { goal, profili } => self.esegui_goal_maven(goal, profili),
        };
        self.sync_debug_tool();
        task
    }

    /// Alterna un breakpoint; con la sessione viva lo manda subito a gdb.
    pub(super) fn toggle_debug_breakpoint(&mut self, path: &Path, line: i64) -> Task<Message> {
        let da_mandare = self.debug.toggle_breakpoint(path, line);
        self.sync_debug_tool();
        self.refresh_debug_marks();
        match da_mandare {
            Some(file) if self.debug.commands.is_some() => {
                self.debug_send(DebugCommand::SetBreakpoints(file))
            }
            _ => Task::none(),
        }
    }

    /// Ricostruisce l'istantanea del pannello Debug.
    pub(super) fn sync_debug_tool(&mut self) {
        let debug = &self.debug;
        let setup_loaded = debug.setup.is_some() && debug.setup_root == self.project_root;
        let setup_problem = match &debug.setup {
            _ if self.project_root.is_none() => {
                Some("Apri una cartella di progetto per usare il debugger.".to_owned())
            }
            None => None,
            Some(setup) => match (&setup.configs, &setup.adapter) {
                (Err(errore), _) => Some(format!(
                    "{errore}. Le configurazioni di lancio vivono in {} nella radice del progetto.",
                    lume_dap::LAUNCH_FILE
                )),
                (Ok(_), None) => {
                    Some("gdb non trovato su PATH: installa GNU gdb 14 o successivo.".to_owned())
                }
                (Ok(_), Some(_)) => None,
            },
        };
        let trust_prompt = debug.asking_trust.then(|| {
            let programma = debug
                .selected_config()
                .map(|config| config.program.display().to_string())
                .unwrap_or_default();
            format!("Avviare il debug esegue {programma} con gdb. Ti fidi di questo progetto fino alla chiusura di Lume?")
        });
        let console_total = debug.console.len();
        let console: Vec<_> = debug
            .console
            .lines()
            .skip(console_total.saturating_sub(VIEW_CONSOLE_LINES))
            .cloned()
            .collect();
        let breakpoints = debug
            .breakpoints
            .iter()
            .flat_map(|(path, lines)| {
                lines
                    .iter()
                    .map(move |(line, state)| (path.clone(), *line, state.verified))
            })
            .collect();
        let program = debug
            .spawn
            .as_ref()
            .map(|spawn| spawn.config.name.clone())
            .filter(|_| debug.is_live());
        // Una sessione avviata dal widget «Esegui» (File corrente) non dipende
        // da .lume/launch.toml: il pannello mostra quella e basta.
        let (configs, selected_config, setup_problem) = match &debug.override_config {
            Some(config) => (
                vec![config.name.clone()],
                0,
                setup_problem.filter(|problema| problema.contains("gdb")),
            ),
            None => (
                debug
                    .configs()
                    .iter()
                    .map(|config| config.name.clone())
                    .collect(),
                debug.selected_config,
                setup_problem,
            ),
        };
        let view = DebugView {
            phase: debug.phase,
            configs,
            selected_config,
            setup_problem,
            setup_loaded,
            trust_prompt,
            supports_pause: true,
            position: debug.history.position(),
            at_latest: debug.history.is_at_latest(),
            stop: debug.history.current().cloned(),
            selected_frame: debug.selected_frame,
            snippet: debug.snippet.clone(),
            scopes: debug.scopes.clone(),
            children: debug.children.clone(),
            expanded: debug.expanded.clone(),
            detail_error: debug.detail_error.clone(),
            last_error: debug.last_error.clone(),
            exit_code: debug.exit_code,
            console,
            console_total,
            evaluations: debug.evaluations.iter().cloned().collect(),
            breakpoints,
            program,
        };
        if let Some(tool) = self.registry.get_mut(tools::debug::ID) {
            tool.update(&tools::debug::snapshot_event(view));
        }
    }

    /// Marker dell'editor: breakpoint, riga corrente, righe già fermate.
    pub(super) fn refresh_debug_marks(&mut self) {
        self.debug.editor_marks = self.debug.compute_editor_marks();
    }

    /// Il clic sul numero di riga alterna un breakpoint solo dopo che il
    /// pannello Debug è stato aperto in questo progetto: per chi non usa il
    /// debugger quel clic resta quello di sempre.
    pub(super) fn debug_gutter_enabled(&self) -> bool {
        self.feature_is_enabled(crate::features::Feature::Debugger)
            && self.project_root.is_some()
            && self.debug.setup_root == self.project_root
    }

    pub(super) fn click_breakpoint_marker(
        &mut self,
        group: crate::editor_groups::GroupId,
        line: usize,
    ) -> Task<Message> {
        let Some(path) = self
            .groups
            .active_in(group)
            .and_then(|id| self.workspace.get(id))
            .and_then(|doc| doc.path().map(Path::to_path_buf))
        else {
            return self.notify_error("Breakpoint: il file non è ancora salvato su disco".into());
        };
        let riga = i64::try_from(line).unwrap_or(i64::MAX - 1) + 1;
        self.toggle_debug_breakpoint(&path, riga)
    }

    /// F9: breakpoint sulla riga del cursore.
    pub(super) fn toggle_breakpoint_at_cursor(&mut self) -> Task<Message> {
        if !self.feature_is_enabled(crate::features::Feature::Debugger) {
            return self.notify_error("Il debugger è spento nel pannello Estensioni".into());
        }
        let Some(doc) = self.active_doc().and_then(|id| self.workspace.get(id)) else {
            return self.notify_error("Breakpoint: nessun file aperto".into());
        };
        let Some(path) = doc.path().map(Path::to_path_buf) else {
            return self.notify_error("Breakpoint: il file non è ancora salvato su disco".into());
        };
        let rope = doc.rope();
        let head = doc.selection().head.min(rope.len_chars());
        let riga = i64::try_from(rope.char_to_line(head)).unwrap_or(i64::MAX - 1) + 1;
        let task = self.toggle_debug_breakpoint(&path, riga);
        let stato = if self.debug.has_breakpoint(&path, riga) {
            format!("Breakpoint alla riga {riga}")
        } else {
            format!("Breakpoint tolto dalla riga {riga}")
        };
        Task::batch([task, self.notify(NoticeKind::Info, stato)])
    }

    /// F5: con il programma in pausa continua; senza sessione avvia la
    /// configurazione scelta (aprendo il pannello se serve).
    pub(super) fn debug_continue_or_start(&mut self) -> Task<Message> {
        if !self.feature_is_enabled(crate::features::Feature::Debugger) {
            return self.notify_error("Il debugger è spento nel pannello Estensioni".into());
        }
        match self.debug.phase() {
            Phase::Fermo => self.debug_action(DebugAction::Continue),
            Phase::Ferma => {
                let mostra = self.show_dock(tools::debug::ID, false);
                if self.debug.setup.is_some() && self.debug.setup_root == self.project_root {
                    Task::batch([mostra, self.debug_action(DebugAction::Start)])
                } else {
                    mostra
                }
            }
            _ => self.notify(
                NoticeKind::Info,
                "Debug: il programma è già in esecuzione".into(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn spegnere_il_debugger_non_fa_cadere_la_sessione_prima_della_raccolta() {
        let (mut app, _) = Lume::new_without_store();
        app.debug.setup = Some(Setup {
            adapter: Some(PathBuf::from("gdb")),
            ..Setup::vuoto(lume_dap::launch::parse(
                "[[configurazione]]\nnome='p'\nadattatore='gdb'\nprogramma='${workspaceFolder}/p'\n",
                Path::new(if cfg!(windows) { "C:\\" } else { "/" }),
            ))
        });
        app.debug.prepare_spawn().unwrap();
        let epoch = app.debug.epoch;
        let _ = app.update(Message::Feature(
            crate::features::FeatureCommand::SetEnabled(crate::features::Feature::Debugger, false),
        ));
        assert!(
            app.debug_spawn().is_some(),
            "la subscription resta finché gdb non è raccolto"
        );
        let _ = app.update(Message::Debug(
            epoch,
            DebugEvent::Closed("adattatore chiuso".into()),
        ));
        assert!(app.debug_spawn().is_none());
        assert_eq!(app.debug.phase(), Phase::Ferma);
    }
}
