//! Token per sessione, record di scoperta Windows e scelta dell'istanza da
//! parte del ponte.
//!
//! Il token (32 byte da `getrandom`) nasce a ogni Avvia e vive solo nel
//! `McpWorkspace` e nel record di scoperta nella cartella dati dell'utente.
//! Su Windows il record è `<dati>/mcp/<pid>.json`, sotto `%LOCALAPPDATA%` con
//! l'ACL dell'utente; su Unix la scoperta versionata è in
//! `crate::unix`. **Mai** nello store, nella config del client, nei log, in
//! status bar, in argv: per questo il suo `Debug` è redatto. Limite noto: chi
//! punta `LUME_DATA_DIR` a una cartella leggibile da altri espone il token.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone)]
pub struct Token([u8; 32]);

impl Token {
    pub fn generate() -> Result<Self, String> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|error| format!("generatore casuale del sistema non disponibile: {error}"))?;
        Ok(Self(bytes))
    }

    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }

    pub fn from_hex(text: &str) -> Option<Self> {
        if text.len() != 64 {
            return None;
        }
        let mut bytes = [0_u8; 32];
        for (slot, pair) in bytes.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
            let pair = std::str::from_utf8(pair).ok()?;
            *slot = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(Self(bytes))
    }

    /// Confronto **a tempo costante** del testo presentato con la forma
    /// esadecimale minuscola: la lunghezza è pubblica, il contenuto no.
    pub fn matches(&self, presented: &str) -> bool {
        ct_eq(presented.as_bytes(), self.to_hex().as_bytes())
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(«redatto»)")
    }
}

impl PartialEq for Token {
    fn eq(&self, other: &Self) -> bool {
        ct_eq(&self.0, &other.0)
    }
}

impl Eq for Token {}

impl Serialize for Token {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Token {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::from_hex(&text).ok_or_else(|| serde::de::Error::custom("token non valido"))
    }
}

/// Xor-fold su tutti i byte: nessuna uscita anticipata alla prima differenza.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Il nonce rende il nome della pipe imprevedibile prima dell'avvio.
pub fn new_nonce() -> Result<String, String> {
    let mut bytes = [0_u8; 4];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("generatore casuale del sistema non disponibile: {error}"))?;
    Ok(hex(&bytes))
}

pub fn pipe_name(pid: u32, nonce: &str) -> String {
    format!(r"\\.\pipe\lume-mcp-{pid}-{nonce}")
}

/// La prima riga che il ponte manda. Il server non scrive un byte prima di
/// averla verificata.
pub fn preamble(token: &Token) -> String {
    serde_json::json!({"lume": "hello", "token": token.to_hex()}).to_string()
}

pub fn check_preamble(line: &[u8], token: &Token) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
        return false;
    };
    value.get("lume").and_then(serde_json::Value::as_str) == Some("hello")
        && value
            .get("token")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|presented| token.matches(presented))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryRecord {
    pub pid: u32,
    pub pipe: String,
    /// Radice canonica: il ponte la confronta con `--project` e con la cwd.
    pub root: PathBuf,
    pub token: Token,
    pub version: String,
    pub started_at: u64,
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

pub fn discovery_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("mcp")
}

pub fn record_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.json"))
}

/// Tiene in vita il file di scoperta: `Drop` lo cancella quando lo stream
/// muore (idioma `let _keep = watcher` di `fs_subscription`,
/// `crates/lume-app/src/app.rs:7741`). Alla chiusura del processo il `Drop`
/// può non girare: per questo `esci` lo cancella anche a mano.
pub struct DiscoveryFile {
    path: PathBuf,
    pipe: String,
}

impl DiscoveryFile {
    pub fn write(dir: &Path, record: &DiscoveryRecord) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = record_path(dir, record.pid);
        let temp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(record).map_err(std::io::Error::other)?;
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, &path)?;
        Ok(Self {
            path,
            pipe: record.pipe.clone(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DiscoveryFile {
    fn drop(&mut self) {
        let still_ours = std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<DiscoveryRecord>(&bytes).ok())
            .is_some_and(|record| record.pipe == self.pipe);
        if still_ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub fn read_records(dir: &Path) -> Vec<(PathBuf, DiscoveryRecord)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| {
            let bytes = std::fs::read(&path).ok()?;
            let record = serde_json::from_slice::<DiscoveryRecord>(&bytes).ok()?;
            Some((path, record))
        })
        .collect()
}

#[cfg(windows)]
#[allow(unsafe_code)]
pub fn pid_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: OpenProcess non tocca memoria nostra; l'handle, se non nullo,
    // si usa solo qui e si chiude prima di uscire.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0_u32;
        let read = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        // Un processo uscito ma con handle ancora aperti esiste ancora come
        // oggetto: conta il codice d'uscita, non l'apertura.
        read && code == STILL_ACTIVE as u32
    }
}

/// Fuori da Windows la scoperta delle pipe non esiste: i record Unix hanno un
/// lettore separato e non usano questo controllo.
#[cfg(not(windows))]
pub fn pid_is_alive(_pid: u32) -> bool {
    false
}

/// Cancella i file dei PID morti e restituisce gli altri. Una volta sola,
/// quando serve (avvio del server, avvio del ponte): nessun timer.
pub fn prune_stale(dir: &Path) -> Vec<DiscoveryRecord> {
    let mut alive = Vec::new();
    for (path, record) in read_records(dir) {
        if pid_is_alive(record.pid) {
            alive.push(record);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
    alive
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChooseError {
    #[error("nessun Lume con il server MCP acceso: avvialo da Estensioni → Server MCP")]
    NoInstance,
    #[error("nessun Lume con il server MCP acceso ha aperto {}", .0.display())]
    NoMatch(PathBuf),
    #[error("più istanze di Lume con il server MCP acceso ({}): scegli con --project <cartella>", elenco(.0))]
    Ambiguous(Vec<PathBuf>),
}

fn elenco(roots: &[PathBuf]) -> String {
    roots
        .iter()
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// `--project` → cwd → istanza unica → errore (D-M7). Nessun «più recente»:
/// indovinare l'istanza vorrebbe dire dare a un agente il progetto sbagliato.
pub fn choose(
    records: &[DiscoveryRecord],
    project: Option<&Path>,
    cwd: Option<&Path>,
) -> Result<DiscoveryRecord, ChooseError> {
    let containing = |dir: &Path| {
        records
            .iter()
            .filter(|record| dir.starts_with(&record.root))
            .max_by_key(|record| record.root.components().count())
            .cloned()
    };
    if let Some(project) = project {
        return containing(project).ok_or_else(|| ChooseError::NoMatch(project.to_path_buf()));
    }
    if let Some(cwd) = cwd
        && let Some(found) = containing(cwd)
    {
        return Ok(found);
    }
    match records {
        [] => Err(ChooseError::NoInstance),
        [only] => Ok(only.clone()),
        many => Err(ChooseError::Ambiguous(
            many.iter().map(|record| record.root.clone()).collect(),
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn record(pid: u32, root: &str, pipe: &str) -> DiscoveryRecord {
        DiscoveryRecord {
            pid,
            pipe: pipe.to_owned(),
            root: PathBuf::from(root),
            token: Token::generate().unwrap(),
            version: "0.2.1".to_owned(),
            started_at: now_unix(),
        }
    }

    #[test]
    fn il_token_ha_64_cifre_e_non_si_ripete() {
        let a = Token::generate().unwrap();
        let b = Token::generate().unwrap();
        assert_eq!(a.to_hex().len(), 64);
        assert_ne!(a.to_hex(), b.to_hex());
        assert_eq!(Token::from_hex(&a.to_hex()).unwrap(), a);
        assert!(Token::from_hex("zz").is_none());
    }

    #[test]
    fn il_token_non_compare_nel_debug() {
        let voce = record(1, "C:\\p", "\\\\.\\pipe\\x");
        let esadecimale = voce.token.to_hex();
        assert!(!format!("{:?}", voce.token).contains(&esadecimale));
        assert!(!format!("{voce:?}").contains(&esadecimale));
    }

    #[test]
    fn il_confronto_e_esatto() {
        let token = Token::generate().unwrap();
        assert!(token.matches(&token.to_hex()));
        assert!(!token.matches(&token.to_hex().to_uppercase()));
        assert!(!token.matches(token.to_hex().get(..63).unwrap()));
        assert!(!token.matches(""));
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }

    #[test]
    fn il_preambolo_passa_solo_col_token_giusto() {
        let token = Token::generate().unwrap();
        let altro = Token::generate().unwrap();
        assert!(check_preamble(preamble(&token).as_bytes(), &token));
        assert!(!check_preamble(preamble(&altro).as_bytes(), &token));
        assert!(!check_preamble(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            &token
        ));
        assert!(!check_preamble(b"", &token));
    }

    #[test]
    fn nonce_e_nome_della_pipe() {
        let nonce = new_nonce().unwrap();
        assert_eq!(nonce.len(), 8);
        assert!(nonce.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(pipe_name(42, "0a1b2c3d"), r"\\.\pipe\lume-mcp-42-0a1b2c3d");
    }

    #[test]
    fn il_file_di_scoperta_nasce_e_muore_con_la_guardia() {
        let dati = tempfile::tempdir().unwrap();
        let dir = discovery_dir(dati.path());
        let voce = record(std::process::id(), "C:\\p", "pipe-a");
        let guardia = DiscoveryFile::write(&dir, &voce).unwrap();
        let letti = read_records(&dir);
        assert_eq!(letti.len(), 1);
        assert_eq!(letti[0].1, voce, "il token sopravvive al giro su disco");
        let percorso = guardia.path().to_path_buf();
        drop(guardia);
        assert!(!percorso.exists());
    }

    /// Ferma e Avvia di fila: il nuovo stream può scrivere il file prima che
    /// il vecchio muoia, e il vecchio non deve cancellarlo.
    #[test]
    fn la_guardia_non_cancella_il_file_di_un_altra_accensione() {
        let dati = tempfile::tempdir().unwrap();
        let dir = discovery_dir(dati.path());
        let vecchia =
            DiscoveryFile::write(&dir, &record(std::process::id(), "C:\\p", "pipe-vecchia"))
                .unwrap();
        let _nuova =
            DiscoveryFile::write(&dir, &record(std::process::id(), "C:\\p", "pipe-nuova")).unwrap();
        drop(vecchia);
        let letti = read_records(&dir);
        assert_eq!(letti.len(), 1);
        assert_eq!(letti[0].1.pipe, "pipe-nuova");
    }

    #[cfg(windows)]
    #[test]
    fn un_pid_morto_rende_stantio_il_file() {
        let mut figlio = std::process::Command::new("cmd")
            .args(["/C", "exit 0"])
            .spawn()
            .unwrap();
        let morto = figlio.id();
        figlio.wait().unwrap();
        assert!(
            !pid_is_alive(morto),
            "processo uscito (l'handle del figlio è ancora aperto)"
        );
        assert!(pid_is_alive(std::process::id()));
        let dati = tempfile::tempdir().unwrap();
        let dir = discovery_dir(dati.path());
        let _vivo =
            DiscoveryFile::write(&dir, &record(std::process::id(), "C:\\vivo", "p1")).unwrap();
        let morto_file = DiscoveryFile::write(&dir, &record(morto, "C:\\morto", "p2")).unwrap();
        let percorso_morto = morto_file.path().to_path_buf();
        std::mem::forget(morto_file); // un crash non esegue la guardia
        let vivi = prune_stale(&dir);
        assert_eq!(vivi.len(), 1);
        assert_eq!(vivi[0].root, PathBuf::from("C:\\vivo"));
        assert!(!percorso_morto.exists());
    }

    /// Percorsi Windows scritti a mano: altrove `C:\…` è un componente solo.
    #[cfg(windows)]
    #[test]
    fn la_scelta_segue_project_poi_cwd_poi_l_unica() {
        let lume = record(1, "C:\\lavoro\\lume", "a");
        let altro = record(2, "C:\\lavoro\\altro", "b");
        let dentro = record(3, "C:\\lavoro\\lume\\crates\\x", "c");
        let tutte = [lume.clone(), altro.clone(), dentro.clone()];
        assert_eq!(
            choose(
                &tutte,
                Some(Path::new("C:\\lavoro\\altro")),
                Some(Path::new("C:\\lavoro\\lume"))
            )
            .unwrap(),
            altro,
            "--project vince sulla cwd"
        );
        assert_eq!(
            choose(
                &tutte,
                None,
                Some(Path::new("C:\\lavoro\\lume\\crates\\x\\src"))
            )
            .unwrap(),
            dentro,
            "la radice più profonda che contiene la cwd"
        );
        assert_eq!(
            choose(&tutte, Some(Path::new("C:\\altrove")), None),
            Err(ChooseError::NoMatch(PathBuf::from("C:\\altrove")))
        );
        assert_eq!(
            choose(std::slice::from_ref(&lume), None, Some(Path::new("D:\\x"))).unwrap(),
            lume
        );
        assert!(
            matches!(choose(&tutte, None, Some(Path::new("D:\\x"))), Err(ChooseError::Ambiguous(radici)) if radici.len() == 3)
        );
        assert_eq!(choose(&[], None, None), Err(ChooseError::NoInstance));
        let messaggio =
            ChooseError::Ambiguous(vec![PathBuf::from("C:\\a"), PathBuf::from("C:\\b")])
                .to_string();
        assert!(
            messaggio.contains("C:\\a") && messaggio.contains("--project"),
            "{messaggio}"
        );
    }
}
