#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    unsafe_code
)]
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child as StdChild, Command as StdCommand, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use lume_lsp::{
    ClientError, CommandSource, FrameDecoder, RpcMessage, STDERR_CAPACITY, ServerCommand,
    ServerProcess, TransportConfig, TransportEvent, encode_message,
};
use serde_json::{Value, json};

const HELPER_MODE: &str = "LUME_LSP_FAKE_SERVER";
const RECORD_FILE: &str = "LUME_LSP_FAKE_RECORD_FILE";
const CRASH_METHOD: &str = "LUME_LSP_FAKE_CRASH_METHOD";
const DELAY_MS: &str = "LUME_LSP_FAKE_DELAY_MS";
const EMIT_DIAGNOSTICS: &str = "LUME_LSP_FAKE_EMIT_DIAGNOSTICS";
const IGNORE_SHUTDOWN: &str = "LUME_LSP_FAKE_IGNORE_SHUTDOWN";
const STDERR_BYTES: &str = "LUME_LSP_FAKE_STDERR_BYTES";
const HOST_PID_FILE: &str = "LUME_LSP_FORCED_HOST_PID_FILE";
const HOLD_STDERR_MODE: &str = "LUME_LSP_HOLD_STDERR";
const SPAWN_STDERR_DESCENDANT: &str = "LUME_LSP_SPAWN_STDERR_DESCENDANT";
const DESCENDANT_PID_FILE: &str = "LUME_LSP_DESCENDANT_PID_FILE";
#[cfg(windows)]
const WINDOWS_PROBE_MODE: &str = "windows_spawn_contract_helper";
#[cfg(windows)]
const WINDOWS_PROBE_FILE: &str = "LUME_LSP_WINDOWS_PROBE_FILE";
#[cfg(windows)]
const WINDOWS_UNICODE_ENV: &str = "LUME_LSP_Ä_CASE";
#[cfg(windows)]
const WINDOWS_PATH_HOST_MODE: &str = "windows_effective_path_host_helper";
#[cfg(windows)]
const WINDOWS_PATH_PROGRAM: &str = "LUME_LSP_WINDOWS_PATH_PROGRAM";
#[cfg(windows)]
const WINDOWS_PATH_OVERRIDE: &str = "LUME_LSP_WINDOWS_PATH_OVERRIDE";
#[cfg(windows)]
const WINDOWS_PATH_CHILD_CWD: &str = "LUME_LSP_WINDOWS_PATH_CHILD_CWD";

fn append_record(method: &str) {
    let path = std::env::var_os(RECORD_FILE).expect("fake server record path");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open fake server record");
    writeln!(file, "{method}").expect("record fake server method");
}

fn fake_server_main() {
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let mut decoder = FrameDecoder::default();
    let mut buffer = [0_u8; 4096];
    let delay = std::env::var(DELAY_MS)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis);
    let ignore_shutdown = std::env::var_os(IGNORE_SHUTDOWN).is_some();
    let crash_method = std::env::var(CRASH_METHOD).ok();
    let _stderr_descendant = std::env::var_os(SPAWN_STDERR_DESCENDANT).map(|_| {
        let pid_file = std::env::var_os(DESCENDANT_PID_FILE).expect("descendant pid file");
        let child = StdCommand::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "hold_stderr_helper", "--nocapture"])
            .env_remove(HELPER_MODE)
            .env(HOLD_STDERR_MODE, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn stderr descendant");
        std::fs::write(pid_file, child.id().to_string()).expect("publish descendant pid");
        child
    });

    if let Some(stderr_bytes) = std::env::var(STDERR_BYTES)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
    {
        stderr.write_all(b"obsolete-prefix:").unwrap();
        let fill = vec![b'x'; stderr_bytes];
        stderr.write_all(&fill).unwrap();
        stderr.write_all(b"::newest-stderr").unwrap();
        stderr.flush().unwrap();
    }

    loop {
        let read = stdin.read(&mut buffer).expect("read fake server stdin");
        if read == 0 {
            if ignore_shutdown {
                loop {
                    thread::park_timeout(Duration::from_secs(60));
                }
            }
            return;
        }

        for frame in decoder.push(&buffer[..read]).expect("decode client frame") {
            let message: RpcMessage = serde_json::from_slice(&frame).expect("parse client message");
            let (method, response_id) = match message {
                RpcMessage::Request(request) => (request.method, Some(request.id)),
                RpcMessage::Notification(notification) => (notification.method, None),
                RpcMessage::Response(_) => continue,
            };
            append_record(&method);
            if crash_method.as_deref() == Some(method.as_str()) {
                std::process::exit(23);
            }
            if let Some(delay) = delay {
                thread::sleep(delay);
            }
            if ignore_shutdown && (method == "shutdown" || method == "exit") {
                continue;
            }

            if let Some(id) = response_id {
                let result = if method == "initialize" {
                    json!({ "capabilities": {} })
                } else {
                    Value::Null
                };
                let bytes = encode_message(&RpcMessage::success(id, result))
                    .expect("encode fake server response");
                stdout
                    .write_all(&bytes)
                    .expect("write fake server response");
                stdout.flush().expect("flush fake server response");

                if method == "initialize" && std::env::var_os(EMIT_DIAGNOSTICS).is_some() {
                    let diagnostic = RpcMessage::notification(
                        "textDocument/publishDiagnostics",
                        json!({ "uri": "file:///fixture", "diagnostics": [] }),
                    );
                    let bytes = encode_message(&diagnostic).expect("encode fake diagnostic");
                    stdout.write_all(&bytes).expect("write fake diagnostic");
                    stdout.flush().expect("flush fake diagnostic");
                }
            }

            if method == "exit" {
                return;
            }
        }
    }
}

fn fake_server_helper() {
    fake_server_main();
}

struct TempFile(PathBuf);

impl TempFile {
    fn unique(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "lume-lsp-{label}-{}-{nonce}.txt",
            std::process::id()
        )))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(windows)]
fn file_is_nonempty(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0)
}

#[cfg(windows)]
struct TempDir(PathBuf);

#[cfg(windows)]
impl TempDir {
    fn unique_with_spaces(label: &str) -> Self {
        let path = TempFile::unique(label).0.with_extension("dir with spaces");
        std::fs::create_dir(&path).expect("create temporary directory");
        Self(path)
    }
}

#[cfg(windows)]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fake_command(record: &Path, extra_env: &[(&str, String)]) -> ServerCommand {
    let mut env = vec![
        (OsString::from(HELPER_MODE), OsString::from("1")),
        (OsString::from(RECORD_FILE), record.as_os_str().to_owned()),
    ];
    env.extend(
        extra_env
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value))),
    );
    ServerCommand {
        program: std::env::current_exe().expect("current integration-test executable"),
        args: vec![
            OsString::from("--ignored"),
            OsString::from("--exact"),
            OsString::from("fake_server_helper"),
            OsString::from("--nocapture"),
        ],
        cwd: None,
        env,
        source: CommandSource::Bundled,
    }
}

async fn graceful_shutdown_records_the_lsp_sequence() {
    let record = TempFile::unique("graceful");
    let mut process = ServerProcess::spawn(
        fake_command(&record.0, &[(EMIT_DIAGNOSTICS, "1".into())]),
        TransportConfig::default(),
    )
    .expect("spawn fake language server");
    let pid = process.process_id();
    assert!(process_exists(pid), "fake server {pid} did not start");
    let transport = process.transport().clone();

    let _: Value = transport
        .request(
            "initialize",
            json!({ "rootUri": null }),
            Duration::from_secs(2),
        )
        .await
        .expect("initialize response");
    let event = tokio::time::timeout(Duration::from_secs(2), process.events().recv())
        .await
        .expect("diagnostic deadline")
        .expect("diagnostic event");
    assert!(matches!(
        event,
        TransportEvent::Notification(notification) if notification.method == "textDocument/publishDiagnostics"
    ));
    transport.notify("initialized", json!({})).unwrap();
    transport
        .notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": "file:///fixture", "version": 1, "text": "a" } }),
        )
        .unwrap();
    transport
        .notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": "file:///fixture", "version": 2 }, "contentChanges": [{ "text": "b" }] }),
        )
        .unwrap();

    let status = process
        .shutdown(Duration::from_secs(2))
        .await
        .expect("graceful shutdown");
    assert!(status.success(), "fake server status: {status:?}");
    assert!(wait_until_process_is_gone(pid));

    let methods = std::fs::read_to_string(&record.0).expect("fake server method record");
    assert_eq!(
        methods.lines().collect::<Vec<_>>(),
        [
            "initialize",
            "initialized",
            "textDocument/didOpen",
            "textDocument/didChange",
            "shutdown",
            "exit",
        ]
    );
}

fn process_exists(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{
            Foundation::{ERROR_INVALID_PARAMETER, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        };

        // SAFETY: `OpenProcess` non ha precondizioni sulla memoria; l'esito viene controllato subito dopo.
        let raw_process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if raw_process.is_null() {
            return std::io::Error::last_os_error().raw_os_error()
                != Some(ERROR_INVALID_PARAMETER as i32);
        }
        // SAFETY: `raw_process` non è nullo (controllato sopra) ed è un handle appena aperto: `OwnedHandle` ne prende la proprietà.
        let process = unsafe { OwnedHandle::from_raw_handle(raw_process) };
        // SAFETY: `process` è un handle valido aperto con il diritto di sincronizzazione; il timeout 0 non blocca.
        match unsafe { WaitForSingleObject(process.as_raw_handle() as HANDLE, 0) } {
            WAIT_OBJECT_0 => false,
            WAIT_TIMEOUT => true,
            _ => true,
        }
    }
    #[cfg(not(windows))]
    {
        StdCommand::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|status| status.success())
    }
}

fn wait_until_process_is_gone(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_exists(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    !process_exists(pid)
}

fn force_kill_process(pid: u32) -> Result<(), String> {
    #[cfg(windows)]
    let status = StdCommand::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("could not run taskkill for {pid}: {error}"))?;
    #[cfg(not(windows))]
    let status = StdCommand::new("kill")
        .args(["-9", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("could not run kill for {pid}: {error}"))?;
    if !status.success() && process_exists(pid) {
        return Err(format!("forced cleanup command for {pid} failed: {status}"));
    }
    if !wait_until_process_is_gone(pid) {
        return Err(format!("process {pid} survived forced cleanup"));
    }
    Ok(())
}

struct PublishedPidCleanup<'a> {
    pid_file: &'a Path,
    disarmed: bool,
}

impl Drop for PublishedPidCleanup<'_> {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        if let Ok(pid) = std::fs::read_to_string(self.pid_file)
            && let Ok(pid) = pid.trim().parse::<u32>()
            && process_exists(pid)
            && let Err(error) = force_kill_process(pid)
        {
            eprintln!("stderr-descendant cleanup failed: {error}");
        }
    }
}

async fn crash_is_reported_and_reaped() {
    let record = TempFile::unique("crash");
    let process = ServerProcess::spawn(
        fake_command(&record.0, &[(CRASH_METHOD, "crash".into())]),
        TransportConfig::default(),
    )
    .unwrap();
    let pid = process.process_id();
    let result = process
        .transport()
        .request::<_, Value>("crash", Value::Null, Duration::from_secs(2))
        .await;
    assert!(matches!(
        result,
        Err(ClientError::Disconnected | ClientError::Io(_))
    ));
    assert!(process.shutdown(Duration::from_millis(250)).await.is_err());
    assert!(
        wait_until_process_is_gone(pid),
        "crashed server {pid} not reaped"
    );
}

async fn shutdown_timeout_kills_and_reaps() {
    let record = TempFile::unique("timeout");
    let process = ServerProcess::spawn(
        fake_command(&record.0, &[(IGNORE_SHUTDOWN, "1".into())]),
        TransportConfig::default(),
    )
    .unwrap();
    let pid = process.process_id();
    let started = Instant::now();
    let result = process.shutdown(Duration::from_millis(150)).await;
    assert!(
        matches!(
            result,
            Err(ClientError::Timeout {
                method: "shutdown",
                ..
            })
        ),
        "unexpected timed shutdown result: {result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        wait_until_process_is_gone(pid),
        "timed-out server {pid} survived"
    );
}

async fn exhausted_shutdown_deadline_hands_off_reaping() {
    let record = TempFile::unique("zero-deadline");
    let process = ServerProcess::spawn(
        fake_command(&record.0, &[(IGNORE_SHUTDOWN, "1".into())]),
        TransportConfig::default(),
    )
    .unwrap();
    let pid = process.process_id();
    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_millis(250), process.shutdown(Duration::ZERO))
        .await
        .expect("shutdown exceeded its exhausted deadline");

    assert!(
        matches!(
            result,
            Err(ClientError::Timeout {
                method: "shutdown",
                ..
            })
        ),
        "unexpected zero-deadline result: {result:?}"
    );
    assert!(started.elapsed() < Duration::from_millis(250));
    assert!(
        wait_until_process_is_gone(pid),
        "detached shutdown reaper left server {pid} alive"
    );
}

async fn drop_returns_quickly_and_reaps_the_server() {
    let record = TempFile::unique("drop-reaper");
    let process = ServerProcess::spawn(
        fake_command(&record.0, &[(IGNORE_SHUTDOWN, "1".into())]),
        TransportConfig::default(),
    )
    .unwrap();
    let pid = process.process_id();
    assert!(process_exists(pid), "drop probe {pid} did not start");

    let started = Instant::now();
    drop(process);
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "ServerProcess::drop blocked for {:?}",
        started.elapsed()
    );
    assert!(
        wait_until_process_is_gone(pid),
        "Drop reaper left server {pid} alive"
    );
}

async fn stderr_ring_keeps_only_the_newest_256_kib() {
    let record = TempFile::unique("stderr");
    let process = ServerProcess::spawn(
        fake_command(
            &record.0,
            &[(STDERR_BYTES, (STDERR_CAPACITY + 64 * 1024).to_string())],
        ),
        TransportConfig::default(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let snapshot = loop {
        let snapshot = process.stderr_snapshot();
        if snapshot.ends_with(b"::newest-stderr") {
            break snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "stderr reader did not drain fake server"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert_eq!(snapshot.len(), STDERR_CAPACITY);
    assert!(!snapshot.starts_with(b"obsolete-prefix:"));
    process.shutdown(Duration::from_secs(2)).await.unwrap();
}

async fn inherited_stderr_descendant_does_not_extend_shutdown_deadline() {
    let record = TempFile::unique("stderr-descendant");
    let pid_file = TempFile::unique("stderr-descendant-pid");
    let mut cleanup = PublishedPidCleanup {
        pid_file: &pid_file.0,
        disarmed: false,
    };
    let process = ServerProcess::spawn(
        fake_command(
            &record.0,
            &[
                (SPAWN_STDERR_DESCENDANT, "1".into()),
                (
                    DESCENDANT_PID_FILE,
                    pid_file.0.to_string_lossy().into_owned(),
                ),
            ],
        ),
        TransportConfig::default(),
    )
    .unwrap();
    let ready_deadline = Instant::now() + Duration::from_secs(5);
    while !pid_file.0.exists() && Instant::now() < ready_deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let descendant_pid = std::fs::read_to_string(&pid_file.0)
        .expect("fake server did not publish stderr descendant PID")
        .trim()
        .parse::<u32>()
        .unwrap();
    assert!(process_exists(descendant_pid));

    let started = Instant::now();
    let shutdown = tokio::time::timeout(
        Duration::from_millis(750),
        process.shutdown(Duration::from_millis(200)),
    )
    .await;
    assert!(
        shutdown.is_ok(),
        "shutdown remained blocked on inherited stderr"
    );
    assert!(started.elapsed() < Duration::from_millis(750));
    assert!(wait_until_process_is_gone(descendant_pid));
    cleanup.disarmed = true;
}

#[cfg(windows)]
fn windows_probe_command(probe_file: &Path, cwd: &Path) -> (ServerCommand, Vec<String>) {
    let expected_args = vec![
        "plain".to_owned(),
        "two words".to_owned(),
        "quote\"inside".to_owned(),
        "trailing slash\\".to_owned(),
        "slashes\\\\\\\"quote".to_owned(),
        String::new(),
    ];
    let mut args = vec![OsString::from(WINDOWS_PROBE_MODE)];
    args.extend(expected_args.iter().map(OsString::from));
    (
        ServerCommand {
            program: std::env::current_exe().expect("current integration-test executable"),
            args,
            cwd: Some(cwd.to_owned()),
            env: vec![
                (
                    OsString::from(WINDOWS_PROBE_FILE),
                    probe_file.as_os_str().to_owned(),
                ),
                (OsString::from(WINDOWS_UNICODE_ENV), OsString::from("old")),
                (OsString::from("lume_lsp_ä_case"), OsString::from("new")),
            ],
            source: CommandSource::Bundled,
        },
        expected_args,
    )
}

#[cfg(windows)]
async fn windows_spawn_preserves_argv_unicode_env_and_spaced_cwd() {
    let probe_file = TempFile::unique("windows-spawn-contract");
    let cwd = TempDir::unique_with_spaces("windows-spawn-cwd");
    let (command, expected_args) = windows_probe_command(&probe_file.0, &cwd.0);
    let process = ServerProcess::spawn(command, TransportConfig::default())
        .expect("spawn Windows contract probe");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !file_is_nonempty(&probe_file.0) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let probe: Value = serde_json::from_slice(
        &std::fs::read(&probe_file.0).expect("Windows contract probe did not publish"),
    )
    .expect("parse Windows contract probe");

    assert_eq!(probe["args"], json!(expected_args));
    assert_eq!(probe["env"], "new");
    assert_eq!(probe["cwd"], cwd.0.to_string_lossy().as_ref());

    let pid = process.process_id();
    drop(process);
    assert!(wait_until_process_is_gone(pid), "probe {pid} survived Drop");
}

#[cfg(windows)]
async fn windows_server_starts_inside_a_job() {
    let probe_file = TempFile::unique("windows-first-instruction-job");
    let cwd = TempDir::unique_with_spaces("windows-first-instruction-cwd");
    let (command, _) = windows_probe_command(&probe_file.0, &cwd.0);
    let process = ServerProcess::spawn(command, TransportConfig::default())
        .expect("spawn Windows first-instruction probe");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !file_is_nonempty(&probe_file.0) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let probe: Value = serde_json::from_slice(
        &std::fs::read(&probe_file.0).expect("Windows first-instruction probe did not publish"),
    )
    .expect("parse Windows first-instruction probe");
    let pid = process.process_id();

    assert_eq!(
        probe["in_job_on_entry"], true,
        "server {pid} ran before Job assignment"
    );

    drop(process);
    assert!(wait_until_process_is_gone(pid), "probe {pid} survived Drop");
}

#[cfg(windows)]
async fn windows_system_path_program_is_resolved() {
    let process = ServerProcess::spawn(
        ServerCommand {
            program: PathBuf::from("where"),
            args: vec![OsString::from("cmd.exe")],
            cwd: None,
            env: Vec::new(),
            source: CommandSource::SystemPath,
        },
        TransportConfig::default(),
    )
    .expect("resolve bare Windows SystemPath command with implicit .exe");
    let pid = process.process_id();

    drop(process);
    assert!(
        wait_until_process_is_gone(pid),
        "SystemPath probe {pid} survived Drop"
    );
}

#[cfg(windows)]
async fn windows_effective_path_host_helper() {
    let probe_file =
        PathBuf::from(std::env::var_os(WINDOWS_PROBE_FILE).expect("Windows PATH probe file"));
    let program =
        PathBuf::from(std::env::var_os(WINDOWS_PATH_PROGRAM).expect("Windows PATH probe program"));
    let override_path =
        std::env::var_os(WINDOWS_PATH_OVERRIDE).expect("Windows child PATH override");
    let child_cwd = PathBuf::from(
        std::env::var_os(WINDOWS_PATH_CHILD_CWD).expect("Windows child working directory"),
    );
    let process = ServerProcess::spawn(
        ServerCommand {
            program,
            args: vec![OsString::from(WINDOWS_PROBE_MODE)],
            cwd: Some(child_cwd),
            env: vec![
                (OsString::from("pAtH"), override_path),
                // Rust's Windows Command resolver appends `.exe` for an
                // extensionless bare name; it deliberately does not consult PATHEXT.
                (OsString::from("PaThExT"), OsString::from(".CMD;.BAT")),
                (
                    OsString::from(WINDOWS_PROBE_FILE),
                    probe_file.as_os_str().to_owned(),
                ),
                (OsString::from(WINDOWS_UNICODE_ENV), OsString::from("path")),
            ],
            source: CommandSource::SystemPath,
        },
        TransportConfig::default(),
    )
    .expect("resolve bare program through effective child PATH");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !file_is_nonempty(&probe_file) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        file_is_nonempty(&probe_file),
        "child PATH probe did not publish"
    );
    let pid = process.process_id();
    drop(process);
    assert!(
        wait_until_process_is_gone(pid),
        "child PATH probe {pid} survived Drop"
    );
}

#[cfg(windows)]
async fn windows_bare_program_uses_effective_path_and_child_cwd() {
    let root = TempDir::unique_with_spaces("windows-effective-path");
    let parent_collision = root.0.join("parent collision");
    let child_cwd = root.0.join("child cwd");
    let mut child_search = root.0.join("child override");
    while child_search.as_os_str().encode_wide().count() < 280 {
        child_search.push("long executable directory segment");
    }
    std::fs::create_dir_all(&parent_collision).expect("create parent collision directory");
    std::fs::create_dir_all(&child_cwd).expect("create child working directory");
    std::fs::create_dir_all(&child_search).expect("create long child PATH directory");

    let bare_program = "lume-isolated-path-probe";
    let executable_name = format!("{bare_program}.exe");
    let current_exe = std::env::current_exe().expect("current integration-test executable");
    let parent_candidate = parent_collision.join(&executable_name);
    let child_candidate = child_search.join(&executable_name);
    std::fs::copy(&current_exe, &parent_candidate).expect("copy parent collision executable");
    std::fs::copy(&current_exe, &child_candidate).expect("copy child PATH executable");
    assert!(
        child_candidate.as_os_str().encode_wide().count() > 260,
        "PATH executable must exercise a long absolute application name"
    );

    let probe_file = root.0.join("effective-path-probe.json");
    let mut host = StdCommand::new(&current_exe);
    host.args([
        "--ignored",
        "--exact",
        WINDOWS_PATH_HOST_MODE,
        "--nocapture",
    ])
    .current_dir(&parent_collision)
    .env("PATH", &parent_collision)
    .env("PATHEXT", ".COM;.EXE")
    .env(WINDOWS_PROBE_FILE, &probe_file)
    .env(WINDOWS_PATH_PROGRAM, bare_program)
    .env(WINDOWS_PATH_OVERRIDE, &child_search)
    .env(WINDOWS_PATH_CHILD_CWD, &child_cwd)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped());
    let output = host.output().expect("run isolated Windows PATH host");
    assert!(
        output.status.success(),
        "isolated Windows PATH host failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let probe: Value = serde_json::from_slice(
        &std::fs::read(&probe_file).expect("effective PATH probe did not publish"),
    )
    .expect("parse effective PATH probe");
    let actual_exe = PathBuf::from(probe["exe"].as_str().expect("probe executable path"));
    assert_eq!(
        actual_exe
            .canonicalize()
            .expect("canonical probe executable"),
        child_candidate
            .canonicalize()
            .expect("canonical child PATH executable"),
        "parent cwd/PATH collision won over the effective child PATH"
    );
    assert_eq!(probe["cwd"], child_cwd.to_string_lossy().as_ref());
}

#[cfg(windows)]
async fn windows_invalid_path_candidate_does_not_mask_relative_executable() {
    let root = TempDir::unique_with_spaces("windows-relative-effective-path");
    let child_cwd = root.0.join("child cwd");
    let invalid_search = child_cwd.join("invalid candidate");
    let valid_search = child_cwd.join("relative executable");
    std::fs::create_dir_all(&invalid_search).expect("create invalid PATH directory");
    std::fs::create_dir_all(&valid_search).expect("create valid PATH directory");

    let bare_program = "lume-relative-path-probe";
    let executable_name = format!("{bare_program}.exe");
    std::fs::create_dir(invalid_search.join(&executable_name))
        .expect("create directory that must not count as an executable");
    let valid_candidate = valid_search.join(&executable_name);
    let current_exe = std::env::current_exe().expect("current integration-test executable");
    std::fs::copy(&current_exe, &valid_candidate).expect("copy relative PATH executable");

    let relative_path = std::env::join_paths([
        PathBuf::from("invalid candidate"),
        PathBuf::from("relative executable"),
    ])
    .expect("join relative child PATH entries");
    let probe_file = root.0.join("relative-effective-path-probe.json");
    let output = StdCommand::new(&current_exe)
        .args([
            "--ignored",
            "--exact",
            WINDOWS_PATH_HOST_MODE,
            "--nocapture",
        ])
        .env(WINDOWS_PROBE_FILE, &probe_file)
        .env(WINDOWS_PATH_PROGRAM, bare_program)
        .env(WINDOWS_PATH_OVERRIDE, relative_path)
        .env(WINDOWS_PATH_CHILD_CWD, &child_cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("run relative Windows PATH host");
    assert!(
        output.status.success(),
        "relative PATH host failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let probe: Value = serde_json::from_slice(
        &std::fs::read(&probe_file).expect("relative PATH probe did not publish"),
    )
    .expect("parse relative PATH probe");
    let actual_exe = PathBuf::from(probe["exe"].as_str().expect("probe executable path"));
    assert_eq!(
        actual_exe
            .canonicalize()
            .expect("canonical probe executable"),
        valid_candidate
            .canonicalize()
            .expect("canonical relative PATH executable"),
    );
}

struct ForcedHostGuard {
    host: StdChild,
    host_reaped: bool,
    pid_file: TempFile,
    descendant_pid_file: TempFile,
    _record_file: TempFile,
    server_pid: Option<u32>,
    descendant_pid: Option<u32>,
}

impl ForcedHostGuard {
    fn published_server_pid(&self) -> Option<u32> {
        self.server_pid.or_else(|| {
            std::fs::read_to_string(&self.pid_file.0)
                .ok()
                .and_then(|value| value.trim().parse().ok())
        })
    }

    fn published_descendant_pid(&self) -> Option<u32> {
        self.descendant_pid.or_else(|| {
            std::fs::read_to_string(&self.descendant_pid_file.0)
                .ok()
                .and_then(|value| value.trim().parse().ok())
        })
    }

    fn force_host_exit(&mut self) -> Result<(), String> {
        if self.host_reaped {
            return Ok(());
        }
        match self
            .host
            .try_wait()
            .map_err(|error| format!("could not query forced host: {error}"))?
        {
            Some(_) => {}
            None => self
                .host
                .kill()
                .map_err(|error| format!("could not terminate forced host: {error}"))?,
        }
        self.host
            .wait()
            .map_err(|error| format!("could not reap forced host: {error}"))?;
        self.host_reaped = true;
        Ok(())
    }

    fn confirm_tree_gone(&mut self, server_pid: u32, descendant_pid: u32) -> Result<(), String> {
        let mut survivors = Vec::new();
        if process_exists(server_pid) {
            survivors.push(format!("fake server {server_pid}"));
        }
        if process_exists(descendant_pid) {
            survivors.push(format!("server descendant {descendant_pid}"));
        }
        if !survivors.is_empty() {
            return Err(format!(
                "{} survived forced host termination",
                survivors.join(" and ")
            ));
        }
        self.server_pid = None;
        self.descendant_pid = None;
        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        if let Err(error) = self.force_host_exit() {
            errors.push(error);
        }
        for (label, pid) in [
            ("server", self.published_server_pid()),
            ("descendant", self.published_descendant_pid()),
        ] {
            if let Some(pid) = pid
                && process_exists(pid)
                && let Err(error) = force_kill_process(pid)
            {
                errors.push(format!("{label} cleanup failed: {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

impl Drop for ForcedHostGuard {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!("forced-host RAII cleanup failed: {error}");
        }
    }
}

async fn forced_host_helper() {
    let pid_file = PathBuf::from(std::env::var_os(HOST_PID_FILE).expect("host pid file"));
    let descendant_pid_file =
        PathBuf::from(std::env::var_os(DESCENDANT_PID_FILE).expect("descendant pid file"));
    let record_file = PathBuf::from(std::env::var_os(RECORD_FILE).expect("host record file"));
    let process = ServerProcess::spawn(
        fake_command(
            &record_file,
            &[
                (IGNORE_SHUTDOWN, "1".into()),
                (SPAWN_STDERR_DESCENDANT, "1".into()),
                (
                    DESCENDANT_PID_FILE,
                    descendant_pid_file.to_string_lossy().into_owned(),
                ),
            ],
        ),
        TransportConfig::default(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !descendant_pid_file.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        descendant_pid_file.exists(),
        "forced host server did not publish descendant PID"
    );
    std::fs::write(pid_file, process.process_id().to_string()).unwrap();
    std::future::pending::<()>().await;
}

#[cfg(any(windows, unix))]
async fn forced_host_termination_leaves_no_server() {
    let pid_file = TempFile::unique("forced-host-pid");
    let descendant_pid_file = TempFile::unique("forced-host-descendant-pid");
    let record_file = TempFile::unique("forced-host-record");
    let host = StdCommand::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "forced_host_helper", "--nocapture"])
        .env(HOST_PID_FILE, &pid_file.0)
        .env(DESCENDANT_PID_FILE, &descendant_pid_file.0)
        .env(RECORD_FILE, &record_file.0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut guard = ForcedHostGuard {
        host,
        host_reaped: false,
        pid_file,
        descendant_pid_file,
        _record_file: record_file,
        server_pid: None,
        descendant_pid: None,
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while !guard.pid_file.0.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let pid = std::fs::read_to_string(&guard.pid_file.0)
        .unwrap_or_else(|error| panic!("forced host did not publish server PID: {error}"))
        .trim()
        .parse::<u32>()
        .unwrap();
    guard.server_pid = Some(pid);
    let descendant_pid = std::fs::read_to_string(&guard.descendant_pid_file.0)
        .expect("forced host did not publish descendant PID")
        .trim()
        .parse::<u32>()
        .unwrap();
    guard.descendant_pid = Some(descendant_pid);
    assert!(
        process_exists(pid),
        "nested fake server {pid} did not start"
    );
    assert!(
        process_exists(descendant_pid),
        "nested server descendant {descendant_pid} did not start"
    );
    guard
        .force_host_exit()
        .expect("terminate and reap forced host");
    assert!(
        wait_until_process_is_gone(pid),
        "fake server {pid} survived forced host termination"
    );
    assert!(
        wait_until_process_is_gone(descendant_pid),
        "server descendant {descendant_pid} survived forced host termination"
    );
    guard
        .confirm_tree_gone(pid, descendant_pid)
        .expect("verify forced-host server-tree cleanup");
}

#[tokio::main]
async fn main() {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    assert!(
        Path::new(env!("CARGO_BIN_EXE_lume-process-watchdog")).is_file(),
        "Cargo did not build the Unix process-watchdog helper"
    );
    #[cfg(windows)]
    let in_job_on_entry = {
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let mut in_job = 0;
        // SAFETY: `in_job` è una locale viva e scrivibile; `GetCurrentProcess` dà lo pseudo-handle e il job nullo chiede «qualsiasi job».
        let queried =
            unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job) };
        assert_ne!(queried, 0, "query initial Job membership");
        in_job != 0
    };
    #[cfg(windows)]
    if std::env::args().any(|argument| argument == WINDOWS_PROBE_MODE) {
        let args = std::env::args()
            .skip_while(|argument| argument != WINDOWS_PROBE_MODE)
            .skip(1)
            .collect::<Vec<_>>();
        let probe = json!({
            "args": args,
            "env": std::env::var(WINDOWS_UNICODE_ENV).expect("Unicode environment override"),
            "cwd": std::env::current_dir().expect("probe cwd").to_string_lossy(),
            "exe": std::env::current_exe().expect("probe executable").to_string_lossy(),
            "in_job_on_entry": in_job_on_entry,
        });
        std::fs::write(
            std::env::var_os(WINDOWS_PROBE_FILE).expect("Windows probe file"),
            serde_json::to_vec(&probe).expect("encode Windows probe"),
        )
        .expect("publish Windows probe");
        std::future::pending::<()>().await;
        return;
    }
    #[cfg(windows)]
    if std::env::args().any(|argument| argument == WINDOWS_PATH_HOST_MODE) {
        windows_effective_path_host_helper().await;
        return;
    }
    if std::env::var_os(HOLD_STDERR_MODE).is_some()
        && std::env::args().any(|argument| argument == "hold_stderr_helper")
    {
        loop {
            thread::park_timeout(Duration::from_secs(60));
        }
    }
    if std::env::var_os(HELPER_MODE).is_some()
        && std::env::args().any(|argument| argument == "fake_server_helper")
    {
        fake_server_helper();
        return;
    }
    if std::env::var_os(HOST_PID_FILE).is_some()
        && std::env::args().any(|argument| argument == "forced_host_helper")
    {
        forced_host_helper().await;
        return;
    }
    // Su Windows `process_exists` usa l'API di sistema e serve solo
    // `taskkill` per la pulizia; su Unix `kill` fa entrambe le cose.
    if !lume_testkit::richiedi(&[
        lume_testkit::Strumento::EseguibileCorrente,
        lume_testkit::Strumento::Terminatore,
    ]) {
        eprintln!("server_process: saltato");
        return;
    }
    graceful_shutdown_records_the_lsp_sequence().await;
    eprintln!("server_process::graceful ... ok");
    crash_is_reported_and_reaped().await;
    eprintln!("server_process::crash ... ok");
    shutdown_timeout_kills_and_reaps().await;
    eprintln!("server_process::timeout_kill ... ok");
    exhausted_shutdown_deadline_hands_off_reaping().await;
    eprintln!("server_process::shutdown_handoff ... ok");
    drop_returns_quickly_and_reaps_the_server().await;
    eprintln!("server_process::drop_reaper ... ok");
    stderr_ring_keeps_only_the_newest_256_kib().await;
    eprintln!("server_process::stderr_bound ... ok");
    inherited_stderr_descendant_does_not_extend_shutdown_deadline().await;
    eprintln!("server_process::inherited_stderr_deadline ... ok");
    #[cfg(windows)]
    {
        windows_spawn_preserves_argv_unicode_env_and_spaced_cwd().await;
        eprintln!("server_process::windows_spawn_contract ... ok");
        windows_server_starts_inside_a_job().await;
        eprintln!("server_process::windows_first_instruction_job ... ok");
        windows_system_path_program_is_resolved().await;
        eprintln!("server_process::windows_system_path ... ok");
        windows_bare_program_uses_effective_path_and_child_cwd().await;
        eprintln!("server_process::windows_effective_path ... ok");
        windows_invalid_path_candidate_does_not_mask_relative_executable().await;
        eprintln!("server_process::windows_relative_path ... ok");
    }
    #[cfg(any(windows, unix))]
    {
        forced_host_termination_leaves_no_server().await;
        eprintln!("server_process::forced_host_orphan ... ok");
    }
}
