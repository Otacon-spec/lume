#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::time::{Duration, Instant};

use lume_terminal::{TerminalEvent, TerminalSession, TerminalSize};
use portable_pty::CommandBuilder;

fn test_shell() -> CommandBuilder {
    #[cfg(windows)]
    {
        let mut command = CommandBuilder::new("cmd.exe");
        command.arg("/Q");
        command.arg("/K");
        command
    }
    #[cfg(not(windows))]
    {
        CommandBuilder::new("sh")
    }
}

#[test]
fn shell_roundtrip_resize_and_exit_are_real() {
    lume_testkit::richiedi!(lume_testkit::Strumento::ShellPty);
    let session = TerminalSession::spawn(test_shell(), TerminalSize::new(24, 80)).unwrap();
    assert!(session.process_id().is_some());

    session.resize(TerminalSize::new(40, 120)).unwrap();
    assert_eq!(session.size().unwrap(), TerminalSize::new(40, 120));
    session.write(b"echo LUME_PTY_OK\r\nexit\r\n").unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut output_seen = false;
    let mut marker_seen = false;
    let mut exited = None;
    while Instant::now() < deadline && (exited.is_none() || !marker_seen) {
        match session.recv_timeout(Duration::from_millis(250)) {
            Ok(TerminalEvent::Output) => output_seen = true,
            Ok(TerminalEvent::Exited(status)) => exited = Some(status),
            Ok(TerminalEvent::Error(error)) => panic!("PTY reader error: {error}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("PTY event channel closed: {error}"),
        }
        marker_seen = session
            .visible_text()
            .is_ok_and(|rows| rows.iter().any(|row| row.contains("LUME_PTY_OK")));
    }

    assert!(output_seen, "nessuna notifica output PTY");
    assert_eq!(exited, Some(0));

    let screen = session.visible_text().unwrap().join("\n");
    assert!(marker_seen, "griglia emulata: {screen:?}");
}

#[cfg(unix)]
#[test]
fn lo_spawn_unix_imposta_term_e_colorterm_mancanti_nella_shell_reale() {
    lume_testkit::richiedi!(lume_testkit::Strumento::ShellPty);
    let mut command = CommandBuilder::new("sh");
    command.args([
        "-c",
        "printf '\\nLUME_ENV:%s:%s\\n' \"$TERM\" \"$COLORTERM\"",
    ]);
    command.env_remove("TERM");
    command.env_remove("COLORTERM");

    let session = TerminalSession::spawn(command, TerminalSize::new(24, 80)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut exited = false;
    let mut output_seen = false;
    while Instant::now() < deadline && !(exited && output_seen) {
        match session.recv_timeout(Duration::from_millis(250)) {
            Ok(TerminalEvent::Exited(status)) => {
                assert_eq!(status, 0);
                exited = true;
            }
            Ok(TerminalEvent::Error(error)) => panic!("PTY reader error: {error}"),
            Ok(TerminalEvent::Output) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("PTY event channel closed: {error}"),
        }
        output_seen = session
            .visible_text()
            .unwrap()
            .join("\n")
            .contains("LUME_ENV:xterm-256color:truecolor");
    }

    assert!(exited, "la shell di verifica non è terminata");
    let screen = session.visible_text().unwrap().join("\n");
    assert!(
        screen.contains("LUME_ENV:xterm-256color:truecolor"),
        "ambiente shell ricevuto dal PTY: {screen:?}"
    );
}

#[cfg(unix)]
#[test]
fn lo_spawn_unix_conserva_term_e_colorterm_espliciti() {
    lume_testkit::richiedi!(lume_testkit::Strumento::ShellPty);
    let mut command = CommandBuilder::new("sh");
    command.args([
        "-c",
        "printf '\\nLUME_ENV:%s:%s\\n' \"$TERM\" \"$COLORTERM\"",
    ]);
    command.env("TERM", "screen-256color");
    command.env("COLORTERM", "24bit");

    let session = TerminalSession::spawn(command, TerminalSize::new(24, 80)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut exited = false;
    let mut output_seen = false;
    while Instant::now() < deadline && !(exited && output_seen) {
        match session.recv_timeout(Duration::from_millis(250)) {
            Ok(TerminalEvent::Exited(status)) => {
                assert_eq!(status, 0);
                exited = true;
            }
            Ok(TerminalEvent::Error(error)) => panic!("PTY reader error: {error}"),
            Ok(TerminalEvent::Output) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("PTY event channel closed: {error}"),
        }
        output_seen = session
            .visible_text()
            .unwrap()
            .join("\n")
            .contains("LUME_ENV:screen-256color:24bit");
    }

    assert!(exited, "la shell di verifica non è terminata");
    let screen = session.visible_text().unwrap().join("\n");
    assert!(
        screen.contains("LUME_ENV:screen-256color:24bit"),
        "ambiente esplicito sostituito nel PTY: {screen:?}"
    );
}

fn process_exists(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    {
        let Ok(output) = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
        else {
            return false;
        };
        let pid = pid.to_string();
        String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .any(|field| field == pid)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|status| status.success())
    }
}

fn wait_until_process_is_gone(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_exists(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    !process_exists(pid)
}

fn force_kill_process(pid: u32) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
    #[cfg(not(target_os = "windows"))]
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
}

struct CrashHarnessGuard {
    host: std::process::Child,
    pid_file: std::path::PathBuf,
    child_pid: Option<u32>,
}

impl Drop for CrashHarnessGuard {
    fn drop(&mut self) {
        let pid = self.child_pid.or_else(|| {
            std::fs::read_to_string(&self.pid_file)
                .ok()
                .and_then(|value| value.trim().parse().ok())
        });
        let _ = self.host.kill();
        let _ = self.host.wait();
        if let Some(pid) = pid
            && process_exists(pid)
        {
            force_kill_process(pid);
        }
        let _ = std::fs::remove_file(&self.pid_file);
    }
}

#[test]
#[ignore = "subprocess helper for forced-host termination"]
fn crash_host_helper() {
    let Some(pid_file) = std::env::var_os("LUME_CRASH_PID_FILE") else {
        return;
    };
    let session = TerminalSession::spawn(test_shell(), TerminalSize::new(24, 80)).unwrap();
    let pid = session.process_id().expect("PTY child PID");
    std::fs::write(pid_file, pid.to_string()).unwrap();
    loop {
        std::thread::park_timeout(Duration::from_secs(60));
    }
}

#[test]
fn forced_host_termination_leaves_no_pty_child() {
    // `process_exists` restituisce false se tasklist/kill mancano: senza
    // questa riga il test passerebbe dicendo «nessun figlio rimasto».
    lume_testkit::richiedi!(
        lume_testkit::Strumento::ShellPty,
        lume_testkit::Strumento::EseguibileCorrente,
        lume_testkit::Strumento::ElencoProcessi,
        lume_testkit::Strumento::Terminatore,
    );
    let pid_file = std::env::temp_dir().join(format!(
        "lume-pty-crash-{}-{}.pid",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let host = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "crash_host_helper", "--nocapture"])
        .env("LUME_CRASH_PID_FILE", &pid_file)
        .spawn()
        .unwrap();
    let mut harness = CrashHarnessGuard {
        host,
        pid_file,
        child_pid: None,
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    while !harness.pid_file.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    let pid = std::fs::read_to_string(&harness.pid_file)
        .unwrap_or_else(|error| panic!("helper non pronto: {error}"))
        .trim()
        .parse::<u32>()
        .unwrap();
    assert!(process_exists(pid), "il child PTY non è partito");

    harness.child_pid = Some(pid);
    harness.host.kill().unwrap();
    harness.host.wait().unwrap();
    let gone = wait_until_process_is_gone(pid);
    if gone {
        harness.child_pid = None;
    }
    assert!(gone, "child PTY {pid} sopravvissuto al crash dell'host");
}

/// Comando che stampa abbastanza da riempire piu' letture da 16 KB, poi un
/// marcatore che dice «ho finito». Serve a provocare **molti** chunk.
fn comando_output_ampio(marcatore: &str) -> String {
    let riempimento = "X".repeat(48);
    #[cfg(windows)]
    {
        format!("for /L %i in (1,1,800) do @echo LINEA_%i_{riempimento}\r\necho {marcatore}\r\n")
    }
    #[cfg(not(windows))]
    {
        format!(
            "i=1; while [ $i -le 800 ]; do echo \"LINEA_${{i}}_{riempimento}\"; i=$((i+1)); done\necho {marcatore}\n"
        )
    }
}

fn attendi_marcatore(session: &TerminalSession, marcatore: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let visto = session
            .visible_text()
            .is_ok_and(|rows| rows.iter().any(|row| row.contains(marcatore)));
        if visto {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("marcatore «{marcatore}» mai comparso sulla griglia");
}

/// Il risveglio del PTY e' coalescato sul flag di output pendente: finche' la
/// UI non drena, **un solo** risveglio resta in volo per quanti chunk arrivino.
///
/// Senza questa garanzia il ramo push costerebbe un `update()` per ogni lettura
/// da 16 KB — sotto un `cargo build` nel terminale, piu' del tick a 33 ms che
/// ha sostituito. Il secondo atto e' quello che conta davvero: dopo il drain il
/// risveglio deve tornare, altrimenti l'output resterebbe invisibile.
#[test]
fn il_risveglio_pty_e_coalescato_e_torna_dopo_il_drain() {
    lume_testkit::richiedi!(lume_testkit::Strumento::ShellPty);
    let (wakeup_tx, wakeup_rx) = std::sync::mpsc::channel();
    // Il crate chiama una funzione: il canale std resta solo lato test, per
    // contare i risvegli. `Sender` è `Sync`, quindi la chiusura è `Send + Sync`.
    let wakeup: lume_terminal::Wakeup = std::sync::Arc::new(move || {
        let _ = wakeup_tx.send(());
    });
    let session =
        TerminalSession::spawn_with_wakeup(test_shell(), TerminalSize::new(24, 80), Some(wakeup))
            .unwrap();

    // ── Atto 1: molti chunk, nessun drain → esattamente un risveglio. ────────
    session
        .write(comando_output_ampio("MARCA_UNO").as_bytes())
        .unwrap();
    attendi_marcatore(&session, "MARCA_UNO");

    // Il primo si aspetta (la `send` segue di poco l'`advance` che ha reso
    // visibile il marcatore); gli altri, se ci fossero, sono gia' in coda.
    wakeup_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("nessun risveglio dopo l'output");
    let in_piu = wakeup_rx.try_iter().count();
    assert_eq!(
        in_piu, 0,
        "{} risvegli oltre il primo senza che la UI abbia drenato: la coalescenza non tiene",
        in_piu
    );

    // ── Atto 2: la UI drena, il flag si riapre, il risveglio torna. ──────────
    assert!(
        matches!(session.try_recv(), Ok(TerminalEvent::Output)),
        "il drain doveva restituire l'evento Output pendente"
    );
    session
        .write(comando_output_ampio("MARCA_DUE").as_bytes())
        .unwrap();
    attendi_marcatore(&session, "MARCA_DUE");

    wakeup_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("nessun risveglio dopo il drain: output invisibile alla UI");
    let in_piu = wakeup_rx.try_iter().count();
    assert_eq!(in_piu, 0, "{in_piu} risvegli oltre il primo dopo il drain");

    // ── Atto 3: l'uscita del processo sveglia comunque. ──────────────────────
    while session.try_recv().is_ok() {}
    session.write(b"exit\r\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut uscito = false;
    while Instant::now() < deadline && !uscito {
        match session.recv_timeout(Duration::from_millis(250)) {
            Ok(TerminalEvent::Exited(_)) => uscito = true,
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("canale eventi chiuso: {error}"),
        }
    }
    assert!(uscito, "la shell non e' uscita");
    assert!(
        wakeup_rx.try_iter().count() >= 1,
        "l'uscita del processo non ha svegliato la UI"
    );
}
