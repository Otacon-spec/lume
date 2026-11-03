//! Delta nativo degli alberi: metadati delle leaf, mai payload del worktree.
//! Storia mantiene i rewrites configurati; integrazione li spegne per avere
//! remove/put espliciti. Non è preflight né apply/lock/confinamento filesystem.
use crate::{ChangeKind, ChangedFile, GitError, GitRepository};
use gix::object::tree::{EntryMode, diff::Change};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Leaf {
    pub(crate) id: gix::ObjectId,
    pub(crate) mode: EntryMode,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TreeChange {
    Put {
        path: PathBuf,
        before: Option<Leaf>,
        after: Leaf,
    },
    Remove {
        path: PathBuf,
        before: Leaf,
    },
    Rewrite {
        source: PathBuf,
        path: PathBuf,
        before: Leaf,
        after: Leaf,
        copy: bool,
    },
}
impl TreeChange {
    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Put { path, .. } | Self::Remove { path, .. } | Self::Rewrite { path, .. } => path,
        }
    }
    pub(crate) fn into_file(self) -> ChangedFile {
        let (path, kind) = match self {
            Self::Put {
                path,
                before,
                after,
            } => (
                path,
                match before {
                    None => ChangeKind::Added,
                    Some(before) if before.mode != after.mode => ChangeKind::TypeChanged,
                    Some(_) => ChangeKind::Modified,
                },
            ),
            Self::Remove { path, .. } => (path, ChangeKind::Removed),
            Self::Rewrite { path, copy, .. } => (
                path,
                if copy {
                    ChangeKind::Copied
                } else {
                    ChangeKind::Renamed
                },
            ),
        };
        ChangedFile { path, kind }
    }
}
fn path(raw: &gix::bstr::BStr) -> Result<PathBuf, gix::Exn> {
    Ok(gix::path::try_from_bstr(raw)
        .map_err(|error| error.erased())?
        .into_owned())
}
impl GitRepository {
    /// Metadati old/new dagli oggetti veri. Rewrites attivi solo per storia:
    /// per checkout futuro usare false, anche se diff.renames è configurato.
    /// Cancellazione fra callback; non interrompe letture ODB/rename nativi.
    pub(crate) fn tree_changes(
        &self,
        old: gix::ObjectId,
        new: gix::ObjectId,
        rewrites: bool,
        cancel: Option<&AtomicBool>,
    ) -> Result<Vec<TreeChange>, GitError> {
        let cancelled = || cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed));
        if cancelled() {
            return Err(GitError::Cancelled);
        }
        let fail = |error: gix::Error| GitError::Diff(error.to_string());
        let old = self.repository.find_tree(old).map_err(fail)?;
        let new = self.repository.find_tree(new).map_err(fail)?;
        let mut platform = old.changes().map_err(fail)?;
        platform.options(|opts| {
            opts.track_path();
            if !rewrites {
                opts.track_rewrites(None);
            }
        });
        let mut result = Vec::new();
        platform
            .for_each_to_obtain_tree(&new, |change| {
                if cancelled() {
                    return Ok::<_, gix::Exn>(std::ops::ControlFlow::Break(()));
                }
                match change {
                    Change::Addition {
                        location,
                        entry_mode,
                        id,
                        ..
                    } if !entry_mode.is_tree() => result.push(TreeChange::Put {
                        path: path(location)?,
                        before: None,
                        after: Leaf {
                            id: id.detach(),
                            mode: entry_mode,
                        },
                    }),
                    Change::Deletion {
                        location,
                        entry_mode,
                        id,
                        ..
                    } if !entry_mode.is_tree() => result.push(TreeChange::Remove {
                        path: path(location)?,
                        before: Leaf {
                            id: id.detach(),
                            mode: entry_mode,
                        },
                    }),
                    Change::Modification {
                        location,
                        previous_entry_mode,
                        previous_id,
                        entry_mode,
                        id,
                    } => {
                        let before = (!previous_entry_mode.is_tree()).then_some(Leaf {
                            id: previous_id.detach(),
                            mode: previous_entry_mode,
                        });
                        if !entry_mode.is_tree() {
                            result.push(TreeChange::Put {
                                path: path(location)?,
                                before,
                                after: Leaf {
                                    id: id.detach(),
                                    mode: entry_mode,
                                },
                            });
                        } else if let Some(before) = before {
                            result.push(TreeChange::Remove {
                                path: path(location)?,
                                before,
                            });
                        }
                    }
                    Change::Rewrite {
                        source_location,
                        location,
                        source_entry_mode,
                        source_id,
                        entry_mode,
                        id,
                        copy,
                        ..
                    } => result.push(TreeChange::Rewrite {
                        source: path(source_location)?,
                        path: path(location)?,
                        before: Leaf {
                            id: source_id.detach(),
                            mode: source_entry_mode,
                        },
                        after: Leaf {
                            id: id.detach(),
                            mode: entry_mode,
                        },
                        copy,
                    }),
                    _ => {}
                }
                Ok::<_, gix::Exn>(std::ops::ControlFlow::Continue(()))
            })
            .map_err(fail)?;
        // Anche alberi identici/nessun callback: mai pubblicare il prefisso
        // di una walk cancellata come piano completo. Errori ODB restano sopra.
        if cancelled() {
            return Err(GitError::Cancelled);
        }
        result.sort_by(|a, b| a.path().cmp(b.path()));
        Ok(result)
    }
}
#[cfg(test)]
mod tests;
