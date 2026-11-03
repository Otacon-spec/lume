//! Commit resolved con originale presente: guardie reali, non race orchestrata.
use lume_core::file_write::prepare_atomic_resolved;
use std::io;
#[test]
fn commit_su_originale_presente_esercita_il_secondo_probe() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, b"prima")?;
    let prepared = prepare_atomic_resolved(&path, b"dopo")?;
    assert_eq!(std::fs::read(&path)?, b"prima");
    prepared.commit()?;
    assert_eq!(std::fs::read(&path)?, b"dopo");
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 1);
    Ok(())
}
#[cfg(unix)]
#[test]
fn link_comparso_dopo_prepare_impedisce_commit_preserva_altro_e_pulisce() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    let other = dir.path().join("altro.txt");
    std::fs::write(&path, b"prima")?;
    std::fs::write(&other, b"conservare")?;
    let prepared = prepare_atomic_resolved(&path, b"NO")?;
    std::fs::remove_file(&path)?;
    std::os::unix::fs::symlink(&other, &path)?;
    assert!(prepared.commit().is_err());
    assert_eq!(std::fs::read(&other)?, b"conservare");
    assert!(std::fs::symlink_metadata(&path)?.file_type().is_symlink());
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 2);
    Ok(())
}
#[cfg(unix)]
#[test]
#[allow(unsafe_code)]
fn fifo_comparso_dopo_prepare_impedisce_commit_e_pulisce() -> Result<(), Box<dyn std::error::Error>>
{
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::FileTypeExt},
    };
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    std::fs::write(&path, b"prima")?;
    let prepared = prepare_atomic_resolved(&path, b"NO")?;
    std::fs::remove_file(&path)?;
    let raw = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: il `CString` resta vivo e il FIFO viene creato solo nella directory temporanea posseduta dal test.
    assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
    assert!(prepared.commit().is_err());
    assert!(std::fs::symlink_metadata(&path)?.file_type().is_fifo());
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 1);
    Ok(())
}
