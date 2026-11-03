//! Snapshot stash con cache gix realmente calda: factory/ODB/Git/FS reali.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use std::{fs, process::Command};
fn git(root: &Path, args: &[&str]) -> String {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", null)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Lume Test")
        .env("GIT_AUTHOR_EMAIL", "lume@example.test")
        .env("GIT_COMMITTER_NAME", "Lume Test")
        .env("GIT_COMMITTER_EMAIL", "lume@example.test")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .trim_end_matches(['\r', '\n'])
        .to_owned()
}
fn init() -> (tempfile::TempDir, GitRepository) {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Lume Test"]);
    git(dir.path(), &["config", "user.email", "lume@example.test"]);
    git(dir.path(), &["config", "core.autocrlf", "false"]);
    fs::write(dir.path().join("a.txt"), b"uno\n").unwrap();
    git(dir.path(), &["add", "--", "a.txt"]);
    git(dir.path(), &["commit", "-m", "base"]);
    let repo = GitRepository::discover(dir.path()).unwrap();
    (dir, repo)
}
fn reset_time(path: &Path, time: std::time::SystemTime) {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(time))
        .unwrap();
}
fn warm(repo: &GitRepository) {
    repo.repository.index_or_load_from_head().unwrap();
}
#[test]
fn stash_secondo_parent_usa_stage_esterno_non_cache_mtime() {
    let (dir, repo) = init();
    warm(&repo);
    let path = dir.path().join(".git/index");
    let time = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(dir.path().join("b.txt"), b"staged esterno\n").unwrap();
    git(dir.path(), &["add", "--", "b.txt"]);
    reset_time(&path, time);
    let cached = repo.repository.index_or_load_from_head().unwrap();
    assert!(
        cached
            .entry_by_path_and_stage("b.txt".into(), gix::index::entry::Stage::Unconflicted)
            .is_none(),
        "precondizione: cache nativa è davvero precedente"
    );
    drop(cached);
    let index = fs::read(&path).unwrap();
    repo.stash_push(Some("snapshot fresco")).unwrap();
    assert_eq!(
        git(dir.path(), &["show", "refs/stash^2:b.txt"]),
        "staged esterno"
    );
    assert_eq!(
        git(dir.path(), &["show", "refs/stash:b.txt"]),
        "staged esterno"
    );
    assert_eq!(fs::read(path).unwrap(), index);
    assert_eq!(
        fs::read(dir.path().join("b.txt")).unwrap(),
        b"staged esterno\n"
    );
    assert!(!dir.path().join(".git/index.lock").exists());
    git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
#[test]
fn stash_rifiuta_index_lock_esterno_prima_odb_ref_o_worktree() {
    let (dir, repo) = init();
    fs::write(dir.path().join("a.txt"), b"dirty\n").unwrap();
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    fs::write(dir.path().join(".git/index.lock"), b"owner esterno").unwrap();
    assert!(repo.stash_push(Some("deve rifiutare")).is_err());
    assert!(!dir.path().join(".git/refs/stash").exists());
    assert!(!dir.path().join(".git/logs/refs/stash").exists());
    assert_eq!(
        fs::read(dir.path().join(".git/index.lock")).unwrap(),
        b"owner esterno"
    );
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"dirty\n");
}
#[test]
fn stash_index_corrotto_mtime_cache_non_pubblica_ref() {
    let (dir, repo) = init();
    warm(&repo);
    let path = dir.path().join(".git/index");
    let time = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, b"corrotto\n").unwrap();
    reset_time(&path, time);
    assert!(
        repo.repository.index_or_load_from_head().is_ok(),
        "precondizione: cache resta valida"
    );
    assert!(repo.stash_push(Some("rifiuto snapshot corrotto")).is_err());
    assert!(!dir.path().join(".git/refs/stash").exists());
    assert!(!dir.path().join(".git/logs/refs/stash").exists());
    assert_eq!(fs::read(path).unwrap(), b"corrotto\n");
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn cache_valida_non_nasconde_corruzione_index_in_tutti_i_consumer() {
    for action in [
        "status", "stage", "commit", "amend", "discard", "diff", "stash",
    ] {
        let (dir, repo) = init();
        warm(&repo);
        let path = dir.path().join(".git/index");
        let time = fs::metadata(&path).unwrap().modified().unwrap();
        let head = git(dir.path(), &["rev-parse", "HEAD"]);
        let log = fs::read(dir.path().join(".git/logs/HEAD")).unwrap();
        fs::write(&path, b"index corrotto reale\n").unwrap();
        reset_time(&path, time);
        assert!(
            repo.repository.index_or_load_from_head().is_ok(),
            "cache ancora realmente valida: {action}"
        );
        let result = match action {
            "status" => repo.status().map(|_| ()),
            "stage" => repo.stage(["a.txt"]),
            "commit" | "amend" => repo
                .commit_with("rifiuto reale", action == "amend")
                .map(|_| ()),
            "discard" => repo.discard(["a.txt"]).map(|_| ()),
            "diff" => repo.diff_worktree_file(Path::new("a.txt")).map(|_| ()),
            "stash" => repo.stash_push(Some("rifiuto reale")),
            _ => unreachable!(),
        };
        assert!(
            result.is_err(),
            "nessun consumer deve aggirare corruzione con cache valida: {action}"
        );
        assert_eq!(fs::read(&path).unwrap(), b"index corrotto reale\n");
        assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
        assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
        assert_eq!(fs::read(dir.path().join(".git/logs/HEAD")).unwrap(), log);
        assert!(!dir.path().join(".git/index.lock").exists());
        assert!(!dir.path().join(".git/refs/stash").exists());
    }
}

#[test]
fn stash_drop_ripristina_il_reflog_se_fallisce_la_pubblicazione_della_ref() {
    let (dir, repo) = init();
    fs::write(dir.path().join("a.txt"), b"stash me\n").unwrap();
    repo.stash_push(Some("older")).unwrap();
    fs::write(dir.path().join("a.txt"), b"top stash\n").unwrap();
    repo.stash_push(Some("failure injection")).unwrap();
    let ref_path = dir.path().join(".git/refs/stash");
    let log_path = dir.path().join(".git/logs/refs/stash");
    let ref_before = fs::read(&ref_path).unwrap();
    let log_before = fs::read(&log_path).unwrap();

    let error = repo
        .drop_stash_entry_with(0, None, || {
            Err(GitError::Stash("fault before refs/stash rename".into()))
        })
        .unwrap_err();

    assert!(error.to_string().contains("reflog ripristinato"), "{error}");
    assert_eq!(fs::read(&ref_path).unwrap(), ref_before);
    assert_eq!(fs::read(&log_path).unwrap(), log_before);
    assert_eq!(repo.stash_list().unwrap().len(), 2);
    assert!(!ref_path.with_extension("lock").exists());
    assert!(!log_path.with_extension("lock").exists());
    assert_eq!(git(dir.path(), &["fsck", "--strict", "--no-progress"]), "");
}

#[test]
fn stash_drop_dichiara_residuo_se_fallisce_anche_il_ripristino_del_reflog() {
    let (dir, repo) = init();
    fs::write(dir.path().join("a.txt"), b"older\n").unwrap();
    repo.stash_push(Some("older")).unwrap();
    fs::write(dir.path().join("a.txt"), b"top\n").unwrap();
    repo.stash_push(Some("top")).unwrap();
    let ref_path = dir.path().join(".git/refs/stash");
    let log_path = dir.path().join(".git/logs/refs/stash");
    let lock_path = log_path.with_extension("lock");
    let ref_before = fs::read(&ref_path).unwrap();
    let log_before = fs::read(&log_path).unwrap();

    let error = repo
        .drop_stash_entry_with(0, None, || {
            fs::write(&lock_path, b"external lock").unwrap();
            Err(GitError::Stash("fault before refs/stash rename".into()))
        })
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("reflog pubblicato ma ripristino fallito"),
        "{error}"
    );
    assert_eq!(fs::read(&ref_path).unwrap(), ref_before);
    assert_ne!(fs::read(&log_path).unwrap(), log_before);
    assert_eq!(fs::read(&lock_path).unwrap(), b"external lock");
    fs::remove_file(lock_path).unwrap();
}
#[test]
fn cache_genuina_status_stage_commit_amend_discard_diff_restano_freschi() {
    for action in ["status", "stage", "commit", "amend", "discard", "diff"] {
        let (dir, repo) = init();
        warm(&repo);
        let path = dir.path().join(".git/index");
        let time = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(dir.path().join("a.txt"), b"staged esterno\n").unwrap();
        fs::write(dir.path().join("b.txt"), b"nuovo esterno\n").unwrap();
        git(dir.path(), &["add", "--", "a.txt", "b.txt"]);
        reset_time(&path, time);
        let cached = repo.repository.index_or_load_from_head().unwrap();
        assert!(
            cached
                .entry_by_path_and_stage("b.txt".into(), gix::index::entry::Stage::Unconflicted)
                .is_none()
        );
        drop(cached);
        let index = fs::read(&path).unwrap();
        match action {
            "status" => assert!(
                repo.status()
                    .unwrap()
                    .iter()
                    .any(|s| s.path == Path::new("b.txt") && s.index.is_some())
            ),
            "stage" => {
                fs::write(dir.path().join("c.txt"), b"stage locale\n").unwrap();
                repo.stage(["c.txt"]).unwrap();
                assert_eq!(
                    git(dir.path(), &["diff", "--cached", "--name-only"]),
                    "a.txt\nb.txt\nc.txt"
                );
            }
            "commit" | "amend" => {
                repo.commit_with("include stage reale", action == "amend")
                    .unwrap();
                assert_eq!(git(dir.path(), &["show", "HEAD:b.txt"]), "nuovo esterno");
            }
            "discard" => {
                fs::write(dir.path().join("a.txt"), b"dirty nuovo\n").unwrap();
                repo.discard(["a.txt"]).unwrap();
                assert_eq!(
                    fs::read(dir.path().join("a.txt")).unwrap(),
                    b"staged esterno\n"
                );
            }
            "diff" => {
                fs::write(dir.path().join("a.txt"), b"dirty nuovo\n").unwrap();
                let diff = repo.diff_worktree_file(Path::new("a.txt")).unwrap();
                assert!(
                    diff.hunks
                        .iter()
                        .flat_map(|h| &h.old_lines)
                        .any(|r| r.contains("staged esterno"))
                );
            }
            _ => unreachable!(),
        }
        if action != "stage" {
            assert_eq!(fs::read(&path).unwrap(), index);
        }
        assert!(!dir.path().join(".git/index.lock").exists());
        git(dir.path(), &["fsck", "--strict", "--no-progress"]);
    }
}
