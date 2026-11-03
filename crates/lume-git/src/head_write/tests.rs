//! Snapshot e transazioni su object/ref store reali, non modello sostitutivo.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use gix::bstr::ByteSlice;
fn fixture() -> (
    tempfile::TempDir,
    gix::Repository,
    gix::ObjectId,
    gix::ObjectId,
) {
    let dir = tempfile::tempdir().unwrap();
    let repo = gix::init(dir.path()).unwrap();
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(repo.git_dir().join("config"))
        .unwrap()
        .write_all(b"\n[user]\n name = Lume Test\n email = lume@example.test\n")
        .unwrap();
    drop(repo);
    let repo = gix::open(dir.path()).unwrap();
    let tree = repo
        .write_object(gix::objs::Tree::empty())
        .unwrap()
        .detach();
    let sig =
        gix::actor::SignatureRef::from_bytes(b"Lume Test <lume@example.test> 1700000000 +0000")
            .unwrap();
    let first = repo
        .commit_as(sig, sig, "HEAD", "base", tree, [] as [gix::ObjectId; 0])
        .unwrap()
        .detach();
    let object = gix::objs::Commit {
        message: "candidato".into(),
        tree,
        author: sig.into(),
        committer: sig.into(),
        encoding: None,
        parents: [first].into_iter().collect(),
        extra_headers: Vec::new(),
    };
    let next = repo.write_object(&object).unwrap().detach();
    (dir, repo, first, next)
}
fn symref(repo: &gix::Repository, name: &str, target: &str) {
    repo.edit_reference(RefEdit {
        name: name.try_into().unwrap(),
        deref: false,
        change: Change::Update {
            log: LogChange {
                mode: RefLog::AndReference,
                force_create_reflog: false,
                message: "fixture".into(),
            },
            expected: PreviousValue::Any,
            new: Target::Symbolic(target.try_into().unwrap()),
        },
    })
    .unwrap();
}
fn oid(repo: &gix::Repository, name: &str) -> gix::ObjectId {
    repo.find_reference(name)
        .unwrap()
        .into_fully_peeled_id()
        .unwrap()
        .detach()
}
#[test]
fn altro_branch_con_stesso_oid_non_accetta_avanzamento_del_piano_precedente() {
    let (_dir, repo, first, next) = fixture();
    let snapshot = HeadSnapshot::capture(&repo).unwrap();
    let original = repo.head_name().unwrap().unwrap();
    repo.reference("refs/heads/secondo", first, PreviousValue::Any, "fixture")
        .unwrap();
    symref(&repo, "HEAD", "refs/heads/secondo");
    assert!(
        snapshot.advance(&repo, next, "commit: candidato").is_err(),
        "OID uguale non significa ancora sullo stesso branch"
    );
    assert_eq!(oid(&repo, original.as_bstr().to_str().unwrap()), first);
    assert_eq!(oid(&repo, "refs/heads/secondo"), first);
}
#[test]
fn alias_intermedio_cambiato_stesso_oid_non_riapplica_payload_su_altro_branch() {
    let (_dir, repo, first, next) = fixture();
    let original = repo.head_name().unwrap().unwrap();
    repo.reference("refs/heads/secondo", first, PreviousValue::Any, "fixture")
        .unwrap();
    symref(
        &repo,
        "refs/heads/alias",
        original.as_bstr().to_str().unwrap(),
    );
    symref(&repo, "HEAD", "refs/heads/alias");
    let snapshot = HeadSnapshot::capture(&repo).unwrap();
    symref(&repo, "refs/heads/alias", "refs/heads/secondo");
    assert!(
        snapshot.advance(&repo, next, "commit: candidato").is_err(),
        "ogni collegamento simbolico deve ancora corrispondere al piano"
    );
    assert_eq!(oid(&repo, "refs/heads/secondo"), first);
}
#[test]
fn prepare_possiede_lock_head_e_branch_drop_libera_senza_pubblicare() {
    let (_dir, repo, first, next) = fixture();
    let snapshot = HeadSnapshot::capture(&repo).unwrap();
    let branch = repo.head_name().unwrap().unwrap();
    let old_log = std::fs::read(repo.git_dir().join("logs/HEAD")).unwrap();
    let transaction = snapshot.prepare(&repo, next, "commit: candidato").unwrap();
    assert!(repo.git_dir().join("HEAD.lock").exists());
    let change = repo.edit_reference(RefEdit {
        name: "HEAD".try_into().unwrap(),
        deref: false,
        change: Change::Update {
            log: LogChange {
                mode: RefLog::AndReference,
                force_create_reflog: false,
                message: "fixture".into(),
            },
            expected: PreviousValue::Any,
            new: Target::Object(first),
        },
    });
    assert!(
        change.is_err(),
        "Git cooperativo non può cambiare HEAD sotto prepare"
    );
    assert!(
        repo.reference(branch.clone(), next, PreviousValue::Any, "fixture")
            .is_err(),
        "anche il leaf è bloccato"
    );
    drop(transaction);
    assert!(!repo.git_dir().join("HEAD.lock").exists());
    assert_eq!(oid(&repo, "HEAD"), first);
    assert_eq!(
        std::fs::read(repo.git_dir().join("logs/HEAD")).unwrap(),
        old_log
    );
    snapshot.advance(&repo, next, "commit: candidato").unwrap();
    assert_eq!(oid(&repo, "HEAD"), next);
    let log = std::fs::read_to_string(repo.git_dir().join("logs/HEAD")).unwrap();
    let last = log.lines().last().unwrap();
    assert!(last.starts_with(&format!("{first} {next} ")));
    assert!(last.ends_with("\tcommit: candidato"));
}
#[test]
fn prepare_senza_cambio_oid_mantiene_leaf_e_non_cancella_ref_o_reflog() {
    for mode in 0..3 {
        let (_dir, repo, first, next) = fixture();
        let branch = repo.head_name().unwrap().unwrap();
        if mode == 1 {
            symref(
                &repo,
                "refs/heads/alias",
                branch.as_bstr().to_str().unwrap(),
            );
            symref(&repo, "HEAD", "refs/heads/alias");
        } else if mode == 2 {
            repo.reference("HEAD", first, PreviousValue::Any, "fixture")
                .unwrap();
        }
        let snapshot = HeadSnapshot::capture(&repo).unwrap();
        let head_target = repo.find_reference("HEAD").unwrap().target().into_owned();
        let head_log = std::fs::read(repo.git_dir().join("logs/HEAD")).unwrap();
        let leaf_log = std::fs::read(
            repo.git_dir()
                .join("logs")
                .join(branch.as_bstr().to_str().unwrap()),
        )
        .unwrap();
        let prepared = snapshot
            .prepare(&repo, first, "amend senza effetti")
            .unwrap();
        let leaf: FullName = if mode == 2 {
            "HEAD".try_into().unwrap()
        } else {
            branch.clone()
        };
        assert!(
            repo.reference(leaf, next, PreviousValue::Any, "fixture")
                .is_err(),
            "anche lo stesso OID deve mantenere il leaf sotto lease: modo{mode}"
        );
        if mode == 1 {
            assert!(
                repo.reference("refs/heads/alias", next, PreviousValue::Any, "fixture")
                    .is_err(),
                "marker posseduto anche per alias"
            );
        }
        drop(prepared);
        snapshot
            .advance(&repo, first, "amend senza effetti")
            .unwrap();
        assert_eq!(oid(&repo, "HEAD"), first);
        assert_eq!(
            repo.find_reference("HEAD").unwrap().target().into_owned(),
            head_target
        );
        assert_eq!(
            std::fs::read(repo.git_dir().join("logs/HEAD")).unwrap(),
            head_log
        );
        assert_eq!(
            std::fs::read(
                repo.git_dir()
                    .join("logs")
                    .join(branch.as_bstr().to_str().unwrap())
            )
            .unwrap(),
            leaf_log
        );
    }
}

#[test]
fn detached_head_aggiornato_senza_scegliere_o_creare_un_branch() {
    let (_dir, repo, first, next) = fixture();
    let branch = repo.head_name().unwrap().unwrap();
    repo.edit_reference(RefEdit {
        name: "HEAD".try_into().unwrap(),
        deref: false,
        change: Change::Update {
            log: LogChange {
                mode: RefLog::AndReference,
                force_create_reflog: false,
                message: "fixture".into(),
            },
            expected: PreviousValue::Any,
            new: Target::Object(first),
        },
    })
    .unwrap();
    let snapshot = HeadSnapshot::capture(&repo).unwrap();
    snapshot.advance(&repo, next, "commit: detached").unwrap();
    assert!(repo.head().unwrap().is_detached());
    assert_eq!(oid(&repo, "HEAD"), next);
    assert_eq!(oid(&repo, branch.as_bstr().to_str().unwrap()), first);
}

#[test]
fn unborn_creazione_vera_reflog_e_stesso_snapshot_non_riapplica_dopo_commit() {
    let (_source, source, _first, next) = fixture();
    let target = tempfile::tempdir().unwrap();
    let repo = gix::init(target.path()).unwrap();
    // Oggetto/identità creati nel repo reale, non oid inesistente dal source.
    let bytes = source.find_object(next).unwrap().data.clone();
    let object = gix::objs::CommitRef::from_bytes(&bytes, source.object_hash()).unwrap();
    let tree = repo
        .write_object(gix::objs::Tree::empty())
        .unwrap()
        .detach();
    let commit = gix::objs::Commit {
        tree,
        parents: Default::default(),
        author: object.author().unwrap().into(),
        committer: object.committer().unwrap().into(),
        message: "iniziale".into(),
        encoding: None,
        extra_headers: Vec::new(),
    };
    let id = repo.write_object(&commit).unwrap().detach();
    let snapshot = HeadSnapshot::capture(&repo).unwrap();
    assert!(snapshot.id.is_none());
    // Configurazione d'identità solo della fixture; riapertura del cache config.
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(repo.git_dir().join("config"))
        .unwrap()
        .write_all(b"\n[user]\n name = Lume Test\n email = lume@example.test\n")
        .unwrap();
    drop(repo);
    let repo = gix::open(target.path()).unwrap();
    snapshot
        .advance(&repo, id, "commit (initial): iniziale")
        .unwrap();
    assert_eq!(oid(&repo, "HEAD"), id);
    let log = std::fs::read(repo.git_dir().join("logs/HEAD")).unwrap();
    assert!(
        snapshot
            .advance(&repo, id, "commit (initial): iniziale")
            .is_err(),
        "lo stesso piano unborn non deve essere riapplicato al branch ora presente"
    );
    assert_eq!(
        std::fs::read(repo.git_dir().join("logs/HEAD")).unwrap(),
        log
    );
}

#[test]
fn oid_diverso_sullo_stesso_branch_non_viene_sovrascritto() {
    let (_dir, repo, _first, next) = fixture();
    let snapshot = HeadSnapshot::capture(&repo).unwrap();
    let branch = repo.head_name().unwrap().unwrap();
    repo.reference(branch, next, PreviousValue::Any, "fixture")
        .unwrap();
    assert!(snapshot.advance(&repo, next, "commit: candidato").is_err());
    assert_eq!(oid(&repo, "HEAD"), next);
}
