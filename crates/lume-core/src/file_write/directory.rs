//! Una directory privata per chiamata, senza mkdir ricorsiva o rollback.
use std::{fs, io, path::Path};

/// Fase osservata, NON una prova di ownership del nome contro noncooperatori.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryCreationPhase {
    /// Nessuna chiamata alla primitiva di creazione iniziata.
    NotAttempted,
    /// Chiamata iniziata senza successo confermato: controllare il filesystem,
    /// non interpretare un errore nativo come garanzia universale di no-write.
    CreationUnconfirmed,
    /// La primitiva ha restituito successo, anche se la verifica è poi fallita.
    Created,
}
/// Esito consumabile: mantiene fase e verifica distinte, niente nomi nel Debug.
/// Nessuna rimozione al Drop: una directory creata è un effetto reale da
/// dichiarare, non da nascondere cancellando nomi potenzialmente sostituiti.
#[must_use = "raccogliere fase ed esito della directory sul worker"]
pub struct DirectoryCreation {
    phase: DirectoryCreationPhase,
    result: io::Result<()>,
}
impl std::fmt::Debug for DirectoryCreation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryCreation").finish_non_exhaustive()
    }
}
impl DirectoryCreation {
    pub fn into_parts(self) -> (DirectoryCreationPhase, io::Result<()>) {
        (self.phase, self.result)
    }
}
/// Crea una sola directory privata se il parent diretto esiste: Unix0700;
/// Windows DACL protetta utente effettivo+SYSTEM impostata alla creazione e
/// verificata con il backend ACL del writer. Nessun chmod/DACL fallback,
/// overwrite, contenuto scritto, directory intermedia o cleanup implicito.
///
/// Solo controllo metadata del parent diretto, NON confinamento degli
/// antenati/radice/alias né freeze dei diritti/contenuti. Il consumatore deve
/// possedere e rivalidare l'intento corrente prima di questa azione reale.
/// Tutto l'I/O iniziato va raccolto sul worker; non è un'operazione UI/join,
/// una transazione con file/index/ref o una garanzia fsync/power-loss.
/// Su Created+Err la directory rimane: esito/protezioni da verificare prima
/// di aggiungere file. CreationUnconfirmed non autorizza retry/rollback.
/// La DACL della nuova directory non sostituisce le verifiche del writer
/// protetto sui futuri file figli o quelle dei permessi di cancellazione.
pub fn create_private_directory(path: &Path) -> DirectoryCreation {
    let mut phase = DirectoryCreationPhase::NotAttempted;
    let result = (|| {
        let path = std::path::absolute(path)?;
        if path.file_name().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "la directory richiesta non ha un nome",
            ));
        }
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "parent della directory assente",
            )
        })?;
        let metadata = fs::symlink_metadata(parent)?;
        let linked = metadata.file_type().is_symlink();
        #[cfg(windows)]
        let linked = {
            use std::os::windows::fs::MetadataExt;
            linked || metadata.file_attributes() & 0x400 != 0
        };
        if linked || !metadata.is_dir() || metadata.permissions().readonly() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "parent della nuova directory link/reparse/readonly o non regolare",
            ));
        }
        #[cfg(windows)]
        {
            super::windows::create_directory(&path, &mut phase)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            phase = DirectoryCreationPhase::CreationUnconfirmed;
            fs::DirBuilder::new().mode(0o700).create(&path)?;
            phase = DirectoryCreationPhase::Created;
            let current = fs::symlink_metadata(&path)?;
            if !current.is_dir()
                || current.file_type().is_symlink()
                || current.permissions().mode() & 0o777 != 0o700
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "protezioni della nuova directory non confermate: directory creata da verificare",
                ));
            }
        }
        #[cfg(not(any(windows, unix)))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "creazione directory protetta non supportata su questa piattaforma",
            ));
        }
        Ok(())
    })();
    DirectoryCreation { phase, result }
}
