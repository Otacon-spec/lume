#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Solo Git vero, remote bare su disco e repository temporanei.
mod common;
use lume_git::{GitError, GitRepository, validate_ref_name};
use std::sync::atomic::AtomicBool;
fn no() -> AtomicBool {
    AtomicBool::new(false)
}
fn open(dir: &std::path::Path) -> GitRepository {
    GitRepository::discover(dir).unwrap()
}
fn feat() -> tempfile::TempDir {
    let work = common::init_repo();
    common::git(work.path(), &["checkout", "-b", "feat"]);
    common::commit_file(work.path(), "a.txt", "feat\n", "feat");
    common::git(work.path(), &["checkout", "main"]);
    work
}
fn remote_feat() -> (tempfile::TempDir, tempfile::TempDir) {
    let (origin, work) = common::repo_with_origin();
    let other = common::clone_of(origin.path());
    common::git(other.path(), &["checkout", "-b", "feat"]);
    common::commit_file(other.path(), "a.txt", "feat\n", "feat");
    common::git(other.path(), &["push", "origin", "feat"]);
    common::git(work.path(), &["fetch", "origin"]);
    (origin, work)
}
#[test]
fn nomi_validi_e_nomi_opzione_o_ref_ostili() {
    for name in [
        "main",
        "feature/x",
        "release-1.0",
        "utente/fix_123",
        "feature/città",
        "main;calc",
    ] {
        assert!(validate_ref_name(name).is_ok(), "{name}");
    }
    for name in [
        "",
        "-f",
        "--upload-pack=calc",
        "--orphan",
        "a b",
        "a..b",
        "x/",
        "feat.lock",
        "@{u}",
        "a~1",
        "a:b",
        "/x",
        "a//b",
        ".nascosto",
        "x/.y",
        "x/z.lock",
        "x.",
        "a\nb",
        "a\\b",
        "a?b",
        "a[b",
    ] {
        assert!(
            matches!(validate_ref_name(name), Err(GitError::InvalidBranchName(_))),
            "{name}"
        );
    }
}
#[test]
fn switch_cambia_branch_e_file_senza_guess_remoto() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = feat();
    open(work.path())
        .switch_branch(&common::cli(), "feat", &no())
        .unwrap();
    assert_eq!(
        open(work.path()).branch_name().unwrap().as_deref(),
        Some("feat")
    );
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "feat\n"
    );
    let (_origin, work) = remote_feat();
    assert!(matches!(
        open(work.path()).switch_branch(&common::cli(), "feat", &no()),
        Err(GitError::Switch(_))
    ));
    assert!(
        !common::git(work.path(), &["branch", "--format=%(refname:short)"])
            .lines()
            .any(|x| x == "feat")
    );
}
#[test]
fn switch_rifiuta_opzioni_inesistente_e_modifiche_locali_senza_perderle() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = feat();
    let repo = open(work.path());
    let cli = common::cli();
    assert!(matches!(
        repo.switch_branch(&cli, "--force", &no()),
        Err(GitError::InvalidBranchName(_))
    ));
    assert!(matches!(
        repo.switch_branch(&cli, "main;calc", &no()),
        Err(GitError::Switch(_))
    ));
    std::fs::write(work.path().join("a.txt"), "bozza\n").unwrap();
    assert!(matches!(
        repo.switch_branch(&cli, "feat", &no()),
        Err(GitError::Switch(_))
    ));
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "bozza\n"
    );
    assert_eq!(
        open(work.path()).branch_name().unwrap().as_deref(),
        Some("main")
    );
}
#[test]
fn switch_annullato_non_cambia_branch() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = feat();
    assert!(matches!(
        open(work.path()).switch_branch(&common::cli(), "feat", &AtomicBool::new(true)),
        Err(GitError::Cancelled)
    ));
    assert_eq!(
        open(work.path()).branch_name().unwrap().as_deref(),
        Some("main")
    );
}
#[test]
fn switch_remoto_crea_tracking_esatto_anche_se_esiste_un_locale_ambiguo() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (_origin, work) = remote_feat();
    common::git(work.path(), &["branch", "origin/feat", "main"]);
    let repo = open(work.path());
    assert_eq!(
        repo.switch_remote_branch(&common::cli(), "origin", "feat", &no())
            .unwrap(),
        "feat"
    );
    assert_eq!(
        open(work.path()).upstream().unwrap().unwrap().tracking_ref,
        "refs/remotes/origin/feat"
    );
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "feat\n"
    );
}
#[test]
fn switch_remoto_rifiuta_locale_esistente_e_nomi_ostili() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (_origin, work) = remote_feat();
    common::git(work.path(), &["branch", "feat", "origin/feat"]);
    let repo = open(work.path());
    assert!(matches!(
        repo.switch_remote_branch(&common::cli(), "origin", "feat", &no()),
        Err(GitError::Switch(_))
    ));
    for (remote, name) in [("--upload-pack=calc", "feat"), ("origin", "--force")] {
        assert!(matches!(
            repo.switch_remote_branch(&common::cli(), remote, name, &no()),
            Err(GitError::InvalidBranchName(_))
        ));
    }
}
#[test]
fn push_pubblica_il_branch_upstream_esplicito() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (origin, work) = common::repo_with_origin();
    common::commit_file(work.path(), "b.txt", "due\n", "secondo");
    assert!(
        open(work.path())
            .push(&common::cli(), &no())
            .unwrap()
            .contains("main")
    );
    assert_eq!(
        common::git(origin.path(), &["rev-parse", "main"]),
        common::git(work.path(), &["rev-parse", "HEAD"])
    );
}
#[test]
fn push_senza_upstream_imposta_origin_ma_senza_remote_rifiuta() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let work = common::init_repo();
    assert!(matches!(
        open(work.path()).push(&common::cli(), &no()),
        Err(GitError::NoUpstream(_))
    ));
    let origin = common::bare_origin();
    common::git(
        work.path(),
        &["remote", "add", "origin", &origin.path().to_string_lossy()],
    );
    open(work.path()).push(&common::cli(), &no()).unwrap();
    assert_eq!(
        open(work.path()).upstream().unwrap().unwrap().tracking_ref,
        "refs/remotes/origin/main"
    );
    assert_eq!(
        common::git(origin.path(), &["rev-parse", "main"]),
        common::git(work.path(), &["rev-parse", "HEAD"])
    );
}
#[test]
fn push_non_fast_forward_non_forza_neanche_se_il_remote_configura_force() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (origin, work) = common::repo_with_origin();
    let other = common::clone_of(origin.path());
    common::commit_file(other.path(), "x.txt", "altro\n", "altrui");
    common::git(other.path(), &["push", "origin", "main"]);
    let before = common::git(origin.path(), &["rev-parse", "main"]);
    common::commit_file(work.path(), "b.txt", "mio\n", "mio");
    common::git(
        work.path(),
        &["config", "remote.origin.push", "+refs/heads/*:refs/heads/*"],
    );
    assert!(matches!(
        open(work.path()).push(&common::cli(), &no()),
        Err(GitError::Push(_))
    ));
    assert_eq!(common::git(origin.path(), &["rev-parse", "main"]), before);
}
#[test]
fn push_annullato_e_head_staccato_non_pubblicano() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (origin, work) = common::repo_with_origin();
    let before = common::git(origin.path(), &["rev-parse", "main"]);
    common::commit_file(work.path(), "b.txt", "due\n", "secondo");
    assert!(matches!(
        open(work.path()).push(&common::cli(), &AtomicBool::new(true)),
        Err(GitError::Cancelled)
    ));
    common::git(work.path(), &["checkout", "--detach"]);
    assert!(matches!(
        open(work.path()).push(&common::cli(), &no()),
        Err(GitError::Push(_))
    ));
    assert_eq!(common::git(origin.path(), &["rev-parse", "main"]), before);
}
#[test]
fn push_rifiuta_upstream_personalizzato_senza_ripiegare_su_origin() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (origin, work) = common::repo_with_origin();
    let before = common::git(origin.path(), &["rev-parse", "main"]);
    common::commit_file(work.path(), "b.txt", "due\n", "secondo");
    common::git(
        work.path(),
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/main:refs/custom/main",
        ],
    );
    assert!(matches!(
        open(work.path()).push(&common::cli(), &no()),
        Err(GitError::NoUpstream(_))
    ));
    assert_eq!(common::git(origin.path(), &["rev-parse", "main"]), before);
}
#[test]
fn push_non_pubblica_tag_impliciti_o_altri_branch() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (origin, work) = common::repo_with_origin();
    common::commit_file(work.path(), "b.txt", "due\n", "secondo");
    common::git(work.path(), &["tag", "-a", "locale", "-m", "locale"]);
    common::git(work.path(), &["branch", "altro"]);
    common::git(work.path(), &["config", "push.followTags", "true"]);
    open(work.path()).push(&common::cli(), &no()).unwrap();
    assert_eq!(common::git(origin.path(), &["tag", "--list"]), "");
    assert_eq!(
        common::git(origin.path(), &["branch", "--format=%(refname:short)"]),
        "main"
    );
}
