//! Salva con nome reale, non test del dialogo OS. Nessun server/DB.
use super::*;

#[path = "save_as_binding58.rs"]
mod binding;

fn nuovo(app: &mut Lume) -> lume_core::DocId {
    let id = app.new_untitled().unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));
    id
}

#[test]
fn save_as_non_scrive_ne_rilega_prima_del_poll_del_worker() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo(&mut app);
    let path = dir.path().join("nuovo.txt");
    let task = app.save_as(id, path.clone());
    assert!(!path.exists(), "nessun I/O nel thread UI");
    assert!(app.workspace.get(id).unwrap().path().is_none());
    assert!(app.workspace.get(id).unwrap().is_dirty());
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "X");
    assert_eq!(app.workspace.get(id).unwrap().path(), Some(path.as_path()));
    assert!(!app.workspace.get(id).unwrap().is_dirty());
}

#[cfg(unix)]
#[test]
fn save_as_rifiuta_alias_symlink_di_altro_buffer_prima_di_scrivere() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let target = write_and_open(&mut app, dir.path(), "altro.txt", "da conservare");
    let owner = app.active_doc().unwrap();
    let alias = dir.path().join("alias.txt");
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    let id = nuovo(&mut app);
    let task = app.save_as(id, alias.clone());
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "da conservare");
    assert_eq!(app.workspace.get_by_path(&target), Some(owner));
    assert!(app.workspace.get(id).unwrap().path().is_none());
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(testo_avviso(&app).contains("già aperto"));
}

#[cfg(unix)]
#[test]
fn save_as_rifiuta_hardlink_di_altro_buffer_prima_di_scrivere() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let target = write_and_open(&mut app, dir.path(), "altro.txt", "da conservare");
    let alias = dir.path().join("alias.txt");
    std::fs::hard_link(&target, &alias).unwrap();
    let id = nuovo(&mut app);
    let task = app.save_as(id, alias);
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "da conservare");
    assert!(app.workspace.get(id).unwrap().path().is_none());
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(testo_avviso(&app).contains("già aperto"));
}
