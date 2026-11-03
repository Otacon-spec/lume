//! Stato interno del widget editor, vissuto nel `widget::Tree` di iced.
//!
//! I campi usano `Cell`/`RefCell` perché `Widget::draw` riceve `&Tree`:
//! la cache dei `Paragraph` viene riempita in modo lazy anche durante il
//! draw (interior mutability, nessuna rientranza).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;

use iced::advanced::input_method::Preedit;
use iced::keyboard::Modifiers;
use lume_core::DocId;
use lume_syntax::{HighlightSnapshot, LanguageId, TokenClass};

/// Stato transitorio della composizione IME. Il documento cambia solo al commit.
#[derive(Debug, Default)]
pub struct ImeComposition {
    open: bool,
    preedit: Option<Preedit>,
}

impl ImeComposition {
    pub fn open(&mut self) {
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.preedit = None;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn preedit(
        &mut self,
        content: impl Into<String>,
        selection: Option<std::ops::Range<usize>>,
    ) {
        let content = content.into();
        self.preedit = (!content.is_empty()).then_some(Preedit {
            content,
            selection,
            text_size: None,
        });
    }

    pub fn current(&self) -> Option<&Preedit> {
        self.preedit.as_ref()
    }

    pub fn commit(&mut self, content: impl Into<String>) -> Option<String> {
        self.preedit = None;
        let content = content.into();
        (!content.is_empty()).then_some(content)
    }
}

/// Il `Paragraph` concreto del renderer di iced (cosmic-text).
pub type LineParagraph = <iced::Renderer as iced::advanced::text::Renderer>::Paragraph;

/// Entry della cache per-riga: il paragraph shaped, il testo della riga
/// (per hit-test e selezione senza riallocare a ogni frame) e i token
/// incorporati negli span (per validare l'entry dopo un remap).
pub struct CachedLine {
    pub paragraph: LineParagraph,
    pub text: String,
    /// Token della riga in byte relativi, come sono stati colorati.
    pub tokens: Vec<(Range<usize>, TokenClass)>,
    /// Generazione della cache a cui l'entry è valida.
    pub epoch: u64,
    /// Versione del documento alla quale i token sono stati verificati.
    pub verified_version: u64,
    /// I token incorporati sono stati confrontati con la proiezione corrente.
    pub tokens_verified: bool,
}

/// Entry di cache con generazione promuovibile (per [`remap_cache`]).
pub trait Versioned {
    fn set_version(&mut self, version: u64);
}

impl Versioned for CachedLine {
    fn set_version(&mut self, version: u64) {
        self.epoch = version;
    }
}

/// Invalidazione mirata della cache dopo un edit singolo: le righe nel
/// range toccato `first..=old_last` vengono scartate, quelle sopra vengono
/// promosse alla nuova generazione, quelle sotto vengono rimappate di
/// `delta` e promosse. Conservativa: una promozione sbagliata degrada solo
/// in un rebuild della riga (la validazione dei token è nel widget).
pub fn remap_cache<V: Versioned>(
    cache: &mut HashMap<usize, V>,
    first: usize,
    old_last: usize,
    delta: isize,
    new_version: u64,
) {
    let old: Vec<(usize, V)> = cache.drain().collect();
    for (line, mut entry) in old {
        if line < first {
            entry.set_version(new_version);
            cache.insert(line, entry);
        } else if line > old_last {
            let new_line = line as isize + delta;
            if new_line >= 0 {
                entry.set_version(new_version);
                cache.insert(new_line as usize, entry);
            }
        }
        // first..=old_last: righe toccate dall'edit, scartate
    }
}

/// Capture tree-sitter calcolate in un'unica query per una finestra di righe.
pub struct HighlightWindow {
    pub document: DocId,
    pub version: u64,
    pub language: LanguageId,
    pub first_line: usize,
    pub last_line: usize,
    pub tokens: Vec<(Range<usize>, TokenClass)>,
}

impl HighlightWindow {
    pub fn covers(
        &self,
        document: DocId,
        version: u64,
        language: LanguageId,
        first_line: usize,
        last_line: usize,
    ) -> bool {
        self.document == document
            && self.version == version
            && self.language == language
            && self.first_line <= first_line
            && self.last_line >= last_line
    }
}

/// Stato del widget: scroll, focus, drag e cache per-riga dei paragraph.
pub struct EditorState {
    /// Offset orizzontale del testo; il gutter resta fisso.
    pub scroll_x: Cell<f32>,
    /// Offset verticale di scroll in pixel (>= 0).
    pub scroll_y: Cell<f32>,
    /// Larghezza massima stimata del documento alla versione corrente.
    /// Composizione IME non ancora inserita nel documento.
    pub ime: RefCell<ImeComposition>,
    pub content_width: Cell<f32>,
    pub is_focused: Cell<bool>,
    pub last_focus_request: Cell<Option<(u64, bool)>>,
    pub is_dragging: Cell<bool>,
    /// Ultimi modificatori noti (per Shift+click).
    pub modifiers: Cell<Modifiers>,
    /// Documento e versione all'ultima sincronizzazione della cache.
    pub last_document: Cell<Option<DocId>>,
    pub last_version: Cell<u64>,
    /// Tema incorporato negli span colorati dei paragraph.
    pub last_theme: RefCell<Option<String>>,
    /// Linguaggio sintattico incorporato nei paragraph.
    pub last_syntax: Cell<Option<LanguageId>>,
    pub last_snapshot: RefCell<Option<HighlightSnapshot>>,
    pub last_fold_spans: RefCell<Vec<super::folding::FoldSpan>>,
    pub syntax_requested: RefCell<Option<(DocId, u64, Range<usize>)>>,
    pub syntax_context: RefCell<
        Option<(
            u64,
            u64,
            lume_syntax::SyntaxReceiver,
            lume_syntax::LanguageId,
        )>,
    >,
    /// Generazione della cache paragraph; cambia per documento/versione/tema.
    pub cache_epoch: Cell<u64>,
    /// Posizione head all'ultima sincronizzazione (per l'ensure-visible).
    pub last_cursor: Cell<usize>,
    /// Cache per-riga: riga -> paragraph shaped + testo + token.
    pub cache: RefCell<HashMap<usize, CachedLine>>,
    #[cfg(test)]
    pub paragraph_builds: Cell<usize>,
    /// Token per una finestra più ampia del viewport: una sola query tree-sitter.
    pub highlight_window: RefCell<Option<HighlightWindow>>,
    #[cfg(test)]
    pub highlight_queries: Cell<usize>,
    /// Ultima parola puntata pubblicata come `HoverAt` (limite alla fonte).
    pub hover_word: Cell<Option<usize>>,
    /// Prima riga visibile del popup di completamento.
    pub popup_top: Cell<usize>,
    /// Paragraph del preedit IME; deve sopravvivere al draw del renderer.
    pub ime_cache: RefCell<Option<LineParagraph>>,
    ///
    /// NOTA: i paragraph DEVONO restare vivi oltre il draw — il renderer
    /// tiene solo una `Weak` e li materializza a fine frame.
    /// Cache dei numeri di riga del gutter: riga -> paragraph.
    pub gutter_cache: RefCell<HashMap<usize, LineParagraph>>,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            scroll_x: Cell::new(0.0),
            scroll_y: Cell::new(0.0),
            content_width: Cell::new(0.0),
            is_focused: Cell::new(false),
            last_focus_request: Cell::new(None),
            is_dragging: Cell::new(false),
            ime: RefCell::new(ImeComposition::default()),
            modifiers: Cell::new(Modifiers::default()),
            last_document: Cell::new(None),
            last_version: Cell::new(u64::MAX),
            last_theme: RefCell::new(None),
            last_syntax: Cell::new(None),
            last_snapshot: RefCell::new(None),
            last_fold_spans: RefCell::new(Vec::new()),
            syntax_requested: RefCell::new(None),
            syntax_context: RefCell::new(None),
            cache_epoch: Cell::new(0),
            ime_cache: RefCell::new(None),
            hover_word: Cell::new(None),
            popup_top: Cell::new(0),
            last_cursor: Cell::new(0),
            cache: RefCell::new(HashMap::new()),
            #[cfg(test)]
            paragraph_builds: Cell::new(0),
            highlight_window: RefCell::new(None),
            #[cfg(test)]
            highlight_queries: Cell::new(0),
            gutter_cache: RefCell::new(HashMap::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{HashMap, ImeComposition, Versioned, remap_cache};

    // Il paragraph non è costruibile senza renderer: i test usano entry
    // (String, u64) — la funzione sotto test è la remap_cache REALE,
    // generica su Versioned.
    impl Versioned for (String, u64) {
        fn set_version(&mut self, v: u64) {
            self.1 = v;
        }
    }

    fn keys(cache: &HashMap<usize, (String, u64)>) -> Vec<(usize, &str, u64)> {
        let mut v: Vec<_> = cache
            .iter()
            .map(|(k, (t, ver))| (*k, t.as_str(), *ver))
            .collect();
        v.sort();
        v
    }

    fn seed() -> HashMap<usize, (String, u64)> {
        (0..6).map(|i| (i, (format!("r{i}"), 1))).collect()
    }

    #[test]
    fn edit_su_una_riga_scarta_solo_quella() {
        let mut c = seed();
        remap_cache(&mut c, 2, 2, 0, 2);
        assert_eq!(
            keys(&c),
            [
                (0, "r0", 2),
                (1, "r1", 2),
                (3, "r3", 2),
                (4, "r4", 2),
                (5, "r5", 2)
            ]
        );
    }

    #[test]
    fn newline_inserito_sposta_in_giu_le_righe_sotto() {
        let mut c = seed();
        remap_cache(&mut c, 1, 1, 1, 2);
        assert_eq!(
            keys(&c),
            [
                (0, "r0", 2),
                (3, "r2", 2),
                (4, "r3", 2),
                (5, "r4", 2),
                (6, "r5", 2)
            ]
        );
    }

    #[test]
    fn cancellazione_multiriga_rimappa_in_su_e_scarta_il_range() {
        let mut c = seed();
        remap_cache(&mut c, 1, 3, -2, 2);
        assert_eq!(keys(&c), [(0, "r0", 2), (2, "r4", 2), (3, "r5", 2)]);
    }

    #[test]
    fn delta_negativo_non_produce_righe_negative() {
        let mut c: HashMap<usize, (String, u64)> = [(0, ("r0".into(), 1)), (5, ("r5".into(), 1))]
            .into_iter()
            .collect();
        remap_cache(&mut c, 0, 0, -3, 2);
        assert_eq!(keys(&c), [(2, "r5", 2)]);
    }

    #[test]
    fn preedit_is_transient_and_commit_is_returned_once() {
        let mut ime = ImeComposition::default();
        ime.open();
        ime.preedit("nih", Some(1..3));

        let current = ime.current().unwrap();
        assert_eq!(current.content, "nih");
        assert_eq!(current.selection, Some(1..3));
        assert_eq!(ime.commit("日本"), Some("日本".to_owned()));
        assert!(ime.current().is_none());
        assert_eq!(ime.commit(""), None);
    }

    #[test]
    fn closing_ime_clears_uncommitted_text() {
        let mut ime = ImeComposition::default();
        ime.open();
        ime.preedit("x", None);
        ime.close();

        assert!(ime.current().is_none());
        assert!(!ime.is_open());
    }
}
