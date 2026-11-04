//! Scrittura di un file senza troncare l'originale prima del successo.
//!
//! Il temporaneo nasce nella stessa directory della destinazione: il commit
//! è una sostituzione sullo stesso filesystem, mai una copia dopo un unlink.
//! Prima del commit si sincronizzano contenuto e permessi del temporaneo.
//! Un errore precedente al commit lascia intatto il file originale.
//!
//! Si conservano i permessi e si segue un link simbolico esistente, senza
//! sostituire il link. Su Windows il temporaneo nasce con DACL protetta,
//! verificata prima di scrivere, e ReplaceFileW conserva la DACL originale.
//! Filesystem senza ACL persistenti o protezioni non riproducibili sono
//! rifiutati, senza fallback a una scrittura meno sicura.
//!
//! La sostituzione cambia l'identità del file: altri hard link restano sul
//! vecchio contenuto. Non si promette di conservare tutti i metadati (ACL su
//! Unix, attributi estesi), né una transazione tra più file o il rilevamento
//! di modifiche concorrenti. ReplaceFileW documenta errori parziali di rename:
//! non si promette il rollback dei nomi in quei casi. Su Unix, dopo il rename
//! si sincronizza anche la directory: un suo errore non annulla il salvataggio
//! già visibile, ma viene restituito come avviso da `commit_reporting`. Su
//! Windows la directory non viene sincronizzata.

use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
use std::path::Path;

mod directory;
pub use directory::{DirectoryCreation, DirectoryCreationPhase, create_private_directory};
#[cfg(windows)]
// DACL, handle e rename atomico implementati tramite API Win32.
#[allow(unsafe_code)]
mod windows;

/// Crea un file **vuoto** con i permessi consueti, senza toccarne uno che
/// esiste già.
///
/// Diversamente da `write_atomic` non lascia un temporaneo: il file nasce
/// col nome richiesto, e un nome già preso è un errore, non una sovrascrittura.
/// Serve all'albero del progetto, dove «Nuovo file…» non deve mai
/// sovrascrivere il file che l'utente sta vedendo.
pub fn crea_file(path: &Path) -> io::Result<()> {
    // Nessun `sync_all`: il file è vuoto, e un errore di sincronizzazione
    // dopo la creazione lascerebbe il file su disco mentre l'app direbbe
    // che la creazione è fallita.
    let _file = File::options().write(true).create_new(true).open(path)?;
    Ok(())
}

/// Crea una cartella con i permessi consueti, se il nome è libero.
///
/// Come `crea_file`: un nome già preso è un errore, e una cartella non
/// viene mai sovrascritta.
pub fn crea_cartella(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

/// Salva tutto il contenuto e solo dopo sostituisce la destinazione.
///
/// I file in sola lettura vengono rifiutati anche se la directory consentisse
/// di sostituirli: l'editor non deve aggirare la protezione del file.
/// Un nuovo file usa i permessi restrittivi del temporaneo.
pub fn write_atomic(path: &Path, content: &[u8]) -> io::Result<()> {
    write_atomic_with(path, |file| file.write_all(content))
}

/// Salva solo se il file letto all'apertura conserva gli stessi byte.
/// Il controllo avviene sul worker prima di preparare il temporaneo e di nuovo
/// immediatamente prima del commit. Non è un lock contro writer non cooperativi.
pub fn write_atomic_if_unchanged(
    path: &Path,
    content: &[u8],
    expected_fingerprint: [u8; 32],
) -> io::Result<()> {
    prepare_atomic_if_unchanged(path, content, expected_fingerprint)?.commit()
}

/// Fotografa un file regolare sul worker; `None` significa che non esiste.
/// Un leaf non regolare viene rifiutato prima di aprirlo.
pub fn fingerprint_if_regular(path: &Path) -> io::Result<Option<[u8; 32]>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            io::Error::new(io::ErrorKind::InvalidInput, "destinazione non regolare"),
        ),
        Ok(_) => fingerprint_path(path).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Payload sincronizzato in una directory esistente, senza destinazione.
/// Non clonabile, nessuna API commit/retarget/path: non autorizza un checkout.
/// prepare_at consuma il payload e crea un NUOVO writer protetto: non rinomina
/// il temporaneo né trasferisce autorizzazioni dalla collocazione precedente.
/// Il chiamante possiede disco/handle fino ad abort/drop, entrambi sul worker.
/// Debug redatto; Drop tenta unlink senza receipt, abort restituisce il solo
/// esito unlink nativo, non errori close-handle/join/fsync directory/rollback.
#[must_use = "consumare o abbandonare il payload sul worker, non nella UI"]
pub struct PreparedPayload {
    temporary: tempfile::NamedTempFile,
}
impl std::fmt::Debug for PreparedPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedPayload").finish_non_exhaustive()
    }
}
/// Due esiti distinti: writer eventualmente preparato e cleanup sorgente.
/// Un errore unlink non fa perdere l'owner del nuovo writer; il consumatore
/// deve raccoglierlo/abortirlo sul worker, non trattare la coppia come successo.
/// Nessuna clonazione/Debug dei byte, percorsi o errori nativi.
#[must_use = "raccogliere scrittura e cleanup prima di decidere la pubblicazione"]
pub struct PayloadPreparation {
    write: io::Result<PreparedWrite>,
    cleanup: io::Result<()>,
}
impl std::fmt::Debug for PayloadPreparation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PayloadPreparation").finish_non_exhaustive()
    }
}
impl PayloadPreparation {
    pub fn into_results(self) -> (io::Result<PreparedWrite>, io::Result<()>) {
        (self.write, self.cleanup)
    }
}
impl PreparedPayload {
    pub fn abort(self) -> io::Result<()> {
        self.temporary.close()
    }
    /// Consuma i byte tramite il file handle, senza riaprire il nome sorgente.
    /// Nuovo prepare_atomic_with: permessi/accesso/DACL del target attuale e
    /// nuovo temporaneo nel suo parent; nessuna mkdir/commit implicita.
    /// Il chiamante deve validare root, intenti, epoche, topology e policy;
    /// questa copia non congela il contenuto contro altri writer sullo stesso
    /// file, né costituisce confinamento o transazione tra i due filesystem.
    /// Copy/sync/cleanup sono I/O iniziati da raccogliere sul worker, non UI.
    /// Errori interni prima del nuovo handle hanno cleanup Drop best-effort.
    pub fn prepare_at(self, path: &Path) -> PayloadPreparation {
        let mut temporary = self.temporary;
        let write = temporary.as_file_mut().rewind().and_then(|()| {
            prepare_atomic_with(path, |output| {
                io::copy(temporary.as_file_mut(), output).map(|_| ())
            })
        });
        let cleanup = temporary.close();
        PayloadPreparation { write, cleanup }
    }
}

/// Prepara un payload privato in un parent già esistente, senza destinazione,
/// directory create o publish. Utile prima delle future transizioni Git;
/// non prepara i permessi della destinazione futura né garantisce che questa
/// sia sul medesimo filesystem. Il consumatore dovrà rivalidare tutto.
/// Rifiuta parent diretto link/reparse/non-directory/readonly; gli antenati
/// e i cambi noncooperativi non sono confinati da questo controllo metadata.
/// Un errore prima di restituire il handle ha cleanup Drop best-effort.
pub fn prepare_payload(parent: &Path, content: &[u8]) -> io::Result<PreparedPayload> {
    let parent = std::path::absolute(parent)?;
    let metadata = fs::symlink_metadata(&parent)?;
    let linked = metadata.file_type().is_symlink();
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        linked || metadata.file_attributes() & 0x400 != 0
    };
    if linked || !metadata.is_dir() || metadata.permissions().readonly() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "parent payload non disponibile: directory regolare scrivibile richiesta",
        ));
    }
    let mut temporary = protected_temporary(&parent, None)?;
    temporary.as_file_mut().write_all(content)?;
    temporary.as_file().sync_all()?;
    Ok(PreparedPayload { temporary })
}

fn protected_temporary(
    parent: &Path,
    original: Option<&File>,
) -> io::Result<tempfile::NamedTempFile> {
    #[cfg(windows)]
    {
        windows::temporary(parent, original)
    }
    #[cfg(not(windows))]
    {
        let _ = original;
        tempfile::Builder::new()
            .prefix(".lume-save-")
            .tempfile_in(parent)
    }
}

/// Scrittura preparata, ma non ancora pubblicata. Non è clonabile: il commit
/// consuma il solo proprietario. Il drop tenta il cleanup del temporaneo e
/// chiude i handle, senza restituire errori; abort esplicito raccoglie l'esito
/// del cleanup, senza modificare l'originale. Anche il drop può eseguire I/O:
/// preparazione, commit e abort/drop vanno eseguiti sul worker, non in UI.
/// Nessun testo o percorso nel Debug.
///
/// Non rileva cambiamenti concorrenti e non valida epoche/permessi MCP:
/// il consumatore deve rivalidare la richiesta prima di chiamare commit.
/// Più PreparedWrite non costituiscono una transazione multi-file.
pub struct PreparedWrite {
    temporary: tempfile::NamedTempFile,
    destination: std::path::PathBuf,
    original: Option<File>,
    expected_fingerprint: Option<[u8; 32]>,
    enforce_expected_state: bool,
    resolved: bool,
    #[cfg(unix)]
    executable: Option<bool>,
}

impl std::fmt::Debug for PreparedWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedWrite").finish_non_exhaustive()
    }
}

impl PreparedWrite {
    /// Destinazione risolta alla preparazione, anche attraverso un symlink.
    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// Richiede il modo executable Git senza modificare gli altri bit di
    /// accesso. Nuovi file rimangono privati: si aggiunge solo user-execute;
    /// per un blob non executable si eliminano tutti i bit execute.
    /// L'I/O sui permessi e sync avviene nel commit, non in questo setter.
    #[cfg(unix)]
    pub fn with_executable(mut self, executable: bool) -> Self {
        self.executable = Some(executable);
        self
    }

    /// Consuma la preparazione senza pubblicare il contenuto.
    /// Esito del solo cleanup del temporaneo, I/O sul worker; non rollback.
    pub fn abort(self) -> io::Result<()> {
        let Self {
            temporary,
            destination: _,
            original,
            expected_fingerprint: _,
            enforce_expected_state: _,
            resolved: _,
            #[cfg(unix)]
                executable: _,
        } = self;
        drop(original);
        temporary.close()
    }

    /// Pubblica un solo file. Come write_atomic, non promette rollback di
    /// rename parziali Windows. L'avviso sulla sincronizzazione della
    /// directory viene scartato: chi deve mostrarlo usa `commit_reporting`.
    pub fn commit(self) -> io::Result<()> {
        self.commit_reporting().map(drop)
    }

    /// Come `commit`, ma restituisce l'errore della sincronizzazione della
    /// directory dopo il rename (solo Unix). Il file è già pubblicato: non è
    /// un fallimento del salvataggio, ma la durabilità dopo una perdita di
    /// corrente non è garantita. Un filesystem che non supporta la
    /// sincronizzazione delle directory non produce avviso.
    pub fn commit_reporting(self) -> io::Result<Option<io::Error>> {
        let Self {
            temporary,
            destination,
            original,
            expected_fingerprint,
            enforce_expected_state,
            resolved,
            #[cfg(unix)]
            executable,
        } = self;
        // La preparazione può essere rimasta in attesa: non aggirare una
        // protezione/readonly applicata dopo, né ricreare un file eliminato.
        let current = match fs::symlink_metadata(&destination) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "la destinazione è cambiata dopo la preparazione",
                    ));
                }
                if original.is_none() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "la destinazione è stata creata da un altro processo",
                    ));
                }
                let permissions = metadata.permissions();
                if permissions.readonly() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "il file è diventato in sola lettura dopo la preparazione",
                    ));
                }
                let current = open_write_probe(&destination, resolved)?;
                if temporary.as_file().metadata()?.permissions() != permissions {
                    temporary.as_file().set_permissions(permissions)?;
                    temporary.as_file().sync_all()?;
                }
                Some(current)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && original.is_none() => None,
            Err(error) => return Err(error),
        };
        if enforce_expected_state {
            match expected_fingerprint {
                Some(expected) if fingerprint_for_precondition(&destination)? != expected => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "il file è cambiato sul disco dopo la lettura",
                    ));
                }
                None if current.is_some() => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "la destinazione è stata creata dopo la lettura",
                    ));
                }
                _ => {}
            }
        }
        #[cfg(unix)]
        if let Some(executable) = executable {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = temporary.as_file().metadata()?.permissions();
            let mode = permissions.mode();
            permissions.set_mode(if executable {
                mode | 0o100
            } else {
                mode & !0o111
            });
            temporary.as_file().set_permissions(permissions)?;
            temporary.as_file().sync_all()?;
        }
        #[cfg(windows)]
        windows::commit(temporary, &destination, original.is_some())?;
        #[cfg(not(windows))]
        if original.is_some() {
            temporary
                .persist(&destination)
                .map_err(|error| error.error)?;
        } else {
            temporary
                .persist_noclobber(&destination)
                .map_err(|error| error.error)?;
        }
        drop(current);
        drop(original);
        #[cfg(unix)]
        return Ok(sync_parent_directory(&destination));
        #[cfg(not(unix))]
        Ok(None)
    }
}

/// Sincronizza la directory che contiene `destination`, così il rename
/// sopravvive a una perdita di corrente. Errore restituito, mai propagato:
/// il rename è già avvenuto.
#[cfg(unix)]
fn sync_parent_directory(destination: &Path) -> Option<io::Error> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    sync_directory(parent)
}

/// Sincronizza una directory dopo rename/link fatti al suo interno (solo
/// Unix; altrove non fa nulla). Restituisce l'errore invece di propagarlo:
/// chi chiama ha già pubblicato il file e deve solo segnalarlo.
pub fn sync_directory(directory: &Path) -> Option<io::Error> {
    #[cfg(unix)]
    {
        match File::open(directory).and_then(|directory| directory.sync_all()) {
            Ok(()) => None,
            Err(error) if directory_sync_unsupported(&error) => None,
            Err(error) => Some(error),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        None
    }
}

/// fsync(2): EINVAL indica un file che non supporta la sincronizzazione;
/// alcuni filesystem (FUSE, rete) rispondono ENOTSUP/EOPNOTSUPP o ENOSYS.
#[cfg(unix)]
fn directory_sync_unsupported(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code) if code == libc::EINVAL
            || code == libc::ENOTSUP
            || code == libc::EOPNOTSUPP
            || code == libc::ENOSYS
    )
}

/// Prepara e sincronizza contenuto/protezioni sullo stesso filesystem della
/// destinazione; lascia l'originale intatto fino al commit esplicito.
/// Può essere usato su un worker, restituendo solo il handle per la seconda
/// fase. Spazio disco/handle rimangono posseduti dal chiamante fino al drop.
pub fn prepare_atomic(path: &Path, content: &[u8]) -> io::Result<PreparedWrite> {
    prepare_atomic_with(path, |file| file.write_all(content))
}

/// Variante di `prepare_atomic` con precondizione sui byte letti in precedenza.
pub fn prepare_atomic_if_unchanged(
    path: &Path,
    content: &[u8],
    expected_fingerprint: [u8; 32],
) -> io::Result<PreparedWrite> {
    prepare_atomic_with_policy(
        path,
        |file| file.write_all(content),
        false,
        Some(expected_fingerprint),
        true,
    )
}

/// Prepara un path già risolto richiedendo che al commit esista ancora con la
/// stessa impronta, oppure resti assente se la fotografia era `None`.
pub fn prepare_atomic_resolved_if_unchanged(
    path: &Path,
    content: &[u8],
    expected_fingerprint: Option<[u8; 32]>,
) -> io::Result<PreparedWrite> {
    prepare_atomic_with_policy(
        path,
        |file| file.write_all(content),
        true,
        expected_fingerprint,
        true,
    )
}

/// Prepara un nome già risolto dal chiamante. La destinazione deve essere
/// assoluta; non autorizza un secondo attraversamento di un symlink leaf.
/// Non è confinement degli antenati né congelamento dell'identità del file.
/// Il handle resta consuming: prepare/commit/abort/drop devono stare sul worker.
pub fn prepare_atomic_resolved(path: &Path, content: &[u8]) -> io::Result<PreparedWrite> {
    prepare_atomic_with_policy(path, |file| file.write_all(content), true, None, false)
}

fn write_atomic_with(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    prepare_atomic_with(path, write)?.commit()
}

fn prepare_atomic_with(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<PreparedWrite> {
    prepare_atomic_with_policy(path, write, false, None, false)
}

fn prepare_atomic_with_policy(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
    resolved: bool,
    expected_fingerprint: Option<[u8; 32]>,
    enforce_expected_state: bool,
) -> io::Result<PreparedWrite> {
    if resolved && !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "il nome già risolto deve essere assoluto",
        ));
    }
    let destination = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() && resolved => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "il nome già risolto è diventato un link: nessuna preparazione",
            ));
        }
        Ok(metadata) if metadata.file_type().is_symlink() => crate::canonical_path(path)?,
        // Il commit può arrivare da un altro task, dopo un cambio di cwd.
        Ok(_) => std::path::absolute(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => std::path::absolute(path)?,
        Err(error) => return Err(error),
    };
    let (permissions, original) = match fs::metadata(&destination) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "la destinazione non è un file regolare",
                ));
            }
            let permissions = metadata.permissions();
            if permissions.readonly() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "il file è in sola lettura",
                ));
            }
            // readonly non basta: i diritti effettivi possono negare la
            // scrittura anche se la directory permette di sostituire il file.
            // L'apertura non tronca nulla; il handle resta vivo fino al commit.
            let original = open_write_probe(&destination, resolved)?;
            (Some(permissions), Some(original))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (None, None),
        Err(error) => return Err(error),
    };
    if enforce_expected_state {
        match expected_fingerprint {
            Some(expected) if fingerprint_for_precondition(&destination)? != expected => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "il file è cambiato sul disco prima della preparazione",
                ));
            }
            None if original.is_some() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "la destinazione è stata creata prima della preparazione",
                ));
            }
            _ => {}
        }
    }
    // Path::parent restituisce un percorso vuoto per un semplice nome relativo.
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = protected_temporary(parent, original.as_ref())?;
    write(temporary.as_file_mut())?;
    if let Some(permissions) = permissions {
        temporary.as_file().set_permissions(permissions)?;
    }
    temporary.as_file().sync_all()?;
    Ok(PreparedWrite {
        temporary,
        destination,
        original,
        expected_fingerprint,
        enforce_expected_state,
        resolved,
        #[cfg(unix)]
        executable: None,
    })
}

fn fingerprint_path(path: &Path) -> io::Result<[u8; 32]> {
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "impronta disponibile solo per file regolari",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "destinazione reparse non valida per fingerprint",
            ));
        }
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let chunk = buffer.get(..read).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "dimensione letta non valida")
        })?;
        hasher.update(chunk);
    }
    Ok(hasher.finalize().into())
}

fn fingerprint_for_precondition(path: &Path) -> io::Result<[u8; 32]> {
    fingerprint_path(path).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("il file è cambiato sul disco o non è verificabile: {error}"),
        )
    })
}

fn open_write_probe(path: &Path, resolved: bool) -> io::Result<File> {
    let mut options = File::options();
    options.write(true);
    // READ_CONTROL/lettura DACL del backend Windows esistente; su Unix
    // nessun diritto read aggiuntivo. Questa apertura NON tronca/scrive.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.read(true);
        if resolved {
            options.custom_flags(
                windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
            );
        }
    }
    #[cfg(unix)]
    if resolved {
        use std::os::unix::fs::OpenOptionsExt;
        // Un leaf sostituito da FIFO dopo lo stat non deve sospendere il
        // probe; l'FD ottenuto viene classificato prima di preparare temporanei.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if resolved {
        let metadata = file.metadata()?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                != 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "il probe risolto è un reparse point",
                ));
            }
        }
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "il probe risolto non è un file regolare",
            ));
        }
    }
    Ok(file)
}

#[cfg(all(test, unix))]
#[allow(unsafe_code)]
mod resolved_probe_tests;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn fingerprint(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }

    #[test]
    fn commit_reporting_pubblica_e_sincronizza_la_directory_senza_avvisi() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documento.txt");
        fs::write(&path, b"prima").unwrap();

        let warning = prepare_atomic(&path, b"dopo").unwrap().commit_reporting();

        assert!(warning.unwrap().is_none());
        assert_eq!(fs::read(&path).unwrap(), b"dopo");
        assert_eq!(voci(directory.path()), 1);
    }

    #[cfg(unix)]
    #[test]
    fn errore_reale_della_sincronizzazione_directory_viene_restituito() {
        let directory = tempfile::tempdir().unwrap();
        let mancante = directory.path().join("rimossa").join("documento.txt");

        let error = sync_parent_directory(&mancante).expect("errore restituito");

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[cfg(unix)]
    #[test]
    fn directory_senza_supporto_fsync_non_produce_avviso() {
        for code in [libc::EINVAL, libc::ENOTSUP, libc::EOPNOTSUPP, libc::ENOSYS] {
            assert!(directory_sync_unsupported(&io::Error::from_raw_os_error(
                code
            )));
        }
        for code in [libc::EIO, libc::ENOSPC, libc::EROFS] {
            assert!(!directory_sync_unsupported(&io::Error::from_raw_os_error(
                code
            )));
        }
    }

    #[test]
    fn save_condizionale_rifiuta_modifica_della_stessa_lunghezza_prima_della_preparazione() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documento.txt");
        fs::write(&path, b"prima").unwrap();
        fs::write(&path, b"dopo!").unwrap();

        let result = write_atomic_if_unchanged(&path, b"locale", fingerprint(b"prima"));

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&path).unwrap(), b"dopo!");
        assert_eq!(voci(directory.path()), 1);
    }

    #[test]
    fn commit_condizionale_rifiuta_modifica_dopo_la_preparazione() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documento.txt");
        fs::write(&path, b"prima").unwrap();
        let prepared =
            prepare_atomic_if_unchanged(&path, b"locale", fingerprint(b"prima")).unwrap();
        fs::write(&path, b"dopo!").unwrap();

        let result = prepared.commit();

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&path).unwrap(), b"dopo!");
        assert_eq!(voci(directory.path()), 1);
    }

    #[cfg(unix)]
    #[test]
    fn alias_symlink_condizionale_rifiuta_modifica_stessa_lunghezza() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("target.txt");
        let alias = directory.path().join("alias.txt");
        fs::write(&path, b"prima").unwrap();
        symlink(&path, &alias).unwrap();
        let expected = fingerprint(b"prima");
        fs::write(&path, b"dopo!").unwrap();

        let result = write_atomic_if_unchanged(&alias, b"locale", expected);

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&path).unwrap(), b"dopo!");
        assert!(
            fs::symlink_metadata(&alias)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(voci(directory.path()), 2);
    }

    #[cfg(unix)]
    #[test]
    fn alias_hardlink_condizionale_rileva_mutazione_condivisa() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("originale.txt");
        let alias = directory.path().join("hardlink.txt");
        fs::write(&path, b"prima").unwrap();
        fs::hard_link(&path, &alias).unwrap();
        let expected = fingerprint(b"prima");
        fs::write(&alias, b"dopo!").unwrap();

        let result = write_atomic_if_unchanged(&path, b"locale", expected);

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&path).unwrap(), b"dopo!");
        assert_eq!(fs::read(&alias).unwrap(), b"dopo!");
        assert_eq!(voci(directory.path()), 2);
    }

    #[cfg(unix)]
    #[test]
    #[allow(unsafe_code)]
    fn destinazione_diventata_fifo_dopo_prepare_e_rifiutata() -> io::Result<()> {
        use std::os::unix::{ffi::OsStrExt, fs::FileTypeExt};
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("destinazione.txt");
        fs::write(&path, b"prima")?;
        let prepared = prepare_atomic_if_unchanged(&path, b"locale", fingerprint(b"prima"))?;
        fs::remove_file(&path)?;
        let fifo = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path nul"))?;
        // SAFETY: il `CString` del percorso resta vivo e il FIFO è creato nel percorso temporaneo del test.
        if unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) } != 0 {
            return Err(io::Error::last_os_error());
        }

        assert_eq!(
            prepared.commit().unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(fs::symlink_metadata(&path)?.file_type().is_fifo());
        assert_eq!(voci(directory.path()), 1);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn link_e_hardlink_preservano_il_comportamento_condizionale() -> io::Result<()> {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("source.txt");
        let alias_link = directory.path().join("link.txt");
        let alias_hard = directory.path().join("hard.txt");
        fs::write(&path, b"prima")?;
        symlink(&path, &alias_link)?;
        write_atomic_if_unchanged(&alias_link, b"dopo!", fingerprint(b"prima"))?;
        assert!(fs::symlink_metadata(&alias_link)?.file_type().is_symlink());
        assert_eq!(fs::read(&path)?, b"dopo!");

        fs::hard_link(&path, &alias_hard)?;
        write_atomic_if_unchanged(&path, b"finale", fingerprint(b"dopo!"))?;
        assert_eq!(fs::read(&path)?, b"finale");
        assert_eq!(fs::read(&alias_hard)?, b"dopo!");
        assert_eq!(voci(directory.path()), 3);
        Ok(())
    }

    #[test]
    fn target_risolto_condizionale_rifiuta_cambio_esterno_sul_target() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("destinazione.txt");
        fs::write(&path, b"prima").unwrap();
        let expected = fingerprint_if_regular(&path).unwrap();
        let prepared = prepare_atomic_resolved_if_unchanged(&path, b"locale", expected).unwrap();
        fs::write(&path, b"dopo!").unwrap();

        let result = prepared.commit();

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&path).unwrap(), b"dopo!");
        assert_eq!(voci(directory.path()), 1);
    }

    #[test]
    fn target_risolto_inizialmente_assente_non_sovrascrive_file_creato_dopo_prepare() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nuovo.txt");
        let prepared = prepare_atomic_resolved_if_unchanged(&path, b"locale", None).unwrap();
        fs::write(&path, b"esterno").unwrap();

        let result = prepared.commit();

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), b"esterno");
        assert_eq!(voci(directory.path()), 1);
    }

    fn voci(path: &Path) -> usize {
        fs::read_dir(path).unwrap().count()
    }

    #[test]
    fn crea_un_file_vuoto_e_non_tocca_uno_esistente() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nuovo.txt");
        crea_file(&path).unwrap();
        assert!(path.is_file());
        assert_eq!(fs::read(&path).unwrap(), b"");
        assert_eq!(voci(directory.path()), 1);

        // il nome già preso è un errore: niente sovrascrittura.
        let errore = crea_file(&path).unwrap_err();
        assert_eq!(errore.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), b"");
    }

    #[test]
    fn crea_una_cartella_e_non_tocca_una_esistente() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nuova");
        crea_cartella(&path).unwrap();
        assert!(path.is_dir());
        assert_eq!(voci(directory.path()), 1);

        let errore = crea_cartella(&path).unwrap_err();
        assert_eq!(errore.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(voci(directory.path()), 1);
    }

    #[test]
    fn sostituisce_il_contenuto_e_non_lascia_temporanei() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documento.txt");
        fs::write(&path, "originale lungo").unwrap();
        write_atomic(&path, b"nuovo").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"nuovo");
        assert_eq!(voci(directory.path()), 1);
    }

    #[test]
    fn crea_un_file_nuovo_e_salva_anche_un_contenuto_vuoto() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documento.txt");
        write_atomic(&path, b"testo").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"testo");
        write_atomic(&path, b"").unwrap();
        assert!(fs::read(&path).unwrap().is_empty());
        assert_eq!(voci(directory.path()), 1);
    }

    #[test]
    fn una_scrittura_parziale_fallita_non_tocca_l_originale() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documento.txt");
        fs::write(&path, "originale").unwrap();
        let result = write_atomic_with(&path, |file| {
            file.write_all(b"parziale")?;
            Err(io::Error::other("scrittura interrotta"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"originale");
        assert_eq!(voci(directory.path()), 1);
    }

    #[test]
    fn un_commit_fallito_elimina_il_temporaneo() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documento.txt");
        // La destinazione diventa una directory dopo la validazione iniziale:
        // il rename deve fallire e non lasciare il file parziale in giro.
        let result = write_atomic_with(&path, |file| {
            file.write_all(b"nuovo")?;
            fs::create_dir(&path)
        });
        assert!(result.is_err());
        assert!(path.is_dir());
        assert_eq!(voci(directory.path()), 1);
        assert_eq!(voci(&path), 0);
    }

    #[test]
    fn rifiuta_una_directory_senza_toccarne_il_contenuto() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cartella");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("figlio.txt"), b"originale").unwrap();
        assert!(write_atomic(&path, b"nuovo").is_err());
        assert_eq!(fs::read(path.join("figlio.txt")).unwrap(), b"originale");
        assert_eq!(voci(directory.path()), 1);
    }

    #[test]
    fn rifiuta_un_file_in_sola_lettura_anche_se_la_directory_e_scrivibile() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("documento.txt");
        fs::write(&path, b"originale").unwrap();
        let original = fs::metadata(&path).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&path, readonly).unwrap();
        let result = write_atomic(&path, b"nuovo");
        // Ripristino anche su Windows, dove un file readonly blocca il cleanup.
        fs::set_permissions(&path, original).unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&path).unwrap(), b"originale");
        assert_eq!(voci(directory.path()), 1);
    }

    #[test]
    fn una_directory_mancante_non_crea_niente() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("assente/documento.txt");
        assert!(write_atomic(&path, b"nuovo").is_err());
        assert_eq!(voci(directory.path()), 0);
    }

    #[cfg(unix)]
    #[test]
    fn non_aggira_i_permessi_effettivi_del_proprietario() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("protetto.txt");
        fs::write(&path, b"originale").unwrap();
        let original = fs::metadata(&path).unwrap().permissions();
        // Gli altri possono scrivere, il proprietario no: readonly() è falso.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o466)).unwrap();
        if File::options().write(true).open(&path).is_ok() {
            fs::set_permissions(&path, original).unwrap();
            eprintln!("SALTATO: l'utente può aggirare i permessi Unix (per esempio root)");
            return;
        }
        let result = write_atomic(&path, b"nuovo");
        fs::set_permissions(&path, original).unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&path).unwrap(), b"originale");
        assert_eq!(voci(directory.path()), 1);
    }

    #[cfg(unix)]
    #[test]
    fn salva_un_file_scrivibile_ma_non_piu_leggibile() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("solo-scrittura.txt");
        fs::write(&path, b"originale").unwrap();
        let original = fs::metadata(&path).unwrap().permissions();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o200)).unwrap();
        let result = write_atomic(&path, b"nuovo");
        let saved_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        fs::set_permissions(&path, original).unwrap();
        result.unwrap();
        assert_eq!(saved_mode, 0o200);
        assert_eq!(fs::read(&path).unwrap(), b"nuovo");
        assert_eq!(voci(directory.path()), 1);
    }

    #[cfg(unix)]
    #[test]
    fn conserva_i_permessi_unix_del_file() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("script.sh");
        fs::write(&path, b"originale").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
        write_atomic(&path, b"nuovo").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o750
        );
    }

    #[cfg(unix)]
    #[test]
    fn salva_il_bersaglio_senza_sostituire_il_link_simbolico() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("bersaglio.txt");
        let link = directory.path().join("link.txt");
        fs::write(&target, b"originale").unwrap();
        symlink("bersaglio.txt", &link).unwrap();
        write_atomic(&link, b"nuovo").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"nuovo");
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(voci(directory.path()), 2);
    }

    #[cfg(unix)]
    #[test]
    fn un_link_simbolico_interrotto_non_viene_sostituito() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let link = directory.path().join("link.txt");
        symlink("assente.txt", &link).unwrap();
        assert!(write_atomic(&link, b"nuovo").is_err());
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(voci(directory.path()), 1);
    }
}
