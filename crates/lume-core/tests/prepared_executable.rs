#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Modo Git su temporanei veri, senza alterare accessi non executable.
use lume_core::file_write::prepare_atomic;
use std::{fs, os::unix::fs::PermissionsExt};
#[test]
fn executable_si_applica_ai_permessi_correnti_al_commit_non_a_quelli_vecchi() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("script");
    fs::write(&path, "vecchio").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let prepared = prepare_atomic(&path, b"nuovo")
        .unwrap()
        .with_executable(true);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o620)).unwrap();
    prepared.commit().unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o720
    );
    assert_eq!(fs::read(path).unwrap(), b"nuovo");
}
#[test]
fn nuovo_executable_rimane_privato() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("script");
    prepare_atomic(&path, b"privato")
        .unwrap()
        .with_executable(true)
        .commit()
        .unwrap();
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o700
    );
}
#[test]
fn blob_non_executable_toglie_solo_bit_execute() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("script");
    fs::write(&path, "prima").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o761)).unwrap();
    prepare_atomic(&path, b"dopo")
        .unwrap()
        .with_executable(false)
        .commit()
        .unwrap();
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o660
    );
}
