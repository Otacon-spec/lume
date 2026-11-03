use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NOTE_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NoteId(String);

impl NoteId {
    #[must_use]
    pub fn new() -> Self {
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let process_id = std::process::id();
        let sequence = NOTE_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);

        Self(format!("{nanoseconds:x}-{process_id:x}-{sequence:x}"))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for NoteId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<&str> for NoteId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for NoteId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Display for NoteId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeLink {
    pub workspace: PathBuf,
    pub path: PathBuf,
    pub from: usize,
    pub to: usize,
    pub symbol: Option<String>,
    pub extra: BTreeMap<String, toml::Value>,
}

impl CodeLink {
    #[must_use]
    pub fn new(
        workspace: impl Into<PathBuf>,
        path: impl Into<PathBuf>,
        from: usize,
        to: usize,
        symbol: Option<&str>,
    ) -> Self {
        match Self::try_new(workspace, path, from, to, symbol) {
            Ok(link) => link,
            Err(error) => panic!("invalid code link: {error}"),
        }
    }

    pub fn try_new(
        workspace: impl Into<PathBuf>,
        path: impl Into<PathBuf>,
        from: usize,
        to: usize,
        symbol: Option<&str>,
    ) -> Result<Self, NoteError> {
        if from == 0 || to == 0 {
            return Err(NoteError::LineZero);
        }

        let workspace = workspace.into();
        let path = path.into();
        if contains_parent_traversal(&workspace) {
            return Err(NoteError::ParentTraversal);
        }
        let path = normalize_workspace_relative_path(&path)?;

        Ok(Self {
            workspace,
            path,
            from: from.min(to),
            to: from.max(to),
            symbol: symbol.map(str::to_owned),
            extra: BTreeMap::new(),
        })
    }

    fn has_same_identity(&self, other: &Self) -> bool {
        self.workspace == other.workspace
            && self.path == other.path
            && self.from == other.from
            && self.to == other.to
    }
}

fn contains_parent_traversal(path: &Path) -> bool {
    path.components()
        .any(|component| component == Component::ParentDir)
}

fn normalize_workspace_relative_path(path: &Path) -> Result<PathBuf, NoteError> {
    let raw_path = path.to_string_lossy();
    let bytes = raw_path.as_bytes();
    let has_windows_prefix = matches!(
        (bytes.first(), bytes.get(1)),
        (Some(drive), Some(b':')) if drive.is_ascii_alphabetic()
    ) || raw_path.starts_with(r"\");

    if path.is_absolute() || has_windows_prefix {
        return Err(NoteError::PathNotWorkspaceRelative);
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err(NoteError::ParentTraversal),
            Component::Prefix(_) | Component::RootDir => {
                return Err(NoteError::PathNotWorkspaceRelative);
            }
        }
    }

    Ok(normalized)
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoteHeader {
    pub format: u32,
    pub id: NoteId,
    pub title: String,
    pub updated_unix_ms: u64,
    pub paper: bool,
    pub links: Vec<CodeLink>,
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub header: NoteHeader,
    pub markdown: String,
    pub path: Option<PathBuf>,
}

impl Note {
    #[must_use]
    pub fn empty(id: NoteId) -> Self {
        Self {
            header: NoteHeader {
                format: 1,
                id,
                title: String::new(),
                updated_unix_ms: 0,
                paper: false,
                links: Vec::new(),
                extra: BTreeMap::new(),
            },
            markdown: String::new(),
            path: None,
        }
    }

    pub fn add_link(&mut self, link: CodeLink) -> bool {
        if let Some(existing) = self
            .header
            .links
            .iter_mut()
            .find(|existing| existing.has_same_identity(&link))
        {
            existing.symbol = link.symbol;
            return false;
        }

        self.header.links.push(link);
        true
    }

    /// Toglie il collegamento alla posizione indicata, se presente.
    pub fn remove_link(&mut self, index: usize) -> Option<CodeLink> {
        (index < self.header.links.len()).then(|| self.header.links.remove(index))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoteSummary {
    pub id: NoteId,
    pub title: String,
    pub updated_unix_ms: u64,
    pub preview: String,
    pub path: PathBuf,
    pub error: Option<String>,
    /// Collegamenti noti senza dover aprire il corpo completo dell'appunto.
    pub links: Vec<CodeLink>,
}

pub(crate) const MAX_PREVIEW_CHARS: usize = 240;

pub(crate) fn preview_of(markdown: &str) -> String {
    let mut preview = String::new();
    let mut characters = 0;
    for word in markdown.split_whitespace() {
        if characters >= MAX_PREVIEW_CHARS {
            break;
        }
        if !preview.is_empty() {
            preview.push(' ');
            characters += 1;
        }
        for character in word
            .chars()
            .take(MAX_PREVIEW_CHARS.saturating_sub(characters))
        {
            preview.push(character);
            characters += 1;
        }
    }
    preview
}

impl Note {
    #[must_use]
    pub fn summary(&self, error: Option<&NoteError>) -> NoteSummary {
        NoteSummary {
            id: self.header.id.clone(),
            title: self.header.title.clone(),
            updated_unix_ms: self.header.updated_unix_ms,
            preview: preview_of(&self.markdown),
            path: self.path.clone().unwrap_or_default(),
            error: error.map(ToString::to_string),
            links: self.header.links.clone(),
        }
    }
}

/// Errori degli appunti. I messaggi sono in italiano perché arrivano fino
/// all'utente; il testo dei messaggi del sistema operativo o delle librerie
/// che vi sono incapsulati resta quello originale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteError {
    LineZero,
    ParentTraversal,
    PathNotWorkspaceRelative,
    InvalidFrontMatter(String),
    Serialize(String),
    Io(String),
    Cancelled { cleanup_error: Option<String> },
    Conflict { disk: crate::FileIdentity },
    ReadOnly { path: PathBuf },
    PathOutsideRoot { path: PathBuf },
    NoteTooLarge { path: PathBuf, limit: u64 },
    Trash(String),
}

impl fmt::Display for NoteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LineZero => {
                formatter.write_str("le righe dei collegamenti al codice partono da 1")
            }
            Self::ParentTraversal => formatter
                .write_str("il percorso di un collegamento al codice non può contenere '..'"),
            Self::PathNotWorkspaceRelative => formatter.write_str(
                "il percorso di un collegamento al codice deve essere relativo al progetto",
            ),
            Self::InvalidFrontMatter(error) => {
                write!(formatter, "intestazione dell'appunto non valida: {error}")
            }
            Self::Serialize(error) => {
                write!(formatter, "impossibile serializzare l'appunto: {error}")
            }
            Self::Io(error) => write!(formatter, "errore di accesso al file dell'appunto: {error}"),
            Self::Cancelled {
                cleanup_error: None,
            } => {
                formatter.write_str("salvataggio dell'appunto annullato prima della pubblicazione")
            }
            Self::Cancelled {
                cleanup_error: Some(error),
            } => write!(
                formatter,
                "salvataggio dell'appunto annullato; pulizia del temporaneo fallita: {error}"
            ),
            Self::Conflict { .. } => formatter.write_str("l'appunto è cambiato su disco"),
            Self::ReadOnly { path } => {
                write!(formatter, "l'appunto è in sola lettura: {}", path.display())
            }
            Self::PathOutsideRoot { path } => {
                write!(
                    formatter,
                    "il percorso dell'appunto è fuori dalla libreria: {}",
                    path.display()
                )
            }
            Self::NoteTooLarge { path, limit } => write!(
                formatter,
                "l'appunto supera il limite di {limit} byte: {}",
                path.display()
            ),
            Self::Trash(error) => write!(
                formatter,
                "impossibile spostare l'appunto nel cestino di sistema: {error}"
            ),
        }
    }
}

impl std::error::Error for NoteError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i_messaggi_d_errore_sono_in_italiano() {
        let percorso = PathBuf::from("appunti/idea.md");
        let casi = [
            (
                NoteError::LineZero,
                "le righe dei collegamenti al codice partono da 1".to_owned(),
            ),
            (
                NoteError::ParentTraversal,
                "il percorso di un collegamento al codice non può contenere '..'".to_owned(),
            ),
            (
                NoteError::PathNotWorkspaceRelative,
                "il percorso di un collegamento al codice deve essere relativo al progetto"
                    .to_owned(),
            ),
            (
                NoteError::InvalidFrontMatter("x".to_owned()),
                "intestazione dell'appunto non valida: x".to_owned(),
            ),
            (
                NoteError::Serialize("x".to_owned()),
                "impossibile serializzare l'appunto: x".to_owned(),
            ),
            (
                NoteError::Io("x".to_owned()),
                "errore di accesso al file dell'appunto: x".to_owned(),
            ),
            (
                NoteError::ReadOnly {
                    path: percorso.clone(),
                },
                "l'appunto è in sola lettura: appunti/idea.md".to_owned(),
            ),
            (
                NoteError::PathOutsideRoot {
                    path: percorso.clone(),
                },
                "il percorso dell'appunto è fuori dalla libreria: appunti/idea.md".to_owned(),
            ),
            (
                NoteError::NoteTooLarge {
                    path: percorso,
                    limit: 1024,
                },
                "l'appunto supera il limite di 1024 byte: appunti/idea.md".to_owned(),
            ),
            (
                NoteError::Trash("x".to_owned()),
                "impossibile spostare l'appunto nel cestino di sistema: x".to_owned(),
            ),
        ];
        for (errore, atteso) in casi {
            assert_eq!(errore.to_string(), atteso);
        }
    }

    #[test]
    fn un_link_identico_non_si_duplica() {
        let mut note = Note::empty(NoteId::from("n1"));
        let link = CodeLink::new("C:/repo", "src/main.rs", 4, 7, Some("main"));
        assert!(note.add_link(link.clone()));
        assert!(!note.add_link(link));
        assert_eq!(note.header.links.len(), 1);
    }

    #[test]
    fn un_link_con_la_stessa_identita_aggiorna_il_simbolo_senza_duplicarsi() {
        let mut note = Note::empty(NoteId::from("n1"));
        let original = CodeLink::new("C:/repo", "src/main.rs", 4, 7, Some("old_name"));
        let recalculated = CodeLink::new("C:/repo", "src/main.rs", 4, 7, Some("new_name"));

        assert!(note.add_link(original));
        assert!(!note.add_link(recalculated));
        assert_eq!(note.header.links.len(), 1);
        assert_eq!(
            note.header
                .links
                .first()
                .and_then(|link| link.symbol.as_deref()),
            Some("new_name")
        );
    }

    #[test]
    fn il_range_del_link_e_sempre_ordinato_e_uno_based() {
        let link = CodeLink::new("C:/repo", "src/lib.rs", 9, 3, None);
        assert_eq!((link.from, link.to), (3, 9));
        assert!(CodeLink::try_new("C:/repo", "src/lib.rs", 0, 2, None).is_err());
    }

    #[test]
    fn il_path_del_link_e_normalizzato_e_relativo_al_workspace() {
        let Ok(link) = CodeLink::try_new("C:/repo", "./src/./model.rs", 1, 1, None) else {
            panic!("a normalized workspace-relative path must be accepted");
        };
        assert_eq!(link.path, PathBuf::from("src/model.rs"));

        for invalid in [
            "../outside.rs",
            "/rooted.rs",
            r"\rooted.rs",
            "C:/absolute.rs",
            r"C:\absolute.rs",
            r"C:drive-relative.rs",
            r"\\server\share\file.rs",
            r"\\?\C:\verbatim.rs",
        ] {
            assert!(
                CodeLink::try_new("C:/repo", invalid, 1, 1, None).is_err(),
                "accepted non-workspace-relative path: {invalid}"
            );
        }
    }
}
