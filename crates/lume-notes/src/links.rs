//! Ricerca inversa pura dei collegamenti appunto-codice.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use crate::model::{CodeLink, NoteError, NoteId, NoteSummary};

/// Un appunto che cita un intervallo di righe a base 1, estremi inclusi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedNote {
    pub note: NoteId,
    pub title: String,
    pub from: usize,
    pub to: usize,
    pub symbol: Option<String>,
}

/// Indice `file -> appunti` ricostruito dai riepiloghi.
#[derive(Debug, Clone, Default)]
pub struct LinkIndex {
    by_file: HashMap<String, Vec<LinkedNote>>,
}

impl LinkIndex {
    /// Costruisce un indice ordinato indipendentemente dall'ordine dei file.
    #[must_use]
    pub fn build(summaries: &[NoteSummary]) -> Self {
        let mut by_file: HashMap<String, Vec<LinkedNote>> = HashMap::new();
        for summary in summaries {
            for link in &summary.links {
                let Ok(file) = resolve_link(link) else {
                    continue;
                };
                let Some(relative) = relative_path(&link.workspace, &file) else {
                    continue;
                };
                by_file
                    .entry(key(&link.workspace, &relative))
                    .or_default()
                    .push(LinkedNote {
                        note: summary.id.clone(),
                        title: summary.title.clone(),
                        from: link.from,
                        to: link.to,
                        symbol: link.symbol.clone(),
                    });
            }
        }
        for linked in by_file.values_mut() {
            linked.sort_by(|left, right| {
                (left.from, left.to, &left.title, left.note.as_str()).cmp(&(
                    right.from,
                    right.to,
                    &right.title,
                    right.note.as_str(),
                ))
            });
        }
        Self { by_file }
    }

    /// Restituisce gli appunti che citano il percorso relativo nel workspace.
    #[must_use]
    pub fn for_file(&self, workspace: &Path, relative: &Path) -> &[LinkedNote] {
        if !safe_relative(relative) || has_parent(workspace) {
            return &[];
        }
        self.by_file
            .get(&key(workspace, relative))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_file.is_empty()
    }
}

fn key(workspace: &Path, relative: &Path) -> String {
    let mut text = workspace
        .join(relative)
        .to_string_lossy()
        .replace('\\', "/");
    if let Some(without_prefix) = text.strip_prefix("//?/") {
        text = without_prefix.to_owned();
    }
    while text.len() > 1 && text.ends_with('/') {
        text.pop();
    }
    if cfg!(windows) {
        text = text.to_lowercase();
    }
    text
}

fn has_parent(path: &Path) -> bool {
    path.components()
        .any(|component| component == Component::ParentDir)
}

fn safe_relative(path: &Path) -> bool {
    let mut has_normal = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_normal = true,
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => return false,
        }
    }
    has_normal
}

fn same_component(left: Component<'_>, right: Component<'_>) -> bool {
    if cfg!(windows) {
        left.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
    } else {
        left == right
    }
}

/// Restituisce il percorso relativo per componenti, non per prefisso testuale.
#[must_use]
pub fn relative_path(workspace: &Path, file: &Path) -> Option<PathBuf> {
    if has_parent(workspace) || has_parent(file) {
        return None;
    }
    let mut remaining = file.components();
    for root_component in workspace.components() {
        if !same_component(root_component, remaining.next()?) {
            return None;
        }
    }
    let mut relative = PathBuf::new();
    for component in remaining {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => return None,
        }
    }
    (!relative.as_os_str().is_empty()).then_some(relative)
}

/// Risolve il percorso di un link pubblico dopo averlo rivalidato.
///
/// # Errors
/// Restituisce un errore per righe zero, attraversamento `..` o percorsi non
/// relativi al workspace.
pub fn resolve_link(link: &CodeLink) -> Result<PathBuf, NoteError> {
    if link.from == 0 || link.to == 0 {
        return Err(NoteError::LineZero);
    }
    if has_parent(&link.workspace) || has_parent(&link.path) {
        return Err(NoteError::ParentTraversal);
    }
    if !safe_relative(&link.path) {
        return Err(NoteError::PathNotWorkspaceRelative);
    }
    Ok(link.workspace.join(&link.path))
}

/// Converte righe a base 1 e inclusive in indici a base 0 limitati al testo.
#[must_use]
pub fn clamp_lines(from: usize, to: usize, total: usize) -> (usize, usize) {
    if total == 0 {
        return (0, 0);
    }
    let last = total - 1;
    let start = from.saturating_sub(1).min(last);
    let end = to.saturating_sub(1).min(last).max(start);
    (start, end)
}
