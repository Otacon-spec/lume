//! Dalle diagnostiche del server ai segni dell'editor.
//!
//! Funzione pura: nessuno stato, nessun widget. Il widget riceve intervalli
//! in char già stretti al testo attuale e disegna soltanto.

use lsp_types::DiagnosticSeverity;
use lume_lsp::Problem;
use lume_ui::editor::{DiagnosticMark, MarkSeverity};
use ropey::Rope;

use crate::lsp_pos::char_index_of;

/// I segni di un documento, ordinati per inizio e poi per gravità.
///
/// Le posizioni LSP sono UTF-16 e passano da `char_index_of`. Una riga
/// inesistente indica un testo server superato: quell'estremo cade in fondo al
/// documento. Un intervallo rovesciato viene raddrizzato; `end` non supera mai
/// `rope.len_chars()`.
pub fn marks_for(problems: &[Problem], rope: &Rope) -> Vec<DiagnosticMark> {
    let fine = rope.len_chars();
    let mut segni: Vec<DiagnosticMark> = problems
        .iter()
        .map(|problem| {
            let da = char_index_of(rope, problem.range.start)
                .unwrap_or(fine)
                .min(fine);
            let a = char_index_of(rope, problem.range.end)
                .unwrap_or(fine)
                .min(fine);
            DiagnosticMark {
                range: da.min(a)..da.max(a),
                severity: severity_of(problem.severity),
            }
        })
        .collect();
    segni.sort_by_key(|segno| (segno.range.start, segno.severity.rank()));
    segni
}

/// LSP lascia al client la lettura di una gravità assente: si tratta come
/// informazione, senza trasformarla in un allarme.
fn severity_of(severity: Option<DiagnosticSeverity>) -> MarkSeverity {
    match severity {
        Some(DiagnosticSeverity::ERROR) => MarkSeverity::Error,
        Some(DiagnosticSeverity::WARNING) => MarkSeverity::Warning,
        Some(DiagnosticSeverity::HINT) => MarkSeverity::Hint,
        _ => MarkSeverity::Info,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use lsp_types::{DiagnosticSeverity, Position, Range, Uri};
    use lume_lsp::{Problem, ProblemId, ServerId};
    use lume_ui::editor::{DiagnosticMark, MarkSeverity};
    use ropey::Rope;

    use super::marks_for;

    fn problema(da: (u32, u32), a: (u32, u32), severity: Option<DiagnosticSeverity>) -> Problem {
        Problem {
            id: ProblemId(1),
            server_id: ServerId::new("rust-analyzer"),
            uri: "file:///w/a.rs".parse::<Uri>().unwrap(),
            range: Range::new(Position::new(da.0, da.1), Position::new(a.0, a.1)),
            severity,
            code: None,
            source: None,
            message: "m".to_owned(),
            related: Vec::new(),
        }
    }

    #[test]
    fn un_problema_diventa_un_segno_in_char_con_la_sua_gravita() {
        let rope = Rope::from_str("fn mian() {}\nlet x = 1;\n");
        let segni = marks_for(
            &[problema((0, 3), (0, 7), Some(DiagnosticSeverity::ERROR))],
            &rope,
        );
        assert_eq!(
            segni,
            vec![DiagnosticMark {
                range: 3..7,
                severity: MarkSeverity::Error
            }]
        );
    }

    #[test]
    fn le_colonne_utf16_cadono_sui_char_giusti_dopo_un_emoji() {
        let rope = Rope::from_str("x😀\nabc\n");
        let segni = marks_for(
            &[problema((1, 1), (1, 2), Some(DiagnosticSeverity::WARNING))],
            &rope,
        );
        assert_eq!(segni[0].range, 4..5, "`b` è il char 4 del documento");
        let sulla_riga = marks_for(
            &[problema((0, 1), (0, 3), Some(DiagnosticSeverity::HINT))],
            &rope,
        );
        assert_eq!(
            sulla_riga[0].range,
            1..2,
            "l'emoji vale due unità ma un char"
        );
    }

    #[test]
    fn la_gravita_mancante_e_informazione_e_le_altre_si_mappano() {
        let rope = Rope::from_str("abcdef\n");
        let gravita =
            |severity| marks_for(&[problema((0, 0), (0, 1), severity)], &rope)[0].severity;
        assert_eq!(gravita(None), MarkSeverity::Info);
        assert_eq!(
            gravita(Some(DiagnosticSeverity::ERROR)),
            MarkSeverity::Error
        );
        assert_eq!(
            gravita(Some(DiagnosticSeverity::WARNING)),
            MarkSeverity::Warning
        );
        assert_eq!(
            gravita(Some(DiagnosticSeverity::INFORMATION)),
            MarkSeverity::Info
        );
        assert_eq!(gravita(Some(DiagnosticSeverity::HINT)), MarkSeverity::Hint);
    }

    #[test]
    fn un_intervallo_oltre_la_fine_del_testo_si_stringe_senza_panico() {
        let rope = Rope::from_str("ab\ncd");
        let len = rope.len_chars();
        let segni = marks_for(
            &[
                problema((0, 1), (9, 0), Some(DiagnosticSeverity::ERROR)),
                problema((7, 0), (8, 3), Some(DiagnosticSeverity::WARNING)),
                problema((1, 0), (1, 99), Some(DiagnosticSeverity::HINT)),
            ],
            &rope,
        );
        assert_eq!(segni.len(), 3);
        for segno in &segni {
            assert!(
                segno.range.start <= segno.range.end && segno.range.end <= len,
                "{:?} esce dal testo di {len} char",
                segno.range
            );
        }
        assert!(
            segni.iter().any(|segno| segno.range == (len..len)),
            "una riga inesistente collassa in un segno vuoto in fondo"
        );
        assert!(
            segni.iter().any(|segno| segno.range == (3..len)),
            "una colonna esagerata si ferma a fine riga"
        );
    }

    #[test]
    fn un_intervallo_rovesciato_si_raddrizza_e_i_segni_sono_ordinati() {
        let rope = Rope::from_str("abcdef\nghijkl\n");
        let segni = marks_for(
            &[
                problema((1, 0), (1, 2), Some(DiagnosticSeverity::WARNING)),
                problema((0, 4), (0, 1), Some(DiagnosticSeverity::ERROR)),
            ],
            &rope,
        );
        assert_eq!(segni[0].range, 1..4, "rovesciato: raddrizzato");
        assert_eq!(segni[1].range, 7..9);
        assert!(marks_for(&[], &rope).is_empty());
    }
}
