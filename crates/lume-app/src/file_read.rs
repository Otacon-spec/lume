//! Lettura di file di testo con un tetto di dimensione.
//!
//! Un editor con `Rope` in memoria non è la sede giusta per un log da un
//! gigabyte: il file sparirebbe due volte in RAM (la `String` e la `Rope`) e
//! l'interfaccia resterebbe ferma finché la lettura non finisce. Qui il tetto
//! è **uno**, verificato sui metadati prima di leggere e poi di nuovo sui byte
//! letti, perché un file può crescere fra i due momenti.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

/// Oltre questa dimensione un file non si apre nell'editor (decisione S8).
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

const MIB: u64 = 1024 * 1024;

#[derive(Debug)]
pub enum FileReadError {
    TooLarge { size: u64, limit: u64 },
    NotAFile,
    NotUtf8,
    Io(std::io::Error),
}

impl fmt::Display for FileReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { size, limit } => write!(
                f,
                "File troppo grande per l'editor: {} MiB, il limite è {} MiB",
                size.div_ceil(MIB),
                limit.div_ceil(MIB)
            ),
            Self::NotAFile => f.write_str("Non è un file regolare"),
            Self::NotUtf8 => f.write_str("Il file non è testo UTF-8 valido"),
            Self::Io(errore) => write!(f, "{errore}"),
        }
    }
}

impl std::error::Error for FileReadError {}

impl From<std::io::Error> for FileReadError {
    fn from(errore: std::io::Error) -> Self {
        Self::Io(errore)
    }
}

impl From<FileReadError> for std::io::Error {
    fn from(errore: FileReadError) -> Self {
        match errore {
            FileReadError::Io(errore) => errore,
            altro => std::io::Error::new(std::io::ErrorKind::InvalidData, altro.to_string()),
        }
    }
}

/// Legge un file regolare di al più `max` byte.
pub fn read_bytes_capped(path: &Path, max: u64) -> Result<Vec<u8>, FileReadError> {
    read_bytes_capped_with(path, max, || {})
}

// Scheduling controllato nei test della stessa lettura nativa. Il caller
// normale non cambia il nome tra precheck e open; altri writer possono farlo.
fn read_bytes_capped_with(
    path: &Path,
    max: u64,
    before_open: impl FnOnce(),
) -> Result<Vec<u8>, FileReadError> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(FileReadError::NotAFile);
    }
    if metadata.len() > max {
        return Err(FileReadError::TooLarge {
            size: metadata.len(),
            limit: max,
        });
    }
    #[cfg(test)]
    test_read_gate::pause(path)?;
    before_open();
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // I file regolari non diventano async. Una FIFO rimpiazzata dopo il
        // precheck si apre senza attendere writer e viene rifiutata sul FD.
        // Non O_NOFOLLOW: l'apertura manuale di symlink rimane supportata,
        // NON un confine MCP/root/sandbox o protezione anti-TOCTOU.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() {
        return Err(FileReadError::NotAFile);
    }
    if opened.len() > max {
        return Err(FileReadError::TooLarge {
            size: opened.len(),
            limit: max,
        });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(0));
    // `max + 1`: se ne arriva uno in più, il file è cresciuto dopo il controllo.
    file.take(max.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(FileReadError::TooLarge {
            size: bytes.len() as u64,
            limit: max,
        });
    }
    Ok(bytes)
}

/// Come `read_bytes_capped`, ma il contenuto deve essere UTF-8.
#[cfg(test)]
fn read_text_capped(path: &Path, max: u64) -> Result<String, FileReadError> {
    String::from_utf8(read_bytes_capped(path, max)?).map_err(|_| FileReadError::NotUtf8)
}

/// Come `read_text_capped`, conservando l'impronta dei byte esatti letti
/// prima che l'editor normalizzi BOM e fine riga.
pub fn read_testo_capped(path: &Path, max: u64) -> Result<Testo, FileReadError> {
    let bytes = read_bytes_capped(path, max)?;
    Testo::from_bytes(bytes)
}

/// Il testo di un file letto, condivisibile senza copiarlo. Il `Debug` stampa
/// la sola lunghezza: `Message` è `Debug` e un log non deve ricevere 30 MiB.
#[derive(Clone)]
pub struct Testo {
    text: Arc<String>,
    fingerprint: [u8; 32],
    prepared: Option<Arc<std::sync::Mutex<Option<PreparedContent>>>>,
    #[cfg(test)]
    prepared_thread: Option<std::thread::ThreadId>,
}

enum PreparedContent {
    UnchangedReload,
    Open(lume_core::PreparedText),
    Reload(lume_core::PreparedReload),
}

impl Testo {
    pub(crate) fn unchanged_reload(&self) -> bool {
        self.prepared.as_ref().is_some_and(|payload| {
            payload
                .lock()
                .is_ok_and(|payload| matches!(*payload, Some(PreparedContent::UnchangedReload)))
        })
    }

    pub(crate) fn prepare_reload_unless_unchanged(
        mut self,
        snapshot: Option<lume_core::SaveSnapshot>,
        accepted: Option<[u8; 32]>,
    ) -> Self {
        if accepted == Some(self.fingerprint) {
            self.prepared = Some(Arc::new(std::sync::Mutex::new(Some(
                PreparedContent::UnchangedReload,
            ))));
            self
        } else {
            self.prepare(snapshot)
        }
    }
    #[cfg(test)]
    pub(crate) fn has_prepared(&self) -> bool {
        self.prepared.as_ref().is_some_and(|payload| {
            payload.lock().is_ok_and(|payload| {
                matches!(
                    *payload,
                    Some(PreparedContent::Open(_) | PreparedContent::Reload(_))
                )
            })
        }) && self
            .prepared_thread
            .is_some_and(|thread| thread != std::thread::current().id())
    }

    /// Solo sul worker: normalizzazione, Rope e inverse undo prima della UI.
    pub(crate) fn prepare(mut self, snapshot: Option<lume_core::SaveSnapshot>) -> Self {
        let text = lume_core::PreparedText::prepare(&self.text);
        let payload = match snapshot {
            Some(snapshot) => PreparedContent::Reload(snapshot.prepare_reload(text)),
            None => PreparedContent::Open(text),
        };
        self.prepared = Some(Arc::new(std::sync::Mutex::new(Some(payload))));
        #[cfg(test)]
        {
            self.prepared_thread = Some(std::thread::current().id());
        }
        self
    }

    fn take_prepared(&self) -> Option<PreparedContent> {
        self.prepared.as_ref()?.lock().ok()?.take()
    }

    pub(crate) fn take_open(&self) -> Option<lume_core::PreparedText> {
        match self.take_prepared() {
            Some(PreparedContent::Open(text)) => Some(text),
            #[cfg(test)]
            None if self.prepared.is_none() => Some(lume_core::PreparedText::prepare(self)),
            _ => None,
        }
    }

    pub(crate) fn apply_reload(
        &self,
        document: &mut lume_core::Document,
    ) -> Option<lume_core::AppliedEdit> {
        match self.take_prepared() {
            Some(PreparedContent::Reload(reload)) => document.reload_prepared(reload),
            #[cfg(test)]
            Some(PreparedContent::Open(text)) => {
                let reload = document.read_snapshot().prepare_reload(text);
                document.reload_prepared(reload)
            }
            #[cfg(test)]
            None if self.prepared.is_none() => Some(document.reload_from_str(self)),
            _ => None,
        }
    }
    fn share_owned(mut text: String) -> Arc<String> {
        // Il file può essere cresciuto durante la lettura: non conservare la
        // capacità in eccesso. Nei file stabili capacity=len, nessuna copia.
        text.shrink_to_fit();
        Arc::new(text)
    }

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, FileReadError> {
        let fingerprint = Sha256::digest(&bytes).into();
        let text = String::from_utf8(bytes).map_err(|_| FileReadError::NotUtf8)?;
        Ok(Self {
            text: Self::share_owned(text),
            fingerprint,
            prepared: None,
            #[cfg(test)]
            prepared_thread: None,
        })
    }

    /// Il corpo di una nota appena riletto dal `NoteStore`, con l'impronta dei
    /// byte esatti del file serializzato: nell'editor vive solo il Markdown,
    /// ma eco del watcher e conflitti si confrontano con il file intero.
    pub(crate) fn from_note(original_bytes: &[u8], body: String) -> Self {
        let fingerprint: [u8; 32] = Sha256::digest(original_bytes).into();
        Self {
            text: Self::share_owned(body),
            fingerprint,
            prepared: None,
            #[cfg(test)]
            prepared_thread: None,
        }
    }

    /// Impronta SHA-256 dei byte grezzi letti dal file, inclusi BOM e CRLF.
    pub fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
}

impl std::ops::Deref for Testo {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

impl From<String> for Testo {
    fn from(testo: String) -> Self {
        let fingerprint = Sha256::digest(testo.as_bytes()).into();
        Self {
            text: Self::share_owned(testo),
            fingerprint,
            prepared: None,
            #[cfg(test)]
            prepared_thread: None,
        }
    }
}

impl fmt::Debug for Testo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Testo({} byte)", self.text.len())
    }
}

/// Cosa fare quando la lettura di un file chiesto dall'utente è tornata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DopoApertura {
    Niente,
    /// Porta il caret sulla riga (1-based).
    Riga(usize),
    /// Seleziona un intervallo di righe a base 1, estremi inclusi.
    IntervalloRighe {
        da: usize,
        a: usize,
    },
    /// Navigazione interna: porta il caret sulla riga (0-based).
    Navigazione {
        riga: usize,
    },
    /// Salto LSP: riga e carattere UTF-16, entrambi 0-based. L'epoch
    /// progetto revoca la pubblicazione dopo un cambio root, anche ABA;
    /// non è autorizzazione/confinamento filesystem né owner del reader.
    PosizioneLsp {
        riga: u32,
        carattere: u32,
        progetto: u64,
        /// Documento e revisione della richiesta, non del buffer target.
        /// Non è un token di istanza/binding o autorità di pubblicazione.
        origine: (lume_core::DocId, u64),
        /// Epoca privata della navigazione: cambio scheda o nuova domanda
        /// revocano anche un risultato già letto, senza cancellare/annullare IO.
        navigazione: u64,
    },
    /// Un risultato di «Cerca ovunque»: riga opzionale e sessione salvata.
    Ricerca {
        riga: Option<usize>,
    },
    /// Un file arrivato dalla shell del sistema operativo.
    Avvio {
        fuori_progetto: bool,
    },
}

#[derive(Debug, Clone)]
pub struct LetturaFile {
    pub path: PathBuf,
    pub dopo: DopoApertura,
    pub esito: Result<Testo, String>,
}

/// La lettura vera, **bloccante**: si chiama dal worker reader posseduto.
#[cfg(test)]
pub fn leggi_per_apertura(path: PathBuf, dopo: DopoApertura) -> LetturaFile {
    leggi_per_apertura_con_snapshot(path, dopo, None)
}

pub(crate) fn leggi_per_apertura_con_snapshot(
    path: PathBuf,
    dopo: DopoApertura,
    snapshot: Option<lume_core::SaveSnapshot>,
) -> LetturaFile {
    let esito = read_testo_capped(&path, MAX_FILE_BYTES)
        .map(|text| text.prepare(snapshot))
        .map_err(|errore| errore.to_string());
    LetturaFile { path, dopo, esito }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn shared_text_does_not_retain_oversized_capacity() {
        let mut text = String::with_capacity(4 * 1024 * 1024);
        text.push_str("testo λ");
        let result = Testo::from(text);
        assert_eq!(result.text.capacity(), result.len());
        assert_eq!(&*result, "testo λ");
    }

    #[test]
    fn owned_utf8_buffer_is_shared_without_full_copy() {
        let text = "λ\r\n".repeat(262_144);
        let pointer = text.as_ptr();
        let digest: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        let result = Testo::from(text);
        assert_eq!(
            result.as_ptr(),
            pointer,
            "costruzione non deve copiare il buffer posseduto"
        );
        assert_eq!(result.fingerprint(), digest);
        assert_eq!(result.clone().as_ptr(), pointer);
    }

    #[test]
    fn owned_raw_bytes_preserve_buffer_bom_and_fingerprint() {
        let bytes = b"\xef\xbb\xbfprima\r\nseconda\n".to_vec();
        let pointer = bytes.as_ptr();
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        let result = Testo::from_bytes(bytes).unwrap();
        assert_eq!(
            result.as_ptr(),
            pointer,
            "lettura UTF8 non deve copiare i byte validati"
        );
        assert_eq!(result.fingerprint(), digest);
        assert_eq!(&*result, "\u{feff}prima\r\nseconda\n");
        assert!(matches!(
            Testo::from_bytes(vec![0xff]),
            Err(FileReadError::NotUtf8)
        ));
    }

    #[cfg(unix)]
    use std::os::unix::fs::FileTypeExt;

    const MIB: u64 = 1024 * 1024;

    fn scrivi(dir: &tempfile::TempDir, nome: &str, contenuto: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(nome);
        std::fs::write(&path, contenuto).unwrap();
        path
    }

    #[test]
    fn il_debug_di_un_testo_non_stampa_il_contenuto() {
        let testo = Testo::from("segreto ".repeat(10_000));
        let stampa = format!("{testo:?}");
        assert_eq!(stampa, "Testo(80000 byte)");
        assert!(!stampa.contains("segreto"));
    }

    #[test]
    fn fingerprint_riguarda_i_byte_originali_prima_di_bom_e_crlf() {
        use sha2::{Digest, Sha256};

        let dir = tempfile::tempdir().unwrap();
        let path = scrivi(&dir, "misto.txt", b"\xEF\xBB\xBFciao\r\nmondo\r\n");
        let testo = read_testo_capped(&path, 1024).unwrap();
        assert_eq!(&*testo, "\u{feff}ciao\r\nmondo\r\n");
        let raw: [u8; 32] = Sha256::digest(b"\xEF\xBB\xBFciao\r\nmondo\r\n").into();
        let normalized: [u8; 32] = Sha256::digest("ciao\nmondo\n".as_bytes()).into();
        assert_eq!(testo.fingerprint(), raw);
        assert_ne!(testo.fingerprint(), normalized);
    }

    #[test]
    fn leggi_per_apertura_porta_il_percorso_e_il_seguito() {
        let dir = tempfile::tempdir().unwrap();
        let path = scrivi(&dir, "a.rs", b"fn a() {}\n");
        let lettura = leggi_per_apertura(path.clone(), DopoApertura::Riga(3));
        assert_eq!(lettura.path, path);
        assert!(matches!(lettura.dopo, DopoApertura::Riga(3)));
        assert_eq!(&*lettura.esito.unwrap(), "fn a() {}\n");
    }

    #[test]
    fn leggi_per_apertura_di_un_file_enorme_da_il_messaggio_in_italiano() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enorme.log");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        let lettura = leggi_per_apertura(path, DopoApertura::Niente);
        let motivo = lettura.esito.unwrap_err();
        assert!(
            motivo.starts_with("File troppo grande per l'editor"),
            "{motivo}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn nome_sostituito_con_directory_rifiutato_come_non_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = scrivi(&dir, "a.txt", b"prima");
        let result = read_bytes_capped_with(&path, 100, || {
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir(&path).unwrap();
        });
        assert!(matches!(result, Err(FileReadError::NotAFile)), "{result:?}");
    }

    #[test]
    fn dimensione_del_file_aperto_non_del_nome_precedente() {
        let dir = tempfile::tempdir().unwrap();
        let path = scrivi(&dir, "a.txt", b"prima");
        let replacement = scrivi(&dir, "altro.txt", &[b'x'; 1024]);
        let result = read_bytes_capped_with(&path, 100, || {
            std::fs::remove_file(&path).unwrap();
            std::fs::rename(&replacement, &path).unwrap();
        });
        assert!(
            matches!(
                result,
                Err(FileReadError::TooLarge {
                    size: 1024,
                    limit: 100
                })
            ),
            "{result:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn apertura_manuale_di_symlink_regolare_preservata() {
        let dir = tempfile::tempdir().unwrap();
        let source = scrivi(&dir, "source.txt", b"testo reale\n");
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        assert_eq!(read_bytes_capped(&link, 100).unwrap(), b"testo reale\n");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    // Child isolato: la fixture appartiene al parent che raccoglie anche
    // timeout/kill/wait, così nessuna FIFO/tempdir resta persa dopo SIGKILL.
    #[test]
    #[cfg(unix)]
    #[ignore = "helper isolato eseguito e verificato dal test parent"]
    #[allow(unsafe_code)]
    fn child_fifo_sostituita_fra_precheck_e_open() {
        use std::os::unix::ffi::OsStrExt;
        let Some(root) = std::env::var_os("LUME_TEST_FIFO_READ_ROOT") else {
            return;
        };
        let path = PathBuf::from(root).join("read.txt");
        let result = read_bytes_capped_with(&path, 100, || {
            std::fs::remove_file(&path).unwrap();
            let native = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            // Puntatore CString valido per la chiamata; nome solo nella
            // TempDir posseduta dal parent, nessun dato/procedura esterno.
            // SAFETY: la `CString` resta viva per la chiamata e il percorso appartiene alla fixture temporanea; `mkfifo` legge solo il puntatore C.
            assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
        });
        assert!(matches!(result, Err(FileReadError::NotAFile)), "{result:?}");
    }

    #[test]
    #[cfg(unix)]
    fn fifo_sostituita_non_blocca_open_e_child_raccolto() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        scrivi(&dir, "read.txt", b"prima");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "file_read::tests::child_fifo_sostituita_fra_precheck_e_open",
                "--ignored",
                "--nocapture",
            ])
            .env("LUME_TEST_FIFO_READ_ROOT", dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        let mut timeout = false;
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if started.elapsed() > Duration::from_secs(5) {
                timeout = true;
                // Solo child posseduto; race con exit gestita, sempre wait.
                let _ = child.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(10)); // solo child attivo, non app idle
        }
        let output = child.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            !timeout,
            "apertura FIFO bloccata, child ucciso/raccolto: {text}"
        );
        assert!(
            output.status.success(),
            "{text}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            text.contains("running 1 test") && text.contains("1 passed; 0 failed"),
            "test child realmente eseguito: {text}"
        );
        assert!(
            std::fs::symlink_metadata(dir.path().join("read.txt"))
                .unwrap()
                .file_type()
                .is_fifo()
        );
    }

    #[test]
    fn un_file_sotto_il_tetto_si_legge() {
        let dir = tempfile::tempdir().unwrap();
        let path = scrivi(&dir, "a.rs", "fn main() {}\n".as_bytes());
        assert_eq!(read_text_capped(&path, 1024).unwrap(), "fn main() {}\n");
        assert_eq!(read_bytes_capped(&path, 1024).unwrap(), b"fn main() {}\n");
    }

    #[test]
    fn il_tetto_e_incluso_e_un_byte_in_piu_no() {
        let dir = tempfile::tempdir().unwrap();
        let giusto = scrivi(&dir, "giusto.txt", &[b'a'; 100]);
        let uno_in_piu = scrivi(&dir, "uno_in_piu.txt", &[b'a'; 101]);
        assert_eq!(read_bytes_capped(&giusto, 100).unwrap().len(), 100);
        assert!(matches!(
            read_bytes_capped(&uno_in_piu, 100),
            Err(FileReadError::TooLarge {
                size: 101,
                limit: 100
            })
        ));
    }

    /// Attenzione: «Senza leggerlo»: l'errore porta la dimensione **del file** (40 MiB),
    /// non quella che una lettura con `take(max + 1)` avrebbe visto (33 MiB).
    /// È la prova che il rifiuto avviene sui metadati.
    #[test]
    fn un_file_oltre_il_tetto_e_rifiutato_senza_leggerlo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enorme.log");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(40 * MIB)
            .unwrap();
        let errore = read_text_capped(&path, MAX_FILE_BYTES).unwrap_err();
        assert!(
            matches!(errore, FileReadError::TooLarge { size, limit }
                if size == 40 * MIB && limit == MAX_FILE_BYTES),
            "{errore:?}"
        );
        assert_eq!(
            errore.to_string(),
            "File troppo grande per l'editor: 40 MiB, il limite è 32 MiB"
        );
    }

    #[test]
    fn la_dimensione_si_arrotonda_per_eccesso() {
        let errore = FileReadError::TooLarge {
            size: 32 * MIB + 1,
            limit: 32 * MIB,
        };
        assert_eq!(
            errore.to_string(),
            "File troppo grande per l'editor: 33 MiB, il limite è 32 MiB"
        );
    }

    #[test]
    fn un_file_non_utf8_da_un_messaggio_chiaro() {
        let dir = tempfile::tempdir().unwrap();
        // BOM UTF-16 LE seguito da testo a due byte per carattere.
        let path = scrivi(&dir, "utf16.txt", &[0xFF, 0xFE, b'a', 0x00, 0xC3]);
        let errore = read_text_capped(&path, 1024).unwrap_err();
        assert!(matches!(errore, FileReadError::NotUtf8), "{errore:?}");
        assert_eq!(errore.to_string(), "Il file non è testo UTF-8 valido");
        // Le stesse byte come binario si leggono: il tetto non giudica il contenuto.
        assert_eq!(read_bytes_capped(&path, 1024).unwrap().len(), 5);
    }

    #[test]
    fn una_directory_non_e_un_file() {
        let dir = tempfile::tempdir().unwrap();
        let errore = read_text_capped(dir.path(), 1024).unwrap_err();
        assert!(matches!(errore, FileReadError::NotAFile), "{errore:?}");
        assert_eq!(errore.to_string(), "Non è un file regolare");
    }

    #[test]
    fn un_file_che_non_esiste_conserva_l_errore_di_sistema() {
        let dir = tempfile::tempdir().unwrap();
        let errore = read_text_capped(&dir.path().join("manca.rs"), 1024).unwrap_err();
        let come_io: std::io::Error = errore.into();
        assert_eq!(come_io.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn gli_altri_errori_diventano_invalid_data_col_messaggio_in_italiano() {
        let come_io: std::io::Error = FileReadError::NotUtf8.into();
        assert_eq!(come_io.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(come_io.to_string(), "Il file non è testo UTF-8 valido");
    }

    /// Percorsi con spazi, accenti, un trattino lungo e un emoji: nessun
    /// passaggio da `to_str`. (Non è il test omonimo di `lume-testkit`:
    /// quello riguarda le voci di `PATH`; si filtra con `file_read::`.)
    #[test]
    fn percorsi_con_spazi_e_accenti() {
        let dir = tempfile::tempdir().unwrap();
        let cartella = dir.path().join("cartella con spazi — è più 日本");
        std::fs::create_dir(&cartella).unwrap();
        let path = cartella.join("citt\u{e0} \u{1F600}.txt");
        std::fs::write(&path, "ciao\n").unwrap();
        assert_eq!(read_text_capped(&path, 1024).unwrap(), "ciao\n");
    }
}

#[cfg(test)]
pub(crate) mod test_read_gate {
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        sync::{Mutex, OnceLock, mpsc},
    };
    struct Gate {
        started: mpsc::Sender<u32>,
        release: mpsc::Receiver<()>,
    }
    fn gates() -> &'static Mutex<HashMap<PathBuf, Gate>> {
        static GATES: OnceLock<Mutex<HashMap<PathBuf, Gate>>> = OnceLock::new();
        GATES.get_or_init(Default::default)
    }
    pub(crate) struct Release {
        path: PathBuf,
        sender: Option<mpsc::Sender<()>>,
    }
    impl Drop for Release {
        fn drop(&mut self) {
            gates()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&self.path);
            if let Some(sender) = self.sender.take() {
                let _ = sender.send(());
            }
        }
    }
    pub(crate) fn install(path: &Path) -> (mpsc::Receiver<u32>, Release) {
        let (started, receiver) = mpsc::channel();
        let (sender, release) = mpsc::channel();
        gates()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(path.to_path_buf(), Gate { started, release });
        (
            receiver,
            Release {
                path: path.to_path_buf(),
                sender: Some(sender),
            },
        )
    }
    pub(super) fn pause(path: &Path) -> std::io::Result<()> {
        let gate = gates()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(path);
        if let Some(gate) = gate {
            #[cfg(target_os = "linux")]
            let identity = std::fs::read_link("/proc/thread-self")?
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .ok_or_else(|| std::io::Error::other("Identità thread proc assente"))?
                .parse::<u32>()
                .map_err(std::io::Error::other)?;
            #[cfg(not(target_os = "linux"))]
            let identity = 0;
            gate.started.send(identity).map_err(std::io::Error::other)?;
            gate.release
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}
