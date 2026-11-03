//! Nome già risolto: non trasformare un retarget del leaf in permesso di
//! scrivere altrove. Fixture native locali, non prova di confinement.
#[cfg(unix)]
use lume_core::file_write::prepare_atomic;
use lume_core::file_write::prepare_atomic_resolved;
use std::{io, path::Path};
fn entries(path: &Path) -> io::Result<usize> {
    Ok(std::fs::read_dir(path)?.count())
}
fn error<T>(result: io::Result<T>) -> io::Error {
    match result {
        Err(e) => e,
        Ok(_) => panic!("errore atteso, preparazione erroneamente ammessa"),
    }
}

#[test]
fn nuovo_nome_assoluto_prepare_abort_e_commit_reali() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("nuovo.txt");
    let prepared = prepare_atomic_resolved(&path, b"nuovo")?;
    assert_eq!(prepared.destination(), path);
    assert!(!path.exists());
    assert_eq!(entries(dir.path())?, 1);
    prepared.abort()?;
    assert_eq!(entries(dir.path())?, 0);
    prepare_atomic_resolved(&path, b"nuovo")?.commit()?;
    assert_eq!(std::fs::read(&path)?, b"nuovo");
    assert_eq!(entries(dir.path())?, 1);
    Ok(())
}
#[test]
fn nome_relativo_rifiutato_prima_di_preparare_o_scrivere() {
    let err = error(prepare_atomic_resolved(Path::new("."), b"no"));
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    assert!(err.to_string().contains("assoluto"));
}

#[cfg(unix)]
#[test]
fn symlink_comparso_sul_nome_risolto_non_e_riattraversato() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("selezionato.txt");
    let other = dir.path().join("altro.txt");
    std::fs::write(&path, b"origine")?;
    std::fs::write(&other, b"conservare")?;
    let resolved = std::fs::canonicalize(&path)?;
    std::fs::remove_file(&path)?;
    std::os::unix::fs::symlink(&other, &path)?;
    let result = prepare_atomic_resolved(&resolved, b"NO");
    // Consuma anche l'eventuale preparazione erronea. Il semplice adapter
    // storico segue il link e modifica davvero l'altro file.
    let status = match result {
        Ok(p) => p.commit(),
        Err(e) => Err(e),
    };
    assert_eq!(std::fs::read(&other)?, b"conservare");
    assert!(status.is_err());
    assert!(std::fs::symlink_metadata(&path)?.file_type().is_symlink());
    assert_eq!(entries(dir.path())?, 2);
    Ok(())
}
#[cfg(unix)]
#[test]
fn save_ordinario_conserva_il_link_manuale_ma_resolved_non_lo_segue() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("link.txt");
    let other = dir.path().join("altro.txt");
    std::fs::write(&other, b"inizio")?;
    std::os::unix::fs::symlink(&other, &path)?;
    prepare_atomic(&path, b"ordinario")?.commit()?;
    assert_eq!(std::fs::read(&other)?, b"ordinario");
    assert!(std::fs::symlink_metadata(&path)?.file_type().is_symlink());
    let result = prepare_atomic_resolved(&path, b"NO");
    let status = match result {
        Ok(p) => p.commit(),
        Err(e) => Err(e),
    };
    assert_eq!(std::fs::read(&other)?, b"ordinario");
    assert!(status.is_err());
    assert_eq!(entries(dir.path())?, 2);
    Ok(())
}
#[cfg(unix)]
#[test]
fn readonly_errore_preserva_originale_e_non_lascia_temporanei() -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("protetto.txt");
    std::fs::write(&path, b"conservare")?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444))?;
    let err = error(prepare_atomic_resolved(&path, b"NO"));
    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(std::fs::read(&path)?, b"conservare");
    assert_eq!(entries(dir.path())?, 1);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}
