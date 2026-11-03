//! Commit/status con index esterno a mtime invariato e lock nativo conteso. Git/FS reali,
//! soltanto repository TempDir di proprietà della fixture.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use lume_git::{GitError, GitRepository};
use std::{fs, path::Path, time::SystemTime};
fn reset_time(path: &Path, time: SystemTime) {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(time))
        .unwrap();
}
fn warm(repo: &GitRepository) {
    // Stash45 ora legge fresco: questa chiamata esercita una fotografia
    // precedente, NON riscalda la cache gix. Le precondizioni cache native
    // vere sono mantenute in tests/stash_index/mod.rs (accesso cfg(test) alla factory
    // interna, cache effettivamente verificata dopo CLI+mtime invariato).
    repo.stash_push(Some("fixture snapshot commit/status"))
        .unwrap();
}
fn staged() -> (tempfile::TempDir, GitRepository) {
    let dir = common::init_repo();
    fs::write(dir.path().join("a.txt"), b"due\n").unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    repo.stage(["a.txt"]).unwrap();
    (dir, repo)
}
#[test]
fn commit_e_amend_rifiutati_con_index_lock_esterno_non_pubblicano_ref_o_log() {
    for amend in [false, true] {
        let (dir, repo) = staged();
        let index = fs::read(dir.path().join(".git/index")).unwrap();
        let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
        let log = fs::read(dir.path().join(".git/logs/HEAD")).unwrap();
        fs::write(dir.path().join(".git/index.lock"), b"proprieta esterna\n").unwrap();
        assert!(
            repo.commit_with("deve rifiutare", amend).is_err(),
            "anche commit/amend devono possedere index.lock: amend={amend}"
        );
        assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
        assert_eq!(fs::read(dir.path().join(".git/logs/HEAD")).unwrap(), log);
        assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
        assert_eq!(
            fs::read(dir.path().join(".git/index.lock")).unwrap(),
            b"proprieta esterna\n"
        );
    }
}
#[test]
fn status_vede_stage_esterno_con_mtime_uguale_senza_modificare_index() {
    let (dir, repo) = staged();
    warm(&repo);
    fs::write(dir.path().join("b.txt"), b"concorrente\n").unwrap();
    let path = dir.path().join(".git/index");
    let time = fs::metadata(&path).unwrap().modified().unwrap();
    common::git(dir.path(), &["add", "--", "b.txt"]);
    reset_time(&path, time);
    let actual = fs::read(&path).unwrap();
    let status = repo.status().unwrap();
    assert!(
        status
            .iter()
            .any(|f| f.path == Path::new("b.txt") && f.index.is_some()),
        "cache mtime non è snapshot dello stage corrente: {status:?}"
    );
    assert_eq!(fs::read(&path).unwrap(), actual);
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn commit_non_omette_stage_esterno_con_stesso_mtime_dopo_snapshot_precedente() {
    let (dir, repo) = staged();
    warm(&repo);
    fs::write(dir.path().join("b.txt"), b"concorrente\n").unwrap();
    let path = dir.path().join(".git/index");
    let time = fs::metadata(&path).unwrap().modified().unwrap();
    common::git(dir.path(), &["add", "--", "b.txt"]);
    reset_time(&path, time);
    let index = fs::read(&path).unwrap();
    repo.commit("include lo stage reale").unwrap();
    assert_eq!(
        common::git(dir.path(), &["ls-tree", "--name-only", "HEAD"]),
        "a.txt\nb.txt",
        "albero del commit non può usare la cache superata"
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        index,
        "commit non riscrive index per nascondere uno stage perso"
    );
    assert!(common::git(dir.path(), &["status", "--porcelain"]).is_empty());
    assert!(!dir.path().join(".git/index.lock").exists());
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
#[test]
fn status_non_nasconde_index_corrotto_con_timestamp_dello_snapshot() {
    let (dir, repo) = staged();
    warm(&repo);
    let path = dir.path().join(".git/index");
    let time = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, b"corrotto\n").unwrap();
    reset_time(&path, time);
    assert!(
        repo.status().is_err(),
        "status deve aprire il file reale, non la precedente cache valida"
    );
    assert_eq!(fs::read(&path).unwrap(), b"corrotto\n");
}
#[test]
fn commit_corrotto_dopo_snapshot_non_pubblica_ref_e_libera_lease_propria() {
    let (dir, repo) = staged();
    warm(&repo);
    let path = dir.path().join(".git/index");
    let time = fs::metadata(&path).unwrap().modified().unwrap();
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    let log = fs::read(dir.path().join(".git/logs/HEAD")).unwrap();
    fs::write(&path, b"corrotto\n").unwrap();
    reset_time(&path, time);
    assert!(
        repo.commit("rifiutato").is_err(),
        "index reale corrotto non si aggira con la cache"
    );
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(fs::read(dir.path().join(".git/logs/HEAD")).unwrap(), log);
    assert_eq!(fs::read(&path).unwrap(), b"corrotto\n");
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn errore_ref_preserva_stage_libera_index_lease_e_permette_retry_reale() {
    let (dir, repo) = staged();
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    let log = fs::read(dir.path().join(".git/logs/HEAD")).unwrap();
    fs::write(dir.path().join(".git/HEAD.lock"), b"HEAD esterna\n").unwrap();
    assert!(repo.commit("prima rifiutata").is_err());
    assert!(!dir.path().join(".git/index.lock").exists());
    assert_eq!(
        fs::read(dir.path().join(".git/HEAD.lock")).unwrap(),
        b"HEAD esterna\n"
    );
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(fs::read(dir.path().join(".git/logs/HEAD")).unwrap(), log);
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    fs::remove_file(dir.path().join(".git/HEAD.lock")).unwrap();
    repo.commit("retry esplicito").unwrap();
    assert!(!dir.path().join(".git/index.lock").exists());
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(common::git(dir.path(), &["show", "HEAD:a.txt"]), "due");
    assert!(common::git(dir.path(), &["status", "--porcelain"]).is_empty());
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
#[test]
fn index_assente_status_conserva_empty_commit_non_inventa_nuovo_snapshot_staged() {
    let (dir, repo) = staged();
    warm(&repo);
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    fs::remove_file(dir.path().join(".git/index")).unwrap();
    let status = repo.status().unwrap();
    assert!(status.iter().any(|f| f.path==Path::new("a.txt") && f.index==Some(lume_git::ChangeKind::Removed)), "fallback status resta Empty, non Head/cache staged");
    assert!(!dir.path().join(".git/index").exists());
    // Commit conserva il proprio fallback Head: NothingStaged è valutato
    // sulla medesima versione, non dall'Empty diverso usato da status.
    assert!(matches!(
        repo.commit("nessuna versione staged su disco"),
        Err(GitError::NothingStaged)
    ));
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert!(!dir.path().join(".git/index").exists());
    assert!(!dir.path().join(".git/index.lock").exists());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"due\n");
}
#[test]
fn commit_linked_worktree_rispetta_lease_locale_e_preserva_index_main() {
    let dir = common::init_repo();
    let holder = tempfile::tempdir().unwrap();
    let linked = holder.path().join("linked");
    common::git(
        dir.path(),
        &["worktree", "add", "-b", "linked", linked.to_str().unwrap()],
    );
    fs::write(linked.join("a.txt"), b"linked\n").unwrap();
    let repo = GitRepository::discover(&linked).unwrap();
    repo.stage(["a.txt"]).unwrap();
    let index_path = gix::discover(&linked).unwrap().index_path();
    let lock = index_path.with_file_name("index.lock");
    let index = fs::read(&index_path).unwrap();
    let main_index = fs::read(dir.path().join(".git/index")).unwrap();
    let main_head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    fs::write(&lock, b"linked esterno\n").unwrap();
    assert!(repo.commit("rifiutato linked").is_err());
    assert_eq!(fs::read(&lock).unwrap(), b"linked esterno\n");
    fs::remove_file(&lock).unwrap();
    repo.commit("solo linked").unwrap();
    assert_eq!(fs::read(&index_path).unwrap(), index);
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), main_index);
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), main_head);
    assert_eq!(common::git(&linked, &["show", "HEAD:a.txt"]), "linked");
    assert!(!lock.exists());
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}

#[test]
fn solo_intent_to_add_non_crea_commit_del_file_vuoto() {
    let dir = common::init_repo();
    fs::write(dir.path().join("nuovo.sql"), b"select 1;\n").unwrap();
    common::git(dir.path(), &["add", "-N", "--", "nuovo.sql"]);
    let repo = GitRepository::discover(dir.path()).unwrap();
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    assert!(
        matches!(repo.commit("non staged"), Err(GitError::NothingStaged)),
        "intent-to-add non è payload approvato nello stage"
    );
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(
        fs::read(dir.path().join("nuovo.sql")).unwrap(),
        b"select 1;\n"
    );
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn commit_con_stage_reale_conserva_intent_to_add_fuori_dal_commit() {
    let (dir, repo) = staged();
    fs::write(dir.path().join("nuovo.sql"), b"select 1;\n").unwrap();
    common::git(dir.path(), &["add", "-N", "--", "nuovo.sql"]);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    repo.commit("solo staged").unwrap();
    assert_eq!(
        common::git(dir.path(), &["ls-tree", "--name-only", "HEAD"]),
        "a.txt"
    );
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(
        fs::read(dir.path().join("nuovo.sql")).unwrap(),
        b"select 1;\n"
    );
    assert!(common::git(dir.path(), &["ls-files", "--debug", "nuovo.sql"]).contains("20004000"));
    assert!(!dir.path().join(".git/index.lock").exists());
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
