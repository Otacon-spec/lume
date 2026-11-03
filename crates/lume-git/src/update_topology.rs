//! Diagnostica topologica: proietta remove/put senza mutare filesystem.
//! Non dirty check, payload/attrs/access preflight, token/lease o confinamento.
use crate::{
    GitError, GitRepository,
    tree_changes::TreeChange,
    worktree::{check_safe_path, linklike},
};
use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
fn cancellation(cancel: &AtomicBool) -> Result<(), GitError> {
    if cancel.load(Ordering::Relaxed) {
        Err(GitError::Cancelled)
    } else {
        Ok(())
    }
}
fn fail(error: impl std::fmt::Display) -> GitError {
    GitError::Worktree(error.to_string())
}
fn regular(mode: gix::object::tree::EntryMode) -> bool {
    matches!(
        mode.kind(),
        gix::object::tree::EntryKind::Blob | gix::object::tree::EntryKind::BlobExecutable
    )
}
fn safe_metadata(path: &Path) -> Result<Option<fs::Metadata>, GitError> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if linklike(&meta) {
                return Err(fail("link/reparse point nel percorso del delta"));
            }
            if !meta.is_dir() && !meta.is_file() {
                return Err(fail(
                    "dispositivo o tipo non supportato nel percorso del delta",
                ));
            }
            if meta.permissions().readonly() {
                return Err(fail("percorso del delta in sola lettura"));
            }
            Ok(Some(meta))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(fail(error)),
    }
}
/// Cammino esistente prima delle rimozioni. Un file-antenato presente nella
/// lista remove viene proiettato assente, NON eliminato/seguito come directory.
fn inspect(
    root: &Path,
    path: &Path,
    put: bool,
    removed: &BTreeSet<&Path>,
    cancel: &AtomicBool,
) -> Result<(), GitError> {
    let mut absolute = root.to_path_buf();
    let mut relative = std::path::PathBuf::new();
    let mut parts = path.components().peekable();
    while let Some(part) = parts.next() {
        cancellation(cancel)?;
        absolute.push(part);
        relative.push(part);
        let Some(meta) = safe_metadata(&absolute)? else {
            return Ok(());
        };
        if parts.peek().is_some() {
            if meta.is_dir() {
                continue;
            }
            if removed.contains(relative.as_path()) {
                return Ok(());
            }
            return Err(fail("file-antenato non previsto tra le rimozioni"));
        }
        if meta.is_file() {
            return Ok(());
        }
        if !put {
            return Err(fail("leaf da rimuovere diventata cartella"));
        }
        if !removed.iter().any(|path| path.starts_with(&relative)) {
            return Err(fail("cartella occupata non prevista tra le rimozioni"));
        }
        return inspect_collapsing_directory(root, &relative, removed, cancel);
    }
    Err(fail("percorso del delta vuoto"))
}
/// Una ReadDir viva per profondità, non raccolta eager di tutte le entry.
/// Ogni leaf deve figurare tra le remove; directory vuote estranee preservate.
fn inspect_collapsing_directory(
    root: &Path,
    path: &Path,
    removed: &BTreeSet<&Path>,
    cancel: &AtomicBool,
) -> Result<(), GitError> {
    let mut stack = vec![(
        path.to_path_buf(),
        fs::read_dir(root.join(path)).map_err(fail)?,
    )];
    while let Some((directory, entries)) = stack.last_mut() {
        cancellation(cancel)?;
        let Some(entry) = entries.next() else {
            stack.pop();
            continue;
        };
        let entry = entry.map_err(fail)?;
        let child = directory.join(entry.file_name());
        let Some(meta) = safe_metadata(&root.join(&child))? else {
            return Err(fail("entry scomparsa durante verifica topologica"));
        };
        if meta.is_dir() {
            if !removed.iter().any(|path| path.starts_with(&child)) {
                return Err(fail(
                    "cartella locale estranea nel sottoalbero da sostituire",
                ));
            }
            let entries = fs::read_dir(root.join(&child)).map_err(fail)?;
            stack.push((child, entries));
        } else if !removed.contains(child.as_path()) {
            return Err(fail("file locale estraneo nel sottoalbero da sostituire"));
        }
    }
    cancellation(cancel)
}
impl GitRepository {
    /// Diagnostica FS di due tree reali, compresa proiezione file↔directory.
    /// Non controlla dirty/stage, oggetti blob/attributi/ACL/alias e non tiene
    /// lease index/HEAD. Un Ok non autorizza una pubblicazione successiva.
    /// I/O e Drop ReadDir sul worker; cancellazione fra chiamate, non deadline.
    pub fn check_update_topology(
        &self,
        old: gix::ObjectId,
        new: gix::ObjectId,
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        let changes = self.tree_changes(old, new, false, Some(cancel))?;
        self.ensure_update_topology(&changes, cancel)
    }
    /// La futura orchestrazione dovrà passare il delta del proprio snapshot
    /// live e rivalidare al publish: questa diagnostica NON acquisisce owners.
    pub(crate) fn ensure_update_topology(
        &self,
        changes: &[TreeChange],
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        cancellation(cancel)?;
        let root = safe_metadata(&self.work_dir)?.ok_or_else(|| fail("radice worktree assente"))?;
        if !root.is_dir() {
            return Err(fail("radice worktree non è una cartella"));
        }
        let mut removed = BTreeSet::new();
        // Prima tutti i nomi/modi, senza metadata di destinazioni parziali.
        for change in changes {
            cancellation(cancel)?;
            check_safe_path(change.path())?;
            match change {
                TreeChange::Remove { path, before } if regular(before.mode) => {
                    removed.insert(path.as_path());
                }
                TreeChange::Put { before, after, .. }
                    if regular(after.mode) && before.is_none_or(|leaf| regular(leaf.mode)) => {}
                _ => {
                    return Err(fail(
                        "delta non supportato: link/submodule o rewrite invece di remove/put",
                    ));
                }
            }
        }
        for change in changes {
            cancellation(cancel)?;
            inspect(
                &self.work_dir,
                change.path(),
                matches!(change, TreeChange::Put { .. }),
                &removed,
                cancel,
            )?;
        }
        cancellation(cancel)
    }
}
