//! Probe FD effettivo: barriera stretta del leaf, non degli antenati.
use super::open_write_probe;
use std::{io, os::unix::fs::FileTypeExt};

#[test]
fn fd_regolare_non_troncato_e_link_non_attraversato() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("file.txt");
    let link = dir.path().join("link.txt");
    std::fs::write(&path, b"conservare")?;
    let fd = open_write_probe(&path, true)?;
    assert!(fd.metadata()?.is_file());
    drop(fd);
    assert_eq!(std::fs::read(&path)?, b"conservare");
    std::os::unix::fs::symlink(&path, &link)?;
    assert!(open_write_probe(&link, true).is_err());
    assert_eq!(std::fs::read(&path)?, b"conservare");
    Ok(())
}

#[test]
fn fifo_sostituito_nel_probe_non_blocca_il_worker_prova_child_reale()
-> Result<(), Box<dyn std::error::Error>> {
    use std::{
        ffi::CString,
        os::unix::ffi::OsStrExt,
        time::{Duration, Instant},
    };
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("fifo");
    let raw = CString::new(path.as_os_str().as_bytes())?;
    // Path nativo e CStr valido; FIFO di proprietà della sola fixture.
    // SAFETY: il `CString` resta vivo e il FIFO viene creato solo nella directory temporanea posseduta dal test.
    assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "file_write::resolved_probe_tests::fifo_helper",
            "--ignored",
            "--nocapture",
        ])
        .env("LUME_RESOLVED_FIFO_TEST_DIR", dir.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let stop = child.kill();
                let collected = child.wait();
                panic!("probe FIFO non terminato: stop={stop:?}, wait={collected:?}; test fallito");
            }
            Err(error) => {
                // Wait incerto non è prova di processo vivo: niente kill
                // cieco. Conservare l'owner e riferire l'esito della raccolta.
                let collected = child.wait();
                panic!("try_wait fallito: {error}; raccolta={collected:?}; nessun kill");
            }
        }
    }
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"));
    assert!(std::fs::symlink_metadata(&path)?.file_type().is_fifo());
    Ok(())
}

#[test]
#[ignore = "eseguito e raccolto soltanto dal parent proprietario della FIFO"]
fn fifo_helper() -> io::Result<()> {
    let root = std::env::var_os("LUME_RESOLVED_FIFO_TEST_DIR")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "solo parent proprietario"))?;
    let path = std::path::PathBuf::from(root).join("fifo");
    assert!(std::fs::symlink_metadata(&path)?.file_type().is_fifo());
    assert!(open_write_probe(&path, true).is_err());
    Ok(())
}
