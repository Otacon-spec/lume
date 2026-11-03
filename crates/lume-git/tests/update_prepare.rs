//! Preparazione core reale, lease native e cleanup in TempDir, non apply.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use lume_git::{GitError, GitRepository};
use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
fn no() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}
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
fn temps(root: &Path) -> Vec<std::path::PathBuf> {
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
#[test]
fn prepare_update_to_blocca_ref_di_destinazione_senza_pubblicarla() {
    let dir = common::init_repo();
    let tree = target(dir.path(), &[("a.txt", b"nuovo\n")]);
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
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    let log = fs::read(dir.path().join(".git/logs/HEAD")).unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    let prepared = repo.prepare_update_to(tree, destination, no()).unwrap();
    assert!(dir.path().join(".git/HEAD.lock").exists());
    assert!(dir.path().join(".git/refs/heads/main.lock").exists());
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(fs::read(dir.path().join(".git/logs/HEAD")).unwrap(), log);
    drop(prepared);
    assert!(!dir.path().join(".git/HEAD.lock").exists());
    assert!(!dir.path().join(".git/refs/heads/main.lock").exists());
}
#[test]
fn publish_applica_file_index_ref_e_reflog_nel_repository_reale() {
    let dir = common::init_repo();
    common::commit_file(dir.path(), "b.txt", "elimina\n", "aggiunge b");
    let tree = target(
        dir.path(),
        &[("a.txt", b"aggiornato\n"), ("c.txt", b"nuovo\n")],
    );
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
    let plan = repo.prepare_update_to(tree, destination, no()).unwrap();
    let receipt = plan.publish(no()).unwrap();
    assert_eq!(receipt.updated_files(), 2);
    assert_eq!(receipt.removed_files(), 1);
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"aggiornato\n");
    assert_eq!(fs::read(dir.path().join("c.txt")).unwrap(), b"nuovo\n");
    assert!(!dir.path().join("b.txt").exists());
    assert_eq!(common::git(dir.path(), &["write-tree"]), tree.to_string());
    assert_eq!(
        common::git(dir.path(), &["rev-parse", "HEAD"]),
        destination.to_string()
    );
    let log = fs::read_to_string(dir.path().join(".git/logs/HEAD")).unwrap();
    assert!(
        log.lines()
            .last()
            .unwrap()
            .ends_with("\tpull: aggiornamento")
    );
}
#[test]
fn publish_rivalida_dirty_dopo_prepare_e_non_pubblica_index_o_ref() {
    let dir = common::init_repo();
    let tree = target(dir.path(), &[("a.txt", b"remoto\n")]);
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
    let plan = repo.prepare_update_to(tree, destination, no()).unwrap();
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    fs::write(dir.path().join("a.txt"), b"edizione locale\n").unwrap();
    let error = plan.publish(no()).unwrap_err();
    assert_eq!(error.phase, lume_git::UpdatePublishPhase::Revalidate);
    assert_eq!(error.worktree, lume_git::UpdatePublishState::NotAttempted);
    assert_eq!(error.index, lume_git::UpdatePublishState::NotAttempted);
    assert_eq!(error.reference, lume_git::UpdatePublishState::NotAttempted);
    assert_eq!(error.cleanup_attempted, 1);
    assert_eq!(error.cleanup_failed, 0);
    assert_eq!(
        fs::read(dir.path().join("a.txt")).unwrap(),
        b"edizione locale\n"
    );
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert!(!dir.path().join(".git/index.lock").exists());
    assert!(!dir.path().join(".git/HEAD.lock").exists());
    assert!(temps(dir.path()).is_empty());
}
#[test]
fn piano_prepare_senza_commit_destinazione_non_puo_pubblicare() {
    let dir = common::init_repo();
    let tree = target(dir.path(), &[("a.txt", b"remoto\n")]);
    let repo = GitRepository::discover(dir.path()).unwrap();
    let plan = repo.prepare_update(tree, no()).unwrap();
    let error = plan.publish(no()).unwrap_err();
    assert_eq!(error.phase, lume_git::UpdatePublishPhase::Revalidate);
    assert_eq!(error.worktree, lume_git::UpdatePublishState::NotAttempted);
    assert_eq!(error.cleanup_attempted, 1);
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert_eq!(
        common::git(dir.path(), &["write-tree"]),
        common::git(dir.path(), &["rev-parse", "HEAD^{tree}"])
    );
    assert!(!dir.path().join(".git/index.lock").exists());
    assert!(!dir.path().join(".git/HEAD.lock").exists());
}
#[test]
fn prepare_rifiuta_commit_destinazione_non_discendente() {
    let dir = common::init_repo();
    let tree = target(dir.path(), &[("a.txt", b"altro\n")]);
    let output = std::process::Command::new("git")
        .args(["commit-tree", &tree.to_string()])
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
    assert!(
        GitRepository::discover(dir.path())
            .unwrap()
            .prepare_update_to(tree, destination, no())
            .is_err()
    );
    assert!(temps(dir.path()).is_empty());
    assert!(!dir.path().join(".git/index.lock").exists());
    assert!(!dir.path().join(".git/HEAD.lock").exists());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
}
#[test]
fn prepared_update_tiene_temp_lease_e_non_pubblica_nulla_fino_drop() {
    let dir = common::init_repo();
    common::commit_file(dir.path(), "b.txt", "b\n", "b");
    let tree = target(dir.path(), &[("a.txt", b"nuovo\n"), ("c.txt", b"c\n")]);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    let log = fs::read(dir.path().join(".git/logs/HEAD")).unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    let prepared = repo.prepare_update(tree, no()).unwrap();
    assert_eq!(prepared.prepared_files(), 2);
    assert_eq!(prepared.planned_deletions(), 1);
    assert_eq!(temps(dir.path()).len(), 2);
    assert!(dir.path().join(".git/index.lock").exists());
    assert!(dir.path().join(".git/HEAD.lock").exists());
    assert!(dir.path().join(".git/refs/heads/main.lock").exists());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert_eq!(fs::read(dir.path().join("b.txt")).unwrap(), b"b\n");
    assert!(!dir.path().join("c.txt").exists());
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(fs::read(dir.path().join(".git/logs/HEAD")).unwrap(), log);
    assert!(!format!("{prepared:?}").contains("a.txt"));
    drop(prepared);
    assert!(temps(dir.path()).is_empty());
    for lock in [
        ".git/index.lock",
        ".git/HEAD.lock",
        ".git/refs/heads/main.lock",
    ] {
        assert!(!dir.path().join(lock).exists());
    }
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
#[test]
fn nuovi_attributi_in_temp_non_preferiscono_worktree_vecchio() {
    let dir = common::init_repo();
    common::commit_file(dir.path(), ".gitattributes", "a.txt -text\n", "attrs old");
    let tree = target(
        dir.path(),
        &[
            (".gitattributes", b"a.txt text eol=crlf\n"),
            ("a.txt", b"nuovo\n"),
        ],
    );
    let repo = GitRepository::discover(dir.path()).unwrap();
    let prepared = repo.prepare_update(tree, no()).unwrap();
    let payloads = temps(dir.path())
        .into_iter()
        .map(|p| fs::read(p).unwrap())
        .collect::<Vec<_>>();
    assert!(payloads.iter().any(|p| p == b"nuovo\r\n"));
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert_eq!(
        fs::read(dir.path().join(".gitattributes")).unwrap(),
        b"a.txt -text\n"
    );
    drop(prepared);
    assert!(temps(dir.path()).is_empty());
}
#[test]
fn secondo_payload_non_blob_libera_prime_prepare_senza_publish() {
    let dir = common::init_repo();
    let raw = gix::open(dir.path()).unwrap();
    let blob = raw
        .write_object(gix::objs::BlobRef { data: b"nuovo\n" })
        .unwrap()
        .detach();
    let notblob = raw
        .write_object(&gix::objs::Tree { entries: vec![] })
        .unwrap()
        .detach();
    let tree = raw
        .write_object(&gix::objs::Tree {
            entries: vec![
                gix::objs::tree::Entry {
                    mode: gix::object::tree::EntryKind::Blob.into(),
                    filename: "a.txt".into(),
                    oid: blob,
                },
                gix::objs::tree::Entry {
                    mode: gix::object::tree::EntryKind::Blob.into(),
                    filename: "b.txt".into(),
                    oid: notblob,
                },
            ],
        })
        .unwrap()
        .detach();
    let result = GitRepository::discover(dir.path())
        .unwrap()
        .prepare_update(tree, no())
        .map(|plan| plan.prepared_files());
    assert!(result.is_err());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert!(!dir.path().join("b.txt").exists());
    assert!(temps(dir.path()).is_empty());
    assert!(!dir.path().join(".git/index.lock").exists());
    assert!(!dir.path().join(".git/HEAD.lock").exists());
}
#[test]
fn foreign_index_head_lock_e_dirty_stage_rifiutati_senza_temp() {
    for scenario in ["index", "HEAD", "dirty", "stage"] {
        let dir = common::init_repo();
        let tree = target(dir.path(), &[("a.txt", b"nuovo\n")]);
        let foreign = dir.path().join(format!(".git/{scenario}.lock"));
        match scenario {
            "index" | "HEAD" => fs::write(&foreign, b"foreign\n").unwrap(),
            "dirty" => fs::write(dir.path().join("a.txt"), b"locale\n").unwrap(),
            _ => {
                fs::write(dir.path().join("altro"), b"stage\n").unwrap();
                common::git(dir.path(), &["add", "--", "altro"]);
            }
        }
        let index = fs::read(dir.path().join(".git/index")).unwrap();
        assert!(
            GitRepository::discover(dir.path())
                .unwrap()
                .prepare_update(tree, no())
                .is_err()
        );
        assert!(temps(dir.path()).is_empty());
        assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
        if scenario == "index" || scenario == "HEAD" {
            assert_eq!(fs::read(foreign).unwrap(), b"foreign\n");
        }
        if scenario != "index" {
            assert!(!dir.path().join(".git/index.lock").exists());
        }
    }
}
#[test]
fn parent_mancante_e_transizioni_payload_reali_senza_creare_directory() {
    for path in ["nuova/child", "a.txt/child"] {
        let dir = common::init_repo();
        let tree = target(dir.path(), &[(path, b"nuovo\n")]);
        let repo = GitRepository::discover(dir.path()).unwrap();
        let plan = repo.prepare_update(tree, no()).unwrap();
        assert_eq!(plan.prepared_files(), 1);
        assert_eq!(temps(dir.path()).len(), 1);
        assert_eq!(
            fs::read(temps(dir.path()).first().unwrap()).unwrap(),
            b"nuovo\n"
        );
        assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
        assert!(!dir.path().join("nuova").exists());
        plan.abort().unwrap();
        assert!(temps(dir.path()).is_empty());
        assert!(!dir.path().join(".git/index.lock").exists());
    }
}
#[test]
fn readonly_suffisso_preflight_non_crea_temporaneo_primo_payload() {
    let dir = common::init_repo();
    common::commit_file(dir.path(), "b.txt", "b\n", "b");
    let tree = target(
        dir.path(),
        &[("a.txt", b"nuovo a\n"), ("b.txt", b"nuovo b\n")],
    );
    let path = dir.path().join("b.txt");
    let original = fs::metadata(&path).unwrap().permissions();
    let mut ro = original.clone();
    ro.set_readonly(true);
    fs::set_permissions(&path, ro).unwrap();
    let result = GitRepository::discover(dir.path())
        .unwrap()
        .prepare_update(tree, no())
        .map(|plan| plan.prepared_files());
    fs::set_permissions(path, original).unwrap();
    assert!(result.is_err());
    assert!(temps(dir.path()).is_empty());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
    assert_eq!(fs::read(dir.path().join("b.txt")).unwrap(), b"b\n");
}
#[test]
fn drop_non_annulla_edits_o_creation_successivi_del_noncooperatore() {
    let dir = common::init_repo();
    let tree = target(dir.path(), &[("a.txt", b"nuovo\n"), ("c.txt", b"c\n")]);
    let repo = GitRepository::discover(dir.path()).unwrap();
    let prepared = repo.prepare_update(tree, no()).unwrap();
    fs::write(dir.path().join("a.txt"), b"edits tardivi\n").unwrap();
    fs::write(dir.path().join("c.txt"), b"file esterno tardivo\n").unwrap();
    drop(prepared);
    assert!(temps(dir.path()).is_empty());
    assert_eq!(
        fs::read(dir.path().join("a.txt")).unwrap(),
        b"edits tardivi\n"
    );
    assert_eq!(
        fs::read(dir.path().join("c.txt")).unwrap(),
        b"file esterno tardivo\n"
    );
    assert!(!dir.path().join(".git/index.lock").exists());
    assert!(!dir.path().join(".git/HEAD.lock").exists());
}
#[test]
fn precancel_non_avvia_prepare_o_lease() {
    let dir = common::init_repo();
    let tree = target(dir.path(), &[("a.txt", b"nuovo\n")]);
    assert!(matches!(
        GitRepository::discover(dir.path())
            .unwrap()
            .prepare_update(tree, Arc::new(AtomicBool::new(true))),
        Err(GitError::Cancelled)
    ));
    assert!(temps(dir.path()).is_empty());
    assert!(!dir.path().join(".git/index.lock").exists());
}
