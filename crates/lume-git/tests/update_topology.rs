//! Topologia Git/FS reale, non apply o token di pubblicazione.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use lume_git::{GitError, GitRepository};
use std::{fs, path::Path, sync::atomic::AtomicBool};
fn tree(root: &Path) -> gix::ObjectId {
    gix::ObjectId::from_hex(common::git(root, &["rev-parse", "HEAD^{tree}"]).as_bytes()).unwrap()
}
fn pair(file_to_dir: bool) -> (tempfile::TempDir, gix::ObjectId, gix::ObjectId) {
    let dir = common::init_repo();
    common::commit_file(
        dir.path(),
        if file_to_dir { "slot" } else { "slot/b" },
        "base\n",
        "old",
    );
    let old = tree(dir.path());
    common::git(dir.path(), &["switch", "-c", "nuovo"]);
    common::git(dir.path(), &["rm", "-r", "--", "slot"]);
    common::commit_file(
        dir.path(),
        if file_to_dir { "slot/b" } else { "slot" },
        "nuovo\n",
        "new",
    );
    let new = tree(dir.path());
    common::git(dir.path(), &["switch", "main"]);
    (dir, old, new)
}
fn no() -> AtomicBool {
    AtomicBool::new(false)
}
#[test]
fn file_cartella_proiezione_non_rifiuta_antenato_da_rimuovere() {
    let (dir, old, new) = pair(true);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    let head = common::git(dir.path(), &["rev-parse", "HEAD"]);
    GitRepository::discover(dir.path())
        .unwrap()
        .check_update_topology(old, new, &no())
        .unwrap();
    assert_eq!(fs::read(dir.path().join("slot")).unwrap(), b"base\n");
    assert!(!dir.path().join("slot/b").exists());
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(common::git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert!(!dir.path().join(".git/index.lock").exists());
    common::git(dir.path(), &["fsck", "--strict", "--no-progress"]);
}
#[test]
fn cartella_file_proiezione_non_cancella_contenuto_o_directory() {
    let (dir, old, new) = pair(false);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    GitRepository::discover(dir.path())
        .unwrap()
        .check_update_topology(old, new, &no())
        .unwrap();
    assert!(dir.path().join("slot").is_dir());
    assert_eq!(fs::read(dir.path().join("slot/b")).unwrap(), b"base\n");
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
}
#[test]
fn collasso_rifiuta_file_locale_e_cartella_vuota_non_previsti() {
    for empty_dir in [false, true] {
        let (dir, old, new) = pair(false);
        if empty_dir {
            fs::create_dir(dir.path().join("slot/locale")).unwrap();
        } else {
            fs::write(dir.path().join("slot/locale"), b"locale\n").unwrap();
        }
        assert!(
            GitRepository::discover(dir.path())
                .unwrap()
                .check_update_topology(old, new, &no())
                .is_err()
        );
        assert!(dir.path().join("slot/locale").exists());
        assert_eq!(fs::read(dir.path().join("slot/b")).unwrap(), b"base\n");
    }
}
#[test]
fn readonly_leaf_o_parent_proteggono_anche_la_rimozione_virtuale() {
    for file_to_dir in [false, true] {
        let (dir, old, new) = pair(file_to_dir);
        let path = dir.path().join(if file_to_dir { "slot" } else { "slot/b" });
        let original = fs::metadata(&path).unwrap().permissions();
        let mut permissions = original.clone();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        let result = GitRepository::discover(dir.path())
            .unwrap()
            .check_update_topology(old, new, &no());
        fs::set_permissions(path, original).unwrap();
        assert!(result.is_err());
    }
}
#[cfg(unix)]
#[test]
fn link_leaf_parent_e_link_nel_collasso_non_seguiti() {
    use std::os::unix::fs::symlink;
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("esterno"), b"esterno\n").unwrap();
    for collapse in [false, true] {
        let (dir, old, new) = pair(!collapse);
        if collapse {
            symlink(outside.path(), dir.path().join("slot/locale")).unwrap();
        } else {
            fs::remove_file(dir.path().join("slot")).unwrap();
            symlink(outside.path(), dir.path().join("slot")).unwrap();
        }
        assert!(
            GitRepository::discover(dir.path())
                .unwrap()
                .check_update_topology(old, new, &no())
                .is_err()
        );
        assert_eq!(
            fs::read(outside.path().join("esterno")).unwrap(),
            b"esterno\n"
        );
    }
}
#[test]
fn collasso_annidato_e_parent_readonly_sono_verificati_senza_mutazioni() {
    let dir = common::init_repo();
    common::commit_file(dir.path(), "slot/dir/a", "a\n", "a");
    common::commit_file(dir.path(), "slot/dir/b", "b\n", "b");
    let old = tree(dir.path());
    common::git(dir.path(), &["switch", "-c", "nuovo"]);
    common::git(dir.path(), &["rm", "-r", "--", "slot"]);
    common::commit_file(dir.path(), "slot", "new\n", "new");
    let new = tree(dir.path());
    common::git(dir.path(), &["switch", "main"]);
    let repo = GitRepository::discover(dir.path()).unwrap();
    repo.check_update_topology(old, new, &no()).unwrap();
    let path = dir.path().join("slot/dir");
    let original = fs::metadata(&path).unwrap().permissions();
    let mut ro = original.clone();
    ro.set_readonly(true);
    fs::set_permissions(&path, ro).unwrap();
    let result = repo.check_update_topology(old, new, &no());
    fs::set_permissions(path, original).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read(dir.path().join("slot/dir/a")).unwrap(), b"a\n");
    assert_eq!(fs::read(dir.path().join("slot/dir/b")).unwrap(), b"b\n");
}
#[test]
fn modi_non_supportati_e_nomi_ostili_in_tree_reali_rifiutati() {
    let dir = common::init_repo();
    let raw = gix::open(dir.path()).unwrap();
    let old = raw
        .write_object(&gix::objs::Tree { entries: vec![] })
        .unwrap()
        .detach();
    let blob = raw
        .write_object(gix::objs::BlobRef { data: b"payload\n" })
        .unwrap()
        .detach();
    let commit =
        gix::ObjectId::from_hex(common::git(dir.path(), &["rev-parse", "HEAD"]).as_bytes())
            .unwrap();
    for (name, kind, id) in [
        ("link", gix::object::tree::EntryKind::Link, blob),
        ("sub", gix::object::tree::EntryKind::Commit, commit),
        ("..", gix::object::tree::EntryKind::Blob, blob),
        (".git", gix::object::tree::EntryKind::Blob, blob),
    ] {
        let new = raw
            .write_object(&gix::objs::Tree {
                entries: vec![gix::objs::tree::Entry {
                    mode: kind.into(),
                    filename: name.into(),
                    oid: id,
                }],
            })
            .unwrap()
            .detach();
        assert!(
            GitRepository::discover(dir.path())
                .unwrap()
                .check_update_topology(old, new, &no())
                .is_err(),
            "{name}"
        );
    }
    assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"uno\n");
}
#[test]
fn precancel_e_tree_non_valido_non_diventano_ok() {
    let (dir, old, new) = pair(true);
    let repo = GitRepository::discover(dir.path()).unwrap();
    assert!(matches!(
        repo.check_update_topology(old, new, &AtomicBool::new(true)),
        Err(GitError::Cancelled)
    ));
    let commit =
        gix::ObjectId::from_hex(common::git(dir.path(), &["rev-parse", "HEAD"]).as_bytes())
            .unwrap();
    assert!(repo.check_update_topology(old, commit, &no()).is_err());
    assert_eq!(fs::read(dir.path().join("slot")).unwrap(), b"base\n");
}
#[test]
fn diagnostica_topologia_non_autorizza_dirty_o_stage_ne_persistenza() {
    let (dir, old, new) = pair(true);
    fs::write(dir.path().join("slot"), b"dirty\n").unwrap();
    common::git(dir.path(), &["add", "--", "slot"]);
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    GitRepository::discover(dir.path())
        .unwrap()
        .check_update_topology(old, new, &no())
        .unwrap();
    assert_eq!(fs::read(dir.path().join("slot")).unwrap(), b"dirty\n");
    assert_eq!(fs::read(dir.path().join(".git/index")).unwrap(), index);
}
