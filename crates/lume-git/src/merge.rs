//! Merge locale interattivo: Git CLI mantiene MERGE_HEAD e le voci index 1–3
//! quando occorre risolvere conflitti. Ogni operazione è esplicita e cancellabile.
use crate::{GitCli, GitError, GitRepository, cli_ops::validate_ref_name};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MergeOutcome {
    UpToDate {
        commit: String,
    },
    FastForward {
        commit: String,
    },
    Merged {
        commit: String,
    },
    Conflicts {
        commit: String,
        incoming: String,
        paths: Vec<PathBuf>,
    },
}

impl GitRepository {
    /// Percorsi ancora non risolti dal merge corrente, letti dall'index fresco.
    pub fn merge_conflicts(&self) -> Result<Vec<PathBuf>, GitError> {
        if !self.merge_in_progress() {
            return Ok(Vec::new());
        }
        let index = crate::index_write::fresh_index(
            &self.repository,
            crate::index_write::MissingIndex::Empty,
        )
        .map_err(GitError::Status)?;
        let mut paths = index
            .entries()
            .iter()
            .filter(|entry| entry.stage() != gix::index::entry::Stage::Unconflicted)
            .map(|entry| {
                gix::path::try_from_bstr(entry.path(&index))
                    .map(|path| path.into_owned())
                    .map_err(|error| GitError::Status(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    /// Avvia il merge di un branch locale esistente. Il worktree deve essere
    /// pulito; conflitti vengono lasciati a Git per staging/continue/abort.
    pub fn merge_branch(
        &self,
        cli: &GitCli,
        name: &str,
        cancel: &AtomicBool,
    ) -> Result<MergeOutcome, GitError> {
        if cancel.load(Ordering::Acquire) {
            return Err(GitError::Cancelled);
        }
        if self.merge_in_progress() {
            return Err(GitError::MergeInProgress);
        }
        validate_ref_name(name)?;
        if self.branch_name()?.is_none() {
            return Err(GitError::Merge(
                "HEAD staccato: seleziona un branch locale".into(),
            ));
        }
        let head = self
            .repository
            .head()
            .map_err(|error| GitError::Merge(error.to_string()))?
            .id()
            .ok_or_else(|| GitError::Merge("HEAD non contiene un commit".into()))?
            .detach();
        let reference_name = format!("refs/heads/{name}");
        let incoming = self
            .repository
            .find_reference(&reference_name)
            .map_err(|_| GitError::Merge("branch locale non trovato".into()))?
            .into_fully_peeled_id()
            .map_err(|error| GitError::Merge(error.to_string()))?
            .detach();
        if incoming == head {
            return Ok(MergeOutcome::UpToDate {
                commit: head.to_string(),
            });
        }
        if self
            .status()?
            .iter()
            .any(|file| file.index.is_some() || file.worktree.is_some())
        {
            return Err(GitError::PullDirty(
                "salva o metti in stash le modifiche prima del merge".into(),
            ));
        }
        let index = crate::index_write::fresh_index(
            &self.repository,
            crate::index_write::MissingIndex::Empty,
        )
        .map_err(GitError::Status)?;
        if index.entries().iter().any(|entry| {
            entry.stage() != gix::index::entry::Stage::Unconflicted
                || entry
                    .flags
                    .contains(gix::index::entry::Flags::INTENT_TO_ADD)
        }) {
            return Err(GitError::ConflictedIndex);
        }
        self.refuse_external_merge_drivers()?;
        let current_commit = self
            .repository
            .find_commit(head)
            .map_err(|error| GitError::Merge(error.to_string()))?;
        let current_tree = current_commit
            .tree_id()
            .map_err(|error| GitError::Merge(error.to_string()))?
            .detach();
        let incoming_commit = self
            .repository
            .find_commit(incoming)
            .map_err(|error| GitError::Merge(error.to_string()))?;
        let incoming_tree = incoming_commit
            .tree_id()
            .map_err(|error| GitError::Merge(error.to_string()))?
            .detach();
        for tree in [current_tree, incoming_tree] {
            let tree_index = self
                .repository
                .index_from_tree(&tree)
                .map_err(|error| GitError::Merge(error.to_string()))?;
            self.refuse_unowned_update_filters(
                &tree_index,
                gix::worktree::stack::state::attributes::Source::IdMapping,
                true,
                None,
                cancel,
            )?;
        }
        let output = cli.run(
            &self.work_dir,
            &["merge", "--no-edit", "--no-stat", "--", &reference_name],
            cancel,
        )?;
        if self.merge_in_progress() {
            let paths = self.merge_conflicts()?;
            return Ok(MergeOutcome::Conflicts {
                commit: head.to_string(),
                incoming: incoming.to_string(),
                paths,
            });
        }
        if !output.success() {
            return Err(GitError::Merge(output.failure_text()));
        }
        let current = self
            .repository
            .head()
            .map_err(|error| GitError::Merge(error.to_string()))?
            .id()
            .ok_or_else(|| GitError::Merge("HEAD non contiene un commit".into()))?
            .detach();
        if current == head {
            return Ok(MergeOutcome::UpToDate {
                commit: current.to_string(),
            });
        }
        let parent_count = self
            .repository
            .find_commit(current)
            .map_err(|error| GitError::Merge(error.to_string()))?
            .parent_ids()
            .count();
        if parent_count > 1 {
            Ok(MergeOutcome::Merged {
                commit: current.to_string(),
            })
        } else {
            Ok(MergeOutcome::FastForward {
                commit: current.to_string(),
            })
        }
    }

    /// Conclude il merge corrente solo quando tutte le voci conflittuali sono
    /// risolte e staged; `git merge --continue` preserva la semantica Git.
    pub fn merge_continue(&self, cli: &GitCli, cancel: &AtomicBool) -> Result<String, GitError> {
        if cancel.load(Ordering::Acquire) {
            return Err(GitError::Cancelled);
        }
        if !self.merge_in_progress() {
            return Err(GitError::MergeNotInProgress);
        }
        let index = crate::index_write::fresh_index(
            &self.repository,
            crate::index_write::MissingIndex::Empty,
        )
        .map_err(GitError::Status)?;
        if index.entries().iter().any(|entry| {
            entry.stage() != gix::index::entry::Stage::Unconflicted
                || entry
                    .flags
                    .contains(gix::index::entry::Flags::INTENT_TO_ADD)
        }) {
            return Err(GitError::ConflictedIndex);
        }
        let output = cli.run(&self.work_dir, &["merge", "--continue"], cancel)?;
        if !output.success() {
            return Err(GitError::Merge(output.failure_text()));
        }
        let head = cli.run(&self.work_dir, &["rev-parse", "--verify", "HEAD"], cancel)?;
        if !head.success() {
            return Err(GitError::Merge(head.failure_text()));
        }
        Ok(head.stdout.trim().to_owned())
    }

    /// Abbandona il merge corrente e lascia a Git il ripristino dello stato
    /// pre-merge. Il chiamante deve chiedere conferma prima di invocarlo.
    pub fn merge_abort(&self, cli: &GitCli, cancel: &AtomicBool) -> Result<(), GitError> {
        if cancel.load(Ordering::Acquire) {
            return Err(GitError::Cancelled);
        }
        if !self.merge_in_progress() {
            return Err(GitError::MergeNotInProgress);
        }
        let output = cli.run(&self.work_dir, &["merge", "--abort"], cancel)?;
        if output.success() {
            Ok(())
        } else {
            Err(GitError::Merge(output.failure_text()))
        }
    }
}
