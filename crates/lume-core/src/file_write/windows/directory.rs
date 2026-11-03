//! Riusa le API ACL del writer, prima di creare una directory vuota.
use super::{
    SecurityDescriptor, current_user_sid, require_persistent_acls, security_error, wide_path,
    win32_bool,
};
use crate::file_write::DirectoryCreationPhase;
use std::{fs::OpenOptions, io, mem::size_of, os::windows::fs::OpenOptionsExt, path::Path};
use windows_sys::Win32::{
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::{
        CreateDirectoryW, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL,
    },
};

pub(in crate::file_write) fn create_directory(
    path: &Path,
    phase: &mut DirectoryCreationPhase,
) -> io::Result<()> {
    wide_path(path)?;
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "parent della directory assente",
        )
    })?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "nome directory assente"))?;
    let parent = std::fs::canonicalize(parent)?;
    let directory = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(&parent)?;
    if !directory.metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "parent non è una directory",
        ));
    }
    require_persistent_acls(&directory)?;
    let user = current_user_sid()?;
    // Stessa policy del nuovo file/payload del writer: niente Everyone o
    // ereditarietà del parent; ACL privata passata a CreateDirectoryW.
    let security =
        SecurityDescriptor::from_sddl(&format!("O:{user}D:P(A;;FA;;;{user})(A;;FA;;;SY)"))?;
    security.entries()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.0.0,
        bInheritHandle: 0,
    };
    let path = parent.join(name);
    let wide = wide_path(&path)?;
    *phase = DirectoryCreationPhase::CreationUnconfirmed;
    // SAFETY: percorso NUL terminato, descrittore validato vivo per la call.
    // Nessuna apertura di un nome preesistente/fallback DACL di default.
    win32_bool(unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) })?;
    *phase = DirectoryCreationPhase::Created;
    // Un errore da qui conserva la fase Created; non si cancella la directory
    // di nascosto e non si inserisce alcun file prima di confermare la DACL.
    let created = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | READ_CONTROL)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(&path)?;
    let metadata = created.metadata()?;
    use std::os::windows::fs::MetadataExt;
    if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
        return Err(security_error(
            "nuova directory non regolare/reparse: creazione da verificare",
        ));
    }
    require_persistent_acls(&created)?;
    let actual = SecurityDescriptor::from_file(&created)?;
    if !actual.is_protected()? || actual.entries()? != security.entries()? {
        return Err(security_error(
            "Windows non ha conservato la DACL protetta della nuova directory",
        ));
    }
    security.verify_identity(&actual)?;
    Ok(())
}
