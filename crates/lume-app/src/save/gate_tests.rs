//! Controprove del confine protocollo, non lifecycle/close GUI nativi.
use super::*;

type Esito = Result<(), Box<dyn std::error::Error>>;

#[test]
fn ultima_lease_nega_e_sveglia_il_worker_senza_ricreare_autorita() -> Esito {
    let gate = Gate::new();
    let lease = gate.ready().ok_or("lease preparata assente")?;
    let weak = lease.downgrade();
    let other = lease.clone();
    let waiting = gate.clone();
    let worker = std::thread::spawn(move || waiting.wait_commit());
    drop(lease);
    assert!(weak.upgrade().is_some());
    drop(other);
    assert!(!worker.join().map_err(|_| "worker terminato con panic")?);
    assert!(weak.upgrade().is_none());
    assert!(gate.ready().is_none());
    Ok(())
}

#[test]
fn granted_revocabile_durante_update_non_attraversa_commit_started() -> Esito {
    let gate = Gate::new();
    let pause = gate.pause();
    let lease = gate.ready().ok_or("lease preparata assente")?;
    assert!(lease.grant());
    assert!(!lease.grant(), "decisione consumante");
    let waiting = gate.clone();
    let worker = std::thread::spawn(move || waiting.wait_commit());
    assert_eq!(gate.lock().state, State::Granted);
    pause.finish(false);
    assert!(!worker.join().map_err(|_| "worker terminato con panic")?);
    assert_eq!(gate.lock().state, State::Aborting);
    Ok(())
}

#[test]
fn il_permesso_per_diagnostica_resta_granted_e_revocabile() -> Esito {
    let gate = Gate::new();
    let lease = gate.ready().ok_or("lease preparata assente")?;
    assert!(lease.grant());
    assert!(gate.wait_permission());
    assert_eq!(gate.lock().state, State::Granted);
    gate.deny();
    assert!(!gate.wait_commit());
    assert_eq!(gate.lock().state, State::Aborting);
    Ok(())
}

#[test]
fn commit_started_e_monotono_e_revoca_non_finge_rollback() -> Esito {
    let gate = Gate::new();
    let lease = gate.ready().ok_or("lease preparata assente")?;
    assert!(lease.grant());
    assert!(gate.wait_commit());
    drop(lease);
    gate.deny();
    assert_eq!(gate.lock().state, State::CommitStarted);
    gate.done();
    assert_eq!(gate.lock().state, State::Done);
    Ok(())
}

#[test]
fn pausa_incompleta_nega_e_restituisce_il_confine_al_worker() -> Esito {
    let gate = Gate::new();
    let pause = gate.pause();
    let lease = gate.ready().ok_or("lease preparata assente")?;
    assert!(lease.grant());
    drop(pause);
    assert!(!gate.wait_commit());
    assert_eq!(gate.lock().ui_busy, 0);
    Ok(())
}
