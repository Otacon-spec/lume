//! Alias SaveAs: diagnostica worker prima di prepare/commit. Path/identità
//! filesystem non sono permesso di pubblicazione né confinement anti-TOCTOU.
use lume_core::DocId;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Consuma sempre il handle sul worker: precommit rifiutato → attempt abort;
/// pass → commit. Non è handshake UI né congelamento di path/antenati/byte.
#[cfg(all(test, unix))]
pub(super) fn publish(
    prepared: lume_core::file_write::PreparedWrite,
    selected: &Path,
    document: DocId,
    opened: &[(DocId, PathBuf)],
    resolved: &Path,
) -> Result<(), String> {
    publish_authorized(prepared, selected, document, opened, resolved, || true).map(drop)
}

/// Il confine consumante segue i controlli finali: Granted resta revocabile
/// durante la diagnostica. Il callback non gira sotto mutex o sul thread UI.
pub(super) fn publish_authorized(
    prepared: lume_core::file_write::PreparedWrite,
    selected: &Path,
    document: DocId,
    opened: &[(DocId, PathBuf)],
    resolved: &Path,
    begin_commit: impl FnOnce() -> bool,
) -> Result<Option<String>, String> {
    let checked = if prepared.destination() != resolved {
        Err("preparazione di un'altra destinazione: commit rifiutato".into())
    } else {
        destination(selected, document, opened).and_then(|current| {
            if current == resolved {
                Ok(())
            } else {
                Err("la destinazione scelta è cambiata dopo la preparazione".into())
            }
        })
    };
    let checked = checked.and_then(|()| {
        if begin_commit() {
            Ok(())
        } else {
            Err("autorità Salva con nome revocata prima del commit".into())
        }
    });
    if let Err(error) = checked {
        // Abort riferisce il solo unlink, non close/fsync/stdjoin. Anche
        // quando fallisce non si riprova né si perde il risultato diagnostico.
        return match prepared.abort() {
            Ok(()) => Err(format!(
                "{error}; unlink del temporaneo eseguito, commit non chiamato"
            )),
            Err(cleanup) => Err(format!(
                "{error}; cleanup temporaneo fallito: {cleanup}; residui da verificare, nessun commit chiamato"
            )),
        };
    }
    super::commit_with_warning(prepared, selected)
}

#[cfg(all(test, unix))]
mod publish_tests;

fn resolved(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // Non seguire un symlink pendente facendolo diventare nuovo file.
            if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "link senza destinazione: scegli un altro nome",
                ));
            }
            let absolute = std::path::absolute(path)?;
            let parent = absolute.parent().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "il percorso non ha una cartella",
                )
            })?;
            let name = absolute.file_name().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "il percorso non ha un nome file",
                )
            })?;
            Ok(fs::canonicalize(parent)?.join(name))
        }
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
pub(crate) fn identity(path: &Path) -> io::Result<Option<(u64, u64)>> {
    use std::os::unix::fs::MetadataExt;
    match fs::metadata(path) {
        Ok(m) => Ok(Some((m.dev(), m.ino()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
#[cfg(windows)]
pub(crate) fn identity(path: &Path) -> io::Result<Option<(u64, u64)>> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    // SAFETY: `zeroed` inizializza una struttura C scrivibile che l’API riempie prima della lettura.
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // FD vivo e buffer valido; solo identità native, nessuna scrittura/rename.
    // SAFETY: l’handle `file` resta aperto e `information` è un output valido e scrivibile per tutta la chiamata.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Some((
        u64::from(information.dwVolumeSerialNumber),
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    )))
}
#[cfg(not(any(unix, windows)))]
pub(crate) fn identity(_path: &Path) -> io::Result<Option<(u64, u64)>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "identità native SaveAs non disponibili su questa piattaforma",
    ))
}

#[cfg(windows)]
fn same_path(a: &Path, b: &Path) -> io::Result<bool> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
    let a: Vec<u16> = a.as_os_str().encode_wide().collect();
    let b: Vec<u16> = b.as_os_str().encode_wide().collect();
    let len = |n: usize| {
        i32::try_from(n)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "percorso troppo lungo"))
    };
    // Lunghezze esatte UTF16 native (anche surrogati non paired), niente lossy.
    // SAFETY: le due slice UTF-16 restano vive e i conteggi convertiti a `i32` descrivono le lunghezze effettive.
    let result =
        unsafe { CompareStringOrdinal(a.as_ptr(), len(a.len())?, b.as_ptr(), len(b.len())?, 1) };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(result == CSTR_EQUAL)
}
#[cfg(not(windows))]
fn same_path(a: &Path, b: &Path) -> io::Result<bool> {
    Ok(a == b)
}

pub(super) fn destination(
    path: &Path,
    document: DocId,
    opened: &[(DocId, PathBuf)],
) -> Result<PathBuf, String> {
    let destination =
        resolved(path).map_err(|e| format!("Salvataggio di {} fallito: {e}", path.display()))?;
    let key = identity(&destination)
        .map_err(|e| format!("Identità della destinazione non verificabile: {e}"))?;
    for (owner, path) in opened.iter().filter(|(id, _)| *id != document) {
        let other = resolved(path)
            .map_err(|e| format!("Alias di {} non verificabile: {e}", path.display()))?;
        let other_key = identity(&other)
            .map_err(|e| format!("Identità del file aperto non verificabile: {e}"))?;
        let same_name = same_path(&destination, &other)
            .map_err(|e| format!("Confronto dei nomi nativi non verificabile: {e}"))?;
        if same_name || key.is_some() && key == other_key {
            return Err(format!(
                "{} è già aperto in un'altra scheda ({owner:?}): scegli un altro nome",
                path.display()
            ));
        }
    }
    Ok(destination)
}
