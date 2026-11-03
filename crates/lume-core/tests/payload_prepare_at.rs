//! Nuovo writer dalla sorgente posseduta: FS reale, due esiti non conflati.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use lume_core::file_write::prepare_payload;
use std::{
    fs,
    path::{Path, PathBuf},
};
fn temps(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".lume-save-")
        })
        .collect()
}
#[test]
fn nuova_preparazione_copia_tutti_i_byte_ma_non_pubblica() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let destination = target.path().join("file");
    fs::write(&destination, b"originale").unwrap();
    let bytes = [0xff, 0, 1, 2, b'\n'];
    let payload = prepare_payload(source.path(), &bytes).unwrap();
    let outcome = payload.prepare_at(&destination);
    assert_eq!(format!("{outcome:?}"), "PayloadPreparation { .. }");
    let (write, cleanup) = outcome.into_results();
    cleanup.unwrap();
    let write = write.unwrap();
    assert!(temps(source.path()).is_empty());
    assert_eq!(fs::read(&destination).unwrap(), b"originale");
    assert_eq!(temps(target.path()).len(), 1);
    assert_eq!(
        fs::read(temps(target.path()).first().unwrap()).unwrap(),
        bytes
    );
    write.abort().unwrap();
    assert!(temps(target.path()).is_empty());
    assert_eq!(fs::read(destination).unwrap(), b"originale");
}
#[test]
fn nuovo_target_parent_creato_dal_consumatore_commit_esplicito() {
    let dir = tempfile::tempdir().unwrap();
    let payload = prepare_payload(dir.path(), b"payload completo\n").unwrap();
    let parent = dir.path().join("nuova");
    fs::create_dir(&parent).unwrap();
    let destination = parent.join("file");
    let (write, cleanup) = payload.prepare_at(&destination).into_results();
    cleanup.unwrap();
    assert!(!destination.exists());
    assert!(temps(dir.path()).is_empty());
    let write = write.unwrap();
    assert_eq!(write.destination(), destination);
    write.commit().unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"payload completo\n");
    assert!(temps(&parent).is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[test]
fn target_missing_parent_o_readonly_errore_primary_e_cleanup_sorgente() {
    for scenario in ["missing", "readonly"] {
        let dir = tempfile::tempdir().unwrap();
        let destination = if scenario == "missing" {
            dir.path().join("missing/file")
        } else {
            dir.path().join("file")
        };
        let permissions = if scenario == "readonly" {
            fs::write(&destination, b"originale").unwrap();
            let p = fs::metadata(&destination).unwrap().permissions();
            let mut readonly = p.clone();
            readonly.set_readonly(true);
            fs::set_permissions(&destination, readonly).unwrap();
            Some(p)
        } else {
            None
        };
        let payload = prepare_payload(dir.path(), b"nuovo").unwrap();
        let (write, cleanup) = payload.prepare_at(&destination).into_results();
        if let Some(p) = permissions {
            fs::set_permissions(&destination, p).unwrap();
        }
        assert!(write.is_err());
        cleanup.unwrap();
        assert!(temps(dir.path()).is_empty());
        if scenario == "readonly" {
            assert_eq!(fs::read(destination).unwrap(), b"originale");
        } else {
            assert!(!dir.path().join("missing").exists());
        }
    }
}
#[cfg(unix)]
#[test]
fn errore_unlink_non_perde_writer_nuovo_e_legge_fd_non_nome_sostituito() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let payload = prepare_payload(source.path(), b"byte posseduti").unwrap();
    let name = temps(source.path()).pop().unwrap();
    fs::remove_file(&name).unwrap();
    fs::create_dir(&name).unwrap();
    let destination = target.path().join("file");
    let (write, cleanup) = payload.prepare_at(&destination).into_results();
    assert!(cleanup.is_err());
    let write = write.unwrap();
    assert!(name.is_dir());
    assert!(!destination.exists());
    assert_eq!(
        fs::read(temps(target.path()).first().unwrap()).unwrap(),
        b"byte posseduti"
    );
    write.abort().unwrap();
    assert!(temps(target.path()).is_empty());
    fs::remove_dir(name).unwrap();
}
#[cfg(unix)]
#[test]
fn doppio_errore_prepare_e_cleanup_restituiti_separatamente() {
    let dir = tempfile::tempdir().unwrap();
    let payload = prepare_payload(dir.path(), b"byte posseduti").unwrap();
    let name = temps(dir.path()).pop().unwrap();
    fs::remove_file(&name).unwrap();
    fs::create_dir(&name).unwrap();
    let (write, cleanup) = payload
        .prepare_at(&dir.path().join("missing/file"))
        .into_results();
    assert_eq!(write.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    assert!(cleanup.is_err());
    assert!(name.is_dir());
    assert!(!dir.path().join("missing").exists());
    fs::remove_dir(name).unwrap();
}
