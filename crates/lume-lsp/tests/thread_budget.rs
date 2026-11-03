#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    unsafe_code
)]
//! Misura i thread aggiunti da ogni server LSP vivo a riposo.

use std::io::{Read, Write};

use lume_lsp::{FrameDecoder, RpcMessage, encode_message};
use serde_json::json;

const HELPER_MODE: &str = "LUME_LSP_THREAD_BUDGET_SERVER";

/// Soglia attesa dalla struttura Windows corrente; va confermata dal test
/// Toolhelp nativo prima di considerarla misurata.
#[cfg(windows)]
const TETTO_THREAD_PER_SERVER: usize = 2;
#[cfg(windows)]
const TOLLERANZA_POOL: usize = 1;

fn fake_server_main() {
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut decoder = FrameDecoder::default();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stdin.read(&mut buffer).expect("read fake server stdin");
        if read == 0 {
            return;
        }
        for frame in decoder.push(&buffer[..read]).expect("decode client frame") {
            let message: RpcMessage = serde_json::from_slice(&frame).expect("parse client message");
            match message {
                RpcMessage::Request(request) => {
                    let result = if request.method == "initialize" {
                        json!({ "capabilities": {} })
                    } else {
                        serde_json::Value::Null
                    };
                    let bytes = encode_message(&RpcMessage::success(request.id, result))
                        .expect("encode fake server response");
                    stdout
                        .write_all(&bytes)
                        .expect("write fake server response");
                    stdout.flush().expect("flush fake server response");
                }
                RpcMessage::Notification(notification) if notification.method == "exit" => return,
                RpcMessage::Notification(_) | RpcMessage::Response(_) => {}
            }
        }
    }
}

#[cfg(windows)]
mod measurement {
    use std::{ffi::OsString, mem::size_of, time::Duration};

    use lume_lsp::{CommandSource, ServerCommand, ServerProcess, TransportConfig};
    use serde_json::{Value, json};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::GetCurrentProcessId,
        },
    };

    use super::{HELPER_MODE, TETTO_THREAD_PER_SERVER, TOLLERANZA_POOL};

    fn process_thread_count() -> usize {
        // SAFETY: GetCurrentProcessId has no preconditions.
        let pid = unsafe { GetCurrentProcessId() };
        // SAFETY: Toolhelp accepts this flag and a zero process ID for a system-wide thread snapshot.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        assert_ne!(
            snapshot,
            INVALID_HANDLE_VALUE,
            "Toolhelp snapshot: {}",
            std::io::Error::last_os_error()
        );
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..THREADENTRY32::default()
        };
        // SAFETY: snapshot is a valid snapshot handle and entry is initialized with dwSize.
        let mut has_entry = unsafe { Thread32First(snapshot, &mut entry) };
        let mut count = 0;
        while has_entry != 0 {
            if entry.th32OwnerProcessID == pid {
                count += 1;
            }
            // SAFETY: snapshot remains open and entry remains writable for the next result.
            has_entry = unsafe { Thread32Next(snapshot, &mut entry) };
        }
        // SAFETY: snapshot is an owned handle returned by CreateToolhelp32Snapshot.
        let closed = unsafe { CloseHandle(snapshot) };
        assert_ne!(
            closed,
            0,
            "CloseHandle(Toolhelp snapshot): {}",
            std::io::Error::last_os_error()
        );
        count
    }

    fn command() -> ServerCommand {
        ServerCommand {
            program: std::env::current_exe().expect("integration test executable"),
            args: Vec::new(),
            cwd: None,
            env: vec![(OsString::from(HELPER_MODE), OsString::from("1"))],
            source: CommandSource::Bundled,
        }
    }

    async fn start_server() -> ServerProcess {
        let process = ServerProcess::spawn(command(), TransportConfig::default())
            .expect("spawn fake language server");
        let _: Value = process
            .transport()
            .request(
                "initialize",
                json!({ "rootUri": null }),
                Duration::from_secs(5),
            )
            .await
            .expect("initialize response");
        process
    }

    pub async fn per_server_thread_budget() {
        let one = start_server().await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let with_one = process_thread_count();

        let mut others = Vec::new();
        for _ in 0..3 {
            others.push(start_server().await);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        let with_four = process_thread_count();
        let added = with_four.saturating_sub(with_one);
        eprintln!(
            "thread_budget: 1 server={with_one}, 4 server={with_four}, {added} thread in più per 3 server ({:.1} ciascuno)",
            added as f64 / 3.0
        );

        for process in others.into_iter().chain(std::iter::once(one)) {
            process
                .shutdown(Duration::from_secs(5))
                .await
                .expect("graceful shutdown");
        }
        assert!(
            added <= 3 * TETTO_THREAD_PER_SERVER + TOLLERANZA_POOL,
            "3 server aggiuntivi costano {added} thread; tetto {TETTO_THREAD_PER_SERVER} per server (+{TOLLERANZA_POOL} di tolleranza del pool)"
        );
        eprintln!("thread_budget::per_server ... ok");
    }
}

fn main() {
    if std::env::var_os(HELPER_MODE).is_some() {
        fake_server_main();
        return;
    }
    #[cfg(windows)]
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Tokio runtime")
        .block_on(measurement::per_server_thread_budget());
    #[cfg(not(windows))]
    eprintln!("thread_budget::per_server ... skipped (misura Windows soltanto)");
}
