//! Dati di sola lettura per gli overlay dell'editor.
use std::ops::Range;

use iced::{Color, Rectangle};
use lume_theme::LumeTheme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkSeverity {
    Error,
    Warning,
    Info,
    Hint,
}
impl MarkSeverity {
    pub const fn rank(self) -> u8 {
        match self {
            Self::Error => 0,
            Self::Warning => 1,
            Self::Info => 2,
            Self::Hint => 3,
        }
    }
    pub fn color(self, theme: &LumeTheme) -> Color {
        match self {
            Self::Error => theme.bad,
            Self::Warning => theme.warn,
            Self::Info => theme.accent,
            Self::Hint => theme.muted,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticMark {
    pub range: Range<usize>,
    pub severity: MarkSeverity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineMark {
    pub start_col: usize,
    pub end_col: usize,
    pub severity: MarkSeverity,
}

/// Autore attribuito alla riga corrispondente del buffer corrente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameAuthorLine {
    pub author_name: String,
}

pub fn blame_author_on_line(rows: &[Option<BlameAuthorLine>], line: usize) -> Option<&str> {
    rows.get(line)
        .and_then(Option::as_ref)
        .map(|row| row.author_name.as_str())
}

pub fn marks_on_line(
    marks: &[DiagnosticMark],
    line_start: usize,
    content_end: usize,
) -> Vec<LineMark> {
    marks
        .iter()
        .filter_map(|mark| {
            let (start, end) = (mark.range.start, mark.range.end);
            let touches = if start >= end {
                start >= line_start && start <= content_end
            } else {
                start <= content_end && end > line_start
            };
            touches.then(|| {
                let start_col = start.clamp(line_start, content_end) - line_start;
                let end_col = end.clamp(line_start, content_end) - line_start;
                LineMark {
                    start_col,
                    end_col: end_col.max(start_col),
                    severity: mark.severity,
                }
            })
        })
        .collect()
}
pub fn worst_severity(marks: &[LineMark]) -> Option<MarkSeverity> {
    marks
        .iter()
        .map(|mark| mark.severity)
        .min_by_key(|severity| severity.rank())
}
pub const UNDERLINE_H: f32 = 1.5;
pub const DOT_D: f32 = 6.0;
pub fn underline_rect(x0: f32, x1: f32, line_y: f32, line_h: f32, min_w: f32) -> Rectangle {
    Rectangle {
        x: x0,
        y: line_y + line_h - UNDERLINE_H - 1.0,
        width: (x1 - x0).max(min_w),
        height: UNDERLINE_H,
    }
}
pub fn dot_rect(column_x: f32, column_w: f32, line_y: f32, line_h: f32) -> Rectangle {
    Rectangle {
        x: column_x + (column_w - DOT_D) / 2.0,
        y: line_y + (line_h - DOT_D) / 2.0,
        width: DOT_D,
        height: DOT_D,
    }
}
/// Larghezza della barretta che identifica le righe citate da un appunto.
pub const NOTE_BAR_W: f32 = 3.0;

pub fn note_bar_rect(column_x: f32, line_y: f32, line_h: f32) -> Rectangle {
    Rectangle {
        x: column_x + 1.0,
        y: line_y,
        width: NOTE_BAR_W,
        height: line_h,
    }
}

pub fn line_has_note(ranges: &[Range<usize>], line: usize) -> bool {
    ranges.iter().any(|range| range.contains(&line))
}
/// Stato di un punto di interruzione, per il colore del marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakpointMark {
    /// L'adattatore l'ha accettato.
    Verified,
    /// L'adattatore l'ha rifiutato o lo tiene in sospeso.
    Rejected,
    /// Solo un dato: nessuna sessione l'ha ancora visto (design D-DAP-5).
    Unsent,
}

impl BreakpointMark {
    pub fn color(self, theme: &LumeTheme) -> Color {
        match self {
            Self::Verified => theme.bad,
            Self::Rejected => theme.warn,
            Self::Unsent => Color {
                a: 0.6,
                ..theme.bad
            },
        }
    }
}

/// I segni del debugger in un file (piano 17). Righe a base 0, ordinate.
#[derive(Debug, Clone, Copy, Default)]
pub struct DebugMarks<'a> {
    pub breakpoints: &'a [(usize, BreakpointMark)],
    /// Dove il programma è fermo, se è in questo file.
    pub current_line: Option<usize>,
    /// Righe in cui il programma si è già fermato in questa sessione.
    pub executed: &'a [usize],
    /// Il clic sul numero di riga alterna un breakpoint: vero solo se il
    /// debugger è acceso e il file ha un percorso.
    pub clickable: bool,
}

impl DebugMarks<'_> {
    pub fn breakpoint_on(&self, line: usize) -> Option<BreakpointMark> {
        self.breakpoints
            .binary_search_by_key(&line, |(riga, _)| *riga)
            .ok()
            .and_then(|indice| self.breakpoints.get(indice))
            .map(|(_, mark)| *mark)
    }

    pub fn was_executed(&self, line: usize) -> bool {
        self.executed.binary_search(&line).is_ok()
    }
}

/// Lo sfondo della riga dove il programma è fermo (verde del mockup).
pub fn debug_current_color(theme: &LumeTheme) -> Color {
    Color {
        a: 0.14,
        ..theme.good
    }
}

/// Lo sfondo delle righe in cui il programma si è già fermato.
pub fn debug_executed_color(theme: &LumeTheme) -> Color {
    Color {
        a: 0.05,
        ..theme.good
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EditorOverlay<'a> {
    /// Ordinati per inizio (lo garantisce `diag_marks::marks_for`).
    pub marks: &'a [DiagnosticMark],
    /// Autori allineati alle righe del buffer, quando il blame è visibile.
    pub blame: Option<&'a [Option<BlameAuthorLine>]>,
    /// Il popup di completamento, se aperto in questa colonna.
    pub popup: Option<&'a super::popup::EditorPopup>,
    /// La scheda hover, se c'è.
    pub hover: Option<&'a super::popup::HoverCard>,
    /// Ctrl+Spazio ha senso: il server dichiara il completamento.
    pub complete_enabled: bool,
    /// Il mouse fermo su una parola ha senso: il server dichiara l'hover.
    pub hover_enabled: bool,
    /// Righe a base 0 e intervalli semiaperti citati da un appunto.
    pub note_ranges: &'a [Range<usize>],
    /// Proiezione compatta dei blocchi collassati (`&[]` quando nessuno è chiuso).
    pub fold_spans: &'a [super::folding::FoldSpan],
    /// Breakpoint, riga corrente e righe eseguite del debugger.
    pub debug: DebugMarks<'a>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use lume_theme::LumeTheme;

    use super::{
        BlameAuthorLine, DiagnosticMark, LineMark, MarkSeverity, blame_author_on_line, dot_rect,
        line_has_note, marks_on_line, note_bar_rect, underline_rect, worst_severity,
    };

    fn segno(range: std::ops::Range<usize>, severity: MarkSeverity) -> DiagnosticMark {
        DiagnosticMark { range, severity }
    }
    #[test]
    fn la_barretta_dell_appunto_non_copre_il_punto_della_diagnostica() {
        let barra = note_bar_rect(42.0, 50.0, 20.0);
        assert_eq!(barra.width, super::NOTE_BAR_W);
        assert_eq!(barra.height, 20.0);
        assert_eq!(barra.y, 50.0);
        let punto = dot_rect(42.0, 16.0, 50.0, 20.0);
        assert!(barra.intersection(&punto).is_none());
        assert!(barra.x >= 42.0 && barra.x + barra.width <= 42.0 + 16.0);
    }

    #[test]
    fn una_riga_ha_l_appunto_se_sta_in_un_intervallo_semiaperto() {
        let righe = [1..3, 7..8];
        assert!(!line_has_note(&righe, 0));
        assert!(line_has_note(&righe, 1));
        assert!(line_has_note(&righe, 2));
        assert!(!line_has_note(&righe, 3));
        assert!(line_has_note(&righe, 7));
        assert!(!line_has_note(&righe, 8));
        assert!(!line_has_note(&[], 0));
        let vuoto = std::iter::once(2..2).collect::<Vec<_>>();
        assert!(!line_has_note(&vuoto, 2));
    }
    #[test]
    fn blame_riga_nuova_non_mostra_autore_e_le_immutate_si() {
        let rows = vec![
            None,
            Some(BlameAuthorLine {
                author_name: "Ada Lovelace".into(),
            }),
        ];
        assert_eq!(blame_author_on_line(&rows, 0), None);
        assert_eq!(blame_author_on_line(&rows, 1), Some("Ada Lovelace"));
        assert_eq!(blame_author_on_line(&rows, 2), None);
    }
    fn colonne(start_col: usize, end_col: usize, severity: MarkSeverity) -> LineMark {
        LineMark {
            start_col,
            end_col,
            severity,
        }
    }

    #[test]
    fn un_segno_dentro_la_riga_diventa_colonne_relative() {
        let segni = [segno(12..15, MarkSeverity::Error)];
        assert_eq!(
            marks_on_line(&segni, 10, 20),
            vec![colonne(2, 5, MarkSeverity::Error)]
        );
        assert!(marks_on_line(&segni, 0, 9).is_empty());
        assert!(marks_on_line(&segni, 16, 30).is_empty());
    }
    #[test]
    fn un_segno_su_piu_righe_si_taglia_su_ciascuna() {
        let segni = [segno(5..30, MarkSeverity::Warning)];
        assert_eq!(
            marks_on_line(&segni, 0, 10),
            vec![colonne(5, 10, MarkSeverity::Warning)]
        );
        assert_eq!(
            marks_on_line(&segni, 11, 20),
            vec![colonne(0, 9, MarkSeverity::Warning)]
        );
        assert_eq!(
            marks_on_line(&segni, 21, 40),
            vec![colonne(0, 9, MarkSeverity::Warning)]
        );
    }
    #[test]
    fn un_segno_oltre_la_fine_del_testo_non_dipinge_fuori_dalle_righe() {
        let lontano = [segno(40..50, MarkSeverity::Error)];
        assert!(marks_on_line(&lontano, 5, 12).is_empty());
        let a_cavallo = [segno(8..50, MarkSeverity::Error)];
        assert_eq!(
            marks_on_line(&a_cavallo, 5, 12),
            vec![colonne(3, 7, MarkSeverity::Error)]
        );
    }
    #[test]
    fn un_segno_vuoto_tocca_solo_la_riga_che_lo_contiene() {
        let in_fondo_ad_a = [segno(10..10, MarkSeverity::Info)];
        assert_eq!(
            marks_on_line(&in_fondo_ad_a, 0, 10),
            vec![colonne(10, 10, MarkSeverity::Info)]
        );
        assert!(marks_on_line(&in_fondo_ad_a, 11, 20).is_empty());
        let all_inizio_di_b = [segno(11..11, MarkSeverity::Info)];
        assert!(marks_on_line(&all_inizio_di_b, 0, 10).is_empty());
        assert_eq!(
            marks_on_line(&all_inizio_di_b, 11, 20),
            vec![colonne(0, 0, MarkSeverity::Info)]
        );
    }
    #[test]
    fn un_segno_sul_solo_a_capo_ha_colonne_vuote_sulla_sua_riga() {
        let segni = [segno(10..11, MarkSeverity::Hint)];
        assert_eq!(
            marks_on_line(&segni, 0, 10),
            vec![colonne(10, 10, MarkSeverity::Hint)]
        );
        assert!(marks_on_line(&segni, 11, 20).is_empty());
    }
    #[test]
    fn la_gravita_peggiore_vince_nel_margine() {
        let righe = [
            colonne(0, 1, MarkSeverity::Hint),
            colonne(2, 3, MarkSeverity::Warning),
            colonne(4, 5, MarkSeverity::Info),
        ];
        assert_eq!(worst_severity(&righe), Some(MarkSeverity::Warning));
        assert_eq!(worst_severity(&[]), None);
        assert!(MarkSeverity::Error.rank() < MarkSeverity::Warning.rank());
        assert!(MarkSeverity::Info.rank() < MarkSeverity::Hint.rank());
    }
    #[test]
    fn i_colori_delle_gravita_sono_token_del_tema() {
        let t = LumeTheme::builtin().remove(0);
        assert_eq!(MarkSeverity::Error.color(&t), t.bad);
        assert_eq!(MarkSeverity::Warning.color(&t), t.warn);
        assert_eq!(MarkSeverity::Info.color(&t), t.accent);
        assert_eq!(MarkSeverity::Hint.color(&t), t.muted);
    }
    #[test]
    fn la_sottolineatura_sta_nella_riga_e_ha_una_larghezza_minima() {
        let r = underline_rect(100.0, 130.0, 50.0, 20.0, 6.0);
        assert_eq!((r.x, r.width), (100.0, 30.0));
        assert!(r.y >= 50.0 && r.y + r.height <= 70.0);
        let stub = underline_rect(100.0, 100.0, 50.0, 20.0, 6.0);
        assert_eq!(stub.width, 6.0);
    }
    #[test]
    fn il_punto_e_centrato_nella_colonna_del_marcatore() {
        let d = dot_rect(42.0, 16.0, 50.0, 20.0);
        assert_eq!(d.x + d.width / 2.0, 50.0);
        assert_eq!(d.y + d.height / 2.0, 60.0);
    }
}
