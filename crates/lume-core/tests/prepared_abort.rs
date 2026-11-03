//! Abort reale del temporaneo protetto: esito cleanup distinto dal Drop.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use lume_core::file_write::prepare_atomic;
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
fn abort_consuma_senza_pubblicare_e_restituisce_esito_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("originale");
    fs::write(&existing, b"originale\n").unwrap();
    for destination in [&existing, &dir.path().join("nuovo")] {
        let prepared = prepare_atomic(destination, b"payload\n").unwrap();
        assert_eq!(temps(dir.path()).len(), 1);
        prepared.abort().unwrap();
        assert!(temps(dir.path()).is_empty());
        assert_eq!(fs::read(&existing).unwrap(), b"originale\n");
        assert!(!dir.path().join("nuovo").exists());
    }
}
#[cfg(unix)]
#[test]
fn abort_non_dichiara_successo_se_unlink_fallisce() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("originale");
    fs::write(&destination, b"originale\n").unwrap();
    let prepared = prepare_atomic(&destination, b"payload\n").unwrap();
    let temp = temps(dir.path()).pop().unwrap();
    fs::remove_file(&temp).unwrap();
    fs::create_dir(&temp).unwrap();
    assert!(prepared.abort().is_err());
    assert!(temp.is_dir());
    assert_eq!(fs::read(&destination).unwrap(), b"originale\n");
    fs::remove_dir(temp).unwrap();
}
