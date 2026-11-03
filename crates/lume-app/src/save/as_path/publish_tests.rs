//! Punto reale worker dopo prepare: mutate entry native, nessun mock/hook.
use super::*;
use lume_core::file_write::prepare_atomic_resolved;
struct Fixture {
    dir: tempfile::TempDir,
    dest: PathBuf,
    other: PathBuf,
    selected: PathBuf,
    ws: lume_core::Workspace,
    id: DocId,
}
impl Fixture {
    fn new() -> io::Result<Self> {
        let dir = tempfile::tempdir()?;
        let dest = dir.path().join("target.txt");
        let other = dir.path().join("altro.txt");
        let selected = dir.path().join("link.txt");
        fs::write(&dest, b"origine")?;
        fs::write(&other, b"conservare")?;
        std::os::unix::fs::symlink(&dest, &selected)?;
        let mut ws = lume_core::Workspace::new();
        let id = ws.open(None, "NO");
        Ok(Self {
            dir,
            dest,
            other,
            selected,
            ws,
            id,
        })
    }
    fn retarget(&self) -> io::Result<()> {
        fs::remove_file(&self.selected)?;
        std::os::unix::fs::symlink(&self.other, &self.selected)
    }
    fn unchanged(&self) -> io::Result<()> {
        assert_eq!(fs::read(&self.dest)?, b"origine");
        assert_eq!(fs::read(&self.other)?, b"conservare");
        assert!(self.ws.get(self.id).is_some());
        Ok(())
    }
    fn count(&self) -> io::Result<usize> {
        Ok(fs::read_dir(self.dir.path())?.count())
    }
}
fn failure<T>(result: Result<T, String>) -> String {
    match result {
        Err(e) => e,
        Ok(_) => panic!("commit erroneamente ammesso"),
    }
}
#[test]
fn alias_divenuto_di_altro_buffer_dopo_prepare_fa_abort_non_commit() -> io::Result<()> {
    let mut f = Fixture::new()?;
    let owner = f.ws.open(Some(f.other.clone()), "conservare");
    let prepared = prepare_atomic_resolved(&f.dest, b"NO")?;
    assert_eq!(f.count()?, 4);
    f.retarget()?;
    let error = failure(publish(
        prepared,
        &f.selected,
        f.id,
        &[(owner, f.other.clone())],
        &f.dest,
    ));
    assert!(error.contains("già aperto"));
    assert!(error.contains("unlink del temporaneo eseguito"));
    assert!(error.contains("commit non chiamato"));
    f.unchanged()?;
    assert_eq!(f.count()?, 3);
    Ok(())
}
#[test]
fn nome_risolto_cambiato_dopo_prepare_anche_senza_buffer_fa_abort() -> io::Result<()> {
    let f = Fixture::new()?;
    let prepared = prepare_atomic_resolved(&f.dest, b"NO")?;
    f.retarget()?;
    let error = failure(publish(prepared, &f.selected, f.id, &[], &f.dest));
    assert!(error.contains("cambiata dopo"));
    assert!(error.contains("commit non chiamato"));
    f.unchanged()?;
    assert_eq!(f.count()?, 3);
    Ok(())
}
#[test]
fn cleanup_fallito_reale_preserva_diagnosi_e_non_riprova_commit() -> io::Result<()> {
    let f = Fixture::new()?;
    let prepared = prepare_atomic_resolved(&f.dest, b"NO")?;
    // Rimuovere il SOLO temporaneo della fixture: errore unlink reale su
    // abort, non filesystem finto né simulazione di altre fasi di cleanup.
    let mut temp = None;
    for entry in fs::read_dir(f.dir.path())? {
        let path = entry?.path();
        if path != f.dest && path != f.other && path != f.selected {
            assert!(temp.is_none());
            temp = Some(path);
        }
    }
    let temp = temp.ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "temporaneo della fixture assente")
    })?;
    fs::remove_file(temp)?;
    f.retarget()?;
    let error = failure(publish(prepared, &f.selected, f.id, &[], &f.dest));
    assert!(error.contains("cambiata dopo"));
    assert!(error.contains("cleanup temporaneo fallito"));
    assert!(error.contains("nessun commit chiamato"));
    f.unchanged()?;
    assert_eq!(f.count()?, 3);
    Ok(())
}
#[test]
fn preparazione_foreign_non_pubblica_sul_nome_che_non_ha_preparato() -> io::Result<()> {
    let f = Fixture::new()?;
    let prepared = prepare_atomic_resolved(&f.other, b"NO")?;
    let error = failure(publish(prepared, &f.selected, f.id, &[], &f.dest));
    assert!(error.contains("un'altra destinazione"));
    f.unchanged()?;
    assert_eq!(f.count()?, 3);
    Ok(())
}
#[test]
fn diagnostica_alias_rifiutata_non_attraversa_il_confine_commit_started() -> io::Result<()> {
    let f = Fixture::new()?;
    let prepared = prepare_atomic_resolved(&f.dest, b"NO")?;
    f.retarget()?;
    let chiamate = std::cell::Cell::new(0);
    let error = failure(publish_authorized(
        prepared,
        &f.selected,
        f.id,
        &[],
        &f.dest,
        || {
            chiamate.set(chiamate.get() + 1);
            true
        },
    ));
    assert_eq!(
        chiamate.get(),
        0,
        "diagnostica prima del confine consumante"
    );
    assert!(error.contains("commit non chiamato"));
    f.unchanged()?;
    assert_eq!(f.count()?, 3);
    Ok(())
}

#[test]
fn revoca_granted_dopo_diagnostica_aborta_il_preparato_reale() -> io::Result<()> {
    let f = Fixture::new()?;
    let prepared = prepare_atomic_resolved(&f.dest, b"NO")?;
    let gate = crate::save::gate::Gate::new();
    let lease = gate
        .ready()
        .ok_or_else(|| io::Error::other("lease assente"))?;
    assert!(lease.grant());
    assert!(gate.wait_permission());
    let error = failure(publish_authorized(
        prepared,
        &f.selected,
        f.id,
        &[],
        &f.dest,
        || {
            gate.deny();
            gate.wait_commit()
        },
    ));
    assert!(error.contains("revocata prima del commit"));
    assert!(error.contains("commit non chiamato"));
    f.unchanged()?;
    assert_eq!(f.count()?, 3);
    Ok(())
}

#[test]
fn revoca_dopo_commit_started_raccoglie_pubblicazione_reale_senza_fingere_rollback()
-> io::Result<()> {
    let f = Fixture::new()?;
    let prepared = prepare_atomic_resolved(&f.dest, b"nuovo")?;
    let gate = crate::save::gate::Gate::new();
    let lease = gate
        .ready()
        .ok_or_else(|| io::Error::other("lease assente"))?;
    assert!(lease.grant());
    assert!(gate.wait_permission());

    let result = publish_authorized(prepared, &f.selected, f.id, &[], &f.dest, || {
        let started = gate.wait_commit();
        assert!(started, "il worker deve attraversare CommitStarted");
        // Simula la revoca/chiusura osservata dopo il confine. È ormai
        // troppo tardi per annullare: il proprietario deve raccogliere.
        lease.deny();
        started
    });

    assert!(result.is_ok(), "il commit iniziato va raccolto: {result:?}");
    assert_eq!(fs::read(&f.dest)?, b"nuovo");
    assert_eq!(fs::read(&f.other)?, b"conservare");
    assert_eq!(f.count()?, 3);
    Ok(())
}

#[test]
fn prepared_handle_corretto_pubblica_e_preserva_symlink_scelto() -> io::Result<()> {
    let f = Fixture::new()?;
    let prepared = prepare_atomic_resolved(&f.dest, b"nuovo")?;
    assert!(publish(prepared, &f.selected, f.id, &[], &f.dest).is_ok());
    assert_eq!(fs::read(&f.dest)?, b"nuovo");
    assert_eq!(fs::read(&f.other)?, b"conservare");
    assert!(fs::symlink_metadata(&f.selected)?.file_type().is_symlink());
    assert_eq!(f.count()?, 3);
    Ok(())
}
