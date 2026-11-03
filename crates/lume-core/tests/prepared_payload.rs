//! Payload senza destinazione: temporanei/protezioni/cleanup FS reali.
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
fn payload_binario_privato_sincronizzato_senza_destinazione_o_directory() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("originale"), b"originale\n").unwrap();
    let bytes = b"\0\xff\x01testo\n";
    let payload = prepare_payload(dir.path(), bytes).unwrap();
    let files = temps(dir.path());
    assert_eq!(files.len(), 1);
    assert_eq!(fs::read(files.first().unwrap()).unwrap(), bytes);
    assert_eq!(
        fs::read(dir.path().join("originale")).unwrap(),
        b"originale\n"
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    let debug = format!("{payload:?}");
    assert_eq!(debug, "PreparedPayload { .. }");
    assert!(!debug.contains(&dir.path().display().to_string()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(files.first().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    payload.abort().unwrap();
    assert!(temps(dir.path()).is_empty());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}
#[test]
fn payload_vuoto_e_drop_cleanup_fallback_reali() {
    let dir = tempfile::tempdir().unwrap();
    let payload = prepare_payload(dir.path(), b"").unwrap();
    assert_eq!(temps(dir.path()).len(), 1);
    assert!(
        fs::read(temps(dir.path()).first().unwrap())
            .unwrap()
            .is_empty()
    );
    drop(payload);
    assert!(temps(dir.path()).is_empty());
}
#[test]
fn parent_assente_file_e_readonly_non_creano_temporanei() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file");
    fs::write(&file, b"originale").unwrap();
    assert!(prepare_payload(&dir.path().join("mancante"), b"nuovo").is_err());
    assert!(prepare_payload(&file, b"nuovo").is_err());
    let folder = dir.path().join("readonly");
    fs::create_dir(&folder).unwrap();
    let original = fs::metadata(&folder).unwrap().permissions();
    let mut readonly = original.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&folder, readonly).unwrap();
    let result = prepare_payload(&folder, b"nuovo");
    fs::set_permissions(&folder, original).unwrap();
    assert!(result.is_err());
    assert!(temps(&folder).is_empty());
    assert!(temps(dir.path()).is_empty());
    assert_eq!(fs::read(&file).unwrap(), b"originale");
}
#[cfg(unix)]
#[test]
fn parent_diretto_symlink_rifiutato_senza_follow_o_payload_esterno() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let link = dir.path().join("link");
    symlink(outside.path(), &link).unwrap();
    assert!(prepare_payload(&link, b"riservato").is_err());
    assert!(temps(outside.path()).is_empty());
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
#[cfg(unix)]
#[test]
fn abort_errore_unlink_preserva_directory_sostituita_noncooperativa() {
    let dir = tempfile::tempdir().unwrap();
    let payload = prepare_payload(dir.path(), b"payload").unwrap();
    let temp = temps(dir.path()).pop().unwrap();
    fs::remove_file(&temp).unwrap();
    fs::create_dir(&temp).unwrap();
    assert!(payload.abort().is_err());
    assert!(temp.is_dir());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    fs::remove_dir(temp).unwrap();
}
