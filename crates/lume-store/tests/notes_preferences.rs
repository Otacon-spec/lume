#![allow(clippy::unwrap_used)]

use lume_store::StoreHandle;

#[test]
fn notes_directory_is_global_and_survives_restart() {
    futures::executor::block_on(async {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("lume.sqlite3");
        let notes = temp.path().join("Appunti personali");

        let store = StoreHandle::open_at(&database).await.unwrap();
        assert_eq!(store.load_notes_directory().await.unwrap(), None);
        store.save_notes_directory(notes.clone()).await.unwrap();
        drop(store);

        let reopened = StoreHandle::open_at(database).await.unwrap();
        assert_eq!(reopened.load_notes_directory().await.unwrap(), Some(notes));
    });
}
