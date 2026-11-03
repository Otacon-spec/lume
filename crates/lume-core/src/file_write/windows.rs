//! Backend Windows: nessun contenuto viene scritto prima di creare e verificare
//! un temporaneo con DACL protetta. Il chiamante deve aprire l'originale con
//! GENERIC_READ | GENERIC_WRITE | READ_CONTROL e FILE_SHARE_DELETE, mantenendo
//! tale handle fino al commit. Non sono abilitate eccezioni agli errori ACL.
//!
//! Sono rifiutati filesystem senza ACL persistenti, DACL assenti/NULL e
//! descrittori che Windows non riesce a riprodurre fedelmente. La sostituzione
//! cambia l'identità del file: altri hardlink restano sul vecchio contenuto;
//! non si garantiscono gli EA o tutti i metadati (in particolare owner/SACL
//! della destinazione non rientrano nelle garanzie di ReplaceFileW).
//! Non si rilevano modifiche concorrenti a percorsi, contenuto o ACL.
//!
//! Gli errori prima di ReplaceFileW lasciano intatto l'originale. L'API Win32
//! documenta invece errori parziali 1176/1177 che possono cambiare i nomi anche
//! se restituisce errore: non si promette un rollback in quei casi, né la
//! persistenza del rename dopo una perdita di corrente. Nei due errori
//! parziali si conserva il temporaneo, indicandone il percorso nell'errore.
//! Negli altri casi il cleanup è RAII, soggetto ai diritti di cancellazione.

mod directory;
pub(super) use directory::create_directory;

use std::fs::{File, OpenOptions};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_NO_TOKEN, ERROR_UNABLE_TO_MOVE_REPLACEMENT,
    ERROR_UNABLE_TO_MOVE_REPLACEMENT_2, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, EqualSid, GROUP_SECURITY_INFORMATION, GetAce,
    GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorGroup,
    GetSecurityDescriptorOwner, GetTokenInformation, INHERITED_ACE, IsValidAcl,
    IsValidSecurityDescriptor, IsValidSid, OWNER_SECURITY_INFORMATION, PSID,
    SE_DACL_AUTO_INHERIT_REQ, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, SetSecurityDescriptorControl,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetVolumeInformationByHandleW, READ_CONTROL, ReplaceFileW,
};
use windows_sys::Win32::System::SystemServices::FILE_PERSISTENT_ACLS;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
};

pub(super) fn temporary(
    parent: &Path,
    original: Option<&File>,
) -> io::Result<tempfile::NamedTempFile> {
    // canonicalize stabilizza i percorsi relativi e fornisce il prefisso Win32
    // per i percorsi lunghi. La validazione NUL precede qualunque API di sistema.
    wide_path(parent)?;
    let parent = std::fs::canonicalize(parent)?;
    let directory = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(&parent)?;
    if !directory.metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "il parent non è una directory",
        ));
    }
    require_persistent_acls(&directory)?;
    let security = match original {
        Some(file) => {
            require_writable_file(&file.metadata()?)?;
            let security = SecurityDescriptor::from_file(file)?;
            // GetSecurityInfo restituisce un descrittore self-relative allocato
            // da Windows. Si cambia solo il controllo, mai la DACL originale.
            // SAFETY: allocazione valida e mutabile, posseduta da security.
            win32_bool(unsafe {
                SetSecurityDescriptorControl(
                    security.0.0,
                    SE_DACL_PROTECTED | SE_DACL_AUTO_INHERIT_REQ,
                    SE_DACL_PROTECTED,
                )
            })?;
            security
        }
        None => {
            let user = current_user_sid()?;
            SecurityDescriptor::from_sddl(&format!("O:{user}D:P(A;;FA;;;{user})(A;;FA;;;SY)"))?
        }
    };
    // Validazione completa prima della creazione; mai un fallback ai permessi
    // std o a una DACL di default se le API di sicurezza falliscono.
    security.entries()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.0.0,
        bInheritHandle: 0,
    };
    let temporary = tempfile::Builder::new()
        .prefix(".lume-save-")
        .make_in(&parent, |path| {
            let path = wide_path(path)?;
            // SAFETY: percorso terminato NUL; attributi e descrittore restano vivi.
            // CREATE_NEW non apre né tronca un file preesistente; l'handle non è
            // ereditabile e condivide DELETE per consentire ReplaceFileW/persist.
            let handle = unsafe {
                CreateFileW(
                    path.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE | READ_CONTROL,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    &attributes,
                    CREATE_NEW,
                    FILE_ATTRIBUTE_NORMAL,
                    ptr::null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: handle nuovo valido, trasferito una sola volta a File.
            Ok(unsafe { File::from_raw_handle(handle) })
        })?;
    // Il NamedTempFile possiede già handle e nome: qualsiasi errore da qui in
    // avanti attiva il cleanup. Non è stato ancora scritto alcun contenuto.
    require_persistent_acls(temporary.as_file())?;
    let actual = SecurityDescriptor::from_file(temporary.as_file())?;
    if !actual.is_protected()? || actual.entries()? != security.entries()? {
        return Err(security_error(
            "Windows non ha conservato la DACL protetta richiesta",
        ));
    }
    security.verify_identity(&actual)?;
    Ok(temporary)
}

pub(super) fn commit(
    temporary: tempfile::NamedTempFile,
    destination: &Path,
    existed: bool,
) -> io::Result<()> {
    wide_path(destination)?;
    let destination = absolute_destination(destination)?;
    let destination_wide = wide_path(&destination)?;
    if !existed {
        temporary.as_file().sync_all()?;
        // Su Windows persist_noclobber usa MoveFileExW senza REPLACE_EXISTING:
        // il rename è atomico e una destinazione comparsa nel frattempo causa
        // errore. PersistError mantiene il temporaneo e lo elimina al Drop.
        temporary
            .persist_noclobber(&destination)
            .map_err(|error| error.error)?;
        return Ok(());
    }
    require_writable_file(&std::fs::metadata(&destination)?)?;
    let temporary_wide = wide_path(temporary.path())?;
    temporary.as_file().sync_all()?;
    // ReplaceFileW apre il replacement senza condivisione: anche un handle
    // che condivide DELETE lo bloccherebbe se è ancora aperto in lettura o
    // scrittura. Si chiude il file, ma TempPath mantiene proprietà e cleanup
    // del nome fino al commit (o al recupero di un errore parziale).
    let (file, mut temporary) = temporary.into_parts();
    drop(file);
    // SAFETY: entrambi i percorsi sono terminati NUL e restano vivi; flag zero
    // impone la conservazione della DACL e degli altri metadati supportati
    // dall'API. Nessun fallback a rename/copia e nessun IGNORE_*_ERRORS.
    let result = unsafe {
        ReplaceFileW(
            destination_wide.as_ptr(),
            temporary_wide.as_ptr(),
            ptr::null(),
            0,
            ptr::null(),
            ptr::null(),
        )
    };
    if result == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_UNABLE_TO_MOVE_REPLACEMENT as i32)
            || error.raw_os_error() == Some(ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 as i32)
        {
            // In questi casi Win32 può avere già rimosso/rinominato l'originale.
            // Eliminare il temporaneo rischierebbe di perdere anche il nuovo
            // contenuto; si conserva il file e si segnala dove recuperarlo.
            temporary.disable_cleanup(true);
            return Err(io::Error::new(
                error.kind(),
                format!(
                    "sostituzione Windows incompleta ({error}); temporaneo conservato per recupero: {}",
                    temporary.display(),
                ),
            ));
        }
        return Err(error);
    }
    // Il rename è riuscito: non si deve più eliminare il vecchio nome, che
    // potrebbe essere riutilizzato. L'handle del file è già stato chiuso.
    temporary.disable_cleanup(true);
    Ok(())
}

fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL nel percorso Windows",
        ));
    }
    wide.push(0);
    Ok(wide)
}

fn absolute_destination(destination: &Path) -> io::Result<PathBuf> {
    let destination = std::path::absolute(destination)?;
    let name = destination.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "la destinazione non ha un nome di file",
        )
    })?;
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "la destinazione non ha una directory",
        )
    })?;
    Ok(std::fs::canonicalize(parent)?.join(name))
}

fn require_writable_file(metadata: &std::fs::Metadata) -> io::Result<()> {
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "la destinazione non è un file regolare",
        ));
    }
    if metadata.permissions().readonly() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "il file è in sola lettura",
        ));
    }
    Ok(())
}

fn require_persistent_acls(file: &File) -> io::Result<()> {
    let mut flags = 0;
    // SAFETY: handle prestato valido e unico output non NULL su stack.
    win32_bool(unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut flags,
            ptr::null_mut(),
            0,
        )
    })?;
    if flags & FILE_PERSISTENT_ACLS == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "il filesystem non garantisce ACL persistenti",
        ));
    }
    Ok(())
}

// LocalFree è distinto da CloseHandle: tutte le allocazioni dei descrittori e
// delle stringhe SID restano possedute anche lungo i rami di errore.
struct LocalAllocation(*mut std::ffi::c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: unica proprietà di una allocazione LocalAlloc Win32.
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

struct SecurityDescriptor(LocalAllocation);

impl SecurityDescriptor {
    fn from_file(file: &File) -> io::Result<Self> {
        let mut descriptor = ptr::null_mut();
        // SAFETY: handle prestato; output inizializzato e poi gestito con RAII.
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        let descriptor = Self(LocalAllocation(descriptor));
        win32_status(status)?;
        descriptor.validate()?;
        if descriptor.sid(true)?.is_null() || descriptor.sid(false)?.is_null() {
            return Err(security_error(
                "owner o gruppo mancante nel descrittore del file",
            ));
        }
        Ok(descriptor)
    }

    fn from_sddl(sddl: &str) -> io::Result<Self> {
        let sddl: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: SDDL interno terminato NUL, output gestito con LocalFree.
        let result = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        let descriptor = Self(LocalAllocation(descriptor));
        win32_bool(result)?;
        descriptor.validate()?;
        Ok(descriptor)
    }

    fn validate(&self) -> io::Result<()> {
        // SAFETY: se non NULL, il puntatore proviene dalle API Win32 sopra.
        if self.0.0.is_null() || unsafe { IsValidSecurityDescriptor(self.0.0) } == 0 {
            return Err(security_error("descrittore di sicurezza non valido"));
        }
        Ok(())
    }

    fn dacl(&self) -> io::Result<*mut ACL> {
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = ptr::null_mut();
        // SAFETY: descrittore validato, output su stack; ACL vive quanto self.
        win32_bool(unsafe {
            GetSecurityDescriptorDacl(self.0.0, &mut present, &mut acl, &mut defaulted)
        })?;
        // Una DACL assente/NULL consente accesso universale: rifiutarla evita
        // di confonderla con una DACL vuota o con un default ereditabile.
        // SAFETY: `acl` è non nullo e validato da `IsValidAcl` nel metodo `dacl`.
        if present == 0 || acl.is_null() || unsafe { IsValidAcl(acl) } == 0 {
            return Err(security_error("DACL assente, NULL o non valida"));
        }
        Ok(acl)
    }

    fn is_protected(&self) -> io::Result<bool> {
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: descrittore validato e output su stack.
        win32_bool(unsafe { GetSecurityDescriptorControl(self.0.0, &mut control, &mut revision) })?;
        Ok(control & SE_DACL_PROTECTED != 0)
    }

    fn entries(&self) -> io::Result<Vec<Vec<u8>>> {
        let acl = self.dacl()?;
        // SAFETY: IsValidAcl ha validato intestazione e dimensioni delle ACE.
        let count = unsafe { (*acl).AceCount };
        let mut entries = Vec::with_capacity(usize::from(count));
        for index in 0..u32::from(count) {
            let mut ace = ptr::null_mut();
            // SAFETY: indice entro AceCount di una DACL valida ancora viva.
            win32_bool(unsafe { GetAce(acl, index, &mut ace) })?;
            // SAFETY: GetAce restituisce una ACE valida nell'allocazione Win32.
            let size = usize::from(unsafe { (*ace.cast::<ACE_HEADER>()).AceSize });
            if size < size_of::<ACE_HEADER>() {
                return Err(security_error("ACE troppo corta"));
            }
            // SAFETY: dimensione validata da IsValidAcl e proveniente da GetAce.
            let bytes = unsafe { std::slice::from_raw_parts(ace.cast::<u8>(), size) };
            // Windows può convertire una ACE ereditata in esplicita durante
            // la creazione protetta. Si ignora solo INHERITED_ACE; maschere,
            // SID, ordine, INHERIT_ONLY e altri flag devono restare identici.
            entries.push(
                bytes
                    .iter()
                    .enumerate()
                    .map(|(offset, byte)| {
                        if offset == 1 {
                            *byte & !(INHERITED_ACE as u8)
                        } else {
                            *byte
                        }
                    })
                    .collect(),
            );
        }
        Ok(entries)
    }

    fn verify_identity(&self, actual: &Self) -> io::Result<()> {
        for owner in std::iter::once(true).chain(std::iter::once(false)) {
            let expected = self.sid(owner)?;
            // Il nuovo SDDL non specifica il gruppo: viene scelto dal token.
            // Per un originale si conservano entrambi, anche per OWNER_RIGHTS
            // e i SID CREATOR_*; una sostituzione automatica viene rifiutata.
            if !expected.is_null() {
                let actual = actual.sid(owner)?;
                // SAFETY: SID validati, prestati dai descrittori ancora vivi.
                if actual.is_null() || unsafe { EqualSid(expected, actual) } == 0 {
                    return Err(security_error(
                        "Windows non ha conservato owner o gruppo del temporaneo",
                    ));
                }
            }
        }
        Ok(())
    }

    fn sid(&self, owner: bool) -> io::Result<PSID> {
        let mut sid = ptr::null_mut();
        let mut defaulted = 0;
        // SAFETY: descrittore valido e output inizializzati su stack.
        let result = unsafe {
            if owner {
                GetSecurityDescriptorOwner(self.0.0, &mut sid, &mut defaulted)
            } else {
                GetSecurityDescriptorGroup(self.0.0, &mut sid, &mut defaulted)
            }
        };
        win32_bool(result)?;
        // SAFETY: `sid` è non nullo e proviene dal descrittore di sicurezza ancora vivo; il controllo non lo modifica.
        if !sid.is_null() && unsafe { IsValidSid(sid) } == 0 {
            return Err(security_error("SID non valido nel descrittore"));
        }
        Ok(sid)
    }
}

fn current_user_sid() -> io::Result<String> {
    let mut token = ptr::null_mut();
    // Si usa il token effettivo del thread se impersona un utente. Solo
    // ERROR_NO_TOKEN consente il fallback; gli errori di sicurezza si propagano.
    // SAFETY: pseudo-handle valido e output su stack; non si chiude lo pseudo-handle.
    if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_NO_TOKEN as i32) {
            return Err(error);
        }
        // SAFETY: pseudo-handle di processo valido e output su stack.
        win32_bool(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) })?;
    }
    // SAFETY: Open*Token ha restituito un handle reale nuovo, posseduto una volta.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut size = 0;
    // SAFETY: interrogazione della dimensione, senza buffer di output.
    let result = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            ptr::null_mut(),
            0,
            &mut size,
        )
    };
    if result == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
            return Err(error);
        }
    }
    if (size as usize) < size_of::<TOKEN_USER>() {
        return Err(security_error("dimensione TOKEN_USER non valida"));
    }
    // Vec<usize> garantisce l'allineamento di TOKEN_USER e dei puntatori SID.
    let mut buffer: Vec<usize> =
        std::iter::repeat_n(0, (size as usize).div_ceil(size_of::<usize>())).collect();
    let mut returned = 0;
    // SAFETY: buffer allineato, di almeno size byte, vivo fino alla conversione SID.
    win32_bool(unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut returned,
        )
    })?;
    if returned < size_of::<TOKEN_USER>() as u32 || returned > size {
        return Err(security_error("TOKEN_USER incompleto"));
    }
    // SAFETY: il buffer contiene TOKEN_USER valido scritto da GetTokenInformation.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    // SAFETY: il SID punta al `TOKEN_USER` inizializzato nel buffer allineato e ancora vivo.
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        return Err(security_error("SID dell'utente corrente non valido"));
    }
    let mut string = ptr::null_mut();
    // SAFETY: SID valido ancora nel buffer; stringa di output allocata da Win32.
    let result = unsafe { ConvertSidToStringSidW(sid, &mut string) };
    let allocation = LocalAllocation(string.cast());
    win32_bool(result)?;
    if allocation.0.is_null() {
        return Err(security_error("stringa SID mancante"));
    }
    let mut wide = Vec::new();
    let mut cursor = string;
    loop {
        // SAFETY: ConvertSidToStringSidW garantisce una stringa terminata NUL;
        // LocalAllocation la mantiene viva per l'intera lettura.
        let unit = unsafe { *cursor };
        if unit == 0 {
            break;
        }
        wide.push(unit);
        // SAFETY: dopo un carattere non NUL esiste almeno il successivo terminatore.
        cursor = unsafe { cursor.add(1) };
    }
    String::from_utf16(&wide).map_err(|_| security_error("stringa SID non valida"))
}

fn security_error(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

fn win32_bool(result: windows_sys::core::BOOL) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn win32_status(status: u32) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::io::Write;
    use windows_sys::Win32::Security::Authorization::{ConvertStringSidToSidW, SetSecurityInfo};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, IsWellKnownSid, PROTECTED_DACL_SECURITY_INFORMATION, WinLocalSystemSid,
    };
    use windows_sys::Win32::Storage::FileSystem::{FILE_ALL_ACCESS, WRITE_DAC};
    use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;

    fn open_original(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .access_mode(GENERIC_READ | GENERIC_WRITE | READ_CONTROL | WRITE_DAC)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(path)
    }

    fn set_dacl(file: &File, sddl: &str) -> io::Result<()> {
        let descriptor = SecurityDescriptor::from_sddl(sddl)?;
        // SAFETY: il descrittore possiede la DACL per tutta la chiamata.
        let status = unsafe {
            SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                descriptor.dacl()?,
                ptr::null_mut(),
            )
        };
        win32_status(status)
    }

    fn broad_directory() -> io::Result<tempfile::TempDir> {
        let directory = tempfile::tempdir()?;
        let handle = OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(directory.path())?;
        // Fixture indipendente: Everyone eredita pieno accesso ai figli.
        set_dacl(&handle, "D:P(A;OICI;FA;;;WD)")?;
        Ok(directory)
    }

    fn private_original(directory: &Path) -> io::Result<(std::path::PathBuf, File)> {
        let path = directory.join("originale.txt");
        std::fs::write(&path, b"originale")?;
        let file = open_original(&path)?;
        let user = current_user_sid()?;
        set_dacl(&file, &format!("D:P(A;;FA;;;{user})(A;;FA;;;SY)"))?;
        Ok((path, file))
    }

    #[test]
    fn temporaneo_privato_sotto_directory_accessibile_a_everyone() -> io::Result<()> {
        let directory = broad_directory()?;
        let (_path, original) = private_original(directory.path())?;
        let expected = SecurityDescriptor::from_file(&original)?;
        let temp = temporary(directory.path(), Some(&original))?;
        let actual = SecurityDescriptor::from_file(temp.as_file())?;
        assert!(actual.is_protected()?);
        assert_eq!(actual.entries()?, expected.entries()?);
        assert_eq!(actual.entries()?.len(), 2);
        Ok(())
    }

    #[test]
    fn errore_read_control_non_crea_temporanei_e_non_tocca_originale() -> io::Result<()> {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_READ_DATA, FILE_WRITE_DATA, SYNCHRONIZE,
        };
        let directory = broad_directory()?;
        let (path, original) = private_original(directory.path())?;
        let before = SecurityDescriptor::from_file(&original)?.entries()?;
        // Il contenuto è accessibile ma l'handle non possiede READ_CONTROL.
        let limited = OpenOptions::new()
            .access_mode(FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&path)?;
        assert!(temporary(directory.path(), Some(&limited)).is_err());
        assert_eq!(std::fs::read(&path)?, b"originale");
        assert_eq!(SecurityDescriptor::from_file(&original)?.entries()?, before);
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn dacl_null_rifiutata_senza_creare_temporanei() -> io::Result<()> {
        let directory = broad_directory()?;
        let (path, original) = private_original(directory.path())?;
        // SAFETY: handle con WRITE_DAC; fixture volutamente universale (NULL).
        win32_status(unsafe {
            SetSecurityInfo(
                original.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        })?;
        assert!(temporary(directory.path(), Some(&original)).is_err());
        assert_eq!(std::fs::read(&path)?, b"originale");
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn nuovo_file_concede_accesso_solo_a_utente_corrente_e_system() -> io::Result<()> {
        let directory = broad_directory()?;
        let mut temp = temporary(directory.path(), None)?;
        let user = current_user_sid()?;
        let user_wide: Vec<u16> = user.encode_utf16().chain(std::iter::once(0)).collect();
        let mut user_sid = ptr::null_mut();
        // SAFETY: stringa UTF-16 terminata; l'output viene liberato con LocalFree.
        win32_bool(unsafe { ConvertStringSidToSidW(user_wide.as_ptr(), &mut user_sid) })?;
        let user_sid = LocalAllocation(user_sid);
        let security = SecurityDescriptor::from_file(temp.as_file())?;
        assert!(security.is_protected()?);
        let dacl = security.dacl()?;
        assert_eq!(security.entries()?.len(), 2);
        let mut saw_user = false;
        let mut saw_system = false;
        for index in 0..2 {
            let mut ace = ptr::null_mut();
            // SAFETY: DACL validata, indice entro il conteggio verificato sopra.
            win32_bool(unsafe { GetAce(dacl, index, &mut ace) })?;
            // SAFETY: GetAce restituisce una ACE valida nella DACL ancora viva.
            let header = unsafe { &*ace.cast::<ACE_HEADER>() };
            assert_eq!(u32::from(header.AceType), ACCESS_ALLOWED_ACE_TYPE);
            // SAFETY: tipo ACCESS_ALLOWED verificato; SidStart è il SID inline.
            let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            assert_eq!(allowed.Mask, FILE_ALL_ACCESS);
            let sid = ptr::addr_of!(allowed.SidStart).cast_mut().cast();
            // SAFETY: entrambi i SID provengono da API Win32 e restano vivi.
            let is_user = unsafe { EqualSid(sid, user_sid.0) } != 0;
            // SAFETY: i SID provengono da API Win32, restano validi e vivi durante questi confronti.
            let is_system = unsafe { IsWellKnownSid(sid, WinLocalSystemSid) } != 0;
            assert!(is_user || is_system, "ACE inattesa nel nuovo file");
            saw_user |= is_user;
            saw_system |= is_system;
        }
        assert!(saw_user && saw_system);
        temp.write_all(b"nuovo")?;
        let destination = directory.path().join("nuovo.txt");
        commit(temp, &destination, false)?;
        assert_eq!(std::fs::read(&destination)?, b"nuovo");
        let persisted = SecurityDescriptor::from_file(&open_original(&destination)?)?;
        assert!(persisted.is_protected()?);
        assert_eq!(persisted.entries()?, security.entries()?);
        Ok(())
    }

    #[test]
    fn replace_conserva_dacl_e_data_di_creazione() -> io::Result<()> {
        let directory = broad_directory()?;
        let (path, original) = private_original(directory.path())?;
        let before = SecurityDescriptor::from_file(&original)?;
        let creation = original.metadata()?.created()?;
        let mut temp = temporary(directory.path(), Some(&original))?;
        temp.write_all(b"nuovo")?;
        commit(temp, &path, true)?;
        assert_eq!(std::fs::read(&path)?, b"nuovo");
        let after_file = open_original(&path)?;
        let after = SecurityDescriptor::from_file(&after_file)?;
        assert_eq!(before.entries()?, after.entries()?);
        assert_eq!(creation, after_file.metadata()?.created()?);
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn abort_e_commit_fallito_lasciano_immutato_originale() -> io::Result<()> {
        let directory = broad_directory()?;
        let (path, original) = private_original(directory.path())?;
        let before = SecurityDescriptor::from_file(&original)?.entries()?;
        let mut temp = temporary(directory.path(), Some(&original))?;
        temp.write_all(b"parziale")?;
        drop(temp);
        assert_eq!(std::fs::read(&path)?, b"originale");
        let mut temp = temporary(directory.path(), Some(&original))?;
        temp.write_all(b"nuovo")?;
        let invalid = directory.path().join("cartella");
        std::fs::create_dir(&invalid)?;
        assert!(commit(temp, &invalid, true).is_err());
        assert_eq!(std::fs::read(&path)?, b"originale");
        assert_eq!(SecurityDescriptor::from_file(&original)?.entries()?, before);
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 2);
        Ok(())
    }

    #[test]
    fn replace_bloccato_da_handle_senza_share_delete_conserva_originale() -> io::Result<()> {
        let directory = broad_directory()?;
        let (path, original) = private_original(directory.path())?;
        let before = SecurityDescriptor::from_file(&original)?.entries()?;
        let blocker = OpenOptions::new()
            .access_mode(GENERIC_READ | GENERIC_WRITE | READ_CONTROL)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&path)?;
        let mut temp = temporary(directory.path(), Some(&original))?;
        temp.write_all(b"nuovo")?;
        let result = commit(temp, &path, true);
        drop(blocker);
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path)?, b"originale");
        assert_eq!(SecurityDescriptor::from_file(&original)?.entries()?, before);
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn readonly_rifiutato_sia_prima_della_scrittura_sia_al_commit() -> io::Result<()> {
        let directory = broad_directory()?;
        let (path, original) = private_original(directory.path())?;
        let temp = temporary(directory.path(), Some(&original))?;
        let permissions = original.metadata()?.permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&path, readonly)?;
        let create_result = temporary(directory.path(), Some(&original));
        let commit_result = commit(temp, &path, true);
        std::fs::set_permissions(&path, permissions)?;
        assert_eq!(
            create_result.err().map(|error| error.kind()),
            Some(io::ErrorKind::PermissionDenied)
        );
        assert_eq!(
            commit_result.err().map(|error| error.kind()),
            Some(io::ErrorKind::PermissionDenied)
        );
        assert_eq!(std::fs::read(&path)?, b"originale");
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn nuovo_file_non_sovrascrive_destinazione_comparsa_nel_frattempo() -> io::Result<()> {
        let directory = broad_directory()?;
        let mut temp = temporary(directory.path(), None)?;
        temp.write_all(b"nuovo")?;
        let path = directory.path().join("concorrente.txt");
        std::fs::write(&path, b"originale concorrente")?;
        assert!(commit(temp, &path, false).is_err());
        assert_eq!(std::fs::read(&path)?, b"originale concorrente");
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn nuova_directory_privata_sotto_parent_everyone() -> io::Result<()> {
        use crate::file_write::{DirectoryCreationPhase, create_private_directory};
        let parent = broad_directory()?;
        let path = parent.path().join("privata");
        let (phase, result) = create_private_directory(&path).into_parts();
        assert_eq!(phase, DirectoryCreationPhase::Created);
        result?;
        let handle = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES | READ_CONTROL)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&path)?;
        let user = current_user_sid()?;
        let expected =
            SecurityDescriptor::from_sddl(&format!("O:{user}D:P(A;;FA;;;{user})(A;;FA;;;SY)"))?;
        let actual = SecurityDescriptor::from_file(&handle)?;
        assert!(actual.is_protected()?);
        assert_eq!(actual.entries()?, expected.entries()?);
        expected.verify_identity(&actual)?;
        assert_eq!(std::fs::read_dir(&path)?.count(), 0);
        drop(handle);
        std::fs::remove_dir(&path)?;
        Ok(())
    }
    #[test]
    fn directory_pre_esistente_non_cambia_dacl_o_contenuto() -> io::Result<()> {
        use crate::file_write::{DirectoryCreationPhase, create_private_directory};
        let directory = broad_directory()?;
        let path = directory.path().join("esistente");
        std::fs::create_dir(&path)?;
        std::fs::write(path.join("file"), b"originale")?;
        let handle = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES | READ_CONTROL)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&path)?;
        let before = SecurityDescriptor::from_file(&handle)?.entries()?;
        let (phase, result) = create_private_directory(&path).into_parts();
        assert_eq!(phase, DirectoryCreationPhase::CreationUnconfirmed);
        assert_eq!(
            result.err().map(|e| e.kind()),
            Some(io::ErrorKind::AlreadyExists)
        );
        assert_eq!(SecurityDescriptor::from_file(&handle)?.entries()?, before);
        assert_eq!(std::fs::read(path.join("file"))?, b"originale");
        Ok(())
    }
    #[test]
    fn nul_nel_percorso_rifiutato_con_cleanup() -> io::Result<()> {
        use std::os::windows::ffi::OsStringExt;
        let directory = broad_directory()?;
        let bad = directory
            .path()
            .join(std::ffi::OsString::from_wide(&[97, 0, 98]));
        assert_eq!(
            temporary(&bad, None).err().map(|error| error.kind()),
            Some(io::ErrorKind::InvalidInput)
        );
        let temp = temporary(directory.path(), None)?;
        assert_eq!(
            commit(temp, &bad, false).err().map(|error| error.kind()),
            Some(io::ErrorKind::InvalidInput)
        );
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);
        Ok(())
    }
}
