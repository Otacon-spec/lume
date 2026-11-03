//! Scope ricerca: file/engine/Task veri, non risultati di un backend simulato.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use futures::{StreamExt, stream::SelectAll};
use std::sync::atomic::Ordering;
fn drive(app: &mut Lume, task: Task<Message>) -> Vec<(u64, lume_core::SearchUpdate)> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut streams = SelectAll::new();
            if let Some(stream) = iced_runtime::task::into_stream(task) {
                streams.push(stream);
            }
            let mut observed = Vec::new();
            while let Some(action) = streams.next().await {
                if let iced_runtime::Action::Output(message) = action {
                    if let Message::SearchUpdate(generation, update) = &message
                        && let Some(update) = update.observe()
                    {
                        observed.push((*generation, update));
                    }
                    let task = app.update(message);
                    if let Some(stream) = iced_runtime::task::into_stream(task) {
                        streams.push(stream);
                    }
                }
            }
            observed
        })
}
fn setup() -> (Lume, tempfile::TempDir, tempfile::TempDir) {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    std::fs::write(first.path().join("prima.txt"), "ago vecchio\n").unwrap();
    std::fs::write(second.path().join("seconda.txt"), "ago nuovo\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.set_project_root(first.path().to_path_buf());
    let requests = app.registry.set_active_panel(Some(tools::search_tool::ID));
    app.pending_tool_requests.extend(requests);
    let _ = app.drain_tool_requests();
    let task = app.update(Message::Tool(
        tools::search_tool::ID,
        ToolEvent::new(tools::search_tool::Event::Query("ago".into())),
    ));
    let updates = drive(&mut app, task);
    assert!(updates.iter().any(|(_,update)|matches!(update,lume_core::SearchUpdate::Batch(files) if files.iter().any(|file|file.path==first.path().join("prima.txt")))));
    (app, first, second)
}
#[test]
fn cambio_radice_rifiuta_batch_gia_consegnato_e_rilancia_sul_progetto_nuovo() {
    let (mut app, first, second) = setup();
    let old = app.search_gen.load(Ordering::SeqCst);
    let late = lume_core::SearchUpdate::Batch(vec![lume_core::FileMatches {
        path: first.path().join("prima.txt"),
        hits: Vec::new(),
    }]);
    let task = app.update(Message::FolderPicked(Some(second.path().to_path_buf())));
    assert!(
        app.search_gen.load(Ordering::SeqCst) > old,
        "il setter deve invalidare la ricerca del progetto precedente"
    );
    assert!(!app.deliver_search_update(old, late));
    let updates = drive(&mut app, task);
    assert!(updates.iter().any(|(_,update)|matches!(update,lume_core::SearchUpdate::Batch(files) if files.iter().any(|file|file.path==second.path().join("seconda.txt")))));
    assert!(updates.iter().all(|(_,update)|!matches!(update,lume_core::SearchUpdate::Batch(files) if files.iter().any(|file|file.path==first.path().join("prima.txt")))));
}
#[test]
fn file_di_avvio_fuori_progetto_drena_rilancio_cerca_senza_evento_successivo() {
    let (mut app, _first, second) = setup();
    let reading = crate::file_read::leggi_per_apertura(
        second.path().join("seconda.txt"),
        crate::file_read::DopoApertura::Avvio {
            fuori_progetto: true,
        },
    );
    let task = app.update(Message::FileReadFinished(
        app.scritture.guarda(None).epoca,
        None,
        reading,
    ));
    let updates = drive(&mut app, task);
    assert_eq!(app.project_root.as_deref(), Some(second.path()));
    assert!(updates.iter().any(|(_,update)|matches!(update,lume_core::SearchUpdate::Batch(files) if files.iter().any(|file|file.path==second.path().join("seconda.txt")))));
}

#[test]
fn cambio_radice_a_pannello_chiuso_non_accende_ricerca_alla_riapertura_scope_nuovo() {
    let (mut app, _first, second) = setup();
    let requests = app.registry.set_active_panel(None);
    app.pending_tool_requests.extend(requests);
    let _ = app.drain_tool_requests();
    let task = app.update(Message::FolderPicked(Some(second.path().to_path_buf())));
    assert!(drive(&mut app, task).is_empty());
    let requests = app.registry.set_active_panel(Some(tools::search_tool::ID));
    app.pending_tool_requests.extend(requests);
    let task = app.drain_tool_requests();
    let updates = drive(&mut app, task);
    assert!(updates.iter().any(|(_,update)|matches!(update,lume_core::SearchUpdate::Batch(files) if files.iter().any(|file|file.path==second.path().join("seconda.txt")))));
}
