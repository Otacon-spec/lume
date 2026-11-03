#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Snapshot reali, disco e thread: prerequisito del salvataggio fuori dalla UI.

use lume_core::{ChangeSet, DocId, Document, SaveSnapshot, Transaction, file_write};

fn inserisci(doc: &mut Document, testo: &str) {
    let pos = doc.rope().len_chars();
    doc.apply(Transaction::new(ChangeSet::insert_at(pos, testo, pos)));
}

#[test]
fn lo_snapshot_e_send_sync_e_non_segue_il_testo_digitato_dopo() {
    fn richiede_send_sync<T: Send + Sync>() {}
    richiede_send_sync::<SaveSnapshot>();
    let mut doc = Document::from_str("prima", None, DocId(1));
    inserisci(&mut doc, " salvata");
    let snapshot = doc.snapshot_for_save();
    inserisci(&mut doc, " ancora sporca");
    assert_eq!(snapshot.id(), DocId(1));
    assert_eq!(snapshot.version(), 1);
    assert_eq!(snapshot.payload(), b"prima salvata");
    assert!(doc.mark_snapshot_saved(&snapshot));
    assert!(doc.is_dirty());
    assert_eq!(doc.rope().to_string(), "prima salvata ancora sporca");
}

#[test]
fn una_scrittura_sul_thread_non_marca_salvato_il_testo_successivo() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("file.txt");
    std::fs::write(&path, b"prima").unwrap();
    let mut doc = Document::from_str("prima", Some(path.clone()), DocId(2));
    inserisci(&mut doc, " salvata");
    let snapshot = doc.snapshot_for_save();
    let copia = snapshot.clone();
    let (via, attesa) = std::sync::mpsc::channel();
    let (esito, ricevuto) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        attesa.recv().unwrap();
        let result = file_write::write_atomic(&path, &copia.payload());
        esito.send(result).unwrap();
    });
    inserisci(&mut doc, " ancora sporca");
    via.send(()).unwrap();
    ricevuto.recv().unwrap().unwrap();
    thread.join().unwrap();
    assert_eq!(
        std::fs::read(doc.path().unwrap()).unwrap(),
        b"prima salvata"
    );
    assert!(doc.mark_snapshot_saved(&snapshot));
    assert!(doc.is_dirty());
}

#[test]
fn la_fotografia_del_save_separa_i_gruppi_undo() {
    let mut doc = Document::from_str("", None, DocId(3));
    inserisci(&mut doc, "a");
    let snapshot = doc.snapshot_for_save();
    inserisci(&mut doc, "b");
    assert!(doc.mark_snapshot_saved(&snapshot));
    assert!(doc.is_dirty());
    doc.undo().unwrap();
    assert_eq!(doc.rope().to_string(), "a");
    assert!(
        !doc.is_dirty(),
        "undo ritorna al contenuto realmente salvato"
    );
    doc.redo().unwrap();
    assert!(doc.is_dirty());
    doc.undo().unwrap();
    doc.undo().unwrap();
    assert!(doc.rope().to_string().is_empty());
    assert!(doc.is_dirty(), "undo oltre il save è diverso dal disco");
}

#[test]
fn una_conferma_vecchia_non_sostituisce_un_salvataggio_piu_recente() {
    let mut doc = Document::from_str("", None, DocId(4));
    inserisci(&mut doc, "a");
    let prima = doc.snapshot_for_save();
    inserisci(&mut doc, "b");
    let seconda = doc.snapshot_for_save();
    assert!(doc.mark_snapshot_saved(&seconda));
    assert!(!doc.is_dirty());
    assert!(!doc.mark_snapshot_saved(&prima));
    assert!(!doc.is_dirty());
    doc.undo().unwrap();
    assert!(doc.is_dirty());
}

#[test]
fn il_reload_invalida_le_conferme_precedenti() {
    let mut doc = Document::from_str("prima", None, DocId(5));
    inserisci(&mut doc, " modificata");
    let snapshot = doc.snapshot_for_save();
    doc.reload_from_str("esterna\r\n");
    assert!(!doc.mark_snapshot_saved(&snapshot));
    assert!(!doc.is_dirty());
    assert_eq!(doc.save_payload(), b"esterna\r\n");
}

#[test]
fn un_documento_nuovo_con_lo_stesso_id_non_accetta_la_vecchia_conferma() {
    let mut vecchio = Document::from_str("vecchio", None, DocId(6));
    let snapshot = vecchio.snapshot_for_save();
    let mut nuovo = Document::from_str("nuovo", None, DocId(6));
    inserisci(&mut nuovo, " modificato");
    assert!(!nuovo.mark_snapshot_saved(&snapshot));
    assert!(nuovo.is_dirty());
}

#[test]
fn lo_snapshot_conserva_bom_crlf_unicode_e_coda_senza_a_capo() {
    let mut doc = Document::from_str("\u{feff}però\r\n終", None, DocId(7));
    let snapshot = doc.snapshot_for_save();
    assert_eq!(snapshot.payload(), "\u{feff}però\r\n終".as_bytes());
    assert_eq!(snapshot.payload(), doc.save_payload());
    assert_eq!(snapshot.normalized_text(), "però\n終");
    doc.reload_from_str("altro\n");
    assert_eq!(snapshot.payload(), "\u{feff}però\r\n終".as_bytes());
}

#[test]
fn una_scrittura_rifiutata_non_diventa_un_save_riuscito() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("readonly.txt");
    std::fs::write(&path, b"originale").unwrap();
    let mut doc = Document::from_str("originale", Some(path.clone()), DocId(8));
    inserisci(&mut doc, " modifica");
    let snapshot = doc.snapshot_for_save();
    let originali = std::fs::metadata(&path).unwrap().permissions();
    let mut readonly = originali.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&path, readonly).unwrap();
    let esito = file_write::write_atomic(&path, &snapshot.payload());
    std::fs::set_permissions(&path, originali).unwrap();
    assert!(esito.is_err());
    assert!(doc.is_dirty());
    assert_eq!(std::fs::read(&path).unwrap(), b"originale");
}

#[test]
fn il_preflight_non_muta_testo_dirty_selezione_o_gruppi_undo() {
    use lume_core::Range;
    let mut doc = Document::from_str("", None, DocId(10));
    doc.apply(Transaction::with_selection(
        ChangeSet::insert_at(0, "a", 0),
        Range::point(1),
    ));
    let snapshot = doc.snapshot_for_save();
    doc.apply(Transaction::with_selection(
        ChangeSet::insert_at(1, "b", 1),
        Range::point(2),
    ));
    let version = doc.version();
    let selection = doc.selection();
    let history_bytes = doc.history_bytes();
    assert!(doc.is_dirty());
    assert!(!doc.matches_snapshot(&snapshot));
    assert!(doc.can_mark_snapshot_saved(&snapshot));
    assert!(doc.can_mark_snapshot_saved(&snapshot));
    assert_eq!(doc.rope().to_string(), "ab");
    assert_eq!(doc.version(), version);
    assert_eq!(doc.selection(), selection);
    assert_eq!(doc.history_bytes(), history_bytes);
    assert!(doc.is_dirty(), "il preflight non è un acknowledgment");
    doc.apply(Transaction::with_selection(
        ChangeSet::insert_at(2, "c", 2),
        Range::point(3),
    ));
    doc.undo().unwrap();
    assert_eq!(
        doc.rope().to_string(),
        "a",
        "b e c restano nello stesso gruppo undo"
    );
    assert!(doc.is_dirty(), "a non è stato ancora scritto o confermato");
    doc.redo().unwrap();
    assert_eq!(doc.rope().to_string(), "abc");
    assert_eq!(snapshot.payload(), b"a");
}

#[test]
fn il_preflight_ammette_edit_successivi_ma_solo_l_ack_del_file_scritto_pulisce() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preflight.txt");
    std::fs::write(&path, b"prima").unwrap();
    let mut doc = Document::from_str("prima", Some(path.clone()), DocId(11));
    inserisci(&mut doc, " salvata");
    let snapshot = doc.snapshot_for_save();
    inserisci(&mut doc, " nuova");
    assert!(doc.can_mark_snapshot_saved(&snapshot));
    assert!(doc.is_dirty());
    assert_eq!(std::fs::read(&path).unwrap(), b"prima");
    file_write::write_atomic(&path, &snapshot.payload()).unwrap();
    assert!(doc.mark_snapshot_saved(&snapshot));
    assert!(doc.is_dirty());
    assert_eq!(std::fs::read(&path).unwrap(), b"prima salvata");
    doc.undo().unwrap();
    assert_eq!(doc.rope().to_string(), "prima salvata");
    assert!(!doc.is_dirty());
}

#[test]
fn il_preflight_rifiuta_snapshot_superati_o_di_un_altra_istanza() {
    let mut doc = Document::from_str("", None, DocId(12));
    inserisci(&mut doc, "a");
    let prima = doc.snapshot_for_save();
    inserisci(&mut doc, "b");
    let seconda = doc.snapshot_for_save();
    assert!(doc.can_mark_snapshot_saved(&prima));
    assert!(doc.can_mark_snapshot_saved(&seconda));
    assert!(doc.mark_snapshot_saved(&seconda));
    assert!(!doc.can_mark_snapshot_saved(&prima));
    assert!(doc.can_mark_snapshot_saved(&seconda));
    assert!(!doc.is_dirty());
    let mut estraneo = Document::from_str("ab", None, DocId(12));
    let foreign = estraneo.snapshot_for_save();
    assert!(!doc.can_mark_snapshot_saved(&foreign));
    assert!(!doc.is_dirty());
    doc.reload_from_str("esterna");
    assert!(!doc.can_mark_snapshot_saved(&seconda));
    assert!(!doc.is_dirty());
    assert_eq!(doc.rope().to_string(), "esterna");
}

#[test]
fn il_preflight_non_riserva_il_binding_ne_lo_rianima_dopo_un_rebind_aba() {
    use lume_core::Workspace;
    let mut workspace = Workspace::new();
    let id = workspace.open(Some("prima.txt".into()), "testo");
    let snapshot = workspace.get_mut(id).unwrap().snapshot_for_save();
    assert!(
        workspace
            .get(id)
            .unwrap()
            .can_mark_snapshot_saved(&snapshot)
    );
    workspace.rebind_path(id, "seconda.txt".into()).unwrap();
    assert!(
        !workspace
            .get(id)
            .unwrap()
            .can_mark_snapshot_saved(&snapshot)
    );
    workspace.rebind_path(id, "prima.txt".into()).unwrap();
    assert!(
        !workspace
            .get(id)
            .unwrap()
            .can_mark_snapshot_saved(&snapshot)
    );
    assert!(
        !workspace
            .get_mut(id)
            .unwrap()
            .mark_snapshot_saved(&snapshot)
    );
    assert_eq!(workspace.get(id).unwrap().rope().to_string(), "testo");
    assert!(!workspace.get(id).unwrap().is_dirty());
}

#[test]
fn il_debug_dello_snapshot_non_mostra_il_contenuto() {
    let mut doc = Document::from_str("contenuto privato da non registrare", None, DocId(9));
    let snapshot = doc.snapshot_for_save();
    let debug = format!("{snapshot:?}");
    assert!(!debug.contains("contenuto privato"));
    assert!(debug.contains("SaveSnapshot"));
}

#[cfg(unix)]
#[test]
fn commit_conserva_i_permessi_unix_cambiati_durante_l_attesa() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("script.sh");
    std::fs::write(&path, b"prima").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
    let prepared = file_write::prepare_atomic(&path, b"dopo").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    prepared.commit().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"dopo");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn readonly_applicato_dopo_prepare_rifiuta_commit_e_pulisce_temporaneo() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("protetto.txt");
    std::fs::write(&path, b"prima").unwrap();
    let prepared = file_write::prepare_atomic(&path, b"dopo").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert_eq!(
        prepared.commit().unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"prima");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o400
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}
