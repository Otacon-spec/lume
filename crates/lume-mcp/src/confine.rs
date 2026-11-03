//! Confinamento dei percorsi chiesti da un agente.
//!
//! Radice e percorso si canonicalizzano (`std::fs::canonicalize` risolve
//! symlink, junction e nomi 8.3) e si confrontano **per componenti**
//! (`strip_prefix`). Si rifiutano `..` in qualunque punto, gli assoluti fuori
//! dalla radice, i link che escono e ogni cosa sotto `.git` (lì ci sono gli
//! hook eseguibili).
//!
//! **Non** si riusa `Lume::resolve_path` (`crates/lume-app/src/app.rs:6318-6329`):
//! il suo ripiego su `<root>/src/<p>` qui sarebbe un buco.

use std::path::{Component, Path, PathBuf};

use crate::catalog::ToolError;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfineError {
    #[error("percorso vuoto")]
    Empty,
    #[error("«..» non è ammesso nei percorsi")]
    ParentDir,
    #[error("il percorso esce dalla radice del progetto")]
    Outside,
    #[error("la cartella .git non è accessibile agli agenti")]
    GitDir,
    #[error("file o cartella inesistente: {0}")]
    NotFound(String),
    #[error("percorso non leggibile: {0}")]
    Io(String),
    #[error("percorso non sicuro per una scrittura: {0}")]
    Unsafe(String),
}

impl From<ConfineError> for ToolError {
    fn from(error: ConfineError) -> Self {
        ToolError::new(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confined {
    pub absolute: PathBuf,
    pub relative: PathBuf,
}

impl Confined {
    /// Il percorso relativo con `/`, come lo si restituisce agli agenti.
    pub fn slash(&self) -> String {
        to_slash(&self.relative)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    canonical: PathBuf,
}

impl Root {
    pub fn new(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            canonical: canonical(path)?,
        })
    }

    pub fn path(&self) -> &Path {
        &self.canonical
    }

    pub fn resolve(&self, requested: &str) -> Result<Confined, ConfineError> {
        let candidate = self.candidate(requested)?;
        let absolute = canonical(&candidate).map_err(|error| io_error(requested, &error))?;
        self.confine(absolute)
    }

    pub fn resolve_lenient(&self, requested: &str) -> Result<Confined, ConfineError> {
        let candidate = self.candidate(requested)?;
        match canonical(&candidate) {
            Ok(absolute) => self.confine(absolute),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = candidate.file_name().ok_or(ConfineError::Empty)?.to_owned();
                let parent = candidate.parent().ok_or(ConfineError::Outside)?;
                let parent = canonical(parent).map_err(|error| io_error(requested, &error))?;
                self.confine(parent.join(name))
            }
            Err(error) => Err(ConfineError::Io(error.to_string())),
        }
    }

    /// Risolve un bersaglio di scrittura senza creare directory e senza seguire
    /// link o reparse point. La verifica TOCTOU e gli hard link restano fuori
    /// dal confine garantito.
    pub fn resolve_for_write(&self, requested: &str) -> Result<Confined, ConfineError> {
        // Controlla i componenti originali prima della normalizzazione di
        // `candidate`: su Windows un punto/spazio finale cambia il nome letto.
        for part in Path::new(requested).components() {
            if let Component::Normal(name) = part {
                check_write_component(&name.to_string_lossy())?;
            }
        }
        let candidate = self.candidate(requested)?;
        let name = candidate.file_name().ok_or(ConfineError::Empty)?.to_owned();
        let parent = candidate.parent().ok_or(ConfineError::Outside)?;
        let parent = canonical(parent).map_err(|error| io_error(requested, &error))?;
        let target = parent.join(name);
        let confined = self.confine(target.clone())?;
        match std::fs::symlink_metadata(&target) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
                    return Err(ConfineError::Unsafe(format!(
                        "{} è un collegamento: la scrittura lo seguirebbe",
                        confined.slash()
                    )));
                }
                if metadata.is_dir() {
                    return Err(ConfineError::Unsafe(format!(
                        "{} è una cartella, non un file",
                        confined.slash()
                    )));
                }
                let absolute = canonical(&target).map_err(|error| io_error(requested, &error))?;
                self.confine(absolute)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(confined),
            Err(error) => Err(ConfineError::Io(error.to_string())),
        }
    }

    pub fn relative_of(&self, absolute: &Path) -> Option<Confined> {
        self.confine(absolute.to_path_buf()).ok()
    }

    fn candidate(&self, requested: &str) -> Result<PathBuf, ConfineError> {
        let requested = requested.trim();
        if requested.is_empty() {
            return Err(ConfineError::Empty);
        }
        let path = Path::new(requested);
        if path.components().any(|part| part == Component::ParentDir) {
            return Err(ConfineError::ParentDir);
        }
        // Assoluti, radicati (`\Windows`) e relativi a un disco (`C:x`): si
        // canonicalizzano così come sono e decide il confronto con la radice.
        let rooted = path.is_absolute()
            || path.has_root()
            || path
                .components()
                .any(|part| matches!(part, Component::Prefix(_)));
        if rooted {
            return Ok(path.to_path_buf());
        }
        Ok(join_components(&self.canonical, path))
    }

    fn confine(&self, absolute: PathBuf) -> Result<Confined, ConfineError> {
        let relative = absolute
            .strip_prefix(&self.canonical)
            .map_err(|_| ConfineError::Outside)?
            .to_path_buf();
        let in_git = relative.components().any(|part| {
            matches!(part, Component::Normal(name) if name.to_string_lossy().eq_ignore_ascii_case(".git"))
        });
        if in_git {
            return Err(ConfineError::GitDir);
        }
        Ok(Confined { absolute, relative })
    }
}

fn io_error(requested: &str, error: &std::io::Error) -> ConfineError {
    if error.kind() == std::io::ErrorKind::NotFound {
        ConfineError::NotFound(requested.to_owned())
    } else {
        ConfineError::Io(error.to_string())
    }
}

fn check_write_component(name: &str) -> Result<(), ConfineError> {
    if crate::gate::is_git_name(name) {
        return Err(ConfineError::GitDir);
    }
    #[cfg(windows)]
    {
        if name.contains(':') {
            return Err(ConfineError::Unsafe(format!(
                "{name}: i flussi alternativi non sono ammessi"
            )));
        }
        if !safe_component(name) {
            return Err(ConfineError::Unsafe(format!(
                "{name}: nome che Windows legge diversamente"
            )));
        }
        if name
            .as_bytes()
            .windows(2)
            .any(|pair| matches!(pair, [b'~', digit] if digit.is_ascii_digit()))
        {
            return Err(ConfineError::Unsafe(format!(
                "{name}: alias 8.3 non ammesso"
            )));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// Aggiunge i componenti **uno per uno**: `/` e `\` diventano separatori veri
/// anche se `base` fosse un percorso verbatim.
pub fn join_components(base: &Path, relative: &Path) -> PathBuf {
    let mut out = base.to_path_buf();
    for part in relative.components() {
        if let Component::Normal(name) = part {
            out.push(name);
        }
    }
    out
}

pub fn to_slash(path: &Path) -> String {
    path.components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(simplify)
}

/// Toglie `\\?\` quando il percorso resta lo stesso anche senza (come il
/// crate dunce): sotto 260 caratteri, niente nomi riservati (`CON`, `NUL`…),
/// niente componenti che finiscono con punto o spazio. Altrimenti lo lascia:
/// un confronto che fallisce è un rifiuto, mai un'uscita dalla radice.
pub fn simplify(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(text) = path.to_str()
            && text.len() < 260
        {
            if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
                if rest.split('\\').all(safe_component) {
                    return PathBuf::from(format!(r"\\{rest}"));
                }
            } else if let Some(rest) = text.strip_prefix(r"\\?\")
                && rest.as_bytes().get(1) == Some(&b':')
                && rest.split('\\').skip(1).all(safe_component)
            {
                return PathBuf::from(rest);
            }
        }
    }
    path
}

#[cfg(windows)]
fn safe_component(component: &str) -> bool {
    const RISERVATI: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = component.split('.').next().unwrap_or(component);
    !component.ends_with('.')
        && !component.ends_with(' ')
        && !RISERVATI.iter().any(|name| stem.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::fs;

    fn radice() -> (tempfile::TempDir, Root) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src").join("main.rs"), "fn main() {}\n").unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git").join("config"), "[core]\n").unwrap();
        let root = Root::new(dir.path()).unwrap();
        (dir, root)
    }

    #[test]
    fn un_percorso_relativo_dentro_la_radice_si_risolve() {
        let (_dir, root) = radice();
        let trovato = root.resolve("src/main.rs").unwrap();
        assert_eq!(trovato.slash(), "src/main.rs");
        assert!(trovato.absolute.is_file());
        assert!(trovato.absolute.starts_with(root.path()));
    }

    #[test]
    fn i_due_punti_sono_rifiutati_anche_se_restano_dentro() {
        let (_dir, root) = radice();
        assert_eq!(
            root.resolve("src/../src/main.rs"),
            Err(ConfineError::ParentDir)
        );
        assert_eq!(root.resolve("../fuori.txt"), Err(ConfineError::ParentDir));
    }

    #[test]
    fn un_assoluto_fuori_dalla_radice_e_rifiutato_uno_dentro_no() {
        let (_dir, root) = radice();
        let altra = tempfile::tempdir().unwrap();
        let fuori = altra.path().join("segreto.txt");
        fs::write(&fuori, "x").unwrap();
        assert_eq!(
            root.resolve(fuori.to_str().unwrap()),
            Err(ConfineError::Outside)
        );
        let dentro = root.path().join("src").join("main.rs");
        assert_eq!(
            root.resolve(dentro.to_str().unwrap()).unwrap().slash(),
            "src/main.rs"
        );
    }

    #[test]
    fn la_cartella_git_e_chiusa() {
        let (_dir, root) = radice();
        assert_eq!(root.resolve(".git/config"), Err(ConfineError::GitDir));
        assert_eq!(root.resolve(".git"), Err(ConfineError::GitDir));
        #[cfg(windows)]
        assert_eq!(root.resolve(".GIT/config"), Err(ConfineError::GitDir));
    }

    #[test]
    fn un_file_che_manca_e_not_found_ma_lenient_lo_accetta_se_la_cartella_c_e() {
        let (_dir, root) = radice();
        assert!(matches!(
            root.resolve("src/cancellato.rs"),
            Err(ConfineError::NotFound(_))
        ));
        assert_eq!(
            root.resolve_lenient("src/cancellato.rs").unwrap().slash(),
            "src/cancellato.rs"
        );
        assert!(matches!(
            root.resolve_lenient("manca/cancellato.rs"),
            Err(ConfineError::NotFound(_))
        ));
    }

    #[test]
    fn vuoto_e_radicato_senza_disco_non_passano() {
        let (_dir, root) = radice();
        assert_eq!(root.resolve("  "), Err(ConfineError::Empty));
        assert!(root.resolve("\\Windows\\win.ini").is_err());
        assert!(root.resolve("C:secret.txt").is_err());
    }

    #[test]
    fn relative_of_accetta_solo_quel_che_sta_sotto() {
        let (_dir, root) = radice();
        let dentro = root.path().join("src").join("main.rs");
        assert_eq!(root.relative_of(&dentro).unwrap().slash(), "src/main.rs");
        assert!(root.relative_of(Path::new("C:\\altrove\\x.rs")).is_none());
        assert!(
            root.relative_of(&root.path().join(".git").join("HEAD"))
                .is_none()
        );
    }

    #[cfg(windows)]
    #[test]
    fn una_junction_che_esce_dalla_radice_e_rifiutata() {
        let (dir, root) = radice();
        let altra = tempfile::tempdir().unwrap();
        fs::write(altra.path().join("segreto.txt"), "x").unwrap();
        let link = dir.path().join("fuga");
        let esito = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(altra.path())
            .output()
            .unwrap();
        assert!(
            esito.status.success(),
            "mklink /J: {}",
            String::from_utf8_lossy(&esito.stderr)
        );
        assert_eq!(root.resolve("fuga/segreto.txt"), Err(ConfineError::Outside));
    }

    /// Review Focus: un agente scrive `/` e sbaglia le maiuscole. Con un
    /// percorso verbatim `\\?\` la `/` non sarebbe un separatore e il file
    /// «non esisterebbe».
    #[cfg(windows)]
    #[test]
    fn barre_in_avanti_e_maiuscole_diverse_si_risolvono_su_windows() {
        let (_dir, root) = radice();
        let trovato = root.resolve("SRC/Main.RS").unwrap();
        assert!(trovato.absolute.is_file());
        assert!(
            trovato.slash().eq_ignore_ascii_case("src/main.rs"),
            "{}",
            trovato.slash()
        );
        assert_eq!(root.resolve(r"src\main.rs").unwrap().slash(), "src/main.rs");
    }

    #[test]
    fn una_radice_con_spazi_e_accenti_funziona() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("progetto con spazi è");
        fs::create_dir_all(dir.join("sotto")).unwrap();
        fs::write(dir.join("sotto").join("città.txt"), "x").unwrap();
        let root = Root::new(&dir).unwrap();
        assert_eq!(
            root.resolve("sotto/città.txt").unwrap().slash(),
            "sotto/città.txt"
        );
    }

    #[test]
    fn resolve_for_write_accetta_file_nuovi_ed_esistenti_senza_creare_directory() {
        let (_dir, root) = radice();
        let new_file = root.resolve_for_write("src/nuovo.rs").unwrap();
        assert_eq!(new_file.slash(), "src/nuovo.rs");
        assert!(!new_file.absolute.exists());
        assert!(new_file.absolute.starts_with(root.path()));
        assert!(
            root.resolve_for_write("src/main.rs")
                .unwrap()
                .absolute
                .is_file()
        );
        assert!(matches!(
            root.resolve_for_write("missing/nuovo.rs"),
            Err(ConfineError::NotFound(_))
        ));
    }

    #[test]
    fn resolve_for_write_rifiuta_traversal_assoluti_fuori_git_e_directory() {
        let (_dir, root) = radice();
        assert_eq!(
            root.resolve_for_write("../fuori.txt"),
            Err(ConfineError::ParentDir)
        );
        assert_eq!(
            root.resolve_for_write("src/../../fuori.txt"),
            Err(ConfineError::ParentDir)
        );
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("nuovo.txt");
        assert_eq!(
            root.resolve_for_write(outside_file.to_str().unwrap()),
            Err(ConfineError::Outside)
        );
        assert!(!outside_file.exists());
        assert_eq!(
            root.resolve_for_write(root.path().join("src/nuovo.rs").to_str().unwrap())
                .unwrap()
                .slash(),
            "src/nuovo.rs"
        );
        for path in [
            ".git/config",
            ".GIT/hooks/pre-commit",
            ".git./config",
            ".git /config",
            "GIT~1/config",
            ".git::$INDEX_ALLOCATION/x",
            "src/.git/x",
        ] {
            assert_eq!(
                root.resolve_for_write(path),
                Err(ConfineError::GitDir),
                "{path}"
            );
        }
        assert!(root.resolve_for_write(".gitignore").is_ok());
        assert!(matches!(
            root.resolve_for_write("src"),
            Err(ConfineError::Unsafe(_))
        ));
        assert_eq!(root.resolve_for_write("  "), Err(ConfineError::Empty));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_for_write_rifiuta_symlink_anche_se_penzolante() {
        use std::os::unix::fs::symlink;
        let (dir, root) = radice();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("nuovo.txt");
        symlink(&target, dir.path().join("dangling.txt")).unwrap();
        assert!(matches!(
            root.resolve_for_write("dangling.txt"),
            Err(ConfineError::Unsafe(_))
        ));
        assert!(!target.exists());
    }

    #[cfg(windows)]
    #[test]
    fn il_prefisso_verbatim_si_toglie_solo_quando_e_sicuro() {
        assert_eq!(
            simplify(PathBuf::from(r"\\?\C:\a\b")),
            PathBuf::from(r"C:\a\b")
        );
        assert_eq!(
            simplify(PathBuf::from(r"\\?\UNC\srv\share\x")),
            PathBuf::from(r"\\srv\share\x")
        );
        assert_eq!(
            simplify(PathBuf::from(r"\\?\C:\a\CON")),
            PathBuf::from(r"\\?\C:\a\CON")
        );
        assert_eq!(
            simplify(PathBuf::from(r"\\?\C:\a\b.")),
            PathBuf::from(r"\\?\C:\a\b.")
        );
        assert!(
            !Root::new(Path::new("."))
                .unwrap()
                .path()
                .to_string_lossy()
                .starts_with(r"\\?\")
        );
    }
}
