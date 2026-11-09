//! Demo minimale dell'editor: apre un Document con una fixture Rust,
//! monta `editor_view` e applica gli `EditorCommand` al Document.
//!
//! `cargo run -p lume-ui --example editor_demo`

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use iced::Element;
use lume_core::{AppliedEdit, DocId, Document, Range, movement};
use lume_syntax::{InputEditRaw, LanguageId, SyntaxState};
use lume_theme::LumeTheme;
use lume_ui::editor::{EditorCommand, Motion, PageDirection, editor_view};

const FIXTURE: &str = r#"//! Fixture: un piccolo modulo Rust per esercitare l'highlighting.

use std::collections::HashMap;
use std::fmt;

/// Un token di configurazione con nome e priorita'.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigEntry {
    pub name: String,
    pub priority: u32,
    pub enabled: bool,
}

impl ConfigEntry {
    /// Crea una entry abilitata con priorita' di default (10).
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            priority: 10,
            enabled: true,
        }
    }

    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }
}

impl fmt::Display for ConfigEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (p={})", self.name, self.priority)
    }
}

/// Registro delle entry, indicizzate per nome.
#[derive(Default)]
pub struct Registry {
    entries: HashMap<String, ConfigEntry>,
    version: u64,
}

impl Registry {
    pub fn insert(&mut self, entry: ConfigEntry) -> Option<ConfigEntry> {
        self.version += 1;
        self.entries.insert(entry.name.clone(), entry)
    }

    pub fn get(&self, name: &str) -> Option<&ConfigEntry> {
        self.entries.get(name)
    }

    /// Le entry attive, ordinate per priorita' decrescente.
    pub fn active(&self) -> Vec<&ConfigEntry> {
        let mut out: Vec<_> = self.entries.values().filter(|e| e.enabled).collect();
        out.sort_by(|a, b| b.priority.cmp(&a.priority));
        out
    }
}

fn main() {
    let mut registry = Registry::default();
    registry.insert(ConfigEntry::new("alpha").with_priority(30));
    registry.insert(ConfigEntry::new("beta"));
    registry.insert(ConfigEntry::new("gamma").with_priority(0x2A));

    for entry in registry.active() {
        println!("attiva: {entry}");
    }

    let total: u32 = registry.active().iter().map(|e| e.priority).sum();
    assert!(total > 0, "almeno una priorita' positiva");
    println!("totale = {total}, versione = {}", registry.version);
}
"#;

/// Quante righe muove PageUp/PageDown nella demo.
struct App {
    doc: Document,
    syntax: Option<SyntaxState>,
    theme: LumeTheme,
}

#[derive(Debug, Clone)]
enum Msg {
    Editor(EditorCommand),
}

impl App {
    fn new() -> Self {
        let doc = Document::from_str(FIXTURE, None, DocId(1));
        let syntax = SyntaxState::new(LanguageId::Rust, doc.rope().slice(..));
        Self {
            doc,
            syntax,
            theme: LumeTheme::builtin().into_iter().next().unwrap_or_default(),
        }
    }

    /// Riallinea tree-sitter dopo un edit applicato al documento.
    fn resync(&mut self, edit: AppliedEdit) {
        if let Some(syntax) = &mut self.syntax {
            let raw = InputEditRaw {
                start_byte: edit.start_byte,
                old_end_byte: edit.old_end_byte,
                new_end_byte: edit.new_end_byte,
                start_point: edit.start_point,
                old_end_point: edit.old_end_point,
                new_end_point: edit.new_end_point,
            };
            syntax.apply_edit(&raw, self.doc.rope().slice(..));
        }
    }

    fn multi_edit(&mut self, edit: lume_core::multi_edit::MultiEdit<'_>) {
        match lume_core::multi_edit::apply(&mut self.doc, edit) {
            Ok(Some(edit)) => self.resync(edit),
            Ok(None) => {}
            Err(error) => eprintln!("{error}"),
        }
    }

    fn apply_motion(&mut self, motion: Motion, extend: bool) {
        let rope = self.doc.rope();
        let ranges = self
            .doc
            .selections()
            .ranges()
            .iter()
            .map(|sel| {
                let head = sel.head;
                let col = head - rope.line_to_char(rope.char_to_line(head.min(rope.len_chars())));
                let new_head = match motion {
                    Motion::Left => movement::prev_grapheme(rope, head),
                    Motion::Right => movement::next_grapheme(rope, head),
                    Motion::WordLeft => movement::word_left(rope, head),
                    Motion::WordRight => movement::word_right(rope, head),
                    Motion::Up => movement::line_up(rope, head, col),
                    Motion::Down => movement::line_down(rope, head, col),
                    Motion::Home => movement::line_home_smart(rope, head),
                    Motion::End => movement::line_end(rope, head),
                    Motion::DocStart => movement::doc_start(rope),
                    Motion::DocEnd => movement::doc_end(rope),
                };
                if extend {
                    Range::new(sel.anchor, new_head)
                } else {
                    Range::point(new_head)
                }
            })
            .collect();
        if let Ok(set) = lume_core::SelectionSet::try_new(
            ranges,
            self.doc.selections().primary_index(),
            rope.len_chars(),
        ) {
            self.doc.set_selections(set);
        }
    }

    fn apply_page(&mut self, direction: PageDirection, lines: usize, extend: bool) {
        let rope = self.doc.rope();
        let ranges = self
            .doc
            .selections()
            .ranges()
            .iter()
            .map(|sel| {
                let head = sel.head;
                let col = head - rope.line_to_char(rope.char_to_line(head.min(rope.len_chars())));
                let new_head = (0..lines.max(1)).fold(head, |position, _| match direction {
                    PageDirection::Up => movement::line_up(rope, position, col),
                    PageDirection::Down => movement::line_down(rope, position, col),
                });
                if extend {
                    Range::new(sel.anchor, new_head)
                } else {
                    Range::point(new_head)
                }
            })
            .collect();
        if let Ok(set) = lume_core::SelectionSet::try_new(
            ranges,
            self.doc.selections().primary_index(),
            rope.len_chars(),
        ) {
            self.doc.set_selections(set);
        }
    }

    fn update(&mut self, msg: Msg) {
        let Msg::Editor(cmd) = msg;
        match cmd {
            EditorCommand::Insert(s) | EditorCommand::Paste(s) => {
                self.multi_edit(lume_core::multi_edit::MultiEdit::Replace(&s))
            }
            EditorCommand::Enter => self.multi_edit(lume_core::multi_edit::MultiEdit::Enter),
            EditorCommand::Tab => self.multi_edit(lume_core::multi_edit::MultiEdit::Tab),
            EditorCommand::Backspace => {
                self.multi_edit(lume_core::multi_edit::MultiEdit::Backspace)
            }
            EditorCommand::Delete => self.multi_edit(lume_core::multi_edit::MultiEdit::Delete),
            EditorCommand::Cut => self.multi_edit(lume_core::multi_edit::MultiEdit::Cut),
            EditorCommand::ToggleCursor { .. }
            | EditorCommand::AddNextOccurrence
            | EditorCommand::AddCursorLine { .. }
            | EditorCommand::KeepPrimary => {
                let result = match cmd {
                    EditorCommand::ToggleCursor { pos } => lume_core::occurrence::toggle(
                        self.doc.selections(),
                        pos,
                        self.doc.rope().len_chars(),
                    ),
                    EditorCommand::AddNextOccurrence => {
                        lume_core::occurrence::next(self.doc.rope(), self.doc.selections())
                    }
                    EditorCommand::AddCursorLine { above } => lume_core::occurrence::adjacent(
                        self.doc.rope(),
                        self.doc.selections(),
                        above,
                    ),
                    _ => Ok(self.doc.selections().keep_primary()),
                };
                match result {
                    Ok(set) => self.doc.set_selections(set),
                    Err(error) => eprintln!("{error}"),
                }
            }
            EditorCommand::MovePage {
                direction,
                lines,
                extend,
            } => self.apply_page(direction, lines, extend),
            EditorCommand::Copy => {} // la clipboard e' gia' stata scritta dal widget
            EditorCommand::NoteMarkerClicked { .. }
            | EditorCommand::FoldClicked { .. }
            | EditorCommand::BreakpointClicked { .. } => {}
            // il menu' contestuale e' dell'app, non del widget: la demo non
            // ne ha uno, e ignorarlo qui e' la risposta giusta
            EditorCommand::ContextMenu { .. } => {}
            // popup e hover non esistono nell'esempio: non c'è un server
            EditorCommand::TriggerComplete
            | EditorCommand::PopupUp
            | EditorCommand::PopupDown
            | EditorCommand::PopupAccept
            | EditorCommand::PopupDismiss
            | EditorCommand::HoverAt(_) => {}
            EditorCommand::Move(motion, extend) => self.apply_motion(motion, extend),
            EditorCommand::SetCursor { pos, extend } => {
                let sel = self.doc.selection();
                self.doc.set_selection(if extend {
                    Range::new(sel.anchor, pos)
                } else {
                    Range::point(pos)
                });
            }
            EditorCommand::Undo => {
                if let Some(edit) = self.doc.undo() {
                    self.resync(edit);
                }
            }
            EditorCommand::Redo => {
                if let Some(edit) = self.doc.redo() {
                    self.resync(edit);
                }
            }
            EditorCommand::SelectAll => {
                let len = self.doc.rope().len_chars();
                self.doc.set_selection(Range::new(0, len));
            }
            EditorCommand::Save => {
                self.doc.commit_undo_group();
                self.doc.mark_saved();
                println!("save richiesto ({} byte)", self.doc.save_payload().len());
            }
        }
    }

    fn view(&self) -> Element<'_, Msg> {
        editor_view(
            &self.doc,
            self.syntax.as_ref(),
            &self.theme,
            &[],
            lume_ui::editor::EditorOverlay::default(),
            Msg::Editor,
        )
    }
}

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("Lume - editor demo")
        .theme(|app: &App| app.theme.iced_theme())
        .font(lume_ui::JETBRAINS_MONO)
        .font(lume_ui::JETBRAINS_MONO_MEDIUM)
        .font(lume_ui::JETBRAINS_MONO_SEMIBOLD)
        .window(iced::window::Settings {
            size: iced::Size::new(1000.0, 700.0),
            ..Default::default()
        })
        .run()
}
