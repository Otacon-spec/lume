//! Rebinding del modello reale: non scrive file e non concede autorità I/O.
use lume_core::{DocId, Document, RebindError, Workspace};
use std::path::{Path, PathBuf};

fn documento(ws: &Workspace, id: DocId) -> &Document {
    match ws.get(id) {
        Some(doc) => doc,
        None => panic!("documento della fixture assente: {id:?}"),
    }
}

fn documento_mut(ws: &mut Workspace, id: DocId) -> &mut Document {
    match ws.get_mut(id) {
        Some(doc) => doc,
        None => panic!("documento della fixture assente: {id:?}"),
    }
}

#[test]
fn rebind_sposta_il_path_e_l_indice() {
    let mut ws = Workspace::new();
    let a = PathBuf::from("a.txt");
    let b = PathBuf::from("b.txt");
    let id = ws.open(Some(a.clone()), "x");
    assert_eq!(ws.rebind_path(id, b.clone()), Ok(()));
    assert_eq!(documento(&ws, id).path(), Some(b.as_path()));
    assert_eq!(ws.get_by_path(&b), Some(id));
    assert_eq!(ws.get_by_path(&a), None);
    assert_ne!(ws.open(Some(a), "altro"), id);
}

#[test]
fn rebind_da_untitled_rende_il_documento_raggiungibile_per_path() {
    let mut ws = Workspace::new();
    let id = ws.open(None, "x");
    let p = PathBuf::from("nuovo.txt");
    assert_eq!(ws.rebind_path(id, p.clone()), Ok(()));
    assert_eq!(ws.get_by_path(&p), Some(id));
    assert_eq!(ws.open(Some(p.clone()), "ignorato"), id);
    assert!(ws.close(id).is_some());
    assert_eq!(ws.get_by_path(&p), None);
}

#[test]
fn rebind_rifiutato_conserva_documento_e_proprietario() {
    let mut ws = Workspace::new();
    let p = PathBuf::from("occupato.txt");
    let occupato = ws.open(Some(p.clone()), "primo");
    let id = ws.open(None, "secondo");
    let snapshot = documento_mut(&mut ws, id).snapshot_for_save();
    assert_eq!(
        ws.rebind_path(id, p.clone()),
        Err(RebindError::PathTaken(occupato))
    );
    assert!(documento(&ws, id).path().is_none());
    assert!(documento(&ws, id).matches_snapshot(&snapshot));
    assert_eq!(ws.get_by_path(&p), Some(occupato));
    assert_eq!(documento(&ws, occupato).rope().to_string(), "primo");
    assert_eq!(documento(&ws, id).rope().to_string(), "secondo");
}

#[test]
fn rebind_sul_proprio_path_e_noop_anche_per_snapshot() {
    let mut ws = Workspace::new();
    let p = PathBuf::from("a.txt");
    let id = ws.open(Some(p.clone()), "x");
    let snapshot = documento_mut(&mut ws, id).snapshot_for_save();
    assert_eq!(ws.rebind_path(id, p.clone()), Ok(()));
    assert_eq!(ws.get_by_path(&p), Some(id));
    assert_eq!(ws.len(), 1);
    assert!(documento(&ws, id).matches_snapshot(&snapshot));
}

#[test]
fn rebind_di_id_sconosciuto_non_muta_il_workspace() {
    let mut ws = Workspace::new();
    assert_eq!(
        ws.rebind_path(DocId(42), PathBuf::from("libero.txt")),
        Err(RebindError::UnknownDoc)
    );
    assert!(ws.is_empty());
    assert!(ws.get_by_path(Path::new("libero.txt")).is_none());
}

#[test]
fn id_sconosciuto_con_path_occupato_rifiuta_senza_mutare() {
    let mut ws = Workspace::new();
    let p = PathBuf::from("occupato.txt");
    let id = ws.open(Some(p.clone()), "contenuto");
    assert_eq!(
        ws.rebind_path(DocId(42), p.clone()),
        Err(RebindError::PathTaken(id))
    );
    assert_eq!(ws.get_by_path(&p), Some(id));
    assert_eq!(ws.len(), 1);
}

#[test]
fn rebind_non_modifica_testo_selezione_storico_o_stato_salvato() {
    use lume_core::{ChangeSet, Range, Transaction};
    let mut ws = Workspace::new();
    let id = ws.open(Some("a.txt".into()), "\u{feff}a\r\n");
    let doc = documento_mut(&mut ws, id);
    doc.apply(Transaction::new(ChangeSet::insert_at(
        0,
        "X",
        doc.rope().len_chars(),
    )));
    doc.set_selection(Range::point(1));
    let snapshot = doc.snapshot_for_save();
    let version = doc.version();
    let selection = doc.selection();
    let ending = doc.line_ending();
    assert!(doc.is_dirty());
    assert_eq!(ws.rebind_path(id, "b.txt".into()), Ok(()));
    let doc = documento_mut(&mut ws, id);
    assert_eq!(doc.version(), version);
    assert_eq!(doc.selection(), selection);
    assert_eq!(doc.line_ending(), ending);
    assert!(doc.bom());
    assert!(doc.is_dirty());
    assert_eq!(doc.save_payload(), snapshot.payload());
    assert!(doc.undo().is_some());
    assert_eq!(doc.rope().to_string(), "a\n");
    assert!(!doc.is_dirty());
}

#[test]
fn cambio_path_revoca_snapshot_del_legame_precedente() {
    use lume_core::{ChangeSet, Transaction};
    let mut ws = Workspace::new();
    let id = ws.open(Some("a.txt".into()), "prima");
    let doc = documento_mut(&mut ws, id);
    doc.apply(Transaction::new(ChangeSet::insert_at(
        0,
        "X",
        doc.rope().len_chars(),
    )));
    let snapshot = doc.snapshot_for_save();
    assert_eq!(ws.rebind_path(id, "b.txt".into()), Ok(()));
    let doc = documento_mut(&mut ws, id);
    assert!(
        !doc.matches_snapshot(&snapshot),
        "il vecchio legame non è attuale"
    );
    assert!(
        !doc.mark_snapshot_saved(&snapshot),
        "un ack vecchio non pulisce il nuovo file"
    );
    assert!(doc.is_dirty());
    let nuovo = doc.snapshot_for_save();
    assert!(doc.mark_snapshot_saved(&nuovo));
    assert!(!doc.is_dirty());
}

#[test]
fn ritornare_al_vecchio_path_non_rianima_snapshot_revocato() {
    let mut ws = Workspace::new();
    let id = ws.open(Some("a.txt".into()), "testo");
    let snapshot = documento_mut(&mut ws, id).snapshot_for_save();
    assert_eq!(ws.rebind_path(id, "b.txt".into()), Ok(()));
    assert_eq!(ws.rebind_path(id, "a.txt".into()), Ok(()));
    let doc = documento_mut(&mut ws, id);
    assert!(!doc.matches_snapshot(&snapshot));
    assert!(!doc.mark_snapshot_saved(&snapshot));
    assert_eq!(ws.get_by_path(Path::new("a.txt")), Some(id));
    assert!(ws.get_by_path(Path::new("b.txt")).is_none());
}

#[test]
fn ack_prima_del_rebind_preserva_le_modifiche_piu_recenti() {
    use lume_core::{ChangeSet, Transaction};
    let mut ws = Workspace::new();
    let id = ws.open(None, "prima");
    let doc = documento_mut(&mut ws, id);
    doc.apply(Transaction::new(ChangeSet::insert_at(
        0,
        "X",
        doc.rope().len_chars(),
    )));
    let snapshot = doc.snapshot_for_save();
    doc.apply(Transaction::new(ChangeSet::insert_at(
        0,
        "Y",
        doc.rope().len_chars(),
    )));
    // Modello della ricevuta riuscita: la pubblicazione I/O non è provata qui.
    assert!(doc.mark_snapshot_saved(&snapshot));
    assert!(doc.is_dirty());
    assert_eq!(ws.rebind_path(id, "nuovo.txt".into()), Ok(()));
    let doc = documento_mut(&mut ws, id);
    assert_eq!(doc.rope().to_string(), "YXprima");
    assert!(doc.is_dirty());
    assert!(!doc.mark_snapshot_saved(&snapshot));
    assert!(doc.undo().is_some());
    assert_eq!(doc.rope().to_string(), "Xprima");
    assert!(!doc.is_dirty());
}

#[test]
fn rebind_non_crea_directory_ne_file_per_destinazione_inesistente() {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(error) => panic!("directory privata della fixture non disponibile: {error}"),
    };
    let impossibile = dir.path().join("manca").join("nuovo.txt");
    let mut ws = Workspace::new();
    let id = ws.open(None, "x");
    assert_eq!(ws.rebind_path(id, impossibile.clone()), Ok(()));
    assert!(!impossibile.exists());
    assert!(!dir.path().join("manca").exists());
    assert_eq!(ws.get_by_path(&impossibile), Some(id));
    // Solo indice del modello: nessuna promessa di file esistente o pubblicato.
}

#[cfg(unix)]
#[test]
fn rebind_conserva_byte_non_utf8_e_backslash_letterale() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let p = PathBuf::from(OsString::from_vec(b"nome\\\xff.txt".to_vec()));
    let mut ws = Workspace::new();
    let id = ws.open(None, "x");
    assert_eq!(ws.rebind_path(id, p.clone()), Ok(()));
    assert_eq!(documento(&ws, id).path(), Some(p.as_path()));
    assert_eq!(ws.get_by_path(&p), Some(id));
    assert!(ws.get_by_path(Path::new("nome/\u{fffd}.txt")).is_none());
}
