//! Apply/pop stash tramite l'eccezione Git CLI dichiarata dal progetto.
use crate::{GitCli, GitError, GitRepository, worktree::check_safe_path};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StashApplyOutcome {
    Applied,
    Conflicts(Vec<PathBuf>),
}

impl GitRepository {
    /// Applica una voce mantenendola nello stash. Git è eseguito sul worker e
    /// l'esito conflitto conserva la voce per una risoluzione manuale.
    pub fn stash_apply(
        &self,
        cli: &GitCli,
        index: usize,
        cancel: Arc<AtomicBool>,
    ) -> Result<StashApplyOutcome, GitError> {
        self.apply_stash(cli, index, cancel, false)
    }

    /// Applica e rimuove la voce solo quando Git conclude senza conflitti.
    pub fn stash_pop(
        &self,
        cli: &GitCli,
        index: usize,
        cancel: Arc<AtomicBool>,
    ) -> Result<StashApplyOutcome, GitError> {
        self.apply_stash(cli, index, cancel, true)
    }

    fn apply_stash(
        &self,
        cli: &GitCli,
        index: usize,
        cancel: Arc<AtomicBool>,
        pop: bool,
    ) -> Result<StashApplyOutcome, GitError> {
        if cancel.load(Ordering::Acquire) {
            return Err(GitError::Cancelled);
        }
        if self.merge_in_progress() {
            return Err(GitError::Stash("merge già in corso".into()));
        }
        let entry = self
            .stash_list()?
            .into_iter()
            .find(|entry| entry.index == index)
            .ok_or_else(|| GitError::Stash(format!("stash@{{{index}}} non esiste")))?;
        let stash_id = self.stash_oid(index)?;
        if !stash_id.to_string().starts_with(&entry.commit_id) {
            return Err(GitError::Stash(
                "lo stash è cambiato durante la preparazione dell'apply".into(),
            ));
        }
        let stash_commit = self
            .repository
            .find_commit(stash_id)
            .map_err(|error| GitError::Stash(error.to_string()))?;
        let base = stash_commit
            .parent_ids()
            .next()
            .ok_or_else(|| GitError::Stash("commit stash senza parent base".into()))?
            .detach();
        let base_tree = self
            .repository
            .find_commit(base)
            .map_err(|error| GitError::Stash(error.to_string()))?
            .tree_id()
            .map_err(|error| GitError::Stash(error.to_string()))?
            .detach();
        let stash_tree = stash_commit
            .tree_id()
            .map_err(|error| GitError::Stash(error.to_string()))?
            .detach();
        let changes = self.tree_changes(base_tree, stash_tree, false, Some(&cancel))?;
        let mut touched = Vec::with_capacity(changes.len());
        for change in &changes {
            check_safe_path(change.path())?;
            touched.push(change.path().to_path_buf());
        }
        self.ensure_update_topology(&changes, &cancel)?;
        self.check_update_clean(&touched, Arc::clone(&cancel))
            .map_err(|error| match error {
                GitError::Cancelled => GitError::Cancelled,
                other => GitError::Stash(other.to_string()),
            })?;
        if cancel.load(Ordering::Acquire) {
            return Err(GitError::Cancelled);
        }
        // L'indice è solo un indirizzo nel reflog e può cambiare dopo il
        // preflight se un altro processo fa `stash drop`. Applica l'OID che
        // abbiamo già validato, così non si reinterpretano i selettori.
        let selector = stash_id.to_string();
        let output = cli.run(
            &self.work_dir,
            &["stash", "apply", "--index", selector.as_str()],
            &cancel,
        )?;
        if output.success() {
            if pop {
                self.stash_drop_oid(&stash_id).map_err(|error| {
                    GitError::Stash(format!(
                        "modifiche applicate, ma non è stato possibile eliminare lo stash: {error}"
                    ))
                })?;
            }
            return Ok(StashApplyOutcome::Applied);
        }
        let mut conflicts = self
            .status()?
            .into_iter()
            .filter(|status| {
                status.index == Some(crate::ChangeKind::Conflict)
                    || status.worktree == Some(crate::ChangeKind::Conflict)
            })
            .map(|status| status.path)
            .collect::<Vec<_>>();
        conflicts.sort();
        conflicts.dedup();
        if !conflicts.is_empty() {
            Ok(StashApplyOutcome::Conflicts(conflicts))
        } else {
            Err(GitError::Stash(output.failure_text()))
        }
    }

    fn stash_oid(&self, index: usize) -> Result<gix::ObjectId, GitError> {
        let reference = self
            .repository
            .find_reference("refs/stash")
            .map_err(|error| GitError::Stash(error.to_string()))?;
        let mut platform = reference.log_iter();
        let Some(entries) = platform
            .all()
            .map_err(|error| GitError::Stash(error.to_string()))?
        else {
            return Err(GitError::Stash("reflog stash assente".into()));
        };
        let entries = entries
            .map(|entry| entry.map_err(|error| GitError::Stash(error.to_string())))
            .collect::<Result<Vec<_>, _>>()?;
        entries
            .iter()
            .rev()
            .nth(index)
            .map(|entry| entry.new_oid())
            .ok_or_else(|| GitError::Stash(format!("stash@{{{index}}} non esiste")))
    }
}
