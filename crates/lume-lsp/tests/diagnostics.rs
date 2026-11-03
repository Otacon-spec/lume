#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString,
    Position, PublishDiagnosticsParams, Range, Uri,
};
use lume_lsp::{DiagnosticStore, PublishResult, ServerId};

fn uri(value: &str) -> Uri {
    value.parse().expect("valid URI")
}

fn diagnostic(line: u32, severity: DiagnosticSeverity, message: &str) -> Diagnostic {
    Diagnostic {
        range: Range::new(Position::new(line, 1), Position::new(line, 4)),
        severity: Some(severity),
        code: Some(NumberOrString::String("fixture.code".into())),
        code_description: None,
        source: Some("fixture".into()),
        message: message.into(),
        related_information: Some(vec![DiagnosticRelatedInformation {
            location: Location::new(
                uri("file:///workspace/related.rs"),
                Range::new(Position::new(2, 0), Position::new(2, 1)),
            ),
            message: "related".into(),
        }]),
        tags: None,
        data: None,
    }
}

#[test]
fn publish_replaces_one_server_uri_generation_and_ignores_stale_versions() {
    let server = ServerId::new("fixture");
    let document = uri("file:///workspace/main.rs");
    let mut store = DiagnosticStore::new();
    assert_eq!(
        store.publish(
            server.clone(),
            PublishDiagnosticsParams::new(
                document.clone(),
                vec![diagnostic(8, DiagnosticSeverity::WARNING, "old")],
                Some(3),
            ),
        ),
        PublishResult::Applied
    );
    assert_eq!(
        store.publish(
            server.clone(),
            PublishDiagnosticsParams::new(
                document.clone(),
                vec![diagnostic(1, DiagnosticSeverity::ERROR, "stale")],
                Some(2),
            ),
        ),
        PublishResult::IgnoredStale
    );
    assert_eq!(store.snapshot()[0].message, "old");

    store.publish(
        server,
        PublishDiagnosticsParams::new(
            document,
            vec![diagnostic(4, DiagnosticSeverity::ERROR, "replacement")],
            Some(4),
        ),
    );
    let problems = store.snapshot();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].message, "replacement");
    assert_eq!(problems[0].related.len(), 1);
}

#[test]
fn snapshots_keep_servers_independent_sort_stably_and_clear_exactly() {
    let first = ServerId::new("first");
    let second = ServerId::new("second");
    let document = uri("file:///workspace/lib.rs");
    let mut store = DiagnosticStore::new();
    store.publish(
        first.clone(),
        PublishDiagnosticsParams::new(
            document.clone(),
            vec![diagnostic(9, DiagnosticSeverity::WARNING, "warning")],
            Some(1),
        ),
    );
    store.publish(
        second.clone(),
        PublishDiagnosticsParams::new(
            document,
            vec![diagnostic(12, DiagnosticSeverity::ERROR, "error")],
            Some(1),
        ),
    );
    let first_snapshot = store.snapshot();
    let second_snapshot = store.snapshot();
    assert_eq!(
        first_snapshot, second_snapshot,
        "snapshot order must be stable"
    );
    assert_eq!(first_snapshot[0].message, "error");
    assert_eq!(first_snapshot[1].message, "warning");
    assert_ne!(first_snapshot[0].id, first_snapshot[1].id);

    store.clear_server(&second);
    let remaining = store.snapshot();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].server_id, first);
    store.clear_server(&first);
    assert!(store.snapshot().is_empty());
}

fn pubblica(store: &mut DiagnosticStore, uri: &str, con_problemi: bool) {
    let diagnostics = if con_problemi {
        vec![Diagnostic {
            range: Range::default(),
            message: "e".to_owned(),
            ..Diagnostic::default()
        }]
    } else {
        Vec::new()
    };
    store.publish(
        ServerId::new("rust-analyzer"),
        PublishDiagnosticsParams {
            uri: uri.parse().unwrap(),
            diagnostics,
            version: None,
        },
    );
}

#[test]
fn rilasciare_toglie_solo_le_generazioni_vuote() {
    let mut store = DiagnosticStore::new();
    let server = ServerId::new("rust-analyzer");
    pubblica(&mut store, "file:///a.rs", true);
    pubblica(&mut store, "file:///b.rs", false);
    assert_eq!(store.generation_count(), 2);
    assert!(!store.release(&server, &uri("file:///a.rs")));
    assert!(store.release(&server, &uri("file:///b.rs")));
    assert_eq!(store.generation_count(), 1);
    assert_eq!(store.snapshot().len(), 1);
    assert!(!store.release(&server, &uri("file:///mai.rs")));
}

#[test]
fn l_istantanea_fusa_ordina_i_problemi_di_piu_negozi_come_una_sola() {
    let mut ra = DiagnosticStore::new();
    let mut tombi = DiagnosticStore::new();
    ra.publish(
        ServerId::new("rust-analyzer"),
        PublishDiagnosticsParams::new(
            uri("file:///w/a.rs"),
            vec![diagnostic(4, DiagnosticSeverity::WARNING, "avviso")],
            None,
        ),
    );
    tombi.publish(
        ServerId::new("tombi"),
        PublishDiagnosticsParams::new(
            uri("file:///w/Cargo.toml"),
            vec![diagnostic(1, DiagnosticSeverity::ERROR, "errore")],
            None,
        ),
    );
    let merged = DiagnosticStore::merged_snapshot([&ra, &tombi]);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].message, "errore");
    assert_eq!(merged[1].message, "avviso");
    assert!(DiagnosticStore::merged_snapshot(std::iter::empty::<&DiagnosticStore>()).is_empty());
}

#[test]
fn le_diagnostiche_di_un_solo_file_si_leggono_per_uri() {
    let mut store = DiagnosticStore::new();
    let server = ServerId::new("rust-analyzer");
    store.publish(
        server.clone(),
        PublishDiagnosticsParams::new(
            uri("file:///w/a.rs"),
            vec![
                diagnostic(1, DiagnosticSeverity::ERROR, "uno"),
                diagnostic(2, DiagnosticSeverity::WARNING, "due"),
            ],
            None,
        ),
    );
    store.publish(
        server,
        PublishDiagnosticsParams::new(
            uri("file:///w/b.rs"),
            vec![diagnostic(3, DiagnosticSeverity::HINT, "tre")],
            None,
        ),
    );
    let di_a = store.for_uri(&uri("file:///w/a.rs"));
    assert_eq!(di_a.len(), 2);
    assert!(
        di_a.iter()
            .all(|problem| problem.uri.as_str() == "file:///w/a.rs")
    );
    assert!(store.for_uri(&uri("file:///w/mai-visto.rs")).is_empty());
}
