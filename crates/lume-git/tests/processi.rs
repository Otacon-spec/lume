#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Git vero, shell/helper veri e PID reali; verifica cleanup del gruppo e dei
//! discendenti escaped adottati dal supervisor Linux.
mod common;
use lume_git::GitError;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

static SERIAL: Mutex<()> = Mutex::new(());
fn readers() -> usize {
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|s| s.trim() == "lume-git-pipe")
        })
        .count()
}
fn pid(path: &std::path::Path) -> i32 {
    std::fs::read_to_string(path)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}
fn not_running(pid: i32) {
    let start = Instant::now();
    loop {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Ok(stat)
                if stat
                    .rsplit_once(") ")
                    .is_some_and(|(_, tail)| tail.starts_with('Z')) =>
            {
                return;
            }
            _ => {}
        }
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "PID {pid} ancora eseguibile dopo cleanup"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn running(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, tail)| !tail.starts_with('Z'))
    })
}

#[test]
fn cancel_termina_helper_nel_gruppo_e_joina_entrambi_i_lettori() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let _serial = SERIAL.lock().unwrap();
    let repo = common::init_repo();
    assert_eq!(readers(), 0);
    let flag = Arc::new(AtomicBool::new(false));
    let cancel = flag.clone();
    let dir = repo.path().to_path_buf();
    let worker = std::thread::spawn(move || {
        common::cli().run(&dir,&["-c","alias.attendi=!sleep 30 & echo $! > helper.pid; echo $$ > shell.pid; echo pronto; wait","attendi"],&cancel)
    });
    let start = Instant::now();
    while !repo.path().join("shell.pid").exists()
        || std::fs::read_to_string(repo.path().join("shell.pid")).is_ok_and(|s| s.is_empty())
    {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(5));
    }
    let helper = pid(&repo.path().join("helper.pid"));
    let shell = pid(&repo.path().join("shell.pid"));
    assert!(readers() > 0);
    flag.store(true, Ordering::Release);
    assert!(matches!(worker.join().unwrap(), Err(GitError::Cancelled)));
    assert!(start.elapsed() < Duration::from_secs(10));
    not_running(helper);
    not_running(shell);
    assert_eq!(readers(), 0, "run non ritorna lasciando lettori staccati");
}

#[test]
fn successo_con_helper_che_tiene_pipe_aperta_non_trattiene_thread() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let _serial = SERIAL.lock().unwrap();
    let repo = common::init_repo();
    let start = Instant::now();
    let output = common::cli()
        .run(
            repo.path(),
            &[
                "-c",
                "alias.background=!sleep 30 & echo $! > helper.pid; echo pronto",
                "background",
            ],
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(output.success());
    assert_eq!(output.stdout.trim(), "pronto");
    assert!(start.elapsed() < Duration::from_secs(5));
    not_running(pid(&repo.path().join("helper.pid")));
    assert_eq!(readers(), 0);
}

#[test]
fn stderr_e_stdout_limitati_sono_svuotati_e_joinati_anche_su_exit_nonzero() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let _serial = SERIAL.lock().unwrap();
    let repo = common::init_repo();
    let output = common::cli()
        .run(
            repo.path(),
            &[
                "-c",
                "alias.errore=!printf '%0200000d' 0; printf '%0200000d' 0 >&2; exit 7",
                "errore",
            ],
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(output.code, Some(7));
    assert_eq!(output.stdout.len(), 64 * 1024);
    assert_eq!(output.stderr.len(), 64 * 1024);
    assert_eq!(readers(), 0);
}

#[test]
fn successo_non_lascia_helper_che_evadono_il_gruppo_con_setsid() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    if std::process::Command::new("setsid")
        .arg("--help")
        .output()
        .is_err()
    {
        return;
    }
    let _serial = SERIAL.lock().unwrap();
    let repo = common::init_repo();
    let start = Instant::now();
    let output = common::cli()
        .run(
            repo.path(),
            &[
                "-c",
                "alias.escape=!setsid sh -c 'sleep 30 & echo $! > escaped.pid'",
                "escape",
            ],
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(output.success(), "{}", output.failure_text());
    let escaped = pid(&repo.path().join("escaped.pid"));
    let escaped_running = running(escaped);
    if escaped_running {
        // Il test deve fallire sul wrapper attuale, ma non lasciare un helper
        // di 30 secondi fuori dal gruppo nella macchina di test.
        let _ = std::process::Command::new("/bin/kill")
            .args(["-KILL", &escaped.to_string()])
            .status();
        not_running(escaped);
    }
    assert!(
        !escaped_running,
        "helper setsid {escaped} è sopravvissuto al ritorno di Git"
    );
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(readers(), 0);
}

#[test]
fn cancel_termina_anche_il_helper_escaped_e_il_suo_discendente() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    if std::process::Command::new("setsid")
        .arg("--help")
        .output()
        .is_err()
    {
        return;
    }
    let _serial = SERIAL.lock().unwrap();
    let repo = common::init_repo();
    let flag = Arc::new(AtomicBool::new(false));
    let cancel = flag.clone();
    let dir = repo.path().to_path_buf();
    let worker = std::thread::spawn(move || {
        common::cli().run(
            &dir,
            &[
                "-c",
                "alias.escape=!setsid sh -c 'sleep 30 & echo $! > escaped-child.pid; echo $$ > escaped-shell.pid; wait'",
                "escape",
            ],
            &cancel,
        )
    });
    let start = Instant::now();
    while !repo.path().join("escaped-child.pid").exists()
        || !repo.path().join("escaped-shell.pid").exists()
    {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(5));
    }
    let escaped_child = pid(&repo.path().join("escaped-child.pid"));
    let escaped_shell = pid(&repo.path().join("escaped-shell.pid"));
    flag.store(true, Ordering::Release);
    assert!(matches!(worker.join().unwrap(), Err(GitError::Cancelled)));
    not_running(escaped_child);
    not_running(escaped_shell);
    assert_eq!(readers(), 0);
}
