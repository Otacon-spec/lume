//! Dock «Log Git» (prototipo v2): albero dei branch (225px) · lista commit
//! con grafo a corsie e filtro «Testo o hash» · dettagli (235px) con i FILE
//! MODIFICATI del commit selezionato. Ogni dato arriva da gitoxide tramite
//! `ToolRequest::GitLog` / `ToolRequest::GitCommitFiles`; i controlli senza
//! backend del prototipo (Console, Worktrees, filtri Branch/Utente/Data,
//! strip icone) non sono renderizzati.

use std::path::{Path, PathBuf};

use iced::font::Weight;
use iced::widget::{Space, button, canvas, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use lume_git::{Branches, ChangeKind, ChangedFile, CommitSummary};
use lume_theme::LumeTheme;
use lume_ui::commit_graph::{self, GraphRow, ROW_H};
use lume_ui::icons::Icon;
use lume_ui::tool::{Tool, ToolCtx, ToolEvent, ToolId, ToolKind, ToolRequest};
use lume_ui::{MONO, styles, ui_font};

pub const ID: ToolId = ToolId("gitlog");
/// Riferimento di default del log.
pub const HEAD: &str = "HEAD";
/// Commit caricati per riferimento.
pub const LOG_LIMIT: usize = 200;

const TOOLBAR_H: f32 = 32.0;
const TREE_W: f32 = 225.0;
const DETAILS_W: f32 = 235.0;

/// Log caricato per un riferimento: branch + commit (dal più recente).
#[derive(Clone, Debug)]
pub struct LogSnapshot {
    pub reference: String,
    pub branches: Branches,
    pub commits: Vec<CommitSummary>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Local,
    Remote,
}

#[derive(Clone, Debug)]
pub enum Event {
    SetRoot(PathBuf),
    Loaded(Result<LogSnapshot, String>),
    /// File modificati di un commit (id, risultato).
    FilesLoaded(String, Result<Vec<ChangedFile>, String>),
    BranchFilter(String),
    TextFilter(String),
    /// Click su HEAD o su un branch: ricarica il log da quel riferimento.
    SelectReference(String),
    /// Cambia al branch locale selezionato dal Log Git.
    SwitchBranch(String),
    /// Crea un branch locale dal remoto selezionato e ci passa.
    SwitchRemoteBranch {
        remote: String,
        name: String,
    },
    SelectCommit(String),
    ToggleSection(Section),
    /// Riga di FILE MODIFICATI: apre il diff con quel commit come base.
    OpenDiff {
        path: PathBuf,
        commit: String,
    },
}

/// Chip mostrato accanto al messaggio quando un branch punta al commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chip {
    /// Branch corrente (bg `sel`, testo `accent`).
    Current(String),
    /// Altro branch locale (bg `good_bg`, testo `good`).
    Local(String),
    /// Branch remoto «origin/main» (contorno `border2`, testo `sec`).
    Remote(String),
}

pub struct GitLogTool {
    root: Option<PathBuf>,
    snapshot: Option<LogSnapshot>,
    rows: Vec<GraphRow>,
    error: Option<String>,
    busy: bool,
    branch_filter: String,
    text_filter: String,
    selected: Option<String>,
    files: Option<(String, Result<Vec<ChangedFile>, String>)>,
    local_open: bool,
    remote_open: bool,
    git_available: bool,
    /// Riferimento da ripristinare quando il dock viene riaperto.
    reopen_reference: String,
}

impl GitLogTool {
    pub fn new() -> Self {
        Self {
            root: None,
            snapshot: None,
            rows: Vec::new(),
            error: None,
            busy: false,
            branch_filter: String::new(),
            text_filter: String::new(),
            selected: None,
            files: None,
            local_open: true,
            remote_open: true,
            git_available: false,
            reopen_reference: HEAD.to_owned(),
        }
    }

    fn reference(&self) -> &str {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.reference.as_str())
            .unwrap_or(HEAD)
    }

    fn selected_commit(&self) -> Option<&CommitSummary> {
        let snapshot = self.snapshot.as_ref()?;
        let id = self.selected.as_deref()?;
        snapshot.commits.iter().find(|commit| commit.id == id)
    }

    fn release(&mut self) {
        if !self.busy {
            self.reopen_reference = self.reference().to_owned();
        }
        self.snapshot = None;
        self.rows.clear();
        self.error = None;
        self.busy = false;
        self.selected = None;
        self.files = None;
        self.git_available = false;
    }

    // ------------------------------------------------------------ colonne

    fn toolbar<'a>(&self, t: &'a LumeTheme) -> Element<'a, ToolEvent> {
        let (sel, accent_on) = (t.sel, t.accent_on);
        let pill = container(text("Log").size(12).color(accent_on))
            .padding(Padding::from([3, 10]))
            .style(move |_| container::Style {
                background: Some(Background::Color(sel)),
                border: Border {
                    radius: 6.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            });
        container(row![pill].align_y(Alignment::Center))
            .width(Length::Fill)
            .height(TOOLBAR_H)
            .padding(Padding::from([0, 10]))
            .align_y(Alignment::Center)
            .into()
    }

    fn branch_tree<'a>(&'a self, t: &'a LumeTheme) -> Element<'a, ToolEvent> {
        let input = text_input("Branch o tag", &self.branch_filter)
            .on_input(|value| ToolEvent::new(Event::BranchFilter(value)))
            .padding(Padding::from([5, 8]))
            .size(11.5)
            .style(styles::input(t));

        let mut list = column![].spacing(0);
        let filter = self.branch_filter.trim().to_lowercase();
        let matches = |name: &str| filter.is_empty() || name.to_lowercase().contains(&filter);

        let head_active = self.reference() == HEAD;
        list = list.push(tree_row(
            t,
            row![
                text("HEAD").size(12).color(t.text),
                text("(branch corrente)").size(12).color(t.muted),
            ]
            .spacing(4)
            .into(),
            6.0,
            head_active,
            Some(Event::SelectReference(HEAD.into())),
        ));

        let Some(snapshot) = &self.snapshot else {
            return column![input, list].spacing(8).into();
        };
        let head = snapshot.branches.head.as_deref();

        list = list.push(section_row(t, "Locali", self.local_open, Section::Local));
        if self.local_open {
            let mut last_group: Option<&str> = None;
            for branch in snapshot.branches.local.iter().filter(|b| matches(&b.name)) {
                let current = head == Some(branch.name.as_str());
                let active = self.reference() == branch.name;
                let (group, leaf) = match branch.name.split_once('/') {
                    Some((group, leaf)) => (Some(group), leaf),
                    None => (None, branch.name.as_str()),
                };
                if let Some(group) = group
                    && last_group != Some(group)
                {
                    list = list.push(
                        container(text(format!("▾ {group}")).size(12).color(t.tree))
                            .padding(pad(3.0, 6.0, 3.0, 20.0)),
                    );
                }
                last_group = group;
                let indent = if group.is_some() { 34.0 } else { 20.0 };
                let mut label = row![].spacing(6).align_y(Alignment::Center);
                if current {
                    label = label.push(Icon::BranchTag.view(11.0, t.warn));
                }
                label = label.push(text(leaf).size(12).color(t.text));
                let select = tree_row(
                    t,
                    label.into(),
                    indent,
                    active || current,
                    Some(Event::SelectReference(branch.name.clone())),
                );
                let row = if self.git_available && !current {
                    row![
                        select,
                        button(text("Cambia").size(9).color(t.sec))
                            .padding(Padding::from([2, 5]))
                            .on_press(ToolEvent::new(Event::SwitchBranch(branch.name.clone())))
                    ]
                    .align_y(Alignment::Center)
                } else {
                    row![select].align_y(Alignment::Center)
                };
                list = list.push(row);
            }
        }

        list = list.push(section_row(t, "Remoti", self.remote_open, Section::Remote));
        if self.remote_open {
            let mut last_remote: Option<&str> = None;
            for branch in snapshot.branches.remote.iter().filter(|b| matches(&b.name)) {
                if last_remote != Some(branch.remote.as_str()) {
                    list = list.push(
                        container(text(format!("▾ {}", branch.remote)).size(12).color(t.tree))
                            .padding(pad(3.0, 6.0, 3.0, 20.0)),
                    );
                    last_remote = Some(branch.remote.as_str());
                }
                let reference = format!("{}/{}", branch.remote, branch.name);
                let active = self.reference() == reference;
                let select = tree_row(
                    t,
                    text(&branch.name).size(12).color(t.text).into(),
                    34.0,
                    active,
                    Some(Event::SelectReference(reference)),
                );
                let row = if self.git_available {
                    row![
                        select,
                        button(text("Crea").size(9).color(t.sec))
                            .padding(Padding::from([2, 5]))
                            .on_press(ToolEvent::new(Event::SwitchRemoteBranch {
                                remote: branch.remote.clone(),
                                name: branch.name.clone(),
                            }))
                    ]
                    .align_y(Alignment::Center)
                } else {
                    row![select].align_y(Alignment::Center)
                };
                list = list.push(row);
            }
        }

        column![
            input,
            scrollable(list).width(Length::Fill).height(Length::Fill)
        ]
        .spacing(8)
        .into()
    }

    fn commit_list<'a>(&'a self, t: &'a LumeTheme) -> Element<'a, ToolEvent> {
        let input = text_input("Testo o hash", &self.text_filter)
            .on_input(|value| ToolEvent::new(Event::TextFilter(value)))
            .padding(Padding::from([5, 8]))
            .size(11.5)
            .style(styles::input(t));
        let header = container(input)
            .width(Length::Fill)
            .padding(Padding::from([8, 10]));

        let mut list = column![];
        let Some(snapshot) = &self.snapshot else {
            let message = self.error.as_deref().unwrap_or(if self.busy {
                "Caricamento…"
            } else if self.root.is_some() {
                "La cartella aperta non è un repository Git."
            } else {
                "Apri una cartella per vedere il log."
            });
            list = list.push(
                container(text(message).size(12).color(t.muted)).padding(Padding::from([8, 10])),
            );
            return column![header, hsep(t), list].into();
        };
        if let Some(error) = &self.error {
            list = list
                .push(container(text(error).size(11).color(t.bad)).padding(Padding::from([4, 10])));
        }

        let palette = [t.accent, t.good, t.warn];
        let lanes = self.rows.iter().map(|row| row.lane + 1).max().unwrap_or(1);
        let cell_w = commit_graph::cell_width(lanes);
        let filter = self.text_filter.trim().to_lowercase();
        for (commit, graph_row) in snapshot.commits.iter().zip(&self.rows) {
            if !matches_text(commit, &filter) {
                continue;
            }
            let selected = self.selected.as_deref() == Some(commit.id.as_str());
            let cell = canvas(commit_graph::graph_cell(graph_row, &palette))
                .width(cell_w)
                .height(ROW_H);
            let mut content = row![
                cell,
                container(
                    text(&commit.subject)
                        .size(12)
                        .color(t.text)
                        .wrapping(text::Wrapping::None)
                )
                .clip(true)
                .width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            for chip in chips_for(&snapshot.branches, &commit.id) {
                content = content.push(chip_view(t, chip));
            }
            content = content
                .push(text(&commit.author_name).size(11).color(t.sec))
                .push(
                    text(format_commit_date(commit.author_time, commit.author_offset))
                        .size(11)
                        .font(MONO)
                        .color(t.muted),
                );
            list = list.push(
                button(content)
                    .width(Length::Fill)
                    .height(ROW_H)
                    .padding(pad(0.0, 10.0, 0.0, 6.0))
                    .style(styles::hover_row(
                        t.text,
                        t.hover,
                        selected.then_some(t.sel),
                        0.0,
                    ))
                    .on_press(ToolEvent::new(Event::SelectCommit(commit.id.clone()))),
            );
        }

        column![
            header,
            hsep(t),
            scrollable(list).width(Length::Fill).height(Length::Fill)
        ]
        .into()
    }

    /// Il diff aperto arriva dal ToolCtx: lo possiede l'app, qui serve solo
    /// a evidenziare la riga corrispondente.
    fn details<'a>(
        &'a self,
        t: &'a LumeTheme,
        open_diff: Option<(&Path, Option<&str>)>,
    ) -> Element<'a, ToolEvent> {
        let Some(commit) = self.selected_commit() else {
            return container(text("Seleziona un commit").size(12).color(t.muted))
                .padding(12)
                .into();
        };
        let short = commit.id.get(..7).unwrap_or(&commit.id);
        let mut content = column![
            text(&commit.subject)
                .size(12)
                .font(ui_font(Weight::Medium))
                .color(t.text),
            text(short).size(11).font(MONO).color(t.muted),
            text(format!(
                "{} · {}",
                commit.author_name,
                format_commit_date(commit.author_time, commit.author_offset)
            ))
            .size(11)
            .color(t.sec),
        ]
        .spacing(4);
        if let Some(body) = commit
            .body
            .as_deref()
            .filter(|body| !body.trim().is_empty())
        {
            content = content.push(text(body.trim()).size(11).color(t.muted));
        }
        content = content.push(
            container(text("FILE MODIFICATI").size(10.5).color(t.sec))
                .padding(pad(8.0, 0.0, 0.0, 0.0)),
        );
        match &self.files {
            Some((id, Ok(files))) if *id == commit.id => {
                if files.is_empty() {
                    content = content.push(text("Nessun file").size(11.5).color(t.muted));
                }
                for file in files {
                    let (letter, color) = change_letter(file.kind, t);
                    let label = row![
                        text(letter).size(11).font(MONO).color(color).width(12),
                        container(
                            text(file.path.to_string_lossy().into_owned())
                                .size(11.5)
                                .color(t.ui)
                                .wrapping(text::Wrapping::None)
                        )
                        .clip(true)
                        .width(Length::Fill),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center);
                    let mut open = button(label)
                        .width(Length::Fill)
                        .padding(Padding::from([3, 4]))
                        .style(styles::hover_row(t.ui, t.hover, None, 4.0));
                    let selected_open = open_diff.is_some_and(|(path, base)| {
                        path == file.path && base == Some(commit.id.as_str())
                    });
                    if selected_open {
                        open = open.style(styles::hover_row(t.ui, t.hover, Some(t.sel), 4.0));
                    }
                    open = open.on_press(ToolEvent::new(Event::OpenDiff {
                        path: file.path.clone(),
                        commit: commit.id.clone(),
                    }));
                    content = content.push(open);
                }
            }
            Some((id, Err(error))) if *id == commit.id => {
                content = content.push(text(error).size(11).color(t.bad));
            }
            _ => content = content.push(text("Caricamento…").size(11.5).color(t.muted)),
        }
        scrollable(container(content).padding(12).width(Length::Fill))
            .height(Length::Fill)
            .into()
    }
}

impl Default for GitLogTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for GitLogTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Git"
    }

    fn icon(&self) -> Icon {
        Icon::GitLog
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Dock
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
            return vec![ToolRequest::GitLog(self.reopen_reference.clone())];
        }
        Vec::new()
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        let tree = container(self.branch_tree(t))
            .width(TREE_W)
            .height(Length::Fill)
            .padding(8);
        let details = container(self.details(t, ctx.open_diff))
            .width(DETAILS_W)
            .height(Length::Fill);
        column![
            self.toolbar(t),
            hsep(t),
            row![
                tree,
                vsep(t),
                container(self.commit_list(t))
                    .width(Length::Fill)
                    .height(Length::Fill),
                vsep(t),
                details,
            ]
            .height(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn update(&mut self, event: &ToolEvent) -> Vec<ToolRequest> {
        match event.downcast::<Event>() {
            Some(Event::SetRoot(root)) => {
                self.root = Some(root.clone());
                self.reopen_reference = HEAD.to_owned();
                self.snapshot = None;
                self.rows.clear();
                self.error = None;
                self.selected = None;
                self.files = None;
                self.busy = true;
                Vec::new()
            }
            Some(Event::Loaded(result)) => {
                self.busy = false;
                match result {
                    Ok(snapshot) => {
                        self.reopen_reference = snapshot.reference.clone();
                        self.rows = commit_graph::assign_lanes(
                            &snapshot
                                .commits
                                .iter()
                                .map(|commit| (commit.id.clone(), commit.parent_ids.clone()))
                                .collect::<Vec<_>>(),
                        );
                        if let Some(selected) = &self.selected
                            && !snapshot.commits.iter().any(|c| &c.id == selected)
                        {
                            self.selected = None;
                        }
                        self.snapshot = Some(snapshot.clone());
                        self.error = None;
                    }
                    Err(error) => self.error = Some(error.clone()),
                }
                Vec::new()
            }
            Some(Event::FilesLoaded(id, result)) => {
                if self.selected.as_deref() == Some(id.as_str()) {
                    self.files = Some((id.clone(), result.clone()));
                }
                Vec::new()
            }
            Some(Event::BranchFilter(value)) => {
                self.branch_filter = value.clone();
                Vec::new()
            }
            Some(Event::TextFilter(value)) => {
                self.text_filter = value.clone();
                Vec::new()
            }
            Some(Event::SelectReference(reference)) => {
                self.reopen_reference = reference.clone();
                self.busy = true;
                vec![ToolRequest::GitLog(reference.clone())]
            }
            Some(Event::SwitchBranch(name)) if self.git_available && !self.busy => {
                vec![ToolRequest::GitAction(
                    lume_ui::tool::GitAction::SwitchBranch(name.clone()),
                )]
            }
            Some(Event::SwitchRemoteBranch { remote, name })
                if self.git_available && !self.busy =>
            {
                vec![ToolRequest::GitAction(
                    lume_ui::tool::GitAction::SwitchRemoteBranch {
                        remote: remote.clone(),
                        name: name.clone(),
                    },
                )]
            }
            Some(Event::SwitchBranch(_) | Event::SwitchRemoteBranch { .. }) => Vec::new(),
            Some(Event::SelectCommit(id)) => {
                self.selected = Some(id.clone());
                if self.files.as_ref().is_some_and(|(cached, _)| cached == id) {
                    Vec::new()
                } else {
                    self.files = None;
                    vec![ToolRequest::GitCommitFiles(id.clone())]
                }
            }
            Some(Event::ToggleSection(Section::Local)) => {
                self.local_open = !self.local_open;
                Vec::new()
            }
            Some(Event::ToggleSection(Section::Remote)) => {
                self.remote_open = !self.remote_open;
                Vec::new()
            }
            Some(Event::OpenDiff { path, commit }) => vec![ToolRequest::GitOpenDiff {
                path: path.clone(),
                commit: Some(commit.clone()),
            }],
            None => Vec::new(),
        }
    }
}

// ------------------------------------------------------------ helper puri

/// Chip dei branch che puntano al commit: prima il corrente, poi i locali,
/// poi i remoti.
pub fn chips_for(branches: &Branches, id: &str) -> Vec<Chip> {
    let head = branches.head.as_deref();
    let mut chips = Vec::new();
    for branch in branches.local.iter().filter(|b| b.tip == id) {
        if head == Some(branch.name.as_str()) {
            chips.insert(0, Chip::Current(branch.name.clone()));
        } else {
            chips.push(Chip::Local(branch.name.clone()));
        }
    }
    for branch in branches.remote.iter().filter(|b| b.tip == id) {
        chips.push(Chip::Remote(format!("{}/{}", branch.remote, branch.name)));
    }
    chips
}

/// Filtro «Testo o hash»: sottostringa (case-insensitive) del messaggio o
/// prefisso dell'hash.
pub fn matches_text(commit: &CommitSummary, filter: &str) -> bool {
    filter.is_empty()
        || commit.subject.to_lowercase().contains(filter)
        || commit.id.starts_with(filter)
}

/// «dd/MM, HH:mm» nell'ora locale dell'autore (offset del commit).
pub fn format_commit_date(seconds: i64, offset: i32) -> String {
    let local = seconds + i64::from(offset);
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (_, month, day) = civil_from_days(days);
    format!(
        "{day:02}/{month:02}, {:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
}

/// Età relativa di un istante rispetto a `now` (secondi unix).
pub fn relative_age(seconds: i64, now: i64) -> String {
    let delta = (now - seconds).max(0);
    match delta {
        0..=59 => "adesso".into(),
        60..=3_599 => format!("{} min fa", delta / 60),
        3_600..=86_399 => format!("{} h fa", delta / 3_600),
        86_400..=172_799 => "ieri".into(),
        172_800..=2_591_999 => format!("{} g fa", delta / 86_400),
        _ => {
            let (_, month, day) = civil_from_days(seconds.div_euclid(86_400));
            format!("{day:02}/{month:02}")
        }
    }
}

/// Giorni dal 1970-01-01 → (anno, mese, giorno) del calendario gregoriano
/// (algoritmo di Howard Hinnant, senza dipendenze).
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn change_letter(kind: ChangeKind, t: &LumeTheme) -> (&'static str, Color) {
    match kind {
        ChangeKind::Added | ChangeKind::Copied | ChangeKind::IntentToAdd => ("A", t.good),
        ChangeKind::Removed => ("D", t.bad),
        ChangeKind::Renamed => ("R", t.warn),
        ChangeKind::TypeChanged => ("T", t.warn),
        ChangeKind::Conflict => ("!", t.bad),
        ChangeKind::Modified => ("M", t.warn),
    }
}

// ------------------------------------------------------------ widget

const fn pad(top: f32, right: f32, bottom: f32, left: f32) -> Padding {
    Padding {
        top,
        right,
        bottom,
        left,
    }
}

fn tree_row<'a>(
    t: &LumeTheme,
    label: Element<'a, ToolEvent>,
    indent: f32,
    active: bool,
    on_press: Option<Event>,
) -> Element<'a, ToolEvent> {
    let mut b = button(label)
        .width(Length::Fill)
        .padding(pad(3.0, 6.0, 3.0, indent))
        .style(styles::hover_row(
            t.text,
            t.hover,
            active.then_some(t.sel),
            4.0,
        ));
    if let Some(event) = on_press {
        b = b.on_press(ToolEvent::new(event));
    }
    b.into()
}

fn section_row<'a>(
    t: &LumeTheme,
    label: &'static str,
    open: bool,
    section: Section,
) -> Element<'a, ToolEvent> {
    let chevron = if open { "▾" } else { "▸" };
    button(text(format!("{chevron} {label}")).size(12).color(t.tree))
        .width(Length::Fill)
        .padding(pad(5.0, 6.0, 3.0, 6.0))
        .style(styles::hover_row(t.tree, t.hover, None, 4.0))
        .on_press(ToolEvent::new(Event::ToggleSection(section)))
        .into()
}

fn chip_view<'a>(t: &LumeTheme, chip: Chip) -> Element<'a, ToolEvent> {
    let (label, bg, fg, border) = match chip {
        Chip::Current(name) => (name, Some(t.sel), t.accent, Color::TRANSPARENT),
        Chip::Local(name) => (name, Some(t.good_bg), t.good, Color::TRANSPARENT),
        Chip::Remote(name) => (name, None, t.sec, t.border2),
    };
    container(text(label).size(9.5).color(fg))
        .padding(Padding::from([1, 7]))
        .style(move |_| container::Style {
            background: bg.map(Background::Color),
            border: Border {
                radius: 99.0.into(),
                width: 1.0,
                color: border,
            },
            ..container::Style::default()
        })
        .into()
}

fn hsep<'a>(t: &LumeTheme) -> Element<'a, ToolEvent> {
    let color = t.border;
    container(Space::new().width(Length::Fill).height(1))
        .style(move |_| styles::bg(color))
        .into()
}

fn vsep<'a>(t: &LumeTheme) -> Element<'a, ToolEvent> {
    let color = t.border;
    container(Space::new().width(1).height(Length::Fill))
        .style(move |_| styles::bg(color))
        .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use lume_git::{BranchRef, RemoteBranchRef};

    fn commit(id: &str, parents: &[&str], subject: &str) -> CommitSummary {
        CommitSummary {
            id: id.into(),
            parent_ids: parents.iter().map(|p| p.to_string()).collect(),
            subject: subject.into(),
            body: None,
            author_name: "Autrice di prova".into(),
            author_email: "prova@example.test".into(),
            author_time: 1_700_000_000,
            author_offset: 3_600,
            committer_time: 1_700_000_000,
        }
    }

    fn branches() -> Branches {
        Branches {
            head: Some("main".into()),
            local: vec![
                BranchRef {
                    name: "feat/retry".into(),
                    tip: "b".into(),
                },
                BranchRef {
                    name: "main".into(),
                    tip: "a".into(),
                },
            ],
            remote: vec![RemoteBranchRef {
                remote: "origin".into(),
                name: "main".into(),
                tip: "a".into(),
            }],
        }
    }

    #[test]
    fn selezioni_emettono_solo_richieste_reali_al_backend() {
        let mut tool = GitLogTool::new();
        tool.update(&ToolEvent::new(Event::SetRoot(PathBuf::from("C:/repo"))));
        assert!(tool.busy);

        let requests = tool.update(&ToolEvent::new(Event::SelectReference("feat/retry".into())));
        assert!(matches!(&requests[..], [ToolRequest::GitLog(r)] if r == "feat/retry"));

        let snapshot = LogSnapshot {
            reference: "feat/retry".into(),
            branches: branches(),
            commits: vec![commit("b", &["a"], "retry"), commit("a", &[], "radice")],
        };
        tool.update(&ToolEvent::new(Event::Loaded(Ok(snapshot))));
        assert!(!tool.busy);
        assert_eq!(tool.rows.len(), 2);
        assert_eq!(tool.reference(), "feat/retry");

        let requests = tool.update(&ToolEvent::new(Event::SelectCommit("b".into())));
        assert!(matches!(&requests[..], [ToolRequest::GitCommitFiles(id)] if id == "b"));
        tool.update(&ToolEvent::new(Event::FilesLoaded(
            "b".into(),
            Ok(vec![ChangedFile {
                path: PathBuf::from("src/lib.rs"),
                kind: ChangeKind::Modified,
            }]),
        )));
        assert!(tool.files.is_some());
        // riselezione dello stesso commit: file già in cache, nessuna richiesta
        assert!(
            tool.update(&ToolEvent::new(Event::SelectCommit("b".into())))
                .is_empty()
        );
        // file di un commit non più selezionato: ignorati
        tool.update(&ToolEvent::new(Event::SelectCommit("a".into())));
        tool.update(&ToolEvent::new(Event::FilesLoaded("b".into(), Ok(vec![]))));
        assert!(tool.files.is_none());
    }

    #[test]
    fn chip_corrente_locale_remoto() {
        let chips = chips_for(&branches(), "a");
        assert_eq!(
            chips,
            vec![
                Chip::Current("main".into()),
                Chip::Remote("origin/main".into())
            ]
        );
        assert_eq!(
            chips_for(&branches(), "b"),
            vec![Chip::Local("feat/retry".into())]
        );
        assert!(chips_for(&branches(), "zzz").is_empty());
    }

    #[test]
    fn cambio_branch_dal_log_emette_azioni_git_e_si_disabilita_senza_cli() {
        use lume_ui::tool::{GitAction, ToolRequest};

        let mut tool = GitLogTool::new();
        tool.update(&ToolEvent::new(Event::Loaded(Ok(LogSnapshot {
            reference: "HEAD".into(),
            branches: branches(),
            commits: vec![commit("a", &[], "radice")],
        }))));

        tool.git_available = false;
        assert!(
            tool.update(&ToolEvent::new(Event::SwitchBranch("feat/retry".into())))
                .is_empty()
        );
        assert!(
            tool.update(&ToolEvent::new(Event::SwitchRemoteBranch {
                remote: "origin".into(),
                name: "main".into(),
            }))
            .is_empty()
        );

        tool.git_available = true;
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::SwitchBranch("feat/retry".into())))
                .as_slice(),
            [ToolRequest::GitAction(GitAction::SwitchBranch(name))] if name == "feat/retry"
        ));
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::SwitchRemoteBranch {
                remote: "origin".into(),
                name: "main".into(),
            }))
            .as_slice(),
            [ToolRequest::GitAction(GitAction::SwitchRemoteBranch { remote, name })]
                if remote == "origin" && name == "main"
        ));
    }

    #[test]
    fn chiusura_log_rilascia_grafo_e_dettagli_e_riapre_lo_stesso_riferimento() {
        let mut tool = GitLogTool::new();
        tool.root = Some(PathBuf::from("repo"));
        tool.update(&ToolEvent::new(Event::Loaded(Ok(LogSnapshot {
            reference: "feat/retry".into(),
            branches: branches(),
            commits: vec![commit("a", &[], "radice")],
        }))));
        tool.update(&ToolEvent::new(Event::SelectCommit("a".into())));
        tool.files = Some(("a".into(), Ok(vec![])));

        assert!(tool.on_visibility_changed(false).is_empty());
        assert!(tool.snapshot.is_none() && tool.rows.is_empty());
        assert!(tool.selected.is_none() && tool.files.is_none());

        let requests = tool.on_visibility_changed(true);
        assert!(
            matches!(requests.as_slice(), [ToolRequest::GitLog(reference)] if reference == "feat/retry")
        );
        assert!(tool.busy && tool.git_available);
    }

    #[test]
    fn apertura_log_non_duplica_caricamento_in_volo_o_senza_root() {
        let mut loading = GitLogTool::new();
        loading.update(&ToolEvent::new(Event::SetRoot(PathBuf::from("repo"))));
        assert!(loading.on_visibility_changed(true).is_empty());
        assert!(GitLogTool::new().on_visibility_changed(true).is_empty());
    }

    #[test]
    fn filtro_testo_o_hash_e_date() {
        let c = commit("abc1234def", &[], "Fix del Parser");
        assert!(matches_text(&c, ""));
        assert!(matches_text(&c, "parser"));
        assert!(matches_text(&c, "abc12"));
        assert!(!matches_text(&c, "zzz"));

        // 1700000000 = 2023-11-14 22:13:20 UTC → +01:00 = 23:13
        assert_eq!(format_commit_date(1_700_000_000, 3_600), "14/11, 23:13");
        assert_eq!(format_commit_date(0, 0), "01/01, 00:00");
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));

        let now = 1_700_000_000;
        assert_eq!(relative_age(now - 10, now), "adesso");
        assert_eq!(relative_age(now - 300, now), "5 min fa");
        assert_eq!(relative_age(now - 7_200, now), "2 h fa");
        assert_eq!(relative_age(now - 100_000, now), "ieri");
        assert_eq!(relative_age(now - 5 * 86_400, now), "5 g fa");
        assert_eq!(relative_age(now - 90 * 86_400, now), "16/08");
    }
}
