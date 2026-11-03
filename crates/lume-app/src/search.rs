//! Overlay «Cerca ovunque»: doppio Maiusc (<400 ms) o clic sulla pill.
//! Tab Tutto/File/Simboli/Azioni, filtro live, max 9 risultati,
//! navigazione da tastiera (↑↓ / ↵ / Tab / Esc).
//!
//! Da non confondere: cerca **per nome** file, simboli e azioni. La ricerca
//! nel testo dei file è il motore di `lume-core/src/search.rs`, mostrato dal
//! pannello «Cerca» (`tools/search_tool.rs`).

use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use iced::widget::{Space, button, column, container, mouse_area, row, text, text_input};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use ignore::WalkBuilder;
use lume_icons::FileIconContext;

use lume_ui::MONO;
use lume_ui::file_icons::{FileIcon, FileIconSource};

use crate::menu::MenuAct;
use crate::tools::structure;

pub static INPUT_ID: &str = "cerca-ovunque";
pub(crate) const MAX_FILE_SEARCH_RESULTS: usize = 64;
const FILE_SEARCH_DEBOUNCE: Duration = Duration::from_millis(75);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSearchError {
    Superseded,
    WorkerUnavailable(String),
}

impl std::fmt::Display for FileSearchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Superseded => formatter.write_str("ricerca sostituita"),
            Self::WorkerUnavailable(error) => {
                write!(formatter, "worker ricerca file non disponibile: {error}")
            }
        }
    }
}

struct FileSearchRequest {
    root: PathBuf,
    query: String,
    generation: Arc<AtomicU64>,
    my_generation: u64,
    submitted_at: Instant,
    sender: oneshot::Sender<Result<Vec<Entry>, FileSearchError>>,
}

#[derive(Default)]
struct FileSearchMailbox {
    pending: Option<FileSearchRequest>,
}

struct FileSearchShared {
    mailbox: Mutex<FileSearchMailbox>,
    wake: Condvar,
    shutdown: AtomicBool,
}

pub struct GlobalFileSearchWorker {
    shared: Arc<FileSearchShared>,
    thread: Option<JoinHandle<()>>,
}

impl GlobalFileSearchWorker {
    pub fn start() -> std::io::Result<Self> {
        let shared = Arc::new(FileSearchShared {
            mailbox: Mutex::new(FileSearchMailbox::default()),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        let thread = thread::Builder::new()
            .name("lume-file-palette".into())
            .spawn(move || file_search_worker_loop(&thread_shared))?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    pub fn search(
        &self,
        root: PathBuf,
        query: String,
        generation: Arc<AtomicU64>,
        my_generation: u64,
    ) -> Result<oneshot::Receiver<Result<Vec<Entry>, FileSearchError>>, FileSearchError> {
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Err(FileSearchError::WorkerUnavailable(
                "worker arrestato".into(),
            ));
        }
        let (sender, receiver) = oneshot::channel();
        let request = FileSearchRequest {
            root,
            query,
            generation,
            my_generation,
            submitted_at: Instant::now(),
            sender,
        };
        let mut mailbox = self
            .shared
            .mailbox
            .lock()
            .map_err(|_| FileSearchError::WorkerUnavailable("mailbox danneggiata".into()))?;
        if let Some(previous) = mailbox.pending.replace(request) {
            let _ = previous.sender.send(Err(FileSearchError::Superseded));
        }
        drop(mailbox);
        self.shared.wake.notify_one();
        Ok(receiver)
    }

    #[cfg(test)]
    #[allow(clippy::expect_used)] // helper solo per i test
    pub fn thread_id(&self) -> std::thread::ThreadId {
        self.thread
            .as_ref()
            .expect("worker thread must exist")
            .thread()
            .id()
    }
}

impl Drop for GlobalFileSearchWorker {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        if let Ok(mut mailbox) = self.shared.mailbox.lock()
            && let Some(request) = mailbox.pending.take()
        {
            let _ = request.sender.send(Err(FileSearchError::WorkerUnavailable(
                "worker arrestato".into(),
            )));
        }
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn file_search_worker_loop(shared: &FileSearchShared) {
    while !shared.shutdown.load(Ordering::Acquire) {
        let Some(mut request) = wait_for_file_search_request(shared) else {
            return;
        };
        loop {
            let elapsed = request.submitted_at.elapsed();
            if elapsed >= FILE_SEARCH_DEBOUNCE {
                break;
            }
            let Ok(mailbox) = shared.mailbox.lock() else {
                let _ = request.sender.send(Err(FileSearchError::WorkerUnavailable(
                    "mailbox danneggiata".into(),
                )));
                return;
            };
            let remaining = FILE_SEARCH_DEBOUNCE - elapsed;
            let Ok((mut mailbox, _)) = shared.wake.wait_timeout(mailbox, remaining) else {
                let _ = request.sender.send(Err(FileSearchError::WorkerUnavailable(
                    "attesa worker fallita".into(),
                )));
                return;
            };
            if shared.shutdown.load(Ordering::Acquire) {
                let _ = request.sender.send(Err(FileSearchError::WorkerUnavailable(
                    "worker arrestato".into(),
                )));
                return;
            }
            if let Some(latest) = mailbox.pending.take() {
                let _ = request.sender.send(Err(FileSearchError::Superseded));
                request = latest;
            }
        }

        let entries = file_entries(
            &request.root,
            &request.query,
            &request.generation,
            request.my_generation,
            Some(&shared.shutdown),
        );
        let result = if shared.shutdown.load(Ordering::Acquire) {
            Err(FileSearchError::WorkerUnavailable(
                "worker arrestato".into(),
            ))
        } else if request.generation.load(Ordering::Acquire) != request.my_generation {
            Err(FileSearchError::Superseded)
        } else {
            Ok(entries)
        };
        let _ = request.sender.send(result);
    }
}

fn wait_for_file_search_request(shared: &FileSearchShared) -> Option<FileSearchRequest> {
    let mut mailbox = shared.mailbox.lock().ok()?;
    while mailbox.pending.is_none() && !shared.shutdown.load(Ordering::Acquire) {
        mailbox = shared.wake.wait(mailbox).ok()?;
    }
    mailbox.pending.take()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalSearchMessage {
    Open(SearchTab),
    QueryChanged(String),
    TabClicked(SearchTab),
    Activate(usize),
    Navigate(i32),
    Accept,
    CycleTab,
    Escape,
    ShiftPressed,
    AnyKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalSearchIntent {
    None,
    Opened,
    Closed,
    Activate(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchTab {
    Tutto,
    File,
    Simboli,
    Azioni,
}

impl SearchTab {
    pub const ALL: [SearchTab; 4] = [Self::Tutto, Self::File, Self::Simboli, Self::Azioni];

    pub fn label(self) -> &'static str {
        match self {
            Self::Tutto => "Tutto",
            Self::File => "File",
            Self::Simboli => "Simboli",
            Self::Azioni => "Azioni",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Tutto => Self::File,
            Self::File => Self::Simboli,
            Self::Simboli => Self::Azioni,
            Self::Azioni => Self::Tutto,
        }
    }
}

struct SearchState {
    pub open: bool,
    pub query: String,
    pub tab: SearchTab,
    pub selected: usize,
    pub last_shift: Option<Instant>,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            tab: SearchTab::Tutto,
            selected: 0,
            last_shift: None,
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
    }
}

pub struct GlobalSearchOverlay {
    state: SearchState,
    file_results: Vec<Entry>,
    file_search_generation: Option<u64>,
    file_searching: bool,
    file_search_error: Option<String>,
    /// Simboli del file attivo, fotografati da `structure_snapshot`: niente
    /// worker, niente generazione — è già in memoria.
    symbol_results: Vec<Entry>,
    /// Perché non c'è niente in Simboli — vuoto = c'è, come
    /// `structure::Snapshot::empty_reason`. Senza questo, «Language server
    /// non avviato» e «questo file non ha simboli» avrebbero la stessa
    /// faccia: «Nessun risultato» non distingue le due cose.
    symbol_empty_reason: &'static str,
}

impl Default for GlobalSearchOverlay {
    fn default() -> Self {
        Self {
            state: SearchState::new(),
            file_results: Vec::new(),
            file_search_generation: None,
            file_searching: false,
            file_search_error: None,
            symbol_results: Vec::new(),
            symbol_empty_reason: "",
        }
    }
}

impl GlobalSearchOverlay {
    #[cfg(test)]
    pub fn open(tab: SearchTab) -> Self {
        let mut overlay = Self::default();
        overlay.state.open = true;
        overlay.state.tab = tab;
        overlay
    }

    pub fn update(&mut self, message: GlobalSearchMessage) -> GlobalSearchIntent {
        match message {
            GlobalSearchMessage::Open(tab) => {
                self.state.open = true;
                self.state.tab = tab;
                self.state.selected = 0;
                self.cancel_file_search();
                GlobalSearchIntent::Opened
            }
            GlobalSearchMessage::QueryChanged(query) => {
                self.state.query = query;
                self.state.selected = 0;
                self.cancel_file_search();
                GlobalSearchIntent::None
            }
            GlobalSearchMessage::TabClicked(tab) => {
                self.state.tab = tab;
                self.state.selected = 0;
                self.cancel_file_search();
                GlobalSearchIntent::None
            }
            GlobalSearchMessage::Activate(index) => GlobalSearchIntent::Activate(index),
            GlobalSearchMessage::Navigate(delta) => {
                let result_count = self.entries().len();
                if self.state.open && result_count > 0 {
                    self.state.selected = (self.state.selected as i32 + delta)
                        .rem_euclid(result_count as i32)
                        as usize;
                }
                GlobalSearchIntent::None
            }
            GlobalSearchMessage::Accept => {
                if self.state.open {
                    GlobalSearchIntent::Activate(self.state.selected)
                } else {
                    GlobalSearchIntent::None
                }
            }
            GlobalSearchMessage::CycleTab => {
                if self.state.open {
                    self.state.tab = self.state.tab.next();
                    self.state.selected = 0;
                    self.cancel_file_search();
                }
                GlobalSearchIntent::None
            }
            GlobalSearchMessage::Escape => {
                self.state.close();
                GlobalSearchIntent::Closed
            }
            GlobalSearchMessage::ShiftPressed => {
                let now = Instant::now();
                match self.state.last_shift {
                    Some(previous) if now.duration_since(previous).as_millis() < 400 => {
                        self.state.last_shift = None;
                        if !self.state.open {
                            self.state.open = true;
                            self.state.tab = SearchTab::Tutto;
                            self.state.selected = 0;
                            return GlobalSearchIntent::Opened;
                        }
                    }
                    _ => self.state.last_shift = Some(now),
                }
                GlobalSearchIntent::None
            }
            GlobalSearchMessage::AnyKey => {
                self.state.last_shift = None;
                GlobalSearchIntent::None
            }
        }
    }

    /// True nei 400 ms dopo un tasto Shift: solo allora l'app ascolta
    /// «qualsiasi tasto» per annullare il doppio Shift (niente update per
    /// ogni battuta fuori da quella finestra).
    pub fn shift_window_open(&self) -> bool {
        self.state
            .last_shift
            .is_some_and(|previous| previous.elapsed().as_millis() < 400)
    }

    pub fn close(&mut self) {
        self.state.close();
        self.cancel_file_search();
    }

    pub fn is_open(&self) -> bool {
        self.state.open
    }

    pub fn begin_file_search(&mut self, generation: u64) {
        self.file_results.clear();
        self.file_search_generation = Some(generation);
        self.file_searching = true;
        self.file_search_error = None;
        self.state.selected = 0;
    }

    pub fn finish_file_search(&mut self, generation: u64, result: Result<Vec<Entry>, String>) {
        if self.file_search_generation == Some(generation) {
            match result {
                Ok(entries) => {
                    self.file_results = entries;
                    self.file_search_error = None;
                }
                Err(error) => {
                    self.file_results.clear();
                    self.file_search_error = Some(error);
                }
            }
            self.file_searching = false;
            self.state.selected = 0;
        }
    }

    /// La vista sostituita conserva la fotografia, ma non attende più il worker.
    pub fn suspend_file_search(&mut self) {
        self.file_search_generation = None;
        self.file_searching = false;
    }

    pub fn cancel_file_search(&mut self) {
        self.file_results.clear();
        self.suspend_file_search();
        self.file_search_error = None;
        self.state.selected = 0;
        // Non è una ricerca file, ma questo è il collo di bottiglia che
        // `close()` e ogni cambio di query/scheda attraversano già: è qui che
        // i Simboli di una sessione chiusa smettono di essere mostrati.
        self.symbol_results.clear();
        self.symbol_empty_reason = "";
    }

    /// Rimpiazza i Simboli con la fotografia del file attivo. Chi chiama
    /// decide quando: aprire la ricerca, cambiare scheda o digitare — mai da
    /// sé, perché non ha modo di sapere se il file attivo sia cambiato.
    pub fn set_symbol_results(&mut self, snapshot: &structure::Snapshot) {
        self.symbol_results = symbol_entries(snapshot);
        // Vuoto solo perché senza percorso non c'è entry possibile (vedi
        // `symbol_entries`): qui il motivo non conta, non c'è niente da dire
        // che l'utente possa fare qualcosa con.
        self.symbol_empty_reason = if snapshot.path.is_some() {
            snapshot.empty_reason
        } else {
            ""
        };
    }

    pub fn file_query(&self) -> Option<String> {
        let query = self.state.query.trim();
        let vuole_file = !matches!(self.state.tab, SearchTab::Azioni | SearchTab::Simboli);
        (self.state.open && vuole_file && !query.is_empty()).then(|| query.to_owned())
    }

    #[cfg(test)]
    pub fn is_file_searching(&self) -> bool {
        self.file_searching
    }

    #[cfg(test)]
    pub fn file_search_error(&self) -> Option<&str> {
        self.file_search_error.as_deref()
    }

    pub fn entries(&self) -> Vec<Entry> {
        results(&self.state, &self.file_results, &self.symbol_results)
    }

    pub fn view<'a>(
        &'a self,
        theme: &'a lume_theme::LumeTheme,
        file_icons: &'a dyn FileIconSource,
    ) -> Element<'a, GlobalSearchMessage> {
        overlay(
            &self.state,
            BackgroundResults {
                file_results: &self.file_results,
                symbol_results: &self.symbol_results,
                symbol_empty_reason: self.symbol_empty_reason,
                file_searching: self.file_searching,
                file_search_error: self.file_search_error.as_deref(),
            },
            theme,
            file_icons,
        )
    }
}

/// Colore simbolico dell'icona risultato, risolto sul tema.
#[derive(Debug, Clone, Copy)]
enum Tone {
    Warn,
    Accent,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub tab: SearchTab,
    pub name: String,
    pub detail: String,
    search_path: Option<String>,
    icon: String,
    tone: Tone,
    pub shortcut: String,
    pub act: Option<MenuAct>,
    pub path: Option<PathBuf>,
    /// Riga del file da raggiungere, **1-based** come `Row::line` di
    /// Struttura. Solo le entry Simboli la usano; le altre restano `None` e
    /// aprono il file senza spostare il cursore.
    pub line: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FileMatchRank {
    tier: u8,
    position: usize,
    path_len: usize,
}

#[derive(Debug)]
struct RankedFileMatch {
    rank: FileMatchRank,
    entry: Entry,
}

impl PartialEq for RankedFileMatch {
    fn eq(&self, other: &Self) -> bool {
        self.rank == other.rank && self.entry.path == other.entry.path
    }
}

impl Eq for RankedFileMatch {}

impl PartialOrd for RankedFileMatch {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedFileMatch {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank
            .cmp(&other.rank)
            .then_with(|| self.entry.path.cmp(&other.entry.path))
    }
}

struct RankedFileMatches {
    query: String,
    limit: usize,
    heap: BinaryHeap<RankedFileMatch>,
}

impl RankedFileMatches {
    fn new(query: &str, limit: usize) -> Self {
        Self {
            query: normalize_search_query(query),
            limit,
            heap: BinaryHeap::with_capacity(limit),
        }
    }

    fn consider(&mut self, entry: Entry) {
        let Some(rank) = rank_file_entry(&entry, &self.query) else {
            return;
        };
        let candidate = RankedFileMatch { rank, entry };
        if self.heap.len() < self.limit {
            self.heap.push(candidate);
        } else if self.heap.peek().is_some_and(|worst| candidate < *worst) {
            self.heap.pop();
            self.heap.push(candidate);
        }
    }

    fn into_sorted(self) -> Vec<Entry> {
        let mut ranked = self.heap.into_vec();
        ranked.sort();
        ranked.into_iter().map(|item| item.entry).collect()
    }
}

fn rank_file_entry(entry: &Entry, lowercase_query: &str) -> Option<FileMatchRank> {
    let name_match = find_case_insensitive(&entry.name, lowercase_query);
    let path_match = entry
        .search_path
        .as_deref()
        .and_then(|path| find_path_case_insensitive(path, lowercase_query));
    let (tier, position) = if entry.name.len() == lowercase_query.len() && name_match == Some(0) {
        (0, 0)
    } else if name_match == Some(0) {
        (1, 0)
    } else if let Some(position) = name_match {
        (2, position)
    } else if let Some(position) = path_match {
        (3, position)
    } else {
        return None;
    };
    Some(FileMatchRank {
        tier,
        position,
        path_len: entry
            .search_path
            .as_deref()
            .map_or(entry.name.len() + entry.detail.len(), str::len),
    })
}

pub(crate) fn resolve_entry_icon(source: &dyn FileIconSource, entry: &Entry) -> Option<FileIcon> {
    let path = entry.path.as_deref()?;
    let context = FileIconContext::file(
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| entry.name.clone()),
    );
    source.icon(&context).ok()
}

/// File e Simboli hanno entrambi un `path` (serve ad aprirli), ma solo i
/// primi vogliono l'icona del tipo di file: un simbolo mostra il genere
/// (`fn`, `struct`…), non l'estensione del file in cui vive.
fn shows_file_type_icon(entry: &Entry) -> bool {
    entry.tab != SearchTab::Simboli && entry.path.is_some()
}

fn entry(
    tab: SearchTab,
    name: impl Into<String>,
    detail: impl Into<String>,
    icon: impl Into<String>,
    tone: Tone,
    shortcut: impl Into<String>,
    act: Option<MenuAct>,
) -> Entry {
    Entry {
        tab,
        name: name.into(),
        detail: detail.into(),
        search_path: None,
        icon: icon.into(),
        tone,
        shortcut: shortcut.into(),
        act,
        path: None,
        line: None,
    }
}

/// Le entry Simboli: stessa fotografia del pannello Struttura, un'altra
/// vetrina. Senza percorso (es. la console SQL) non c'è dove atterrare, e
/// niente si mostra — la stessa regola di `structure.rs`.
fn symbol_entries(snapshot: &structure::Snapshot) -> Vec<Entry> {
    let Some(path) = snapshot.path.clone() else {
        return Vec::new();
    };
    snapshot
        .rows
        .iter()
        .map(|row| Entry {
            tab: SearchTab::Simboli,
            name: row.name.clone(),
            detail: row.detail.clone().unwrap_or_default(),
            search_path: None,
            icon: symbol_glyph(row.kind).to_owned(),
            tone: Tone::Accent,
            shortcut: String::new(),
            act: None,
            path: Some(path.clone()),
            line: Some(row.line),
        })
        .collect()
}

/// Un solo carattere per la cella icona (22px): `row.kind` di Struttura è
/// un'etichetta di parola («struct», «field»…), pensata per una riga larga,
/// non per questa cella — ci straboccherebbe. `ƒ` per le funzioni segue la
/// stessa convenzione del prototipo; il resto è l'iniziale del genere.
fn symbol_glyph(kind: &str) -> &'static str {
    match kind {
        "fn" => "ƒ",
        "struct" => "S",
        "class" => "C",
        "trait" => "T",
        "enum" => "E",
        "mod" => "M",
        "ns" => "N",
        "prop" => "P",
        "field" => "F",
        "new" => "⊕",
        "let" => "V",
        "const" => "K",
        "case" => "c",
        "type" => "τ",
        "op" => "±",
        "file" => "◆",
        _ => "•",
    }
}

fn action_entries() -> Vec<Entry> {
    vec![
        entry(
            SearchTab::Azioni,
            "Nuovo file",
            "File",
            "⌘",
            Tone::Accent,
            "Ctrl+N",
            Some(MenuAct::NewFile),
        ),
        entry(
            SearchTab::Azioni,
            "Apri cartella…",
            "File",
            "⌘",
            Tone::Accent,
            "Ctrl+O",
            Some(MenuAct::OpenFolder),
        ),
        entry(
            SearchTab::Azioni,
            "Salva",
            "File",
            "⌘",
            Tone::Accent,
            "Ctrl+S",
            Some(MenuAct::Save),
        ),
        entry(
            SearchTab::Azioni,
            "Salva con nome…",
            "File",
            "⌘",
            Tone::Accent,
            "",
            Some(MenuAct::SaveAs),
        ),
        entry(
            SearchTab::Azioni,
            "Salva tutto",
            "File",
            "⌘",
            Tone::Accent,
            "Ctrl+Maiusc+S",
            Some(MenuAct::SaveAll),
        ),
        entry(
            SearchTab::Azioni,
            "Esci",
            "File",
            "⌘",
            Tone::Accent,
            "Alt+F4",
            Some(MenuAct::Exit),
        ),
        entry(
            SearchTab::Azioni,
            "Terminale",
            "Visualizza",
            "⌘",
            Tone::Accent,
            "Alt+F12",
            Some(MenuAct::ToggleTerminal),
        ),
        entry(
            SearchTab::Azioni,
            "Log Git",
            "Visualizza",
            "⌘",
            Tone::Accent,
            "",
            Some(MenuAct::ShowGitLog),
        ),
        entry(
            SearchTab::Azioni,
            "Problemi",
            "Visualizza",
            "⌘",
            Tone::Accent,
            "",
            Some(MenuAct::ShowProblems),
        ),
        entry(
            SearchTab::Azioni,
            "Gestione estensioni…",
            "Visualizza",
            "⌘",
            Tone::Accent,
            "Ctrl+Maiusc+X",
            Some(MenuAct::ShowExtensions),
        ),
        entry(
            SearchTab::Azioni,
            "Barra dei menù classica",
            "Visualizza",
            "⌘",
            Tone::Accent,
            "",
            Some(MenuAct::MenuBar),
        ),
        entry(
            SearchTab::Azioni,
            "Segnalibro sulla riga corrente",
            "Naviga",
            "⌘",
            Tone::Accent,
            "F11",
            Some(MenuAct::ToggleBookmark),
        ),
    ]
}

/// File reali del progetto, filtrati da `.gitignore` e senza `.git`.
/// La raccolta è cancellabile e limitata: non materializza mai l'intero
/// progetto in memoria.
fn file_entries(
    root: &Path,
    query: &str,
    generation: &AtomicU64,
    my_generation: u64,
    shutdown: Option<&AtomicBool>,
) -> Vec<Entry> {
    let query = normalize_search_query(query.trim());
    if query.is_empty()
        || generation.load(Ordering::Relaxed) != my_generation
        || shutdown.is_some_and(|flag| flag.load(Ordering::Acquire))
    {
        return Vec::new();
    }

    let walker = WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git" && entry.file_name() != ".jj")
        .build()
        .flatten();
    let mut ranked = RankedFileMatches::new(&query, MAX_FILE_SEARCH_RESULTS);
    for entry in walker {
        if generation.load(Ordering::Relaxed) != my_generation
            || shutdown.is_some_and(|flag| flag.load(Ordering::Acquire))
        {
            return Vec::new();
        }
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.into_path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| path.to_string_lossy());
        let relative_text = relative.to_string_lossy();
        if !contains_case_insensitive(&name, &query)
            && find_path_case_insensitive(&relative_text, &query).is_none()
        {
            continue;
        }
        let search_path = relative_text.into_owned();
        ranked.consider(Entry {
            tab: SearchTab::File,
            name: name.into_owned(),
            detail: relative
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|parent| parent.display().to_string())
                .unwrap_or_else(|| ".".into()),
            search_path: Some(search_path),
            icon: String::new(),
            tone: Tone::Warn,
            shortcut: String::new(),
            act: None,
            path: Some(path),
            line: None,
        });
    }
    ranked.into_sorted()
}

fn contains_case_insensitive(haystack: &str, lowercase_needle: &str) -> bool {
    find_case_insensitive(haystack, lowercase_needle).is_some()
}

fn normalize_search_query(query: &str) -> String {
    query.replace('\\', "/").to_lowercase()
}

fn find_case_insensitive(haystack: &str, lowercase_needle: &str) -> Option<usize> {
    if haystack.is_ascii() && lowercase_needle.is_ascii() {
        let needle = lowercase_needle.as_bytes();
        haystack
            .as_bytes()
            .windows(needle.len())
            .position(|window| window.eq_ignore_ascii_case(needle))
    } else {
        haystack.to_lowercase().find(lowercase_needle)
    }
}

fn find_path_case_insensitive(haystack: &str, lowercase_needle: &str) -> Option<usize> {
    if lowercase_needle.is_empty() {
        return Some(0);
    }
    if haystack.is_ascii() && lowercase_needle.is_ascii() {
        let needle = lowercase_needle.as_bytes();
        haystack
            .as_bytes()
            .windows(needle.len())
            .position(|window| {
                window.iter().zip(needle).all(|(&left, &right)| {
                    if matches!(left, b'/' | b'\\') && matches!(right, b'/' | b'\\') {
                        true
                    } else {
                        left.eq_ignore_ascii_case(&right)
                    }
                })
            })
    } else {
        haystack
            .replace('\\', "/")
            .to_lowercase()
            .find(lowercase_needle)
    }
}

/// Risultati reali filtrati (max 9).
fn results(state: &SearchState, file_results: &[Entry], symbol_results: &[Entry]) -> Vec<Entry> {
    let q = normalize_search_query(state.query.trim());
    let mut entries = action_entries();
    entries.extend(file_results.iter().cloned());
    entries.extend(symbol_results.iter().cloned());
    entries
        .into_iter()
        .filter(|entry| state.tab == SearchTab::Tutto || entry.tab == state.tab)
        .filter(|entry| {
            q.is_empty()
                || find_case_insensitive(&entry.name, &q).is_some()
                || find_case_insensitive(&entry.detail, &q).is_some()
                || entry
                    .search_path
                    .as_deref()
                    .is_some_and(|path| find_path_case_insensitive(path, &q).is_some())
        })
        .take(9)
        .collect()
}

fn search_feedback<'a>(
    query: &str,
    tab: SearchTab,
    symbol_empty_reason: &'a str,
    file_searching: bool,
    file_search_error: Option<&'a str>,
    entries_empty: bool,
) -> Option<&'a str> {
    if let Some(error) = file_search_error {
        Some(error)
    } else if file_searching {
        Some("Ricerca file…")
    } else if entries_empty {
        if query.trim().is_empty() && tab == SearchTab::File {
            Some("Digita per cercare un file")
        } else if tab == SearchTab::Simboli && !symbol_empty_reason.is_empty() {
            // «Nessun risultato» non distinguerebbe «server non avviato» da
            // «questo file non ha simboli»: qui il motivo vero c'è già.
            Some(symbol_empty_reason)
        } else {
            Some("Nessun risultato")
        }
    } else {
        None
    }
}

/// I risultati che non vivono nello stato dell'input: file (worker in
/// sfondo) e Simboli (fotografia sincrona). Raggruppati per non far
/// sforare `overlay` oltre i 7 argomenti che clippy tollera.
struct BackgroundResults<'a> {
    file_results: &'a [Entry],
    symbol_results: &'a [Entry],
    symbol_empty_reason: &'a str,
    file_searching: bool,
    file_search_error: Option<&'a str>,
}

fn overlay<'a>(
    s: &'a SearchState,
    bg: BackgroundResults<'a>,
    t: &'a lume_theme::LumeTheme,
    file_icons: &'a dyn FileIconSource,
) -> Element<'a, GlobalSearchMessage> {
    let BackgroundResults {
        file_results,
        symbol_results,
        symbol_empty_reason,
        file_searching,
        file_search_error,
    } = bg;
    // ---- tab
    let mut tabs = row![].spacing(4);
    for tab in SearchTab::ALL {
        let active = s.tab == tab;
        let color = if active { t.text } else { t.sec };
        let underline = container(Space::new())
            .width(Length::Fill)
            .height(2)
            .style({
                let c = if active { t.accent } else { Color::TRANSPARENT };
                move |_| lume_ui::styles::bg(c)
            });
        tabs = tabs.push(
            mouse_area(column![
                container(text(tab.label()).size(12).color(color)).padding(Padding {
                    top: 5.0,
                    right: 10.0,
                    bottom: 6.0,
                    left: 10.0
                }),
                underline,
            ])
            .on_press(GlobalSearchMessage::TabClicked(tab))
            .interaction(iced::mouse::Interaction::Pointer),
        );
    }
    let tabs = row![
        tabs,
        Space::new().width(Length::Fill),
        container(text("doppio Maiusc").size(11).color(t.muted)).padding(Padding {
            top: 6.0,
            ..Padding::ZERO
        }),
    ]
    .padding(Padding {
        top: 10.0,
        right: 14.0,
        bottom: 0.0,
        left: 14.0,
    });

    // ---- input con autofocus
    let input = row![
        lume_ui::Icon::Search.view(14.0, t.sec),
        text_input("Digita per cercare file, simboli e azioni…", &s.query)
            .id(INPUT_ID)
            .on_input(GlobalSearchMessage::QueryChanged)
            .size(13.5)
            .padding(0)
            .style({
                let t = t.clone();
                move |_, _| text_input::Style {
                    background: Background::Color(Color::TRANSPARENT),
                    border: Border::default(),
                    icon: t.icon,
                    placeholder: t.muted,
                    value: t.text,
                    selection: t.sel,
                }
            }),
    ]
    .spacing(10)
    .padding(Padding {
        top: 11.0,
        right: 14.0,
        bottom: 11.0,
        left: 14.0,
    })
    .align_y(Alignment::Center);

    // ---- risultati (max 9)
    let entries = results(s, file_results, symbol_results);
    let mut list = column![];
    if let Some(label) = search_feedback(
        &s.query,
        s.tab,
        symbol_empty_reason,
        file_searching,
        file_search_error,
        entries.is_empty(),
    ) {
        let label_color = if file_search_error.is_some() {
            t.bad
        } else {
            t.muted
        };
        list = list.push(
            container(text(label).size(12.5).color(label_color)).padding(Padding {
                top: 18.0,
                right: 14.0,
                bottom: 18.0,
                left: 14.0,
            }),
        );
    }
    for (pos, it) in entries.iter().enumerate() {
        let icon_color = match it.tone {
            Tone::Warn => t.warn,
            Tone::Accent => t.accent,
        };
        let selected = pos == s.selected;
        let icon: Element<'_, GlobalSearchMessage> = if shows_file_type_icon(it) {
            match resolve_entry_icon(file_icons, it) {
                Some(handle) => handle.view(15.0, t.sec),
                None => text("").into(),
            }
        } else {
            text(it.icon.clone())
                .size(11)
                .font(iced::Font {
                    weight: iced::font::Weight::Semibold,
                    ..MONO
                })
                .color(icon_color)
                .into()
        };
        let r = button(
            row![
                container(icon)
                    .width(22)
                    .height(22)
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center)
                    .style({
                        let inset = t.inset;
                        move |_| container::Style {
                            background: Some(Background::Color(inset)),
                            border: Border {
                                radius: 5.0.into(),
                                ..Border::default()
                            },
                            ..container::Style::default()
                        }
                    }),
                text(it.name.clone()).size(12.5).color(t.text),
                text(it.detail.clone()).size(11.5).color(t.muted),
                Space::new().width(Length::Fill),
                text(it.shortcut.clone()).size(11).color(t.muted),
            ]
            .spacing(11)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding(Padding {
            top: 7.0,
            right: 10.0,
            bottom: 7.0,
            left: 10.0,
        })
        .style(lume_ui::styles::hover_row(
            t.text,
            t.hover2,
            selected.then_some(t.hover2),
            6.0,
        ))
        .on_press(GlobalSearchMessage::Activate(pos));
        list = list.push(r);
    }
    let list = container(list).width(Length::Fill).padding(5);

    // ---- footer hint
    let hint = |s2: &'static str| text(s2).size(11).color(t.muted);
    let footer = row![
        hint("↑↓ naviga"),
        hint("↵ apri"),
        hint("Tab cambia scheda"),
        hint("esc chiudi"),
    ]
    .spacing(14)
    .padding(Padding {
        top: 9.0,
        right: 14.0,
        bottom: 9.0,
        left: 14.0,
    });

    let sep = || {
        container(Space::new())
            .width(Length::Fill)
            .height(1)
            .style({
                let border = t.border;
                move |_| lume_ui::styles::bg(border)
            })
    };

    let panel = container(column![tabs, sep(), input, sep(), list, sep(), footer])
        .width(640)
        .style(|_| lume_ui::styles::overlay_panel(t));

    // ---- backdrop + pannello a 100px dall'alto
    let backdrop = mouse_area(
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| lume_ui::styles::bg(Color::from_rgba8(8, 10, 13, 0.55))),
    )
    .on_press(GlobalSearchMessage::Escape);

    iced::widget::stack![
        backdrop,
        container(panel)
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .padding(Padding {
                top: 100.0,
                ..Padding::ZERO
            }),
    ]
    .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{
        Entry, FileSearchError, GlobalFileSearchWorker, GlobalSearchIntent, GlobalSearchMessage,
        GlobalSearchOverlay, MAX_FILE_SEARCH_RESULTS, MenuAct, RankedFileMatches, SearchTab, Tone,
        action_entries, file_entries, resolve_entry_icon, search_feedback, shows_file_type_icon,
        structure, symbol_glyph,
    };
    use crate::editor_tab_bar::{EditorTab, resolve_tab_icon};
    use crate::tools::project::resolve_row_icon;
    use iced::widget::svg;
    use lume_core::DocId;
    use lume_icons::{FileIconContext, IconThemeError};
    use lume_ui::file_icons::{FileIcon, FileIconSource};
    use std::cell::Cell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::time::{SystemTime, UNIX_EPOCH};

    const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0h1v1H0z"/></svg>"#;

    struct CountingSource(Cell<usize>);

    impl FileIconSource for CountingSource {
        fn icon(&self, _context: &FileIconContext) -> Result<FileIcon, IconThemeError> {
            self.0.set(self.0.get() + 1);
            Ok(FileIcon {
                handle: svg::Handle::from_memory(SVG),
                tintable: false,
                tint_role: None,
            })
        }
    }

    /// Prima di questo gap l'unico modo di aprire Estensioni era la
    /// scorciatoia: mancava sia dal menù sia da qui.
    #[test]
    fn azioni_include_nuovo_file_ed_esci() {
        let azioni = action_entries();
        for (nome, atteso, shortcut) in [
            ("Nuovo file", MenuAct::NewFile, "Ctrl+N"),
            ("Esci", MenuAct::Exit, "Alt+F4"),
        ] {
            let voce = azioni
                .iter()
                .find(|e| e.name == nome)
                .unwrap_or_else(|| panic!("l'azione «{nome}» deve esistere"));
            assert_eq!(voce.tab, SearchTab::Azioni);
            assert_eq!(voce.act, Some(atteso));
            assert_eq!(voce.shortcut, shortcut);
        }
    }

    #[test]
    fn azioni_include_gestione_estensioni() {
        let estensioni = action_entries()
            .into_iter()
            .find(|e| e.name == "Gestione estensioni…")
            .expect("l'azione «Gestione estensioni…» deve esistere");
        assert_eq!(estensioni.tab, SearchTab::Azioni);
        assert_eq!(estensioni.act, Some(MenuAct::ShowExtensions));
    }

    fn riga(name: &str, kind: &'static str, detail: Option<&str>, line: usize) -> structure::Row {
        structure::Row {
            name: name.to_owned(),
            detail: detail.map(str::to_owned),
            kind,
            depth: 0,
            line,
        }
    }

    /// La scheda Simboli mostra la fotografia di Struttura: stessi nomi,
    /// stesse righe, e il clic deve saltare esattamente lì.
    #[test]
    fn la_scheda_simboli_mostra_la_fotografia_di_struttura_e_filtra_per_query() {
        let snapshot = structure::Snapshot {
            path: Some(PathBuf::from("src/main.rs")),
            rows: vec![
                riga("main", "fn", Some("fn main()"), 1),
                riga("Config", "struct", None, 8),
            ],
            empty_reason: "",
        };
        let mut overlay = GlobalSearchOverlay::open(SearchTab::Simboli);
        overlay.set_symbol_results(&snapshot);

        let tutti = overlay.entries();
        assert_eq!(tutti.len(), 2);
        assert!(tutti.iter().all(|e| e.tab == SearchTab::Simboli));
        assert_eq!(
            tutti
                .iter()
                .find(|e| e.name == "Config")
                .and_then(|e| e.line),
            Some(8)
        );

        // Come per i File, digitare azzera la fotografia in cache: sta a chi
        // guida (`Lume::sync_search_symbols`) rimetterla, esattamente come
        // fa per ogni tasto battuto. Qui si simula quel giro.
        overlay.update(GlobalSearchMessage::QueryChanged("conf".into()));
        overlay.set_symbol_results(&snapshot);
        let filtrati = overlay.entries();
        assert_eq!(filtrati.len(), 1);
        assert_eq!(filtrati[0].name, "Config");
    }

    /// Un documento senza percorso — la console SQL, per esempio — non ha
    /// dove far atterrare un simbolo: niente entry, come in Struttura.
    #[test]
    fn senza_percorso_niente_entry_simboli() {
        let mut overlay = GlobalSearchOverlay::open(SearchTab::Simboli);
        overlay.set_symbol_results(&structure::Snapshot {
            path: None,
            rows: vec![riga("main", "fn", None, 1)],
            empty_reason: "",
        });
        assert!(overlay.entries().is_empty());
    }

    /// «Nessun risultato» non distinguerebbe «il server non è avviato» da
    /// «questo file non ha simboli»: chi ha spento l'estensione LSP lo deve
    /// leggere, non indovinarlo — è la stessa regola zero-mock che Struttura
    /// applica al proprio `empty_reason`.
    #[test]
    fn set_symbol_results_riporta_il_motivo_solo_con_un_percorso() {
        let mut overlay = GlobalSearchOverlay::open(SearchTab::Simboli);
        overlay.set_symbol_results(&structure::Snapshot {
            path: Some(PathBuf::from("src/main.rs")),
            rows: Vec::new(),
            empty_reason: "Language server non avviato.",
        });
        assert_eq!(overlay.symbol_empty_reason, "Language server non avviato.");

        // Senza percorso il motivo non serve a niente: non c'è comunque
        // un'entry possibile (vedi `symbol_entries`).
        overlay.set_symbol_results(&structure::Snapshot {
            path: None,
            rows: Vec::new(),
            empty_reason: "Nessun file aperto.",
        });
        assert_eq!(overlay.symbol_empty_reason, "");
    }

    #[test]
    fn simboli_mostra_il_motivo_vero_quando_e_vuoto() {
        assert_eq!(
            search_feedback(
                "",
                SearchTab::Simboli,
                "Language server non avviato.",
                false,
                None,
                true,
            ),
            Some("Language server non avviato.")
        );
        // Senza un motivo specifico resta il messaggio generico.
        assert_eq!(
            search_feedback("", SearchTab::Simboli, "", false, None, true),
            Some("Nessun risultato")
        );
    }

    /// Chiudere la ricerca — anche da Esc, che passa da `cancel_file_search`
    /// e non da `close` — svuota anche i Simboli: altrimenti la prossima
    /// apertura mostrerebbe per un istante il file di prima.
    #[test]
    fn chiudere_svuota_anche_i_simboli() {
        let mut overlay = GlobalSearchOverlay::open(SearchTab::Simboli);
        overlay.set_symbol_results(&structure::Snapshot {
            path: Some(PathBuf::from("src/main.rs")),
            rows: vec![riga("main", "fn", None, 1)],
            empty_reason: "",
        });
        assert_eq!(overlay.entries().len(), 1);
        overlay.close();
        assert!(overlay.entries().is_empty());
    }

    #[test]
    fn chiusura_cambio_query_e_scheda_resettano_ancora_i_risultati() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("selettivo.rs");
        fs::write(&file, "fn selettivo() {}\n").unwrap();
        for azione in ["chiudi", "query", "scheda"] {
            let mut overlay = GlobalSearchOverlay::open(SearchTab::Tutto);
            overlay.update(GlobalSearchMessage::QueryChanged("selettivo".into()));
            let generation = AtomicU64::new(7);
            let file_reali = file_entries(root.path(), "selettivo", &generation, 7, None);
            assert_eq!(file_reali.len(), 1);
            overlay.begin_file_search(7);
            overlay.finish_file_search(7, Ok(file_reali));
            overlay.set_symbol_results(&structure::Snapshot {
                path: Some(file.clone()),
                rows: vec![riga("selettivo", "fn", None, 1)],
                empty_reason: "",
            });
            overlay.update(GlobalSearchMessage::Navigate(1));
            assert_eq!(overlay.state.selected, 1);
            assert_eq!(overlay.file_results.len(), 1);
            assert_eq!(overlay.symbol_results.len(), 1);
            match azione {
                "chiudi" => overlay.close(),
                "query" => {
                    overlay.update(GlobalSearchMessage::QueryChanged("altra".into()));
                }
                _ => {
                    overlay.update(GlobalSearchMessage::TabClicked(SearchTab::File));
                }
            }
            assert!(overlay.file_results.is_empty(), "{azione}");
            assert!(overlay.symbol_results.is_empty(), "{azione}");
            assert_eq!(overlay.state.selected, 0, "{azione}");
            assert!(!overlay.file_searching, "{azione}");
            assert_eq!(overlay.file_search_generation, None, "{azione}");
            if azione == "chiudi" {
                assert!(!overlay.is_open());
                assert!(overlay.state.query.is_empty());
            }
        }
    }

    /// Un simbolo condivide il `path` del file con un'entry File — ma
    /// l'icona deve restare il genere (`fn`, `struct`…), non l'icona del
    /// tipo di file in cui vive.
    #[test]
    fn le_entry_simboli_non_mostrano_licona_del_file() {
        let file = Entry {
            tab: SearchTab::File,
            name: "main.rs".into(),
            detail: String::new(),
            search_path: None,
            icon: String::new(),
            tone: Tone::Warn,
            shortcut: String::new(),
            act: None,
            path: Some(PathBuf::from("src/main.rs")),
            line: None,
        };
        let mut simbolo = file.clone();
        simbolo.tab = SearchTab::Simboli;

        assert!(shows_file_type_icon(&file));
        assert!(!shows_file_type_icon(&simbolo));
    }

    /// La cella icona è 22px: un'etichetta di parola come «struct» — quel
    /// che Struttura mostra nella sua riga larga — ci straboccerebbe.
    #[test]
    fn ogni_genere_di_simbolo_ha_un_glifo_di_un_carattere() {
        for kind in [
            "fn",
            "struct",
            "class",
            "trait",
            "enum",
            "mod",
            "ns",
            "prop",
            "field",
            "new",
            "let",
            "const",
            "case",
            "type",
            "op",
            "file",
            "•",
            "qualcosa-di-mai-visto",
        ] {
            assert_eq!(
                symbol_glyph(kind).chars().count(),
                1,
                "«{kind}» non mappa a un glifo di un carattere"
            );
        }
    }

    #[test]
    fn shared_file_icon_resolver_serves_project_search_and_tabs() {
        let source = CountingSource(Cell::new(0));
        assert!(resolve_row_icon(&source, "src", true, true).is_some());
        let entry = Entry {
            tab: SearchTab::File,
            name: "Main.java".to_owned(),
            detail: "src".to_owned(),
            search_path: Some("src/Main.java".to_owned()),
            icon: String::new(),
            tone: Tone::Warn,
            shortcut: String::new(),
            act: None,
            path: Some(PathBuf::from("src/Main.java")),
            line: None,
        };
        assert!(resolve_entry_icon(&source, &entry).is_some());
        let tab = EditorTab {
            id: DocId(1),
            kind: crate::editor_groups::EditorKind::Code,
            name: "lib.rs".to_owned(),
            is_dirty: false,
            is_active: true,
            path: Some(PathBuf::from("src/lib.rs")),
        };
        assert!(resolve_tab_icon(&source, &tab).is_some());
        assert_eq!(source.0.get(), 3);
    }

    #[test]
    fn file_entries_read_project_and_respect_gitignore() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lume_palette_{nanos}"));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("hidden.rs"), "").unwrap();
        fs::write(root.join(".gitignore"), "hidden.rs\n").unwrap();

        let generation = AtomicU64::new(1);
        let entries = file_entries(&root, ".rs", &generation, 1, None);

        assert!(entries.iter().any(|entry| entry.name == "main.rs"));
        assert!(!entries.iter().any(|entry| entry.name == "hidden.rs"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_entries_non_entrano_in_jj() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lume_palette_jj_{nanos}"));
        fs::create_dir_all(root.join(".jj").join("repo")).unwrap();
        fs::write(root.join(".jj").join("repo").join("store.txt"), "").unwrap();
        fs::write(root.join("stored.txt"), "").unwrap();
        let generation = AtomicU64::new(1);
        let entries = file_entries(&root, "store", &generation, 1, None);
        assert!(entries.iter().any(|entry| entry.name == "stored.txt"));
        assert!(!entries.iter().any(|entry| entry.name == "store.txt"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn background_file_search_respects_ignore_and_bounds_results() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lume_palette_worker_{nanos}"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".gitignore"), "hidden.rs\n").unwrap();
        fs::write(root.join("hidden.rs"), "").unwrap();
        for index in 0..(MAX_FILE_SEARCH_RESULTS + 8) {
            fs::write(root.join(format!("visible-{index:03}.rs")), "").unwrap();
        }

        let worker = GlobalFileSearchWorker::start().unwrap();
        let generation = Arc::new(AtomicU64::new(1));
        let receiver = worker
            .search(root.clone(), ".rs".into(), generation, 1)
            .unwrap();
        let entries = futures::executor::block_on(receiver).unwrap().unwrap();

        assert_eq!(entries.len(), MAX_FILE_SEARCH_RESULTS);
        assert!(entries.iter().all(|entry| entry.name != "hidden.rs"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistent_worker_coalesces_burst_to_latest_query() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lume_palette_coalesce_{nanos}"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("alpha.rs"), "").unwrap();
        fs::write(root.join("beta.rs"), "").unwrap();

        let worker = GlobalFileSearchWorker::start().unwrap();
        let generation = Arc::new(AtomicU64::new(1));
        let first = worker
            .search(root.clone(), "alpha".into(), generation.clone(), 1)
            .unwrap();
        generation.store(2, std::sync::atomic::Ordering::SeqCst);
        let second = worker
            .search(root.clone(), "beta".into(), generation, 2)
            .unwrap();

        assert!(matches!(
            futures::executor::block_on(first).unwrap(),
            Err(FileSearchError::Superseded)
        ));
        let latest = futures::executor::block_on(second).unwrap().unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].name, "beta.rs");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ranked_top_k_keeps_late_exact_filename_over_weak_matches() {
        let mut ranked = RankedFileMatches::new("target", 2);
        for name in ["target-helper.rs", "other-target.rs"] {
            ranked.consider(Entry {
                tab: SearchTab::File,
                name: name.into(),
                detail: "src".into(),
                search_path: Some(format!("src/{name}")),
                icon: String::new(),
                tone: Tone::Warn,
                shortcut: String::new(),
                act: None,
                path: Some(PathBuf::from(name)),
                line: None,
            });
        }
        ranked.consider(Entry {
            tab: SearchTab::File,
            name: "target".into(),
            detail: "deep".into(),
            search_path: Some("deep/target".into()),
            icon: String::new(),
            tone: Tone::Warn,
            shortcut: String::new(),
            act: None,
            path: Some(PathBuf::from("deep/target")),
            line: None,
        });

        let entries = ranked.into_sorted();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "target");
    }

    #[test]
    fn ranked_search_matches_directory_and_filename_as_one_path() {
        let mut ranked = RankedFileMatches::new(r"src\main.rs", 2);
        ranked.consider(Entry {
            tab: SearchTab::File,
            name: "main.rs".into(),
            detail: "src".into(),
            search_path: Some("src/main.rs".into()),
            icon: String::new(),
            tone: Tone::Warn,
            shortcut: String::new(),
            act: None,
            path: Some(PathBuf::from("src/main.rs")),
            line: None,
        });

        let entries = ranked.into_sorted();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "main.rs");
    }

    #[test]
    fn worker_error_remains_visible_beside_action_results() {
        assert_eq!(
            search_feedback(
                "file",
                SearchTab::Tutto,
                "",
                false,
                Some("worker non disponibile"),
                false,
            ),
            Some("worker non disponibile")
        );
    }

    #[test]
    fn stale_background_file_search_is_cancelled_by_generation() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lume_palette_cancel_{nanos}"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("main.rs"), "").unwrap();

        let worker = GlobalFileSearchWorker::start().unwrap();
        let generation = Arc::new(AtomicU64::new(2));
        let receiver = worker
            .search(root.clone(), "main".into(), generation, 1)
            .unwrap();
        let result = futures::executor::block_on(receiver).unwrap();

        assert!(matches!(result, Err(FileSearchError::Superseded)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opening_search_does_not_eagerly_enumerate_project_files() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lume_palette_open_{nanos}"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("must-not-be-read.rs"), "fn main() {}").unwrap();

        let overlay = GlobalSearchOverlay::open(SearchTab::Tutto);
        let entries = overlay.entries();

        assert!(
            entries.iter().all(|entry| entry.path.is_none()),
            "opening the palette with an empty query must stay memory-only"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn query_filters_only_file_results_cached_in_memory() {
        let mut overlay = GlobalSearchOverlay::open(SearchTab::Tutto);
        overlay.update(GlobalSearchMessage::QueryChanged("main".into()));
        overlay.begin_file_search(7);
        overlay.finish_file_search(
            7,
            Ok(vec![
                Entry {
                    tab: SearchTab::File,
                    name: "main.rs".into(),
                    detail: "src".into(),
                    search_path: Some("src/main.rs".into()),
                    icon: String::new(),
                    tone: Tone::Warn,
                    shortcut: String::new(),
                    act: None,
                    path: Some(PathBuf::from("src/main.rs")),
                    line: None,
                },
                Entry {
                    tab: SearchTab::File,
                    name: "notes.md".into(),
                    detail: "docs".into(),
                    search_path: Some("docs/notes.md".into()),
                    icon: String::new(),
                    tone: Tone::Warn,
                    shortcut: String::new(),
                    act: None,
                    path: Some(PathBuf::from("docs/notes.md")),
                    line: None,
                },
            ]),
        );

        let entries = overlay.entries();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.as_deref(), Some(Path::new("src/main.rs")));
    }

    #[test]
    fn cached_path_query_matches_the_full_relative_path() {
        let mut overlay = GlobalSearchOverlay::open(SearchTab::File);
        overlay.update(GlobalSearchMessage::QueryChanged(r"src\main.rs".into()));
        overlay.begin_file_search(8);
        overlay.finish_file_search(
            8,
            Ok(vec![Entry {
                tab: SearchTab::File,
                name: "main.rs".into(),
                detail: "src".into(),
                search_path: Some("src/main.rs".into()),
                icon: String::new(),
                tone: Tone::Warn,
                shortcut: String::new(),
                act: None,
                path: Some(PathBuf::from("src/main.rs")),
                line: None,
            }]),
        );

        let entries = overlay.entries();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.as_deref(), Some(Path::new("src/main.rs")));
    }

    #[test]
    fn escape_is_handled_by_the_open_overlay() {
        let mut overlay = GlobalSearchOverlay::open(SearchTab::Tutto);
        assert_eq!(
            overlay.update(GlobalSearchMessage::Escape),
            GlobalSearchIntent::Closed
        );
        assert!(!overlay.is_open());
    }

    #[test]
    fn azioni_include_salva_con_nome() {
        let voce = action_entries()
            .into_iter()
            .find(|e| e.name == "Salva con nome…")
            .expect("l'azione «Salva con nome…» deve esistere");
        assert_eq!(voce.tab, SearchTab::Azioni);
        assert_eq!(voce.act, Some(MenuAct::SaveAs));
    }
}
