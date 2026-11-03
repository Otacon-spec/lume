#![allow(clippy::unwrap_used, clippy::expect_used)]
//! File e handle reali: il temporaneo resta posseduto fino al commit/drop.

use lume_core::file_write::prepare_atomic;

#[test]
fn preparare_non_pubblica_e_il_drop_non_tocca_l_originale() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, b"originale").unwrap();
    let prepared = prepare_atomic(&path, b"nuovo").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"originale");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    drop(prepared);
    assert_eq!(std::fs::read(&path).unwrap(), b"originale");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn un_file_nuovo_esiste_solo_dopo_commit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nuovo.txt");
    let prepared = prepare_atomic(&path, b"contenuto").unwrap();
    assert!(!path.exists());
    prepared.commit().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"contenuto");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn la_preparazione_e_send_e_il_commit_funzionano_su_thread_distinti() {
    fn richiede_send<T: Send>() {}
    richiede_send::<lume_core::file_write::PreparedWrite>();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("thread.txt");
    std::fs::write(&path, b"prima").unwrap();
    let target = path.clone();
    let prepared = std::thread::spawn(move || prepare_atomic(&target, b"dopo").unwrap())
        .join()
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"prima");
    std::thread::spawn(move || prepared.commit().unwrap())
        .join()
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"dopo");
}

#[test]
fn un_errore_del_commit_non_lascia_temporanei_o_tronca_un_altro_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    let prepared = prepare_atomic(&path, b"nuovo").unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("figlio.txt"), b"intatto").unwrap();
    assert!(prepared.commit().is_err());
    assert_eq!(std::fs::read(path.join("figlio.txt")).unwrap(), b"intatto");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn abbandonare_un_batch_preparato_non_modifica_nessun_originale() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    std::fs::write(&first, b"uno").unwrap();
    std::fs::write(&second, b"due").unwrap();
    let batch = vec![
        prepare_atomic(&first, b"UNO").unwrap(),
        prepare_atomic(&second, b"DUE").unwrap(),
    ];
    drop(batch);
    assert_eq!(std::fs::read(&first).unwrap(), b"uno");
    assert_eq!(std::fs::read(&second).unwrap(), b"due");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn errore_sul_secondo_commit_non_annulla_il_primo() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    std::fs::write(&first, b"uno").unwrap();
    let a = prepare_atomic(&first, b"UNO").unwrap();
    let b = prepare_atomic(&second, b"DUE").unwrap();
    std::fs::create_dir(&second).unwrap();
    a.commit().unwrap();
    assert!(b.commit().is_err());
    assert_eq!(std::fs::read(&first).unwrap(), b"UNO");
    assert!(second.is_dir());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn readonly_applicato_dopo_preparazione_non_viene_aggirato() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("readonly.txt");
    std::fs::write(&path, b"prima").unwrap();
    let prepared = prepare_atomic(&path, b"dopo").unwrap();
    let original = std::fs::metadata(&path).unwrap().permissions();
    let mut readonly = original.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&path, readonly).unwrap();
    let result = prepared.commit();
    std::fs::set_permissions(&path, original).unwrap();
    assert_eq!(
        result.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"prima");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn un_file_eliminato_non_viene_ricreato_dalla_vecchia_preparazione() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deleted.txt");
    std::fs::write(&path, b"prima").unwrap();
    let prepared = prepare_atomic(&path, b"dopo").unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        prepared.commit().unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn una_creazione_concorrente_non_viene_sovrascritta() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("created.txt");
    let prepared = prepare_atomic(&path, b"nostro").unwrap();
    std::fs::write(&path, b"altrove").unwrap();
    assert_eq!(
        prepared.commit().unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"altrove");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn permessi_cambiati_dopo_la_preparazione_restano_quelli_correnti() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permissions.txt");
    std::fs::write(&path, b"prima").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let prepared = prepare_atomic(&path, b"dopo").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    prepared.commit().unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"dopo");
}

#[test]
fn il_debug_non_espone_percorso_o_contenuto_del_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nome-privato.txt");
    let prepared = prepare_atomic(&path, b"contenuto privato").unwrap();
    let debug = format!("{prepared:?}");
    assert!(!debug.contains("nome-privato"));
    assert!(!debug.contains("contenuto privato"));
}

#[cfg(unix)]
#[test]
fn la_destinazione_del_link_e_risolta_prima_del_commit() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.txt");
    let other = dir.path().join("other.txt");
    let link = dir.path().join("link.txt");
    std::fs::write(&original, b"prima").unwrap();
    std::fs::write(&other, b"altro").unwrap();
    symlink(&original, &link).unwrap();
    let prepared = prepare_atomic(&link, b"dopo").unwrap();
    assert_eq!(
        prepared.destination(),
        std::fs::canonicalize(&original).unwrap()
    );
    std::fs::remove_file(&link).unwrap();
    symlink(&other, &link).unwrap();
    prepared.commit().unwrap();
    assert_eq!(std::fs::read(&original).unwrap(), b"dopo");
    assert_eq!(std::fs::read(&other).unwrap(), b"altro");
    assert_eq!(std::fs::read(&link).unwrap(), b"altro");
}
