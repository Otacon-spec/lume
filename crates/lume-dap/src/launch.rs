//! `.lume/launch.toml`: le configurazioni di lancio del progetto (D-DAP-3).
//!
//! ```toml
//! [[configurazione]]
//! nome = "prova"
//! adattatore = "gdb"
//! richiesta = "launch"
//! programma = "${workspaceFolder}/target/debug/prova"
//! argomenti = ["--verbose"]
//! cartella = "${workspaceFolder}"
//! ambiente = { RUST_LOG = "debug" }
//! fermati_all_inizio = false
//! ```
//!
//! Leggere il file non esegue nulla; la sessione sì, e chiede fiducia.
//! Un'unica sostituzione: `${workspaceFolder}`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::adapters::AdapterId;

/// Percorso del file, relativo alla radice del progetto.
pub const LAUNCH_FILE: &str = ".lume/launch.toml";
/// Limite di lettura del file: è configurazione, non dati.
pub const MAX_LAUNCH_FILE_BYTES: u64 = 256 * 1024;

const WORKSPACE_FOLDER: &str = "${workspaceFolder}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchConfig {
    pub name: String,
    pub adapter: AdapterId,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub stop_at_entry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LaunchError {
    #[error("{LAUNCH_FILE} non trovato")]
    Missing,
    #[error("{LAUNCH_FILE} non leggibile: {0}")]
    Unreadable(String),
    #[error("{LAUNCH_FILE} oltre {MAX_LAUNCH_FILE_BYTES} byte")]
    TooLarge,
    #[error("{LAUNCH_FILE} non valido: {0}")]
    Syntax(String),
    #[error("{LAUNCH_FILE} non contiene configurazioni")]
    Empty,
    #[error("configurazione {index}: {detail}")]
    Invalid { index: usize, detail: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    configurazione: Vec<RawConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    nome: String,
    adattatore: String,
    #[serde(default = "default_request")]
    richiesta: String,
    programma: String,
    #[serde(default)]
    argomenti: Vec<String>,
    #[serde(default)]
    cartella: Option<String>,
    #[serde(default)]
    ambiente: BTreeMap<String, String>,
    #[serde(default)]
    fermati_all_inizio: bool,
}

fn default_request() -> String {
    "launch".to_owned()
}

fn substitute(value: &str, workspace: &Path) -> String {
    value.replace(WORKSPACE_FOLDER, &workspace.to_string_lossy())
}

/// Interpreta il testo del file.
pub fn parse(text: &str, workspace: &Path) -> Result<Vec<LaunchConfig>, LaunchError> {
    let raw: RawFile =
        toml::from_str(text).map_err(|error| LaunchError::Syntax(error.to_string()))?;
    if raw.configurazione.is_empty() {
        return Err(LaunchError::Empty);
    }
    raw.configurazione
        .into_iter()
        .enumerate()
        .map(|(index, config)| {
            let index = index + 1;
            let invalid = |detail: String| LaunchError::Invalid { index, detail };
            let name = config.nome.trim().to_owned();
            if name.is_empty() {
                return Err(invalid("`nome` vuoto".into()));
            }
            let adapter = AdapterId::from_key(&config.adattatore).ok_or_else(|| {
                invalid(format!(
                    "adattatore `{}` sconosciuto (noti: {})",
                    config.adattatore,
                    AdapterId::ALL.map(AdapterId::key).join(", ")
                ))
            })?;
            if config.richiesta != "launch" {
                return Err(invalid(format!(
                    "richiesta `{}` non supportata: la v1 avvia soltanto (`launch`)",
                    config.richiesta
                )));
            }
            let program = PathBuf::from(substitute(&config.programma, workspace));
            if !program.is_absolute() {
                return Err(invalid(format!(
                    "`programma` deve essere un percorso assoluto (anche con {WORKSPACE_FOLDER}): {}",
                    program.display()
                )));
            }
            let cwd = config
                .cartella
                .map_or_else(|| workspace.to_path_buf(), |cartella| {
                    PathBuf::from(substitute(&cartella, workspace))
                });
            if !cwd.is_absolute() {
                return Err(invalid(format!(
                    "`cartella` deve essere un percorso assoluto: {}",
                    cwd.display()
                )));
            }
            Ok(LaunchConfig {
                name,
                adapter,
                program,
                args: config
                    .argomenti
                    .iter()
                    .map(|argomento| substitute(argomento, workspace))
                    .collect(),
                cwd,
                env: config
                    .ambiente
                    .into_iter()
                    .map(|(chiave, valore)| (chiave, substitute(&valore, workspace)))
                    .collect(),
                stop_at_entry: config.fermati_all_inizio,
            })
        })
        .collect()
}

/// Legge `.lume/launch.toml` dalla radice del progetto (I/O bloccante: va
/// chiamata fuori dal thread della UI).
pub fn load(workspace: &Path) -> Result<Vec<LaunchConfig>, LaunchError> {
    let path = workspace.join(LAUNCH_FILE);
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(LaunchError::Missing);
        }
        Err(error) => return Err(LaunchError::Unreadable(error.to_string())),
    };
    if metadata.len() > MAX_LAUNCH_FILE_BYTES {
        return Err(LaunchError::TooLarge);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| LaunchError::Unreadable(error.to_string()))?;
    parse(&text, workspace)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn radice() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from("C:\\progetto")
        } else {
            PathBuf::from("/progetto")
        }
    }

    #[test]
    fn configurazione_completa_con_sostituzione() {
        let testo = r#"
[[configurazione]]
nome = " prova "
adattatore = "gdb"
programma = "${workspaceFolder}/prova"
argomenti = ["${workspaceFolder}/dati", "-v"]
ambiente = { CASA = "${workspaceFolder}" }
fermati_all_inizio = true
"#;
        let configs = parse(testo, &radice()).unwrap();
        assert_eq!(configs.len(), 1);
        let config = &configs[0];
        assert_eq!(config.name, "prova");
        assert_eq!(config.adapter, AdapterId::Gdb);
        assert_eq!(config.program, radice().join("prova"));
        assert_eq!(config.cwd, radice());
        assert_eq!(config.args[1], "-v");
        assert!(config.args[0].ends_with("dati"));
        assert_eq!(config.env["CASA"], radice().to_string_lossy());
        assert!(config.stop_at_entry);
    }

    #[test]
    fn attach_adattatore_ignoto_e_percorsi_relativi_sono_rifiutati() {
        let attach =
            "[[configurazione]]\nnome='a'\nadattatore='gdb'\nrichiesta='attach'\nprogramma='/p'\n";
        assert!(matches!(
            parse(attach, &radice()),
            Err(LaunchError::Invalid { index: 1, .. })
        ));
        let ignoto = "[[configurazione]]\nnome='a'\nadattatore='lldb'\nprogramma='/p'\n";
        assert!(matches!(
            parse(ignoto, &radice()),
            Err(LaunchError::Invalid { .. })
        ));
        let relativo = "[[configurazione]]\nnome='a'\nadattatore='gdb'\nprogramma='target/p'\n";
        assert!(matches!(
            parse(relativo, &radice()),
            Err(LaunchError::Invalid { .. })
        ));
    }

    #[test]
    fn file_vuoto_campi_ignoti_e_sintassi() {
        assert_eq!(parse("", &radice()), Err(LaunchError::Empty));
        let ignoto = "[[configurazione]]\nnome='a'\nadattatore='gdb'\nprogramma='/p'\nshell='sh'\n";
        assert!(matches!(
            parse(ignoto, &radice()),
            Err(LaunchError::Syntax(_))
        ));
        assert!(matches!(
            parse("[[", &radice()),
            Err(LaunchError::Syntax(_))
        ));
    }

    #[test]
    fn file_assente_e_troppo_grande() {
        let cartella = tempfile::tempdir().unwrap();
        assert_eq!(load(cartella.path()), Err(LaunchError::Missing));
        std::fs::create_dir(cartella.path().join(".lume")).unwrap();
        let grande = vec![b'#'; usize::try_from(MAX_LAUNCH_FILE_BYTES).unwrap() + 1];
        std::fs::write(cartella.path().join(LAUNCH_FILE), grande).unwrap();
        assert_eq!(load(cartella.path()), Err(LaunchError::TooLarge));
    }
}
