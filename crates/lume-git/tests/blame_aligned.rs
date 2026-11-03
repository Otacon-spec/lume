#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod common;

use std::{fs, process::Command};

use lume_git::GitRepository;

fn git(dir: &std::path::Path, author: &str, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", author)
        .env("GIT_AUTHOR_EMAIL", format!("{author}@example.test"))
        .env("GIT_COMMITTER_NAME", author)
        .env("GIT_COMMITTER_EMAIL", format!("{author}@example.test"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn blame_allineato_marca_le_righe_nuove_nel_buffer() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), "Ada", &["init", "-b", "main"]);
    fs::write(dir.path().join("a.txt"), "uno\ndue\ntre\n").unwrap();
    git(dir.path(), "Ada", &["add", "a.txt"]);
    git(dir.path(), "Ada", &["commit", "-m", "base"]);
    fs::write(dir.path().join("a.txt"), "uno\ndue\nBruno\n").unwrap();
    git(dir.path(), "Bruno", &["add", "a.txt"]);
    git(dir.path(), "Bruno", &["commit", "-m", "secondo"]);
    let repo = GitRepository::discover(dir.path()).unwrap();
    let rows = repo
        .blame_aligned(&dir.path().join("a.txt"), "zero\nuno\nDUE\nBruno\n")
        .unwrap();
    assert_eq!(rows.len(), 4);
    assert!(rows[0].is_none());
    assert_eq!(rows[1].as_ref().unwrap().author_name, "Ada");
    assert!(rows[2].is_none());
    assert_eq!(rows[3].as_ref().unwrap().author_name, "Bruno");
}

#[test]
fn blame_allineato_tollera_crlf_grezzo_normalizzato_nel_modello() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), "Ada", &["init", "-b", "main"]);
    fs::write(dir.path().join("a.txt"), "uno\r\ndue\r\n").unwrap();
    git(dir.path(), "Ada", &["add", "a.txt"]);
    git(dir.path(), "Ada", &["commit", "-m", "base CRLF"]);
    let repo = GitRepository::discover(dir.path()).unwrap();
    let rows = repo
        .blame_aligned(&dir.path().join("a.txt"), "uno\ndue\n")
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].as_ref().unwrap().author_name, "Ada");
    assert_eq!(rows[1].as_ref().unwrap().author_name, "Ada");
}
