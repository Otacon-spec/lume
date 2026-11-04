#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use lume_git::{GitError, GitRepository};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, SystemTime},
};
fn check(repo: &GitRepository, paths: &[&str]) -> Result<(), GitError> {
    repo.check_update_clean(
        &paths.iter().map(PathBuf::from).collect::<Vec<_>>(),
        Arc::new(AtomicBool::new(false)),
    )
}
#[test]
fn dirty_toccato_rifiutato_non_toccato_conservato() {
    let dir = common::init_repo();
    let repo = GitRepository::discover(dir.path()).unwrap();
    fs::write(dir.path().join("a.txt"), b"locale non salvato\n").unwrap();
    assert!(matches!(
        check(&repo, &["a.txt"]),
        Err(GitError::PullDirty(_))
    ));
    assert!(check(&repo, &["altro.sql"]).is_ok());
    assert_eq!(
        fs::read(dir.path().join("a.txt")).unwrap(),
        b"locale non salvato\n"
    );
}
#[test]
fn stage_ovunque_rifiutato_e_index_intatto() {
    let dir = common::init_repo();
    fs::write(dir.path().join("z.sql"), b"select 1;\n").unwrap();
    common::git(dir.path(), &["add", "--", "z.sql"]);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert!(matches!(
        check(&repo, &["altro.sql"]),
        Err(GitError::PullDirty(_))
    ));
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
}
#[test]
fn hint_index_non_autorizzano_perdita_di_dirty_reale() {
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        let dir = common::init_repo();
        common::git(dir.path(), &["update-index", flag, "--", "a.txt"]);
        fs::write(dir.path().join("a.txt"), b"locale nascosto\n").unwrap();
        let index = fs::read(dir.path().join(".git/index")).unwrap();
        let repo = GitRepository::discover(dir.path()).unwrap();
        assert!(
            matches!(check(&repo, &["a.txt"]), Err(GitError::PullDirty(_))),
            "hint {flag} non è assenza modifiche"
        );
        assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    }
}
#[test]
fn uguali_stat_e_size_non_sono_uguale_contenuto() {
    let dir = common::init_repo();
    common::git(dir.path(), &["config", "core.trustctime", "false"]);
    common::git(dir.path(), &["config", "core.checkStat", "minimal"]);
    let path = dir.path().join("a.txt");
    let old = fs::read(&path).unwrap();
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(time))
        .unwrap();
    common::git(dir.path(), &["update-index", "--refresh"]);
    let mut changed = old.clone();
    *changed.first_mut().unwrap() ^= 1;
    fs::write(&path, &changed).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(time))
        .unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert!(
        matches!(check(&repo, &["a.txt"]), Err(GitError::PullDirty(_))),
        "stat non certifica byte attuali"
    );
    assert_eq!(fs::read(&path).unwrap(), changed);
}
#[test]
fn ignorato_toccato_non_diventa_permesso_di_sovrascrivere() {
    let dir = common::init_repo();
    common::commit_file(dir.path(), ".gitignore", "privato.sql\n", "ignore");
    fs::write(dir.path().join("privato.sql"), b"lavoro ignorato\n").unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert!(matches!(
        check(&repo, &["privato.sql"]),
        Err(GitError::PullDirty(_))
    ));
    assert!(check(&repo, &["nuovo.sql"]).is_ok());
    assert_eq!(
        fs::read(dir.path().join("privato.sql")).unwrap(),
        b"lavoro ignorato\n"
    );
}
#[test]
fn prefissi_sono_componenti_non_sottostringhe() {
    let dir = common::init_repo();
    fs::create_dir(dir.path().join("dir")).unwrap();
    fs::write(dir.path().join("dir/locale.sql"), b"mio\n").unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert!(matches!(
        check(&repo, &["dir"]),
        Err(GitError::PullDirty(_))
    ));
    assert!(check(&repo, &["directory"]).is_ok());
    fs::write(dir.path().join("a.txt"), b"dirty\n").unwrap();
    assert!(matches!(
        check(&repo, &["a.txt/figlio"]),
        Err(GitError::PullDirty(_))
    ));
}
#[test]
fn lock_index_e_head_contesi_non_sottratti_e_cleanup_reale() {
    let dir = common::init_repo();
    let repo = GitRepository::discover(dir.path()).unwrap();
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    let log = fs::read(dir.path().join(".git/logs/HEAD")).unwrap();
    for name in ["index", "HEAD"] {
        let lock = dir.path().join(format!(".git/{name}.lock"));
        fs::write(&lock, b"esterno\n").unwrap();
        assert!(check(&repo, &["a.txt"]).is_err());
        assert_eq!(fs::read(&lock).unwrap(), b"esterno\n");
        fs::remove_file(&lock).unwrap();
        assert!(!dir.path().join(".git/index.lock").exists());
        assert!(!dir.path().join(".git/HEAD.lock").exists());
        assert!(!dir.path().join(".git/refs/heads/main.lock").exists());
        assert!(check(&repo, &["a.txt"]).is_ok());
    }
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(fs::read(dir.path().join(".git/logs/HEAD")).unwrap(), log);
    assert!(!dir.path().join(".git/index.lock").exists());
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
#[test]
fn intent_to_add_non_toccato_rifiutato_per_non_perderne_flag() {
    let dir = common::init_repo();
    fs::write(dir.path().join("futuro.sql"), b"select 1;\n").unwrap();
    common::git(dir.path(), &["add", "-N", "--", "futuro.sql"]);
    let repo = GitRepository::discover(dir.path()).unwrap();
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    assert!(matches!(
        check(&repo, &["altro.sql"]),
        Err(GitError::PullDirty(_))
    ));
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
fn directory_ignorata_occupata_blocca_sostituzione_del_parent() {
    let dir = common::init_repo();
    common::commit_file(dir.path(), ".gitignore", "privati/\n", "ignore");
    fs::create_dir(dir.path().join("privati")).unwrap();
    fs::write(dir.path().join("privati/mio.sql"), b"da conservare\n").unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert!(matches!(
        check(&repo, &["privati"]),
        Err(GitError::PullDirty(_))
    ));
    assert!(check(&repo, &["altro.sql"]).is_ok());
    assert_eq!(
        fs::read(dir.path().join("privati/mio.sql")).unwrap(),
        b"da conservare\n"
    );
}
// APFS (macOS) rifiuta i nomi non UTF-8: il caso esiste solo sugli altri Unix.
#[cfg(all(unix, not(target_vendor = "apple")))]
#[test]
fn percorsi_raw_distinti_e_backslash_literalmente_toccati() {
    use std::os::unix::ffi::OsStringExt;
    let dir = common::init_repo();
    let raw = PathBuf::from(std::ffi::OsString::from_vec(b"nome\xff.sql".to_vec()));
    let other = PathBuf::from(std::ffi::OsString::from_vec(b"nome\xfe.sql".to_vec()));
    fs::write(dir.path().join(&raw), b"locale\n").unwrap();
    fs::write(dir.path().join("a\\b.sql"), b"backslash\n").unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    let no = || Arc::new(AtomicBool::new(false));
    assert!(matches!(
        repo.check_update_clean(std::slice::from_ref(&raw), no()),
        Err(GitError::PullDirty(_))
    ));
    assert!(
        repo.check_update_clean(std::slice::from_ref(&other), no())
            .is_ok()
    );
    assert!(matches!(
        check(&repo, &["a\\b.sql"]),
        Err(GitError::PullDirty(_))
    ));
    assert!(check(&repo, &["a/b.sql"]).is_ok());
}

#[test]
fn cancellato_non_diventa_ok_e_percorsi_ostili_rifiutati() {
    let dir = common::init_repo();
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert!(matches!(
        repo.check_update_clean(&[], Arc::new(AtomicBool::new(true))),
        Err(GitError::Cancelled)
    ));
    assert!(check(&repo, &[".git/config"]).is_err());
    assert!(check(&repo, &["../fuori"]).is_err());
}
