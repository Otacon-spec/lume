//! FIFO reale e child posseduto: un open bloccato non deve fermare Cerca.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use lume_core::{SearchOptions, SearchScope, SearchUpdate};
use std::{
    process::{Command, Stdio},
    sync::{Arc, atomic::AtomicU64},
    time::{Duration, Instant},
};

#[test]
#[ignore = "helper eseguito dal parent isolato"]
#[allow(unsafe_code)] // mkfifo solo sulla fixture privata, puntatore verificato sotto
fn child_search_fifo() {
    use std::os::unix::ffi::OsStrExt;
    let root = std::path::PathBuf::from(std::env::var_os("LUME_SEARCH_FIFO_ROOT").unwrap());
    let path = root.join("sostituito.txt");
    let native = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: CString valida, percorso nella fixture privata del parent.
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    let regular = root.join("normale.txt");
    std::fs::write(&regular, "ago reale\n").unwrap();
    let updates: Vec<_> = lume_core::search::spawn_search(
        SearchScope::Aperti(vec![path.clone(), regular.clone()]),
        SearchOptions::literal("ago"),
        Arc::new(AtomicU64::new(1)),
        1,
    )
    .into_iter()
    .collect();
    assert!(matches!(
        updates.last(),
        Some(SearchUpdate::Done {
            files: 1,
            matches: 1,
            ..
        })
    ));
    let mut run = lume_core::spawn_search_owned(
        SearchScope::Aperti(vec![path, regular]),
        SearchOptions::literal("ago"),
        Arc::new(AtomicU64::new(1)),
        1,
    )
    .unwrap();
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let mut terminal = false;
    loop {
        match run.events.poll_next(&mut context) {
            std::task::Poll::Ready(Some(SearchUpdate::Done {
                files: 1,
                matches: 1,
                ..
            })) => terminal = true,
            std::task::Poll::Ready(Some(SearchUpdate::Batch(_))) => {}
            std::task::Poll::Ready(None) => break,
            std::task::Poll::Pending => std::thread::sleep(Duration::from_millis(1)),
            other => panic!("esito owned inatteso: {other:?}"),
        }
    }
    assert!(terminal);
    assert!(run.worker.join().is_ok());
    println!("LUME_SEARCH_FIFO_COMPLETE");
}

#[test]
fn fifo_non_blocca_ricerca_e_child_raccolto() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_search_fifo", "--ignored", "--nocapture"])
        .env("LUME_SEARCH_FIFO_ROOT", dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut timeout = false;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            timeout = true;
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !timeout,
        "open FIFO bloccato: child ucciso e raccolto; {stdout}"
    );
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("LUME_SEARCH_FIFO_COMPLETE") && stdout.contains("1 passed; 0 failed"));
}
