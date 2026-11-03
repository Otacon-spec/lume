#![allow(clippy::unwrap_used, clippy::expect_used)]
use lume_lsp::{LspCommand, LspCommandMailbox, MailboxError};
use ropey::Rope;

#[test]
fn snapshot_conserva_revisione_e_coalesce_con_string() {
    let (mailbox, mut receiver) = LspCommandMailbox::with_limits(2, 16);
    let uri: lsp_types::Uri = "file:///test.rs".parse().unwrap();
    let mut rope = Rope::from_str("prima");
    mailbox
        .send(LspCommand::ChangeSnapshot {
            uri: uri.clone(),
            text: rope.clone(),
        })
        .unwrap();
    rope.insert(0, "nuovo ");
    let LspCommand::ChangeSnapshot { text, .. } = receiver.try_recv().unwrap() else {
        panic!("snapshot atteso")
    };
    assert_eq!(text.to_string(), "prima");
    mailbox
        .send(LspCommand::Change {
            uri: uri.clone(),
            text: "stringa".into(),
        })
        .unwrap();
    mailbox
        .send(LspCommand::ChangeSnapshot {
            uri: uri.clone(),
            text: rope,
        })
        .unwrap();
    assert!(matches!(
        receiver.try_recv(),
        Ok(LspCommand::ChangeSnapshot { .. })
    ));
    assert!(receiver.try_recv().is_err());
    mailbox
        .send(LspCommand::ChangeSnapshot {
            uri: uri.clone(),
            text: Rope::from_str("snapshot"),
        })
        .unwrap();
    mailbox
        .send(LspCommand::Change {
            uri,
            text: "finale".into(),
        })
        .unwrap();
    assert!(matches!(receiver.try_recv(), Ok(LspCommand::Change { text, .. }) if text == "finale"));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn budget_snapshot_conta_byte_utf8_e_non_perde_coda_su_rifiuto() {
    let (mailbox, mut receiver) = LspCommandMailbox::with_limits(2, 4);
    let uri: lsp_types::Uri = "file:///test.rs".parse().unwrap();
    mailbox
        .send(LspCommand::ChangeSnapshot {
            uri: uri.clone(),
            text: Rope::from_str("😀"),
        })
        .unwrap();
    assert_eq!(
        mailbox.send(LspCommand::ChangeSnapshot {
            uri,
            text: Rope::from_str("😀x")
        }),
        Err(MailboxError::Full)
    );
    assert!(
        matches!(receiver.try_recv(), Ok(LspCommand::ChangeSnapshot { text, .. }) if text == "😀")
    );
}

#[test]
fn save_e_barriera_per_snapshot_successivo() {
    let (mailbox, mut receiver) = LspCommandMailbox::with_limits(3, 16);
    let uri: lsp_types::Uri = "file:///test.rs".parse().unwrap();
    mailbox
        .send(LspCommand::ChangeSnapshot {
            uri: uri.clone(),
            text: Rope::from_str("prima"),
        })
        .unwrap();
    mailbox.send(LspCommand::Save { uri: uri.clone() }).unwrap();
    mailbox
        .send(LspCommand::ChangeSnapshot {
            uri,
            text: Rope::from_str("dopo"),
        })
        .unwrap();
    assert!(
        matches!(receiver.try_recv(), Ok(LspCommand::ChangeSnapshot { text, .. }) if text == "prima")
    );
    assert!(matches!(receiver.try_recv(), Ok(LspCommand::Save { .. })));
    assert!(
        matches!(receiver.try_recv(), Ok(LspCommand::ChangeSnapshot { text, .. }) if text == "dopo")
    );
}

#[test]
fn ask_non_coalesce_snapshot_di_revisioni_diverse() {
    let (mailbox, mut receiver) = LspCommandMailbox::with_limits(3, 16);
    let uri: lsp_types::Uri = "file:///test.rs".parse().unwrap();
    mailbox
        .send(LspCommand::ChangeSnapshot {
            uri: uri.clone(),
            text: Rope::from_str("prima"),
        })
        .unwrap();
    mailbox
        .send(LspCommand::Ask {
            id: 9,
            request: lume_lsp::LspRequest::Hover {
                uri: uri.clone(),
                position: lsp_types::Position::new(0, 0),
            },
        })
        .unwrap();
    mailbox
        .send(LspCommand::ChangeSnapshot {
            uri,
            text: Rope::from_str("dopo"),
        })
        .unwrap();
    assert!(
        matches!(receiver.try_recv(), Ok(LspCommand::ChangeSnapshot { text, .. }) if text == "prima")
    );
    assert!(matches!(
        receiver.try_recv(),
        Ok(LspCommand::Ask { id: 9, .. })
    ));
    assert!(
        matches!(receiver.try_recv(), Ok(LspCommand::ChangeSnapshot { text, .. }) if text == "dopo")
    );
}
