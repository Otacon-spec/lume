//! Adattatori noti e loro ricerca su `PATH`.
//!
//! La ricerca non lancia processi: scorre le cartelle assolute di `PATH` e
//! controlla che il file esista (regola di `discovery.rs`). «Trovato» non
//! vuol dire «compatibile»: lo dice solo la risposta a `initialize`.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::launch::LaunchConfig;
use crate::process::AdapterCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdapterId {
    /// GNU gdb ≥ 14, interprete `dap` su stdio (D-DAP-4).
    Gdb,
}

impl AdapterId {
    pub const ALL: [Self; 1] = [Self::Gdb];

    /// Il nome usato in `.lume/launch.toml`.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Gdb => "gdb",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Gdb => "GNU gdb (DAP)",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|id| id.key() == key)
    }

    fn executable(self) -> &'static str {
        match self {
            Self::Gdb => {
                if cfg!(windows) {
                    "gdb.exe"
                } else {
                    "gdb"
                }
            }
        }
    }

    /// Argomenti fissi dell'adattatore.
    fn args(self) -> Vec<String> {
        match self {
            Self::Gdb => vec!["-q".into(), "-i".into(), "dap".into()],
        }
    }

    /// Cerca l'eseguibile nelle cartelle date; quelle vuote o relative si
    /// scartano.
    pub fn find_in(self, search_path: &[PathBuf]) -> Option<PathBuf> {
        let file = self.executable();
        search_path
            .iter()
            .filter(|cartella| cartella.is_absolute())
            .map(|cartella| cartella.join(file))
            .find(|candidato| candidato.is_file())
    }

    /// Cerca su `PATH` del processo.
    pub fn find(self) -> Option<PathBuf> {
        let search_path: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|valore| std::env::split_paths(&valore).collect())
            .unwrap_or_default();
        self.find_in(&search_path)
    }

    pub fn command(self, executable: &Path, cwd: &Path) -> AdapterCommand {
        AdapterCommand {
            program: executable.to_path_buf(),
            args: self.args(),
            cwd: cwd.to_path_buf(),
        }
    }

    /// Gli argomenti di `launch`, propri di ogni adattatore.
    pub fn launch_arguments(self, config: &LaunchConfig) -> Value {
        match self {
            Self::Gdb => {
                let mut map = Map::new();
                map.insert(
                    "program".into(),
                    Value::from(config.program.to_string_lossy().into_owned()),
                );
                map.insert(
                    "args".into(),
                    Value::Array(config.args.iter().cloned().map(Value::from).collect()),
                );
                map.insert(
                    "cwd".into(),
                    Value::from(config.cwd.to_string_lossy().into_owned()),
                );
                if !config.env.is_empty() {
                    map.insert(
                        "env".into(),
                        Value::Object(
                            config
                                .env
                                .iter()
                                .map(|(chiave, valore)| {
                                    (chiave.clone(), Value::from(valore.clone()))
                                })
                                .collect(),
                        ),
                    );
                }
                map.insert(
                    "stopAtBeginningOfMainSubprogram".into(),
                    Value::from(config.stop_at_entry),
                );
                Value::Object(map)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn ricerca_solo_in_cartelle_assolute() {
        let cartella = tempfile::tempdir().unwrap();
        let nome = if cfg!(windows) { "gdb.exe" } else { "gdb" };
        std::fs::write(cartella.path().join(nome), b"").unwrap();
        assert_eq!(
            AdapterId::Gdb.find_in(&[cartella.path().to_path_buf()]),
            Some(cartella.path().join(nome))
        );
        assert_eq!(AdapterId::Gdb.find_in(&[PathBuf::from(".")]), None);
        assert_eq!(AdapterId::Gdb.find_in(&[PathBuf::new()]), None);
    }

    #[test]
    fn una_cartella_non_e_un_adattatore() {
        let cartella = tempfile::tempdir().unwrap();
        std::fs::create_dir(cartella.path().join("gdb")).unwrap();
        std::fs::create_dir(cartella.path().join("gdb.exe")).unwrap();
        assert_eq!(
            AdapterId::Gdb.find_in(&[cartella.path().to_path_buf()]),
            None
        );
    }

    #[test]
    fn chiave_della_configurazione() {
        assert_eq!(AdapterId::from_key("gdb"), Some(AdapterId::Gdb));
        assert_eq!(AdapterId::from_key("lldb"), None);
    }
}
