//! Stato sintattico di un buffer: parse iniziale, reparse incrementale
//! dopo un edit e interrogazione degli highlight su un intervallo di byte.

use std::{
    ops::{ControlFlow, Range},
    sync::atomic::{AtomicBool, Ordering},
};

use ropey::RopeSlice;
use streaming_iterator::StreamingIterator;
use tree_sitter::{
    InputEdit, Node, ParseOptions, Parser, Point, QueryCursor, QueryCursorOptions, TextProvider,
    Tree,
};

use crate::FoldRegion;
use crate::language::{LanguageId, spec};
use crate::token::TokenClass;

/// Edit "ponte" neutro: il chiamante non deve importare tree-sitter.
/// I point sono (riga, colonna-in-byte), 0-based.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InputEditRaw {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_point: (usize, usize),
    pub old_end_point: (usize, usize),
    pub new_end_point: (usize, usize),
}

impl InputEditRaw {
    fn to_ts(self) -> InputEdit {
        let pt = |(row, column): (usize, usize)| Point { row, column };
        InputEdit {
            start_byte: self.start_byte,
            old_end_byte: self.old_end_byte,
            new_end_byte: self.new_end_byte,
            start_position: pt(self.start_point),
            old_end_position: pt(self.old_end_point),
            new_end_position: pt(self.new_end_point),
        }
    }
}

pub struct SyntaxState {
    parser: Parser,
    tree: Tree,
    lang: LanguageId,
    /// Spec della lingua, risolta una volta alla creazione.
    spec: &'static crate::language::LanguageSpec,
}

/// Parse chunk-based sul rope: nessuna copia in String.
fn parse_rope(parser: &mut Parser, source: RopeSlice<'_>, old_tree: Option<&Tree>) -> Option<Tree> {
    let mut callback = |byte: usize, _pt: Point| -> &[u8] {
        if byte >= source.len_bytes() {
            return &[];
        }
        let (chunk, chunk_start, _, _) = source.chunk_at_byte(byte);
        chunk.as_bytes().get(byte - chunk_start..).unwrap_or(&[])
    };
    parser.parse_with_options(&mut callback, old_tree, None)
}

impl SyntaxState {
    /// Riparsa senza pubblicare un albero parziale o modificare quello corrente
    /// se il lavoro viene annullato. Il worker può riprovare lo stesso edit.
    pub fn apply_edit_cancellable(
        &mut self,
        edit: &InputEditRaw,
        source: RopeSlice<'_>,
        cancel: &AtomicBool,
    ) -> bool {
        self.apply_edit_checked(edit, source, |_| cancel.load(Ordering::Acquire))
    }

    fn apply_edit_checked(
        &mut self,
        edit: &InputEditRaw,
        source: RopeSlice<'_>,
        mut cancelled: impl FnMut(bool) -> bool,
    ) -> bool {
        if cancelled(false) {
            return false;
        }
        let mut previous = self.tree.clone();
        previous.edit(&edit.to_ts());
        // La copia modificata può costare millisecondi: una revoca arrivata
        // nel frattempo deve fermarsi prima di iniziare il parser.
        if cancelled(false) {
            return false;
        }
        let mut progress = |_: &tree_sitter::ParseState| {
            if cancelled(true) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let mut read = |byte: usize, _: Point| -> &[u8] {
            if byte >= source.len_bytes() {
                return &[];
            }
            let (chunk, start, _, _) = source.chunk_at_byte(byte);
            chunk.as_bytes().get(byte - start..).unwrap_or(&[])
        };
        let tree = self.parser.parse_with_options(
            &mut read,
            Some(&previous),
            Some(ParseOptions::new().progress_callback(&mut progress)),
        );
        if let Some(tree) = tree.filter(|_| !cancelled(false)) {
            self.tree = tree;
            true
        } else {
            // tree-sitter conserva lo stato della parse interrotta per resume:
            // la revisione successiva deve invece iniziare una nuova richiesta.
            self.parser.reset();
            false
        }
    }

    /// Materializza solo i token della finestra richiesta. Parse e query devono
    /// usare lo stesso testo; la revisione è assegnata dal proprietario worker.
    pub fn highlight_snapshot(
        &self,
        source: RopeSlice<'_>,
        revision: u64,
        covered: Range<usize>,
    ) -> Option<crate::HighlightSnapshot> {
        self.highlight_snapshot_cancellable(source, revision, covered, &AtomicBool::new(false))
    }

    /// Snapshot prodotto interamente sul worker; l'annullamento scarta anche
    /// risultati raccolti prima che la query fosse interrotta.
    pub fn highlight_snapshot_cancellable(
        &self,
        source: RopeSlice<'_>,
        revision: u64,
        covered: Range<usize>,
        cancel: &AtomicBool,
    ) -> Option<crate::HighlightSnapshot> {
        if cancel.load(Ordering::Acquire) {
            return None;
        }
        if covered.start > covered.end || covered.end > source.len_bytes() {
            return None;
        }
        let tokens = if covered.is_empty() {
            Vec::new()
        } else {
            self.highlights_cancellable(source, covered.clone(), cancel)?
                .into_iter()
                .filter_map(|(range, class)| {
                    let clipped = range.start.max(covered.start)..range.end.min(covered.end);
                    (!clipped.is_empty()).then_some((clipped, class))
                })
                .collect()
        };
        if cancel.load(Ordering::Acquire) {
            return None;
        }
        let (folding, folding_rejected) = self.folding_regions_cancellable(source, cancel);
        // il produttore può essere stato interrotto: il risultato parziale
        // non si pubblica mai.
        if cancel.load(Ordering::Acquire) {
            return None;
        }
        Some(crate::HighlightSnapshot {
            lang: self.lang,
            revision,
            covered,
            tokens: tokens.into(),
            folding: folding.into(),
            folding_rejected,
        })
    }

    pub fn folding_regions(&self, source: RopeSlice<'_>) -> (Vec<FoldRegion>, bool) {
        self.folding_regions_checked(source, |_| false)
    }

    pub fn folding_regions_cancellable(
        &self,
        source: RopeSlice<'_>,
        cancel: &AtomicBool,
    ) -> (Vec<FoldRegion>, bool) {
        self.folding_regions_checked(source, |_| cancel.load(Ordering::Acquire))
    }

    fn folding_regions_checked(
        &self,
        source: RopeSlice<'_>,
        cancelled: impl Fn(bool) -> bool,
    ) -> (Vec<FoldRegion>, bool) {
        let Some(query) = self.spec.folding_query.as_ref() else {
            return (Vec::new(), false);
        };
        if cancelled(false) {
            return (Vec::new(), false);
        }
        let mut cursor = QueryCursor::new();
        let mut raw: Vec<FoldRegion> = Vec::new();
        let mut progress = |_: &tree_sitter::QueryCursorState| {
            if cancelled(true) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let mut captures = cursor.captures_with_options(
            query,
            self.tree.root_node(),
            RopeProvider(source),
            QueryCursorOptions::new().progress_callback(&mut progress),
        );
        while let Some((m, capture_index)) = captures.next() {
            if cancelled(false) {
                return (Vec::new(), false);
            }
            let Some(capture) = m.captures.get(*capture_index) else {
                continue;
            };
            let node = capture.node;
            let start_line = node.start_position().row;
            // La regola della specifica: se il nodo finisce a colonna zero,
            // quella riga non fa parte della regione. `saturating_sub`
            // copre il caso (impossibile in pratica) del nodo che inizia
            // alla prima riga e finisce alla prima riga.
            let end_line = if node.end_position().column == 0 {
                node.end_position().row.saturating_sub(1)
            } else {
                node.end_position().row
            };
            if end_line > start_line {
                raw.push(FoldRegion {
                    start_line,
                    end_line,
                });
            }
        }
        if cancelled(false) {
            return (Vec::new(), false);
        }
        normalize_fold_regions(raw, || cancelled(false))
    }

    /// Parse completo iniziale. `None` se la spec della lingua non è
    /// disponibile (query di highlight non compilabile): il chiamante
    /// mostra il file senza highlight.
    pub fn new(lang: LanguageId, source: RopeSlice<'_>) -> Option<Self> {
        Self::new_cancellable(lang, source, &AtomicBool::new(false))
    }

    /// Parse iniziale cooperativa: nessuno stato viene restituito dopo cancel.
    pub fn new_cancellable(
        lang: LanguageId,
        source: RopeSlice<'_>,
        cancel: &AtomicBool,
    ) -> Option<Self> {
        Self::new_checked(lang, source, |_| cancel.load(Ordering::Acquire))
    }

    fn new_checked(
        lang: LanguageId,
        source: RopeSlice<'_>,
        cancelled: impl Fn(bool) -> bool,
    ) -> Option<Self> {
        if cancelled(false) {
            return None;
        }
        let sp = spec(lang)?;
        let mut parser = Parser::new();
        parser.set_language(&sp.language).ok()?;
        let mut progress = |_: &tree_sitter::ParseState| {
            if cancelled(true) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let mut read = |byte: usize, _: Point| -> &[u8] {
            if byte >= source.len_bytes() {
                return &[];
            }
            let (chunk, start, _, _) = source.chunk_at_byte(byte);
            chunk.as_bytes().get(byte - start..).unwrap_or(&[])
        };
        let tree = parser.parse_with_options(
            &mut read,
            None,
            Some(ParseOptions::new().progress_callback(&mut progress)),
        )?;
        if cancelled(false) {
            return None;
        }
        Some(Self {
            parser,
            tree,
            lang,
            spec: sp,
        })
    }

    pub fn lang(&self) -> LanguageId {
        self.lang
    }

    /// Albero corrente (utile per test/debug).
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// Registra l'edit sull'albero e riparsa in modo incrementale.
    /// `source` è il testo GIÀ modificato.
    pub fn apply_edit(&mut self, edit: &InputEditRaw, source: RopeSlice<'_>) {
        self.tree.edit(&edit.to_ts());
        if let Some(tree) = parse_rope(&mut self.parser, source, Some(&self.tree)) {
            self.tree = tree;
        }
    }

    /// Highlight per l'intervallo di byte richiesto.
    ///
    /// Risoluzione degli overlap: le capture vengono ordinate per
    /// (start crescente, end decrescente), così ogni span annidato segue il
    /// suo contenitore; una pila di span "aperti" emette i frammenti del
    /// contenitore attorno ai figli. Vince quindi la capture più interna
    /// (o l'ultima, a parità di range), e il contenitore riprende dopo di
    /// essa. Il risultato è ordinato e privo di sovrapposizioni.
    pub fn highlights(
        &self,
        source: RopeSlice<'_>,
        byte_range: Range<usize>,
    ) -> Vec<(Range<usize>, TokenClass)> {
        self.highlights_checked(source, byte_range, |_| false)
            .unwrap_or_default()
    }

    /// Nessun risultato parziale viene pubblicato dopo annullamento.
    pub fn highlights_cancellable(
        &self,
        source: RopeSlice<'_>,
        byte_range: Range<usize>,
        cancel: &AtomicBool,
    ) -> Option<Vec<(Range<usize>, TokenClass)>> {
        self.highlights_checked(source, byte_range, |_| cancel.load(Ordering::Acquire))
    }

    fn highlights_checked(
        &self,
        source: RopeSlice<'_>,
        byte_range: Range<usize>,
        cancelled: impl Fn(bool) -> bool,
    ) -> Option<Vec<(Range<usize>, TokenClass)>> {
        if cancelled(false) {
            return None;
        }
        let sp = self.spec;
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(byte_range);

        // Raccolta delle capture grezze (streaming iterator da tree-sitter 0.25+).
        let mut raw: Vec<(Range<usize>, TokenClass)> = Vec::new();
        let mut progress = |_: &tree_sitter::QueryCursorState| {
            if cancelled(true) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let mut captures = cursor.captures_with_options(
            &sp.query,
            self.tree.root_node(),
            RopeProvider(source),
            QueryCursorOptions::new().progress_callback(&mut progress),
        );
        while let Some((m, capture_index)) = captures.next() {
            if cancelled(false) {
                return None;
            }
            let Some(capture) = m.captures.get(*capture_index) else {
                continue;
            };
            let range = capture.node.byte_range();
            if range.is_empty() {
                continue;
            }
            let Some(&class) = sp.capture_classes.get(capture.index as usize) else {
                continue;
            };
            raw.push((range, class));
        }

        if cancelled(false) {
            return None;
        }

        // Ordinamento stabile: a parità di (start, end) resta l'ordine di
        // emissione, quindi l'ultima capture sovrascrive la precedente.
        let raw = sort_captures(raw, &|| cancelled(false))?;

        let mut out: Vec<(Range<usize>, TokenClass)> = Vec::new();
        let mut stack: Vec<(Range<usize>, TokenClass)> = Vec::new();
        let mut cursor_byte = 0usize; // ultima posizione già emessa

        let emit = |out: &mut Vec<(Range<usize>, TokenClass)>,
                    start: usize,
                    end: usize,
                    class: TokenClass| {
            if start < end {
                out.push((start..end, class));
            }
        };

        for (range, class) in raw {
            if cancelled(false) {
                return None;
            }
            // Chiudi gli span aperti che terminano prima del nuovo.
            while stack.last().is_some_and(|top| top.0.end <= range.start) {
                let Some((r, c)) = stack.pop() else { break };
                emit(&mut out, cursor_byte.max(r.start), r.end, c);
                cursor_byte = cursor_byte.max(r.end);
            }
            // Emetti il pezzo del contenitore che precede il nuovo span.
            if let Some(top) = stack.last() {
                emit(&mut out, cursor_byte.max(top.0.start), range.start, top.1);
            }
            cursor_byte = cursor_byte.max(range.start);
            // Range identico: l'ultima capture rimpiazza la precedente.
            if stack.last().is_some_and(|top| top.0 == range) {
                stack.pop();
            }
            stack.push((range, class));
        }
        // Svuota la pila (i più interni escono per primi).
        while let Some((r, c)) = stack.pop() {
            emit(&mut out, cursor_byte.max(r.start), r.end, c);
            cursor_byte = cursor_byte.max(r.end);
        }

        if cancelled(false) { None } else { Some(out) }
    }
}

/// Mantiene l'ordine stabile delle capture, con checkpoint tra blocchi limitati.
fn sort_captures(
    mut raw: Vec<(Range<usize>, TokenClass)>,
    cancelled: &impl Fn() -> bool,
) -> Option<Vec<(Range<usize>, TokenClass)>> {
    const CHUNK: usize = 1024;
    let compare = |a: &(Range<usize>, TokenClass), b: &(Range<usize>, TokenClass)| {
        a.0.start.cmp(&b.0.start).then(b.0.end.cmp(&a.0.end))
    };
    if cancelled() {
        return None;
    }
    if raw.len() <= CHUNK {
        raw.sort_by(compare);
        return if cancelled() { None } else { Some(raw) };
    }
    let mut ascending = true;
    let mut descending = true;
    for (index, pair) in raw.windows(2).enumerate() {
        if index % 256 == 0 && cancelled() {
            return None;
        }
        let Some((a, rest)) = pair.split_first() else {
            continue;
        };
        let Some(b) = rest.first() else { continue };
        let order = compare(a, b);
        ascending &= order.is_le();
        // Solo ordine strettamente inverso: invertire range uguali
        // cambierebbe la precedenza stabile delle capture.
        descending &= order.is_gt();
        if !ascending && !descending {
            break;
        }
    }
    if descending {
        let len = raw.len();
        for index in 0..len / 2 {
            if index % 256 == 0 && cancelled() {
                return None;
            }
            raw.swap(index, len - 1 - index);
        }
    }
    if ascending || descending {
        return if cancelled() { None } else { Some(raw) };
    }
    for run in raw.chunks_mut(CHUNK) {
        if cancelled() {
            return None;
        }
        run.sort_by(compare);
    }
    let mut scratch = Vec::with_capacity(raw.len());
    let mut width = CHUNK;
    while width < raw.len() {
        scratch.clear();
        for pair in raw.chunks(width.saturating_mul(2)) {
            let (mut left, mut right) = pair.split_at(width.min(pair.len()));
            while let (Some((a, rest_a)), Some((b, rest_b))) =
                (left.split_first(), right.split_first())
            {
                if scratch.len() % 256 == 0 && cancelled() {
                    return None;
                }
                if compare(a, b).is_le() {
                    scratch.push(a.clone());
                    left = rest_a;
                } else {
                    scratch.push(b.clone());
                    right = rest_b;
                }
            }
            // Una sola coda è non vuota; copie a blocchi mantengono i checkpoint.
            for tail in [left, right] {
                for block in tail.chunks(256) {
                    if cancelled() {
                        return None;
                    }
                    scratch.extend_from_slice(block);
                }
            }
        }
        std::mem::swap(&mut raw, &mut scratch);
        width = width.saturating_mul(2);
    }
    if cancelled() { None } else { Some(raw) }
}

/// `TextProvider` chunk-based sul rope, usato dalle query
/// (serve a valutare i predicati testuali tipo `#match?`).
struct RopeProvider<'a>(RopeSlice<'a>);

struct ChunkBytes<'a> {
    chunks: ropey::iter::Chunks<'a>,
}

impl<'a> Iterator for ChunkBytes<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        self.chunks.next().map(str::as_bytes)
    }
}

impl<'a> TextProvider<&'a [u8]> for RopeProvider<'a> {
    type I = ChunkBytes<'a>;

    fn text(&mut self, node: Node<'_>) -> Self::I {
        let end = node.end_byte().min(self.0.len_bytes());
        let start = node.start_byte().min(end);
        ChunkBytes {
            chunks: self.0.byte_slice(start..end).chunks(),
        }
    }
}

/// Normalizza regioni già multilinea. Dopo l'ordinamento, gli intervalli
/// accettati che contengono l'inizio corrente formano una pila: basta il
/// contenitore più interno per rilevare un incrocio. Ogni fine entra/esce
/// dalla pila una volta; i blocchi fratelli non si confrontano fra loro.
fn normalize_fold_regions(
    mut raw: Vec<FoldRegion>,
    cancelled: impl Fn() -> bool,
) -> (Vec<FoldRegion>, bool) {
    if cancelled() {
        return (Vec::new(), false);
    }
    if raw.len() > 65_536 {
        return (Vec::new(), true);
    }
    raw.sort_by(|a, b| {
        a.start_line
            .cmp(&b.start_line)
            .then(b.end_line.cmp(&a.end_line))
    });
    raw.dedup();
    let mut kept: Vec<FoldRegion> = Vec::new();
    let mut ends = Vec::new();
    for region in raw {
        if cancelled() {
            return (Vec::new(), false);
        }
        while ends.last().is_some_and(|end| *end < region.start_line) {
            ends.pop();
        }
        if kept
            .last()
            .is_some_and(|last| last.start_line == region.start_line)
            || ends.last().is_some_and(|end| region.end_line > *end)
        {
            continue;
        }
        ends.push(region.end_line);
        kept.push(region);
    }
    if kept.len() > 16_384 {
        return (Vec::new(), true);
    }
    (kept, false)
}

#[cfg(test)]
mod folding_normalization_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    // Oracolo precedente all'ottimizzazione: confronto con tutti gli
    // intervalli tenuti. Si conserva solo nei test per l'equivalenza.
    fn precedente(mut raw: Vec<FoldRegion>) -> Vec<FoldRegion> {
        raw.sort_by(|a, b| {
            a.start_line
                .cmp(&b.start_line)
                .then(b.end_line.cmp(&a.end_line))
        });
        raw.dedup();
        let mut kept: Vec<FoldRegion> = Vec::new();
        for region in raw {
            if !kept.iter().any(|last| {
                region.start_line == last.start_line
                    || (region.start_line > last.start_line
                        && region.start_line <= last.end_line
                        && region.end_line > last.end_line)
            }) {
                kept.push(region);
            }
        }
        kept
    }

    #[test]
    fn normalizzazione_equivale_all_oracolo_per_incroci_annidamenti_e_stessi_inizi() {
        let possibili: Vec<_> = (0..6)
            .flat_map(|start_line| {
                ((start_line + 1)..7).map(move |end_line| FoldRegion {
                    start_line,
                    end_line,
                })
            })
            .collect();
        // Tutte le sequenze di quattro intervalli: include ordine inverso,
        // duplicati, contenitori già terminati e incroci con antenati.
        for a in &possibili {
            for b in &possibili {
                for c in &possibili {
                    for d in &possibili {
                        let raw = vec![*a, *b, *c, *d];
                        let (actual, rejected) = normalize_fold_regions(raw.clone(), || false);
                        assert!(!rejected);
                        assert_eq!(actual, precedente(raw));
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "diagnostica release appaiata della normalizzazione, non latenza UI"]
    fn diagnostica_normalizzazione_appaiata() {
        use std::{hint::black_box, time::Instant};
        for count in [1_000, 4_000, 16_000] {
            let raw: Vec<_> = (0..count)
                .flat_map(|n| {
                    let region = FoldRegion {
                        start_line: n * 3,
                        end_line: n * 3 + 2,
                    };
                    [region, region]
                })
                .collect();
            for pair in 0..10 {
                for legacy in if pair % 2 == 0 {
                    [true, false]
                } else {
                    [false, true]
                } {
                    let input = raw.clone();
                    let start = Instant::now();
                    let output = if legacy {
                        precedente(input)
                    } else {
                        normalize_fold_regions(input, || false).0
                    };
                    let nanos = start.elapsed().as_nanos();
                    assert_eq!(output.len(), count);
                    black_box(output);
                    println!("normalizzazione,{count},{pair},{legacy},{nanos}");
                }
            }
        }
    }

    #[test]
    fn cancellare_la_normalizzazione_scarta_tutte_le_regioni() {
        let calls = std::cell::Cell::new(0);
        let raw = (0..100)
            .map(|n| FoldRegion {
                start_line: n * 3,
                end_line: n * 3 + 1,
            })
            .collect();
        let (regions, rejected) = normalize_fold_regions(raw, || {
            calls.set(calls.get() + 1);
            calls.get() > 4
        });
        assert!(regions.is_empty());
        assert!(!rejected);
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use ropey::Rope;

    #[test]
    #[ignore = "diagnostica release sort capture, non budget prodotto"]
    fn capture_sort_cost() -> Result<(), &'static str> {
        use std::{hint::black_box, time::Instant};
        for count in [85, 8192, 131072] {
            for order in ["ordered", "reversed", "mixed"] {
                let mut raw: Vec<_> = (0..count)
                    .map(|n| (n * 8..n * 8 + 7, TokenClass::Comment))
                    .collect();
                if order == "reversed" {
                    raw.reverse();
                } else if order == "mixed" {
                    // Permutazione deterministica: nessuna dipendenza da RNG o seed esterni.
                    for index in 0..count {
                        raw.swap(index, (index * 7919 + 17) % count);
                    }
                }
                for mode in ["legacy", "cancellable"] {
                    let mut samples = Vec::new();
                    for sample in 0..31 {
                        let mut input = black_box(raw.clone());
                        let start = Instant::now();
                        let output = if mode == "legacy" {
                            input.sort_by(|a, b| {
                                a.0.start.cmp(&b.0.start).then(b.0.end.cmp(&a.0.end))
                            });
                            input
                        } else {
                            sort_captures(input, &|| false).ok_or("sort")?
                        };
                        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                        assert_eq!(black_box(output.len()), count);
                        if sample > 0 {
                            samples.push(elapsed);
                        }
                    }
                    samples.sort_by(f64::total_cmp);
                    eprintln!(
                        "CAPTURE_SORT count={count} order={order} mode={mode} median_ms={:.6}",
                        samples.get(15).ok_or("median")?
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn capture_sort_fast_paths_observe_cancellation() {
        use std::cell::Cell;
        const COUNT: usize = 16384;
        // 65 controlli classificano l'ordine; il 66esimo entra nel reverse.
        for (reverse, stop_at) in [(false, 4), (true, 4), (true, 66), (true, 70)] {
            let mut raw: Vec<_> = (0..COUNT)
                .map(|n| (n..n + 3, TokenClass::Comment))
                .collect();
            if reverse {
                raw.reverse();
            }
            let calls = Cell::new(0);
            assert!(
                sort_captures(raw, &|| {
                    calls.set(calls.get() + 1);
                    calls.get() >= stop_at
                })
                .is_none()
            );
            assert_eq!(calls.get(), stop_at);
        }
    }

    #[test]
    fn capture_sort_cancellation_during_work_discards_result() {
        use std::cell::Cell;
        for stop_at in [4, 64] {
            let calls = Cell::new(0);
            let mut raw: Vec<_> = (0..16384)
                .rev()
                .map(|n| (n..n + 3, TokenClass::Comment))
                .collect();
            raw.swap(0, 1); // Esclude fast path ordinato/inverso: prova merge.
            let result = sort_captures(raw, &|| {
                calls.set(calls.get() + 1);
                calls.get() >= stop_at
            });
            assert!(result.is_none());
            assert_eq!(calls.get(), stop_at);
        }
    }

    #[test]
    fn capture_sort_preserves_stable_equal_range_precedence() {
        for count in [0, 1, 512, 513, 8193] {
            let raw: Vec<_> = (0..count)
                .rev()
                .flat_map(|n| {
                    [
                        (n % 47..n % 47 + n % 5 + 1, TokenClass::Comment),
                        (n % 47..n % 47 + n % 5 + 1, TokenClass::Keyword),
                    ]
                })
                .collect();
            let mut oracle = raw.clone();
            oracle.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(b.0.end.cmp(&a.0.end)));
            assert_eq!(
                sort_captures(oracle.clone(), &|| false),
                Some(oracle.clone())
            );
            assert_eq!(sort_captures(raw, &|| false), Some(oracle));
            let unique: Vec<_> = (0..count)
                .map(|n| (n * 8..n * 8 + 7, TokenClass::Comment))
                .collect();
            let reversed: Vec<_> = unique.iter().rev().cloned().collect();
            assert_eq!(sort_captures(reversed, &|| false), Some(unique));
        }
    }

    #[test]
    fn cancellation_after_tree_edit_skips_parser_and_preserves_previous_tokens()
    -> Result<(), &'static str> {
        use std::cell::Cell;
        let old = Rope::from_str(&("fn main() {}\n\n".to_owned() + &"// tail\n".repeat(32_000)));
        let mut source = old.clone();
        source.insert(13, "a");
        let mut state = SyntaxState::new(LanguageId::Rust, old.slice(..)).ok_or("seed")?;
        let tokens = state.highlights(old.slice(..), 0..2048);
        let checks = Cell::new(0);
        let progress_seen = Cell::new(false);
        let result = state.apply_edit_checked(
            &InputEditRaw {
                start_byte: 13,
                old_end_byte: 13,
                new_end_byte: 14,
                start_point: (1, 0),
                old_end_point: (1, 0),
                new_end_point: (1, 1),
            },
            source.slice(..),
            |progress| {
                if progress {
                    progress_seen.set(true);
                    false
                } else {
                    checks.set(checks.get() + 1);
                    checks.get() >= 2
                }
            },
        );
        assert!(!result);
        assert!(
            !progress_seen.get(),
            "parse non deve iniziare dopo la revoca"
        );
        assert_eq!(state.tree.root_node().end_byte(), old.len_bytes());
        assert_eq!(state.highlights(old.slice(..), 0..2048), tokens);
        assert!(state.apply_edit_cancellable(
            &InputEditRaw {
                start_byte: 13,
                old_end_byte: 13,
                new_end_byte: 14,
                start_point: (1, 0),
                old_end_point: (1, 0),
                new_end_point: (1, 1),
            },
            source.slice(..),
            &AtomicBool::new(false)
        ));
        let oracle = SyntaxState::new(LanguageId::Rust, source.slice(..)).ok_or("oracle")?;
        assert_eq!(
            state.highlights(source.slice(..), 0..2048),
            oracle.highlights(source.slice(..), 0..2048)
        );
        Ok(())
    }

    #[test]
    #[ignore = "diagnostica release tree edit, non budget prodotto"]
    fn large_tree_edit_and_drop_cost() -> Result<(), &'static str> {
        use std::time::Instant;
        for lines in [40_000, 320_000] {
            let source = Rope::from_str(
                &("fn main() {}\n\n".to_owned() + &"// padding ascii per corpus\n".repeat(lines)),
            );
            let state = SyntaxState::new(LanguageId::Rust, source.slice(..)).ok_or("seed")?;
            let edit = InputEditRaw {
                start_byte: 13,
                old_end_byte: 13,
                new_end_byte: 14,
                start_point: (1, 0),
                old_end_point: (1, 0),
                new_end_point: (1, 1),
            };
            let start = Instant::now();
            let mut previous = state.tree.clone();
            let clone_us = start.elapsed().as_micros();
            let start = Instant::now();
            previous.edit(&edit.to_ts());
            let edit_us = start.elapsed().as_micros();
            assert_eq!(previous.root_node().end_byte(), source.len_bytes() + 1);
            assert_eq!(state.tree.root_node().end_byte(), source.len_bytes());
            let start = Instant::now();
            drop(previous);
            let edited_drop_us = start.elapsed().as_micros();
            let start = Instant::now();
            drop(state);
            let state_drop_us = start.elapsed().as_micros();
            eprintln!(
                "TREE_EDIT bytes={} clone_us={clone_us} edit_us={edit_us} edited_drop_us={edited_drop_us} state_drop_us={state_drop_us} release={}",
                source.len_bytes(),
                !cfg!(debug_assertions)
            );
        }
        Ok(())
    }

    #[test]
    #[ignore = "diagnostica release cancellazione viva, non budget prodotto"]
    fn live_parse_query_cancellation_latency() -> Result<(), Box<dyn std::error::Error>> {
        use std::{
            sync::{Arc, mpsc},
            time::{Duration, Instant},
        };
        for (phase, progress_at) in [
            ("parse", 1),
            ("parse", 64),
            ("query", 1),
            ("query", 64),
            ("edit", 1),
            ("edit", 64),
        ] {
            let source = Rope::from_str(
                &("fn main() {}\na\n".to_owned()
                    + &"// padding ascii per corpus\n".repeat(320_000)),
            );
            let cancel = Arc::new(AtomicBool::new(false));
            let worker_cancel = cancel.clone();
            let (ready_tx, ready_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            std::thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
                let handle = scope.spawn(move || {
                    let progress_count = std::sync::atomic::AtomicUsize::new(0);
                    let cancelled = |progress| {
                        if progress
                            && progress_count.fetch_add(1, Ordering::AcqRel) + 1 == progress_at
                        {
                            let _ = ready_tx.send(());
                        }
                        worker_cancel.load(Ordering::Acquire)
                    };
                    if phase == "parse" {
                        let result =
                            SyntaxState::new_checked(LanguageId::Rust, source.slice(..), cancelled);
                        let _ = done_tx.send(result.is_none());
                    } else if phase == "edit" {
                        let mut old = source.clone();
                        old.remove(13..14);
                        let Some(mut state) = SyntaxState::new(LanguageId::Rust, old.slice(..))
                        else {
                            let _ = done_tx.send(false);
                            return;
                        };
                        let before = state.highlights(old.slice(..), 0..2048);
                        let edit = InputEditRaw {
                            start_byte: 13,
                            old_end_byte: 13,
                            new_end_byte: 14,
                            start_point: (1, 0),
                            old_end_point: (1, 0),
                            new_end_point: (1, 1),
                        };
                        let published =
                            state.apply_edit_checked(&edit, source.slice(..), cancelled);
                        let _ = done_tx.send(!published);
                        assert_eq!(state.tree.root_node().end_byte(), old.len_bytes());
                        assert_eq!(state.highlights(old.slice(..), 0..2048), before);
                    } else {
                        let state = SyntaxState::new(LanguageId::Rust, source.slice(..));
                        let result = state.as_ref().and_then(|state| {
                            state.highlights_checked(source.slice(..), 0..2048, cancelled)
                        });
                        let _ = done_tx.send(result.is_none());
                        drop(state);
                    }
                });
                let ready = ready_rx.recv_timeout(Duration::from_secs(5));
                let start = Instant::now();
                cancel.store(true, Ordering::Release);
                ready?;
                let discarded = done_rx.recv_timeout(Duration::from_secs(5))?;
                let return_ms = start.elapsed().as_secs_f64() * 1000.0;
                handle
                    .join()
                    .map_err(|_| std::io::Error::other("diagnostic worker panicked"))?;
                let join_ms = start.elapsed().as_secs_f64() * 1000.0;
                assert!(discarded, "risultato cancellato non deve essere pubblicato");
                eprintln!(
                    "LIVE_CANCEL phase={phase} progress_at={progress_at} return_ms={return_ms:.6} joined_ms={join_ms:.6} release={}",
                    !cfg!(debug_assertions)
                );
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn cancellation_during_initial_parse_returns_no_state() {
        use std::cell::Cell;
        let source = Rope::from_str(&"// padding\n".repeat(32_000));
        let progress_seen = Cell::new(false);
        let result = SyntaxState::new_checked(LanguageId::Rust, source.slice(..), |progress| {
            if progress {
                progress_seen.set(true);
            }
            progress_seen.get()
        });
        assert!(progress_seen.get());
        assert!(result.is_none());
    }

    #[test]
    fn cancellation_during_query_discards_partial_captures() -> Result<(), &'static str> {
        use std::cell::Cell;
        let source =
            Rope::from_str(&("fn main() {}\n".to_owned() + &"// padding\n".repeat(32_000)));
        let state = SyntaxState::new(LanguageId::Rust, source.slice(..)).ok_or("parse")?;
        let stopped = Cell::new(false);
        let progress_seen = Cell::new(false);
        let result =
            state.highlights_checked(source.slice(..), 0..source.len_bytes(), |progress| {
                if progress {
                    progress_seen.set(true);
                    stopped.set(true);
                }
                stopped.get()
            });
        assert!(progress_seen.get());
        assert!(result.is_none());
        let retry = state
            .highlights_checked(source.slice(..), 0..2048, |_| false)
            .ok_or("retry")?;
        assert_eq!(retry, state.highlights(source.slice(..), 0..2048));
        Ok(())
    }

    #[test]
    fn cancellation_during_parse_keeps_tree_and_resets_parser() -> Result<(), &'static str> {
        let mut source =
            Rope::from_str(&("fn main() {}\n".to_owned() + &"// padding\n".repeat(32_000)));
        let mut state =
            SyntaxState::new(LanguageId::Rust, source.slice(..)).ok_or("initial parse")?;
        let before = state.tree.root_node().to_sexp();
        source.insert(0, "a");
        let edit = InputEditRaw {
            start_byte: 0,
            old_end_byte: 0,
            new_end_byte: 1,
            start_point: (0, 0),
            old_end_point: (0, 0),
            new_end_point: (0, 1),
        };
        let mut checks = 0;
        let mut in_progress = false;
        assert!(
            !state.apply_edit_checked(&edit, source.slice(..), |progress| {
                in_progress |= progress;
                checks += 1;
                progress
            })
        );
        // Check iniziale e dopo tree edit, poi callback tree-sitter.
        // Una parse completata arriverebbe al check finale; il caso grande
        // deve invece essere interrotto dal callback mentre produce il tree.
        assert_eq!(checks, 3);
        assert!(in_progress);
        assert_eq!(state.tree.root_node().to_sexp(), before);
        assert!(state.apply_edit_checked(&edit, source.slice(..), |_| false));
        let fresh = SyntaxState::new(LanguageId::Rust, source.slice(..)).ok_or("fresh parse")?;
        assert_eq!(
            state.tree.root_node().to_sexp(),
            fresh.tree.root_node().to_sexp()
        );
        assert_eq!(
            state.highlights(source.slice(..), 0..2048),
            fresh.highlights(source.slice(..), 0..2048)
        );
        Ok(())
    }
}
