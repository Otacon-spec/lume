//! Editor di codice custom: widget `iced::advanced::Widget` che disegna un
//! `lume_core::Document` con gutter, highlighting tree-sitter, selezione,
//! caret e scroll virtualizzato. Il widget non muta mai il documento:
//! pubblica `EditorCommand` che l'app applica.

pub mod commands;
pub mod folding;
pub mod overlay;
pub mod popup;
pub mod state;
pub mod syntax_delivery;
pub mod syntax_subscription;
pub mod viewport;
pub mod widget;

pub use commands::{EditorCommand, Motion, PageDirection};
pub use folding::{FoldProjection, FoldProjectionRef, FoldSpan, is_foldable_start};
pub use overlay::{BlameAuthorLine, DiagnosticMark, EditorOverlay, MarkSeverity};
pub use popup::{CARD_MAX_COLS, EditorPopup, HoverCard, HoverLine, PopupRow, clip_text};
pub use state::EditorState;
pub use widget::{EditorView, FONT_SIZE, LINE_H};

use lume_core::Document;
use lume_syntax::{SyntaxState, TokenClass};
use lume_theme::LumeTheme;

/// Costruisce l'elemento editor pronto da montare in una view.
pub fn editor_view<'a, M: Clone + 'a>(
    doc: &'a Document,
    syntax: Option<&'a SyntaxState>,
    theme: &'a LumeTheme,
    highlights: &'a [std::ops::Range<usize>],
    overlay: EditorOverlay<'a>,
    on_command: impl Fn(EditorCommand) -> M + 'a,
) -> iced::Element<'a, M> {
    iced::Element::new(EditorView::new(doc, syntax, theme, highlights, on_command).overlay(overlay))
}

/// Colore di una classe di token secondo il tema (mapping del prototipo).
pub fn token_color(t: &LumeTheme, c: TokenClass) -> iced::Color {
    match c {
        TokenClass::Keyword => t.accent,
        TokenClass::Type => t.warn,
        TokenClass::Function => t.func,
        TokenClass::String => t.string,
        TokenClass::Number | TokenClass::Constant => t.num,
        TokenClass::Comment => t.comment,
        TokenClass::Attribute => t.tree,
        TokenClass::Operator
        | TokenClass::Punctuation
        | TokenClass::Variable
        | TokenClass::Property
        | TokenClass::Text => t.ui,
    }
}
