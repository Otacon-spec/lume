//! Creazioni/limiti reali su filesystem isolato; niente apply Git o UI.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use lume_core::file_write::{DirectoryCreationPhase, create_private_directory, prepare_payload};
use std::{fs, io};
#[test]
fn directory_privata_creata_effetto_reale_senza_cleanup_al_drop() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("privata");
    let outcome = create_private_directory(&path);
    assert_eq!(format!("{outcome:?}"), "DirectoryCreation { .. }");
    let (phase, result) = outcome.into_parts();
    assert_eq!(phase, DirectoryCreationPhase::Created);
    result.unwrap();
    assert!(path.is_dir());
    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let second = root.path().join("seconda");
    drop(create_private_directory(&second));
    assert!(second.is_dir());
}
#[test]
fn parent_assente_o_file_rifiutato_senza_creare_intermedie() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("file"), b"originale").unwrap();
    for prefix in ["missing", "file"] {
        let path = root.path().join(prefix).join("figlia");
        let (phase, result) = create_private_directory(&path).into_parts();
        assert_eq!(phase, DirectoryCreationPhase::NotAttempted);
        assert!(result.is_err());
    }
    assert!(!root.path().join("missing").exists());
    assert_eq!(fs::read(root.path().join("file")).unwrap(), b"originale");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}
#[test]
fn nomi_occupati_non_sovrascritti_esito_native_non_confermato() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    fs::write(&file, b"originale").unwrap();
    let directory = root.path().join("dir");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("child"), b"figlio").unwrap();
    let before = fs::metadata(&directory).unwrap().permissions();
    for path in [&file, &directory] {
        let (phase, result) = create_private_directory(path).into_parts();
        assert_eq!(phase, DirectoryCreationPhase::CreationUnconfirmed);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    }
    assert_eq!(fs::read(file).unwrap(), b"originale");
    assert_eq!(fs::read(directory.join("child")).unwrap(), b"figlio");
    assert_eq!(fs::metadata(directory).unwrap().permissions(), before);
}
#[cfg(unix)]
#[test]
fn parent_link_o_readonly_rifiutato_e_leaf_link_non_seguito() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let link = root.path().join("link");
    symlink(outside.path(), &link).unwrap();
    let (phase, result) = create_private_directory(&link.join("child")).into_parts();
    assert_eq!(phase, DirectoryCreationPhase::NotAttempted);
    assert!(result.is_err());
    assert!(!outside.path().join("child").exists());
    let (phase, result) = create_private_directory(&link).into_parts();
    assert_eq!(phase, DirectoryCreationPhase::CreationUnconfirmed);
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let parent = root.path().join("readonly");
    fs::create_dir(&parent).unwrap();
    let before = fs::metadata(&parent).unwrap().permissions();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();
    let (phase, result) = create_private_directory(&parent.join("child")).into_parts();
    fs::set_permissions(&parent, before).unwrap();
    assert_eq!(phase, DirectoryCreationPhase::NotAttempted);
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert!(!parent.join("child").exists());
}
#[test]
fn payload_in_parent_nuovo_writer_protetto_commit_esplicito() {
    let root = tempfile::tempdir().unwrap();
    let payload = prepare_payload(root.path(), b"byte reali\0\xff").unwrap();
    let parent = root.path().join("nuova");
    let (phase, result) = create_private_directory(&parent).into_parts();
    assert_eq!(phase, DirectoryCreationPhase::Created);
    result.unwrap();
    let destination = parent.join("file");
    let (write, cleanup) = payload.prepare_at(&destination).into_results();
    cleanup.unwrap();
    assert!(!destination.exists());
    write.unwrap().commit().unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"byte reali\0\xff");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[cfg(target_os = "linux")]
#[test]
#[allow(unsafe_code)]
fn directory_creata_ma_protezione_non_confermata_rimane_e_riporta_fase() {
    const FLAG: &str = "LUME_TEST_UMASK_DIRECTORY";
    if std::env::var_os(FLAG).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "directory_creata_ma_protezione_non_confermata_rimane_e_riporta_fase",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(FLAG, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            output.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("running 1 test"));
        assert!(stdout.contains("test result: ok. 1 passed; 0 failed"));
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    // Il mask di processo viene cambiato solo in questo child isolato, mai nel
    // runner che esegue altri test in parallelo. ABI mode_t Linux u32.
    unsafe extern "C" {
        fn umask(mask: u32) -> u32;
    }
    struct Restore(u32);
    impl Drop for Restore {
        fn drop(&mut self) {
            // SAFETY: `self.0` è il valore restituito dalla precedente `umask`; ripristinarlo non usa puntatori né memoria esterna.
            unsafe {
                umask(self.0);
            }
        }
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("creata");
    // SAFETY: la chiamata imposta la maschera del processo figlio isolato; `Restore` conserva il valore precedente e lo ripristina.
    let restore = Restore(unsafe { umask(0o700) });
    let outcome = create_private_directory(&path);
    drop(restore);
    let (phase, result) = outcome.into_parts();
    let was_directory = path.is_dir();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    let count = fs::read_dir(root.path()).unwrap().count();
    // Cleanup della fixture prima degli assert: anche la mutazione negativa
    // non lascia una directory000 in un child terminato per panic.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir(&path).unwrap();
    assert_eq!(phase, DirectoryCreationPhase::Created);
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert!(was_directory);
    assert_eq!(mode, 0);
    assert_eq!(count, 1);
}
