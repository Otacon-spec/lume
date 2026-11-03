//! Pannello Git alimentato esclusivamente dal backend gitoxide.
//!
//! Segue il prototipo aggiornato: due gruppi comprimibili con casella a tre
//! stati, il clic sulla riga apre il diff e **solo** la casella mette in
//! stage, barra strumenti in cima, Amend in fondo.
//!
//! Cosa non si disegna, e perché — stessa regola di `MENUS` in `menu.rs`, «solo
//! comandi con backend disponibile»:
//! - **Anteprima diff**: con il modello d'interazione del prototipo il clic
//!   sulla riga apre già il diff, quindi il pulsante non avrebbe un'azione
//!   propria. Nel prototipo stesso non ha alcun gestore;
//! - **«Commit e push…» e l'ingranaggio opzioni**: `lume-git` non ha push.

use std::collections::BTreeSet;
use std::path::PathBuf;

use iced::widget::{Space, button, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use lume_git::{ChangeKind, CommitSummary, FileStatus};
use lume_ui::MONO;
use lume_ui::icons::Icon;
use lume_ui::styles;
use lume_ui::tool::{Tool, ToolCtx, ToolEvent, ToolId, ToolRequest};

use super::gitlog::relative_age;

pub const ID: ToolId = ToolId("git");

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub branch: Option<String>,
    pub local_branches: Vec<String>,
    /// Remote `origin` configurato, indipendentemente dall'upstream del branch.
    pub has_origin: bool,
    /// `None`: nessun merge; `Some(vuoto)`: merge senza conflitti residui.
    pub merge_conflicts: Option<Vec<PathBuf>>,
    pub has_upstream: bool,
    pub files: Vec<FileStatus>,
    pub commits: Vec<CommitSummary>,
    /// Messaggio del commit di HEAD: precompila la casella quando si attiva
    /// Amend. `None` se il repository non ha ancora commit.
    pub head_message: Option<String>,
}

/// I due gruppi del pannello. «File non versionati» è **untracked**, non
/// `ChangeKind::Added`: il secondo, sul lato index, è un file nuovo già in
/// stage, che è tracciato — vedi `FileStatus::is_untracked`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub enum Group {
    Changes,
    Unversioned,
}

impl Group {
    const ALL: [Group; 2] = [Group::Changes, Group::Unversioned];

    fn title(self) -> &'static str {
        match self {
            Self::Changes => "Modifiche",
            Self::Unversioned => "File non versionati",
        }
    }

    fn holds(self, file: &FileStatus) -> bool {
        match self {
            Self::Changes => !file.is_untracked(),
            Self::Unversioned => file.is_untracked(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    Run(lume_ui::tool::GitAction),
    ActionStarted(lume_ui::tool::GitAction),
    Cancel,
    ActionFinished(lume_ui::tool::GitAction, crate::git_actions::ActionReport),
    SetRoot(PathBuf),
    Loaded(Result<Snapshot, String>, bool),
    /// La casella di un file: mette in stage o toglie, secondo lo stato.
    ToggleFile(PathBuf),
    /// Il clic sulla riga: apre il diff, non tocca lo staging.
    OpenFile(PathBuf),
    /// La casella del gruppo: a tre stati, agisce su tutto il gruppo.
    ToggleGroup(Group),
    CollapseGroup(Group),
    SelectGroup(Group),
    ExpandAll,
    CollapseAll,
    /// Chiede conferma prima di buttare via le modifiche del gruppo scelto.
    AskDiscard,
    ConfirmDiscard,
    CancelDiscard,
    MessageChanged(String),
    ToggleAmend,
    Commit,
    Refresh,
    SelectCommit(usize),
    // ── Blame ────────────────────────────────────────────────────
    /// Apri il pannello blame per il file specificato.
    OpenBlame(PathBuf),
    /// Chiudi il pannello blame.
    CloseBlame,
    /// Risultato caricato dal worker asincrono.
    BlameLoaded(Result<Vec<lume_git::BlameLine>, String>, bool),
    // ── Stash ────────────────────────────────────────────────────
    /// Ricarica la lista degli stash.
    StashRefresh,
    /// Risultati della lista stash.
    StashListLoaded(Result<Vec<lume_git::StashEntry>, String>),
    /// Rimuove la voce stash@{index}.
    StashDrop(usize),
    /// Applica una voce (e, se `pop`, la rimuove solo dopo successo).
    StashApply {
        index: usize,
        pop: bool,
    },
    /// Annulla save/apply/pop in corso.
    CancelStashOperation,
    /// Mostra/nasconde il pannello stash.
    ToggleStashPanel,
    // ── Fetch ────────────────────────────────────────────────────
    /// Mostra o nasconde la scelta dei branch locali.
    ToggleBranchPicker,
    /// Cambia al branch locale selezionato.
    SwitchBranch(String),
    /// Annulla il cambio branch in corso.
    CancelBranchSwitch,
    /// Avvia il merge del branch locale selezionato.
    MergeBranch(String),
    /// Conclude un merge senza conflitti residui.
    MergeContinue,
    /// Chiede conferma prima di annullare il merge.
    AskMergeAbort,
    /// Conferma l'annullamento del merge.
    ConfirmMergeAbort,
    /// Chiude la conferma senza annullare.
    CancelMergeAbort,
    /// Annulla merge/continue/abort in corso.
    CancelMergeOperation,
}

pub struct GitTool {
    root: Option<PathBuf>,
    snapshot: Option<Snapshot>,
    message: String,
    busy: bool,
    error: Option<String>,
    selected_commit: Option<usize>,
    collapsed: BTreeSet<Group>,
    selected_group: Group,
    amend: bool,
    /// Path in attesa di conferma per «Ripristina». `Some` disegna la riga di
    /// conferma al posto della barra strumenti.
    pending_discard: Option<Vec<PathBuf>>,
    // ── Blame ───────────────────────────────────────────────────
    /// File per cui è aperto il pannello blame (side panel).
    blame_path: Option<PathBuf>,
    /// Righe del blame caricato.
    blame_lines: Vec<lume_git::BlameLine>,
    /// `true` se il file ha modifiche nel worktree (blame ≠ worktree attuale).
    blame_dirty: bool,
    blame_loading: bool,
    blame_error: Option<String>,
    // ── Stash ───────────────────────────────────────────────────
    stash_entries: Vec<lume_git::StashEntry>,
    stash_panel_open: bool,
    stash_error: Option<String>,
    stash_busy: bool,
    stash_notice: Option<String>,
    // ── Fetch ───────────────────────────────────────────────────
    fetch_busy: bool,
    pull_busy: bool,
    push_busy: bool,
    fetch_notice: Option<String>,
    branch_picker_open: bool,
    branch_switch_busy: bool,
    merge_busy: bool,
    merge_abort_confirmation: bool,
    merge_notice: Option<String>,
    action: Option<lume_ui::tool::GitAction>,
    notice: Option<crate::git_actions::ActionReport>,
    needs_merge: bool,
    /// `git` rilevato all'apertura della vista, senza processi di controllo.
    git_available: bool,
}

struct Command {
    icon: Icon,
    title: &'static str,
    event: Option<Event>,
}

impl GitTool {
    pub fn new() -> Self {
        Self {
            root: None,
            snapshot: None,
            message: String::new(),
            busy: false,
            error: None,
            selected_commit: None,
            collapsed: BTreeSet::new(),
            selected_group: Group::Changes,
            amend: false,
            pending_discard: None,
            blame_path: None,
            blame_lines: Vec::new(),
            blame_dirty: false,
            blame_loading: false,
            blame_error: None,
            stash_entries: Vec::new(),
            stash_panel_open: false,
            stash_error: None,
            stash_busy: false,
            stash_notice: None,
            fetch_busy: false,
            pull_busy: false,
            push_busy: false,
            fetch_notice: None,
            branch_picker_open: false,
            branch_switch_busy: false,
            merge_busy: false,
            merge_abort_confirmation: false,
            merge_notice: None,
            action: None,
            notice: None,
            needs_merge: false,
            git_available: false,
        }
    }

    /// Il messaggio del pannello senza snapshot. `busy` viene prima di `root`
    /// (come nel Log Git): con l'avvio pigro il pannello si apre prima che
    /// arrivi lo stato, e «non è un repository» sarebbe falso.
    fn messaggio_senza_snapshot(&self) -> &str {
        if let Some(error) = &self.error {
            return error;
        }
        if self.busy {
            "Caricamento…"
        } else if self.root.is_some() {
            "La cartella aperta non è un repository Git."
        } else {
            "Apri una cartella per usare Git."
        }
    }

    fn files_of(&self, group: Group) -> Vec<&FileStatus> {
        match &self.snapshot {
            Some(snapshot) => snapshot
                .files
                .iter()
                .filter(|file| group.holds(file))
                .collect(),
            None => Vec::new(),
        }
    }

    fn has_origin(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.has_origin)
    }

    fn commands(&self, blame: Option<Event>) -> Vec<Command> {
        let free = self.action.is_none()
            && !self.fetch_busy
            && !self.push_busy
            && !self.branch_switch_busy
            && !self.merge_busy
            && !self.stash_busy;
        let idle = free && !self.busy;
        let has_changes = self.snapshot.as_ref().is_some_and(|s| !s.files.is_empty());
        let can_discard = !self.files_of(self.selected_group).is_empty() && !self.busy;
        let mut commands = vec![
            Command {
                icon: Icon::Refresh,
                title: "Aggiorna",
                event: (!self.busy).then_some(Event::Refresh),
            },
            Command {
                icon: Icon::Undo,
                title: "Ripristina",
                event: can_discard.then_some(Event::AskDiscard),
            },
            Command {
                icon: Icon::ChevronExpand,
                title: "Espandi tutto",
                event: Some(Event::ExpandAll),
            },
            Command {
                icon: Icon::ChevronCollapse,
                title: "Comprimi tutto",
                event: Some(Event::CollapseAll),
            },
            Command {
                icon: Icon::Git,
                title: "Blame",
                event: blame,
            },
            Command {
                icon: Icon::Stash,
                title: "Stash",
                event: Some(Event::ToggleStashPanel),
            },
        ];
        if has_changes && self.git_available {
            commands.push(Command {
                icon: Icon::Plus,
                title: "Salva stash",
                event: idle.then_some(Event::Run(lume_ui::tool::GitAction::StashPush(None))),
            });
        }
        if self.has_origin() {
            commands.push(Command {
                icon: Icon::Fetch,
                title: "Fetch",
                event: free.then_some(Event::Run(lume_ui::tool::GitAction::Fetch)),
            });
            if self.git_available {
                commands.push(Command {
                    icon: Icon::Pull,
                    title: "Pull",
                    event: (free && self.snapshot.as_ref().is_some_and(|s| s.has_upstream))
                        .then_some(Event::Run(lume_ui::tool::GitAction::Pull)),
                });
                commands.push(Command {
                    icon: Icon::Push,
                    title: "Push",
                    event: (free && self.snapshot.as_ref().is_some_and(|s| s.branch.is_some()))
                        .then_some(Event::Run(lume_ui::tool::GitAction::Push)),
                });
            }
        }
        if self.needs_merge
            && self.git_available
            && self
                .snapshot
                .as_ref()
                .is_none_or(|s| s.merge_conflicts.is_none())
        {
            commands.push(Command {
                icon: Icon::Merge,
                title: "Unisci",
                event: free.then_some(Event::Run(lume_ui::tool::GitAction::MergeUpstream)),
            });
        }
        commands
    }

    fn release(&mut self) {
        self.snapshot = None;
        self.error = None;
        self.busy = false;
        self.selected_commit = None;
        self.pending_discard = None;
        self.notice = None;
        self.stash_entries.clear();
        self.stash_panel_open = false;
        self.stash_error = None;
        self.stash_notice = None;
        self.fetch_notice = None;
        self.branch_picker_open = false;
        self.merge_abort_confirmation = false;
        self.merge_notice = None;
        self.needs_merge = false;
        self.blame_path = None;
        self.blame_lines.clear();
        self.blame_dirty = false;
        self.blame_loading = false;
        self.blame_error = None;
        self.git_available = false;
    }

    fn staged_count(&self) -> usize {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.files.iter().filter(|f| f.is_staged()).count())
            .unwrap_or(0)
    }

    /// Riga di gruppo: chevron, casella a tre stati, titolo, conteggio.
    fn group_row<'a>(&'a self, ctx: &ToolCtx<'a>, group: Group) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        let files = self.files_of(group);
        let staged = files.iter().filter(|file| file.is_staged()).count();
        let state = if staged == 0 {
            CheckState::Empty
        } else if staged == files.len() {
            CheckState::Full
        } else {
            CheckState::Partial
        };
        let open = !self.collapsed.contains(&group);
        let selected = self.selected_group == group && ctx.open_diff.is_none();

        let chevron = button(text(if open { "▾" } else { "▸" }).size(10).color(t.sec))
            .padding(0)
            .width(14)
            .style(styles::hover_row(t.sec, Color::TRANSPARENT, None, 3.0))
            .on_press(ToolEvent::new(Event::CollapseGroup(group)));
        let check = button(check_box(t, state))
            .padding(0)
            .style(styles::hover_row(t.ui, Color::TRANSPARENT, None, 3.0))
            .on_press(ToolEvent::new(Event::ToggleGroup(group)));

        let inner = row![
            chevron,
            check,
            text(group.title()).size(12).color(t.text),
            text(format!("{} file", files.len())).size(11).color(t.sec),
        ]
        .spacing(7)
        .align_y(Alignment::Center);

        let background = if selected { Some(t.sel) } else { None };
        button(inner)
            .padding(Padding::from([2, 6]))
            .width(Length::Fill)
            .style(styles::hover_row(t.ui, t.hover, background, 5.0))
            .on_press(ToolEvent::new(Event::SelectGroup(group)))
            .into()
    }

    /// Riga di file: casella (stage), pallino del tipo, nome, cartella.
    fn file_row<'a>(&'a self, ctx: &ToolCtx<'a>, file: &'a FileStatus) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        let kind = file.index.or(file.worktree).unwrap_or(ChangeKind::Modified);
        let color = match kind {
            ChangeKind::Added | ChangeKind::Copied => t.good,
            ChangeKind::Conflict | ChangeKind::Removed => t.bad,
            _ => t.warn,
        };
        // Tre stati, non due: con lo staging per hunk un file può avere parte
        // delle modifiche nell'index e parte ancora solo nel worktree. Il
        // trattino lo dice — ed è lo stesso `Partial` che la riga di gruppo usa
        // già, quindi niente token nuovi e niente disegno nuovo.
        //
        // Il clic resta quello di prima: su un file parziale toglie **tutto**
        // dallo stage. È la semantica di git, e chi vuole togliere un solo hunk
        // ora ha la vista «In stage» nel diff.
        let state = if file.is_partially_staged() {
            CheckState::Partial
        } else if file.is_staged() {
            CheckState::Full
        } else {
            CheckState::Empty
        };
        let check = button(check_box(t, state))
            .padding(0)
            .style(styles::hover_row(t.ui, Color::TRANSPARENT, None, 3.0))
            .on_press(ToolEvent::new(Event::ToggleFile(file.path.clone())));

        let name = file
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.path.to_string_lossy().into_owned());
        let parent = file
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| format!("{}/", parent.to_string_lossy().replace('\\', "/")))
            .unwrap_or_default();

        let open = ctx
            .open_diff
            .is_some_and(|(path, commit)| commit.is_none() && path == file.path);
        let background = if open { Some(t.sel) } else { None };

        let inner = row![
            Space::new().width(13),
            check,
            text(change_label(kind)).size(9).font(MONO).color(color),
            text(name).size(12).color(color),
            container(
                text(parent)
                    .size(11)
                    .color(t.muted)
                    .wrapping(text::Wrapping::None)
            )
            .clip(true)
            .width(Length::Fill),
        ]
        .spacing(7)
        .align_y(Alignment::Center);

        button(inner)
            .padding(Padding::from([2, 6]))
            .width(Length::Fill)
            .style(styles::hover_row(t.ui, t.hover, background, 5.0))
            .on_press(ToolEvent::new(Event::OpenFile(file.path.clone())))
            .into()
    }

    /// Barra strumenti, o la riga di conferma quando Ripristina è in attesa.
    fn toolbar<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        if let Some(paths) = &self.pending_discard {
            let label = match paths.len() {
                1 => "Buttare via le modifiche di 1 file?".to_owned(),
                count => format!("Buttare via le modifiche di {count} file?"),
            };
            return row![
                text(label).size(11.5).color(t.warn),
                Space::new().width(Length::Fill),
                button(text("Ripristina").size(11).color(t.bad))
                    .padding(Padding::from([3, 8]))
                    .style(styles::hover_row(t.bad, t.hover, None, 4.0))
                    .on_press(ToolEvent::new(Event::ConfirmDiscard)),
                button(text("Annulla").size(11).color(t.sec))
                    .padding(Padding::from([3, 8]))
                    .style(styles::hover_row(t.sec, t.hover, None, 4.0))
                    .on_press(ToolEvent::new(Event::CancelDiscard)),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into();
        }

        // Blame dipende dal diff aperto, che appartiene al contesto della vista.
        let blame_event = ctx
            .open_diff
            .and_then(|(path, commit)| commit.is_none().then_some(path))
            .map(|path| Event::OpenBlame(path.to_path_buf()));
        let mut toolbar = row![].spacing(2).align_y(Alignment::Center);
        for command in self.commands(blame_event) {
            toolbar = toolbar.push(tool_button(t, command.icon, command.title, command.event));
        }
        toolbar.into()
    }
}

impl GitTool {
    fn merge_bar<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        let Some(snapshot) = &self.snapshot else {
            return Space::new().into();
        };
        if self.merge_abort_confirmation {
            if !self.git_available {
                return row![
                    text("Git CLI non disponibile: impossibile annullare il merge.")
                        .size(11)
                        .color(t.bad),
                    Space::new().width(Length::Fill),
                    button(text("Chiudi").size(11))
                        .padding([3, 8])
                        .on_press(ToolEvent::new(Event::CancelMergeAbort)),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .into();
            }
            return row![
                text("Annullare il merge e ripristinare il worktree?")
                    .size(11)
                    .color(t.warn),
                Space::new().width(Length::Fill),
                button(text("Annulla merge").size(11).color(t.bad))
                    .padding([3, 8])
                    .on_press(ToolEvent::new(Event::ConfirmMergeAbort)),
                button(text("Torna indietro").size(11))
                    .padding([3, 8])
                    .on_press(ToolEvent::new(Event::CancelMergeAbort))
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into();
        }
        let count = snapshot.merge_conflicts.as_ref().map_or(0, Vec::len);
        let status = if self.merge_busy {
            "Operazione merge in corso…".into()
        } else if count > 0 {
            format!("{count} conflitti da risolvere e mettere in stage")
        } else {
            "Merge pronto per essere completato".into()
        };
        let mut bar = row![
            text(status)
                .size(11)
                .color(if count > 0 { t.warn } else { t.text }),
            Space::new().width(Length::Fill)
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if self.merge_busy {
            bar = bar.push(
                button(text("Ferma").size(11))
                    .padding([3, 8])
                    .on_press(ToolEvent::new(Event::CancelMergeOperation)),
            );
        } else if snapshot.merge_conflicts.is_some() && !self.git_available {
            bar = bar.push(
                text("Git CLI non disponibile: impossibile continuare o annullare il merge.")
                    .size(11)
                    .color(t.bad),
            );
        } else if snapshot.merge_conflicts.is_some() {
            bar = bar
                .push(
                    button(text("Continua merge").size(11))
                        .padding([3, 8])
                        .on_press_maybe((count == 0).then(|| ToolEvent::new(Event::MergeContinue))),
                )
                .push(
                    button(text("Annulla…").size(11).color(t.bad))
                        .padding([3, 8])
                        .on_press(ToolEvent::new(Event::AskMergeAbort)),
                );
        }
        if let Some(notice) = &self.merge_notice {
            bar = bar.push(text(notice).size(10).color(t.muted));
        }
        bar.into()
    }
}

impl Tool for GitTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Git"
    }

    fn icon(&self) -> Icon {
        Icon::Git
    }

    fn feature(&self) -> Option<&'static str> {
        Some("git")
    }

    fn on_visibility_changed(&mut self, open: bool) -> Vec<ToolRequest> {
        if !open {
            self.release();
            return Vec::new();
        }
        self.git_available = lume_git::GitCli::locate().is_some();
        if self.snapshot.is_none() && !self.busy && self.root.is_some() {
            self.busy = true;
            return vec![ToolRequest::GitRefresh];
        }
        Vec::new()
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();
        let Some(snapshot) = &self.snapshot else {
            return container(
                text(self.messaggio_senza_snapshot())
                    .size(12)
                    .color(t.muted),
            )
            .padding(12)
            .width(Length::Fill)
            .into();
        };

        let branch = snapshot.branch.as_deref().unwrap_or("HEAD detached");
        let mut content = column![
            row![
                branch_pill(
                    t,
                    branch,
                    snapshot.local_branches.len() > 1 && self.git_available,
                ),
                Space::new().width(Length::Fill),
                text(if self.busy { "…" } else { "" })
                    .size(13)
                    .color(t.muted),
            ]
            .align_y(Alignment::Center),
            self.toolbar(ctx),
        ]
        .spacing(6);

        if self.branch_picker_open {
            let mut branches = column![text("Branch locali:").size(11).color(t.muted)].spacing(4);
            for name in &snapshot.local_branches {
                let active = snapshot.branch.as_deref() == Some(name.as_str());
                let label = if active {
                    format!("⎇ {name}")
                } else {
                    name.clone()
                };
                let available = !active
                    && !self.branch_switch_busy
                    && !self.fetch_busy
                    && !self.push_busy
                    && !self.merge_busy
                    && !self.busy;
                let mut branch_row = row![text(label).size(11).width(Length::Fill)]
                    .spacing(5)
                    .align_y(Alignment::Center);
                if self.git_available {
                    branch_row = branch_row
                        .push(
                            button(text("Cambia").size(10))
                                .padding([3, 7])
                                .on_press_maybe(
                                    available
                                        .then(|| ToolEvent::new(Event::SwitchBranch(name.clone()))),
                                ),
                        )
                        .push(
                            button(text("Unisci").size(10))
                                .padding([3, 7])
                                .on_press_maybe(
                                    (available && snapshot.merge_conflicts.is_none())
                                        .then(|| ToolEvent::new(Event::MergeBranch(name.clone()))),
                                ),
                        );
                }
                branches = branches.push(branch_row);
            }
            if self.branch_switch_busy {
                branches = branches
                    .push(text("Cambio in corso…").size(11).color(t.muted))
                    .push(
                        button(text("Annulla").size(11))
                            .padding([3, 7])
                            .on_press(ToolEvent::new(Event::CancelBranchSwitch)),
                    );
            }
            content = content.push(scrollable(branches).height(Length::Fixed(120.0)));
        }

        if snapshot.merge_conflicts.is_some() || self.merge_busy || self.merge_abort_confirmation {
            content = content.push(self.merge_bar(ctx));
        }

        for group in Group::ALL {
            content = content.push(self.group_row(ctx, group));
            if !self.collapsed.contains(&group) {
                for file in self.files_of(group) {
                    content = content.push(self.file_row(ctx, file));
                }
            }
        }

        if let Some(error) = &self.error {
            content = content.push(text(error).size(11).color(t.bad));
        }

        // Riga Amend: casella, etichetta, «N modificati».
        let staged_count = self.staged_count();
        let amend_state = if self.amend {
            CheckState::Full
        } else {
            CheckState::Empty
        };
        content = content.push(
            row![
                button(check_box(t, amend_state))
                    .padding(0)
                    .style(styles::hover_row(t.ui, Color::TRANSPARENT, None, 3.0))
                    .on_press(ToolEvent::new(Event::ToggleAmend)),
                button(text("Amend").size(12).color(t.text))
                    .padding(0)
                    .style(styles::hover_row(t.text, Color::TRANSPARENT, None, 3.0))
                    .on_press(ToolEvent::new(Event::ToggleAmend)),
                Space::new().width(Length::Fill),
                text(format!(
                    "{staged_count} modificat{}",
                    if staged_count == 1 { "o" } else { "i" }
                ))
                .size(11)
                .color(t.text),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );

        let input = text_input("Messaggio di commit…", &self.message)
            .on_input(|value| ToolEvent::new(Event::MessageChanged(value)))
            .padding(Padding::from([6, 8]))
            .size(12)
            .style(styles::input(t));
        // Con Amend basta il messaggio: correggere il testo di un commit è un
        // uso legittimo, e `commit_with` non pretende nulla in stage.
        let can_commit =
            (self.amend || staged_count > 0) && !self.message.trim().is_empty() && !self.busy;
        let label = if self.amend {
            "Amend".to_owned()
        } else {
            format!("Commit ({staged_count})")
        };
        let mut commit = button(text(label).size(12)).padding(Padding::from([5, 10]));
        if can_commit {
            commit = commit.on_press(ToolEvent::new(Event::Commit));
        }
        content = content.push(input).push(commit);

        if !snapshot.commits.is_empty() {
            content = content.push(section("COMMIT RECENTI", snapshot.commits.len(), t));
            for (index, commit) in snapshot.commits.iter().take(8).enumerate() {
                let short = commit.id.get(..7).unwrap_or(&commit.id);
                let dot_color = if index == 0 { t.accent } else { t.faint };
                content = content.push(
                    button(
                        row![
                            dot(dot_color),
                            text(short).size(10.5).font(MONO).color(t.sec),
                            container(
                                text(&commit.subject)
                                    .size(12)
                                    .color(t.ui)
                                    .wrapping(text::Wrapping::None)
                            )
                            .clip(true)
                            .width(Length::Fill),
                            text(relative_age(commit.author_time, now))
                                .size(11)
                                .color(t.muted),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding(Padding::from([3, 6]))
                    .style(styles::hover_row(t.ui, t.hover, None, 4.0))
                    .on_press(ToolEvent::new(Event::SelectCommit(index))),
                );
            }
        }
        if let Some(index) = self.selected_commit
            && let Some(commit) = snapshot.commits.get(index)
        {
            let parents = match commit.parent_ids.first() {
                Some(parent) => format!("parent {}", parent.get(..7).unwrap_or(parent)),
                None => "nessun parent".into(),
            };
            content = content.push(
                container(
                    column![
                        text(&commit.subject).size(12).color(t.ui),
                        text(format!("{} <{}>", commit.author_name, commit.author_email))
                            .size(10)
                            .color(t.sec),
                        text(parents).size(10).font(MONO).color(t.muted),
                        text(commit.body.as_deref().unwrap_or(""))
                            .size(11)
                            .color(t.muted),
                    ]
                    .spacing(3),
                )
                .padding(8)
                .width(Length::Fill),
            );
        }

        // ── Esito dell'azione unificata ─────────────────────────
        if let Some(action) = &self.action {
            content = content.push(
                row![
                    text(action.label()).size(11).color(t.muted),
                    Space::new().width(Length::Fill),
                    button(text("Annulla").size(10))
                        .padding(Padding::from([2, 6]))
                        .on_press(ToolEvent::new(Event::Cancel)),
                ]
                .align_y(Alignment::Center),
            );
        }
        if let Some(report) = &self.notice {
            let color = match report.kind {
                crate::git_actions::NoticeKind::Ok => t.good,
                crate::git_actions::NoticeKind::Warning => t.warn,
                crate::git_actions::NoticeKind::Error => t.bad,
            };
            content = content.push(text(&report.text).size(11).color(color));
        }

        // ── Fetch notice ────────────────────────────────────────
        if let Some(notice) = &self.fetch_notice {
            content = content.push(
                text(notice)
                    .size(11)
                    .color(if notice.starts_with("Errore") {
                        t.bad
                    } else {
                        t.good
                    }),
            );
        }

        // ── Stash panel ─────────────────────────────────────────
        if self.stash_panel_open {
            content = content.push(
                row![
                    text("STASH").size(10.5).color(t.sec),
                    Space::new().width(Length::Fill),
                    button(text("×").size(11).color(t.muted))
                        .padding(Padding::from([0, 4]))
                        .style(styles::hover_row(t.muted, Color::TRANSPARENT, None, 3.0))
                        .on_press(ToolEvent::new(Event::ToggleStashPanel)),
                ]
                .align_y(Alignment::Center),
            );
            if let Some(err) = &self.stash_error {
                content = content.push(text(err).size(11).color(t.bad));
            }
            if let Some(notice) = &self.stash_notice {
                content = content.push(text(notice).size(11).color(t.sec));
            }
            if self.stash_busy {
                content = content.push(
                    row![
                        text("Operazione stash in corso…").size(11).color(t.muted),
                        Space::new().width(Length::Fill),
                        button(text("Annulla").size(10))
                            .padding(Padding::from([2, 6]))
                            .on_press(ToolEvent::new(Event::CancelStashOperation)),
                    ]
                    .align_y(Alignment::Center),
                );
            }
            if self.stash_entries.is_empty() {
                content = content.push(text("Nessuno stash.").size(11).color(t.muted));
            } else {
                for entry in &self.stash_entries {
                    let label = format!("stash@{{{}}} — {}", entry.index, entry.message);
                    let idx = entry.index;
                    let mut stash_row = row![
                        button(
                            text(label)
                                .size(11)
                                .color(t.ui)
                                .wrapping(text::Wrapping::None)
                        )
                        .width(Length::Fill)
                        .padding(Padding::from([2, 6]))
                        .style(styles::hover_row(t.ui, t.hover, None, 4.0))
                        .on_press(ToolEvent::new(Event::StashRefresh)),
                    ]
                    .align_y(Alignment::Center)
                    .spacing(2);
                    if self.git_available {
                        stash_row = stash_row
                            .push(
                                button(text("Applica").size(9).color(t.ui))
                                    .padding(Padding::from([1, 4]))
                                    .style(styles::hover_row(t.ui, Color::TRANSPARENT, None, 3.0))
                                    .on_press_maybe((!self.stash_busy).then_some(ToolEvent::new(
                                        Event::StashApply {
                                            index: idx,
                                            pop: false,
                                        },
                                    ))),
                            )
                            .push(
                                button(text("Pop").size(9).color(t.sec))
                                    .padding(Padding::from([1, 4]))
                                    .style(styles::hover_row(t.sec, Color::TRANSPARENT, None, 3.0))
                                    .on_press_maybe((!self.stash_busy).then_some(ToolEvent::new(
                                        Event::StashApply {
                                            index: idx,
                                            pop: true,
                                        },
                                    ))),
                            );
                    }
                    stash_row = stash_row.push(
                        button(text("✕").size(9).color(t.bad))
                            .padding(Padding::from([1, 4]))
                            .style(styles::hover_row(t.bad, Color::TRANSPARENT, None, 3.0))
                            .on_press_maybe(
                                (!self.stash_busy).then_some(ToolEvent::new(Event::StashDrop(idx))),
                            ),
                    );
                    content = content.push(stash_row);
                }
            }
        }

        // ── Blame side panel ─────────────────────────────────────
        if let Some(blame_path) = &self.blame_path {
            let file_name = blame_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| blame_path.to_string_lossy().into_owned());

            content = content.push(
                row![
                    text(format!("BLAME · {file_name}")).size(10.5).color(t.sec),
                    Space::new().width(Length::Fill),
                    button(text("×").size(11).color(t.muted))
                        .padding(Padding::from([0, 4]))
                        .style(styles::hover_row(t.muted, Color::TRANSPARENT, None, 3.0))
                        .on_press(ToolEvent::new(Event::CloseBlame)),
                ]
                .align_y(Alignment::Center),
            );

            if self.blame_dirty {
                content = content.push(
                    text("⚠ Il file ha modifiche non committate: blame su HEAD.")
                        .size(10.5)
                        .color(t.warn),
                );
            }
            if self.blame_loading {
                content = content.push(text("Caricamento blame…").size(11).color(t.muted));
            } else if let Some(err) = &self.blame_error {
                content = content.push(text(err).size(11).color(t.bad));
            } else {
                // Raggruppa le righe consecutive dello stesso commit.
                let mut last_commit = "";
                for line in &self.blame_lines {
                    let show_header = line.commit_id.as_str() != last_commit;
                    if show_header {
                        last_commit = line.commit_id.as_str();
                        let age = blame_age(line.author_time, now);
                        content = content.push(
                            container(
                                row![
                                    text(&line.commit_id).size(10).font(MONO).color(t.accent),
                                    text(&line.author_name).size(10).color(t.sec),
                                    text(age).size(10).color(t.muted),
                                ]
                                .spacing(6)
                                .align_y(Alignment::Center),
                            )
                            .padding(Padding::from([1, 6])),
                        );
                    }
                    content = content.push(
                        container(
                            text(format!("{:>4}", line.line_number))
                                .size(10)
                                .font(MONO)
                                .color(t.muted),
                        )
                        .padding(Padding::from([0, 6])),
                    );
                }
            }
        }

        scrollable(container(content).padding(12).width(Length::Fill))
            .height(Length::Fill)
            .into()
    }

    fn update(&mut self, event: &ToolEvent) -> Vec<ToolRequest> {
        match event.downcast::<Event>() {
            Some(Event::Run(action)) => {
                if self.action.is_some() {
                    return Vec::new();
                }
                let idle = !self.busy
                    && !self.fetch_busy
                    && !self.push_busy
                    && !self.branch_switch_busy
                    && !self.merge_busy
                    && !self.stash_busy;
                let allowed = match action {
                    lume_ui::tool::GitAction::StashPush(_) => {
                        idle && self
                            .snapshot
                            .as_ref()
                            .is_some_and(|s| self.git_available && !s.files.is_empty())
                    }
                    lume_ui::tool::GitAction::Fetch => {
                        idle && self.has_origin()
                            && self
                                .snapshot
                                .as_ref()
                                .is_none_or(|s| s.merge_conflicts.is_none())
                    }
                    lume_ui::tool::GitAction::Pull => {
                        idle && self.has_origin()
                            && self.snapshot.as_ref().is_some_and(|s| {
                                self.git_available && s.has_upstream && s.merge_conflicts.is_none()
                            })
                    }
                    lume_ui::tool::GitAction::Push => {
                        idle && self.has_origin()
                            && self.snapshot.as_ref().is_some_and(|s| {
                                self.git_available
                                    && s.branch.is_some()
                                    && s.merge_conflicts.is_none()
                            })
                    }
                    lume_ui::tool::GitAction::MergeUpstream => {
                        idle && self.needs_merge
                            && self
                                .snapshot
                                .as_ref()
                                .is_some_and(|s| self.git_available && s.merge_conflicts.is_none())
                    }
                    _ => false,
                };
                if !allowed {
                    return Vec::new();
                }
                self.action = Some(action.clone());
                self.notice = None;
                self.stash_error = None;
                match action {
                    lume_ui::tool::GitAction::StashPush(_) => {
                        self.stash_busy = true;
                        self.stash_notice = Some("Salvataggio stash in corso…".into());
                    }
                    lume_ui::tool::GitAction::Fetch => {
                        self.fetch_busy = true;
                        self.pull_busy = false;
                        self.fetch_notice = Some("Fetch in corso…".into());
                    }
                    lume_ui::tool::GitAction::Pull => {
                        self.fetch_busy = true;
                        self.pull_busy = true;
                        self.fetch_notice = Some("Pull in corso…".into());
                    }
                    lume_ui::tool::GitAction::Push => {
                        self.push_busy = true;
                        self.fetch_notice = Some("Push in corso…".into());
                    }
                    lume_ui::tool::GitAction::MergeUpstream => {
                        self.merge_busy = true;
                        self.merge_notice = Some("Unione dell'upstream in corso…".into());
                    }
                    _ => {}
                }
                vec![ToolRequest::GitAction(action.clone())]
            }
            Some(Event::ActionStarted(action)) => {
                self.action = Some(action.clone());
                self.notice = None;
                self.stash_notice = None;
                self.stash_error = None;
                self.fetch_notice = None;
                self.merge_notice = None;
                Vec::new()
            }
            Some(Event::Cancel) => {
                let Some(action) = self.action.as_ref() else {
                    return Vec::new();
                };
                match action {
                    lume_ui::tool::GitAction::Fetch => {
                        self.fetch_notice = Some("Annullamento del fetch richiesto…".into());
                    }
                    lume_ui::tool::GitAction::Pull => {
                        self.fetch_notice = Some("Annullamento del pull richiesto…".into());
                    }
                    lume_ui::tool::GitAction::Push => {
                        self.fetch_notice = Some("Annullamento del push richiesto…".into());
                    }
                    lume_ui::tool::GitAction::StashPush(_) => {
                        self.stash_notice = Some("Annullamento richiesto…".into());
                    }
                    lume_ui::tool::GitAction::MergeUpstream => {
                        self.merge_notice = Some("Annullamento richiesto…".into());
                    }
                    _ => {}
                }
                vec![ToolRequest::GitCancel]
            }
            Some(Event::ActionFinished(action, report)) => {
                self.action = None;
                self.fetch_busy = false;
                self.pull_busy = false;
                self.push_busy = false;
                self.stash_busy = false;
                self.branch_switch_busy = false;
                self.merge_busy = false;
                self.merge_abort_confirmation = false;
                self.merge_notice = None;
                self.fetch_notice = None;
                self.stash_notice = None;
                if matches!(action, lume_ui::tool::GitAction::SwitchBranch(_)) {
                    self.branch_picker_open = false;
                }
                self.needs_merge = report.needs_merge;
                self.notice = Some(report.clone());
                Vec::new()
            }
            Some(Event::SetRoot(root)) => {
                self.root = Some(root.clone());
                self.snapshot = None;
                self.error = None;
                self.busy = true;
                self.selected_commit = None;
                self.pending_discard = None;
                self.fetch_busy = false;
                self.pull_busy = false;
                self.push_busy = false;
                self.branch_picker_open = false;
                self.branch_switch_busy = false;
                self.merge_busy = false;
                self.merge_abort_confirmation = false;
                self.merge_notice = None;
                self.fetch_notice = None;
                self.action = None;
                self.notice = None;
                self.needs_merge = false;
                self.stash_busy = false;
                self.stash_notice = None;
                Vec::new()
            }
            Some(Event::Loaded(result, committed)) => {
                self.busy = false;
                match result {
                    Ok(snapshot) => {
                        self.snapshot = Some(snapshot.clone());
                        self.error = None;
                        if *committed {
                            self.message.clear();
                            // Un amend si consuma: lasciarlo acceso farebbe
                            // riscrivere il commit appena creato.
                            self.amend = false;
                        }
                    }
                    Err(error) => {
                        self.error = Some(error.clone());
                    }
                }
                Vec::new()
            }
            Some(Event::ToggleFile(path)) => {
                let staged = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.files.iter().find(|f| &f.path == path))
                    .is_some_and(FileStatus::is_staged);
                self.busy = true;
                if staged {
                    vec![ToolRequest::GitUnstage(vec![path.clone()])]
                } else {
                    vec![ToolRequest::GitStage(vec![path.clone()])]
                }
            }
            Some(Event::OpenFile(path)) => {
                vec![ToolRequest::GitOpenDiff {
                    path: path.clone(),
                    commit: None,
                }]
            }
            Some(Event::ToggleGroup(group)) => {
                let files = self.files_of(*group);
                if files.is_empty() {
                    return Vec::new();
                }
                // Parziale conta come «non tutto»: la casella completa.
                let all_staged = files.iter().all(|file| file.is_staged());
                let paths: Vec<PathBuf> = files.iter().map(|file| file.path.clone()).collect();
                self.busy = true;
                if all_staged {
                    vec![ToolRequest::GitUnstage(paths)]
                } else {
                    vec![ToolRequest::GitStage(paths)]
                }
            }
            Some(Event::CollapseGroup(group)) => {
                if !self.collapsed.remove(group) {
                    self.collapsed.insert(*group);
                }
                Vec::new()
            }
            Some(Event::SelectGroup(group)) => {
                self.selected_group = *group;
                Vec::new()
            }
            Some(Event::ExpandAll) => {
                self.collapsed.clear();
                Vec::new()
            }
            Some(Event::CollapseAll) => {
                self.collapsed = Group::ALL.into_iter().collect();
                Vec::new()
            }
            Some(Event::AskDiscard) => {
                let paths: Vec<PathBuf> = self
                    .files_of(self.selected_group)
                    .iter()
                    .map(|file| file.path.clone())
                    .collect();
                if !paths.is_empty() {
                    self.pending_discard = Some(paths);
                }
                Vec::new()
            }
            Some(Event::ConfirmDiscard) => match self.pending_discard.take() {
                Some(paths) => {
                    self.busy = true;
                    vec![ToolRequest::GitDiscard(paths)]
                }
                None => Vec::new(),
            },
            Some(Event::CancelDiscard) => {
                self.pending_discard = None;
                Vec::new()
            }
            Some(Event::MessageChanged(message)) => {
                self.message = message.clone();
                Vec::new()
            }
            Some(Event::ToggleAmend) => {
                self.amend = !self.amend;
                // Accendendo Amend la casella si precompila con l'oggetto del
                // commit di HEAD, che è quel che si sta per riscrivere; solo
                // se l'utente non ha già scritto qualcosa di suo.
                if self.amend
                    && self.message.trim().is_empty()
                    && let Some(subject) = self
                        .snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.head_message.as_deref())
                        .and_then(|message| message.lines().next())
                {
                    self.message = subject.trim().to_owned();
                }
                Vec::new()
            }
            Some(Event::Commit) if !self.message.trim().is_empty() => {
                self.busy = true;
                vec![ToolRequest::GitCommit {
                    message: self.message.clone(),
                    amend: self.amend,
                }]
            }
            Some(Event::Refresh) => {
                self.busy = true;
                vec![ToolRequest::GitRefresh]
            }
            Some(Event::SelectCommit(index)) => {
                self.selected_commit = Some(*index);
                Vec::new()
            }
            // ── Blame ────────────────────────────────────────────
            Some(Event::OpenBlame(path)) => {
                self.blame_path = Some(path.clone());
                self.blame_lines = Vec::new();
                self.blame_error = None;
                self.blame_loading = true;
                vec![ToolRequest::GitBlame(path.clone())]
            }
            Some(Event::CloseBlame) => {
                self.blame_path = None;
                self.blame_lines = Vec::new();
                self.blame_loading = false;
                self.blame_error = None;
                vec![ToolRequest::GitCloseBlame]
            }
            Some(Event::BlameLoaded(result, dirty)) => {
                self.blame_loading = false;
                self.blame_dirty = *dirty;
                match result {
                    Ok(lines) => {
                        self.blame_lines = lines.clone();
                        self.blame_error = None;
                    }
                    Err(err) => {
                        self.blame_error = Some(err.clone());
                    }
                }
                Vec::new()
            }
            // ── Stash ────────────────────────────────────────────
            Some(Event::ToggleStashPanel) => {
                self.stash_panel_open = !self.stash_panel_open;
                if self.stash_panel_open {
                    vec![ToolRequest::GitStashRefresh]
                } else {
                    Vec::new()
                }
            }
            Some(Event::StashRefresh) => {
                vec![ToolRequest::GitStashRefresh]
            }
            Some(Event::StashListLoaded(result)) => {
                if !self.stash_panel_open {
                    return Vec::new();
                }
                match result {
                    Ok(entries) => {
                        self.stash_entries = entries.clone();
                        self.stash_error = None;
                    }
                    Err(err) => {
                        self.stash_error = Some(err.clone());
                    }
                }
                Vec::new()
            }
            Some(Event::StashApply { index, pop }) if !self.stash_busy => {
                if !self.snapshot.as_ref().is_some_and(|_| self.git_available) {
                    return Vec::new();
                }
                self.stash_busy = true;
                self.stash_error = None;
                self.stash_notice = Some(if *pop {
                    format!("Pop di stash@{{{index}}} in corso…")
                } else {
                    format!("Apply di stash@{{{index}}} in corso…")
                });
                vec![ToolRequest::GitAction(if *pop {
                    lume_ui::tool::GitAction::StashPop(*index)
                } else {
                    lume_ui::tool::GitAction::StashApply(*index)
                })]
            }
            Some(Event::CancelStashOperation) if self.stash_busy => {
                self.stash_notice = Some("Annullamento richiesto…".to_owned());
                vec![ToolRequest::GitCancel]
            }
            Some(Event::StashDrop(index)) => {
                vec![ToolRequest::GitAction(lume_ui::tool::GitAction::StashDrop(
                    *index,
                ))]
            }
            Some(Event::ToggleBranchPicker)
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.local_branches.len() > 1) =>
            {
                self.branch_picker_open = !self.branch_picker_open;
                Vec::new()
            }
            Some(Event::SwitchBranch(name))
                if !self.branch_switch_busy
                    && !self.merge_busy
                    && self
                        .snapshot
                        .as_ref()
                        .is_none_or(|s| s.merge_conflicts.is_none())
                    && !self.fetch_busy
                    && !self.push_busy
                    && !self.busy
                    && self.snapshot.as_ref().is_some_and(|snapshot| {
                        self.git_available
                            && snapshot.branch.as_deref() != Some(name.as_str())
                            && snapshot.local_branches.iter().any(|branch| branch == name)
                    }) =>
            {
                self.branch_switch_busy = true;
                vec![ToolRequest::GitAction(
                    lume_ui::tool::GitAction::SwitchBranch(name.clone()),
                )]
            }
            Some(Event::CancelBranchSwitch) if self.branch_switch_busy => {
                vec![ToolRequest::GitCancel]
            }
            Some(Event::MergeBranch(name))
                if !self.merge_busy
                    && !self.branch_switch_busy
                    && !self.fetch_busy
                    && !self.push_busy
                    && !self.busy
                    && self.snapshot.as_ref().is_some_and(|s| {
                        self.git_available
                            && s.merge_conflicts.is_none()
                            && s.branch.as_deref() != Some(name.as_str())
                            && s.local_branches.contains(name)
                    }) =>
            {
                self.merge_busy = true;
                self.merge_notice = Some(format!("Unione di «{name}» in corso…"));
                vec![ToolRequest::GitAction(
                    lume_ui::tool::GitAction::MergeBranch(name.clone()),
                )]
            }
            Some(Event::MergeContinue)
                if !self.merge_busy
                    && self.snapshot.as_ref().is_some_and(|s| {
                        self.git_available && s.merge_conflicts.as_ref().is_some_and(Vec::is_empty)
                    }) =>
            {
                self.merge_busy = true;
                self.merge_notice = Some("Completamento merge in corso…".into());
                vec![ToolRequest::GitAction(
                    lume_ui::tool::GitAction::MergeContinue("Merge".into()),
                )]
            }
            Some(Event::AskMergeAbort)
                if !self.merge_busy
                    && self
                        .snapshot
                        .as_ref()
                        .is_some_and(|s| self.git_available && s.merge_conflicts.is_some()) =>
            {
                self.merge_abort_confirmation = true;
                Vec::new()
            }
            Some(Event::CancelMergeAbort) => {
                self.merge_abort_confirmation = false;
                Vec::new()
            }
            Some(Event::ConfirmMergeAbort)
                if self.merge_abort_confirmation
                    && !self.merge_busy
                    && self
                        .snapshot
                        .as_ref()
                        .is_some_and(|s| self.git_available && s.merge_conflicts.is_some()) =>
            {
                self.merge_abort_confirmation = false;
                self.merge_busy = true;
                self.merge_notice = Some("Annullamento merge in corso…".into());
                vec![ToolRequest::GitAction(lume_ui::tool::GitAction::MergeAbort)]
            }
            Some(Event::CancelMergeOperation) if self.merge_busy => {
                vec![ToolRequest::GitCancel]
            }
            _ => Vec::new(),
        }
    }
}

fn tool_button<'a>(
    t: &lume_theme::LumeTheme,
    icon: Icon,
    title: &'static str,
    event: Option<Event>,
) -> Element<'a, ToolEvent> {
    let color = if event.is_some() { t.sec } else { t.faint };
    let mut widget = button(icon.view(15.0, color))
        .padding(Padding::from([4, 5]))
        .style(styles::hover_row(color, t.hover2, None, 4.0));
    if let Some(event) = event {
        widget = widget.on_press(ToolEvent::new(event));
    }
    styles::with_hint(t, widget, title)
}

fn section<'a>(
    label: &'static str,
    count: usize,
    theme: &lume_theme::LumeTheme,
) -> Element<'a, ToolEvent> {
    row![
        text(label).size(10.5).color(theme.sec),
        text(count.to_string()).size(10.5).color(theme.muted),
    ]
    .spacing(5)
    .into()
}

/// Pill «⎇ main» del prototipo (bg hover, bordo border2, r5). Mostra «▾»
/// soltanto se esiste un branch alternativo e la CLI è disponibile.
fn branch_pill<'a>(
    t: &lume_theme::LumeTheme,
    branch: &str,
    can_switch: bool,
) -> Element<'a, ToolEvent> {
    let (bg, border) = (t.hover, t.border2);
    let pill = container(
        text(format!("⎇ {branch}{}", if can_switch { " ▾" } else { "" }))
            .size(12)
            .color(t.ui),
    )
    .padding(Padding::from([3, 8]))
    .style(move |_| container::Style {
        background: Some(Background::Color(bg)),
        border: Border {
            radius: 5.0.into(),
            width: 1.0,
            color: border,
        },
        ..container::Style::default()
    });
    if can_switch {
        button(pill)
            .padding(0)
            .on_press(ToolEvent::new(Event::ToggleBranchPicker))
            .into()
    } else {
        pill.into()
    }
}

/// I tre stati della casella del prototipo: «✓», «–», vuota.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckState {
    Full,
    Partial,
    Empty,
}

/// Checkbox 14×14 del prototipo: piena = bg accent con «✓»; parziale = stesso
/// fondo con «–»; vuota = bg inset, bordo faint.
fn check_box<'a>(t: &lume_theme::LumeTheme, state: CheckState) -> Element<'a, ToolEvent> {
    let (bg, border) = match state {
        CheckState::Full | CheckState::Partial => (t.sel, t.accent),
        CheckState::Empty => (t.inset, t.faint),
    };
    let mark: Element<'a, ToolEvent> = match state {
        CheckState::Full => text("✓").size(10).color(t.accent_on).into(),
        CheckState::Partial => text("–").size(10).color(t.accent_on).into(),
        CheckState::Empty => Space::new().into(),
    };
    container(mark)
        .width(14)
        .height(14)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                radius: 3.0.into(),
                width: 1.0,
                color: border,
            },
            ..container::Style::default()
        })
        .into()
}

/// Pallino 7px dei commit recenti (accent per HEAD, faint per gli altri).
fn dot<'a>(color: Color) -> Element<'a, ToolEvent> {
    container(Space::new())
        .width(7)
        .height(7)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: Border {
                radius: 99.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

/// Età abbreviata per il blame (stessa logica di `relative_age`).
fn blame_age(timestamp: i64, now: i64) -> String {
    let secs = (now - timestamp).max(0) as u64;
    if secs < 60 {
        return "adesso".to_owned();
    }
    if secs < 3600 {
        return format!("{}m", secs / 60);
    }
    if secs < 86400 {
        return format!("{}h", secs / 3600);
    }
    if secs < 86400 * 30 {
        return format!("{}g", secs / 86400);
    }
    if secs < 86400 * 365 {
        return format!("{}me", secs / (86400 * 30));
    }
    format!("{}a", secs / (86400 * 365))
}

fn change_label(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "A",
        ChangeKind::Removed => "D",
        ChangeKind::Modified => "M",
        ChangeKind::TypeChanged => "T",
        ChangeKind::Renamed => "R",
        ChangeKind::Copied => "C",
        ChangeKind::IntentToAdd => "I",
        ChangeKind::Conflict => "!",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    /// ATTENZIONE: Con l'avvio pigro il pannello Git si apre prima che arrivi lo stato:
    /// senza snapshot e in caricamento non si dice «non è un repository».
    #[test]
    fn senza_snapshot_in_caricamento_non_si_dice_che_non_e_un_repository() {
        let mut tool = GitTool::new();
        assert_eq!(
            tool.messaggio_senza_snapshot(),
            "Apri una cartella per usare Git."
        );
        tool.update(&ToolEvent::new(Event::SetRoot(PathBuf::from("C:/repo"))));
        assert_eq!(tool.messaggio_senza_snapshot(), "Caricamento…");
    }

    /// Finito il caricamento senza snapshot (nessun repository) il messaggio
    /// cambia; un errore ha la precedenza su tutto.
    #[test]
    fn finito_il_caricamento_senza_repository_il_messaggio_lo_dice() {
        let mut tool = GitTool::new();
        tool.update(&ToolEvent::new(Event::SetRoot(PathBuf::from("C:/repo"))));
        tool.update(&ToolEvent::new(Event::Loaded(Err("guasto".into()), false)));
        assert_eq!(tool.messaggio_senza_snapshot(), "guasto");
    }

    fn file(path: &str, index: Option<ChangeKind>, worktree: Option<ChangeKind>) -> FileStatus {
        FileStatus {
            path: PathBuf::from(path),
            index,
            worktree,
        }
    }

    fn loaded(files: Vec<FileStatus>) -> GitTool {
        let mut tool = GitTool::new();
        tool.update(&ToolEvent::new(Event::Loaded(
            Ok(Snapshot {
                branch: Some("main".into()),
                local_branches: vec!["main".into(), "feature/test".into()],
                has_origin: true,
                merge_conflicts: None,
                has_upstream: false,
                files,
                commits: Vec::new(),
                head_message: Some("commit precedente\n\ncorpo".into()),
            }),
            false,
        )));
        tool.git_available = true;
        tool
    }

    fn command_titles(tool: &GitTool) -> Vec<&'static str> {
        tool.commands(None)
            .into_iter()
            .map(|command| command.title)
            .collect()
    }

    fn command<'a>(commands: &'a [Command], title: &str) -> &'a Command {
        commands
            .iter()
            .find(|command| command.title == title)
            .unwrap_or_else(|| panic!("missing command {title:?}"))
    }

    #[test]
    fn comandi_rete_richiedono_origin_e_cli_secondo_la_capacita() {
        let mut tool = loaded(Vec::new());
        tool.snapshot.as_mut().unwrap().has_origin = false;
        let titles = command_titles(&tool);
        for missing in ["Fetch", "Pull", "Push"] {
            assert!(!titles.contains(&missing), "{missing}: {titles:?}");
        }

        tool.snapshot.as_mut().unwrap().has_origin = true;
        tool.git_available = false;
        let titles = command_titles(&tool);
        assert!(titles.contains(&"Fetch"));
        assert!(!titles.contains(&"Pull"));
        assert!(!titles.contains(&"Push"));

        tool.git_available = true;
        tool.snapshot.as_mut().unwrap().has_upstream = true;
        let commands = tool.commands(None);
        assert_eq!(command(&commands, "Fetch").icon, Icon::Fetch);
        assert_eq!(command(&commands, "Pull").icon, Icon::Pull);
        assert_eq!(command(&commands, "Push").icon, Icon::Push);
        assert!(matches!(
            command(&commands, "Fetch").event,
            Some(Event::Run(lume_ui::tool::GitAction::Fetch))
        ));
    }

    #[test]
    fn i_comandi_di_rete_si_spengono_durante_unazione() {
        let mut tool = loaded(Vec::new());
        tool.snapshot.as_mut().unwrap().has_origin = true;
        tool.snapshot.as_mut().unwrap().has_upstream = true;
        tool.update(&ToolEvent::new(Event::ActionStarted(
            lume_ui::tool::GitAction::Fetch,
        )));
        let commands = tool.commands(None);
        for title in ["Fetch", "Pull", "Push"] {
            assert!(command(&commands, title).event.is_none(), "{title}");
        }
    }

    #[test]
    fn unisci_compare_dopo_pull_che_segnala_conflitti_solo_con_cli() {
        let mut tool = loaded(Vec::new());
        assert!(!command_titles(&tool).contains(&"Unisci"));
        tool.update(&ToolEvent::new(Event::ActionFinished(
            lume_ui::tool::GitAction::Pull,
            crate::git_actions::ActionReport {
                kind: crate::git_actions::NoticeKind::Warning,
                text: "il pull richiede merge".into(),
                needs_merge: true,
            },
        )));
        assert!(matches!(
            command(&tool.commands(None), "Unisci").event,
            Some(Event::Run(lume_ui::tool::GitAction::MergeUpstream))
        ));
        tool.git_available = false;
        assert!(!command_titles(&tool).contains(&"Unisci"));
    }

    #[test]
    fn azioni_git_cli_non_partono_se_git_non_e_disponibile() {
        let mut tool = loaded(Vec::new());
        tool.git_available = false;
        for event in [
            Event::Run(lume_ui::tool::GitAction::Push),
            Event::SwitchBranch("feature/test".into()),
            Event::MergeBranch("feature/test".into()),
            Event::StashApply {
                index: 0,
                pop: false,
            },
            Event::StashApply {
                index: 0,
                pop: true,
            },
        ] {
            assert!(tool.update(&ToolEvent::new(event)).is_empty());
        }
        assert!(!tool.push_busy);
        assert!(!tool.branch_switch_busy);
        assert!(!tool.merge_busy);
        assert!(!tool.stash_busy);

        let mut merge = tool.snapshot.clone().unwrap();
        merge.merge_conflicts = Some(Vec::new());
        tool.update(&ToolEvent::new(Event::Loaded(Ok(merge), false)));
        assert!(
            tool.update(&ToolEvent::new(Event::MergeContinue))
                .is_empty()
        );
        assert!(
            tool.update(&ToolEvent::new(Event::AskMergeAbort))
                .is_empty()
        );
        assert!(!tool.merge_abort_confirmation);
    }

    #[test]
    fn i_non_versionati_stanno_nel_loro_gruppo_e_uscirne_basta_metterli_in_stage() {
        let tool = loaded(vec![
            file("src/main.rs", None, Some(ChangeKind::Modified)),
            file("nuovo.txt", None, Some(ChangeKind::Added)),
            file("staged.txt", Some(ChangeKind::Added), None),
        ]);

        let changes: Vec<_> = tool
            .files_of(Group::Changes)
            .iter()
            .map(|f| f.path.clone())
            .collect();
        let unversioned: Vec<_> = tool
            .files_of(Group::Unversioned)
            .iter()
            .map(|f| f.path.clone())
            .collect();

        assert_eq!(
            unversioned,
            vec![PathBuf::from("nuovo.txt")],
            "solo l'untracked vero: «Added» sull'index e' un file nuovo gia' tracciato"
        );
        assert_eq!(
            changes,
            vec![PathBuf::from("src/main.rs"), PathBuf::from("staged.txt")]
        );
    }

    #[test]
    fn il_clic_sulla_riga_apre_il_diff_e_solo_la_casella_mette_in_stage() {
        let mut tool = loaded(vec![file("src/main.rs", None, Some(ChangeKind::Modified))]);
        let path = PathBuf::from("src/main.rs");

        let requests = tool.update(&ToolEvent::new(Event::OpenFile(path.clone())));
        assert!(
            matches!(
                requests.as_slice(),
                [ToolRequest::GitOpenDiff { path: p, commit: None }] if p == &path
            ),
            "il clic sulla riga non deve toccare lo staging"
        );

        let requests = tool.update(&ToolEvent::new(Event::ToggleFile(path.clone())));
        assert!(matches!(
            requests.as_slice(),
            [ToolRequest::GitStage(paths)] if paths == std::slice::from_ref(&path)
        ));

        let mut tool = loaded(vec![file("src/main.rs", Some(ChangeKind::Modified), None)]);
        let requests = tool.update(&ToolEvent::new(Event::ToggleFile(path.clone())));
        assert!(matches!(
            requests.as_slice(),
            [ToolRequest::GitUnstage(paths)] if paths == &[path]
        ));
    }

    #[test]
    fn la_casella_di_gruppo_agisce_su_tutto_il_gruppo_in_una_richiesta_sola() {
        let mut tool = loaded(vec![
            file("a.rs", None, Some(ChangeKind::Modified)),
            file("b.rs", Some(ChangeKind::Modified), None),
            file("nuovo.txt", None, Some(ChangeKind::Added)),
        ]);

        // Parziale: la casella completa, non svuota.
        let requests = tool.update(&ToolEvent::new(Event::ToggleGroup(Group::Changes)));
        assert!(
            matches!(
                requests.as_slice(),
                [ToolRequest::GitStage(paths)]
                    if paths == &[PathBuf::from("a.rs"), PathBuf::from("b.rs")]
            ),
            "un solo GitStage con tutti i path: N scritture dell'index sarebbero visibilmente lente"
        );

        // Tutto in stage: la casella svuota.
        let mut tool = loaded(vec![
            file("a.rs", Some(ChangeKind::Modified), None),
            file("b.rs", Some(ChangeKind::Modified), None),
        ]);
        let requests = tool.update(&ToolEvent::new(Event::ToggleGroup(Group::Changes)));
        assert!(matches!(
            requests.as_slice(),
            [ToolRequest::GitUnstage(paths)] if paths.len() == 2
        ));
    }

    #[test]
    fn ripristina_chiede_conferma_e_annullare_non_manda_niente() {
        let mut tool = loaded(vec![file("a.rs", None, Some(ChangeKind::Modified))]);

        assert!(
            tool.update(&ToolEvent::new(Event::AskDiscard)).is_empty(),
            "chiedere non butta via nulla"
        );
        assert!(tool.pending_discard.is_some());

        assert!(
            tool.update(&ToolEvent::new(Event::CancelDiscard))
                .is_empty()
        );
        assert!(tool.pending_discard.is_none());

        tool.update(&ToolEvent::new(Event::AskDiscard));
        let requests = tool.update(&ToolEvent::new(Event::ConfirmDiscard));
        assert!(matches!(
            requests.as_slice(),
            [ToolRequest::GitDiscard(paths)] if paths == &[PathBuf::from("a.rs")]
        ));
        assert!(
            tool.pending_discard.is_none(),
            "la conferma si consuma: un secondo clic non deve ripetere l'operazione"
        );
    }

    #[test]
    fn amend_precompila_loggetto_di_head_e_si_spegne_dopo_il_commit() {
        let mut tool = loaded(vec![file("a.rs", Some(ChangeKind::Modified), None)]);

        tool.update(&ToolEvent::new(Event::ToggleAmend));
        assert_eq!(
            tool.message, "commit precedente",
            "si precompila con l'oggetto, non con tutto il corpo"
        );

        let requests = tool.update(&ToolEvent::new(Event::Commit));
        assert!(matches!(
            requests.as_slice(),
            [ToolRequest::GitCommit { message, amend: true }] if message == "commit precedente"
        ));

        tool.update(&ToolEvent::new(Event::Loaded(
            Ok(Snapshot {
                branch: Some("main".into()),
                local_branches: vec!["main".into(), "feature/test".into()],
                has_origin: true,
                merge_conflicts: None,
                has_upstream: false,
                files: Vec::new(),
                commits: Vec::new(),
                head_message: Some("commit precedente".into()),
            }),
            true,
        )));
        assert!(
            !tool.amend,
            "un amend si consuma: restare acceso riscriverebbe il commit appena creato"
        );
        assert!(tool.message.is_empty());
    }

    #[test]
    fn fetch_non_parte_due_volte_e_annullare_attende_la_conferma() {
        let mut tool = loaded(Vec::new());
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::Run(lume_ui::tool::GitAction::Fetch)))
                .as_slice(),
            [ToolRequest::GitAction(lume_ui::tool::GitAction::Fetch)]
        ));
        assert!(tool.fetch_busy);
        assert!(
            tool.update(&ToolEvent::new(Event::Run(lume_ui::tool::GitAction::Fetch)))
                .is_empty()
        );
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::Cancel)).as_slice(),
            [ToolRequest::GitCancel]
        ));
        assert!(tool.fetch_busy, "il worker non ha ancora risposto");
        assert_eq!(
            tool.fetch_notice.as_deref(),
            Some("Annullamento del fetch richiesto…")
        );
        tool.update(&ToolEvent::new(Event::ActionFinished(
            lume_ui::tool::GitAction::Fetch,
            crate::git_actions::ActionReport::warning("Fetch annullato."),
        )));
        assert!(!tool.fetch_busy);
        assert_eq!(tool.notice.as_ref().unwrap().text, "Fetch annullato.");
        assert!(tool.update(&ToolEvent::new(Event::Cancel)).is_empty());
    }

    #[test]
    fn push_richiede_un_branch_e_lannullamento_attende_il_worker() {
        let mut tool = GitTool::new();
        assert!(
            tool.update(&ToolEvent::new(Event::Run(lume_ui::tool::GitAction::Push)))
                .is_empty()
        );
        let mut tool = loaded(Vec::new());
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::Run(lume_ui::tool::GitAction::Push)))
                .as_slice(),
            [ToolRequest::GitAction(lume_ui::tool::GitAction::Push)]
        ));
        assert!(tool.push_busy);
        assert!(
            tool.update(&ToolEvent::new(Event::Run(lume_ui::tool::GitAction::Push)))
                .is_empty()
        );
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::Cancel)).as_slice(),
            [ToolRequest::GitCancel]
        ));
        assert!(tool.push_busy, "il worker non ha ancora risposto");
        tool.update(&ToolEvent::new(Event::ActionFinished(
            lume_ui::tool::GitAction::Push,
            crate::git_actions::ActionReport::warning("Push annullato."),
        )));
        assert!(!tool.push_busy);
        assert_eq!(tool.notice.as_ref().unwrap().text, "Push annullato.");
    }

    #[test]
    fn cambio_branch_consente_solo_un_locale_elencato_e_si_annulla() {
        let mut tool = loaded(Vec::new());
        tool.update(&ToolEvent::new(Event::ToggleBranchPicker));
        assert!(tool.branch_picker_open);
        assert!(
            tool.update(&ToolEvent::new(Event::SwitchBranch("main".into())))
                .is_empty()
        );
        assert!(
            tool.update(&ToolEvent::new(Event::SwitchBranch("../../cattivo".into())))
                .is_empty()
        );
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::SwitchBranch("feature/test".into())))
                .as_slice(),
            [ToolRequest::GitAction(lume_ui::tool::GitAction::SwitchBranch(branch))] if branch == "feature/test"
        ));
        assert!(tool.branch_switch_busy);
        assert!(
            tool.update(&ToolEvent::new(Event::SwitchBranch("feature/test".into())))
                .is_empty()
        );
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::CancelBranchSwitch))
                .as_slice(),
            [ToolRequest::GitCancel]
        ));
        tool.update(&ToolEvent::new(Event::ActionFinished(
            lume_ui::tool::GitAction::SwitchBranch("feature/test".into()),
            crate::git_actions::ActionReport::ok("Branch attivo: feature/test"),
        )));
        assert!(!tool.branch_switch_busy);
        assert!(!tool.branch_picker_open);
    }

    #[test]
    fn merge_bar_blocca_continue_con_conflitti_e_chiede_conferma_per_abort() {
        let mut tool = loaded(vec![file(
            "a.txt",
            Some(ChangeKind::Conflict),
            Some(ChangeKind::Conflict),
        )]);
        tool.update(&ToolEvent::new(Event::Loaded(
            Ok(Snapshot {
                branch: Some("main".into()),
                local_branches: vec!["main".into(), "feature/test".into()],
                has_origin: true,
                merge_conflicts: Some(vec![PathBuf::from("a.txt")]),
                has_upstream: false,
                files: vec![file(
                    "a.txt",
                    Some(ChangeKind::Conflict),
                    Some(ChangeKind::Conflict),
                )],
                commits: Vec::new(),
                head_message: None,
            }),
            false,
        )));
        assert!(
            tool.update(&ToolEvent::new(Event::MergeContinue))
                .is_empty()
        );
        assert!(
            tool.update(&ToolEvent::new(Event::AskMergeAbort))
                .is_empty()
        );
        assert!(tool.merge_abort_confirmation);
        assert!(
            tool.update(&ToolEvent::new(Event::CancelMergeAbort))
                .is_empty()
        );
        assert!(!tool.merge_abort_confirmation);
        assert!(
            tool.update(&ToolEvent::new(Event::AskMergeAbort))
                .is_empty()
        );
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::ConfirmMergeAbort))
                .as_slice(),
            [ToolRequest::GitAction(lume_ui::tool::GitAction::MergeAbort)]
        ));
        assert!(tool.merge_busy);
        assert!(!tool.merge_abort_confirmation);
        tool.update(&ToolEvent::new(Event::ActionFinished(
            lume_ui::tool::GitAction::MergeAbort,
            crate::git_actions::ActionReport::ok("Merge annullato."),
        )));
        assert!(!tool.merge_busy);
        assert!(!tool.merge_abort_confirmation);

        tool.update(&ToolEvent::new(Event::Loaded(
            Ok(Snapshot {
                branch: Some("main".into()),
                local_branches: vec!["main".into(), "feature/test".into()],
                has_origin: true,
                merge_conflicts: Some(Vec::new()),
                has_upstream: false,
                files: vec![file("a.txt", Some(ChangeKind::Modified), None)],
                commits: Vec::new(),
                head_message: None,
            }),
            false,
        )));
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::MergeContinue))
                .as_slice(),
            [ToolRequest::GitAction(
                lume_ui::tool::GitAction::MergeContinue(_)
            )]
        ));
    }

    #[test]
    fn merge_da_branch_locale_si_puo_annullare_e_risultato_aggiorna_stato() {
        let mut tool = loaded(Vec::new());
        tool.update(&ToolEvent::new(Event::ToggleBranchPicker));
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::MergeBranch("feature/test".into())))
                .as_slice(),
            [ToolRequest::GitAction(lume_ui::tool::GitAction::MergeBranch(name))] if name == "feature/test"
        ));
        assert!(tool.merge_busy);
        assert!(
            tool.update(&ToolEvent::new(Event::MergeBranch("feature/test".into())))
                .is_empty()
        );
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::CancelMergeOperation))
                .as_slice(),
            [ToolRequest::GitCancel]
        ));
        tool.update(&ToolEvent::new(Event::ActionFinished(
            lume_ui::tool::GitAction::MergeBranch("feature/test".into()),
            crate::git_actions::ActionReport::warning("Conflitti: a.txt"),
        )));
        assert!(!tool.merge_busy);
        assert!(tool.notice.as_ref().unwrap().text.contains("a.txt"));
    }

    #[test]
    fn apply_pop_stash_blocca_doppie_richieste_e_lannullamento_attende_il_worker() {
        let mut tool = loaded(Vec::new());
        let richiesta = Event::StashApply {
            index: 2,
            pop: true,
        };
        assert!(matches!(
            tool.update(&ToolEvent::new(richiesta.clone())).as_slice(),
            [ToolRequest::GitAction(lume_ui::tool::GitAction::StashPop(
                2
            ))]
        ));
        assert!(tool.stash_busy);
        assert!(tool.update(&ToolEvent::new(richiesta)).is_empty());
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::CancelStashOperation))
                .as_slice(),
            [ToolRequest::GitCancel]
        ));
        assert!(tool.stash_busy);
        assert_eq!(
            tool.stash_notice.as_deref(),
            Some("Annullamento richiesto…")
        );
    }

    #[test]
    fn cambio_progetto_libera_lo_stato_visivo_di_apply_stash() {
        let mut tool = GitTool::new();
        tool.update(&ToolEvent::new(Event::StashApply {
            index: 0,
            pop: false,
        }));
        tool.update(&ToolEvent::new(Event::SetRoot(PathBuf::from("/altro"))));
        assert!(!tool.stash_busy);
        assert!(tool.stash_notice.is_none());
    }

    #[test]
    fn pull_e_disponibile_solo_con_upstream_e_annullamento_attende_il_worker() {
        let mut tool = loaded(Vec::new());
        assert!(
            tool.update(&ToolEvent::new(Event::Run(lume_ui::tool::GitAction::Pull)))
                .is_empty()
        );
        tool.update(&ToolEvent::new(Event::Loaded(
            Ok(Snapshot {
                branch: Some("main".into()),
                local_branches: vec!["main".into(), "feature/test".into()],
                has_origin: true,
                merge_conflicts: None,
                has_upstream: true,
                files: Vec::new(),
                commits: Vec::new(),
                head_message: None,
            }),
            false,
        )));
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::Run(lume_ui::tool::GitAction::Pull)))
                .as_slice(),
            [ToolRequest::GitAction(lume_ui::tool::GitAction::Pull)]
        ));
        assert!(tool.pull_busy);
        assert!(
            tool.update(&ToolEvent::new(Event::Run(lume_ui::tool::GitAction::Pull)))
                .is_empty()
        );
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::Cancel)).as_slice(),
            [ToolRequest::GitCancel]
        ));
        assert!(tool.pull_busy);
        tool.update(&ToolEvent::new(Event::ActionFinished(
            lume_ui::tool::GitAction::Pull,
            crate::git_actions::ActionReport::warning("Pull annullato."),
        )));
        assert!(!tool.pull_busy);
        assert_eq!(tool.notice.as_ref().unwrap().text, "Pull annullato.");
    }

    #[test]
    fn pull_con_conflitti_offre_merge_sull_upstream() {
        let mut tool = loaded(Vec::new());
        tool.snapshot.as_mut().unwrap().has_upstream = true;
        assert!(
            tool.update(&ToolEvent::new(Event::Run(
                lume_ui::tool::GitAction::MergeUpstream
            )))
            .is_empty()
        );

        tool.update(&ToolEvent::new(Event::ActionFinished(
            lume_ui::tool::GitAction::Pull,
            crate::git_actions::ActionReport {
                kind: crate::git_actions::NoticeKind::Warning,
                text: "conflitto nel pull".into(),
                needs_merge: true,
            },
        )));
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::Run(
                lume_ui::tool::GitAction::MergeUpstream
            )))
            .as_slice(),
            [ToolRequest::GitAction(
                lume_ui::tool::GitAction::MergeUpstream
            )]
        ));
        assert!(tool.merge_busy);
    }

    #[test]
    fn apply_stash_mostra_i_percorsi_in_conflitto_e_non_rimuove_le_voci() {
        let mut tool = GitTool::new();
        tool.stash_busy = true;
        let entries = vec![lume_git::StashEntry {
            index: 0,
            message: "wip".into(),
            commit_id: "abc1234".into(),
        }];
        tool.stash_entries = entries;
        tool.update(&ToolEvent::new(Event::ActionFinished(
            lume_ui::tool::GitAction::StashApply(0),
            crate::git_actions::ActionReport::warning("Conflitti in: src/main.rs"),
        )));
        assert!(!tool.stash_busy);
        assert_eq!(tool.stash_entries.len(), 1);
        assert_eq!(
            tool.notice.as_ref().unwrap().text,
            "Conflitti in: src/main.rs"
        );
        assert!(tool.stash_error.is_none());
    }

    #[test]
    fn salva_stash_blocca_doppio_click_e_mostra_conferma() {
        let mut tool = loaded(vec![file("a.txt", None, Some(ChangeKind::Modified))]);
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::Run(
                lume_ui::tool::GitAction::StashPush(None)
            )))
            .as_slice(),
            [ToolRequest::GitAction(lume_ui::tool::GitAction::StashPush(
                None
            ))]
        ));
        assert!(tool.stash_busy);
        assert!(
            tool.update(&ToolEvent::new(Event::Run(
                lume_ui::tool::GitAction::StashPush(None)
            )))
            .is_empty()
        );
        tool.update(&ToolEvent::new(Event::ActionFinished(
            lume_ui::tool::GitAction::StashPush(None),
            crate::git_actions::ActionReport::ok("Stash salvato; worktree ripristinato."),
        )));
        assert!(!tool.stash_busy);
        assert_eq!(
            tool.notice.as_ref().unwrap().text,
            "Stash salvato; worktree ripristinato."
        );
    }

    #[test]
    fn cambio_progetto_non_mostra_il_fetch_del_progetto_precedente() {
        let mut tool = loaded(Vec::new());
        tool.update(&ToolEvent::new(Event::Run(lume_ui::tool::GitAction::Fetch)));
        tool.update(&ToolEvent::new(Event::SetRoot(PathBuf::from("C:/altro"))));
        assert!(!tool.fetch_busy);
        assert!(tool.fetch_notice.is_none());
    }

    #[test]
    fn un_fetch_fallito_libera_lo_stato_occupato() {
        let mut tool = loaded(Vec::new());
        tool.update(&ToolEvent::new(Event::Run(lume_ui::tool::GitAction::Fetch)));
        tool.update(&ToolEvent::new(Event::ActionFinished(
            lume_ui::tool::GitAction::Fetch,
            crate::git_actions::ActionReport::error("Errore: rete assente"),
        )));
        assert!(!tool.fetch_busy);
        assert_eq!(tool.notice.as_ref().unwrap().text, "Errore: rete assente");
    }

    #[test]
    fn amend_non_sovrascrive_un_messaggio_gia_scritto() {
        let mut tool = loaded(Vec::new());
        tool.update(&ToolEvent::new(Event::MessageChanged(
            "il mio testo".into(),
        )));
        tool.update(&ToolEvent::new(Event::ToggleAmend));
        assert_eq!(tool.message, "il mio testo");
    }

    #[test]
    fn espandi_e_comprimi_tutto_agiscono_su_entrambi_i_gruppi() {
        let mut tool = loaded(vec![file("a.rs", None, Some(ChangeKind::Modified))]);
        assert!(tool.collapsed.is_empty());

        tool.update(&ToolEvent::new(Event::CollapseAll));
        assert_eq!(tool.collapsed.len(), 2);

        tool.update(&ToolEvent::new(Event::ExpandAll));
        assert!(tool.collapsed.is_empty());

        tool.update(&ToolEvent::new(Event::CollapseGroup(Group::Changes)));
        assert_eq!(
            tool.collapsed.iter().copied().collect::<Vec<_>>(),
            vec![Group::Changes]
        );
    }

    #[test]
    fn chiusura_git_rilascia_cache_e_riapertura_rilegge_git_e_richiede_refresh() {
        let mut tool = loaded(vec![file("a.rs", None, Some(ChangeKind::Modified))]);
        tool.root = Some(PathBuf::from("repo"));
        tool.update(&ToolEvent::new(Event::MessageChanged("bozza".into())));
        tool.stash_entries.push(lume_git::StashEntry {
            index: 0,
            message: "WIP".into(),
            commit_id: "abc".into(),
        });
        tool.stash_panel_open = true;
        tool.blame_path = Some(PathBuf::from("a.rs"));

        assert!(tool.on_visibility_changed(false).is_empty());
        assert!(tool.snapshot.is_none());
        assert!(tool.stash_entries.is_empty() && !tool.stash_panel_open);
        assert!(tool.blame_path.is_none());
        assert_eq!(tool.message, "bozza");

        let requests = tool.on_visibility_changed(true);
        assert!(matches!(requests.as_slice(), [ToolRequest::GitRefresh]));
        assert!(tool.git_available);
        assert!(tool.busy);
    }

    #[test]
    fn apertura_git_non_duplica_caricamento_in_volo_o_senza_root() {
        let mut loading = GitTool::new();
        loading.update(&ToolEvent::new(Event::SetRoot(PathBuf::from("repo"))));
        assert!(loading.on_visibility_changed(true).is_empty());
        assert!(GitTool::new().on_visibility_changed(true).is_empty());
    }

    #[test]
    fn risultato_stash_tardivo_non_ricrea_la_cache_dopo_la_chiusura() {
        let mut tool = loaded(Vec::new());
        tool.stash_panel_open = false;
        tool.update(&ToolEvent::new(Event::StashListLoaded(Ok(vec![
            lume_git::StashEntry {
                index: 0,
                message: "WIP".into(),
                commit_id: "abc".into(),
            },
        ]))));
        assert!(tool.stash_entries.is_empty());
    }

    #[test]
    fn chiudere_git_non_revoca_una_azione_lunga() {
        let mut tool = loaded(Vec::new());
        tool.update(&ToolEvent::new(Event::Run(lume_ui::tool::GitAction::Fetch)));
        tool.on_visibility_changed(false);
        assert!(matches!(tool.action, Some(lume_ui::tool::GitAction::Fetch)));
        assert!(tool.fetch_busy);
    }
}
