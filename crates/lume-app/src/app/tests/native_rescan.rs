//! Helper child: il parent ferma soltanto questo processo e induce overflow.
use super::*;
use futures::StreamExt;
use std::{path::PathBuf, task::Poll, time::Duration};

#[test]
#[ignore = "overflow nativo: richiede il parent posseduto e marker fuori scope"]
fn overflow_reale_raggiunge_subscription_e_buffer() {
    let root = PathBuf::from(std::env::var_os("LUME_RESCAN_NATIVE_ROOT").expect("parent root"));
    let ready = PathBuf::from(std::env::var_os("LUME_RESCAN_NATIVE_READY").expect("parent marker"));
    assert!(!ready.starts_with(&root), "marker deve essere fuori scope");
    let mut app = app();
    let clean = write_and_open(&mut app, &root, "clean.txt", "prima clean\r\n");
    let clean_id = app.active_doc().unwrap();
    let dirty = write_and_open(&mut app, &root, "dirty.txt", "prima dirty\r\n");
    let dirty_id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let dirty_version = app.workspace.get(dirty_id).unwrap().version();
    let dirty_fingerprint = app.disk_fingerprints[&dirty_id];
    let directories = app.fs_watch_directories();
    assert_eq!(directories, vec![root.clone()]);
    let recipe = iced::advanced::subscription::into_recipes(app.fs_watcher_subscription())
        .pop()
        .expect("recipe");
    let mut stream = recipe.stream(futures::stream::empty().boxed());
    runtime_file_tests().block_on(async {
        futures::future::poll_fn(|cx| match stream.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("evento prima del marker"),
        }).await;
        std::fs::write(&ready, b"watch_ready").unwrap();
        let mut changed_before = 0;
        let event = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match stream.next().await.expect("subscription terminata") {
                    Message::FsRescan(scope) => {
                        assert_eq!(scope, directories);
                        break Message::FsRescan(scope);
                    }
                    Message::FsChanged(_) => changed_before += 1,
                    Message::FsWatchFailed(_, error) => panic!("watcher nativo: {error}"),
                    _ => panic!("messaggio estraneo al watcher"),
                }
            }
        }).await.expect("overflow non consegnato");
        let task = app.update(event);
        let mut outputs = iced_runtime::task::into_stream(task).expect("reload task");
        let mut delivered = 0;
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(action) = outputs.next().await {
                if let iced_runtime::Action::Output(message) = action {
                    delivered += 1;
                    let _ = app.update(message);
                }
            }
        }).await.expect("reload incompleto");
        assert_eq!(delivered, 2, "entrambi i reader devono completarsi");
        let clean_doc = app.workspace.get(clean_id).unwrap();
        assert_eq!(clean_doc.save_payload(), b"esterna clean\r\n");
        assert!(!clean_doc.is_dirty());
        assert_eq!(app.disk_fingerprints[&clean_id], crate::file_read::Testo::from("esterna clean\r\n".to_owned()).fingerprint());
        let dirty_doc = app.workspace.get(dirty_id).unwrap();
        assert_eq!(dirty_doc.save_payload(), b"Xprima dirty\r\n");
        assert!(dirty_doc.is_dirty());
        assert_eq!(dirty_doc.version(), dirty_version);
        assert_eq!(app.disk_fingerprints[&dirty_id], dirty_fingerprint);
        assert!(app.external_conflicts.contains(&dirty_id));
        assert_eq!(std::fs::read(clean).unwrap(), b"esterna clean\r\n");
        assert_eq!(std::fs::read(dirty).unwrap(), b"esterna dirty\r\n");
        eprintln!("NATIVE_RESCAN_APP:rescan_reale=true changed_before={changed_before} reader_outputs={delivered} clean_reloaded=true dirty_preserved=true conflict=true");
    });
    drop(stream);
}
