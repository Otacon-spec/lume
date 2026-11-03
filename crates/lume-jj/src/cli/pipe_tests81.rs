//! Pipe OS posseduta/IO controllato distinti dalla prova del driver jj reale.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(unix)]
use super::*;
#[cfg(unix)]
use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    sync::Arc,
};

#[cfg(unix)]
struct Tracked81 {
    source: UnixStream,
    closed: Arc<AtomicBool>,
}
#[cfg(unix)]
impl Read for Tracked81 {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.source.read(bytes)
    }
}
#[cfg(unix)]
impl std::os::fd::AsRawFd for Tracked81 {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.source.as_raw_fd()
    }
}
#[cfg(unix)]
impl Drop for Tracked81 {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}
#[cfg(unix)]
fn source81() -> (Tracked81, UnixStream, Arc<AtomicBool>) {
    let (source, writer) = UnixStream::pair().unwrap();
    let closed = Arc::new(AtomicBool::new(false));
    (
        Tracked81 {
            source,
            closed: closed.clone(),
        },
        writer,
        closed,
    )
}

#[cfg(unix)]
#[test]
fn drop_capture_chiude_e_joina_source_con_writer_ancora_aperto() {
    let (source, writer, closed) = source81();
    let capture = drain(source).unwrap();
    drop(capture);
    // Writer ancora vivo: EOF non spiega la raccolta della source.
    assert!(
        closed.load(Ordering::Acquire),
        "Drop Capture deve joinare il lettore e chiudere source prima di tornare"
    );
    drop(writer);
}

#[cfg(unix)]
#[test]
fn finish_capture_drena_output_oltre_cap_e_joina_source() {
    let (source, mut writer, closed) = source81();
    let capture = drain(source).unwrap();
    let sender = std::thread::spawn(move || writer.write_all(&vec![b'x'; MAX_CAPTURE * 3]));
    let result = capture.finish(std::time::Duration::from_secs(2));
    let sent = sender.join(); // raccogliere entrambi prima di assertion/errori.
    sent.unwrap().unwrap();
    let text = result.unwrap();
    assert!(closed.load(Ordering::Acquire));
    assert_eq!(text.len(), MAX_CAPTURE);
    assert!(text.bytes().all(|byte| byte == b'x'));
}

#[cfg(unix)]
struct Fault81(Tracked81, bool);
#[cfg(unix)]
impl Read for Fault81 {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        assert!(!self.1, "panic lettore controllato81, non fault nativo JJ");
        Err(io::Error::other("IO lettore controllato81"))
    }
}
#[cfg(unix)]
impl std::os::fd::AsRawFd for Fault81 {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0.as_raw_fd()
    }
}
#[cfg(unix)]
fn failure_all81(panic: bool) {
    let (source, writer_a, closed_a) = source81();
    let (other, writer_b, closed_b) = source81();
    let a = drain(Fault81(source, panic)).unwrap();
    let b = drain(other).unwrap();
    let result = finish_all(Some(a), Some(b), std::time::Duration::from_millis(100));
    assert!(
        result.is_err(),
        "IO/panic del primo non diventa output positivo"
    );
    assert!(closed_a.load(Ordering::Acquire));
    assert!(
        closed_b.load(Ordering::Acquire),
        "secondo reader joined anche dopo errore del primo"
    );
    drop((writer_a, writer_b));
}
#[cfg(unix)]
#[test]
fn finish_all_tenta_entrambi_dopo_io_controllato() {
    failure_all81(false);
}
#[cfg(unix)]
#[test]
fn finish_all_tenta_entrambi_dopo_panic_controllato() {
    failure_all81(true);
}
#[cfg(unix)]
#[test]
fn abort_all_chiude_entrambi_con_writer_aperti() {
    let (a, writer_a, closed_a) = source81();
    let (b, writer_b, closed_b) = source81();
    let a = drain(a).unwrap();
    let b = drain(b).unwrap();
    let result = abort_all(Some(a), Some(b));
    assert!(closed_a.load(Ordering::Acquire));
    assert!(closed_b.load(Ordering::Acquire));
    result.unwrap();
    drop((writer_a, writer_b));
}

// /proc è un witness Linux dell'assenza dei worker col nome dichiarato dopo
// il ritorno, non PID/process-tree, FD/RAM o indipendente attestazione di join.
#[cfg(target_os = "linux")]
fn workers81() -> io::Result<usize> {
    let mut count = 0;
    for entry in std::fs::read_dir("/proc/self/task")? {
        let comm = std::fs::read_to_string(entry?.path().join("comm"));
        match comm {
            Ok(name) if name.trim() == "lume-jj-pipe" => count += 1,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(count)
}
#[cfg(target_os = "linux")]
#[test]
#[ignore = "JJ reale opt-in: test-jj-pipe-real"]
fn jj_reale_output_refusal_ritorno_senza_worker_nominati() -> Result<(), Box<dyn std::error::Error>>
{
    let program = std::env::var_os("LUME_JJ")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| io::Error::other("jj reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("jj reale non assoluto/file").into());
    }
    let cli = JjCli::locate().ok_or_else(|| io::Error::other("discovery jj produzione assente"))?;
    assert_eq!(cli.program(), program);
    let home = std::env::var_os("LUME_JJ_TEST_HOME")
        .ok_or_else(|| io::Error::other("profilo privato runner mancante"))?;
    assert_eq!(std::env::var_os("HOME"), Some(home));
    let fixture = tempfile::Builder::new()
        .prefix("lume-jj81-città spazio-")
        .tempdir()?;
    let root = fixture.path().to_path_buf();
    let flag = AtomicBool::new(false);
    let scenario = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || -> Result<(), Box<dyn std::error::Error>> {
            assert_eq!(workers81()?, 0, "processo test isolato prima del driver");
            let init = cli.run(&root, &["git", "init"], &flag)?;
            assert!(
                init.success(),
                "init nativo positivo richiesto, raw log non pubblicato"
            );
            assert_eq!(workers81()?, 0);
            std::fs::write(root.join("città.txt"), "uno 😀\n")?;
            let status = cli.run(&root, &["status"], &flag)?;
            assert!(status.success());
            assert!(status.message().contains("città.txt"));
            assert_eq!(workers81()?, 0);
            let no = cli.run(&root, &["--lume-opzione-sconosciuta81"], &flag)?;
            assert!(!no.success());
            assert!(no.code.is_some());
            assert!(!no.stderr.is_empty());
            assert_eq!(workers81()?, 0);
            assert_eq!(std::fs::read_to_string(root.join("città.txt"))?, "uno 😀\n");
            Ok(())
        },
    ));
    match scenario {
        Ok(Ok(())) => fixture.close()?,
        failure => {
            let residue = fixture.keep();
            eprintln!(
                "JJ_PIPE_REALE81:fixture_preservata={}; esito/ownership da verificare",
                residue.display()
            );
            return match failure {
                Ok(Err(error)) => Err(error),
                Err(_) => {
                    Err(io::Error::other("scenario nativo81 panic; fixture conservata").into())
                }
                _ => unreachable!(),
            };
        }
    }
    eprintln!(
        "JJ_PIPE_REALE81:sequenza_completa=native_jj_init_status_real_refusal_return_no_named_readers"
    );
    Ok(())
}
