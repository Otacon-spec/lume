//! Predicate menu con filesystem/store reali, non renderer o driver SQL.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::super::Lume;
use lume_core::DocId;
use lume_notes::NoteStore;

fn fixture83() -> (tempfile::TempDir, Lume, DocId, NoteStore) {
    let fixture = tempfile::Builder::new()
        .prefix("lume-link83-città-")
        .tempdir()
        .unwrap();
    let project = fixture.path().join("progetto");
    std::fs::create_dir(&project).unwrap();
    let notes = project.join("Appunti");
    let store = NoteStore::open(&notes).unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    app.finish_notes_load(generation, notes, Vec::new())
        .unwrap();
    app.set_project_root(project.clone());
    let code = project.join("città.txt");
    std::fs::write(&code, "testo 😀\n").unwrap();
    app.open_file(&code).unwrap();
    let doc = app.active_doc().unwrap();
    (fixture, app, doc, store)
}

#[test]
fn codice_in_progetto_con_appunti_pronti_abilita_nuova_nota_anche_senza_sql() {
    let (_fixture, app, code, _store) = fixture83();
    assert!(!app.is_sql_console(code));
    assert_eq!(app.note_link_menu(), (false, true));
}

#[test]
fn appunto_nel_progetto_non_diventa_sorgente_di_collegamento_codice() {
    let (_fixture, mut app, _code, store) = fixture83();
    let note = store.create("Nota reale83").unwrap();
    let doc = app.open_loaded_note(note);
    assert_eq!(app.active_doc(), Some(doc));
    // La nota è dentro project_root: il solo guard percorso non può spiegare
    // il rifiuto. Nessuna falsa nota/risposta store o profilo utente.
    let root = app.project_root.as_ref().unwrap();
    let path = app.workspace.get(doc).unwrap().path().unwrap();
    assert!(lume_notes::relative_path(root, path).is_some());
    assert_eq!(app.note_link_menu(), (false, false));
}

#[cfg(feature = "sql")]
#[test]
fn console_sql_classificata_non_e_sorgente_anche_con_percorso_nel_progetto() {
    let (_fixture, mut app, code, _store) = fixture83();
    assert_eq!(app.note_link_menu(), (false, true));
    // Classificazione controller controllata: non apertura console/vault/DB.
    // Percorso valido volutamente preservato per isolare il guard SQL.
    app.sql_console = Some(code);
    assert!(app.is_sql_console(code));
    assert_eq!(app.note_link_menu(), (false, false));
}
