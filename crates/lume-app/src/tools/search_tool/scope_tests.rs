//! Stato puro del tool, non evidenza di un motore/DB simulato.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
fn populated() -> SearchTool {
    let mut tool = SearchTool::new();
    let _ = tool.update(&ToolEvent::new(Event::Query("ago".into())));
    let _ = tool.update(&ToolEvent::new(Event::Append(vec![FileMatches {
        path: "a.txt".into(),
        hits: vec![lume_core::SearchHit {
            line_number: 1,
            line_text: "ago".into(),
            range_in_line: 0..3,
        }],
    }])));
    let _ = tool.update(&ToolEvent::new(Event::Done {
        files: 1,
        matches: 1,
        truncated: false,
    }));
    tool
}
#[test]
fn conferma_senza_domanda_o_duplicata_non_emette_sostituzione() {
    let mut tool = populated();
    assert!(
        tool.update(&ToolEvent::new(Event::ConfirmReplaceAll(0)))
            .is_empty()
    );
    let _ = tool.update(&ToolEvent::new(Event::AskReplaceAll));
    let ticket = tool.replace_request;
    assert!(
        !tool
            .update(&ToolEvent::new(Event::ConfirmReplaceAll(ticket)))
            .is_empty()
    );
    assert!(
        tool.update(&ToolEvent::new(Event::ConfirmReplaceAll(ticket)))
            .is_empty()
    );
}
#[test]
fn parametri_modificati_e_domanda_nuova_invalidano_ticket_vecchio() {
    let mut tool = populated();
    let _ = tool.update(&ToolEvent::new(Event::AskReplaceAll));
    let old = tool.replace_request;
    let _ = tool.update(&ToolEvent::new(Event::ReplaceChanged("nuovo".into())));
    assert!(
        tool.update(&ToolEvent::new(Event::ConfirmReplaceAll(old)))
            .is_empty()
    );
    let _ = tool.update(&ToolEvent::new(Event::AskReplaceAll));
    let current = tool.replace_request;
    assert!(current > old);
    assert!(
        tool.update(&ToolEvent::new(Event::ConfirmReplaceAll(old)))
            .is_empty()
    );
    assert!(
        tool.pending_replace,
        "ticket estraneo non revoca domanda nuova"
    );
    assert!(
        !tool
            .update(&ToolEvent::new(Event::ConfirmReplaceAll(current)))
            .is_empty()
    );
}
#[test]
fn cambio_scope_libera_cache_revoca_domanda_e_rilancia_solo_se_visibile() {
    for visible in [true, false] {
        let mut tool = populated();
        let _ = tool.update(&ToolEvent::new(Event::AskReplaceAll));
        let old = tool.replace_request;
        let requests = tool.update(&ToolEvent::new(Event::ProjectChanged { visible }));
        assert!(tool.results.is_empty());
        assert_eq!(tool.results.capacity(), 0);
        assert!(!tool.pending_replace);
        assert!(
            tool.update(&ToolEvent::new(Event::ConfirmReplaceAll(old)))
                .is_empty()
        );
        assert_eq!(requests.len(), usize::from(visible));
        assert_eq!(tool.options.needle, "ago");
    }
}
