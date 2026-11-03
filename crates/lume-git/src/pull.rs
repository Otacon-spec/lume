//! Coordinamento nativo upstream/fetch/merge e pubblicazione di un pull.
use crate::{
    GitError, GitRepository, UpdatePublishFailure, UpdatePublishReceipt, head_write::HeadSnapshot,
};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PullFetchState {
    NotStarted,
    OutcomeUnknown,
    Completed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PullFailureStage {
    Configuration,
    Fetch,
    Resolve,
    Merge,
    Prepare,
    Publish,
}

/// Errore con distinzione esplicita tra fetch non avviato, incerto o completato.
#[derive(Debug)]
pub struct PullFailure {
    pub stage: PullFailureStage,
    pub fetch: PullFetchState,
    pub publish: Option<UpdatePublishFailure>,
    pub message: String,
}
impl std::fmt::Display for PullFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "pull interrotto in {:?} (fetch {:?}): {}",
            self.stage, self.fetch, self.message
        )?;
        if let Some(publish) = &self.publish {
            write!(f, "; {publish}")?;
        }
        Ok(())
    }
}
impl std::error::Error for PullFailure {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_failure_mostra_l_esito_parziale_della_pubblicazione() {
        let failure = PullFailure {
            stage: PullFailureStage::Publish,
            fetch: PullFetchState::Completed,
            publish: Some(UpdatePublishFailure {
                phase: crate::UpdatePublishPhase::Index,
                worktree: crate::UpdatePublishState::Committed,
                index: crate::UpdatePublishState::OutcomeUnknown,
                reference: crate::UpdatePublishState::NotAttempted,
                updated_files: 2,
                removed_files: 1,
                created_directories: 1,
                removed_directories: 0,
                cleanup_attempted: 3,
                cleanup_failed: 1,
                message: "errore sintetico per il formato dell'errore".into(),
            }),
            message: "pubblicazione non completata".into(),
        };

        let message = failure.to_string();

        assert!(message.contains("worktree=Committed"), "{message}");
        assert!(message.contains("index=OutcomeUnknown"), "{message}");
        assert!(message.contains("ref=NotAttempted"), "{message}");
        assert!(
            message.contains("file aggiornati 2, rimossi 1"),
            "{message}"
        );
        assert!(message.contains("cleanup 1/3"), "{message}");
    }
}

#[derive(Debug)]
#[must_use = "gestire esplicitamente il risultato del pull"]
pub enum PullOutcome {
    UpToDate {
        commit: String,
    },
    LocalAhead {
        local_commit: String,
        upstream_commit: String,
    },
    FastForward {
        commit: String,
        receipt: UpdatePublishReceipt,
    },
    Merged {
        commit: String,
        receipt: UpdatePublishReceipt,
    },
    /// Fetch riuscito; conflitti restituiti senza worktree/index/ref checkout.
    NeedsMerge {
        upstream_commit: String,
        conflicts: Vec<PathBuf>,
    },
}

fn failure(
    stage: PullFailureStage,
    fetch: PullFetchState,
    error: impl std::fmt::Display,
) -> PullFailure {
    PullFailure {
        stage,
        fetch,
        publish: None,
        message: error.to_string(),
    }
}

impl GitRepository {
    /// Fetch dell'upstream configurato, poi fast-forward o merge nativo.
    /// Conflitti non pubblicano worktree/index/ref; gli errori riportano se il
    /// fetch può aver aggiornato refs e, per errori publish, i suoi esiti parziali.
    pub fn pull_with(&self, cancel: Arc<AtomicBool>) -> Result<PullOutcome, PullFailure> {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(failure(
                PullFailureStage::Configuration,
                PullFetchState::NotStarted,
                GitError::Cancelled,
            ));
        }
        if self.merge_in_progress() {
            return Err(failure(
                PullFailureStage::Configuration,
                PullFetchState::NotStarted,
                GitError::MergeInProgress,
            ));
        }
        let upstream = self
            .upstream()
            .map_err(|error| {
                failure(
                    PullFailureStage::Configuration,
                    PullFetchState::NotStarted,
                    error,
                )
            })?
            .ok_or_else(|| {
                failure(
                    PullFailureStage::Configuration,
                    PullFetchState::NotStarted,
                    GitError::NoUpstream("HEAD".into()),
                )
            })?;
        self.ensure_pull_transport(&upstream.remote)
            .map_err(|error| {
                failure(
                    PullFailureStage::Configuration,
                    PullFetchState::NotStarted,
                    error,
                )
            })?;
        let snapshot = HeadSnapshot::capture(&self.repository).map_err(|error| {
            failure(
                PullFailureStage::Configuration,
                PullFetchState::NotStarted,
                error,
            )
        })?;
        let local = snapshot.id.ok_or_else(|| {
            failure(
                PullFailureStage::Configuration,
                PullFetchState::NotStarted,
                GitError::NoUpstream("HEAD senza commit".into()),
            )
        })?;

        self.fetch_named(&upstream.remote, &cancel)
            .map_err(|error| {
                failure(
                    PullFailureStage::Fetch,
                    PullFetchState::OutcomeUnknown,
                    error,
                )
            })?;
        let fetched = PullFetchState::Completed;
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(failure(
                PullFailureStage::Resolve,
                fetched,
                GitError::Cancelled,
            ));
        }
        let current_upstream = self
            .upstream()
            .map_err(|error| failure(PullFailureStage::Resolve, fetched, error))?;
        if current_upstream.as_ref() != Some(&upstream) {
            return Err(failure(
                PullFailureStage::Resolve,
                fetched,
                "configurazione upstream cambiata durante il fetch",
            ));
        }
        let reference = self
            .repository
            .find_reference(&upstream.tracking_ref)
            .map_err(|error| failure(PullFailureStage::Resolve, fetched, error))?;
        let remote = match reference.target().into_owned() {
            gix::refs::Target::Object(id) => id,
            gix::refs::Target::Symbolic(_) => {
                return Err(failure(
                    PullFailureStage::Resolve,
                    fetched,
                    "tracking ref non diretta",
                ));
            }
        };
        let remote_commit = self
            .repository
            .find_commit(remote)
            .map_err(|error| failure(PullFailureStage::Resolve, fetched, error))?;
        snapshot
            .ensure_current(&self.repository)
            .map_err(|error| failure(PullFailureStage::Resolve, fetched, error))?;

        let local_text = local.to_string();
        let remote_text = remote.to_string();
        if local == remote {
            return Ok(PullOutcome::UpToDate { commit: local_text });
        }
        let base = self
            .repository
            .merge_base(local, remote)
            .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?
            .detach();
        if base == local {
            let tree = remote_commit
                .tree_id()
                .map_err(|error| failure(PullFailureStage::Resolve, fetched, error))?;
            let tree = tree.detach();
            return self
                .publish_pull_tree(tree, remote, &snapshot, Arc::clone(&cancel), fetched, false)
                .map(|receipt| PullOutcome::FastForward {
                    commit: remote_text,
                    receipt,
                });
        }
        if base == remote {
            snapshot
                .ensure_current(&self.repository)
                .map_err(|error| failure(PullFailureStage::Resolve, fetched, error))?;
            return Ok(PullOutcome::LocalAhead {
                local_commit: local_text,
                upstream_commit: remote_text,
            });
        }

        self.refuse_external_merge_drivers()
            .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?;
        let local_commit = self
            .repository
            .find_commit(local)
            .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?;
        let local_tree = local_commit
            .tree_id()
            .map(|tree| tree.detach())
            .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?;
        let remote_tree = remote_commit
            .tree_id()
            .map(|tree| tree.detach())
            .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?;
        let local_index = self
            .repository
            .index_from_tree(&local_tree)
            .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?;
        let remote_index = self
            .repository
            .index_from_tree(&remote_tree)
            .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?;
        let cancel_flag = cancel.as_ref();
        self.refuse_unowned_update_filters(
            &local_index,
            gix::worktree::stack::state::attributes::Source::IdMapping,
            false,
            None,
            cancel_flag,
        )
        .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?;
        self.refuse_unowned_update_filters(
            &remote_index,
            gix::worktree::stack::state::attributes::Source::IdMapping,
            false,
            None,
            cancel_flag,
        )
        .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?;
        let options = self
            .repository
            .tree_merge_options()
            .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?;
        let merged = self
            .repository
            .merge_commits(
                local,
                remote,
                gix::merge::blob::builtin_driver::text::Labels::default(),
                options.into(),
            )
            .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?;
        let unresolved = gix::merge::tree::TreatAsUnresolved::git();
        if merged.tree_merge.has_unresolved_conflicts(unresolved) {
            snapshot
                .ensure_current(&self.repository)
                .map_err(|error| failure(PullFailureStage::Resolve, fetched, error))?;
            let mut conflicts = merged
                .tree_merge
                .conflicts
                .iter()
                .filter(|conflict| conflict.is_unresolved(unresolved))
                .flat_map(|conflict| [conflict.ours.location(), conflict.theirs.location()])
                .map(|path| gix::path::try_from_bstr(path).map(|path| path.into_owned()))
                .collect::<Result<Vec<PathBuf>, _>>()
                .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?;
            conflicts.sort();
            conflicts.dedup();
            return Ok(PullOutcome::NeedsMerge {
                upstream_commit: remote_text,
                conflicts,
            });
        }
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(failure(
                PullFailureStage::Merge,
                fetched,
                GitError::Cancelled,
            ));
        }
        let mut tree = merged.tree_merge.tree;
        let merged_tree = tree
            .write()
            .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?
            .detach();
        let commit = self
            .repository
            .new_commit(
                format!("Merge {}", upstream.tracking_ref),
                merged_tree,
                [local, remote],
            )
            .map_err(|error| failure(PullFailureStage::Merge, fetched, error))?
            .id()
            .detach();
        let receipt = self.publish_pull_tree(
            merged_tree,
            commit,
            &snapshot,
            Arc::clone(&cancel),
            fetched,
            true,
        )?;
        Ok(PullOutcome::Merged {
            commit: commit.to_string(),
            receipt,
        })
    }

    fn publish_pull_tree(
        &self,
        tree: gix::ObjectId,
        destination: gix::ObjectId,
        snapshot: &HeadSnapshot,
        cancel: Arc<AtomicBool>,
        fetched: PullFetchState,
        merge: bool,
    ) -> Result<UpdatePublishReceipt, PullFailure> {
        let reflog_message = if merge {
            "pull: merge"
        } else {
            "pull: fast-forward"
        };
        let prepared = self
            .prepare_update_from_snapshot(
                tree,
                destination,
                snapshot,
                reflog_message,
                cancel.clone(),
            )
            .map_err(|error| failure(PullFailureStage::Prepare, fetched, error))?;
        match prepared.publish(cancel) {
            Ok(receipt) => Ok(receipt),
            Err(publish) => Err(PullFailure {
                stage: PullFailureStage::Publish,
                fetch: fetched,
                message: if merge {
                    "merge commit creato, pubblicazione non completata"
                } else {
                    "fast-forward fetch completato, pubblicazione non completata"
                }
                .into(),
                publish: Some(publish),
            }),
        }
    }

    pub(crate) fn refuse_external_merge_drivers(&self) -> Result<(), GitError> {
        let config = self.repository.config_snapshot();
        if config
            .sections_by_name("merge")
            .into_iter()
            .flatten()
            .any(|section| {
                section
                    .header()
                    .subsection_name()
                    .is_some_and(|name| !name.is_empty())
                    && section.value("driver").is_some()
            })
        {
            return Err(GitError::Worktree(
                "driver di merge esterno configurato: esecuzione non supportata dal worker Git"
                    .into(),
            ));
        }
        Ok(())
    }

    /// Evita trasporti che possono eseguire SSH o remote-helper fuori dal
    /// worker confinato; HTTP(S), git:// e filesystem usano trasporti nativi.
    fn ensure_pull_transport(&self, remote_name: &str) -> Result<(), GitError> {
        let remote = self
            .repository
            .find_remote(remote_name)
            .map_err(|error| GitError::Fetch(error.to_string()))?;
        let url = remote
            .url(gix::remote::Direction::Fetch)
            .ok_or_else(|| GitError::Fetch("URL fetch remoto assente".into()))?;
        match &url.scheme {
            gix::url::Scheme::File
            | gix::url::Scheme::Git
            | gix::url::Scheme::Http
            | gix::url::Scheme::Https => Ok(()),
            gix::url::Scheme::Ssh => Err(GitError::Fetch(
                "trasporto SSH sospeso finché il processo figlio non è confinato e raccolto".into(),
            )),
            gix::url::Scheme::Ext
            | gix::url::Scheme::Helper(_)
            | gix::url::Scheme::HelperUrl(_) => Err(GitError::Fetch(
                "remote-helper esterno non supportato dal worker Git".into(),
            )),
        }
    }
}
