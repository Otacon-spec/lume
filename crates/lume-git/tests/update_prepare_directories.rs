//! Preparazione payload delle transizioni: ODB/lease/core reali, nessun apply.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use lume_git::GitRepository;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};
fn target(root: &Path, files: &[(&str, &[u8])]) -> gix::ObjectId {
    let raw = gix::open(root).unwrap();
    let mut editor = raw.empty_tree().edit().unwrap();
    for (path, bytes) in files {
        let id = raw
            .write_object(gix::objs::BlobRef { data: bytes })
            .unwrap()
            .detach();
        editor
            .upsert(
                gix::bstr::BStr::new(path),
                gix::object::tree::EntryKind::Blob,
                id,
            )
            .unwrap();
    }
    editor.write().unwrap().detach()
}
fn temps(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".lume-save-")
        })
        .collect()
}
fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}
#[test]
fn parent_profondo_assente_payload_nel_parent_esistente_senza_mkdir() {
    let dir = common::init_repo();
    common::commit_file(dir.path(), "dir/old.txt", "old\n", "directory old");
    let tree = target(
        dir.path(),
        &[
            ("a.txt", b"uno\n"),
            ("dir/old.txt", b"old\n"),
            ("dir/new/sub/file", b"nuovo\n"),
        ],
    );
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    let repo = GitRepository::discover(dir.path()).unwrap();
    let plan = repo.prepare_update(tree, cancel()).unwrap();
    assert_eq!(plan.prepared_files(), 1);
    assert_eq!(plan.planned_deletions(), 0);
    assert!(temps(dir.path()).is_empty());
    let temp = temps(&dir.path().join("dir"));
    assert_eq!(temp.len(), 1);
    assert_eq!(fs::read(temp.first().unwrap()).unwrap(), b"nuovo\n");
    assert!(!dir.path().join("dir/new").exists());
    assert_eq!(fs::read(dir.path().join("dir/old.txt")).unwrap(), b"old\n");
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert!(dir.path().join(".git/index.lock").exists());
    plan.abort().unwrap();
    assert!(temps(&dir.path().join("dir")).is_empty());
    assert!(!dir.path().join(".git/index.lock").exists());
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
#[test]
fn file_a_directory_nuovi_attrs_e_payload_protetto_originale_intatto() {
    let dir = common::init_repo();
    let tree = target(
        dir.path(),
        &[
            (".gitattributes", b"a.txt/child text eol=crlf\n"),
            ("a.txt/child", b"figlio\n"),
        ],
    );
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let headlog = fs::read(dir.path().join(".git/logs/HEAD")).unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    let plan = repo.prepare_update(tree, cancel()).unwrap();
    assert_eq!(plan.prepared_files(), 2);
    assert_eq!(plan.planned_deletions(), 1);
    let files = temps(dir.path());
    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|p| fs::read(p).unwrap() == b"figlio\r\n"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let payload = files
            .iter()
            .find(|p| fs::read(p).unwrap() == b"figlio\r\n")
            .unwrap();
        assert_eq!(
            fs::metadata(payload).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert!(!dir.path().join(".gitattributes").exists());
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(
        fs::read(dir.path().join(".git/logs/HEAD")).unwrap(),
        headlog
    );
    assert!(!format!("{plan:?}").contains("child"));
    plan.abort().unwrap();
    assert!(temps(dir.path()).is_empty());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
}
#[test]
fn publish_transizione_file_a_directory_pubblica_payload_index_e_ref() {
    let dir = common::init_repo();
    let tree = target(dir.path(), &[("a.txt/child", b"figlio\n")]);
    let output = std::process::Command::new("git")
        .args(["commit-tree", &tree.to_string(), "-p", "HEAD"])
        .current_dir(dir.path())
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Lume Test")
        .env("GIT_AUTHOR_EMAIL", "lume@example.test")
        .env("GIT_COMMITTER_NAME", "Lume Test")
        .env("GIT_COMMITTER_EMAIL", "lume@example.test")
        .output()
        .unwrap();
    assert!(output.status.success());
    let destination: gix::ObjectId = String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    let prepared = repo.prepare_update_to(tree, destination, cancel()).unwrap();
    let receipt = prepared.publish(cancel()).unwrap();
    assert_eq!(receipt.updated_files(), 1);
    assert_eq!(receipt.removed_files(), 1);
    assert_eq!(receipt.created_directories(), 1);
    assert!(dir.path().join("a.txt").is_dir());
    assert_eq!(
        fs::read(dir.path().join("a.txt/child")).unwrap(),
        b"figlio\n"
    );
    assert_eq!(common::git(dir.path(), &["write-tree"]), tree.to_string());
    assert_eq!(
        common::git(dir.path(), &["rev-parse", "HEAD"]),
        destination.to_string()
    );
}
#[test]
fn directory_a_file_payload_fuori_subtree_senza_rimozioni() {
    let dir = common::init_repo();
    common::commit_file(dir.path(), "dir/a", "a\n", "a");
    common::commit_file(dir.path(), "dir/sub/b", "b\n", "b");
    let tree = target(dir.path(), &[("a.txt", b"uno\n"), ("dir", b"collasso\n")]);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    let plan = repo.prepare_update(tree, cancel()).unwrap();
    assert_eq!(plan.prepared_files(), 1);
    assert_eq!(plan.planned_deletions(), 2);
    assert_eq!(
        fs::read(temps(dir.path()).first().unwrap()).unwrap(),
        b"collasso\n"
    );
    assert!(temps(&dir.path().join("dir")).is_empty());
    assert_eq!(fs::read(dir.path().join("dir/a")).unwrap(), b"a\n");
    assert_eq!(fs::read(dir.path().join("dir/sub/b")).unwrap(), b"b\n");
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    drop(plan);
    assert!(temps(dir.path()).is_empty());
    assert!(dir.path().join("dir/sub").is_dir());
}
#[test]
fn publish_transizione_directory_a_file_rimuove_solo_leaf_tracciate() {
    let dir = common::init_repo();
    common::commit_file(dir.path(), "dir/a", "a\n", "a");
    common::commit_file(dir.path(), "dir/sub/b", "b\n", "b");
    let tree = target(dir.path(), &[("a.txt", b"uno\n"), ("dir", b"collasso\n")]);
    let output = std::process::Command::new("git")
        .args(["commit-tree", &tree.to_string(), "-p", "HEAD"])
        .current_dir(dir.path())
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Lume Test")
        .env("GIT_AUTHOR_EMAIL", "lume@example.test")
        .env("GIT_COMMITTER_NAME", "Lume Test")
        .env("GIT_COMMITTER_EMAIL", "lume@example.test")
        .output()
        .unwrap();
    assert!(output.status.success());
    let destination: gix::ObjectId = String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    let prepared = repo.prepare_update_to(tree, destination, cancel()).unwrap();
    let receipt = prepared.publish(cancel()).unwrap();
    assert_eq!(receipt.updated_files(), 1);
    assert_eq!(receipt.removed_files(), 2);
    assert_eq!(receipt.removed_directories(), 2);
    assert_eq!(fs::read(dir.path().join("dir")).unwrap(), b"collasso\n");
    assert_eq!(common::git(dir.path(), &["write-tree"]), tree.to_string());
    assert_eq!(
        common::git(dir.path(), &["rev-parse", "HEAD"]),
        destination.to_string()
    );
}
#[test]
fn collasso_file_o_cartella_vuota_estranei_rifiutato_prima_payload() {
    for foreign in ["file", "emptydir"] {
        let dir = common::init_repo();
        common::commit_file(dir.path(), "dir/tracked", "tracked\n", "tracked");
        if foreign == "file" {
            fs::write(dir.path().join("dir/foreign"), b"locale\n").unwrap();
        } else {
            fs::create_dir(dir.path().join("dir/foreign")).unwrap();
        }
        let tree = target(dir.path(), &[("a.txt", b"uno\n"), ("dir", b"nuovo\n")]);
        assert!(
            GitRepository::discover(dir.path())
                .unwrap()
                .prepare_update(tree, cancel())
                .is_err()
        );
        assert!(temps(dir.path()).is_empty());
        assert!(temps(&dir.path().join("dir")).is_empty());
        assert!(dir.path().join("dir/foreign").exists());
        assert_eq!(
            fs::read(dir.path().join("dir/tracked")).unwrap(),
            b"tracked\n"
        );
        assert!(!dir.path().join(".git/index.lock").exists());
    }
}
#[cfg(unix)]
#[test]
fn abort_errore_primo_payload_raccoglie_writer_atomic_suffix() {
    let dir = common::init_repo();
    let tree = target(
        dir.path(),
        &[
            ("a.txt/child", b"primo payload\n"),
            ("b.txt", b"secondo atomic\n"),
        ],
    );
    let repo = GitRepository::discover(dir.path()).unwrap();
    let plan = repo.prepare_update(tree, cancel()).unwrap();
    assert_eq!(plan.prepared_files(), 2);
    let all = temps(dir.path());
    let first = all
        .iter()
        .find(|p| fs::read(p).unwrap() == b"primo payload\n")
        .unwrap();
    fs::remove_file(first).unwrap();
    fs::create_dir(first).unwrap();
    assert!(matches!(
        plan.abort(),
        Err(lume_git::GitError::UpdateCleanup {
            attempted: 2,
            failed: 1
        })
    ));
    assert!(first.is_dir());
    assert_eq!(temps(dir.path()).len(), 1);
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert!(!dir.path().join("b.txt").exists());
    for lock in [
        ".git/index.lock",
        ".git/HEAD.lock",
        ".git/refs/heads/main.lock",
    ] {
        assert!(!dir.path().join(lock).exists());
    }
    fs::remove_dir(first).unwrap();
}
#[test]
fn errore_nonblob_suffix_raccoglie_payload_staccato_e_lease() {
    let dir = common::init_repo();
    let raw = gix::open(dir.path()).unwrap();
    let child = raw
        .write_object(gix::objs::BlobRef { data: b"figlio\n" })
        .unwrap()
        .detach();
    let folder = raw
        .write_object(&gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::object::tree::EntryKind::Blob.into(),
                filename: "child".into(),
                oid: child,
            }],
        })
        .unwrap()
        .detach();
    let malformed = raw
        .write_object(&gix::objs::Tree { entries: vec![] })
        .unwrap()
        .detach();
    let tree = raw
        .write_object(&gix::objs::Tree {
            entries: vec![
                gix::objs::tree::Entry {
                    mode: gix::object::tree::EntryKind::Tree.into(),
                    filename: "a.txt".into(),
                    oid: folder,
                },
                gix::objs::tree::Entry {
                    mode: gix::object::tree::EntryKind::Blob.into(),
                    filename: "z".into(),
                    oid: malformed,
                },
            ],
        })
        .unwrap()
        .detach();
    let result = GitRepository::discover(dir.path())
        .unwrap()
        .prepare_update(tree, cancel())
        .map(|plan| plan.prepared_files());
    assert!(result.is_err());
    assert!(temps(dir.path()).is_empty());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert!(!dir.path().join("z").exists());
    assert!(!dir.path().join(".git/index.lock").exists());
    assert!(!dir.path().join(".git/HEAD.lock").exists());
}
