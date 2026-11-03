//! Widget terminale Iced nativo: griglia VT, input, IME, clipboard e selezione.

use std::cell::{Cell, RefCell};

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::text::{
    Alignment as TextAlignment, LineHeight, Paragraph as _, Renderer as _, Shaping, Text, Wrapping,
};
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, InputMethod, Shell, clipboard, input_method};
use iced::font::{Style as FontStyle, Weight};
use iced::keyboard::{self, Key, key::Named};
use iced::mouse::{self, ScrollDelta};
use iced::{Color, Element, Event as IcedEvent, Length, Pixels, Point, Rectangle, Size};
use lume_terminal::{
    TerminalCell, TerminalColor, TerminalPoint, TerminalScroll, TerminalSize, TerminalSnapshot,
};
use lume_theme::LumeTheme;
use lume_ui::MONO;

use crate::tools::terminal::Event;

const FONT_SIZE: f32 = 12.5;
const CELL_W: f32 = 7.6;
const CELL_H: f32 = 18.5;
const PAD_X: f32 = 4.0;
const PAD_Y: f32 = 3.0;

type CellParagraph = <iced::Renderer as iced::advanced::text::Renderer>::Paragraph;
#[derive(Debug, Clone, PartialEq, Eq)]
struct GlyphKey {
    character: char,
    zerowidth: Vec<char>,
    bold: bool,
    italic: bool,
    wide: bool,
}

struct CachedGlyph {
    key: GlyphKey,
    paragraph: CellParagraph,
}

type GlyphCache = std::collections::HashMap<(usize, usize), CachedGlyph>;

fn glyph_key(cell: &TerminalCell) -> Option<GlyphKey> {
    (!cell.hidden && !cell.wide_spacer && cell.character != ' ').then(|| GlyphKey {
        character: cell.character,
        zerowidth: cell.zerowidth.clone(),
        bold: cell.bold,
        italic: cell.italic,
        wide: cell.wide,
    })
}

fn build_glyph(key: &GlyphKey) -> CellParagraph {
    let mut content = key.character.to_string();
    content.extend(key.zerowidth.iter().copied());
    let width = if key.wide { CELL_W * 2.0 } else { CELL_W };
    CellParagraph::with_text(Text {
        content: content.as_str(),
        bounds: Size::new(width, CELL_H),
        size: Pixels(FONT_SIZE),
        line_height: LineHeight::Absolute(Pixels(CELL_H)),
        font: iced::Font {
            weight: if key.bold {
                Weight::Bold
            } else {
                Weight::Normal
            },
            style: if key.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            ..MONO
        },
        align_x: TextAlignment::Left,
        align_y: iced::alignment::Vertical::Top,
        shaping: Shaping::Advanced,
        wrapping: Wrapping::None,
    })
}

fn cached_glyph<'a>(
    cache: &'a mut GlyphCache,
    cell: &TerminalCell,
) -> Option<(&'a CellParagraph, bool)> {
    let key = glyph_key(cell)?;
    let position = (cell.point.row, cell.point.col);
    let reused = cache.get(&position).is_some_and(|cached| cached.key == key);
    if !reused {
        cache.insert(
            position,
            CachedGlyph {
                paragraph: build_glyph(&key),
                key,
            },
        );
    }
    cache
        .get(&position)
        .map(|cached| (&cached.paragraph, reused))
}

#[derive(Default)]
struct TerminalWidgetState {
    focused: Cell<bool>,
    dragging: Cell<bool>,
    modifiers: Cell<keyboard::Modifiers>,
    preedit: RefCell<Option<input_method::Preedit>>,
    glyphs: RefCell<GlyphCache>,
    /// Combinazione per cui si è già confrontata la dimensione del terminale.
    ultima_chiave: Cell<Option<ChiaveResize>>,
}

/// Ciò da cui dipende la decisione di ridimensionare il terminale: se non
/// cambia nessuno di questi quattro valori, il confronto darebbe lo stesso
/// risultato e non si rifà.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ChiaveResize {
    larghezza: f32,
    altezza: f32,
    righe: usize,
    colonne: usize,
}

impl TerminalWidgetState {
    /// La dimensione da chiedere al terminale, se serve chiederla. `calcola`
    /// (la dimensione in celle dai bounds) gira solo quando la chiave cambia;
    /// con la stessa chiave del controllo precedente non si pubblica niente,
    /// neanche se lo snapshot non è ancora stato aggiornato.
    fn resize_richiesto(
        &self,
        bounds: Size,
        righe: usize,
        colonne: usize,
        calcola: impl FnOnce() -> TerminalSize,
    ) -> Option<TerminalSize> {
        let chiave = ChiaveResize {
            larghezza: bounds.width,
            altezza: bounds.height,
            righe,
            colonne,
        };
        if self.ultima_chiave.get() == Some(chiave) {
            return None;
        }
        self.ultima_chiave.set(Some(chiave));
        let size = calcola();
        needs_resize(righe, colonne, size).then_some(size)
    }
}

pub struct TerminalView<'a, M> {
    snapshot: std::sync::Arc<TerminalSnapshot>,
    selection_text: Option<String>,
    theme: &'a LumeTheme,
    on_event: Box<dyn Fn(Event) -> M + 'a>,
}

impl<'a, M> TerminalView<'a, M> {
    pub fn new(
        snapshot: std::sync::Arc<TerminalSnapshot>,
        selection_text: Option<String>,
        theme: &'a LumeTheme,
        on_event: impl Fn(Event) -> M + 'a,
    ) -> Self {
        Self {
            snapshot,
            selection_text,
            theme,
            on_event: Box::new(on_event),
        }
    }

    fn publish(&self, shell: &mut Shell<'_, M>, event: Event) {
        shell.publish((self.on_event)(event));
        shell.capture_event();
        shell.request_redraw();
    }

    fn point_at(&self, bounds: Rectangle, position: Point) -> TerminalPoint {
        let row = ((position.y - bounds.y - PAD_Y) / CELL_H).floor().max(0.0) as usize;
        let col = ((position.x - bounds.x - PAD_X) / CELL_W).floor().max(0.0) as usize;
        TerminalPoint::new(
            row.min(self.snapshot.rows.saturating_sub(1)),
            col.min(self.snapshot.cols.saturating_sub(1)),
        )
    }

    fn terminal_size(bounds: Rectangle) -> TerminalSize {
        let rows = ((bounds.height - PAD_Y * 2.0) / CELL_H)
            .floor()
            .max(1.0)
            .min(u16::MAX as f32) as u16;
        let cols = ((bounds.width - PAD_X * 2.0) / CELL_W)
            .floor()
            .max(1.0)
            .min(u16::MAX as f32) as u16;
        TerminalSize::new(rows, cols)
    }

    fn sync_size(&self, state: &TerminalWidgetState, bounds: Rectangle, shell: &mut Shell<'_, M>) {
        if let Some(size) = state.resize_richiesto(
            bounds.size(),
            self.snapshot.rows,
            self.snapshot.cols,
            || Self::terminal_size(bounds),
        ) {
            shell.publish((self.on_event)(Event::Resize(size)));
            shell.request_redraw();
        }
    }

    fn request_input_method(
        &self,
        state: &TerminalWidgetState,
        bounds: Rectangle,
        shell: &mut Shell<'_, M>,
    ) {
        let point = self.snapshot.cursor.point;
        let cursor = Rectangle::new(
            Point::new(
                bounds.x + PAD_X + point.col as f32 * CELL_W,
                bounds.y + PAD_Y + point.row as f32 * CELL_H,
            ),
            Size::new(CELL_W, CELL_H),
        );
        let preedit = state.preedit.borrow();
        let request = if state.focused.get() {
            InputMethod::Enabled {
                cursor,
                purpose: input_method::Purpose::Terminal,
                preedit: preedit.as_ref().cloned(),
            }
        } else {
            InputMethod::Disabled
        };
        shell.request_input_method(&request);
    }
}

fn needs_resize(rows: usize, cols: usize, size: TerminalSize) -> bool {
    rows != usize::from(size.rows) || cols != usize::from(size.cols)
}

fn xterm_modifier(modifiers: keyboard::Modifiers) -> Option<u8> {
    let value = 1
        + u8::from(modifiers.shift())
        + 2 * u8::from(modifiers.alt())
        + 4 * u8::from(modifiers.control());
    (value > 1).then_some(value)
}

fn clipboard_shortcut(modifiers: keyboard::Modifiers) -> bool {
    #[cfg(target_os = "macos")]
    {
        modifiers.logo() && !modifiers.control()
    }
    #[cfg(not(target_os = "macos"))]
    {
        modifiers.control() && modifiers.shift()
    }
}

fn key_bytes(
    key: &Key,
    modifiers: keyboard::Modifiers,
    text: Option<&str>,
    application_cursor: bool,
) -> Option<Vec<u8>> {
    if modifiers.logo() {
        return None;
    }
    if let Some(modifier) = xterm_modifier(modifiers) {
        let sequence = match key.as_ref() {
            Key::Named(Named::ArrowUp) => Some(format!("\x1b[1;{modifier}A")),
            Key::Named(Named::ArrowDown) => Some(format!("\x1b[1;{modifier}B")),
            Key::Named(Named::ArrowRight) => Some(format!("\x1b[1;{modifier}C")),
            Key::Named(Named::ArrowLeft) => Some(format!("\x1b[1;{modifier}D")),
            Key::Named(Named::Home) => Some(format!("\x1b[1;{modifier}H")),
            Key::Named(Named::End) => Some(format!("\x1b[1;{modifier}F")),
            Key::Named(Named::Insert) => Some(format!("\x1b[2;{modifier}~")),
            Key::Named(Named::Delete) => Some(format!("\x1b[3;{modifier}~")),
            Key::Named(Named::PageUp) => Some(format!("\x1b[5;{modifier}~")),
            Key::Named(Named::PageDown) => Some(format!("\x1b[6;{modifier}~")),
            Key::Named(Named::F1) => Some(format!("\x1b[1;{modifier}P")),
            Key::Named(Named::F2) => Some(format!("\x1b[1;{modifier}Q")),
            Key::Named(Named::F3) => Some(format!("\x1b[1;{modifier}R")),
            Key::Named(Named::F4) => Some(format!("\x1b[1;{modifier}S")),
            Key::Named(Named::F5) => Some(format!("\x1b[15;{modifier}~")),
            Key::Named(Named::F6) => Some(format!("\x1b[17;{modifier}~")),
            Key::Named(Named::F7) => Some(format!("\x1b[18;{modifier}~")),
            Key::Named(Named::F8) => Some(format!("\x1b[19;{modifier}~")),
            Key::Named(Named::F9) => Some(format!("\x1b[20;{modifier}~")),
            Key::Named(Named::F10) => Some(format!("\x1b[21;{modifier}~")),
            Key::Named(Named::F11) => Some(format!("\x1b[23;{modifier}~")),
            Key::Named(Named::F12) => Some(format!("\x1b[24;{modifier}~")),
            _ => None,
        };
        if let Some(sequence) = sequence {
            return Some(sequence.into_bytes());
        }
    }

    let sequence = match key.as_ref() {
        Key::Named(Named::Enter) => Some("\r"),
        Key::Named(Named::Tab) => Some(if modifiers.shift() { "\x1b[Z" } else { "\t" }),
        Key::Named(Named::Backspace) => Some("\x7f"),
        Key::Named(Named::Escape) => Some("\x1b"),
        Key::Named(Named::ArrowUp) => Some(if application_cursor {
            "\x1bOA"
        } else {
            "\x1b[A"
        }),
        Key::Named(Named::ArrowDown) => Some(if application_cursor {
            "\x1bOB"
        } else {
            "\x1b[B"
        }),
        Key::Named(Named::ArrowRight) => Some(if application_cursor {
            "\x1bOC"
        } else {
            "\x1b[C"
        }),
        Key::Named(Named::ArrowLeft) => Some(if application_cursor {
            "\x1bOD"
        } else {
            "\x1b[D"
        }),
        Key::Named(Named::Home) => Some(if application_cursor {
            "\x1bOH"
        } else {
            "\x1b[H"
        }),
        Key::Named(Named::End) => Some(if application_cursor {
            "\x1bOF"
        } else {
            "\x1b[F"
        }),
        Key::Named(Named::Insert) => Some("\x1b[2~"),
        Key::Named(Named::Delete) => Some("\x1b[3~"),
        Key::Named(Named::PageUp) => Some("\x1b[5~"),
        Key::Named(Named::PageDown) => Some("\x1b[6~"),
        Key::Named(Named::F1) => Some("\x1bOP"),
        Key::Named(Named::F2) => Some("\x1bOQ"),
        Key::Named(Named::F3) => Some("\x1bOR"),
        Key::Named(Named::F4) => Some("\x1bOS"),
        Key::Named(Named::F5) => Some("\x1b[15~"),
        Key::Named(Named::F6) => Some("\x1b[17~"),
        Key::Named(Named::F7) => Some("\x1b[18~"),
        Key::Named(Named::F8) => Some("\x1b[19~"),
        Key::Named(Named::F9) => Some("\x1b[20~"),
        Key::Named(Named::F10) => Some("\x1b[21~"),
        Key::Named(Named::F11) => Some("\x1b[23~"),
        Key::Named(Named::F12) => Some("\x1b[24~"),
        Key::Named(Named::Space) if modifiers.control() => return Some(vec![0]),
        _ => None,
    };
    if let Some(sequence) = sequence {
        return Some(sequence.as_bytes().to_vec());
    }

    if modifiers.control()
        && let Key::Character(value) = key.as_ref()
        && let Some(character) = value.chars().next()
    {
        let upper = character.to_ascii_uppercase();
        if upper.is_ascii() && ('@'..='_').contains(&upper) {
            return Some(vec![(upper as u8) & 0x1f]);
        }
    }
    if modifiers.control() {
        return None;
    }

    let value: String = text?
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    if value.is_empty() {
        return None;
    }
    let mut bytes = Vec::with_capacity(value.len() + usize::from(modifiers.alt()));
    if modifiers.alt() {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(value.as_bytes());
    Some(bytes)
}

fn paste_bytes(value: String, bracketed: bool) -> Vec<u8> {
    let sanitized = value.replace("\x1b[201~", "");
    if bracketed {
        format!("\x1b[200~{sanitized}\x1b[201~").into_bytes()
    } else {
        sanitized.into_bytes()
    }
}

fn ansi_color(color: TerminalColor, theme: &LumeTheme) -> Color {
    match color {
        TerminalColor::Rgb { r, g, b } => Color::from_rgb8(r, g, b),
        TerminalColor::Indexed(index) => indexed_color(index),
        TerminalColor::Named(index) => match index {
            0 => Color::from_rgb8(0x24, 0x27, 0x2e),
            1 => theme.bad,
            2 => theme.good,
            3 => theme.warn,
            4 => theme.accent,
            5 => theme.func,
            6 => theme.tree,
            7 => theme.ui,
            8 => theme.faint,
            9 => theme.bad,
            10 => theme.good,
            11 => theme.warn,
            12 => theme.accent,
            13 => theme.func,
            14 => theme.tree,
            15 => theme.text,
            256 | 267 => theme.ui,
            257 => theme.inset,
            258 => theme.accent,
            259 => theme.deep,
            260 => dim(theme.bad),
            261 => dim(theme.good),
            262 => dim(theme.warn),
            263 => dim(theme.accent),
            264 => dim(theme.func),
            265 => dim(theme.tree),
            266 | 268 => theme.muted,
            _ => theme.ui,
        },
    }
}

#[allow(clippy::indexing_slicing)] // il braccio 0..=15 garantisce l'indice nella tabella ANSI
fn indexed_color(index: u8) -> Color {
    const ANSI: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x31, 0x31),
        (0x0d, 0xbc, 0x79),
        (0xe5, 0xe5, 0x10),
        (0x24, 0x72, 0xc8),
        (0xbc, 0x3f, 0xbc),
        (0x11, 0xa8, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x66, 0x66, 0x66),
        (0xf1, 0x4c, 0x4c),
        (0x23, 0xd1, 0x8b),
        (0xf5, 0xf5, 0x43),
        (0x3b, 0x8e, 0xf3),
        (0xd6, 0x70, 0xd6),
        (0x29, 0xb8, 0xdb),
        (0xff, 0xff, 0xff),
    ];
    let (r, g, b) = match index {
        0..=15 => ANSI[index as usize],
        16..=231 => {
            let value = index - 16;
            let channel = |component: u8| {
                if component == 0 {
                    0
                } else {
                    55 + component * 40
                }
            };
            (
                channel(value / 36),
                channel((value / 6) % 6),
                channel(value % 6),
            )
        }
        232..=255 => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    };
    Color::from_rgb8(r, g, b)
}

fn dim(color: Color) -> Color {
    Color {
        r: color.r * 0.65,
        g: color.g * 0.65,
        b: color.b * 0.65,
        a: color.a,
    }
}

fn selected(snapshot: &TerminalSnapshot, point: TerminalPoint) -> bool {
    let Some(selection) = snapshot.selection else {
        return false;
    };
    let point = (point.row, point.col);
    let start = (selection.start.row, selection.start.col);
    let end = (selection.end.row, selection.end.col);
    let (start, end) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    (start..=end).contains(&point)
}

fn cell_colors(cell: &TerminalCell, theme: &LumeTheme) -> (Color, Color) {
    let foreground = ansi_color(cell.foreground, theme);
    let background = ansi_color(cell.background, theme);
    if cell.inverse {
        (background, foreground)
    } else {
        (foreground, background)
    }
}

impl<'a, M> Widget<M, iced::Theme, iced::Renderer> for TerminalView<'a, M> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<TerminalWidgetState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(TerminalWidgetState::default())
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &IcedEvent,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<TerminalWidgetState>();
        self.sync_size(state, bounds, shell);

        match event {
            IcedEvent::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(position) = cursor.position_over(bounds) {
                    state.focused.set(true);
                    let point = self.point_at(bounds, position);
                    if state.modifiers.get().command()
                        && self
                            .snapshot
                            .cells
                            .iter()
                            .any(|cell| cell.point == point && cell.hyperlink.is_some())
                    {
                        self.publish(shell, Event::OpenHyperlink(point));
                    } else {
                        state.dragging.set(true);
                        self.publish(shell, Event::StartSelection(point));
                    }
                } else if state.focused.replace(false) {
                    state.dragging.set(false);
                    state.preedit.borrow_mut().take();
                    shell.request_redraw();
                }
            }
            IcedEvent::Mouse(mouse::Event::CursorMoved { position }) if state.dragging.get() => {
                self.publish(
                    shell,
                    Event::UpdateSelection(self.point_at(bounds, *position)),
                );
            }
            IcedEvent::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.dragging.replace(false) =>
            {
                shell.capture_event();
            }
            IcedEvent::Mouse(mouse::Event::WheelScrolled { delta })
                if cursor.position_over(bounds).is_some() =>
            {
                let lines = match delta {
                    ScrollDelta::Lines { y, .. } => (*y * 3.0).round() as i32,
                    ScrollDelta::Pixels { y, .. } => (*y / CELL_H).round() as i32,
                };
                if lines != 0 {
                    self.publish(shell, Event::Scroll(TerminalScroll::Delta(lines)));
                }
            }
            IcedEvent::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers.set(*modifiers);
            }
            IcedEvent::Keyboard(keyboard::Event::KeyPressed {
                key,
                modifiers,
                text,
                ..
            }) if state.focused.get() => {
                state.modifiers.set(*modifiers);
                let lower = match key.as_ref() {
                    Key::Character(value) => value.to_ascii_lowercase(),
                    _ => String::new(),
                };
                if matches!(key.as_ref(), Key::Named(Named::Shift)) {
                    shell.capture_event();
                } else if clipboard_shortcut(*modifiers) && lower == "c" {
                    if let Some(selection) = &self.selection_text {
                        clipboard.write(clipboard::Kind::Standard, selection.clone());
                    }
                    shell.capture_event();
                } else if modifiers.shift() && matches!(key.as_ref(), Key::Named(Named::PageUp)) {
                    self.publish(shell, Event::Scroll(TerminalScroll::PageUp));
                } else if modifiers.shift() && matches!(key.as_ref(), Key::Named(Named::PageDown)) {
                    self.publish(shell, Event::Scroll(TerminalScroll::PageDown));
                } else if clipboard_shortcut(*modifiers) && lower == "v" {
                    if let Some(value) = clipboard.read(clipboard::Kind::Standard) {
                        self.publish(
                            shell,
                            Event::Input(paste_bytes(value, self.snapshot.bracketed_paste)),
                        );
                    }
                } else if let Some(bytes) = key_bytes(
                    key,
                    *modifiers,
                    text.as_deref(),
                    self.snapshot.application_cursor,
                ) {
                    self.publish(shell, Event::Input(bytes));
                }
            }
            IcedEvent::InputMethod(event) => match event {
                input_method::Event::Opened if state.focused.get() => {}
                input_method::Event::Preedit(content, selection) if state.focused.get() => {
                    *state.preedit.borrow_mut() =
                        (!content.is_empty()).then_some(input_method::Preedit {
                            content: content.clone(),
                            selection: selection.clone(),
                            text_size: None,
                        });
                    shell.capture_event();
                    shell.request_redraw();
                }
                input_method::Event::Commit(content) if state.focused.get() => {
                    state.preedit.borrow_mut().take();
                    if !content.is_empty() {
                        self.publish(shell, Event::Input(content.as_bytes().to_vec()));
                    }
                }
                input_method::Event::Closed => {
                    state.preedit.borrow_mut().take();
                    shell.request_redraw();
                }
                _ => {}
            },
            _ => {}
        }
        self.request_input_method(state, bounds, shell);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let Some(position) = cursor.position_over(layout.bounds()) else {
            return mouse::Interaction::None;
        };
        let state = tree.state.downcast_ref::<TerminalWidgetState>();
        let point = self.point_at(layout.bounds(), position);
        if state.modifiers.get().command()
            && self
                .snapshot
                .cells
                .iter()
                .any(|cell| cell.point == point && cell.hyperlink.is_some())
        {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::Text
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<TerminalWidgetState>();
        let quad = |bounds| Quad {
            bounds,
            ..Quad::default()
        };
        let mut glyphs = state.glyphs.borrow_mut();
        glyphs.retain(|(row, col), _| *row < self.snapshot.rows && *col < self.snapshot.cols);

        renderer.with_layer(bounds, |renderer| {
            renderer.fill_quad(quad(bounds), self.theme.inset);
            for cell in &self.snapshot.cells {
                if cell.point.row >= self.snapshot.rows || cell.point.col >= self.snapshot.cols {
                    continue;
                }
                let x = bounds.x + PAD_X + cell.point.col as f32 * CELL_W;
                let y = bounds.y + PAD_Y + cell.point.row as f32 * CELL_H;
                let render_width = if cell.wide { CELL_W * 2.0 } else { CELL_W };
                let cell_bounds = Rectangle::new(Point::new(x, y), Size::new(render_width, CELL_H));
                let (mut foreground, mut background) = cell_colors(cell, self.theme);
                if selected(&self.snapshot, cell.point) {
                    background = self.theme.sel;
                }
                let is_cursor = state.focused.get()
                    && self.snapshot.cursor.visible
                    && self.snapshot.cursor.point == cell.point;
                if is_cursor {
                    background = self.theme.accent;
                    foreground = self.theme.accent_on;
                }
                if background != self.theme.inset {
                    renderer.fill_quad(quad(cell_bounds), background);
                }
                if let Some((paragraph, _reused)) = cached_glyph(&mut glyphs, cell) {
                    renderer.fill_paragraph(paragraph, Point::new(x, y), foreground, bounds);
                    if cell.underline {
                        renderer.fill_quad(
                            quad(Rectangle::new(
                                Point::new(x, y + CELL_H - 2.0),
                                Size::new(render_width, 1.0),
                            )),
                            foreground,
                        );
                    }
                    if cell.strikeout {
                        renderer.fill_quad(
                            quad(Rectangle::new(
                                Point::new(x, y + CELL_H * 0.55),
                                Size::new(render_width, 1.0),
                            )),
                            foreground,
                        );
                    }
                }
            }
        });
    }
}

impl<'a, M: 'a> From<TerminalView<'a, M>> for Element<'a, M, iced::Theme, iced::Renderer> {
    fn from(widget: TerminalView<'a, M>) -> Self {
        Element::new(widget)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{
        GlyphCache, TerminalView, TerminalWidgetState, build_glyph, cached_glyph,
        clipboard_shortcut, glyph_key, key_bytes, needs_resize, paste_bytes,
    };
    use iced::Rectangle;
    use iced::Size;
    use iced::advanced::text::Paragraph as _;
    use iced::keyboard::{Key, Modifiers, key::Named};
    use lume_terminal::{TerminalCell, TerminalColor, TerminalPoint, TerminalSize};
    use std::cell::Cell;

    #[test]
    fn terminal_size_derives_from_real_bounds() {
        let size = TerminalView::<()>::terminal_size(Rectangle::new(
            iced::Point::ORIGIN,
            iced::Size::new(768.0, 188.0),
        ));
        assert_eq!((size.rows, size.cols), (9, 100));
    }

    #[test]
    fn shift_con_focus_terminale_non_rimbalza_al_listener_globale() {
        use crate::tools::terminal::Event as TerminalEvent;
        use iced::advanced::{Layout, Shell, layout, widget::Tree};
        use iced::keyboard::{
            Event as KeyboardEvent, Location,
            key::{Code, Physical},
        };
        use lume_terminal::{TerminalCursor, TerminalSnapshot};
        use lume_theme::LumeTheme;

        let theme = LumeTheme::default();
        let snapshot = TerminalSnapshot {
            rows: 1,
            cols: 1,
            cells: Vec::new(),
            cursor: TerminalCursor {
                point: TerminalPoint::new(0, 0),
                visible: true,
            },
            selection: None,
            display_offset: 0,
            alternate_screen: false,
            application_cursor: false,
            bracketed_paste: false,
        };
        let mut view: iced::Element<'_, TerminalEvent, iced::Theme, iced::Renderer> =
            TerminalView::new(std::sync::Arc::new(snapshot), None, &theme, |event| event).into();
        let mut tree = Tree::new(&view);
        tree.state
            .downcast_mut::<TerminalWidgetState>()
            .focused
            .set(true);
        let renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
            iced::Font::default(),
            iced::Pixels(16.0),
        ));
        let limits = layout::Limits::new(iced::Size::ZERO, iced::Size::new(80.0, 24.0));
        let node = view.as_widget_mut().layout(&mut tree, &renderer, &limits);
        let event = iced::Event::Keyboard(KeyboardEvent::KeyPressed {
            key: Key::Named(Named::Shift),
            modified_key: Key::Named(Named::Shift),
            physical_key: Physical::Code(Code::ShiftLeft),
            location: Location::Left,
            modifiers: Modifiers::SHIFT,
            text: None,
            repeat: false,
        });
        let mut published = Vec::new();
        let mut shell = Shell::new(&mut published);
        view.as_widget_mut().update(
            &mut tree,
            &event,
            Layout::new(&node),
            iced::mouse::Cursor::Unavailable,
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut shell,
            &iced::Rectangle::new(iced::Point::ORIGIN, iced::Size::new(80.0, 24.0)),
        );

        assert!(
            shell.is_event_captured(),
            "Shift deve restare al terminale focalizzato"
        );
        assert!(
            published
                .iter()
                .all(|event| !matches!(event, TerminalEvent::Input(_))),
            "Shift non invia byte alla shell"
        );
    }

    #[test]
    fn key_encoder_handles_vt_sequences_and_control_bytes() {
        assert_eq!(
            key_bytes(
                &Key::Named(Named::ArrowUp),
                Modifiers::default(),
                None,
                false
            ),
            Some(b"\x1b[A".to_vec())
        );
        let control = Modifiers::CTRL;
        assert_eq!(
            key_bytes(&Key::Character("c".into()), control, Some("c"), false),
            Some(vec![3])
        );
        assert_eq!(
            key_bytes(&Key::Named(Named::ArrowUp), Modifiers::CTRL, None, false,),
            Some(b"\x1b[1;5A".to_vec())
        );
        assert_eq!(
            key_bytes(
                &Key::Named(Named::ArrowLeft),
                Modifiers::ALT | Modifiers::SHIFT,
                None,
                true,
            ),
            Some(b"\x1b[1;4D".to_vec())
        );
        #[cfg(not(target_os = "macos"))]
        assert!(clipboard_shortcut(Modifiers::CTRL | Modifiers::SHIFT));
        #[cfg(target_os = "macos")]
        assert!(clipboard_shortcut(Modifiers::LOGO));
    }

    #[test]
    fn application_cursor_and_bracketed_paste_follow_terminal_modes() {
        assert_eq!(
            key_bytes(
                &Key::Named(Named::ArrowUp),
                Modifiers::default(),
                None,
                true
            ),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            paste_bytes("test\x1b[201~payload".into(), true),
            b"\x1b[200~testpayload\x1b[201~".to_vec()
        );
    }

    #[test]
    fn resize_is_compared_with_the_active_snapshot() {
        let size = lume_terminal::TerminalSize::new(24, 80);
        assert!(!needs_resize(24, 80, size));
        assert!(needs_resize(40, 120, size));
    }

    fn benchmark_cells(rows: usize, cols: usize) -> Vec<TerminalCell> {
        (0..rows * cols)
            .map(|index| {
                let character = match index % 13 {
                    0 => '界',
                    1 => 'e',
                    value => char::from(b'!' + value as u8),
                };
                TerminalCell {
                    point: TerminalPoint::new(index / cols, index % cols),
                    character,
                    zerowidth: (character == 'e')
                        .then_some(vec!['\u{301}'])
                        .unwrap_or_default(),
                    foreground: TerminalColor::Named(7),
                    background: TerminalColor::Named(0),
                    bold: index % 17 == 0,
                    italic: index % 19 == 0,
                    underline: false,
                    strikeout: false,
                    inverse: false,
                    hidden: false,
                    wide_spacer: false,
                    wide: character == '界',
                    hyperlink: None,
                }
            })
            .collect()
    }

    #[test]
    fn glyph_cache_reuses_unchanged_paragraphs() {
        let cell = benchmark_cells(1, 1).pop().unwrap();
        let mut cache = GlyphCache::default();
        assert!(!cached_glyph(&mut cache, &cell).unwrap().1);
        assert!(cached_glyph(&mut cache, &cell).unwrap().1);
        let mut changed = cell;
        changed.bold = !changed.bold;
        assert!(!cached_glyph(&mut cache, &changed).unwrap().1);
    }

    #[test]
    #[ignore = "release performance gate; run explicitly with --ignored"]
    fn terminal_render_cache_benchmark_120x40() {
        const ROWS: usize = 40;
        const COLS: usize = 120;
        const UNCACHED_FRAMES: usize = 5;
        const WARM_FRAMES: usize = 240;
        const FRAME_BUDGET_NS: u128 = 16_667_000;
        let cells = benchmark_cells(ROWS, COLS);

        let uncached_start = std::time::Instant::now();
        for _ in 0..UNCACHED_FRAMES {
            for cell in &cells {
                let key = glyph_key(cell).unwrap();
                let paragraph = build_glyph(&key);
                std::hint::black_box(paragraph.min_bounds().width);
            }
        }
        let uncached_ns = uncached_start.elapsed().as_nanos() / UNCACHED_FRAMES as u128;

        let mut cache = GlyphCache::default();
        for cell in &cells {
            assert!(!cached_glyph(&mut cache, cell).unwrap().1);
        }
        let warm_start = std::time::Instant::now();
        let mut rebuilds = 0usize;
        for _ in 0..WARM_FRAMES {
            for cell in &cells {
                let (paragraph, reused) = cached_glyph(&mut cache, cell).unwrap();
                rebuilds += usize::from(!reused);
                std::hint::black_box(paragraph.min_bounds().width);
            }
        }
        let warm_ns = warm_start.elapsed().as_nanos() / WARM_FRAMES as u128;
        println!(
            "LUME_TERMINAL_BENCH os={} arch={} cpu={} corpus={}x{} cells={} uncached_ns={} warm_ns={} budget_ns={} rebuilds={}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown".into()),
            COLS,
            ROWS,
            cells.len(),
            uncached_ns,
            warm_ns,
            FRAME_BUDGET_NS,
            rebuilds,
        );
        assert_eq!(rebuilds, 0);
        assert!(warm_ns < FRAME_BUDGET_NS, "warm frame {warm_ns} ns");
        assert!(
            warm_ns < uncached_ns,
            "cache non più veloce: {warm_ns} >= {uncached_ns}"
        );
    }

    /// Chiede il resize contando quante volte è stato calcolato.
    fn chiedi(
        stato: &TerminalWidgetState,
        bounds: Size,
        righe: usize,
        colonne: usize,
        risultato: TerminalSize,
        calcoli: &Cell<u32>,
    ) -> Option<TerminalSize> {
        stato.resize_richiesto(bounds, righe, colonne, || {
            calcoli.set(calcoli.get() + 1);
            risultato
        })
    }

    #[test]
    fn il_primo_layout_chiede_il_resize() {
        let stato = TerminalWidgetState::default();
        let calcoli = Cell::new(0);
        let richiesta = chiedi(
            &stato,
            Size::new(768.0, 188.0),
            24,
            80,
            TerminalSize::new(9, 100),
            &calcoli,
        );
        assert_eq!(richiesta, Some(TerminalSize::new(9, 100)));
        assert_eq!(calcoli.get(), 1);
    }

    #[test]
    fn il_primo_layout_gia_allineato_non_pubblica() {
        let stato = TerminalWidgetState::default();
        let calcoli = Cell::new(0);
        let richiesta = chiedi(
            &stato,
            Size::new(768.0, 188.0),
            9,
            100,
            TerminalSize::new(9, 100),
            &calcoli,
        );
        assert_eq!(richiesta, None);
    }

    #[test]
    fn una_raffica_di_eventi_con_la_stessa_geometria_pubblica_una_volta_sola() {
        let stato = TerminalWidgetState::default();
        let calcoli = Cell::new(0);
        let bounds = Size::new(768.0, 188.0);
        let mut richieste = 0;
        // lo snapshot non è ancora stato aggiornato: 24x80 per tutti gli eventi
        for _ in 0..50 {
            if chiedi(&stato, bounds, 24, 80, TerminalSize::new(9, 100), &calcoli).is_some() {
                richieste += 1;
            }
        }
        assert_eq!(richieste, 1, "un solo Resize per una sola geometria");
        assert_eq!(
            calcoli.get(),
            1,
            "e la dimensione si calcola una volta sola"
        );
    }

    #[test]
    fn lo_snapshot_che_si_aggiorna_non_richiede_altro() {
        let stato = TerminalWidgetState::default();
        let calcoli = Cell::new(0);
        let bounds = Size::new(768.0, 188.0);
        assert!(chiedi(&stato, bounds, 24, 80, TerminalSize::new(9, 100), &calcoli).is_some());
        // il backend ha applicato il resize: lo snapshot è 9x100
        assert_eq!(
            chiedi(&stato, bounds, 9, 100, TerminalSize::new(9, 100), &calcoli),
            None
        );
        assert_eq!(
            chiedi(&stato, bounds, 9, 100, TerminalSize::new(9, 100), &calcoli),
            None
        );
    }

    #[test]
    fn una_finestra_ridimensionata_richiede_il_resize() {
        let stato = TerminalWidgetState::default();
        let calcoli = Cell::new(0);
        assert!(
            chiedi(
                &stato,
                Size::new(768.0, 188.0),
                24,
                80,
                TerminalSize::new(9, 100),
                &calcoli
            )
            .is_some()
        );
        assert_eq!(
            chiedi(
                &stato,
                Size::new(768.0, 188.0),
                9,
                100,
                TerminalSize::new(9, 100),
                &calcoli
            ),
            None
        );
        let richiesta = chiedi(
            &stato,
            Size::new(768.0, 300.0),
            9,
            100,
            TerminalSize::new(15, 100),
            &calcoli,
        );
        assert_eq!(richiesta, Some(TerminalSize::new(15, 100)));
    }

    #[test]
    fn un_altro_terminale_con_altre_dimensioni_si_riallinea() {
        let stato = TerminalWidgetState::default();
        let calcoli = Cell::new(0);
        let bounds = Size::new(768.0, 188.0);
        assert!(chiedi(&stato, bounds, 24, 80, TerminalSize::new(9, 100), &calcoli).is_some());
        assert_eq!(
            chiedi(&stato, bounds, 9, 100, TerminalSize::new(9, 100), &calcoli),
            None
        );
        // si passa a un secondo terminale appena creato: 24x80, stessi bounds
        let richiesta = chiedi(&stato, bounds, 24, 80, TerminalSize::new(9, 100), &calcoli);
        assert_eq!(richiesta, Some(TerminalSize::new(9, 100)));
    }

    #[test]
    fn bounds_ostili_non_mandano_in_panic_e_danno_almeno_una_cella() {
        let ostili = [
            Size::new(f32::NAN, f32::NAN),
            Size::new(f32::INFINITY, f32::INFINITY),
            Size::new(-50.0, -50.0),
            Size::new(0.0, 0.0),
            Size::new(1.0e9, 1.0e9),
        ];
        for grandezza in ostili {
            let size =
                TerminalView::<()>::terminal_size(Rectangle::new(iced::Point::ORIGIN, grandezza));
            assert!(
                size.rows >= 1 && size.cols >= 1,
                "{grandezza:?} -> {size:?}"
            );
            let stato = TerminalWidgetState::default();
            let calcoli = Cell::new(0);
            // due controlli di fila: nessun panic, e mai più di una richiesta
            // quando lo snapshot resta uguale e i bounds sono validi
            let _ = chiedi(&stato, grandezza, 24, 80, size, &calcoli);
            let _ = chiedi(&stato, grandezza, 24, 80, size, &calcoli);
        }
    }
}
