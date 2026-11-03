//! Il Cestino di sistema: spostare un file dove l'utente lo ritrova.
//!
//! La destinazione è il Cestino del desktop, non una cancellazione: il file
//! resta recuperabile. Per questo l'esito distingue il Cestino raggiunto dal
//! caso in cui il Cestino non è raggiungibile (un altro volume, o nessun
//! Cestino configurato): lì spostare richiederebbe una cancellazione
//! definitiva, che non è compito di questa funzione.
//!
//! Le directory si spostano con una rinomina nativa senza sovrascritture.
//! Non si attraversano i link e non si copia o cancella ricorsivamente.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

/// Che cosa è successo al file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrashOutcome {
    /// Il file è nel Cestino.
    Trashed,
    /// Il Cestino non è raggiungibile da qui: spostare richiederebbe una
    /// cancellazione definitiva, che va chiesta all'utente.
    NeedsPermanentConfirmation,
}

/// Errore: il file non è finito nel Cestino.
#[derive(Debug, thiserror::Error)]
pub enum TrashError {
    #[error("I/O su {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("non è un file regolare o una cartella (i link non sono ammessi): {0}")]
    NotAFile(PathBuf),
    #[error("percorso non assoluto: {0}")]
    NotAbsolute(PathBuf),
}

/// Sposta un file o una cartella nel Cestino di sistema.
///
/// Il file sparisce dal suo posto ma resta recuperabile nel Cestino; non è
/// una cancellazione definitiva e non lo simula.
pub fn trash_path(path: &Path) -> Result<TrashOutcome, TrashError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| TrashError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !(metadata.is_file() || metadata.is_dir()) || metadata.file_type().is_symlink() {
        return Err(TrashError::NotAFile(path.to_path_buf()));
    }
    #[cfg(windows)]
    {
        // SHFileOperationW non garantisce il recupero di una directory su
        // volumi senza Cestino. Finché manca il backend con tale garanzia,
        // rifiutiamo senza tentare una cancellazione della cartella.
        if metadata.is_dir() {
            return Ok(TrashOutcome::NeedsPermanentConfirmation);
        }
        windows_trash(path)
    }
    #[cfg(not(windows))]
    {
        unix_trash(path)
    }
}

/// Individua il Cestino utente solo quando la radice è assoluta e affidabile.
#[cfg(not(windows))]
fn root_from(
    xdg_data_home: Option<OsString>,
    home: Option<OsString>,
    macos: bool,
) -> Option<PathBuf> {
    let home = home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    if macos {
        return home.map(|path| path.join(".Trash"));
    }
    let base = xdg_data_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|path| path.join(".local").join("share")))?;
    Some(base.join("Trash"))
}

/// Codifica il percorso assoluto nel formato `Path=` di `.trashinfo`.
#[cfg(not(windows))]
fn encode_trash_path(path: &Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let encoded_path = path.to_string_lossy();
    #[cfg(not(unix))]
    let bytes = encoded_path.as_bytes();
    let mut encoded = String::new();
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Convert Unix seconds to a Gregorian UTC date without a timezone library.
#[cfg(not(windows))]
fn civil_from_unix(secs: u64) -> (u64, u32, u32, u32, u32, u32) {
    let days = i64::try_from(secs / 86_400).unwrap_or(i64::MAX / 2);
    let time = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = u64::try_from(yoe + era * 400 + i64::from(month <= 2)).unwrap_or(1970);
    let hour = u32::try_from(time / 3_600).unwrap_or(0);
    let minute = u32::try_from(time % 3_600 / 60).unwrap_or(0);
    let second = u32::try_from(time % 60).unwrap_or(0);
    (year, month, day, hour, minute, second)
}

#[cfg(not(windows))]
fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Rinomina una directory solo se il destinatario è libero, in un solo
/// passo del filesystem. Nessuna scansione del contenuto o dei link.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[allow(unsafe_code)]
fn sposta_cartella(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(ErrorKind::InvalidInput, "NUL nel percorso"))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(ErrorKind::InvalidInput, "NUL nel percorso"))?;
    // SAFETY: entrambi i percorsi sono stringhe C valide per l'intera chiamata;
    // i flag richiedono di fallire se il destinatario esiste già.
    let result = unsafe {
        #[cfg(target_os = "linux")]
        {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        }
        #[cfg(target_os = "macos")]
        {
            libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL)
        }
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn sposta_cartella(_source: &Path, _destination: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        ErrorKind::Unsupported,
        "Cestino per cartelle non disponibile su questo sistema",
    ))
}

/// Move one file into the chosen trash root. Linux writes the freedesktop
/// metadata; macOS uses its `~/.Trash` convention.
#[cfg(not(windows))]
fn trash_into(
    root: &Path,
    path: &Path,
    now_secs: u64,
    freedesktop: bool,
) -> Result<TrashOutcome, TrashError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| TrashError::Io {
        path: path.to_owned(),
        source,
    })?;
    if !(metadata.is_file() || metadata.is_dir()) || metadata.file_type().is_symlink() {
        return Err(TrashError::NotAFile(path.to_owned()));
    }
    let directory = metadata.is_dir();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| TrashError::NotAFile(path.to_path_buf()))?;
    let files = if freedesktop {
        root.join("files")
    } else {
        root.to_owned()
    };
    fs::create_dir_all(&files).map_err(|source| TrashError::Io {
        path: files.clone(),
        source,
    })?;
    let info_dir = root.join("info");
    if freedesktop {
        fs::create_dir_all(&info_dir).map_err(|source| TrashError::Io {
            path: info_dir.clone(),
            source,
        })?;
    }

    for suffix in 0_u32..10_000 {
        let candidate = if suffix == 0 {
            name.to_owned()
        } else {
            format!("{name}.{suffix}")
        };
        let destination = files.join(&candidate);
        let info_path = info_dir.join(format!("{candidate}.trashinfo"));
        let mut info_file = if freedesktop {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&info_path)
            {
                Ok(file) => Some(file),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(TrashError::Io {
                        path: info_path,
                        source,
                    });
                }
            }
        } else {
            None
        };

        if let Some(file) = info_file.as_mut() {
            let (year, month, day, hour, minute, second) = civil_from_unix(now_secs);
            let metadata = format!(
                "[Trash Info]\nPath={}\nDeletionDate={year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}\n",
                encode_trash_path(path)
            );
            if let Err(source) = file
                .write_all(metadata.as_bytes())
                .and_then(|()| file.sync_all())
            {
                let _ = fs::remove_file(&info_path);
                return Err(TrashError::Io {
                    path: info_path,
                    source,
                });
            }
        }

        // hard_link is an atomic no-clobber operation. A prior exists() check
        // followed by rename could overwrite another file under a race.
        let moved = if directory {
            sposta_cartella(path, &destination)
        } else {
            fs::hard_link(path, &destination)
        };
        match moved {
            Ok(()) => {
                if let Err(source) = if directory {
                    Ok(())
                } else {
                    fs::remove_file(path)
                } {
                    let _ = fs::remove_file(&destination);
                    if freedesktop {
                        let _ = fs::remove_file(&info_path);
                    }
                    return Err(TrashError::Io {
                        path: path.to_path_buf(),
                        source,
                    });
                }
                return Ok(TrashOutcome::Trashed);
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                if freedesktop {
                    let _ = fs::remove_file(&info_path);
                }
            }
            Err(error) => {
                if freedesktop {
                    let _ = fs::remove_file(&info_path);
                }
                if error.kind() == ErrorKind::CrossesDevices {
                    return Ok(TrashOutcome::NeedsPermanentConfirmation);
                }
                return Err(TrashError::Io {
                    path: path.to_path_buf(),
                    source: error,
                });
            }
        }
    }
    Err(TrashError::Io {
        path: root.to_path_buf(),
        source: std::io::Error::new(
            ErrorKind::AlreadyExists,
            "troppi file con lo stesso nome nel Cestino",
        ),
    })
}

/// Il Cestino sui sistemi che non sono Windows.
#[cfg(not(windows))]
fn unix_trash(path: &Path) -> Result<TrashOutcome, TrashError> {
    let Some(root) = root_from(
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
        cfg!(target_os = "macos"),
    ) else {
        return Ok(TrashOutcome::NeedsPermanentConfirmation);
    };
    trash_into(&root, path, now_unix_secs(), !cfg!(target_os = "macos"))
}

/// Il percorso ordinario che la shell di Windows accetta: `canonicalize`
/// restituisce la forma `\\?\`, che `SHFileOperationW` non capisce.
#[cfg(windows)]
fn ordinary_windows_path(path: &Path) -> Result<PathBuf, TrashError> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    let verbatim = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    let verbatim_unc = [
        b'\\' as u16,
        b'\\' as u16,
        b'?' as u16,
        b'\\' as u16,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        b'\\' as u16,
    ];
    let ordinary = if wide.starts_with(&verbatim_unc) {
        let mut converted = vec![b'\\' as u16, b'\\' as u16];
        converted.extend_from_slice(wide.get(verbatim_unc.len()..).unwrap_or_default());
        converted
    } else if wide.starts_with(&verbatim) {
        wide.get(verbatim.len()..).unwrap_or_default().to_vec()
    } else {
        wide
    };
    let ordinary = PathBuf::from(OsString::from_wide(&ordinary));
    if ordinary.is_absolute() {
        Ok(ordinary)
    } else {
        Err(TrashError::NotAbsolute(path.to_path_buf()))
    }
}

/// Il Cestino su Windows: la shell con `FOF_ALLOWUNDO` mette nel Cestino.
#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_trash(path: &Path) -> Result<TrashOutcome, TrashError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };

    let shell_path = ordinary_windows_path(path)?;
    let mut source: Vec<u16> = shell_path.as_os_str().encode_wide().collect();
    source.extend([0, 0]);
    let mut operation = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: source.as_ptr(),
        pTo: std::ptr::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    // SAFETY: SHFileOperationW receives a valid structure and a double-NUL-terminated path list.
    let status = unsafe { SHFileOperationW(&mut operation) };
    if status == 0 && operation.fAnyOperationsAborted == 0 {
        Ok(TrashOutcome::Trashed)
    } else {
        Err(TrashError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other(format!("shell status {status}")),
        })
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    #![allow(clippy::unwrap_used)]
    use super::ordinary_windows_path;
    use std::fs;

    #[test]
    fn il_percorso_ordinario_non_ha_il_prefisso_verbatim() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.txt");
        fs::write(&path, b"test").unwrap();
        let canonical = fs::canonicalize(path).unwrap();

        let shell_path = ordinary_windows_path(&canonical).unwrap();

        assert!(shell_path.is_absolute());
        assert!(
            !shell_path
                .as_os_str()
                .to_string_lossy()
                .starts_with(r"\\?\")
        );
    }
}

/// `std::io::Error` non e' `Clone`: qui si copiano tipo e messaggio, che
/// sono cio che l'utente e il log leggono. Il codice di sistema grezzo
/// (`raw_os_error`) non viene riportato.
impl Clone for TrashError {
    fn clone(&self) -> Self {
        match self {
            Self::Io { path, source } => Self::Io {
                path: path.clone(),
                source: std::io::Error::new(source.kind(), source.to_string()),
            },
            Self::NotAFile(path) => Self::NotAFile(path.clone()),
            Self::NotAbsolute(path) => Self::NotAbsolute(path.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::{TrashOutcome, civil_from_unix, encode_trash_path, root_from, trash_into};
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn assoluta(nome: &str) -> PathBuf {
        std::env::temp_dir().join(nome)
    }

    #[test]
    fn la_data_civile_e_quella_giusta() {
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil_from_unix(951_782_400), (2000, 2, 29, 0, 0, 0));
        assert_eq!(civil_from_unix(1_709_210_096), (2024, 2, 29, 12, 34, 56));
        assert_eq!(civil_from_unix(86_399), (1970, 1, 1, 23, 59, 59));
    }

    #[test]
    fn il_percorso_si_codifica_come_vuole_freedesktop() {
        assert_eq!(
            encode_trash_path(Path::new("/home/utente/file/è a#1.txt")),
            "/home/utente/file/%C3%A8%20a%231.txt"
        );
        assert_eq!(encode_trash_path(Path::new("/a-b_c.~x/y")), "/a-b_c.~x/y");
    }

    #[cfg(unix)]
    #[test]
    fn i_byte_non_utf8_del_percorso_non_vengono_sostituiti() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(OsStr::from_bytes(b"/home/utente\xff/file.txt"));
        assert_eq!(encode_trash_path(path), "/home/utente%FF/file.txt");
    }

    #[test]
    fn la_radice_del_cestino_segue_xdg_poi_home() {
        let home = assoluta("home-prova");
        let xdg = assoluta("dati-xdg");
        let dati = home.join(".local").join("share").join("Trash");
        assert_eq!(
            root_from(
                Some(xdg.clone().into_os_string()),
                Some(home.clone().into_os_string()),
                false
            ),
            Some(xdg.join("Trash"))
        );
        assert_eq!(
            root_from(None, Some(home.clone().into_os_string()), false),
            Some(dati.clone())
        );
        assert_eq!(
            root_from(
                Some(OsString::new()),
                Some(home.clone().into_os_string()),
                false
            ),
            Some(dati.clone())
        );
        assert_eq!(
            root_from(
                Some("relativo/xdg".into()),
                Some(home.clone().into_os_string()),
                false
            ),
            Some(dati)
        );
        assert_eq!(
            root_from(None, Some(home.clone().into_os_string()), true),
            Some(home.join(".Trash"))
        );
        assert_eq!(root_from(None, None, false), None);
        assert_eq!(root_from(None, Some("relativa".into()), true), None);
    }

    #[test]
    fn freedesktop_sposta_il_file_e_scrive_il_trashinfo() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file").join("idea è.txt");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "contenuto").unwrap();
        let cestino = dir.path().join("Trash");
        assert_eq!(
            trash_into(&cestino, &file, 1_709_210_096, true).unwrap(),
            TrashOutcome::Trashed
        );
        assert!(!file.exists());
        assert_eq!(
            fs::read_to_string(cestino.join("files").join("idea è.txt")).unwrap(),
            "contenuto"
        );
        let info = fs::read_to_string(cestino.join("info").join("idea è.txt.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath="));
        assert!(info.contains("%C3%A8"));
        assert!(info.ends_with("DeletionDate=2024-02-29T12:34:56\n"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn la_cartella_va_nel_cestino_con_contenuto_e_link_intatti_senza_sovrascrivere() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cartella è");
        let trash = dir.path().join("Trash");
        let external = dir.path().join("esterno.txt");
        fs::write(&external, "esterno").unwrap();
        fs::create_dir_all(trash.join("files/cartella è")).unwrap();
        fs::write(trash.join("files/cartella è/originale"), "preserva").unwrap();
        fs::create_dir_all(source.join("sotto")).unwrap();
        fs::write(source.join("sotto/file.txt"), "contenuto").unwrap();
        std::os::unix::fs::symlink(&external, source.join("link")).unwrap();
        assert_eq!(
            trash_into(&trash, &source, 0, true).unwrap(),
            TrashOutcome::Trashed
        );
        assert!(!source.exists());
        let destination = trash.join("files/cartella è.1");
        assert_eq!(
            fs::read_to_string(destination.join("sotto/file.txt")).unwrap(),
            "contenuto"
        );
        assert!(
            fs::symlink_metadata(destination.join("link"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_link(destination.join("link")).unwrap(), external);
        assert_eq!(fs::read_to_string(&external).unwrap(), "esterno");
        assert_eq!(
            fs::read_to_string(trash.join("files/cartella è/originale")).unwrap(),
            "preserva"
        );
        assert!(!trash.join("info/cartella è.trashinfo").exists());
        assert!(
            fs::read_to_string(trash.join("info/cartella è.1.trashinfo"))
                .unwrap()
                .contains("cartella%20%C3%A8")
        );
    }

    #[cfg(unix)]
    #[test]
    fn il_link_selezionato_non_porta_nel_cestino_la_cartella_destinataria() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("contenuti");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("a.txt"), "preserva").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(matches!(
            super::trash_path(&link),
            Err(super::TrashError::NotAFile(_))
        ));
        assert!(link.is_symlink());
        assert_eq!(
            fs::read_to_string(target.join("a.txt")).unwrap(),
            "preserva"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn la_cartella_su_un_altro_volume_non_viene_cancellata() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let Ok(trash) = tempfile::tempdir_in("/dev/shm") else {
            eprintln!("SALTATO: /dev/shm non disponibile per la prova di volumi distinti");
            return;
        };
        if fs::metadata(dir.path()).unwrap().dev() == fs::metadata(trash.path()).unwrap().dev() {
            eprintln!("SALTATO: /dev/shm e sorgente sono sullo stesso volume");
            return;
        }
        let folder = dir.path().join("moduli");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("a.txt"), "preserva").unwrap();
        assert_eq!(
            trash_into(trash.path(), &folder, 0, true).unwrap(),
            TrashOutcome::NeedsPermanentConfirmation
        );
        assert_eq!(
            fs::read_to_string(folder.join("a.txt")).unwrap(),
            "preserva"
        );
        assert!(!trash.path().join("files/moduli").exists());
        assert!(!trash.path().join("info/moduli.trashinfo").exists());
    }

    #[test]
    fn due_file_con_lo_stesso_nome_non_si_sovrascrivono() {
        let dir = tempfile::tempdir().unwrap();
        let cestino = dir.path().join("Trash");
        for corpo in ["primo", "secondo"] {
            let file = dir.path().join("a.txt");
            fs::write(&file, corpo).unwrap();
            assert_eq!(
                trash_into(&cestino, &file, 0, true).unwrap(),
                TrashOutcome::Trashed
            );
        }
        assert_eq!(
            fs::read_to_string(cestino.join("files").join("a.txt")).unwrap(),
            "primo"
        );
        assert_eq!(
            fs::read_to_string(cestino.join("files").join("a.txt.1")).unwrap(),
            "secondo"
        );
        assert!(cestino.join("info").join("a.txt.1.trashinfo").exists());
    }

    #[test]
    fn su_macos_il_file_va_nel_cestino_senza_trashinfo() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("b.txt");
        fs::write(&file, "x").unwrap();
        let cestino = dir.path().join(".Trash");
        assert_eq!(
            trash_into(&cestino, &file, 0, false).unwrap(),
            TrashOutcome::Trashed
        );
        assert!(cestino.join("b.txt").exists());
        assert!(!cestino.join("info").exists());
    }

    #[test]
    fn un_file_che_non_esiste_e_un_errore_e_non_lascia_il_trashinfo() {
        let dir = tempfile::tempdir().unwrap();
        let cestino = dir.path().join("Trash");
        let assente = dir.path().join("assente.txt");
        let esito = trash_into(&cestino, &assente, 0, true);
        assert!(esito.is_err());
        let rimasti = fs::read_dir(cestino.join("info"))
            .map(|d| d.count())
            .unwrap_or(0);
        assert_eq!(rimasti, 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn un_file_su_un_altro_volume_resta_al_suo_posto() {
        let Some(cestino) = tempfile::Builder::new()
            .prefix("lume-trash-volume-")
            .tempdir_in("/dev/shm")
            .ok()
        else {
            return;
        };
        let sorgente_dir = tempfile::tempdir().unwrap();
        let file = sorgente_dir.path().join("file.txt");
        fs::write(&file, "contenuto").unwrap();

        let esito = trash_into(cestino.path(), &file, 0, true).unwrap();

        if esito == TrashOutcome::NeedsPermanentConfirmation {
            assert!(file.is_file());
            assert!(!cestino.path().join("info/file.txt.trashinfo").exists());
        }
    }
}
