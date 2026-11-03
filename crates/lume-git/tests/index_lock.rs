//! Lock index realmente conteso: gix/Git/filter reali, mai repo utente.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use lume_git::GitRepository;
use std::{fs, path::Path};
fn lock_path(dir: &Path) -> std::path::PathBuf {
    dir.join(".git/index.lock")
}
#[test]
fn lock_esistente_rifiuta_stage_unstage_hunk_senza_sottrarlo_o_scrivere_index() {
    let dir = common::init_repo();
    fs::write(dir.path().join("a.txt"), b"due\n").unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    let diff = repo.diff_worktree_file(Path::new("a.txt")).unwrap();
    repo.stage(["a.txt"]).unwrap();
    let staged = repo.diff_index_file(Path::new("a.txt")).unwrap();
    let old = fs::read(dir.path().join(".git/index")).unwrap();
    fs::write(lock_path(dir.path()), b"lock esterno\n").unwrap();
    assert!(repo.stage(["a.txt"]).is_err());
    assert!(repo.unstage(["a.txt"]).is_err());
    assert!(
        repo.stage_hunk(Path::new("a.txt"), diff.hunks.first().unwrap())
            .is_err()
    );
    assert!(
        repo.unstage_hunk(Path::new("a.txt"), staged.hunks.first().unwrap())
            .is_err()
    );
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), old);
    assert_eq!(fs::read(lock_path(dir.path())).unwrap(), b"lock esterno\n");
}
#[test]
fn errore_dopo_acquisizione_libera_solo_il_proprio_lock() {
    let dir = common::init_repo();
    let repo = GitRepository::discover(dir.path()).unwrap();
    let old = fs::read(dir.path().join(".git/index")).unwrap();
    let outside = tempfile::tempdir().unwrap();
    assert!(repo.stage([outside.path().join("fuori.txt")]).is_err());
    assert!(!lock_path(dir.path()).exists());
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), old);
    fs::write(dir.path().join("a.txt"), b"due\n").unwrap();
    repo.stage(["a.txt"]).unwrap();
    assert!(!lock_path(dir.path()).exists());
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
#[test]
fn stage_non_perde_index_esterno_a_mtime_invariato_dopo_snapshot_precedente() {
    let dir = common::init_repo();
    fs::write(dir.path().join("a.txt"), b"due\n").unwrap();
    fs::write(dir.path().join("b.txt"), b"concorrente\n").unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    repo.diff_index_file(Path::new("a.txt")).unwrap();
    // Status31/diff28/stash45 sono freschi: nessuno riscalda cache mtime.
    // Questo snapshot precedente produce solo refs/ODB nella TempDir. Prova
    // con cache nativa davvero precaricata in tests/stash_index/mod.rs (cfg(test)).
    repo.stash_push(Some("fixture snapshot stage")).unwrap();
    let index_path = dir.path().join(".git/index");
    let time = fs::metadata(&index_path).unwrap().modified().unwrap();
    common::git(dir.path(), &["add", "--", "b.txt"]);
    fs::OpenOptions::new()
        .write(true)
        .open(&index_path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(time))
        .unwrap();
    repo.stage(["a.txt"]).unwrap();
    assert_eq!(
        common::git(dir.path(), &["diff", "--cached", "--name-only"]),
        "a.txt\nb.txt",
        "stesso mtime non autorizza a riusare index precedente"
    );
    repo.unstage(["a.txt"]).unwrap();
    assert_eq!(
        common::git(dir.path(), &["diff", "--cached", "--name-only"]),
        "b.txt"
    );
    assert!(!lock_path(dir.path()).exists());
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}

#[test]
fn linked_worktree_blocca_index_corretto_non_quello_principale() {
    let dir = common::init_repo();
    let holder = tempfile::tempdir().unwrap();
    let linked = holder.path().join("linked");
    common::git(
        dir.path(),
        &["worktree", "add", "-b", "linked", linked.to_str().unwrap()],
    );
    let index_path = gix::discover(&linked).unwrap().index_path();
    let lock = index_path.with_file_name("index.lock");
    let main_index = fs::read(dir.path().join(".git/index")).unwrap();
    let previous = fs::read(&index_path).unwrap();
    fs::write(linked.join("a.txt"), b"due\n").unwrap();
    fs::write(&lock, b"esterno linked\n").unwrap();
    let repo = GitRepository::discover(&linked).unwrap();
    assert!(repo.stage(["a.txt"]).is_err());
    assert_eq!(fs::read(&lock).unwrap(), b"esterno linked\n");
    assert_eq!(fs::read(&index_path).unwrap(), previous);
    fs::remove_file(&lock).unwrap();
    repo.stage(["a.txt"]).unwrap();
    assert!(!lock.exists());
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), main_index);
    assert_eq!(
        common::git(&linked, &["diff", "--cached", "--name-only"]),
        "a.txt"
    );
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
#[test]
fn index_corrotto_non_viene_sostituito_e_guard_libera_lock_acquisito() {
    let dir = common::init_repo();
    fs::write(dir.path().join(".git/index"), b"non un index\n").unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert!(repo.stage(["a.txt"]).is_err());
    assert_eq!(
        fs::read(dir.path().join(".git/index")).unwrap(),
        b"non un index\n"
    );
    assert!(!lock_path(dir.path()).exists());
}

#[cfg(unix)]
#[test]
fn filtro_reale_non_puo_pubblicare_un_secondo_index_durante_stage() {
    let dir = common::init_repo();
    common::commit_file(
        dir.path(),
        ".gitattributes",
        "a.txt filter=contesa\n",
        "attributi",
    );
    fs::write(dir.path().join("a.txt"), b"due\n").unwrap();
    fs::write(dir.path().join("b.txt"), b"concorrente\n").unwrap();
    // Comando esclusivamente della fixture: git reale prova il lock, poi
    // clean passa stdin invariato. Non factory/reader/driver simulato.
    let root = dir.path().to_str().unwrap().replace('\'', "'\\''");
    let clean = format!(
        "cd '{root}' || exit 97; unset GIT_INDEX_FILE GIT_DIR GIT_WORK_TREE; if test -e .git/index.lock; then printf 'si\\n' >>prima.lock; else printf 'no\\n' >>prima.lock; fi; git add -- b.txt >tentativo.out 2>tentativo.err; codice=$?; printf '%s' \"$codice\" >tentativo.esito; cat"
    );
    common::git(dir.path(), &["config", "filter.contesa.clean", &clean]);
    let repo = GitRepository::discover(dir.path()).unwrap();
    repo.stage(["a.txt"]).unwrap();
    let code = fs::read_to_string(dir.path().join("tentativo.esito")).unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("prima.lock"))
            .unwrap()
            .lines()
            .next(),
        Some("si"),
        "il primissimo filtro deve vedere il lock, non soltanto il Git annidato durante refresh"
    );
    assert_ne!(
        code, "0",
        "il lock deve esistere prima del filtro: un Git concorrente non può modificare index"
    );
    assert!(
        fs::read_to_string(dir.path().join("tentativo.err"))
            .unwrap()
            .contains("index.lock")
    );
    assert!(!lock_path(dir.path()).exists());
    assert_eq!(
        common::git(dir.path(), &["diff", "--cached", "--name-only"]),
        "a.txt"
    );
    // Disattivare il filtro della fixture prima di ulteriori verifiche.
    common::git(dir.path(), &["config", "--unset", "filter.contesa.clean"]);
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
