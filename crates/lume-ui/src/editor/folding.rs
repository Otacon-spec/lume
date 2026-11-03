//! Proiezione compatta delle righe per il folding.
//!
//! Trasforma in `O(log F)` (con `F` intervalli effettivamente collassati, non
//! annidati) gli indici di riga del documento negli indici di riga visiva e
//! viceversa. Quando nessun blocco è chiuso lo slice di intervalli è vuoto e
//! la proiezione è l'identità senza allocazioni per frame.

use lume_syntax::FoldRegion;

/// Intervallo collassato esterno effettivo: `start_line` resta visibile,
/// mentre `start_line + 1 ..= end_line` sono nascoste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldSpan {
    /// Riga di intestazione (visibile).
    pub start_line: usize,
    /// Ultima riga nascosta dal blocco (`end_line > start_line`).
    pub end_line: usize,
    /// Numero di righe nascoste da questo blocco (`end_line - start_line`).
    pub hidden_count: usize,
    /// Somma delle righe nascoste dai blocchi *strettamente precedenti* a questo.
    pub hidden_before: usize,
}

/// Proiezione compatta delle righe visibili di un documento.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoldProjection {
    spans: Vec<FoldSpan>,
    total_hidden: usize,
}

impl FoldProjection {
    /// Costruisce la proiezione dai blocchi validi (`regions`, ordinati per
    /// `start_line` crescente e con i contenitori prima dei figli a parità
    /// d'inizio) e da un predicato `is_folded(start_line)`.
    ///
    /// I blocchi collassati annidati dentro un altro blocco collassato vengono
    /// assorbiti dal contenitore esterno, mantenendo intervalli disgiunti e
    /// strettamente ordinati.
    pub fn from_regions(regions: &[FoldRegion], is_folded: impl Fn(usize) -> bool) -> Self {
        let mut spans: Vec<FoldSpan> = Vec::new();
        let mut total_hidden = 0usize;
        for region in regions {
            if region.end_line <= region.start_line || !is_folded(region.start_line) {
                continue;
            }
            if let Some(last) = spans.last()
                && region.start_line <= last.end_line
            {
                // Blocco annidato o sovrapposto a un blocco già collassato:
                // le sue righe sono già nascoste dal contenitore esterno.
                continue;
            }
            let hidden_count = region.end_line - region.start_line;
            spans.push(FoldSpan {
                start_line: region.start_line,
                end_line: region.end_line,
                hidden_count,
                hidden_before: total_hidden,
            });
            total_hidden = total_hidden.saturating_add(hidden_count);
        }
        Self {
            spans,
            total_hidden,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    pub fn spans(&self) -> &[FoldSpan] {
        &self.spans
    }

    pub fn total_hidden(&self) -> usize {
        self.total_hidden
    }

    pub fn as_ref(&self) -> FoldProjectionRef<'_> {
        FoldProjectionRef::new(&self.spans)
    }
}

/// Vista prestata (zero allocazioni) su una slice di [`FoldSpan`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FoldProjectionRef<'a> {
    pub spans: &'a [FoldSpan],
}

impl<'a> FoldProjectionRef<'a> {
    pub const fn new(spans: &'a [FoldSpan]) -> Self {
        Self { spans }
    }

    pub const fn is_empty(self) -> bool {
        self.spans.is_empty()
    }

    /// Totale delle righe nascoste in tutto il documento (`O(1)`).
    pub fn total_hidden(self) -> usize {
        self.spans
            .last()
            .map_or(0, |last| last.hidden_before + last.hidden_count)
    }

    /// Numero totale di righe visibili per un documento con `doc_lines` righe.
    pub fn visible_line_count(self, doc_lines: usize) -> usize {
        if doc_lines == 0 {
            return 0;
        }
        doc_lines.saturating_sub(self.hidden_before_or_at(doc_lines - 1))
    }

    /// Quante righe nascoste cadono in `0..=doc_line` (`O(log F)`).
    fn hidden_before_or_at(self, doc_line: usize) -> usize {
        if self.spans.is_empty() {
            return 0;
        }
        let idx = self
            .spans
            .partition_point(|span| span.start_line <= doc_line);
        let Some(prev) = idx.checked_sub(1).and_then(|i| self.spans.get(i)) else {
            return 0;
        };
        if doc_line <= prev.end_line {
            prev.hidden_before + (doc_line - prev.start_line)
        } else {
            prev.hidden_before + prev.hidden_count
        }
    }

    /// La riga `doc_line` è nascosta dentro un blocco collassato? (`O(log F)`).
    pub fn is_line_hidden(self, doc_line: usize) -> bool {
        if self.spans.is_empty() {
            return false;
        }
        let idx = self
            .spans
            .partition_point(|span| span.start_line < doc_line);
        idx.checked_sub(1)
            .and_then(|i| self.spans.get(i))
            .is_some_and(|span| doc_line > span.start_line && doc_line <= span.end_line)
    }

    /// Se `doc_line` è l'intestazione di un blocco collassato, restituisce il
    /// numero di righe nascoste (`end_line - start_line`), altrimenti `None` (`O(log F)`).
    pub fn collapsed_count_at(self, doc_line: usize) -> Option<usize> {
        if self.spans.is_empty() {
            return None;
        }
        let idx = self
            .spans
            .partition_point(|span| span.start_line < doc_line);
        self.spans
            .get(idx)
            .filter(|span| span.start_line == doc_line)
            .map(|span| span.hidden_count)
    }

    /// Converte una riga del documento nella corrispondente riga visiva (`O(log F)`).
    /// Se `doc_line` è dentro un blocco collassato, viene proiettata sulla riga
    /// visiva della sua intestazione `start_line`.
    pub fn doc_to_visual(self, doc_line: usize) -> usize {
        if self.spans.is_empty() {
            return doc_line;
        }
        doc_line.saturating_sub(self.hidden_before_or_at(doc_line))
    }

    /// Converte una riga visiva (`0..visible_line_count`) nella riga del
    /// documento corrispondente (`O(log F)`).
    pub fn visual_to_doc(self, visual_line: usize, doc_lines: usize) -> usize {
        if doc_lines == 0 {
            return 0;
        }
        if self.spans.is_empty() {
            return visual_line.min(doc_lines - 1);
        }
        let idx = self.spans.partition_point(|span| {
            span.start_line.saturating_sub(span.hidden_before) < visual_line
        });
        let hidden = idx
            .checked_sub(1)
            .and_then(|i| self.spans.get(i))
            .map_or(0, |span| span.hidden_before + span.hidden_count);
        (visual_line + hidden).min(doc_lines - 1)
    }

    /// Elenco esplicito delle righe visibili nella finestra visiva `first_visual..=last_visual`.
    pub fn visible_lines_in(
        self,
        first_visual: usize,
        last_visual: usize,
        doc_lines: usize,
    ) -> Vec<usize> {
        let count = self.visible_line_count(doc_lines);
        if count == 0 || first_visual > last_visual {
            return Vec::new();
        }
        let first = first_visual.min(count - 1);
        let last = last_visual.min(count - 1);
        (first..=last)
            .map(|v| self.visual_to_doc(v, doc_lines))
            .collect()
    }

    /// Riga del documento visibile precedente a `doc_line`.
    pub fn prev_visible_line(self, doc_line: usize, doc_lines: usize) -> usize {
        if doc_lines == 0 {
            return 0;
        }
        let vis = self.doc_to_visual(doc_line.min(doc_lines - 1));
        self.visual_to_doc(vis.saturating_sub(1), doc_lines)
    }

    /// Riga del documento visibile successiva a `doc_line`.
    pub fn next_visible_line(self, doc_line: usize, doc_lines: usize) -> usize {
        if doc_lines == 0 {
            return 0;
        }
        let max_vis = self.visible_line_count(doc_lines).saturating_sub(1);
        let vis = self.doc_to_visual(doc_line.min(doc_lines - 1));
        self.visual_to_doc((vis + 1).min(max_vis), doc_lines)
    }
}

/// La regione `region` è una regione ripiegabile nel documento (`O(log R)`).
pub fn is_foldable_start(regions: &[FoldRegion], start_line: usize) -> bool {
    let idx = regions.partition_point(|r| r.start_line < start_line);
    regions.get(idx).is_some_and(|r| r.start_line == start_line)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{FoldProjection, FoldProjectionRef, is_foldable_start};
    use lume_syntax::FoldRegion;

    fn reg(start_line: usize, end_line: usize) -> FoldRegion {
        FoldRegion {
            start_line,
            end_line,
        }
    }

    #[test]
    fn proiezione_vuota_e_identita_senza_allocazioni() {
        let p = FoldProjectionRef::default();
        assert!(p.is_empty());
        assert_eq!(p.visible_line_count(10), 10);
        for line in 0..10 {
            assert_eq!(p.doc_to_visual(line), line);
            assert_eq!(p.visual_to_doc(line, 10), line);
            assert!(!p.is_line_hidden(line));
            assert_eq!(p.collapsed_count_at(line), None);
        }
    }

    #[test]
    fn oracolo_righe_visibili_con_blocchi_disgiunti_e_annidati() {
        // 12 righe (0..=11):
        // - blocco esterno 1..=5 (contiene il blocco interno 2..=4)
        // - blocco separato 8..=10
        let regions = [reg(1, 5), reg(2, 4), reg(8, 10)];
        // Chiudiamo sia 1..=5 sia l'annidato 2..=4 sia 8..=10:
        // l'annidato 2..=4 deve essere assorbito da 1..=5.
        let proj = FoldProjection::from_regions(&regions, |start| matches!(start, 1 | 2 | 8));
        assert_eq!(proj.spans().len(), 2);
        let p = proj.as_ref();

        // Oracolo esplicito: righe visibili attese [0, 1, 6, 7, 8, 11]
        let expected_visible = vec![0, 1, 6, 7, 8, 11];
        assert_eq!(p.visible_line_count(12), expected_visible.len());
        assert_eq!(
            p.visible_lines_in(0, expected_visible.len() - 1, 12),
            expected_visible
        );

        for (vis_idx, &doc_line) in expected_visible.iter().enumerate() {
            assert_eq!(p.visual_to_doc(vis_idx, 12), doc_line);
            assert_eq!(p.doc_to_visual(doc_line), vis_idx);
            assert!(!p.is_line_hidden(doc_line));
        }
        // Righe nascoste 2..=5 proiettano sulla riga visiva dell'intestazione 1 (vis=1)
        for hidden in 2..=5 {
            assert!(p.is_line_hidden(hidden));
            assert_eq!(p.doc_to_visual(hidden), 1);
        }
        // Righe nascoste 9..=10 proiettano sulla riga visiva dell'intestazione 8 (vis=4)
        for hidden in 9..=10 {
            assert!(p.is_line_hidden(hidden));
            assert_eq!(p.doc_to_visual(hidden), 4);
        }
        assert_eq!(p.collapsed_count_at(1), Some(4));
        assert_eq!(p.collapsed_count_at(2), None);
        assert_eq!(p.collapsed_count_at(8), Some(2));
        assert!(is_foldable_start(&regions, 1));
        assert!(is_foldable_start(&regions, 2));
        assert!(!is_foldable_start(&regions, 3));
    }

    #[test]
    fn navigazione_verticale_salta_le_righe_nascoste() {
        let regions = [reg(1, 4)];
        let proj = FoldProjection::from_regions(&regions, |s| s == 1);
        let p = proj.as_ref();
        // Righe visibili su 6 righe (0..=5): [0, 1, 5]
        assert_eq!(p.next_visible_line(0, 6), 1);
        assert_eq!(p.next_visible_line(1, 6), 5);
        assert_eq!(p.next_visible_line(5, 6), 5);
        assert_eq!(p.prev_visible_line(5, 6), 1);
        assert_eq!(p.prev_visible_line(1, 6), 0);
        assert_eq!(p.prev_visible_line(0, 6), 0);
    }
}
