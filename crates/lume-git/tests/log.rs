#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use lume_git::GitRepository;

#[test]
fn reads_real_commit_graph_messages_authors_and_parents() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_git_log_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    let repository = gix::init(&root).unwrap();
    let tree = repository
        .write_object(gix::objs::Tree::empty())
        .unwrap()
        .detach();
    let signature =
        gix::actor::SignatureRef::from_bytes(b"Lume Test <lume@example.test> 1700000000 +0000")
            .unwrap();

    let first = repository
        .commit_as(
            signature,
            signature,
            "HEAD",
            "primo commit\n\ncorpo iniziale",
            tree,
            std::iter::empty::<gix::ObjectId>(),
        )
        .unwrap()
        .detach();
    let second = repository
        .commit_as(
            signature,
            signature,
            "HEAD",
            "secondo commit\n\ndettagli reali",
            tree,
            [first],
        )
        .unwrap()
        .detach();
    drop(repository);

    let repository = GitRepository::discover(&root).unwrap();
    assert!(repository.branch_name().unwrap().is_some());
    let commits = repository.log(10).unwrap();

    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].id, second.to_string());
    assert_eq!(commits[0].subject, "secondo commit");
    assert_eq!(commits[0].body.as_deref(), Some("dettagli reali"));
    assert_eq!(commits[0].author_name, "Lume Test");
    assert_eq!(commits[0].author_email, "lume@example.test");
    assert_eq!(commits[0].committer_time, 1_700_000_000);
    assert_eq!(commits[0].parent_ids, vec![first.to_string()]);
    assert_eq!(commits[1].id, first.to_string());
    assert!(commits[1].parent_ids.is_empty());
    assert!(repository.log(0).unwrap().is_empty());

    fs::remove_dir_all(root).unwrap();
}
