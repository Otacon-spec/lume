//! Widget `iced::advanced::Widget` dell'editor di codice: gutter, testo
//! con highlighting, selezione, caret e scroll virtualizzato.
//!
//! Il widget è in sola lettura rispetto al `Document`: ogni interazione
//! viene tradotta in un `EditorCommand` pubblicato sulla `Shell`.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::text::{
    Alignment as TextAlignment, Hit, LineHeight, Paragraph as _, Renderer as _, Shaping, Span,
    Text, Wrapping,
};
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, InputMethod, Shell, clipboard, input_method};
use iced::keyboard::{self, Key, key::Named};
use iced::mouse::{self, ScrollDelta};
use iced::{Element, Event, Length, Pixels, Point, Rectangle, Size};
use lume_core::Document;
use lume_syntax::{HighlightSnapshot, SyntaxState, TokenClass};
use lume_theme::LumeTheme;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

use super::commands::{EditorCommand, Motion, PageDirection};
use super::folding::{FoldProjectionRef, is_foldable_start};
use super::overlay::{
    EditorOverlay, MarkSeverity, dot_rect, line_has_note, marks_on_line, note_bar_rect,
    underline_rect, worst_severity,
};
use super::popup::{
    BOX_PAD_X, BOX_PAD_Y, CELL_W, DETAIL_MAX_COLS, LABEL_MAX_COLS, OverlayKeys, POPUP_VISIBLE,
    card_size, clip_text, hover_change, overlay_key, place_box, popup_size, window_top,
    word_start_in_line,
};
use super::state::{CachedLine, EditorState, HighlightWindow, LineParagraph, remap_cache};
use super::token_color;
use super::viewport::ViewportMetrics;

/// Dimensione del font del codice.
pub const FONT_SIZE: f32 = 12.5;
/// Altezza riga: 12.5 * 1.65 (~20.6 px), fedele al prototipo.
pub const LINE_H: f32 = FONT_SIZE * 1.65;
/// Padding verticale del contenuto (sopra e sotto).
const PAD_V: f32 = 10.0;
/// Larghezza della colonna gutter (numeri di riga).
const GUTTER_W: f32 = 42.0;
/// Colonna marker del prototipo (16px) tra gutter e testo: oggi vuota,
/// ospiterà i marker (Run, breakpoint) quando avranno un backend.
pub const MARKER_W: f32 = 16.0;
/// x di partenza del testo rispetto al bordo sinistro dell'editor.
const TEXT_X0: f32 = GUTTER_W + MARKER_W;

pub(crate) fn in_marker_column(x_rel: f32) -> bool {
    (GUTTER_W..TEXT_X0).contains(&x_rel)
}
/// Range normalizzati che intersecano una finestra di caratteri, senza
/// allocazioni né scansione del prefisso fuori viewport.
fn selections_in(ranges: &[lume_core::Range], from: usize, to: usize) -> &[lume_core::Range] {
    let start = ranges.partition_point(|range| range.max() < from);
    let tail = ranges.get(start..).unwrap_or_default();
    let end = tail.partition_point(|range| range.min() <= to);
    tail.get(..end).unwrap_or_default()
}

/// Padding a destra dei numeri di riga (right-aligned).
const GUTTER_PAD_R: f32 = 14.0;
fn display_columns(characters: impl Iterator<Item = char>) -> usize {
    characters.fold(0, |column, character| {
        if character == '\t' {
            column + (4 - column % 4)
        } else {
            column + UnicodeWidthChar::width(character).unwrap_or(0)
        }
    })
}

fn line_display_columns(text: &str) -> usize {
    display_columns(text.chars())
}

fn grapheme_offset_at_byte(text: &str, byte: usize) -> usize {
    let mut boundary = byte.min(text.len());
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text[..boundary].graphemes(true).count()
}

/// I range che toccano `line_start..line_end`, a partire da `from_idx`
/// (i range sono ordinati per inizio: non si torna mai indietro). Restituisce
/// anche l'indice da cui ripartire per la riga successiva — il primo range
/// che potrebbe ancora toccarla, cioè il primo con `end > line_end` non
/// ancora consumato... più semplicemente: il primo il cui **inizio** non è
/// ancora superato dalla fine di questa riga.
fn ranges_touching_line(
    ranges: &[std::ops::Range<usize>],
    from_idx: usize,
    line_start: usize,
    line_end: usize,
) -> (Vec<std::ops::Range<usize>>, usize) {
    let mut trovati = Vec::new();
    let mut i = from_idx;
    let mut prossimo = from_idx;
    while let Some(range) = ranges.get(i) {
        if range.start >= line_end {
            break;
        }
        if range.end > line_start {
            trovati.push(range.clone());
        }
        // avanza `prossimo` solo oltre i range che questa riga esaurisce
        // (end <= line_end): un range che continua sulla riga dopo deve
        // restare visitabile anche lì.
        if range.end <= line_end {
            prossimo = i + 1;
        }
        i += 1;
    }
    (trovati, prossimo)
}

/// Righe extra renderizzate sopra/sotto il viewport.
const OVERDRAW: usize = 2;
/// Righe oltre il visibile mantenute in cache prima dell'eviction.
const CACHE_KEEP: usize = 100;
/// Margine oltre il visibile che deve già avere i token di sintassi: la
/// richiesta parte prima che lo scroll esca dalla finestra colorata, così
/// le righe nuove non vengono prima preparate senza colori e poi rifatte.
const SYNTAX_PREFETCH: usize = CACHE_KEEP / 2;

/// Vista editor: disegna un `Document` e pubblica `EditorCommand`.
/// Un testo monoriga nel font del codice, per popup e scheda.
fn mono_text(content: String, width: f32) -> Text<String> {
    Text {
        content,
        bounds: Size::new(width, LINE_H),
        size: Pixels(FONT_SIZE),
        line_height: LineHeight::Absolute(Pixels(LINE_H)),
        font: crate::MONO,
        align_x: TextAlignment::Left,
        align_y: iced::alignment::Vertical::Top,
        // Advanced: la documentazione può contenere di tutto
        shaping: Shaping::Advanced,
        wrapping: Wrapping::None,
    }
}

type SyntaxViewportCallback<'a, M> = Box<dyn Fn(u64, std::ops::Range<usize>) -> M + 'a>;

/// Una riga del margine: ordinata, riga del cursore, segno peggiore,
/// appunto collegato, freccia di folding (`Some(collassato)` se il blocco inizia qui).
type RigaMargine = (f32, bool, Option<MarkSeverity>, bool, Option<bool>);

pub struct EditorView<'a, M> {
    doc: &'a Document,
    focus_request: Option<(u64, bool)>,
    syntax: Option<&'a SyntaxState>,
    snapshot: Option<&'a HighlightSnapshot>,
    on_syntax_viewport: Option<SyntaxViewportCallback<'a, M>>,
    syntax_context: Option<(
        u64,
        u64,
        lume_syntax::SyntaxReceiver,
        lume_syntax::LanguageId,
    )>,
    theme: &'a LumeTheme,
    highlights: &'a [std::ops::Range<usize>],
    overlay: EditorOverlay<'a>,
    on_command: Box<dyn Fn(EditorCommand) -> M + 'a>,
}

impl<'a, M> EditorView<'a, M> {
    pub fn new(
        doc: &'a Document,
        syntax: Option<&'a SyntaxState>,
        theme: &'a LumeTheme,
        highlights: &'a [std::ops::Range<usize>],
        on_command: impl Fn(EditorCommand) -> M + 'a,
    ) -> Self {
        Self {
            doc,
            focus_request: None,
            syntax,
            snapshot: None,
            on_syntax_viewport: None,
            syntax_context: None,
            theme,
            highlights,
            overlay: EditorOverlay::default(),
            on_command: Box::new(on_command),
        }
    }

    /// Percorso asincrono: legge token già preparati; nessun parser nel draw.
    /// Il chiamante verifica DocId ed epoca radice prima della consegna.
    pub fn from_snapshot(
        doc: &'a Document,
        snapshot: Option<&'a HighlightSnapshot>,
        theme: &'a LumeTheme,
        highlights: &'a [std::ops::Range<usize>],
        on_command: impl Fn(EditorCommand) -> M + 'a,
    ) -> Self {
        let mut view = Self::new(doc, None, theme, highlights, on_command);
        view.snapshot = snapshot;
        view
    }

    /// Il contesto invalida richieste deduplicate oltre la vita del loro owner.
    pub fn syntax_context(
        mut self,
        epoch: u64,
        owner: lume_syntax::SyntaxReceiver,
        language: lume_syntax::LanguageId,
    ) -> Self {
        self.syntax_context = Some((epoch, owner.revocation(), owner, language));
        self
    }

    /// Callback a evento; il chiamante associa documento/radice al messaggio.
    pub fn on_syntax_viewport(
        mut self,
        callback: impl Fn(u64, std::ops::Range<usize>) -> M + 'a,
    ) -> Self {
        self.on_syntax_viewport = Some(Box::new(callback));
        self
    }

    fn fold_projection(&self) -> FoldProjectionRef<'a> {
        FoldProjectionRef::new(self.overlay.fold_spans)
    }

    fn folding_regions(&self) -> &[lume_syntax::FoldRegion] {
        self.snapshot.map_or(&[], HighlightSnapshot::folding)
    }

    fn line_is_foldable(&self, doc_line: usize) -> bool {
        is_foldable_start(self.folding_regions(), doc_line)
    }

    fn request_syntax_viewport(
        &self,
        state: &EditorState,
        bounds: Rectangle,
        shell: &mut Shell<'_, M>,
    ) {
        let Some(callback) = &self.on_syntax_viewport else {
            return;
        };
        if state.syntax_context.borrow().as_ref() != self.syntax_context.as_ref() {
            state.syntax_requested.borrow_mut().take();
            *state.syntax_context.borrow_mut() = self.syntax_context.clone();
        }
        if bounds.height <= 0.0 {
            return;
        }
        let rope = self.doc.rope();
        let count = rope.len_lines();
        let proj = self.fold_projection();
        let vis_count = proj.visible_line_count(count).max(1);
        let scroll = state.scroll_y.get();
        let first_vis = (((scroll - PAD_V) / LINE_H).floor() as isize - OVERDRAW as isize)
            .clamp(0, vis_count as isize - 1) as usize;
        let last_vis = ((((scroll - PAD_V) + bounds.height) / LINE_H).ceil() as isize
            + OVERDRAW as isize)
            .clamp(first_vis as isize, vis_count as isize - 1) as usize;
        let first = proj.visual_to_doc(first_vis, count);
        let last = proj.visual_to_doc(last_vis, count).max(first);
        let end = |line: usize| {
            if line + 1 < count {
                rope.line_to_byte(line + 1)
            } else {
                rope.len_bytes()
            }
        };
        let visible = rope.line_to_byte(first.saturating_sub(SYNTAX_PREFETCH))
            ..end(last.saturating_add(SYNTAX_PREFETCH).min(count - 1));
        let version = self.doc.version();
        if self
            .snapshot
            .is_some_and(|s| s.covers(s.language(), version, visible.clone()))
        {
            return;
        }
        let document = self.doc.id();
        if state
            .syntax_requested
            .borrow()
            .as_ref()
            .is_some_and(|(id, revision, range)| {
                *id == document
                    && *revision == version
                    && range.start <= visible.start
                    && visible.end <= range.end
            })
        {
            return;
        }
        let covered = rope.line_to_byte(first.saturating_sub(CACHE_KEEP))
            ..end(last.saturating_add(CACHE_KEEP).min(count - 1));
        *state.syntax_requested.borrow_mut() = Some((document, version, covered.clone()));
        shell.publish(callback(version, covered));
    }

    /// Un gesto app restituisce il focus una sola volta; non lo ruba
    /// durante la digitazione nei campi o l'apertura di un overlay.
    pub fn focus_request(mut self, epoch: u64, focused: bool) -> Self {
        self.focus_request = Some((epoch, focused));
        self
    }

    pub fn overlay(mut self, overlay: EditorOverlay<'a>) -> Self {
        self.overlay = overlay;
        self
    }

    /// Scroll massimo per un viewport alto `height`.
    fn text_x0(&self) -> f32 {
        TEXT_X0 + self.overlay.blame.map_or(0.0, |_| 136.0)
    }

    fn max_scroll(&self, height: f32) -> f32 {
        let count = self.doc.rope().len_lines();
        let n = self.fold_projection().visible_line_count(count) as f32;
        (n * LINE_H + 2.0 * PAD_V - height).max(0.0)
    }

    /// Testo della riga `line` senza il newline finale.
    fn line_text(&self, line: usize) -> String {
        let rope = self.doc.rope();
        let slice = rope.line(line);
        let mut n = slice.len_chars();
        if n > 0 && slice.char(n - 1) == '\n' {
            n -= 1;
        }
        slice.slice(..n).to_string()
    }

    /// Angolo in alto a sinistra della riga che contiene il char `index`,
    /// con la x del char: dove ancorare popup e scheda. `None` se la riga
    /// è scorsa fuori dall'area.
    fn anchor_point(&self, state: &EditorState, bounds: Rectangle, index: usize) -> Option<Point> {
        let rope = self.doc.rope();
        let index = index.min(rope.len_chars());
        let line = rope.char_to_line(index);
        let proj = self.fold_projection();
        if proj.is_line_hidden(line) {
            return None;
        }
        let vis_line = proj.doc_to_visual(line);
        let col = index - rope.line_to_char(line);
        let y = bounds.y + PAD_V + vis_line as f32 * LINE_H - state.scroll_y.get();
        if y + LINE_H < bounds.y || y > bounds.y + bounds.height {
            return None;
        }
        let x = bounds.x + self.text_x0() - state.scroll_x.get()
            + self.x_at_col_cached(state, line, col);
        Some(Point::new(x, y))
    }

    /// Il rettangolo del popup, se c'è e se la sua riga è visibile.
    fn popup_rect(&self, state: &EditorState, bounds: Rectangle) -> Option<Rectangle> {
        let popup = self.overlay.popup?;
        let anchor = self.anchor_point(state, bounds, popup.anchor)?;
        Some(place_box(
            anchor,
            LINE_H,
            popup_size(&popup.rows, POPUP_VISIBLE),
            bounds,
            false,
        ))
    }

    /// Il rettangolo della scheda hover: di preferenza sopra la parola.
    fn card_rect(&self, state: &EditorState, bounds: Rectangle) -> Option<Rectangle> {
        let card = self.overlay.hover?;
        let anchor = self.anchor_point(state, bounds, card.anchor)?;
        Some(place_box(
            anchor,
            LINE_H,
            card_size(&card.lines),
            bounds,
            true,
        ))
    }

    /// La parola sotto il mouse, come char assoluto del suo inizio. `None`
    /// fuori dal testo, sul gutter, sulla scheda stessa o su un non-parola.
    /// ATTENZIONE: Il gesto è puntare **sopra** una lettera: `hit_test` di cosmic-text
    /// dà il confine più vicino, e sulla metà destra dell'ultima lettera di
    /// una parola cade sullo spazio dopo.
    fn hover_target(&self, state: &EditorState, bounds: Rectangle, at: Point) -> Option<usize> {
        if !bounds.contains(at) || at.x < bounds.x + self.text_x0() {
            return None;
        }
        if self
            .card_rect(state, bounds)
            .is_some_and(|r| r.contains(at))
            || self
                .popup_rect(state, bounds)
                .is_some_and(|r| r.contains(at))
        {
            // il mouse sulla scheda la tiene aperta: `None` qui la chiuderebbe
            return state.hover_word.get();
        }
        let pos = self.resolve_position(state, bounds, at);
        let rope = self.doc.rope();
        let line = rope.char_to_line(pos.min(rope.len_chars()));
        let line_start = rope.line_to_char(line);
        let inizio = word_start_in_line(&self.line_text(line), pos - line_start)?;
        Some(line_start + inizio)
    }

    fn draw_popup(&self, state: &EditorState, renderer: &mut iced::Renderer, bounds: Rectangle) {
        let (Some(popup), Some(rect)) = (self.overlay.popup, self.popup_rect(state, bounds)) else {
            return;
        };
        let top = window_top(
            state.popup_top.get(),
            popup.selected,
            popup.rows.len(),
            POPUP_VISIBLE,
        );
        state.popup_top.set(top);
        renderer.with_layer(rect, |renderer| {
            renderer.fill_quad(
                Quad {
                    bounds: rect,
                    border: iced::Border {
                        color: self.theme.border2,
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Quad::default()
                },
                self.theme.surface,
            );
            for (offset, row) in popup.rows.iter().skip(top).take(POPUP_VISIBLE).enumerate() {
                let y = rect.y + BOX_PAD_Y + offset as f32 * LINE_H;
                if top + offset == popup.selected {
                    renderer.fill_quad(
                        Quad {
                            bounds: Rectangle {
                                x: rect.x + 1.0,
                                y,
                                width: rect.width - 2.0,
                                height: LINE_H,
                            },
                            ..Quad::default()
                        },
                        self.theme.accent_dim,
                    );
                }
                let label = clip_text(&row.label, LABEL_MAX_COLS);
                let label_cols = label.chars().count();
                renderer.fill_text(
                    mono_text(label, rect.width),
                    Point::new(rect.x + BOX_PAD_X, y),
                    self.theme.text,
                    rect,
                );
                if let Some(detail) = &row.detail {
                    renderer.fill_text(
                        mono_text(clip_text(detail, DETAIL_MAX_COLS), rect.width),
                        Point::new(rect.x + BOX_PAD_X + (label_cols + 2) as f32 * CELL_W, y),
                        self.theme.muted,
                        rect,
                    );
                }
            }
        });
    }

    fn draw_card(&self, state: &EditorState, renderer: &mut iced::Renderer, bounds: Rectangle) {
        let (Some(card), Some(rect)) = (self.overlay.hover, self.card_rect(state, bounds)) else {
            return;
        };
        renderer.with_layer(rect, |renderer| {
            renderer.fill_quad(
                Quad {
                    bounds: rect,
                    border: iced::Border {
                        color: self.theme.border2,
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Quad::default()
                },
                self.theme.surface,
            );
            for (index, line) in card.lines.iter().enumerate() {
                let y = rect.y + BOX_PAD_Y + index as f32 * LINE_H;
                if line.code {
                    renderer.fill_quad(
                        Quad {
                            bounds: Rectangle {
                                x: rect.x + 1.0,
                                y,
                                width: rect.width - 2.0,
                                height: LINE_H,
                            },
                            ..Quad::default()
                        },
                        self.theme.inset,
                    );
                }
                renderer.fill_text(
                    mono_text(
                        clip_text(&line.text, super::popup::CARD_MAX_COLS),
                        rect.width,
                    ),
                    Point::new(rect.x + BOX_PAD_X, y),
                    if line.code {
                        self.theme.text
                    } else {
                        self.theme.sec
                    },
                    rect,
                );
            }
        });
    }

    /// Precalcola le capture tree-sitter con una sola query per una finestra
    /// più ampia del viewport. Scroll e righe vicine riusano gli stessi token.
    fn prepare_highlights(&self, state: &EditorState, first_line: usize, last_line: usize) {
        if self.snapshot.is_some() {
            state.highlight_window.borrow_mut().take();
            return;
        }
        let Some(syntax) = self.syntax else {
            state.highlight_window.borrow_mut().take();
            return;
        };
        let rope = self.doc.rope();
        let line_count = rope.len_lines();
        if line_count == 0 {
            state.highlight_window.borrow_mut().take();
            return;
        }
        let first_line = first_line.min(line_count - 1);
        let last_line = last_line.max(first_line).min(line_count - 1);
        let document = self.doc.id();
        let version = self.doc.version();
        let language = syntax.lang();
        if state
            .highlight_window
            .borrow()
            .as_ref()
            .is_some_and(|window| window.covers(document, version, language, first_line, last_line))
        {
            return;
        }

        let window_first = first_line.saturating_sub(CACHE_KEEP);
        let window_last = last_line.saturating_add(CACHE_KEEP).min(line_count - 1);
        let start_byte = rope.line_to_byte(window_first);
        let end_byte = if window_last + 1 < line_count {
            rope.line_to_byte(window_last + 1)
        } else {
            rope.len_bytes()
        };
        let tokens = syntax.highlights(rope.slice(..), start_byte..end_byte);
        *state.highlight_window.borrow_mut() = Some(HighlightWindow {
            document,
            version,
            language,
            first_line: window_first,
            last_line: window_last,
            tokens,
        });
        #[cfg(test)]
        state
            .highlight_queries
            .set(state.highlight_queries.get().saturating_add(1));
    }

    /// Token della riga (byte relativi, clampati al testo, non sovrapposti)
    /// dalla query batch del viewport. Vuoto senza sintassi o riga vuota.
    #[allow(clippy::indexing_slicing)] // first_token <= tokens.len() per definizione di partition_point
    fn line_tokens(
        &self,
        state: &EditorState,
        line: usize,
        text: &str,
    ) -> Vec<(std::ops::Range<usize>, TokenClass)> {
        if (self.syntax.is_none() && self.snapshot.is_none()) || text.is_empty() {
            return Vec::new();
        }
        self.prepare_highlights(state, line, line);
        let rope = self.doc.rope();
        let start_b = rope.line_to_byte(line);
        let end_b = start_b + text.len();

        let mut out = Vec::new();
        let mut cur = 0usize; // byte relativo alla riga, già emesso
        let highlights = state.highlight_window.borrow();
        let tokens = if let Some(snapshot) = self.snapshot {
            if snapshot.covers(snapshot.language(), self.doc.version(), start_b..end_b) {
                snapshot.tokens()
            } else {
                &[]
            }
        } else {
            highlights
                .as_ref()
                .map(|window| window.tokens.as_slice())
                .unwrap_or_default()
        };
        let first_token = tokens.partition_point(|(range, _)| range.end <= start_b);
        for (range, class) in tokens[first_token..]
            .iter()
            .take_while(|(range, _)| range.start < end_b)
        {
            let s = range.start.saturating_sub(start_b).min(text.len()).max(cur);
            let e = range.end.saturating_sub(start_b).min(text.len());
            if e <= s {
                continue;
            }
            out.push((s..e, *class));
            cur = e;
        }
        out
    }

    /// Spans colorati della riga dai suoi token; il testo non coperto usa il
    /// colore UI di default del tema.
    #[allow(clippy::indexing_slicing)] // i range vengono da line_tokens: clampati e ordinati
    fn spans_from_tokens<'t>(
        &self,
        text: &'t str,
        tokens: &[(std::ops::Range<usize>, TokenClass)],
    ) -> Vec<Span<'t, (), iced::Font>> {
        if text.is_empty() {
            return Vec::new();
        }
        let mut spans: Vec<Span<'t, (), iced::Font>> = Vec::new();
        let mut cur = 0usize;
        for (range, class) in tokens {
            if range.start > cur {
                spans.push(Span::new(&text[cur..range.start]));
            }
            spans.push(Span::new(&text[range.clone()]).color(token_color(self.theme, *class)));
            cur = range.end;
        }
        if cur < text.len() {
            spans.push(Span::new(&text[cur..]));
        }
        spans
    }

    /// Spans colorati della riga (query batch del viewport).
    #[cfg(test)]
    fn line_spans<'t>(
        &self,
        state: &EditorState,
        line: usize,
        text: &'t str,
    ) -> Vec<Span<'t, (), iced::Font>> {
        let tokens = self.line_tokens(state, line, text);
        self.spans_from_tokens(text, &tokens)
    }

    /// Costruisce l'entry di cache della riga: paragraph (monospace, no
    /// wrapping), testo e token incorporati.
    fn build_line(&self, state: &EditorState, line: usize) -> CachedLine {
        #[cfg(feature = "performance-diagnostics")]
        let _phase = crate::performance_diagnostics::Span::new("editor.build_line");
        let text = self.line_text(line);
        let tokens = self.line_tokens(state, line, &text);
        let spans = self.spans_from_tokens(&text, &tokens);
        let paragraph = LineParagraph::with_spans(Text {
            content: spans.as_slice(),
            bounds: Size::new(f32::INFINITY, LINE_H),
            size: Pixels(FONT_SIZE),
            line_height: LineHeight::Absolute(Pixels(LINE_H)),
            font: crate::MONO,
            align_x: TextAlignment::Left,
            align_y: iced::alignment::Vertical::Top,
            shaping: Shaping::Advanced,
            wrapping: Wrapping::None,
        });
        #[cfg(test)]
        state
            .paragraph_builds
            .set(state.paragraph_builds.get().saturating_add(1));
        CachedLine {
            paragraph,
            text,
            tokens,
            epoch: state.cache_epoch.get(),
            verified_version: self.doc.version(),
            tokens_verified: true,
        }
    }

    /// Accede all'entry in cache della riga (costruendola se serve) e applica
    /// `f` a paragraph e testo. Un'entry promossa da `remap_cache` viene
    /// verificata una volta per versione: se i token della riga sono cambiati
    /// (es. un «/*» aperto più sopra) viene ricostruita.
    fn with_line<R>(
        &self,
        state: &EditorState,
        line: usize,
        f: impl FnOnce(&LineParagraph, &str) -> R,
    ) -> R {
        let epoch = state.cache_epoch.get();
        let version = self.doc.version();
        let mut cache = state.cache.borrow_mut();
        let entry = cache
            .entry(line)
            .or_insert_with(|| self.build_line(state, line));
        if entry.epoch != epoch {
            *entry = self.build_line(state, line);
        } else if entry.verified_version != version || !entry.tokens_verified {
            let tokens = self.line_tokens(state, line, &entry.text);
            if tokens != entry.tokens {
                *entry = self.build_line(state, line);
            } else {
                entry.verified_version = version;
                entry.tokens_verified = true;
            }
        }
        f(&entry.paragraph, &entry.text)
    }

    /// Accede al paragraph in cache (costruendolo se serve) e vi applica `f`.
    fn with_paragraph<R>(
        &self,
        state: &EditorState,
        line: usize,
        f: impl FnOnce(&LineParagraph) -> R,
    ) -> R {
        self.with_line(state, line, |paragraph, _| f(paragraph))
    }

    /// Sincronizza lo stato riusato da iced con documento, versione e tema.
    /// DocId evita che due tab nuove (entrambe versione zero) condividano testo.
    fn sync(&self, state: &EditorState, bounds: Rectangle) {
        if let Some(request) = self.focus_request
            && state.last_focus_request.get() != Some(request)
        {
            state.last_focus_request.set(Some(request));
            state.is_focused.set(request.1);
            if !request.1 {
                state.ime.borrow_mut().close();
            }
        }
        let document = self.doc.id();
        let version = self.doc.version();
        let head = self.doc.selection().head;
        let document_changed = state.last_document.get() != Some(document);
        let version_changed = state.last_version.get() != version;
        let content_changed = document_changed || version_changed;
        let theme_changed = state.last_theme.borrow().as_deref() != Some(self.theme.id.as_str());
        let syntax = self
            .snapshot
            .map(HighlightSnapshot::language)
            .or_else(|| self.syntax.map(SyntaxState::lang))
            .or_else(|| {
                self.syntax_context
                    .as_ref()
                    .map(|(_, _, _, language)| *language)
            });
        let projection_changed = match (state.last_snapshot.borrow().as_ref(), self.snapshot) {
            (None, None) => false,
            (Some(old), Some(new)) => !old.same_projection(new),
            _ => true,
        };
        let fold_changed = state.last_fold_spans.borrow().as_slice() != self.overlay.fold_spans;
        let syntax_changed = state.last_syntax.get() != syntax || projection_changed;
        let cursor_moved = state.last_cursor.get() != head;

        if document_changed {
            state.scroll_x.set(0.0);
            state.scroll_y.set(0.0);
            state.content_width.set(0.0);
            state.ime.borrow_mut().close();
            state.ime_cache.borrow_mut().take();
            state.gutter_cache.borrow_mut().clear();
        }
        if content_changed || syntax_changed {
            state.highlight_window.borrow_mut().take();
        }
        if content_changed || theme_changed || syntax_changed {
            let new_epoch = state.cache_epoch.get().wrapping_add(1);
            state.cache_epoch.set(new_epoch);
            // Un solo edit, stesso documento/tema/linguaggio: rimappa le righe.
            // with_line verifica i token della nuova revisione prima del riuso.
            let single_edit = content_changed
                && !document_changed
                && !theme_changed
                && state.last_syntax.get() == syntax
                && version == state.last_version.get().wrapping_add(1);
            let projection_only = projection_changed
                && !content_changed
                && !theme_changed
                && state.last_syntax.get() == syntax;
            match (single_edit, self.doc.last_edit()) {
                (true, Some(edit)) => remap_cache(
                    &mut state.cache.borrow_mut(),
                    edit.start_point.0,
                    edit.old_end_point.0,
                    edit.new_end_point.0 as isize - edit.old_end_point.0 as isize,
                    new_epoch,
                ),
                _ if projection_only => {
                    for entry in state.cache.borrow_mut().values_mut() {
                        entry.epoch = new_epoch;
                        entry.tokens_verified = false;
                    }
                }
                _ => state.cache.borrow_mut().clear(),
            }
            let rope = self.doc.rope();
            if content_changed {
                let line = rope.char_to_line(head.min(rope.len_chars()));
                let width = line_display_columns(&self.line_text(line)) as f32 * FONT_SIZE * 0.65;
                state.content_width.set(if document_changed {
                    width
                } else {
                    state.content_width.get().max(width)
                });
            }
        }
        state.last_document.set(Some(document));
        state.last_version.set(version);
        if theme_changed {
            *state.last_theme.borrow_mut() = Some(self.theme.id.clone());
        }
        state.last_syntax.set(syntax);
        if projection_changed {
            *state.last_snapshot.borrow_mut() = self.snapshot.cloned();
        }
        if fold_changed {
            *state.last_fold_spans.borrow_mut() = self.overlay.fold_spans.to_vec();
        }

        let viewport_h = bounds.height;
        // Il margine cambia la proiezione, non il caret: richiamarlo qui
        // annullerebbe lo scroll manuale con cui l'utente ha trovato il blocco.
        if fold_changed {
            state
                .scroll_y
                .set(state.scroll_y.get().clamp(0.0, self.max_scroll(viewport_h)));
        }
        if (content_changed || cursor_moved) && viewport_h > 0.0 {
            state.last_cursor.set(head);
            let rope = self.doc.rope();
            let line = rope.char_to_line(head.min(rope.len_chars()));
            let vis_line = self.fold_projection().doc_to_visual(line);
            let top = PAD_V + vis_line as f32 * LINE_H;
            let bottom = top + LINE_H;
            let max = self.max_scroll(viewport_h);
            let scroll = state.scroll_y.get().clamp(0.0, max);
            let margin = 2.0 * LINE_H;
            if top < scroll {
                state.scroll_y.set((top - margin).clamp(0.0, max));
            } else if bottom > scroll + viewport_h {
                state
                    .scroll_y
                    .set((bottom - viewport_h + margin).clamp(0.0, max));
            } else {
                state.scroll_y.set(scroll);
            }

            let text_viewport = (bounds.width - self.text_x0()).max(0.0);
            let cursor_col = head - rope.line_to_char(line);
            let text = self.line_text(line);
            let cursor_x = self.x_at_col(state, line, cursor_col, &text);
            let scroll_x = ViewportMetrics::ensure_x_visible(
                state.scroll_x.get(),
                text_viewport,
                cursor_x,
                FONT_SIZE,
            );
            state.scroll_x.set(ViewportMetrics::clamp_x(
                scroll_x,
                state.content_width.get().max(cursor_x),
                text_viewport,
            ));
        }
    }

    /// x (relativa all'inizio del testo) della colonna char `col` nel
    /// paragraph della riga con testo `text`.
    #[allow(clippy::indexing_slicing)] // byte proviene da char_indices/len di text
    fn x_in_paragraph(paragraph: &LineParagraph, text: &str, col: usize) -> f32 {
        let byte = text
            .char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(text.len());
        let grapheme = text[..byte].graphemes(true).count();
        paragraph
            .grapheme_position(0, grapheme)
            .map(|pt| pt.x)
            .unwrap_or(0.0)
    }

    /// x (relativa all'inizio del testo) della colonna char `col` sulla riga.
    fn x_at_col(&self, state: &EditorState, line: usize, col: usize, text: &str) -> f32 {
        self.with_paragraph(state, line, |p| Self::x_in_paragraph(p, text, col))
    }

    /// Come `x_at_col` ma con il testo già in cache: nessuna String per frame.
    fn x_at_col_cached(&self, state: &EditorState, line: usize, col: usize) -> f32 {
        self.with_line(state, line, |p, text| Self::x_in_paragraph(p, text, col))
    }

    /// Click/drag → char index assoluto (riga dalla y, colonna via hit_test).
    fn resolve_position(&self, state: &EditorState, bounds: Rectangle, pos: Point) -> usize {
        let rope = self.doc.rope();
        let n_lines = rope.len_lines();
        let proj = self.fold_projection();
        let vis_count = proj.visible_line_count(n_lines);
        let rel_y = pos.y - bounds.y - PAD_V + state.scroll_y.get();
        let vis_line =
            ((rel_y / LINE_H).floor().max(0.0) as usize).min(vis_count.saturating_sub(1));
        let line = proj.visual_to_doc(vis_line, n_lines);

        let rel_x = (pos.x - bounds.x - self.text_x0() + state.scroll_x.get()).max(0.0);
        let (col, content_len) = self.with_line(state, line, |p, text| {
            let content_len = text.chars().count();
            let col = match p.hit_test(Point::new(rel_x, LINE_H * 0.5)) {
                // ATTENZIONE: cosmic-text ritorna l'offset in BYTE nel testo
                // della riga → conversione a char index.
                Some(Hit::CharOffset(byte)) => {
                    let byte = byte.min(text.len());
                    text.get(..byte)
                        .map_or(content_len, |head| head.chars().count())
                }
                None => content_len,
            };
            (col, content_len)
        });
        rope.line_to_char(line) + col.min(content_len)
    }

    /// Testo attualmente selezionato (per Ctrl+C / Ctrl+X).
    fn selected_text(&self) -> Option<String> {
        lume_core::multi_edit::selected_text(self.doc)
    }

    fn request_input_method(
        &self,
        state: &EditorState,
        bounds: Rectangle,
        shell: &mut Shell<'_, M>,
    ) {
        let rope = self.doc.rope();
        let head = self.doc.selection().head.min(rope.len_chars());
        let line = rope.char_to_line(head);
        let vis_line = self.fold_projection().doc_to_visual(line);
        let col = head - rope.line_to_char(line);
        let text = self.line_text(line);
        let x = self.x_at_col(state, line, col, &text);
        let y = PAD_V + vis_line as f32 * LINE_H - state.scroll_y.get();
        let cursor = Rectangle::new(
            Point::new(
                bounds.x + self.text_x0() + x - state.scroll_x.get(),
                bounds.y + y,
            ),
            Size::new(1.0, LINE_H),
        );

        let request: InputMethod<&str> = if state.is_focused.get() {
            InputMethod::Enabled {
                cursor,
                purpose: input_method::Purpose::Normal,
                preedit: None,
            }
        } else {
            InputMethod::Disabled
        };
        shell.request_input_method(&request);
    }

    fn publish(&self, shell: &mut Shell<'_, M>, cmd: EditorCommand) {
        shell.publish((self.on_command)(cmd));
        shell.capture_event();
        shell.request_redraw();
    }

    /// Traduce un KeyPressed in comando. `None` = evento non consumato.
    fn command_for_key(
        &self,
        key: &Key,
        modifiers: keyboard::Modifiers,
        text: Option<&str>,
        clipboard: &mut dyn Clipboard,
        page_lines: usize,
    ) -> Option<EditorCommand> {
        use EditorCommand::*;
        let ctrl = modifiers.command();
        let shift = modifiers.shift();
        let motion = |m: Motion| Some(Move(m, shift));

        match key.as_ref() {
            Key::Named(Named::ArrowLeft) => {
                motion(if ctrl { Motion::WordLeft } else { Motion::Left })
            }
            Key::Named(Named::ArrowRight) => motion(if ctrl {
                Motion::WordRight
            } else {
                Motion::Right
            }),
            Key::Named(Named::ArrowUp) => motion(Motion::Up),
            Key::Named(Named::ArrowDown) => motion(Motion::Down),
            Key::Named(Named::Home) => motion(if ctrl { Motion::DocStart } else { Motion::Home }),
            Key::Named(Named::End) => motion(if ctrl { Motion::DocEnd } else { Motion::End }),
            Key::Named(Named::PageUp) => Some(EditorCommand::MovePage {
                direction: PageDirection::Up,
                lines: page_lines,
                extend: shift,
            }),
            Key::Named(Named::PageDown) => Some(EditorCommand::MovePage {
                direction: PageDirection::Down,
                lines: page_lines,
                extend: shift,
            }),
            Key::Named(Named::Escape) => Some(KeepPrimary),
            Key::Named(Named::Backspace) => Some(Backspace),
            Key::Named(Named::Delete) => Some(Delete),
            Key::Named(Named::Enter) => Some(Enter),
            Key::Named(Named::Tab) => Some(Tab),
            Key::Character(c) if ctrl => match c.to_ascii_lowercase().as_str() {
                "z" if shift => Some(Redo),
                "z" => Some(Undo),
                "y" => Some(Redo),
                "a" => Some(SelectAll),
                "d" => Some(AddNextOccurrence),
                "s" if shift => None,
                "s" => Some(Save),
                "c" => {
                    if let Some(sel) = self.selected_text() {
                        clipboard.write(clipboard::Kind::Standard, sel);
                    }
                    Some(Copy)
                }
                "x" => {
                    if let Some(sel) = self.selected_text() {
                        clipboard.write(clipboard::Kind::Standard, sel);
                    }
                    Some(Cut)
                }
                "v" => clipboard
                    .read(clipboard::Kind::Standard)
                    .map(|s| Paste(s.replace("\r\n", "\n").replace('\r', "\n"))),
                _ => None,
            },
            _ => {
                // Carattere stampabile: no ctrl/alt, testo non di controllo.
                if ctrl || modifiers.alt() {
                    return None;
                }
                let t: String = text?.chars().filter(|c| !c.is_control()).collect();
                (!t.is_empty()).then_some(Insert(t))
            }
        }
    }
}

impl<'a, M> Widget<M, iced::Theme, iced::Renderer> for EditorView<'a, M> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<EditorState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(EditorState::default())
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
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        #[cfg(feature = "performance-diagnostics")]
        let _phase = crate::performance_diagnostics::Span::new("editor.update");
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<EditorState>();
        self.sync(state, bounds);

        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if cursor.position_over(bounds).is_some() {
                    let (dx, dy) = match delta {
                        ScrollDelta::Lines { x, y } => (*x * 3.0 * FONT_SIZE, *y * 3.0 * LINE_H),
                        ScrollDelta::Pixels { x, y } => (*x, *y),
                    };
                    if dx != 0.0 || state.modifiers.get().shift() {
                        let horizontal = if dx != 0.0 { dx } else { dy };
                        let viewport = (bounds.width - self.text_x0()).max(0.0);
                        state.scroll_x.set(ViewportMetrics::clamp_x(
                            state.scroll_x.get() - horizontal,
                            state.content_width.get(),
                            viewport,
                        ));
                    } else {
                        let max = self.max_scroll(bounds.height);
                        state
                            .scroll_y
                            .set((state.scroll_y.get() - dy).clamp(0.0, max));
                    }
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if cursor
                    .position()
                    .zip(self.popup_rect(state, bounds))
                    .is_some_and(|(punto, rect)| rect.contains(punto)) =>
            {
                // Il popup si sceglie con la tastiera: il clic non passa al
                // testo che ci sta sotto.
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(pos) = cursor.position_over(bounds) {
                    state.is_focused.set(true);
                    state.is_dragging.set(!state.modifiers.get().alt());
                    let p = self.resolve_position(state, bounds, pos);
                    let line = self.doc.rope().char_to_line(p);
                    // L'appunto ha la precedenza: era il gesto del margine prima
                    // del folding; il blocco resta comprimibile dal menu.
                    if in_marker_column(pos.x - bounds.x)
                        && line_has_note(self.overlay.note_ranges, line)
                    {
                        state.is_dragging.set(false);
                        self.publish(shell, EditorCommand::NoteMarkerClicked { line });
                        shell.capture_event();
                        shell.request_redraw();
                        return;
                    }
                    if in_marker_column(pos.x - bounds.x) && self.line_is_foldable(line) {
                        state.is_dragging.set(false);
                        self.publish(shell, EditorCommand::FoldClicked { line });
                        shell.capture_event();
                        shell.request_redraw();
                        return;
                    }
                    if state.modifiers.get().alt() {
                        self.publish(shell, EditorCommand::ToggleCursor { pos: p });
                    } else {
                        let extend = state.modifiers.get().shift();
                        self.publish(shell, EditorCommand::SetCursor { pos: p, extend });
                    }
                } else if state.is_focused.replace(false) {
                    state.is_dragging.set(false);
                    state.ime.borrow_mut().close();
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                if let Some(punto) = cursor.position_over(bounds) {
                    state.is_focused.set(true);
                    let p = self.resolve_position(state, bounds, punto);
                    self.publish(
                        shell,
                        EditorCommand::ContextMenu {
                            pos: p,
                            // il punto è già relativo alla finestra: è la
                            // stessa base che usa `pin().position()`
                            screen_x: punto.x.round() as i32,
                            screen_y: punto.y.round() as i32,
                        },
                    );
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if state.is_dragging.get() {
                    let p = self.resolve_position(state, bounds, *position);
                    self.publish(
                        shell,
                        EditorCommand::SetCursor {
                            pos: p,
                            extend: true,
                        },
                    );
                } else if self.overlay.hover_enabled {
                    let ora = self.hover_target(state, bounds, *position);
                    if let Some(nuovo) = hover_change(state.hover_word.get(), ora, true) {
                        state.hover_word.set(ora);
                        shell.publish((self.on_command)(EditorCommand::HoverAt(nuovo)));
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorLeft) => {
                if let Some(nuovo) =
                    hover_change(state.hover_word.get(), None, self.overlay.hover_enabled)
                {
                    state.hover_word.set(None);
                    shell.publish((self.on_command)(EditorCommand::HoverAt(nuovo)));
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.is_dragging.get() {
                    state.is_dragging.set(false);
                    shell.capture_event();
                }
            }
            Event::InputMethod(event) => match event {
                input_method::Event::Opened if state.is_focused.get() => {
                    state.ime.borrow_mut().open();
                    shell.request_redraw();
                }
                input_method::Event::Preedit(content, selection) if state.is_focused.get() => {
                    state
                        .ime
                        .borrow_mut()
                        .preedit(content.clone(), selection.clone());
                    shell.capture_event();
                    shell.request_redraw();
                }
                input_method::Event::Commit(content) if state.is_focused.get() => {
                    let committed = state.ime.borrow_mut().commit(content.clone());
                    if let Some(text) = committed {
                        self.publish(shell, EditorCommand::Insert(text));
                    }
                }
                input_method::Event::Closed => {
                    state.ime.borrow_mut().close();
                    shell.request_redraw();
                }
                _ => {}
            },
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
                state.modifiers.set(*m);
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modifiers,
                text,
                ..
            }) => {
                state.modifiers.set(*modifiers);
                if state.is_focused.get() {
                    if state.ime.borrow().current().is_some() {
                        // La precomposizione appartiene all'IME: solo Commit
                        // pubblica il testo da applicare a tutte le selezioni.
                        shell.capture_event();
                        return;
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Shift)) {
                        shell.capture_event();
                    } else {
                        let overlay = overlay_key(
                            OverlayKeys {
                                popup_open: self.overlay.popup.is_some(),
                                card_open: self.overlay.hover.is_some(),
                                complete_enabled: self.overlay.complete_enabled,
                            },
                            key,
                            *modifiers,
                        );
                        let comando = overlay.or_else(|| {
                            self.command_for_key(
                                key,
                                *modifiers,
                                text.as_deref(),
                                clipboard,
                                ViewportMetrics::page_lines(bounds.height),
                            )
                        });
                        if let Some(cmd) = comando {
                            state.hover_word.set(None);
                            self.publish(shell, cmd);
                        }
                    }
                }
            }
            _ => {}
        }
        self.request_syntax_viewport(state, bounds, shell);
        self.request_input_method(state, bounds, shell);
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        let text_area = Rectangle {
            x: bounds.x + self.text_x0(),
            width: bounds.width - self.text_x0(),
            ..bounds
        };
        if cursor.position_over(text_area).is_some() {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::None
        }
    }

    #[allow(clippy::expect_used)] // ime_cache appena riempita nella riga precedente
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
        #[cfg(feature = "performance-diagnostics")]
        let _phase = crate::performance_diagnostics::Span::new("editor.draw");
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<EditorState>();
        self.sync(state, bounds);

        let rope = self.doc.rope();
        let n_lines = rope.len_lines();
        let proj = self.fold_projection();
        let vis_count = proj.visible_line_count(n_lines).max(1);
        let scroll = state.scroll_y.get();

        // Virtualizzazione: solo righe visive ± OVERDRAW.
        let first_vis =
            (((scroll - PAD_V) / LINE_H).floor() as isize - OVERDRAW as isize).max(0) as usize;
        let last_vis = ((((scroll - PAD_V) + bounds.height) / LINE_H).ceil() as isize
            + OVERDRAW as isize)
            .clamp(0, vis_count as isize - 1) as usize;
        let first = proj.visual_to_doc(first_vis, n_lines);
        let last = proj.visual_to_doc(last_vis, n_lines).max(first);

        self.prepare_highlights(state, first, last);

        // Eviction delle righe lontane dal viewport.
        let keep = |k: &usize| *k + CACHE_KEEP >= first && *k <= last + CACHE_KEEP;
        state.cache.borrow_mut().retain(|k, _| keep(k));
        state.gutter_cache.borrow_mut().retain(|k, _| keep(k));

        let sel = self.doc.selection();
        let head = sel.head.min(rope.len_chars());
        let cursor_line = rope.char_to_line(head);
        let vis_y = |vis_line: usize| bounds.y + PAD_V + vis_line as f32 * LINE_H - scroll;
        let line_y = |doc_line: usize| vis_y(proj.doc_to_visual(doc_line));
        let text_x = bounds.x + self.text_x0() - state.scroll_x.get();
        let gutter_bounds = Rectangle {
            width: GUTTER_W,
            ..bounds
        };
        let text_bounds = Rectangle {
            x: bounds.x + self.text_x0().min(bounds.width),
            width: (bounds.width - self.text_x0()).max(0.0),
            ..bounds
        };

        let quad = |r: Rectangle| Quad {
            bounds: r,
            ..Quad::default()
        };

        renderer.with_layer(bounds, |renderer| {
            // Sfondo dell'editor.
            renderer.fill_quad(quad(bounds), self.theme.bg);

            // Sfondo riga corrente: quad piena larghezza PRIMA del testo.
            let cy = line_y(cursor_line);
            if cy + LINE_H > bounds.y && cy < bounds.y + bounds.height {
                renderer.fill_quad(
                    quad(Rectangle {
                        x: bounds.x,
                        y: cy,
                        width: bounds.width,
                        height: LINE_H,
                    }),
                    self.theme.surface,
                );
            }

            let mut cursore_highlight = 0usize;
            // Un solo layer per il testo di tutte le righe visibili: un layer
            // per riga costava a wgpu una preparazione e un passo di disegno
            // del testo per riga (~50 a frame). Le righe non si sovrappongono
            // e dentro un layer i quad restano sotto il testo, come prima.
            let mut righe_visibili = Vec::with_capacity((last_vis + 1).saturating_sub(first_vis));
            renderer.with_layer(text_bounds, |renderer| {
                for vis_line in first_vis..=last_vis {
                    let line = proj.visual_to_doc(vis_line, n_lines);
                    let y = vis_y(vis_line);
                    let line_slice = rope.line(line);
                    let mut content_len = line_slice.len_chars();
                    if content_len > 0 && line_slice.char(content_len - 1) == '\n' {
                        content_len -= 1;
                    }
                    let visual_width = display_columns(line_slice.chars().take(content_len)) as f32
                        * FONT_SIZE
                        * 0.65;
                    state
                        .content_width
                        .set(state.content_width.get().max(visual_width));
                    let line_start = rope.line_to_char(line);
                    let line_content_end = line_start + content_len;

                    let segni = marks_on_line(self.overlay.marks, line_start, line_content_end);
                    // Evidenziazione dei match di Trova (Ctrl+F): sotto la
                    // selezione, cosicché il match corrente — che dopo un salto
                    // COINCIDE con la selezione — resti quello più in vista.
                    let (righe_match, prossimo) = ranges_touching_line(
                        self.highlights,
                        cursore_highlight,
                        line_start,
                        line_content_end,
                    );
                    cursore_highlight = prossimo;
                    for m in righe_match {
                        let h_col = m.start.saturating_sub(line_start).min(content_len);
                        let e_col = m.end.saturating_sub(line_start).min(content_len);
                        let x0 = self.x_at_col_cached(state, line, h_col);
                        let x1 = self.x_at_col_cached(state, line, e_col);
                        if x1 > x0 {
                            renderer.fill_quad(
                                quad(Rectangle {
                                    x: text_x + x0,
                                    y,
                                    width: x1 - x0,
                                    height: LINE_H,
                                }),
                                self.theme.hl,
                            );
                        }
                    }

                    // Selezione: quad theme.sel dietro al testo.
                    for sel in
                        selections_in(self.doc.selections().ranges(), line_start, line_content_end)
                            .iter()
                            .filter(|sel| !sel.is_empty())
                    {
                        let s_col = sel.min().saturating_sub(line_start).min(content_len);
                        let covers_newline = sel.max() > line_content_end;
                        let x0 = self.x_at_col_cached(state, line, s_col);
                        let x1 = if covers_newline {
                            // La selezione ingloba il newline: stub di mezza cella.
                            self.with_paragraph(state, line, |p| p.min_bounds().width)
                                + FONT_SIZE * 0.5
                        } else {
                            let e_col = (sel.max() - line_start).min(content_len);
                            self.x_at_col_cached(state, line, e_col)
                        };
                        if x1 > x0 {
                            renderer.fill_quad(
                                quad(Rectangle {
                                    x: text_x + x0,
                                    y,
                                    width: x1 - x0,
                                    height: LINE_H,
                                }),
                                self.theme.sel,
                            );
                        }
                    }

                    // Testo della riga (colore default theme.ui per gli span
                    // senza colore esplicito).
                    let mut line_px_w = 0.0f32;
                    if content_len > 0 {
                        self.with_paragraph(state, line, |p| {
                            line_px_w = p.min_bounds().width;
                            renderer.fill_paragraph(
                                p,
                                Point::new(text_x, y),
                                self.theme.ui,
                                text_bounds,
                            );
                        });
                    }
                    if let Some(hidden) = proj.collapsed_count_at(line) {
                        let badge = if hidden == 1 {
                            " … 1 riga ".to_owned()
                        } else {
                            format!(" … {hidden} righe ")
                        };
                        let badge_x = text_x + line_px_w + FONT_SIZE * 0.5;
                        let badge_w = badge.chars().count() as f32 * FONT_SIZE * 0.62;
                        renderer.fill_quad(
                            Quad {
                                bounds: Rectangle {
                                    x: badge_x,
                                    y: y + 2.0,
                                    width: badge_w,
                                    height: LINE_H - 4.0,
                                },
                                border: iced::Border {
                                    color: self.theme.border,
                                    width: 1.0,
                                    radius: 3.0.into(),
                                },
                                ..Quad::default()
                            },
                            self.theme.surface,
                        );
                        renderer.fill_text(
                            mono_text(badge, badge_w + FONT_SIZE),
                            Point::new(badge_x, y),
                            self.theme.muted,
                            text_bounds,
                        );
                    }

                    for segno in &segni {
                        let x0 = self.x_at_col_cached(state, line, segno.start_col);
                        let x1 = self.x_at_col_cached(state, line, segno.end_col);
                        renderer.fill_quad(
                            quad(underline_rect(
                                text_x + x0,
                                text_x + x1,
                                y,
                                LINE_H,
                                FONT_SIZE * 0.5,
                            )),
                            segno.severity.color(self.theme),
                        );
                    }
                    righe_visibili.push((line, y, segni));
                }
            });

            let mut margine: Vec<RigaMargine> = Vec::with_capacity(righe_visibili.len());
            for (line, y, segni) in righe_visibili {
                let fold_marker = self
                    .line_is_foldable(line)
                    .then(|| proj.collapsed_count_at(line).is_some());
                margine.push((
                    y,
                    line == cursor_line,
                    worst_severity(&segni),
                    line_has_note(self.overlay.note_ranges, line),
                    fold_marker,
                ));

                renderer.fill_quad(
                    quad(Rectangle {
                        y,
                        height: LINE_H,
                        ..gutter_bounds
                    }),
                    if line == cursor_line {
                        self.theme.surface
                    } else {
                        self.theme.bg
                    },
                );

                // Numero di riga right-aligned nel gutter. Il paragraph vive
                // nella cache (il renderer trattiene solo una Weak) e il
                // right-align è fatto a mano via min_bounds: iced disattiva
                // l'align interno sui paragraph a riga singola.
                let mut gutter = state.gutter_cache.borrow_mut();
                let number = gutter.entry(line).or_insert_with(|| {
                    LineParagraph::with_text(Text {
                        content: (line + 1).to_string().as_str(),
                        bounds: Size::new(GUTTER_W, LINE_H),
                        size: Pixels(FONT_SIZE),
                        line_height: LineHeight::Absolute(Pixels(LINE_H)),
                        font: crate::MONO,
                        align_x: TextAlignment::Left,
                        align_y: iced::alignment::Vertical::Top,
                        shaping: Shaping::Basic,
                        wrapping: Wrapping::None,
                    })
                });
                let num_w = number.min_bounds().width;
                renderer.fill_paragraph(
                    number,
                    Point::new(bounds.x + GUTTER_W - GUTTER_PAD_R - num_w, y),
                    if line == cursor_line {
                        self.theme.sec
                    } else {
                        self.theme.faint
                    },
                    bounds,
                );

                // La colonna autore viene disegnata sopra il testo scorrevole:
                // anche uno scroll orizzontale estremo non può invaderla.
                if let Some(authors) = self.overlay.blame {
                    let author_bounds = Rectangle {
                        x: bounds.x + TEXT_X0,
                        y,
                        width: 136.0,
                        height: LINE_H,
                    };
                    renderer.fill_quad(quad(author_bounds), self.theme.bg);
                    if let Some(author) = super::overlay::blame_author_on_line(authors, line) {
                        renderer.fill_text(
                            mono_text(clip_text(author, 19), author_bounds.width - 8.0),
                            Point::new(author_bounds.x + 4.0, y),
                            self.theme.faint,
                            author_bounds,
                        );
                    }
                }
            }

            renderer.with_layer(text_bounds, |renderer| {
                let preedit = state
                    .ime
                    .borrow()
                    .current()
                    .map(|preedit| (preedit.content.clone(), preedit.selection.clone()));
                if state.is_focused.get()
                    && let Some((content, selection)) = preedit
                {
                    let col = head - rope.line_to_char(cursor_line);
                    let line_text = self.line_text(cursor_line);
                    let x = self.x_at_col(state, cursor_line, col, &line_text);
                    let y = line_y(cursor_line);
                    if y + LINE_H > bounds.y && y < bounds.y + bounds.height {
                        let paragraph = LineParagraph::with_text(Text {
                            content: content.as_str(),
                            bounds: Size::new(f32::INFINITY, LINE_H),
                            size: Pixels(FONT_SIZE),
                            line_height: LineHeight::Absolute(Pixels(LINE_H)),
                            font: crate::MONO,
                            align_x: TextAlignment::Left,
                            align_y: iced::alignment::Vertical::Top,
                            shaping: Shaping::Advanced,
                            wrapping: Wrapping::None,
                        });
                        *state.ime_cache.borrow_mut() = Some(paragraph);
                        let cache = state.ime_cache.borrow();
                        let paragraph = cache.as_ref().expect("preedit appena memorizzato");
                        let width = paragraph.min_bounds().width.max(2.0);
                        let origin = Point::new(text_x + x, y);
                        renderer.fill_quad(
                            quad(Rectangle::new(origin, Size::new(width, LINE_H))),
                            self.theme.surface,
                        );
                        if let Some(selection) = selection {
                            let start = grapheme_offset_at_byte(&content, selection.start);
                            let end = grapheme_offset_at_byte(&content, selection.end);
                            let x0 = paragraph
                                .grapheme_position(0, start.min(end))
                                .map(|point| point.x)
                                .unwrap_or(0.0);
                            let x1 = paragraph
                                .grapheme_position(0, start.max(end))
                                .map(|point| point.x)
                                .unwrap_or(width);
                            if x1 > x0 {
                                renderer.fill_quad(
                                    quad(Rectangle::new(
                                        Point::new(origin.x + x0, y),
                                        Size::new(x1 - x0, LINE_H),
                                    )),
                                    self.theme.sel,
                                );
                            }
                        }
                        renderer.fill_paragraph(paragraph, origin, self.theme.ui, text_bounds);
                        renderer.fill_quad(
                            quad(Rectangle::new(
                                Point::new(origin.x, y + LINE_H - 2.0),
                                Size::new(width, 1.0),
                            )),
                            self.theme.accent,
                        );
                    }
                } else {
                    state.ime_cache.borrow_mut().take();
                }

                // Caret principali e secondari, solo nel viewport.
                if state.is_focused.get() {
                    let from = rope.line_to_char(first);
                    let to = if last + 1 < n_lines {
                        rope.line_to_char(last + 1)
                    } else {
                        rope.len_chars()
                    };
                    for range in selections_in(self.doc.selections().ranges(), from, to) {
                        let head = range.head.min(rope.len_chars());
                        let line = rope.char_to_line(head);
                        if proj.is_line_hidden(line) {
                            continue;
                        }
                        let y = line_y(line);
                        if y + LINE_H <= bounds.y || y >= bounds.y + bounds.height {
                            continue;
                        }
                        let col = head - rope.line_to_char(line);
                        let x = self.x_at_col_cached(state, line, col);
                        renderer.fill_quad(
                            quad(Rectangle {
                                x: text_x + x,
                                y,
                                width: if *range == sel { 2.0 } else { 1.5 },
                                height: LINE_H,
                            }),
                            self.theme.accent,
                        );
                    }
                }
            });

            let colonna = Rectangle {
                x: bounds.x + GUTTER_W,
                width: MARKER_W,
                ..bounds
            };
            renderer.with_layer(colonna, |renderer| {
                for (y, corrente, peggiore, con_appunto, fold_marker) in margine {
                    renderer.fill_quad(
                        quad(Rectangle {
                            x: colonna.x,
                            y,
                            width: MARKER_W,
                            height: LINE_H,
                        }),
                        if corrente {
                            self.theme.surface
                        } else {
                            self.theme.bg
                        },
                    );
                    if con_appunto {
                        renderer.fill_quad(
                            quad(note_bar_rect(colonna.x, y, LINE_H)),
                            self.theme.accent,
                        );
                    }
                    // Il segno del diagnostico vince sulla freccia aperta: un
                    // errore sulla riga `fn …` deve restare visibile. La freccia
                    // chiusa vince sempre, o il blocco nascosto non si vedrebbe.
                    if let Some(collapsed) = fold_marker
                        && (collapsed || peggiore.is_none())
                    {
                        let freccia = if collapsed { "▸" } else { "▾" };
                        renderer.fill_text(
                            mono_text(freccia.to_owned(), MARKER_W),
                            Point::new(colonna.x + 3.5, y),
                            if collapsed {
                                self.theme.sec
                            } else {
                                self.theme.faint
                            },
                            colonna,
                        );
                    } else if let Some(gravita) = peggiore {
                        renderer.fill_quad(
                            Quad {
                                bounds: dot_rect(colonna.x, MARKER_W, y, LINE_H),
                                border: iced::Border {
                                    radius: 3.0.into(),
                                    ..iced::Border::default()
                                },
                                ..Quad::default()
                            },
                            gravita.color(self.theme),
                        );
                    }
                }
            });
            // Ultimi, così stanno sopra a tutto: ognuno ha il suo livello.
            self.draw_popup(state, renderer, bounds);
            self.draw_card(state, renderer, bounds);
        });
    }
}

impl<'a, M: 'a> From<EditorView<'a, M>> for Element<'a, M, iced::Theme, iced::Renderer> {
    fn from(widget: EditorView<'a, M>) -> Self {
        Element::new(widget)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use iced::Rectangle;
    use iced::advanced::text::Paragraph as _;
    use lume_core::{DocId, Document};
    use lume_syntax::{LanguageId, SyntaxState, TokenClass};
    use lume_theme::LumeTheme;

    use super::{
        EditorState, EditorView, grapheme_offset_at_byte, in_marker_column, line_display_columns,
        ranges_touching_line,
    };

    #[test]
    fn folding_non_richiama_il_caret_fuori_vista() {
        use crate::editor::{EditorOverlay, FoldProjection};
        use lume_syntax::FoldRegion;
        let document = Document::from_str(&"riga\n".repeat(200), None, DocId(91));
        let theme = LumeTheme::default();
        let state = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 400.0).into());
        EditorView::<()>::new(&document, None, &theme, &[], |_| ()).sync(&state, bounds);
        state.scroll_y.set(1000.0);
        state.scroll_x.set(12.0);
        let projection = FoldProjection::from_regions(
            &[FoldRegion {
                start_line: 50,
                end_line: 55,
            }],
            |_| true,
        );
        let overlay = EditorOverlay {
            fold_spans: projection.spans(),
            ..EditorOverlay::default()
        };
        EditorView::<()>::new(&document, None, &theme, &[], |_| ())
            .overlay(overlay)
            .sync(&state, bounds);
        assert_eq!(
            state.scroll_y.get(),
            1000.0,
            "il margine non sposta il viewport sul caret alla riga 1"
        );
        assert_eq!(state.scroll_x.get(), 12.0);
        EditorView::<()>::new(&document, None, &theme, &[], |_| ()).sync(&state, bounds);
        assert_eq!(
            state.scroll_y.get(),
            1000.0,
            "anche espandere conserva lo scroll"
        );
    }

    #[test]
    fn multi_due_viste_seguono_documento_e_storico() {
        use lume_core::{Range, SelectionSet};
        let mut doc = Document::from_str("abc def", None, DocId(1));
        doc.set_selections(
            SelectionSet::try_new(vec![Range::point(1), Range::point(5)], 1, 7).unwrap(),
        );
        let theme = LumeTheme::default();
        let first = EditorState::default();
        let second = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 600.0).into());
        let sync = |doc: &Document| {
            let a = EditorView::<()>::new(doc, None, &theme, &[], |_| ());
            let b = EditorView::<()>::new(doc, None, &theme, &[], |_| ());
            a.sync(&first, bounds);
            b.sync(&second, bounds);
            assert_eq!(a.doc.selections(), b.doc.selections());
            assert_eq!(first.last_cursor.get(), doc.selection().head);
            assert_eq!(second.last_cursor.get(), doc.selection().head);
            assert_eq!(first.last_version.get(), doc.version());
            assert_eq!(second.last_version.get(), doc.version());
        };
        sync(&doc);
        lume_core::multi_edit::apply(&mut doc, lume_core::multi_edit::MultiEdit::Replace("X"))
            .unwrap()
            .unwrap();
        sync(&doc);
        assert_eq!(doc.selection(), Range::point(7));
        doc.undo().unwrap();
        sync(&doc);
        assert_eq!(doc.selection(), Range::point(5));
    }

    #[test]
    fn multi_caret_al_confine_iniziale_del_viewport_restano_visibili() {
        let ranges = [lume_core::Range::new(1, 10), lume_core::Range::point(20)];
        assert_eq!(super::selections_in(&ranges, 10, 15), &ranges[..1]);
    }

    #[test]
    fn salva_tutto_lascia_la_scorciatoia_al_listener_globale() {
        use crate::editor::EditorCommand;
        use iced::keyboard::{Key, Modifiers};
        let doc = Document::from_str("testo", None, DocId(1));
        let theme = LumeTheme::default();
        let view = EditorView::<()>::new(&doc, None, &theme, &[], |_| ());
        let mut clipboard = iced::advanced::clipboard::Null;
        assert_eq!(
            view.command_for_key(
                &Key::Character("s".into()),
                Modifiers::CTRL,
                Some("s"),
                &mut clipboard,
                10
            ),
            Some(EditorCommand::Save)
        );
        for lettera in ["s", "S"] {
            assert_eq!(
                view.command_for_key(
                    &Key::Character(lettera.into()),
                    Modifiers::CTRL | Modifiers::SHIFT,
                    Some(lettera),
                    &mut clipboard,
                    10
                ),
                None
            );
        }
    }

    #[test]
    fn multi_widget_scorciatoie_e_precedenza_popup() {
        use crate::editor::EditorCommand;
        use iced::keyboard::{Key, Modifiers, key::Named};
        let doc = Document::from_str("rosso rosso", None, DocId(1));
        let theme = LumeTheme::default();
        let view = EditorView::<()>::new(&doc, None, &theme, &[], |_| ());
        let mut clipboard = iced::advanced::clipboard::Null;
        assert_eq!(
            view.command_for_key(
                &Key::Character("d".into()),
                Modifiers::CTRL,
                None,
                &mut clipboard,
                10
            ),
            Some(EditorCommand::AddNextOccurrence)
        );
        assert_eq!(
            view.command_for_key(
                &Key::Named(Named::Escape),
                Modifiers::default(),
                None,
                &mut clipboard,
                10
            ),
            Some(EditorCommand::KeepPrimary)
        );
        assert_eq!(
            super::overlay_key(
                super::OverlayKeys {
                    popup_open: true,
                    card_open: false,
                    complete_enabled: true
                },
                &Key::Named(Named::Escape),
                Modifiers::default()
            ),
            Some(EditorCommand::PopupDismiss)
        );
    }

    #[test]
    fn la_colonna_del_marcatore_sta_fra_il_gutter_e_il_testo() {
        assert!(!in_marker_column(0.0));
        assert!(!in_marker_column(super::GUTTER_W - 0.5));
        assert!(in_marker_column(super::GUTTER_W));
        assert!(in_marker_column(super::GUTTER_W + super::MARKER_W - 0.5));
        assert!(!in_marker_column(super::TEXT_X0));
        assert!(!in_marker_column(500.0));
    }

    #[test]
    fn ranges_touching_line_avanza_senza_tornare_indietro() {
        let ranges = vec![0..3, 5..8, 5..9, 20..22, 30..31];
        // riga 0: char 0..10 — prende i primi tre (anche i due che si
        // sovrappongono fra loro, come farebbero due match adiacenti)
        let (trovati, prossimo) = ranges_touching_line(&ranges, 0, 0, 10);
        assert_eq!(trovati, vec![0..3, 5..8, 5..9]);
        // riga successiva riparte da `prossimo`, non da 0
        let (trovati, prossimo2) = ranges_touching_line(&ranges, prossimo, 10, 25);
        assert_eq!(trovati, vec![20..22]);
        let (trovati, _) = ranges_touching_line(&ranges, prossimo2, 25, 35);
        assert_eq!(trovati, vec![30..31]);
    }

    #[test]
    fn ranges_touching_line_riga_senza_match_non_trova_niente() {
        let unico = 10..12;
        let ranges = [unico];
        let (trovati, prossimo) = ranges_touching_line(&ranges, 0, 0, 5);
        assert!(trovati.is_empty());
        assert_eq!(
            prossimo, 0,
            "il cursore non avanza se non ha superato niente"
        );
    }

    #[test]
    fn visual_columns_handle_tabs_wide_and_combining_text() {
        assert_eq!(line_display_columns("a\tb"), 5);
        assert_eq!(line_display_columns("界e\u{301}"), 3);
    }

    #[test]
    fn ime_byte_offsets_are_clamped_to_unicode_boundaries() {
        assert_eq!(grapheme_offset_at_byte("éx", 1), 0);
        assert_eq!(grapheme_offset_at_byte("éx", 2), 1);
    }

    #[test]
    fn switching_same_version_documents_rebuilds_paragraphs() {
        let short = Document::from_str("x", None, DocId(1));
        let long = Document::from_str("xxxxxxxxxxxxxxxxxxxxxxxx", None, DocId(2));
        assert_eq!(short.version(), long.version());

        let theme = LumeTheme::builtin().remove(0);
        let state = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 600.0).into());

        let short_view = EditorView::<()>::new(&short, None, &theme, &[], |_| ());
        short_view.sync(&state, bounds);
        let short_width = short_view.with_paragraph(&state, 0, |p| p.min_bounds().width);

        let long_view = EditorView::<()>::new(&long, None, &theme, &[], |_| ());
        long_view.sync(&state, bounds);
        let long_width = long_view.with_paragraph(&state, 0, |p| p.min_bounds().width);

        assert!(long_width > short_width * 4.0);
    }

    #[test]
    fn viewport_dedup_is_revoked_on_root_owner_or_language_change() {
        let document = Document::from_str("fn main() {}\n", None, DocId(83));
        let theme = LumeTheme::builtin().remove(0);
        let state = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 100.0).into());
        let owner = lume_syntax::SyntaxWorker::default();
        let replacement = lume_syntax::SyntaxWorker::default();
        let mut messages = Vec::new();
        for (epoch, receiver, language) in [
            (7, owner.receiver(), lume_syntax::LanguageId::Rust),
            (7, owner.receiver(), lume_syntax::LanguageId::Rust),
            (8, owner.receiver(), lume_syntax::LanguageId::Rust),
            (8, replacement.receiver(), lume_syntax::LanguageId::Rust),
            (8, replacement.receiver(), lume_syntax::LanguageId::Toml),
        ] {
            let view = EditorView::from_snapshot(&document, None, &theme, &[], |_| (0, 0..0))
                .syntax_context(epoch, receiver, language)
                .on_syntax_viewport(|version, range| (version, range));
            let mut shell = iced::advanced::Shell::new(&mut messages);
            view.request_syntax_viewport(&state, bounds, &mut shell);
        }
        assert_eq!(messages.len(), 4);
    }

    #[test]
    fn revoked_lane_readmits_same_document_without_intermediate_frame() -> Result<(), std::io::Error>
    {
        let document = Document::from_str("fn main() {}\n", None, DocId(84));
        let theme = LumeTheme::builtin().remove(0);
        let state = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 100.0).into());
        let mut worker = lume_syntax::SyntaxWorker::default();
        let mut messages = Vec::new();
        for revoke in [false, false, true, false] {
            if revoke {
                worker.revoke()?;
            }
            let view = EditorView::from_snapshot(&document, None, &theme, &[], |_| (0, 0..0))
                .syntax_context(7, worker.receiver(), lume_syntax::LanguageId::Rust)
                .on_syntax_viewport(|version, range| (version, range));
            let mut shell = iced::advanced::Shell::new(&mut messages);
            view.request_syntax_viewport(&state, bounds, &mut shell);
        }
        assert_eq!(
            messages.len(),
            2,
            "A torna richiedibile dopo revoca senza frame B"
        );
        Ok(())
    }

    #[test]
    fn syntax_viewport_requests_are_event_driven_and_deduplicated() {
        let document = Document::from_str(&"fn main() {}\n".repeat(500), None, DocId(82));
        let theme = LumeTheme::builtin().remove(0);
        let state = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 100.0).into());
        let view = EditorView::from_snapshot(&document, None, &theme, &[], |_| (0, 0..0))
            .on_syntax_viewport(|version, range| (version, range));
        let mut messages = Vec::new();
        {
            let mut shell = iced::advanced::Shell::new(&mut messages);
            view.request_syntax_viewport(&state, bounds, &mut shell);
            view.request_syntax_viewport(&state, bounds, &mut shell);
        }
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].0, document.version());
        assert_eq!(messages[0].1.start, 0);
        state.scroll_y.set(300.0 * super::LINE_H);
        {
            let mut shell = iced::advanced::Shell::new(&mut messages);
            view.request_syntax_viewport(&state, bounds, &mut shell);
        }
        assert_eq!(messages.len(), 2);
        assert!(messages[1].1.start > messages[0].1.end);
        let syntax = SyntaxState::new(LanguageId::Rust, document.rope().slice(..)).unwrap();
        let snapshot = syntax
            .highlight_snapshot(
                document.rope().slice(..),
                document.version(),
                0..document.rope().len_bytes(),
            )
            .unwrap();
        let ready =
            EditorView::from_snapshot(&document, Some(&snapshot), &theme, &[], |_| (0, 0..0))
                .on_syntax_viewport(|version, range| (version, range));
        state.scroll_y.set(0.0);
        let mut shell = iced::advanced::Shell::new(&mut messages);
        ready.request_syntax_viewport(&state, bounds, &mut shell);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn draw_con_scroll_oltre_il_contenuto_non_va_in_panic() {
        use iced::advanced::Widget as _;
        let document = Document::from_str(&"riga\n".repeat(20), None, DocId(86));
        let theme = LumeTheme::builtin().remove(0);
        let mut view =
            EditorView::from_snapshot(&document, None, &theme, &[], |_| (0u64, 0usize..0usize));
        let mut tree = iced::advanced::widget::Tree::new(
            &view
                as &dyn iced::advanced::Widget<
                    (u64, std::ops::Range<usize>),
                    iced::Theme,
                    iced::Renderer,
                >,
        );
        let mut renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
            iced::Font::default(),
            iced::Pixels(16.0),
        ));
        let limits =
            iced::advanced::layout::Limits::new(iced::Size::ZERO, iced::Size::new(800.0, 600.0));
        let node = view.layout(&mut tree, &renderer, &limits);
        view.draw(
            &tree,
            &mut renderer,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style::default(),
            iced::advanced::Layout::new(&node),
            iced::mouse::Cursor::Unavailable,
            &Rectangle::new(iced::Point::ORIGIN, iced::Size::new(800.0, 600.0)),
        );
        // Scroll rimasto oltre la fine senza un cambio che lo riporti nei
        // limiti: il secondo disegno trova first_vis > last_vis.
        tree.state
            .downcast_ref::<EditorState>()
            .scroll_y
            .set(10_000.0 * super::LINE_H);
        view.draw(
            &tree,
            &mut renderer,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style::default(),
            iced::advanced::Layout::new(&node),
            iced::mouse::Cursor::Unavailable,
            &Rectangle::new(iced::Point::ORIGIN, iced::Size::new(800.0, 600.0)),
        );
    }

    #[test]
    fn syntax_viewport_is_requested_before_scroll_leaves_the_window() {
        let document = Document::from_str(&"fn main() {}\n".repeat(500), None, DocId(85));
        let theme = LumeTheme::builtin().remove(0);
        let state = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 100.0).into());
        let view = EditorView::from_snapshot(&document, None, &theme, &[], |_| (0, 0..0))
            .on_syntax_viewport(|version, range| (version, range));
        let mut messages = Vec::new();
        {
            let mut shell = iced::advanced::Shell::new(&mut messages);
            view.request_syntax_viewport(&state, bounds, &mut shell);
        }
        let rope = document.rope();
        let first_end = messages[0].1.end;
        // Visibile ancora dentro la finestra richiesta, ma a meno di
        // SYNTAX_PREFETCH righe dal suo bordo: la richiesta parte già ora.
        let edge_line = rope.byte_to_line(first_end);
        let last_visible = edge_line - super::SYNTAX_PREFETCH / 2;
        state
            .scroll_y
            .set((last_visible as f32 - 6.0) * super::LINE_H);
        {
            let mut shell = iced::advanced::Shell::new(&mut messages);
            view.request_syntax_viewport(&state, bounds, &mut shell);
        }
        assert_eq!(messages.len(), 2, "richiesta anticipata vicino al bordo");
        let ahead = rope.line_to_byte(last_visible + super::SYNTAX_PREFETCH);
        assert!(
            messages[1].1.end > ahead,
            "la nuova finestra copre il margine"
        );
    }

    #[test]
    fn snapshot_path_reads_tokens_without_queries_and_rejects_stale_revision() {
        let document = Document::from_str("fn main() {}\n", None, DocId(81));
        let syntax = SyntaxState::new(LanguageId::Rust, document.rope().slice(..)).unwrap();
        let snapshot = syntax
            .highlight_snapshot(
                document.rope().slice(..),
                document.version(),
                0..document.rope().len_bytes(),
            )
            .unwrap();
        let stale = syntax
            .highlight_snapshot(
                document.rope().slice(..),
                document.version() + 1,
                0..document.rope().len_bytes(),
            )
            .unwrap();
        let theme = LumeTheme::builtin().remove(0);
        let state = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 600.0).into());
        let view = EditorView::<()>::from_snapshot(&document, Some(&snapshot), &theme, &[], |_| ());
        view.sync(&state, bounds);
        assert!(!view.line_tokens(&state, 0, "fn main() {}").is_empty());
        assert_eq!(state.highlight_queries.get(), 0);
        let epoch = state.cache_epoch.get();
        let cloned = snapshot.clone();
        let same = EditorView::<()>::from_snapshot(&document, Some(&cloned), &theme, &[], |_| ());
        same.sync(&state, bounds);
        assert_eq!(state.cache_epoch.get(), epoch);
        let narrow = syntax
            .highlight_snapshot(document.rope().slice(..), document.version(), 0..0)
            .unwrap();
        let changed =
            EditorView::<()>::from_snapshot(&document, Some(&narrow), &theme, &[], |_| ());
        changed.sync(&state, bounds);
        assert_ne!(state.cache_epoch.get(), epoch);
        assert!(changed.line_tokens(&state, 0, "fn main() {}").is_empty());
        assert!(state.highlight_window.borrow().is_none());
        let view = EditorView::<()>::from_snapshot(&document, Some(&stale), &theme, &[], |_| ());
        view.sync(&state, bounds);
        assert!(view.line_tokens(&state, 0, "fn main() {}").is_empty());
        assert_eq!(state.highlight_queries.get(), 0);
    }

    #[test]
    fn equivalent_new_projection_reuses_shaped_line_but_changed_tokens_rebuild() {
        let document = Document::from_str("fn main() {}\n", None, DocId(85));
        let syntax = SyntaxState::new(LanguageId::Rust, document.rope().slice(..)).unwrap();
        let theme = LumeTheme::builtin().remove(0);
        let state = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 600.0).into());
        let full = 0..document.rope().len_bytes();
        let first = syntax
            .highlight_snapshot(document.rope().slice(..), document.version(), full.clone())
            .unwrap();
        let next = syntax
            .highlight_snapshot(document.rope().slice(..), document.version(), full)
            .unwrap();
        assert!(!first.same_projection(&next));
        let view = EditorView::<()>::from_snapshot(&document, Some(&first), &theme, &[], |_| ());
        view.sync(&state, bounds);
        view.with_paragraph(&state, 0, |_| ());
        let builds = state.paragraph_builds.get();
        let view = EditorView::<()>::from_snapshot(&document, Some(&next), &theme, &[], |_| ());
        view.sync(&state, bounds);
        view.with_paragraph(&state, 0, |_| ());
        assert_eq!(
            state.paragraph_builds.get(),
            builds,
            "token identici non richiedono shaping"
        );
        let empty = syntax
            .highlight_snapshot(document.rope().slice(..), document.version(), 0..0)
            .unwrap();
        let view = EditorView::<()>::from_snapshot(&document, Some(&empty), &theme, &[], |_| ());
        view.sync(&state, bounds);
        view.with_paragraph(&state, 0, |_| ());
        assert_eq!(state.paragraph_builds.get(), builds + 1);
        assert!(state.cache.borrow().get(&0).unwrap().tokens.is_empty());
    }

    #[test]
    fn changing_theme_rebuilds_paragraphs_without_requerying_tokens() {
        let document = Document::from_str("fn main() {}\n", None, DocId(7));
        let syntax = SyntaxState::new(LanguageId::Rust, document.rope().slice(..)).unwrap();
        let themes = LumeTheme::builtin();
        let state = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 600.0).into());

        let first = EditorView::<()>::new(&document, Some(&syntax), &themes[0], &[], |_| ());
        first.sync(&state, bounds);
        let first_epoch = state.cache_epoch.get();
        let first_queries = state.highlight_queries.get();
        assert!(state.highlight_window.borrow().is_some());

        let second = EditorView::<()>::new(&document, Some(&syntax), &themes[1], &[], |_| ());
        second.sync(&state, bounds);

        assert_ne!(state.cache_epoch.get(), first_epoch);
        assert_eq!(state.highlight_queries.get(), first_queries);
        assert!(state.highlight_window.borrow().is_some());
    }

    #[test]
    fn enabling_syntax_on_same_document_invalidates_plain_paragraphs() {
        let document = Document::from_str("fn main() {}\n", None, DocId(9));
        let syntax = SyntaxState::new(LanguageId::Rust, document.rope().slice(..)).unwrap();
        let theme = LumeTheme::builtin().remove(0);
        let state = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 600.0).into());

        let plain = EditorView::<()>::new(&document, None, &theme, &[], |_| ());
        plain.sync(&state, bounds);
        let plain_epoch = state.cache_epoch.get();
        assert!(state.highlight_window.borrow().is_none());

        let highlighted = EditorView::<()>::new(&document, Some(&syntax), &theme, &[], |_| ());
        highlighted.sync(&state, bounds);
        highlighted.with_paragraph(&state, 0, |_| ());

        assert_ne!(state.cache_epoch.get(), plain_epoch);
        assert_eq!(state.last_syntax.get(), Some(LanguageId::Rust));
        assert!(state.highlight_window.borrow().is_some());
    }

    #[test]
    #[ignore = "diagnostica release del costo paragraph, non budget end-to-end"]
    fn diagnostic_viewport_projection_shaping_cost() {
        use std::time::Instant;
        let text = "// padding ascii per corpus\n".repeat(45);
        let document = Document::from_str(&text, None, DocId(88));
        let theme = LumeTheme::builtin().remove(0);
        let worker = lume_syntax::SyntaxWorker::default();
        let snapshot = SyntaxState::new(LanguageId::Rust, document.rope().slice(..))
            .unwrap()
            .highlight_snapshot(
                document.rope().slice(..),
                document.version(),
                0..document.rope().len_bytes(),
            )
            .unwrap();
        let bounds = Rectangle::new((0.0, 0.0).into(), (1280.0, 820.0).into());
        for mode in ["plain", "highlight", "alternating"] {
            let state = EditorState::default();
            let mut elapsed = Vec::new();
            for sample in 0..101 {
                let projection = match mode {
                    "highlight" => Some(&snapshot),
                    "alternating" if sample % 2 == 0 => Some(&snapshot),
                    _ => None,
                };
                let view =
                    EditorView::<()>::from_snapshot(&document, projection, &theme, &[], |_| ())
                        .syntax_context(1, worker.receiver(), LanguageId::Rust);
                let start = Instant::now();
                view.sync(&state, bounds);
                for line in 0..45 {
                    view.with_paragraph(&state, line, |_| ());
                }
                if sample > 0 {
                    elapsed.push(start.elapsed().as_secs_f64() * 1000.0);
                }
            }
            elapsed.sort_by(f64::total_cmp);
            println!(
                "projection_cost mode={mode} rows=45 samples={} median_ms={:.6} p95_ms={:.6} builds={}",
                elapsed.len(),
                elapsed[50],
                elapsed[95],
                state.paragraph_builds.get()
            );
            assert!(!worker.is_started());
        }
    }

    #[test]
    fn pending_snapshot_preserves_plain_lines_and_removes_stale_colors() {
        use lume_core::{ChangeSet, Transaction};
        let mut document = Document::from_str("fn one() {}\n\n\n\n", None, DocId(87));
        let theme = LumeTheme::builtin().remove(0);
        let state = EditorState::default();
        let worker = lume_syntax::SyntaxWorker::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 600.0).into());
        let snapshot = SyntaxState::new(LanguageId::Rust, document.rope().slice(..))
            .unwrap()
            .highlight_snapshot(
                document.rope().slice(..),
                document.version(),
                0..document.rope().len_bytes(),
            )
            .unwrap();
        {
            let view =
                EditorView::<()>::from_snapshot(&document, Some(&snapshot), &theme, &[], |_| ())
                    .syntax_context(1, worker.receiver(), LanguageId::Rust);
            view.sync(&state, bounds);
            for line in 0..4 {
                view.with_paragraph(&state, line, |_| ());
            }
        }
        assert!(!state.cache.borrow().get(&0).unwrap().tokens.is_empty());
        let builds = state.paragraph_builds.get();
        let len = document.rope().len_chars();
        document.apply(Transaction::new(ChangeSet::insert_at(12, " ", len)));
        let view = EditorView::<()>::from_snapshot(&document, None, &theme, &[], |_| ())
            .syntax_context(1, worker.receiver(), LanguageId::Rust);
        view.sync(&state, bounds);
        for line in 0..4 {
            view.with_paragraph(&state, line, |_| ());
        }
        assert!(
            state
                .cache
                .borrow()
                .values()
                .all(|entry| entry.tokens.is_empty())
        );
        assert_eq!(
            state.paragraph_builds.get(),
            builds + 2,
            "ricostruire la riga editata e quella con colori obsoleti; riusare le righe vuote"
        );
        assert_eq!(state.last_syntax.get(), Some(LanguageId::Rust));
    }

    #[test]
    fn single_edit_with_new_snapshot_reuses_unchanged_paragraphs() {
        use lume_core::{ChangeSet, Transaction};
        let mut document =
            Document::from_str("fn one() {}\nfn two() {}\nfn three() {}\n", None, DocId(86));
        let theme = LumeTheme::builtin().remove(0);
        let state = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 600.0).into());
        let snapshot = SyntaxState::new(LanguageId::Rust, document.rope().slice(..))
            .unwrap()
            .highlight_snapshot(
                document.rope().slice(..),
                document.version(),
                0..document.rope().len_bytes(),
            )
            .unwrap();
        {
            let view =
                EditorView::<()>::from_snapshot(&document, Some(&snapshot), &theme, &[], |_| ());
            view.sync(&state, bounds);
            for line in 0..3 {
                view.with_paragraph(&state, line, |_| ());
            }
        }
        let builds = state.paragraph_builds.get();
        let len = document.rope().len_chars();
        document.apply(Transaction::new(ChangeSet::insert_at(15, "x", len)));
        let snapshot = SyntaxState::new(LanguageId::Rust, document.rope().slice(..))
            .unwrap()
            .highlight_snapshot(
                document.rope().slice(..),
                document.version(),
                0..document.rope().len_bytes(),
            )
            .unwrap();
        let view = EditorView::<()>::from_snapshot(&document, Some(&snapshot), &theme, &[], |_| ());
        view.sync(&state, bounds);
        for line in 0..3 {
            view.with_paragraph(&state, line, |_| ());
        }
        assert_eq!(state.paragraph_builds.get(), builds + 1);
        drop(view);
        let builds = state.paragraph_builds.get();
        let len = document.rope().len_chars();
        document.apply(Transaction::new(ChangeSet::insert_at(0, "/*", len)));
        let snapshot = SyntaxState::new(LanguageId::Rust, document.rope().slice(..))
            .unwrap()
            .highlight_snapshot(
                document.rope().slice(..),
                document.version(),
                0..document.rope().len_bytes(),
            )
            .unwrap();
        let view = EditorView::<()>::from_snapshot(&document, Some(&snapshot), &theme, &[], |_| ());
        view.sync(&state, bounds);
        for line in 0..3 {
            view.with_paragraph(&state, line, |_| ());
            let cache = state.cache.borrow();
            let entry = cache.get(&line).unwrap();
            assert_eq!(entry.tokens, view.line_tokens(&state, line, &entry.text));
        }
        assert_eq!(
            state.paragraph_builds.get(),
            builds + 3,
            "il commento cambia anche i token delle righe senza edit"
        );
    }

    #[test]
    fn single_line_edit_keeps_the_other_paragraphs_shaped() {
        use lume_core::{ChangeSet, Transaction};

        let mut document = Document::from_str("uno\ndue\ntre\n", None, DocId(11));
        let theme = LumeTheme::builtin().remove(0);
        let state = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 600.0).into());

        {
            let view = EditorView::<()>::new(&document, None, &theme, &[], |_| ());
            view.sync(&state, bounds);
            for line in 0..3 {
                view.with_paragraph(&state, line, |_| ());
            }
        }
        assert_eq!(state.paragraph_builds.get(), 3);
        let epoch_before = state.cache_epoch.get();

        // edit sulla riga 1: le righe 0 e 2 restano in cache (promosse)
        let len = document.rope().len_chars();
        document.apply(Transaction::new(ChangeSet::insert_at(4, "X", len)));
        let view = EditorView::<()>::new(&document, None, &theme, &[], |_| ());
        view.sync(&state, bounds);
        assert_ne!(state.cache_epoch.get(), epoch_before);
        {
            let cache = state.cache.borrow();
            assert!(cache.contains_key(&0) && cache.contains_key(&2));
            assert!(!cache.contains_key(&1), "la riga modificata viene scartata");
        }
        for line in 0..3 {
            view.with_paragraph(&state, line, |_| ());
        }
        assert_eq!(
            state.paragraph_builds.get(),
            4,
            "ricostruita solo la riga 1"
        );
        view.with_line(&state, 1, |_, text| assert_eq!(text, "Xdue"));

        drop(view);
        // newline inserito in testa: tutte le righe scalano di uno
        let len = document.rope().len_chars();
        document.apply(Transaction::new(ChangeSet::insert_at(0, "\n", len)));
        let view = EditorView::<()>::new(&document, None, &theme, &[], |_| ());
        view.sync(&state, bounds);
        // il sync ricostruisce solo la nuova riga 0 (quella del caret)
        let builds = state.paragraph_builds.get();
        view.with_line(&state, 3, |_, text| assert_eq!(text, "tre"));
        assert_eq!(
            state.paragraph_builds.get(),
            builds,
            "riga «tre» rimappata, non ricostruita"
        );
    }

    #[test]
    fn opening_a_block_comment_rebuilds_the_promoted_lines_below() {
        use lume_core::{ChangeSet, Transaction};
        use lume_syntax::InputEditRaw;

        let mut document = Document::from_str("\nfn main() {}\n", None, DocId(12));
        let mut syntax = SyntaxState::new(LanguageId::Rust, document.rope().slice(..)).unwrap();
        let theme = LumeTheme::builtin().remove(0);
        let state = EditorState::default();
        let bounds = Rectangle::new((0.0, 0.0).into(), (800.0, 600.0).into());

        {
            let view = EditorView::<()>::new(&document, Some(&syntax), &theme, &[], |_| ());
            view.sync(&state, bounds);
            view.with_paragraph(&state, 1, |_| ());
        }

        // «/*» sulla riga 0: la riga 1 è promossa dal remap ma i suoi token
        // cambiano (commento) → deve essere ricostruita alla prossima lettura
        let len = document.rope().len_chars();
        let edit = document.apply(Transaction::new(ChangeSet::insert_at(0, "/*", len)));
        syntax.apply_edit(
            &InputEditRaw {
                start_byte: edit.start_byte,
                old_end_byte: edit.old_end_byte,
                new_end_byte: edit.new_end_byte,
                start_point: edit.start_point,
                old_end_point: edit.old_end_point,
                new_end_point: edit.new_end_point,
            },
            document.rope().slice(..),
        );
        let view = EditorView::<()>::new(&document, Some(&syntax), &theme, &[], |_| ());
        view.sync(&state, bounds);
        // il sync ricostruisce la riga 0 (modificata, col caret); la 1 è promossa
        let builds = state.paragraph_builds.get();
        assert!(state.cache.borrow().contains_key(&1), "promossa dal remap");
        view.with_line(&state, 1, |_, text| assert_eq!(text, "fn main() {}"));
        assert_eq!(state.paragraph_builds.get(), builds + 1);
        assert!(
            state.cache.borrow()[&1]
                .tokens
                .iter()
                .all(|(_, class)| *class == TokenClass::Comment)
        );
    }

    #[test]
    fn viewport_uses_one_query_and_keeps_multiline_unicode_capture() {
        let mut source = String::from("/*\n");
        for _ in 0..218 {
            source.push_str("é漢 commento\n");
        }
        source.push_str("*/\n");
        let document = Document::from_str(&source, None, DocId(8));
        let syntax = SyntaxState::new(LanguageId::Rust, document.rope().slice(..)).unwrap();
        let theme = LumeTheme::builtin().remove(0);
        let state = EditorState::default();
        let view = EditorView::<()>::new(&document, Some(&syntax), &theme, &[], |_| ());

        view.prepare_highlights(&state, 140, 160);
        for line in 140..=160 {
            let text = view.line_text(line);
            let _ = view.line_spans(&state, line, &text);
        }

        assert_eq!(state.highlight_queries.get(), 1);
        let start = document.rope().line_to_byte(150);
        let end = document.rope().line_to_byte(151);
        assert!(
            state
                .highlight_window
                .borrow()
                .as_ref()
                .is_some_and(|window| {
                    window.tokens.iter().any(|(range, class)| {
                        *class == TokenClass::Comment && range.start < end && range.end > start
                    })
                })
        );
    }
}
