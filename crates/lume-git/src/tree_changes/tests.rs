//! ODB, tree-diff, refs/index/worktree reali; nessun modello del protocollo.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use gix::object::tree::EntryKind;
use std::{fs, path::Path, sync::atomic::AtomicBool};
fn fixture() -> (tempfile::TempDir, GitRepository) {
    let dir = tempfile::tempdir().unwrap();
    gix::init(dir.path()).unwrap();
    let repo = GitRepository::discover(dir.path()).unwrap();
    fs::write(
        dir.path().join("utente.sql"),
        b"modifiche staged che non sono nel delta\n",
    )
    .unwrap();
    repo.stage([Path::new("utente.sql")]).unwrap();
    (dir, repo)
}
fn tree(repo: &GitRepository, entries: &[(&[u8], EntryKind, &[u8])]) -> gix::ObjectId {
    let mut editor = repo.repository.empty_tree().edit().unwrap();
    for (path, kind, bytes) in entries {
        let id = repo
            .repository
            .write_object(gix::objs::BlobRef { data: bytes })
            .unwrap()
            .detach();
        editor
            .upsert(gix::bstr::BStr::new(path), *kind, id)
            .unwrap();
    }
    editor.write().unwrap().detach()
}
fn oid(repo: &GitRepository, tree: gix::ObjectId, path: &str) -> gix::ObjectId {
    repo.repository
        .find_tree(tree)
        .unwrap()
        .lookup_entry_by_path(path)
        .unwrap()
        .unwrap()
        .object_id()
}
fn path_vec(changes: &[TreeChange]) -> Vec<PathBuf> {
    changes.iter().map(|c| c.path().to_owned()).collect()
}
fn commit(repo: &GitRepository, tree: gix::ObjectId) -> gix::ObjectId {
    let sig =
        gix::actor::SignatureRef::from_bytes(b"Lume Test <lume@example.test> 1700000000 +0000")
            .unwrap();
    repo.repository
        .commit_as(sig, sig, "HEAD", "fixture", tree, [] as [gix::ObjectId; 0])
        .unwrap()
        .detach()
}
#[test]
fn file_che_diventa_directory_rimuove_leaf_vecchia_e_aggiunge_discendenti() {
    let (_dir, repo) = fixture();
    let old = tree(
        &repo,
        &[
            (b"foo", EntryKind::Blob, b"prima"),
            (b"intatto", EntryKind::Blob, b"stesso"),
        ],
    );
    let new = tree(
        &repo,
        &[
            (b"foo/sub/file.sql", EntryKind::Blob, b"dopo"),
            (b"intatto", EntryKind::Blob, b"stesso"),
        ],
    );
    let changes = repo.tree_changes(old, new, false, None).unwrap();
    assert_eq!(
        path_vec(&changes),
        [PathBuf::from("foo"), PathBuf::from("foo/sub/file.sql")]
    );
    assert!(changes.iter().any(|c|matches!(c,TreeChange::Remove{path,before} if path==Path::new("foo")&&before.id==oid(&repo,old,"foo")&&!before.mode.is_tree())));
    assert!(changes.iter().any(|c|matches!(c,TreeChange::Put{path,before:None,after} if path==Path::new("foo/sub/file.sql")&&after.id==oid(&repo,new,"foo/sub/file.sql")&&!after.mode.is_tree())));
}
#[test]
fn directory_che_diventa_file_non_usa_oid_tree_come_blob_prima() {
    let (_dir, repo) = fixture();
    let old = tree(&repo, &[(b"foo/sub/file.sql", EntryKind::Blob, b"prima")]);
    let new = tree(&repo, &[(b"foo", EntryKind::Blob, b"dopo")]);
    let changes = repo.tree_changes(old, new, false, None).unwrap();
    assert!(changes.iter().any(|c|matches!(c,TreeChange::Put{path,before:None,after} if path==Path::new("foo")&&after.id==oid(&repo,new,"foo"))));
    assert!(changes.iter().any(|c|matches!(c,TreeChange::Remove{path,before} if path==Path::new("foo/sub/file.sql")&&before.id==oid(&repo,old,"foo/sub/file.sql"))));
    assert_eq!(changes.len(), 2);
}
#[test]
fn lista_pubblica_commit_radice_mostra_leaf_non_directory() {
    let (_dir, repo) = fixture();
    let id = commit(
        &repo,
        tree(
            &repo,
            &[
                (b"dir/sub/query.sql", EntryKind::Blob, b"select 1"),
                (b"dir/script.sh", EntryKind::BlobExecutable, b"#!/bin/sh"),
            ],
        ),
    );
    let files = repo.commit_files(&id.to_string()).unwrap();
    assert_eq!(
        files.len(),
        2,
        "directory non sono file apribili nella lista del commit: {files:?}"
    );
    assert!(files.iter().all(|file| file.kind == ChangeKind::Added));
}
#[test]
fn oid_mode_prima_dopo_e_delta_senza_mutazioni_di_index_refs_o_worktree() {
    let (dir, repo) = fixture();
    let old = tree(
        &repo,
        &[
            (b"a.sql", EntryKind::Blob, b"prima"),
            (b"remove.sql", EntryKind::Blob, b"rimossa"),
            (b"run.sh", EntryKind::Blob, b"#!/bin/sh"),
        ],
    );
    let new = tree(
        &repo,
        &[
            (b"a.sql", EntryKind::Blob, b"dopo"),
            (b"add.sql", EntryKind::Blob, b"nuova"),
            (b"run.sh", EntryKind::BlobExecutable, b"#!/bin/sh"),
        ],
    );
    let head = commit(&repo, old);
    let index = fs::read(repo.repository.index_path()).unwrap();
    let head_bytes = fs::read(repo.repository.git_dir().join("HEAD")).unwrap();
    let log = fs::read(repo.repository.git_dir().join("logs/HEAD")).unwrap();
    let dirty = fs::read(dir.path().join("utente.sql")).unwrap();
    let changes = repo.tree_changes(old, new, false, None).unwrap();
    assert_eq!(changes.len(), 4);
    assert!(changes.iter().any(|c|matches!(c,TreeChange::Put{path,before:Some(before),after} if path==Path::new("a.sql")&&before.id==oid(&repo,old,"a.sql")&&after.id==oid(&repo,new,"a.sql")&&before.id!=after.id)));
    assert!(changes.iter().any(|c|matches!(c,TreeChange::Put{path,before:Some(before),after} if path==Path::new("run.sh")&&before.id==after.id&&before.mode.kind()==EntryKind::Blob&&after.mode.kind()==EntryKind::BlobExecutable)));
    assert_eq!(repo.repository.head_id().unwrap().detach(), head);
    assert_eq!(fs::read(repo.repository.index_path()).unwrap(), index);
    assert_eq!(
        fs::read(repo.repository.git_dir().join("HEAD")).unwrap(),
        head_bytes
    );
    assert_eq!(
        fs::read(repo.repository.git_dir().join("logs/HEAD")).unwrap(),
        log
    );
    assert_eq!(fs::read(dir.path().join("utente.sql")).unwrap(), dirty);
}
#[test]
fn storia_mantiene_rename_ma_delta_integrazione_esplicita_remove_put() {
    let (_dir, repo) = fixture();
    let old = tree(
        &repo,
        &[(b"prima.sql", EntryKind::Blob, b"contenuto identico")],
    );
    let new = tree(
        &repo,
        &[(b"dopo.sql", EntryKind::Blob, b"contenuto identico")],
    );
    let history = repo.tree_changes(old, new, true, None).unwrap();
    assert_eq!(history.len(), 1);
    assert!(
        matches!(history.first(),Some(TreeChange::Rewrite{source,path,before,after,copy:false}) if source==Path::new("prima.sql")&&path==Path::new("dopo.sql")&&before.id==after.id)
    );
    assert_eq!(
        history.into_iter().next().unwrap().into_file().kind,
        ChangeKind::Renamed
    );
    let base = commit(&repo, old);
    let sig =
        gix::actor::SignatureRef::from_bytes(b"Lume Test <lume@example.test> 1700000000 +0000")
            .unwrap();
    let renamed = repo
        .repository
        .commit_as(sig, sig, "HEAD", "rinomina", new, [base])
        .unwrap()
        .detach();
    let files = repo.commit_files(&renamed.to_string()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files.first().unwrap().kind, ChangeKind::Renamed);
    assert_eq!(files.first().unwrap().path, Path::new("dopo.sql"));
    let delta = repo.tree_changes(old, new, false, None).unwrap();
    assert_eq!(delta.len(), 2);
    assert!(
        !delta
            .iter()
            .any(|c| matches!(c, TreeChange::Rewrite { .. }))
    );
}
#[test]
fn preannullamento_anche_alberi_identici_non_diventa_delta_vuoto_riuscito() {
    let (_dir, repo) = fixture();
    let id = tree(&repo, &[(b"file.sql", EntryKind::Blob, b"select 1")]);
    assert!(
        matches!(
            repo.tree_changes(id, id, false, Some(&AtomicBool::new(true))),
            Err(GitError::Cancelled)
        ),
        "query cancellata non equivale a piano senza cambiamenti"
    );
}
#[cfg(unix)]
#[test]
fn nomi_raw_distinti_e_backslash_letterale_restano_identita_native() {
    use std::os::unix::ffi::OsStringExt;
    let (_dir, repo) = fixture();
    let old = repo.repository.empty_tree().id;
    let new = tree(
        &repo,
        &[
            (b"nome-\xff.sql", EntryKind::Blob, b"uno"),
            (b"nome-\xfe.sql", EntryKind::Blob, b"due"),
            (b"dir\\query.sql", EntryKind::Blob, b"tre"),
        ],
    );
    let changes = repo.tree_changes(old, new, false, None).unwrap();
    assert_eq!(changes.len(), 3);
    for name in [
        b"nome-\xff.sql".as_slice(),
        b"nome-\xfe.sql",
        b"dir\\query.sql",
    ] {
        let path = PathBuf::from(std::ffi::OsString::from_vec(name.to_vec()));
        assert!(changes.iter().any(|c| c.path() == path));
    }
}
#[test]
fn delta_senza_rewrites_non_legge_blob_assenti_per_cercare_rinomine() {
    let (_dir, repo) = fixture();
    let a = gix::ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
    let b = gix::ObjectId::from_hex(b"2222222222222222222222222222222222222222").unwrap();
    assert!(repo.repository.find_object(a).is_err());
    assert!(repo.repository.find_object(b).is_err());
    // TreeEditor verifica l'esistenza dei blob. Per una ODB parziale vera
    // serializzare invece i tree nativi che referenziano gli oid assenti.
    let write_tree = |name: &str, oid| {
        repo.repository
            .write_object(&gix::objs::Tree {
                entries: vec![gix::objs::tree::Entry {
                    mode: EntryKind::Blob.into(),
                    filename: name.into(),
                    oid,
                }],
            })
            .unwrap()
            .detach()
    };
    let old = write_tree("prima.sql", a);
    let new = write_tree("dopo.sql", b);
    let delta = repo.tree_changes(old, new, false, None).unwrap();
    assert!(
        delta
            .iter()
            .any(|c| matches!(c, TreeChange::Remove{before,..} if before.id == a))
    );
    assert!(
        delta
            .iter()
            .any(|c| matches!(c, TreeChange::Put{after,..} if after.id == b))
    );
    assert_eq!(delta.len(), 2);
}
#[test]
fn link_e_gitlink_mantengono_tipo_e_oid_non_diventano_file_regolari() {
    let (_dir, repo) = fixture();
    let empty = repo.repository.empty_tree().id;
    let sub = commit(&repo, empty);
    let link = repo
        .repository
        .write_object(gix::objs::BlobRef {
            data: b"destinazione",
        })
        .unwrap()
        .detach();
    let mut new = repo.repository.empty_tree().edit().unwrap();
    new.upsert("link", EntryKind::Link, link).unwrap();
    new.upsert("sub", EntryKind::Commit, sub).unwrap();
    let new = new.write().unwrap().detach();
    let delta = repo.tree_changes(empty, new, false, None).unwrap();
    assert!(delta.iter().any(|c| matches!(c, TreeChange::Put{path,after,..} if path==Path::new("link") && after.id==link && after.mode.kind()==EntryKind::Link)));
    assert!(delta.iter().any(|c| matches!(c, TreeChange::Put{path,after,..} if path==Path::new("sub") && after.id==sub && after.mode.kind()==EntryKind::Commit)));
    assert_eq!(delta.len(), 2);
}

#[test]
fn oid_non_tree_o_assente_produce_errore_non_piano_vuoto() {
    let (_dir, repo) = fixture();
    let old = repo.repository.empty_tree().id;
    let blob = repo
        .repository
        .write_object(gix::objs::BlobRef { data: b"non tree" })
        .unwrap()
        .detach();
    assert!(repo.tree_changes(old, blob, false, None).is_err());
    let missing = gix::ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap();
    assert!(repo.tree_changes(old, missing, false, None).is_err());
}
