//! Modello terminale testabile: emulazione VT, snapshot e selezione.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use alacritty_terminal::grid::{Dimensions, Scroll as AlacrittyScroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, Processor};

use super::{GridDimensions, PtyResponder, SharedWriter, TerminalError, TerminalSize};
const MAX_HYPERLINK_BYTES: usize = 2048;
/// Righe di storia di una sessione visibile: il default di alacritty.
pub const SCROLLBACK_DEFAULT: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalPoint {
    pub row: usize,
    pub col: usize,
}

impl TerminalPoint {
    pub const fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalColor {
    Named(u16),
    Indexed(u8),
    Rgb { r: u8, g: u8, b: u8 },
}

impl From<Color> for TerminalColor {
    fn from(color: Color) -> Self {
        match color {
            Color::Named(color) => Self::Named(color as u16),
            Color::Indexed(index) => Self::Indexed(index),
            Color::Spec(rgb) => Self::Rgb {
                r: rgb.r,
                g: rgb.g,
                b: rgb.b,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCell {
    pub point: TerminalPoint,
    pub character: char,
    pub zerowidth: Vec<char>,
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub inverse: bool,
    pub hidden: bool,
    pub wide_spacer: bool,
    pub wide: bool,
    pub hyperlink: Option<Arc<str>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSelection {
    pub start: TerminalPoint,
    pub end: TerminalPoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursor {
    pub point: TerminalPoint,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSnapshot {
    pub rows: usize,
    pub cols: usize,
    pub cells: Vec<TerminalCell>,
    pub cursor: TerminalCursor,
    pub selection: Option<TerminalSelection>,
    pub display_offset: usize,
    pub alternate_screen: bool,
    pub application_cursor: bool,
    pub bracketed_paste: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalScroll {
    Delta(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

impl From<TerminalScroll> for AlacrittyScroll {
    fn from(scroll: TerminalScroll) -> Self {
        match scroll {
            TerminalScroll::Delta(lines) => Self::Delta(lines),
            TerminalScroll::PageUp => Self::PageUp,
            TerminalScroll::PageDown => Self::PageDown,
            TerminalScroll::Top => Self::Top,
            TerminalScroll::Bottom => Self::Bottom,
        }
    }
}

/// Il frame in cache, con la generazione a cui appartiene.
type SnapshotCache = Option<(u64, Arc<TerminalSnapshot>)>;

#[derive(Clone)]
pub struct TerminalModel {
    terminal: Arc<Mutex<Term<PtyResponder>>>,
    parser: Arc<Mutex<Processor>>,
    /// Cresce a ogni mutazione visibile (output, resize, scroll, selezione),
    /// sempre **sotto il lock del terminale**: chi lo legge sotto lo stesso
    /// lock non può vedere una generazione che non corrisponde alla griglia.
    generation: Arc<AtomicU64>,
    cache: Arc<Mutex<SnapshotCache>>,
}

impl TerminalModel {
    pub fn new(size: TerminalSize) -> Self {
        let writer: SharedWriter = Arc::new(Mutex::new(Box::new(io::sink())));
        Self::with_writer(size, writer)
    }

    pub(crate) fn with_writer(size: TerminalSize, writer: SharedWriter) -> Self {
        let dimensions = GridDimensions::from(size);
        let responder = PtyResponder { writer };
        Self {
            terminal: Arc::new(Mutex::new(Term::new(
                Config {
                    scrolling_history: SCROLLBACK_DEFAULT,
                    ..Config::default()
                },
                &dimensions,
                responder,
            ))),
            parser: Arc::new(Mutex::new(Processor::new())),
            generation: Arc::new(AtomicU64::new(0)),
            cache: Arc::new(Mutex::new(None)),
        }
    }

    pub fn advance(&self, bytes: &[u8]) -> Result<(), TerminalError> {
        let mut parser = self
            .parser
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let mut terminal = self
            .terminal
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        parser.advance(&mut *terminal, bytes);
        self.bump_generation();
        Ok(())
    }

    pub fn resize(&self, size: TerminalSize) -> Result<(), TerminalError> {
        let mut terminal = self
            .terminal
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        terminal.resize(GridDimensions::from(size));
        self.bump_generation();
        Ok(())
    }

    /// Cambia il limite della storia. Ridurlo **butta** le righe oltre il
    /// limite: rialzarlo dopo non le ridà. La memoria rientra davvero solo se
    /// la riduzione supera la cache interna di alacritty (1.000 righe).
    /// `set_options` manda anche un `Title`/`ResetTitle` all'event proxy:
    /// `PtyResponder` gestisce solo `PtyWrite`, quindi è innocuo.
    pub fn set_scrollback(&self, lines: usize) -> Result<(), TerminalError> {
        let mut terminal = self
            .terminal
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        terminal.set_options(Config {
            scrolling_history: lines,
            ..Config::default()
        });
        self.bump_generation();
        Ok(())
    }

    /// Righe di storia oltre lo schermo.
    pub fn history_size(&self) -> Result<usize, TerminalError> {
        Ok(self
            .terminal
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .grid()
            .history_size())
    }

    pub fn snapshot(&self) -> Result<TerminalSnapshot, TerminalError> {
        let terminal = self
            .terminal
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        Ok(build_snapshot(&terminal))
    }

    /// Lo stesso frame di `snapshot`, ma condiviso: finché la generazione non
    /// cambia si restituisce lo stesso `Arc`, senza allocare né visitare la
    /// griglia. È la strada della `view()`, che gira a ogni messaggio.
    pub fn snapshot_cached(&self) -> Result<Arc<TerminalSnapshot>, TerminalError> {
        let terminal = self
            .terminal
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let generation = self.generation.load(Ordering::Acquire);
        let mut cache = self.cache.lock().map_err(|_| TerminalError::LockPoisoned)?;
        if let Some((cached, snapshot)) = cache.as_ref()
            && *cached == generation
        {
            return Ok(Arc::clone(snapshot));
        }
        let snapshot = Arc::new(build_snapshot(&terminal));
        *cache = Some((generation, Arc::clone(&snapshot)));
        Ok(snapshot)
    }

    /// Butta il frame in cache (dock nascosto: nessuno lo guarda). Il
    /// prossimo `snapshot_cached` lo ricostruisce.
    pub fn release_snapshot_cache(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            *cache = None;
        }
    }

    pub fn snapshot_cache_held(&self) -> bool {
        self.cache.lock().is_ok_and(|cache| cache.is_some())
    }

    fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    #[allow(clippy::indexing_slicing)] // le celle dello snapshot sono entro rows x cols
    pub fn visible_text(&self) -> Result<Vec<String>, TerminalError> {
        let snapshot = self.snapshot()?;
        let mut rows = vec![vec![' '; snapshot.cols]; snapshot.rows];
        for cell in snapshot.cells {
            if !cell.hidden && !cell.wide_spacer {
                rows[cell.point.row][cell.point.col] = cell.character;
            }
        }
        Ok(rows
            .into_iter()
            .map(|row| row.into_iter().collect::<String>().trim_end().to_owned())
            .collect())
    }

    pub fn scroll(&self, scroll: TerminalScroll) -> Result<(), TerminalError> {
        let mut terminal = self
            .terminal
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        terminal.scroll_display(scroll.into());
        self.bump_generation();
        Ok(())
    }

    pub fn start_selection(&self, point: TerminalPoint) -> Result<(), TerminalError> {
        let mut terminal = self
            .terminal
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let point = grid_point(&terminal, point);
        terminal.selection = Some(Selection::new(SelectionType::Simple, point, Side::Left));
        self.bump_generation();
        Ok(())
    }

    pub fn update_selection(&self, point: TerminalPoint) -> Result<(), TerminalError> {
        let mut terminal = self
            .terminal
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let point = grid_point(&terminal, point);
        if let Some(selection) = &mut terminal.selection {
            selection.update(point, Side::Right);
        }
        self.bump_generation();
        Ok(())
    }

    pub fn clear_selection(&self) -> Result<(), TerminalError> {
        let mut terminal = self
            .terminal
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        terminal.selection = None;
        self.bump_generation();
        Ok(())
    }

    pub fn selected_text(&self) -> Result<Option<String>, TerminalError> {
        Ok(self
            .terminal
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .selection_to_string())
    }

    pub fn hyperlink_at(&self, point: TerminalPoint) -> Result<Option<String>, TerminalError> {
        Ok(self
            .snapshot()?
            .cells
            .into_iter()
            .find(|cell| cell.point == point)
            .and_then(|cell| cell.hyperlink)
            .map(|link| link.to_string()))
    }
}

fn build_snapshot(terminal: &Term<PtyResponder>) -> TerminalSnapshot {
    let rows = terminal.screen_lines();
    let cols = terminal.columns();
    let content = terminal.renderable_content();
    let display_offset = content.display_offset;
    let cursor = TerminalCursor {
        point: viewport_point(content.cursor.point, display_offset),
        visible: content.cursor.shape != alacritty_terminal::vte::ansi::CursorShape::Hidden,
    };
    let selection = content.selection.map(|selection| TerminalSelection {
        start: viewport_point(selection.start, display_offset),
        end: viewport_point(selection.end, display_offset),
    });
    let alternate_screen = content.mode.contains(TermMode::ALT_SCREEN);
    let mut cells = Vec::with_capacity(rows.saturating_mul(cols));
    let application_cursor = content.mode.contains(TermMode::APP_CURSOR);
    let bracketed_paste = content.mode.contains(TermMode::BRACKETED_PASTE);

    let mut hyperlinks: HashMap<String, Arc<str>> = HashMap::new();
    for indexed in content.display_iter {
        let point = viewport_point(indexed.point, display_offset);
        if point.row >= rows || point.col >= cols {
            continue;
        }
        let flags = indexed.cell.flags;
        cells.push(TerminalCell {
            point,
            character: indexed.cell.c,
            zerowidth: indexed.cell.zerowidth().unwrap_or_default().to_vec(),
            foreground: indexed.cell.fg.into(),
            background: indexed.cell.bg.into(),
            bold: flags.contains(Flags::BOLD),
            italic: flags.contains(Flags::ITALIC),
            underline: flags.intersects(Flags::ALL_UNDERLINES),
            strikeout: flags.contains(Flags::STRIKEOUT),
            inverse: flags.contains(Flags::INVERSE),
            hidden: flags.contains(Flags::HIDDEN),
            wide_spacer: flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
            hyperlink: indexed.cell.hyperlink().and_then(|link| {
                let uri = link.uri();
                if uri.len() > MAX_HYPERLINK_BYTES
                    || uri.chars().any(|character| character.is_control())
                {
                    return None;
                }
                if let Some(existing) = hyperlinks.get(uri) {
                    return Some(existing.clone());
                }
                let shared: Arc<str> = Arc::from(uri);
                hyperlinks.insert(uri.to_owned(), shared.clone());
                Some(shared)
            }),
            wide: flags.contains(Flags::WIDE_CHAR),
        });
    }

    TerminalSnapshot {
        rows,
        cols,
        cells,
        cursor,
        selection,
        display_offset,
        alternate_screen,
        application_cursor,
        bracketed_paste,
    }
}

fn viewport_point(point: Point, display_offset: usize) -> TerminalPoint {
    TerminalPoint::new(
        (point.line.0 + display_offset as i32).max(0) as usize,
        point.column.0,
    )
}

fn grid_point(terminal: &Term<PtyResponder>, point: TerminalPoint) -> Point {
    let row = point.row.min(terminal.screen_lines().saturating_sub(1));
    let col = point.col.min(terminal.columns().saturating_sub(1));
    Point::new(
        Line(row as i32 - terminal.grid().display_offset() as i32),
        Column(col),
    )
}
