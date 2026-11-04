use crate::{Note, NoteError, NoteId, NoteSummary, ParsedNote, parse, serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_NOTE_BYTES: u64 = 8 * 1024 * 1024;
pub const SCAN_PREFIX_BYTES: u64 = 64 * 1024;
static SAVE_GENERATION: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
thread_local! {
    static TEST_FAULT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIdentity {
    pub len: u64,
    pub modified_unix_nanos: u128,
    fingerprint: u64,
}

#[derive(Debug, Clone)]
pub struct LoadedNote {
    pub note: Note,
    pub identity: FileIdentity,
    pub original_bytes: Vec<u8>,
    pub read_only_error: Option<NoteError>,
}

/// Esito del salvataggio che può anche cambiare nome al file.
#[derive(Debug, Clone)]
pub struct RetitledNote {
    pub loaded: LoadedNote,
    /// Il contenuto è stato salvato con il nome precedente.
    pub rename_error: Option<NoteError>,
    /// Il file è pubblicato, ma la cartella degli appunti non è stata
    /// sincronizzata: da mostrare, non è un salvataggio fallito.
    pub sync_error: Option<NoteError>,
}

impl LoadedNote {
    #[must_use]
    pub fn path(&self) -> &Path {
        self.note.path.as_deref().unwrap_or_else(|| Path::new(""))
    }

    #[must_use]
    pub fn summary(&self) -> NoteSummary {
        self.note.summary(self.read_only_error.as_ref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteDisposition {
    Trashed,
    NeedsPermanentConfirmation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveIsolation {
    WindowsExclusiveTargetHandle,
    BestEffortIdentityCheck,
}

#[derive(Debug, Clone)]
pub struct NoteStore {
    root: PathBuf,
}

impl NoteStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, NoteError> {
        fs::create_dir_all(root.as_ref()).map_err(io_error)?;
        let root = lume_core::canonical_path(root.as_ref()).map_err(io_error)?;
        Ok(Self { root })
    }

    pub fn scan(&self) -> Result<Vec<NoteSummary>, NoteError> {
        let mut summaries = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(io_error)?;
            if !file_type.is_file() || file_type.is_symlink() || !is_markdown(&path) {
                continue;
            }
            match self.scan_entry(&path) {
                Ok(summary) => summaries.push(summary),
                Err(error) => summaries.push(NoteSummary {
                    id: NoteId::new(),
                    title: path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("Senza titolo")
                        .to_owned(),
                    updated_unix_ms: 0,
                    preview: String::new(),
                    path,
                    error: Some(error.to_string()),
                    links: Vec::new(),
                }),
            }
        }
        summaries.sort_by(|left, right| {
            right
                .updated_unix_ms
                .cmp(&left.updated_unix_ms)
                .then_with(|| left.path.cmp(&right.path))
        });
        Ok(summaries)
    }

    fn scan_entry(&self, path: &Path) -> Result<NoteSummary, NoteError> {
        let path = self.confined_existing_file(path)?;
        let len = fs::metadata(&path).map_err(io_error)?.len();
        if len <= SCAN_PREFIX_BYTES {
            return self.load(&path).map(|loaded| loaded.summary());
        }
        let prefix = read_prefix(&path)?;
        if len > MAX_NOTE_BYTES {
            let error = NoteError::NoteTooLarge {
                path: path.clone(),
                limit: MAX_NOTE_BYTES,
            };
            return Ok(oversized_note(&path, &prefix).summary(Some(&error)));
        }
        let parsed = parse(&path, &prefix)?;
        if parsed.read_only_error.is_some() {
            return self.load(&path).map(|loaded| loaded.summary());
        }
        Ok(parsed.note.summary(None))
    }

    #[must_use]
    pub const fn save_isolation(&self) -> SaveIsolation {
        if cfg!(windows) {
            SaveIsolation::WindowsExclusiveTargetHandle
        } else {
            SaveIsolation::BestEffortIdentityCheck
        }
    }

    pub fn load(&self, path: impl AsRef<Path>) -> Result<LoadedNote, NoteError> {
        let path = self.confined_existing_file(path.as_ref())?;
        let (bytes, oversized) = read_bounded(&path)?;
        let identity = identity_for(&path, &bytes)?;
        if oversized {
            let note = oversized_note(&path, &bytes);
            return Ok(LoadedNote {
                note,
                identity,
                original_bytes: bytes,
                read_only_error: Some(NoteError::NoteTooLarge {
                    path,
                    limit: MAX_NOTE_BYTES,
                }),
            });
        }
        let ParsedNote {
            note,
            original_bytes,
            read_only_error,
            ..
        } = parse(&path, &bytes)?;
        Ok(LoadedNote {
            note,
            identity,
            original_bytes,
            read_only_error,
        })
    }

    pub fn create(&self, title: &str) -> Result<LoadedNote, NoteError> {
        self.create_reporting(title).map(|(loaded, _)| loaded)
    }

    /// Come `create`, e restituisce anche l'errore di sincronizzazione della
    /// cartella: la nota è già pubblicata, l'errore è un avviso.
    pub fn create_reporting(
        &self,
        title: &str,
    ) -> Result<(LoadedNote, Option<NoteError>), NoteError> {
        let mut note = Note::empty(NoteId::new());
        note.header.title = title.to_owned();
        self.publish_created(note)
    }

    /// Crea una nota nuova conservando testo e collegamenti di `loaded`.
    pub fn save_copy(&self, loaded: &LoadedNote) -> Result<LoadedNote, NoteError> {
        self.save_copy_reporting(loaded).map(|(copy, _)| copy)
    }

    /// Come `save_copy`, con l'errore di sincronizzazione della cartella.
    pub fn save_copy_reporting(
        &self,
        loaded: &LoadedNote,
    ) -> Result<(LoadedNote, Option<NoteError>), NoteError> {
        let mut copy = loaded.note.clone();
        copy.header.id = NoteId::new();
        copy.header.title = format!("{} (copia)", loaded.note.header.title);
        self.publish_created(copy)
    }

    fn publish_created(
        &self,
        mut note: Note,
    ) -> Result<(LoadedNote, Option<NoteError>), NoteError> {
        note.header.updated_unix_ms = now_unix_ms();
        let path = self.unique_path(&note.header.title, &note.header.id);
        note.path = Some(path.clone());
        let bytes = serialize(&note)?;
        let temporary = self.write_synced_temporary(&path, &bytes)?;
        if let Err(error) = test_fault(1) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = publish_new(&temporary, &path) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        let _ = fs::remove_file(&temporary);
        let sync_error = self.sync_root();
        Ok((self.load(path)?, sync_error))
    }

    pub fn save(&self, loaded: &LoadedNote) -> Result<LoadedNote, NoteError> {
        self.save_authorized(loaded, |_| true)
    }

    /// Come `save`, con l'errore di sincronizzazione della cartella.
    pub fn save_reporting(
        &self,
        loaded: &LoadedNote,
    ) -> Result<(LoadedNote, Option<NoteError>), NoteError> {
        self.save_before_publish(loaded, |_| true)
    }

    /// Sincronizza la cartella degli appunti dopo rename/link al suo interno.
    fn sync_root(&self) -> Option<NoteError> {
        lume_core::file_write::sync_directory(&self.root).map(io_error)
    }

    /// Prepara e sincronizza il temporaneo, poi chiede l'autorizzazione
    /// consumante immediatamente prima della sostituzione del file.
    pub fn save_authorized(
        &self,
        loaded: &LoadedNote,
        authorize: impl FnMut(&Path) -> bool,
    ) -> Result<LoadedNote, NoteError> {
        self.save_before_publish(loaded, authorize)
            .map(|(loaded, _)| loaded)
    }

    /// Restituisce anche l'errore di sincronizzazione della cartella.
    fn save_before_publish(
        &self,
        loaded: &LoadedNote,
        mut authorize: impl FnMut(&Path) -> bool,
    ) -> Result<(LoadedNote, Option<NoteError>), NoteError> {
        if loaded.read_only_error.is_some() {
            return Err(NoteError::ReadOnly {
                path: loaded.path().to_owned(),
            });
        }
        let mut note_to_save = loaded.note.clone();
        note_to_save.header.updated_unix_ms = now_unix_ms();
        let bytes = serialize(&note_to_save)?;
        let path = self.confined_existing_file(loaded.path())?;
        let temporary = self.write_synced_temporary(&path, &bytes)?;
        if !authorize(&path) {
            let cleanup_error = fs::remove_file(&temporary)
                .err()
                .map(|error| error.to_string());
            return Err(NoteError::Cancelled { cleanup_error });
        }
        let result = replace_if_unchanged(&temporary, &path, &loaded.identity);
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        let sync_error = self.sync_root();
        Ok((self.load(path)?, sync_error))
    }

    pub fn rename_for_title(
        &self,
        loaded: &LoadedNote,
        title: &str,
    ) -> Result<LoadedNote, NoteError> {
        self.rename_for_title_authorized(loaded, title, |_| true)
    }

    /// Stagia la nuova nota e chiede l'autorizzazione prima di pubblicarla.
    /// Dopo la pubblicazione, la rimozione della sorgente è parte della stessa
    /// operazione irreversibile e il metodo conserva il proprio recovery.
    pub fn rename_for_title_authorized(
        &self,
        loaded: &LoadedNote,
        title: &str,
        authorize: impl FnMut(&Path) -> bool,
    ) -> Result<LoadedNote, NoteError> {
        self.rename_before_publish(loaded, title, authorize)
            .map(|(loaded, _)| loaded)
    }

    /// Restituisce anche l'errore di sincronizzazione della cartella.
    fn rename_before_publish(
        &self,
        loaded: &LoadedNote,
        title: &str,
        mut authorize: impl FnMut(&Path) -> bool,
    ) -> Result<(LoadedNote, Option<NoteError>), NoteError> {
        let old_path = self.confined_existing_file(loaded.path())?;
        let old_target = open_rename_source(&old_path, &loaded.identity)?;
        if loaded.read_only_error.is_some() {
            return Err(NoteError::ReadOnly { path: old_path });
        }
        let new_path = self.unique_path(title, &loaded.note.header.id);
        let mut renamed_note = loaded.note.clone();
        renamed_note.path = Some(new_path.clone());
        renamed_note.header.title = title.to_owned();
        renamed_note.header.updated_unix_ms = now_unix_ms();
        let bytes = serialize(&renamed_note)?;
        let temporary = self.write_synced_temporary(&new_path, &bytes)?;
        if let Err(error) = test_fault(2) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if !authorize(&new_path) {
            let cleanup_error = fs::remove_file(&temporary)
                .err()
                .map(|error| error.to_string());
            return Err(NoteError::Cancelled { cleanup_error });
        }
        if let Err(error) = publish_new(&temporary, &new_path) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        let _ = fs::remove_file(&temporary);
        if let Err(error) = test_fault(3) {
            let _ = fs::remove_file(&new_path);
            return Err(error);
        }
        if let Err(error) = remove_rename_source(old_target, &old_path, &loaded.identity) {
            let rollback = fs::remove_file(&new_path);
            return Err(if rollback.is_ok() {
                error
            } else {
                NoteError::Io(format!(
                    "pulizia dopo la rinomina fallita: {error}; anche il ripristino è fallito"
                ))
            });
        }
        let sync_error = self.sync_root();
        Ok((self.load(new_path)?, sync_error))
    }

    /// Salva il contenuto e allinea il nome file al titolo, se necessario.
    /// Un errore di rinomina non impedisce di salvare sotto il nome attuale;
    /// i conflitti su disco restano invece errori bloccanti.
    pub fn save_with_title(
        &self,
        loaded: &LoadedNote,
        title: &str,
    ) -> Result<RetitledNote, NoteError> {
        self.save_with_title_authorized(loaded, title, |_| true)
    }

    /// Variante che attraversa un solo gate prima del primo effetto
    /// filesystem. Eventuali recovery/fallback successivi restano raccolti
    /// dalla stessa invocazione e non chiedono una seconda autorizzazione.
    pub fn save_with_title_authorized(
        &self,
        loaded: &LoadedNote,
        title: &str,
        mut authorize: impl FnMut(&Path) -> bool,
    ) -> Result<RetitledNote, NoteError> {
        if slugify(title) == slugify(&loaded.note.header.title) {
            let mut retitled = loaded.clone();
            retitled.note.header.title = title.to_owned();
            let (loaded, sync_error) = self.save_before_publish(&retitled, authorize)?;
            return Ok(RetitledNote {
                loaded,
                rename_error: None,
                sync_error,
            });
        }
        match self.rename_before_publish(loaded, title, &mut authorize) {
            Ok((loaded, sync_error)) => Ok(RetitledNote {
                loaded,
                rename_error: None,
                sync_error,
            }),
            Err(conflict @ (NoteError::Conflict { .. } | NoteError::Cancelled { .. })) => {
                Err(conflict)
            }
            Err(rename_error) => {
                let (loaded, sync_error) = self.save_before_publish(loaded, authorize)?;
                Ok(RetitledNote {
                    loaded,
                    rename_error: Some(rename_error),
                    sync_error,
                })
            }
        }
    }

    pub fn delete(&self, path: impl AsRef<Path>) -> Result<DeleteDisposition, NoteError> {
        let path = self.confined_existing_file(path.as_ref())?;
        platform_trash(&path)
    }

    fn confined_existing_file(&self, path: &Path) -> Result<PathBuf, NoteError> {
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            self.root.join(path)
        };
        let metadata = fs::symlink_metadata(&absolute).map_err(io_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || !is_markdown(&absolute) {
            return Err(NoteError::PathOutsideRoot { path: absolute });
        }
        let canonical = lume_core::canonical_path(&absolute).map_err(io_error)?;
        if canonical.parent() != Some(self.root.as_path()) {
            return Err(NoteError::PathOutsideRoot { path: absolute });
        }
        Ok(canonical)
    }

    fn unique_path(&self, title: &str, id: &NoteId) -> PathBuf {
        let slug = slugify(title);
        let suffix: String = id
            .as_str()
            .chars()
            .rev()
            .take(8)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        for collision in 0_u64.. {
            let file_name = if collision == 0 {
                format!("{slug}-{suffix}.md")
            } else {
                format!("{slug}-{suffix}-{collision}.md")
            };
            let path = self.root.join(file_name);
            if !path.exists() {
                return path;
            }
        }
        unreachable!("the collision counter is unbounded")
    }

    fn write_synced_temporary(
        &self,
        destination: &Path,
        bytes: &[u8],
    ) -> Result<PathBuf, NoteError> {
        let file_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| NoteError::PathOutsideRoot {
                path: destination.to_owned(),
            })?;
        let generation = SAVE_GENERATION.fetch_add(1, Ordering::Relaxed);
        let temporary = self.root.join(format!(".{file_name}.{generation}.tmp"));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(io_error)?;
            file.write_all(bytes).map_err(io_error)?;
            file.flush().map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
            drop(file);
            Ok(temporary.clone())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

#[must_use]
pub fn filter_summaries<'a>(summaries: &'a [NoteSummary], query: &str) -> Vec<&'a NoteSummary> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return summaries.iter().collect();
    }
    summaries
        .iter()
        .filter(|summary| {
            summary.title.to_lowercase().contains(&query)
                || summary.preview.to_lowercase().contains(&query)
        })
        .collect()
}

fn oversized_note(path: &Path, bytes: &[u8]) -> Note {
    let mut note = Note::empty(NoteId::new());
    note.header.title = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Senza titolo")
        .to_owned();
    note.markdown = String::from_utf8_lossy(bytes).into_owned();
    note.path = Some(path.to_owned());
    note
}

fn read_prefix(path: &Path) -> Result<Vec<u8>, NoteError> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(io_error)?
        .take(SCAN_PREFIX_BYTES)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    Ok(bytes)
}

fn read_bounded(path: &Path) -> Result<(Vec<u8>, bool), NoteError> {
    let metadata = fs::metadata(path).map_err(io_error)?;
    let bounded_len = metadata.len().min(MAX_NOTE_BYTES);
    let mut bytes = Vec::with_capacity(bounded_len as usize);
    File::open(path)
        .map_err(io_error)?
        .take(MAX_NOTE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    Ok((bytes, metadata.len() > MAX_NOTE_BYTES))
}

fn identity_for(path: &Path, bytes: &[u8]) -> Result<FileIdentity, NoteError> {
    let metadata = fs::metadata(path).map_err(io_error)?;
    let modified_unix_nanos = metadata
        .modified()
        .unwrap_or(UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(FileIdentity {
        len: metadata.len(),
        modified_unix_nanos,
        fingerprint: hasher.finish(),
    })
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            separator = false;
            slug.push(character);
        } else {
            separator = true;
        }
    }
    if slug.is_empty() {
        "senza-titolo".to_owned()
    } else {
        slug
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn io_error(error: std::io::Error) -> NoteError {
    NoteError::Io(error.to_string())
}

#[cfg(test)]
fn test_fault(point: u8) -> Result<(), NoteError> {
    if TEST_FAULT.get() == point {
        TEST_FAULT.set(0);
        Err(NoteError::Io(format!("injected fault at point {point}")))
    } else {
        Ok(())
    }
}

#[cfg(not(test))]
const fn test_fault(_point: u8) -> Result<(), NoteError> {
    Ok(())
}

fn publish_new(temporary: &Path, destination: &Path) -> Result<(), NoteError> {
    fs::hard_link(temporary, destination).map_err(io_error)
}

#[cfg(not(windows))]
fn replace_if_unchanged(
    temporary: &Path,
    destination: &Path,
    expected: &FileIdentity,
) -> Result<(), NoteError> {
    let (bytes, _) = read_bounded(destination)?;
    let disk = identity_for(destination, &bytes)?;
    if &disk != expected {
        return Err(NoteError::Conflict { disk });
    }
    replace_file(temporary, destination)
}

#[cfg(not(windows))]
struct RenameSource;

#[cfg(not(windows))]
fn open_rename_source(path: &Path, expected: &FileIdentity) -> Result<RenameSource, NoteError> {
    let (bytes, oversized) = read_bounded(path)?;
    let disk = identity_for(path, &bytes)?;
    if &disk != expected {
        return Err(NoteError::Conflict { disk });
    }
    if oversized {
        return Err(NoteError::ReadOnly {
            path: path.to_owned(),
        });
    }
    Ok(RenameSource)
}

#[cfg(not(windows))]
fn remove_rename_source(
    _source: RenameSource,
    path: &Path,
    expected: &FileIdentity,
) -> Result<(), NoteError> {
    let (bytes, _) = read_bounded(path)?;
    let disk = identity_for(path, &bytes)?;
    if &disk != expected {
        return Err(NoteError::Conflict { disk });
    }
    fs::remove_file(path).map_err(io_error)
}

#[cfg(windows)]
struct RenameSource(File);

#[cfg(windows)]
fn open_rename_source(path: &Path, expected: &FileIdentity) -> Result<RenameSource, NoteError> {
    use std::io::{Seek, SeekFrom};
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Foundation::GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_SHARE_DELETE, FILE_SHARE_READ};

    let mut target = OpenOptions::new()
        .access_mode(GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(path)
        .map_err(io_error)?;
    target.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let metadata = target.metadata().map_err(io_error)?;
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_NOTE_BYTES) as usize);
    Read::by_ref(&mut target)
        .take(MAX_NOTE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    let disk = identity_for_file(&target, &bytes)?;
    if &disk != expected {
        return Err(NoteError::Conflict { disk });
    }
    if metadata.len() > MAX_NOTE_BYTES {
        return Err(NoteError::ReadOnly {
            path: path.to_owned(),
        });
    }
    Ok(RenameSource(target))
}

#[cfg(windows)]
fn remove_rename_source(
    mut source: RenameSource,
    path: &Path,
    expected: &FileIdentity,
) -> Result<(), NoteError> {
    use std::io::{Seek, SeekFrom};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
        FILE_DISPOSITION_INFO_EX, FileDispositionInfoEx, SetFileInformationByHandle,
    };

    source.0.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let metadata = source.0.metadata().map_err(io_error)?;
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_NOTE_BYTES) as usize);
    Read::by_ref(&mut source.0)
        .take(MAX_NOTE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    let disk = identity_for_file(&source.0, &bytes)?;
    if &disk != expected {
        return Err(NoteError::Conflict { disk });
    }
    run_windows_commit_hook(path);
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    // SAFETY: `source` is the validated old target opened with DELETE access;
    // `disposition` remains alive for this synchronous handle operation.
    let succeeded = unsafe {
        SetFileInformationByHandle(
            source.0.as_raw_handle(),
            FileDispositionInfoEx,
            (&raw const disposition).cast(),
            u32::try_from(std::mem::size_of_val(&disposition))
                .map_err(|_| NoteError::Io("buffer di eliminazione troppo grande".to_owned()))?,
        )
    };
    if succeeded == 0 {
        Err(io_error(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn replace_if_unchanged(
    temporary: &Path,
    destination: &Path,
    expected: &FileIdentity,
) -> Result<(), NoteError> {
    use std::io::{Seek, SeekFrom};
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};

    let mut target = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(destination)
        .map_err(io_error)?;
    (|| {
        target.seek(SeekFrom::Start(0)).map_err(io_error)?;
        let metadata = target.metadata().map_err(io_error)?;
        let mut bytes = Vec::with_capacity(metadata.len().min(MAX_NOTE_BYTES) as usize);
        Read::by_ref(&mut target)
            .take(MAX_NOTE_BYTES)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        let disk = identity_for_file(&target, &bytes)?;
        if &disk != expected {
            return Err(NoteError::Conflict { disk });
        }
        run_windows_commit_hook(destination);
        replace_file(temporary, destination)
    })()
}

#[cfg(all(test, windows))]
type CommitHook = Box<dyn FnOnce(&Path)>;

#[cfg(all(test, windows))]
thread_local! {
    static WINDOWS_COMMIT_HOOK: std::cell::RefCell<Option<CommitHook>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(all(test, windows))]
fn run_windows_commit_hook(path: &Path) {
    WINDOWS_COMMIT_HOOK.with(|hook_slot| {
        if let Some(hook) = hook_slot.borrow_mut().take() {
            hook(path);
        }
    });
}

#[cfg(all(windows, not(test)))]
fn run_windows_commit_hook(_path: &Path) {}

#[cfg(windows)]
fn identity_for_file(file: &File, bytes: &[u8]) -> Result<FileIdentity, NoteError> {
    let metadata = file.metadata().map_err(io_error)?;
    let modified_unix_nanos = metadata
        .modified()
        .unwrap_or(UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(FileIdentity {
        len: metadata.len(),
        modified_unix_nanos,
        fingerprint: hasher.finish(),
    })
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> Result<(), NoteError> {
    fs::rename(temporary, destination).map_err(io_error)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> Result<(), NoteError> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_RENAME_INFO, FILE_RENAME_INFO_0, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FileRenameInfoEx, SetFileInformationByHandle,
    };

    let replacement = OpenOptions::new()
        .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(temporary)
        .map_err(io_error)?;
    let name: Vec<u16> = destination.as_os_str().encode_wide().collect();
    let header_size = std::mem::size_of::<FILE_RENAME_INFO>() - std::mem::size_of::<u16>();
    let byte_len = name.len() * std::mem::size_of::<u16>();
    let mut buffer = vec![0_u8; header_size + byte_len];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS.
    // POSIX semantics permits replacement while the validated destination handle
    // remains open with delete sharing, so its deny-write share remains effective.
    // SAFETY: `buffer` ha `header_size + byte_len` byte, `info` punta al suo inizio e le scritture (intestazione e nome UTF-16) restano dentro `buffer`; l'allineamento di `info` non è garantito dal tipo `Vec<u8>` ma dall'allocatore di sistema (almeno 8 byte su Windows), da rivedere se il buffer cambia.
    unsafe {
        (*info).Anonymous = FILE_RENAME_INFO_0 { Flags: 0x1 | 0x2 };
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = u32::try_from(byte_len)
            .map_err(|_| NoteError::Io("percorso di destinazione troppo lungo".to_owned()))?;
        std::ptr::copy_nonoverlapping(
            name.as_ptr().cast::<u8>(),
            (*info).FileName.as_mut_ptr().cast::<u8>(),
            byte_len,
        );
    }
    // SAFETY: `buffer` contains a correctly sized FILE_RENAME_INFO followed by
    // the UTF-16 destination name and remains alive for the synchronous call.
    let succeeded = unsafe {
        SetFileInformationByHandle(
            replacement.as_raw_handle(),
            FileRenameInfoEx,
            info.cast(),
            u32::try_from(buffer.len())
                .map_err(|_| NoteError::Io("buffer della rinomina troppo grande".to_owned()))?,
        )
    };
    if succeeded == 0 {
        Err(io_error(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

/// Sposta la nota nel Cestino di sistema.
///
/// La capacità sta in `lume_core::trash`: qui c'è solo l'adattamento
/// all'errore delle note e il caso in cui il Cestino non è raggiungibile.
fn platform_trash(path: &Path) -> Result<DeleteDisposition, NoteError> {
    match lume_core::trash_path(path) {
        Ok(lume_core::TrashOutcome::Trashed) => Ok(DeleteDisposition::Trashed),
        Ok(lume_core::TrashOutcome::NeedsPermanentConfirmation) => {
            Ok(DeleteDisposition::NeedsPermanentConfirmation)
        }
        Err(error) => Err(NoteError::Trash(error.to_string())),
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn save_excludes_a_competing_writer_through_final_verification_and_replace() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        let directory = tempfile::tempdir().unwrap();
        let store = NoteStore::open(directory.path()).unwrap();
        let mut loaded = store.create("Bloccata").unwrap();
        loaded.note.markdown = "nuovo\n".into();
        WINDOWS_COMMIT_HOOK.with(|hook_slot| {
            *hook_slot.borrow_mut() = Some(Box::new(|path| {
                let competing = OpenOptions::new()
                    .write(true)
                    .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                    .open(path);
                assert!(
                    competing.is_err(),
                    "a writer opened during the commit boundary"
                );
            }));
        });

        let saved = store.save(&loaded).unwrap();

        assert_eq!(saved.note.markdown, "nuovo\n");
    }

    #[test]
    fn rename_excludes_a_competing_writer_at_the_final_removal_boundary() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        let directory = tempfile::tempdir().unwrap();
        let store = NoteStore::open(directory.path()).unwrap();
        let loaded = store.create("Prima").unwrap();
        let boundary_reached = Arc::new(AtomicBool::new(false));
        let hook_boundary_reached = Arc::clone(&boundary_reached);
        WINDOWS_COMMIT_HOOK.with(|hook_slot| {
            *hook_slot.borrow_mut() = Some(Box::new(move |path| {
                hook_boundary_reached.store(true, Ordering::SeqCst);
                let competing = OpenOptions::new()
                    .write(true)
                    .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                    .open(path);
                assert!(
                    competing.is_err(),
                    "a writer opened at the rename final-removal boundary"
                );
            }));
        });

        let renamed = store.rename_for_title(&loaded, "Dopo").unwrap();

        assert!(boundary_reached.load(Ordering::SeqCst));
        assert!(!loaded.path().exists());
        assert_eq!(renamed.note.header.title, "Dopo");
    }
}

#[cfg(test)]
mod transaction_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn save_autorizzato_rifiutato_non_pubblica_il_temporaneo() {
        let directory = tempfile::tempdir().unwrap();
        let store = NoteStore::open(directory.path()).unwrap();
        let loaded = store.create("Prima").unwrap();
        let original = fs::read(loaded.path()).unwrap();
        let mut changed = loaded.clone();
        changed.note.markdown.push_str("\nModifica locale\n");
        let path_seen = loaded.path().to_path_buf();

        let result = store.save_authorized(&changed, |path| {
            assert_eq!(path, path_seen);
            assert_eq!(fs::read(path).unwrap(), original);
            assert!(
                fs::read_dir(directory.path())
                    .unwrap()
                    .filter_map(Result::ok)
                    .any(|entry| entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "tmp"))
            );
            false
        });

        assert!(matches!(result, Err(NoteError::Cancelled { .. })));
        assert_eq!(fs::read(loaded.path()).unwrap(), original);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn rename_autorizzato_rifiutato_non_pubblica_la_destinazione() {
        let directory = tempfile::tempdir().unwrap();
        let store = NoteStore::open(directory.path()).unwrap();
        let loaded = store.create("Prima").unwrap();
        let original = fs::read(loaded.path()).unwrap();

        let result = store.rename_for_title_authorized(&loaded, "Dopo", |path| {
            assert_ne!(path, loaded.path());
            assert!(path.extension().is_some_and(|extension| extension == "md"));
            assert_eq!(fs::read(loaded.path()).unwrap(), original);
            assert!(!path.exists());
            assert!(
                fs::read_dir(directory.path())
                    .unwrap()
                    .filter_map(Result::ok)
                    .any(|entry| entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "tmp"))
            );
            false
        });

        assert!(matches!(result, Err(NoteError::Cancelled { .. })));
        assert!(loaded.path().exists());
        assert_eq!(fs::read(loaded.path()).unwrap(), original);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn create_failure_before_publish_leaves_no_visible_note() {
        let directory = tempfile::tempdir().unwrap();
        let store = NoteStore::open(directory.path()).unwrap();
        TEST_FAULT.set(1);

        assert!(store.create("Interrotta").is_err());
        let markdown_files = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| is_markdown(&entry.path()))
            .count();
        assert_eq!(markdown_files, 0);
    }

    #[test]
    fn rename_failure_before_publish_keeps_old_path_and_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let store = NoteStore::open(directory.path()).unwrap();
        let old = store.create("Prima").unwrap();
        TEST_FAULT.set(2);

        assert!(store.rename_for_title(&old, "Dopo").is_err());
        assert!(old.path().exists());
        assert_eq!(store.load(old.path()).unwrap().note.header.title, "Prima");
        assert_eq!(store.scan().unwrap().len(), 1);
    }

    #[test]
    fn una_rinomina_fallita_salva_comunque_il_contenuto_con_il_vecchio_nome() {
        let directory = tempfile::tempdir().unwrap();
        let store = NoteStore::open(directory.path()).unwrap();
        let mut vecchia = store.create("Prima").unwrap();
        vecchia.note.markdown = "# Dopo\n\ncorpo scritto\n".into();
        TEST_FAULT.set(2);

        let esito = store.save_with_title(&vecchia, "Dopo").unwrap();

        assert!(esito.rename_error.is_some());
        assert_eq!(esito.loaded.path(), vecchia.path());
        assert!(
            store
                .load(vecchia.path())
                .unwrap()
                .note
                .markdown
                .contains("corpo scritto")
        );
        assert_eq!(store.scan().unwrap().len(), 1);
    }

    #[test]
    fn rename_failure_after_publish_rolls_back_to_old_path() {
        let directory = tempfile::tempdir().unwrap();
        let store = NoteStore::open(directory.path()).unwrap();
        let old = store.create("Prima").unwrap();
        TEST_FAULT.set(3);

        assert!(store.rename_for_title(&old, "Dopo").is_err());
        assert!(old.path().exists());
        assert_eq!(store.load(old.path()).unwrap().note.header.title, "Prima");
        assert_eq!(store.scan().unwrap().len(), 1);
    }
}
