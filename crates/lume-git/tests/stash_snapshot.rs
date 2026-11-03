//! Fotografia tracked reale: non worktree reset/CLI apply/pop/UI acceptance.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use lume_git::GitRepository;
use std::{fs, process::Command};
#[test]
fn snapshot_esclude_untracked_preserva_stage_e_worktree_distinti() {
    let dir = common::init_repo();
    fs::write(dir.path().join("b.txt"), b"stage\n").unwrap();
    common::git(dir.path(), &["add", "--", "b.txt"]);
    fs::write(dir.path().join("b.txt"), b"worktree\n").unwrap();
    fs::write(
        dir.path().join("untracked.sql"),
        b"solo file non versionato\n",
    )
    .unwrap();
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    GitRepository::discover(dir.path())
        .unwrap()
        .stash_push(Some("tracked soltanto"))
        .unwrap();
    assert_eq!(
        common::git(dir.path(), &["ls-tree", "-r", "--name-only", "refs/stash"]),
        "a.txt\nb.txt"
    );
    assert_eq!(
        common::git(dir.path(), &["show", "refs/stash^2:b.txt"]),
        "stage"
    );
    assert_eq!(
        common::git(dir.path(), &["show", "refs/stash:b.txt"]),
        "worktree"
    );
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(dir.path().join("b.txt")).unwrap(), b"worktree\n");
    assert_eq!(
        fs::read(dir.path().join("untracked.sql")).unwrap(),
        b"solo file non versionato\n"
    );
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
#[test]
fn hint_index_non_nascondono_byte_tracked_nello_stash() {
    for hint in ["--assume-unchanged", "--skip-worktree"] {
        let dir = common::init_repo();
        common::git(dir.path(), &["update-index", hint, "--", "a.txt"]);
        let before = fs::metadata(dir.path().join("a.txt"))
            .unwrap()
            .modified()
            .unwrap();
        fs::write(dir.path().join("a.txt"), b"due\n").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(dir.path().join("a.txt"))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(before))
            .unwrap();
        let index = fs::read(dir.path().join(".git/index")).unwrap();
        GitRepository::discover(dir.path())
            .unwrap()
            .stash_push(Some("byte reali"))
            .unwrap();
        assert_eq!(
            common::git(dir.path(), &["show", "refs/stash:a.txt"]),
            "due",
            "non fidarsi di hint/stat: {hint}"
        );
        assert_eq!(
            common::git(dir.path(), &["show", "refs/stash^2:a.txt"]),
            "uno"
        );
        assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
        assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"due\n");
    }
}
#[test]
fn intent_to_add_rifiutato_prima_snapshot_pubblicato() {
    let dir = common::init_repo();
    fs::write(dir.path().join("a.txt"), b"dirty\n").unwrap();
    fs::write(dir.path().join("new.sql"), b"intent reale\n").unwrap();
    common::git(dir.path(), &["add", "-N", "--", "new.sql"]);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    assert!(
        GitRepository::discover(dir.path())
            .unwrap()
            .stash_push(Some("ITA non è stage approvato"))
            .is_err()
    );
    assert!(!dir.path().join(".git/refs/stash").exists());
    assert!(!dir.path().join(".git/logs/refs/stash").exists());
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"dirty\n");
    assert_eq!(
        fs::read(dir.path().join("new.sql")).unwrap(),
        b"intent reale\n"
    );
    assert!(!dir.path().join(".git/index.lock").exists());
}
#[test]
#[cfg(unix)]
fn helper_selezionato_rifiutato_anche_con_hint_file_pulito() {
    for key in ["filter.cleanup.clean", "filter.cleanup.process"] {
        let dir = common::init_repo();
        // Configura l'hint PRIMA dei driver: Git update-index può hashare e
        // avviare clean anche senza un add. Nessun helper nella fixture CLI.
        common::git(
            dir.path(),
            &["update-index", "--assume-unchanged", "--", "a.txt"],
        );
        let helper = tempfile::tempdir().unwrap();
        let script = helper.path().join("helper.py");
        let marker = helper.path().join("started");
        fs::write(&script,"import pathlib,sys\npathlib.Path(sys.argv[1]).write_text('avviato')\nsys.stdout.buffer.write(sys.stdin.buffer.read())\n").unwrap();
        common::git(
            dir.path(),
            &[
                "config",
                key,
                &format!("python3 '{}' '{}'", script.display(), marker.display()),
            ],
        );
        common::git(dir.path(), &["config", "filter.cleanup.required", "true"]);
        fs::write(
            dir.path().join(".gitattributes"),
            b"[attr]speciale filter=cleanup\na.txt speciale\n",
        )
        .unwrap();
        assert!(
            !marker.exists(),
            "precondizione: nessun helper già avviato dalla fixture"
        );
        let index = fs::read(dir.path().join(".git/index")).unwrap();
        let result = GitRepository::discover(dir.path())
            .unwrap()
            .stash_push(Some("nessun helper non posseduto"));
        assert!(
            matches!(result,Err(lume_git::GitError::Worktree(ref e)) if e.contains("filtro comando selezionato"))
        );
        assert!(!marker.exists());
        assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
        assert!(!dir.path().join(".git/refs/stash").exists());
        assert!(!dir.path().join(".git/index.lock").exists());
    }
}
#[test]
#[cfg(unix)]
fn builtin_info_override_e_driver_solo_smudge_non_bloccano_snapshot_tracked() {
    let dir = common::init_repo();
    let helper = tempfile::tempdir().unwrap();
    let marker = helper.path().join("started");
    // Solo smudge: stash legge verso Git, non checkout. Il comando NON eseguito.
    common::git(
        dir.path(),
        &[
            "config",
            "filter.cleanup.smudge",
            &format!("touch '{}'", marker.display()),
        ],
    );
    fs::write(dir.path().join(".gitattributes"), b"a.txt filter=cleanup\n").unwrap();
    fs::write(dir.path().join("a.txt"), b"due\n").unwrap();
    GitRepository::discover(dir.path())
        .unwrap()
        .stash_push(Some("solo smudge"))
        .unwrap();
    assert_eq!(
        common::git(dir.path(), &["show", "refs/stash:a.txt"]),
        "due"
    );
    assert!(!marker.exists());
    // Driver clean configurato ma info override lo deseleziona nativamente.
    common::git(
        dir.path(),
        &[
            "config",
            "filter.cleanup.clean",
            &format!("touch '{}'", marker.display()),
        ],
    );
    fs::write(
        dir.path().join(".git/info/attributes"),
        b"a.txt -filter text eol=crlf\n",
    )
    .unwrap();
    fs::write(dir.path().join("a.txt"), b"tre\r\n").unwrap();
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    GitRepository::discover(dir.path())
        .unwrap()
        .stash_push(Some("override native CRLF"))
        .unwrap();
    assert_eq!(
        common::git(dir.path(), &["show", "refs/stash:a.txt"]),
        "tre"
    );
    assert!(!marker.exists());
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"tre\r\n");
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    let raw = Command::new("git")
        .args(["show", "refs/stash:a.txt"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(raw.status.success());
    assert_eq!(
        raw.stdout, b"tre\n",
        "verificare byte clean, non stringa trim"
    );
}
#[test]
fn conflitti_index_non_omessi_silenziosamente_nello_stash() {
    let dir = common::init_repo();
    common::git(dir.path(), &["checkout", "-b", "other"]);
    fs::write(dir.path().join("a.txt"), b"loro\n").unwrap();
    common::git(dir.path(), &["commit", "-am", "loro"]);
    common::git(dir.path(), &["checkout", "main"]);
    fs::write(dir.path().join("a.txt"), b"mio\n").unwrap();
    common::git(dir.path(), &["commit", "-am", "mio"]);
    let out = Command::new("git")
        .args(["merge", "--no-edit", "other"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let original = fs::read(dir.path().join("a.txt")).unwrap();
    assert!(
        GitRepository::discover(dir.path())
            .unwrap()
            .stash_push(Some("rifiuto conflitti"))
            .is_err()
    );
    assert!(!dir.path().join(".git/refs/stash").exists());
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), original);
    assert!(dir.path().join(".git/MERGE_HEAD").exists());
    assert!(!dir.path().join(".git/index.lock").exists());
}
