#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::{ffi::OsString, path::PathBuf};

use lume_lsp::{
    ClientError, CommandSource, ConfigurationFingerprint, LspSession, ServerCommand, ServerProcess,
    TransportConfig, TrustRequired, WorkspaceTrust,
};

fn command(source: CommandSource) -> ServerCommand {
    ServerCommand {
        program: PathBuf::from("language-server"),
        args: vec![OsString::from("--stdio")],
        cwd: Some(PathBuf::from("workspace")),
        env: vec![(OsString::from("MODE"), OsString::from("project"))],
        source,
    }
}

#[test]
fn process_and_session_spawns_reject_workspace_code_before_execution() {
    assert!(matches!(
        ServerProcess::spawn(
            command(CommandSource::Workspace),
            TransportConfig::default()
        ),
        Err(ClientError::TrustRequired {
            decision: WorkspaceTrust::Unknown
        })
    ));
    assert!(matches!(
        LspSession::spawn_with_trust(
            command(CommandSource::Workspace),
            TransportConfig::default(),
            WorkspaceTrust::Denied,
        ),
        Err(lume_lsp::SessionError::Client(ClientError::TrustRequired {
            decision: WorkspaceTrust::Denied
        }))
    ));
}

#[test]
fn configuration_fingerprints_are_deterministic_and_sensitive() {
    let first = ConfigurationFingerprint::from_bytes(b"server = 'one'");
    let same = ConfigurationFingerprint::from_bytes(b"server = 'one'");
    let changed = ConfigurationFingerprint::from_bytes(b"server = 'two'");
    assert_eq!(first, same);
    assert_ne!(first, changed);
    assert_eq!(first.as_hex().len(), 64);
}

#[test]
fn workspace_commands_fail_closed_while_managed_commands_do_not_require_trust() {
    assert!(
        command(CommandSource::Bundled)
            .validate(WorkspaceTrust::Unknown)
            .is_ok()
    );
    assert!(
        command(CommandSource::Managed)
            .validate(WorkspaceTrust::Unknown)
            .is_ok()
    );
    assert!(
        command(CommandSource::SystemPath)
            .validate(WorkspaceTrust::Unknown)
            .is_ok()
    );
    assert!(matches!(
        command(CommandSource::Workspace).validate(WorkspaceTrust::Unknown),
        Err(TrustRequired {
            decision: WorkspaceTrust::Unknown
        })
    ));
    assert!(matches!(
        command(CommandSource::Workspace).validate(WorkspaceTrust::Denied),
        Err(TrustRequired {
            decision: WorkspaceTrust::Denied
        })
    ));
    assert!(
        command(CommandSource::Workspace)
            .validate(WorkspaceTrust::Trusted)
            .is_ok()
    );
}
