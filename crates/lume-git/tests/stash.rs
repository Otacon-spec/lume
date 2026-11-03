#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::Command;

use lume_git::GitRepository;

/// Directory posseduta, creata esclusivamente: il clock non è un ID univoco
/// fra test paralleli. Drop ripulisce anche un test che fallisce.
fn temp_root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn signature() -> gix::actor::SignatureRef<'static> {
    gix::actor::SignatureRef::from_bytes(b"Lume Test <lume@example.test> 1700000000 +0100").unwrap()
}

fn commit_file(repository: &gix::Repository, root: &Path, content: &[u8]) {
    fs::write(root.join("nota.md"), content).unwrap();
    let blob = repository.write_blob(content).unwrap().detach();
    let tree = repository
        .write_object(gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: "nota.md".into(),
                oid: blob,
            }],
        })
        .unwrap()
        .detach();
    repository
        .commit_as(
            signature(),
            signature(),
            "HEAD",
            "base",
            tree,
            Vec::<gix::ObjectId>::new(),
        )
        .unwrap();
}

fn git(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}

fn git_ok(root: &Path, args: &[&str]) -> String {
    let output = git(root, args);
    assert!(
        output.status.success(),
        "git {} fallito:\nstdout: {}\nstderr: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn init_repo(root: &Path) -> gix::Repository {
    let repository = gix::init(root).unwrap();
    git_ok(root, &["config", "user.name", "Lume Test"]);
    git_ok(root, &["config", "user.email", "lume@example.test"]);
    git_ok(root, &["config", "commit.gpgsign", "false"]);
    repository
}

/// Lo stash scritto da Lume e' condiviso con Git: deve quindi avere i due
/// parent standard (HEAD e fotografia dell'index) e `git stash apply --index`
/// deve ripristinare separatamente modifiche staged e unstaged.
#[test]
fn stash_push_e_interoperabile_con_git_apply_index() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let temp = temp_root();
    let root = temp.path().to_path_buf();
    let repository = init_repo(&root);
    commit_file(&repository, &root, b"base\n");
    git_ok(&root, &["reset", "--mixed", "HEAD"]);
    fs::write(root.join("in_stage.md"), b"base stage\n").unwrap();
    git_ok(&root, &["add", "in_stage.md"]);
    git_ok(&root, &["commit", "-m", "aggiunge secondo file"]);

    fs::write(root.join("in_stage.md"), b"contenuto staged\n").unwrap();
    git_ok(&root, &["add", "in_stage.md"]);
    fs::write(root.join("nota.md"), b"contenuto unstaged\n").unwrap();

    GitRepository::discover(&root)
        .unwrap()
        .stash_push(Some("interoperabile"))
        .unwrap();

    let parents = git_ok(&root, &["rev-list", "--parents", "-n", "1", "refs/stash"]);
    assert_eq!(
        parents.split_whitespace().count(),
        3,
        "il commit stash deve avere OID + parent HEAD + parent index: {parents}"
    );

    git_ok(&root, &["reset", "--hard", "HEAD"]);
    git_ok(&root, &["stash", "apply", "--index", "stash@{0}"]);

    assert_eq!(
        fs::read_to_string(root.join("in_stage.md"))
            .unwrap()
            .replace("\r\n", "\n"),
        "contenuto staged\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("nota.md"))
            .unwrap()
            .replace("\r\n", "\n"),
        "contenuto unstaged\n"
    );
    assert!(
        git_ok(&root, &["diff", "--cached", "--name-only"])
            .lines()
            .any(|path| path == "in_stage.md")
    );
    assert!(
        git_ok(&root, &["diff", "--name-only"])
            .lines()
            .any(|path| path == "nota.md")
    );
}

/// «Salva stash» deve fotografare anche le modifiche non in stage. Il difetto
/// precedente costruiva il commit dal solo index e salvava quindi il file
/// vecchio pur mostrando una nuova voce stash riuscita.
#[test]
fn stash_push_include_le_modifiche_non_in_stage() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let temp = temp_root();
    let root = temp.path().to_path_buf();
    let repository = init_repo(&root);
    commit_file(&repository, &root, b"vecchio\n");
    fs::write(root.join("nota.md"), b"nuovo\n").unwrap();

    let git = GitRepository::discover(&root).unwrap();
    git.stash_push(Some("prova")).unwrap();

    let repository = gix::discover(&root).unwrap();
    let stash = repository
        .find_reference("refs/stash")
        .unwrap()
        .into_fully_peeled_id()
        .unwrap();
    let tree = repository.find_commit(stash).unwrap().tree().unwrap();
    let entry = tree
        .lookup_entry_by_path("nota.md")
        .unwrap()
        .expect("nota.md deve esistere nello stash");
    let blob = repository.find_object(entry.object_id()).unwrap();
    assert_eq!(blob.data.as_slice(), b"nuovo\n");
}

/// Rimuovere lo stash piu' recente non deve creare una nuova voce di reflog
/// chiamata «stash drop», altrimenti il pannello continua a mostrare due voci.
#[test]
fn stash_drop_rimuove_esattamente_la_voce_scelta() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let temp = temp_root();
    let root = temp.path().to_path_buf();
    let repository = init_repo(&root);
    commit_file(&repository, &root, b"base\n");
    let git = GitRepository::discover(&root).unwrap();

    fs::write(root.join("nota.md"), b"primo\n").unwrap();
    git.stash_push(Some("primo")).unwrap();
    fs::write(root.join("nota.md"), b"secondo\n").unwrap();
    git.stash_push(Some("secondo")).unwrap();
    assert_eq!(git.stash_list().unwrap().len(), 2);

    git.stash_drop(0).unwrap();

    let entries = git.stash_list().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries.first().unwrap().message, "primo");
}

/// stash_push fotografa i file tracciati senza ripristinare index/worktree.
#[test]
fn stash_push_salva_i_tracciati_preservando_index_worktree_e_non_versionati() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let temp = temp_root();
    let root = temp.path().to_path_buf();
    let repository = init_repo(&root);
    commit_file(&repository, &root, b"base\n");
    git_ok(&root, &["reset", "--mixed", "HEAD"]);
    fs::write(root.join("nota.md"), b"modificato\n").unwrap();
    fs::write(root.join("nuovo_in_stage.md"), b"nuovo\n").unwrap();
    git_ok(&root, &["add", "nuovo_in_stage.md"]);
    fs::write(root.join("mai_versionato.md"), b"mio\n").unwrap();
    let index_before = fs::read(root.join(".git/index")).unwrap();

    GitRepository::discover(&root)
        .unwrap()
        .stash_push(Some("wip"))
        .unwrap();

    assert_eq!(fs::read(root.join("nota.md")).unwrap(), b"modificato\n");
    assert_eq!(
        fs::read(root.join("nuovo_in_stage.md")).unwrap(),
        b"nuovo\n"
    );
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index_before);
    assert_eq!(fs::read(root.join("mai_versionato.md")).unwrap(), b"mio\n");
    assert_eq!(
        git_ok(&root, &["status", "--porcelain"]).trim_end(),
        " M nota.md\nA  nuovo_in_stage.md\n?? mai_versionato.md"
    );
    let salvati = git_ok(&root, &["ls-tree", "-r", "--name-only", "refs/stash"]);
    assert_eq!(
        salvati.lines().collect::<Vec<_>>(),
        vec!["nota.md", "nuovo_in_stage.md"]
    );
    assert_eq!(git_ok(&root, &["stash", "list"]).lines().count(), 1);
}

#[test]
fn stash_drop_di_una_voce_in_mezzo_lascia_il_reflog_concatenato() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let temp = temp_root();
    let root = temp.path().to_path_buf();
    let repository = init_repo(&root);
    commit_file(&repository, &root, b"base\n");
    let git = GitRepository::discover(&root).unwrap();
    for nome in ["uno", "due", "tre"] {
        fs::write(root.join("nota.md"), format!("{nome}\n")).unwrap();
        git.stash_push(Some(nome)).unwrap();
    }

    git.stash_drop(1).unwrap();

    let elenco: Vec<String> = git_ok(&root, &["stash", "list", "--format=%gs"])
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(elenco, vec!["tre".to_owned(), "uno".to_owned()]);
    let log = fs::read_to_string(root.join(".git/logs/refs/stash")).unwrap();
    let mut atteso_old = "0".repeat(40);
    for line in log.lines() {
        let mut parti = line.splitn(3, ' ');
        let old = parti.next().unwrap();
        let new = parti.next().unwrap();
        assert_eq!(old, atteso_old, "reflog non concatenato: {log}");
        atteso_old = new.to_owned();
    }
    assert!(!log.contains("stash drop"), "{log}");
    assert_eq!(
        git_ok(&root, &["rev-parse", "refs/stash"]).trim(),
        atteso_old
    );
    git_ok(&root, &["fsck", "--no-progress"]);
}

#[test]
fn stash_drop_dell_ultimo_toglie_ref_e_reflog() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let temp = temp_root();
    let root = temp.path().to_path_buf();
    let repository = init_repo(&root);
    commit_file(&repository, &root, b"base\n");
    let git = GitRepository::discover(&root).unwrap();
    fs::write(root.join("nota.md"), b"x\n").unwrap();
    git.stash_push(Some("x")).unwrap();

    git.stash_drop(0).unwrap();

    assert!(git.stash_list().unwrap().is_empty());
    assert!(!root.join(".git/refs/stash").exists());
    assert!(!root.join(".git/logs/refs/stash").exists());
    assert_eq!(git_ok(&root, &["stash", "list"]).trim(), "");
}

#[test]
fn stash_drop_rifiuta_se_il_reflog_e_bloccato() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let temp = temp_root();
    let root = temp.path().to_path_buf();
    let repository = init_repo(&root);
    commit_file(&repository, &root, b"base\n");
    let git = GitRepository::discover(&root).unwrap();
    fs::write(root.join("nota.md"), b"x\n").unwrap();
    git.stash_push(Some("x")).unwrap();
    let lock = root.join(".git/logs/refs/stash.lock");
    fs::write(&lock, b"").unwrap();

    let errore = git.stash_drop(0).unwrap_err();

    assert!(matches!(errore, lume_git::GitError::Stash(_)), "{errore:?}");
    assert!(lock.exists(), "il lock di un altro processo non si tocca");
    assert_eq!(git.stash_list().unwrap().len(), 1);
}

#[test]
fn stash_drop_rifiuta_lock_ref_anche_per_una_voce_non_in_cima() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let temp = temp_root();
    let root = temp.path().to_path_buf();
    let repository = init_repo(&root);
    commit_file(&repository, &root, b"base\n");
    let git = GitRepository::discover(&root).unwrap();
    for nome in ["uno", "due"] {
        fs::write(root.join("nota.md"), format!("{nome}\n")).unwrap();
        git.stash_push(Some(nome)).unwrap();
    }
    let log_path = root.join(".git/logs/refs/stash");
    let log_before = fs::read(&log_path).unwrap();
    let lock = root.join(".git/refs/stash.lock");
    fs::write(&lock, b"").unwrap();

    let errore = git.stash_drop(1).unwrap_err();

    assert!(matches!(errore, lume_git::GitError::Stash(_)), "{errore:?}");
    assert!(
        lock.exists(),
        "il lock refs di un altro processo non si tocca"
    );
    assert_eq!(fs::read(&log_path).unwrap(), log_before);
    assert_eq!(git.stash_list().unwrap().len(), 2);
}

#[test]
fn stash_drop_di_un_indice_inesistente_e_un_errore() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let temp = temp_root();
    let root = temp.path().to_path_buf();
    let repository = init_repo(&root);
    commit_file(&repository, &root, b"base\n");
    let git = GitRepository::discover(&root).unwrap();
    assert!(matches!(
        git.stash_drop(0).unwrap_err(),
        lume_git::GitError::Stash(_)
    ));
}

#[cfg(unix)]
#[test]
fn stash_drop_non_segue_un_lock_reflog_simbolico_rotto() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    use std::os::unix::fs::symlink;

    let temp = temp_root();
    let root = temp.path().to_path_buf();
    let repository = init_repo(&root);
    commit_file(&repository, &root, b"base\n");
    let git = GitRepository::discover(&root).unwrap();
    fs::write(root.join("nota.md"), b"x\n").unwrap();
    git.stash_push(Some("x")).unwrap();

    let lock = root.join(".git/logs/refs/stash.lock");
    let target = root.join("fuori-dal-reflog");
    symlink(&target, &lock).unwrap();
    let reflog = fs::read(root.join(".git/logs/refs/stash")).unwrap();

    let result = git.stash_drop(0);

    assert!(matches!(result, Err(lume_git::GitError::Stash(_))));
    assert!(
        fs::symlink_metadata(&lock)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        !target.exists(),
        "il controllo del lock non deve seguire il link"
    );
    assert_eq!(fs::read(root.join(".git/logs/refs/stash")).unwrap(), reflog);
    assert_eq!(git.stash_list().unwrap().len(), 1);
}
