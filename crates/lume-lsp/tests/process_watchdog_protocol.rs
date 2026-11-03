#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::{ffi::OsString, path::PathBuf};

use lume_lsp::process_watchdog::{Readiness, WatchdogCommand};

#[test]
fn watchdog_command_round_trip_preserves_native_command_data() {
    let expected = WatchdogCommand {
        program: OsString::from("/Applications/Lume Helper/语言-server"),
        args: vec![
            OsString::from("--stdio"),
            OsString::from("value with spaces"),
        ],
        cwd: Some(PathBuf::from("/tmp/project with spaces")),
        env: vec![
            (OsString::from("JAVA_HOME"), OsString::from("/opt/jdk 25")),
            (
                OsString::from("LUME_INTERNAL_WATCHDOG_PROGRAM"),
                OsString::from("server-owned-value"),
            ),
        ],
        parent_pid: 42,
        readiness_socket: PathBuf::from("/tmp/lume-watchdog/ready.sock"),
    };

    let encoded = expected.to_environment().expect("encode watchdog command");
    let actual = WatchdogCommand::from_environment(encoded).expect("decode watchdog command");

    assert_eq!(actual, expected);
}

#[test]
fn watchdog_command_rejects_unbounded_argument_counts() {
    let environment = vec![
        (
            OsString::from("LUME_INTERNAL_WATCHDOG_MODE"),
            OsString::from("1"),
        ),
        (
            OsString::from("LUME_INTERNAL_WATCHDOG_ARG_COUNT"),
            OsString::from("999999"),
        ),
    ];

    let error = WatchdogCommand::from_environment(environment)
        .expect_err("unbounded watchdog arguments must fail closed");

    assert!(error.to_string().contains("argument count"));
}

#[test]
fn readiness_messages_round_trip_and_reject_trailing_bytes() {
    let ready = Readiness::Ready { server_pid: 91 };
    assert_eq!(Readiness::decode(&ready.encode()), Ok(ready));

    let failed = Readiness::Error("could not spawn server".into());
    assert_eq!(Readiness::decode(&failed.encode()), Ok(failed));

    let mut malformed = Readiness::Ready { server_pid: 7 }.encode();
    malformed.extend_from_slice(b"junk");
    assert!(Readiness::decode(&malformed).is_err());
}
