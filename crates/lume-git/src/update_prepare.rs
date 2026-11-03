//! Preparazione e publish nativi per delta Git con owners index/HEAD e writer
//! protetti del core. Non sono ancora orchestrazione pull/fetch/merge/UI.
#[cfg(test)]
mod tests;
use crate::{
    GitError, GitRepository,
    head_write::{HeadSnapshot, PreparedHead},
    index_write::{LockedIndex, MissingIndex},
    tree_changes::TreeChange,
    worktree::{destination_metadata, git_path, linklike},
};
use std::{
    cell::Cell,
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
fn cancelled(cancel: &AtomicBool) -> Result<(), GitError> {
    if cancel.load(Ordering::Relaxed) {
        Err(GitError::Cancelled)
    } else {
        Ok(())
    }
}
fn fail(error: impl std::fmt::Display) -> GitError {
    GitError::Worktree(error.to_string())
}
enum PreparedContent {
    Atomic(lume_core::file_write::PreparedWrite),
    Payload(lume_core::file_write::PreparedPayload),
}
impl PreparedContent {
    fn abort(self) -> std::io::Result<()> {
        match self {
            Self::Atomic(write) => write.abort(),
            Self::Payload(payload) => payload.abort(),
        }
    }
}
struct PreparedFile {
    // Identità/mode richiesti conservati, non autorizzazione al futuro publish.
    _path: PathBuf,
    _mode: gix::object::tree::EntryMode,
    content: PreparedContent,
}
enum Placement {
    Atomic,
    Payload(PathBuf),
}
/// Trova un parent già esistente senza attraversare file rimossi proiettati.
/// Nessuna creazione directory né controllo dei permessi del futuro target.
fn placement(
    root: &Path,
    path: &Path,
    removed: &BTreeSet<&Path>,
    cancel: &AtomicBool,
) -> Result<Placement, GitError> {
    crate::worktree::check_safe_path(path)?;
    let root_meta = fs::symlink_metadata(root).map_err(fail)?;
    if !root_meta.is_dir() || linklike(&root_meta) || root_meta.permissions().readonly() {
        return Err(fail("radice preparazione non disponibile"));
    }
    let mut full = root.to_path_buf();
    let mut relative = PathBuf::new();
    let mut parts = path.components().peekable();
    while let Some(part) = parts.next() {
        cancelled(cancel)?;
        let parent = full.clone();
        full.push(part);
        relative.push(part);
        let last = parts.peek().is_none();
        let meta = match fs::symlink_metadata(&full) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(if last {
                    Placement::Atomic
                } else {
                    Placement::Payload(parent)
                });
            }
            Err(error) => return Err(fail(error)),
        };
        if linklike(&meta) || meta.permissions().readonly() || (!meta.is_dir() && !meta.is_file()) {
            return Err(fail(
                "percorso preparazione link/reparse/readonly o non regolare",
            ));
        }
        if last {
            return Ok(if meta.is_file() {
                Placement::Atomic
            } else {
                Placement::Payload(parent)
            });
        }
        if meta.is_dir() {
            continue;
        }
        if removed.contains(relative.as_path()) {
            return Ok(Placement::Payload(parent));
        }
        return Err(fail("file-antenato non previsto tra le rimozioni"));
    }
    Err(fail("percorso preparazione vuoto"))
}
fn cleanup(writes: Vec<PreparedFile>) -> (usize, usize) {
    let attempted = writes.len();
    let mut failed = 0;
    for write in writes {
        if write.content.abort().is_err() {
            failed += 1;
        }
    }
    (attempted, failed)
}
fn failed_preparation(source: GitError, writes: Vec<PreparedFile>) -> GitError {
    let (attempted, failed) = cleanup(writes);
    if failed == 0 {
        source
    } else {
        GitError::UpdatePrepareCleanup {
            source: Box::new(source),
            attempted,
            failed,
        }
    }
}
/// Piano non clonabile. Drop/abort esegue I/O: tenere e distruggere sul worker.
/// Campi/temporanei/transaction no-op non esposti. Solo piani legati a un
/// commit discendente esplicito possono pubblicare; non è rollback atomico.
#[must_use = "mantenere o abbandonare il piano sul worker, non nella UI"]
pub struct PreparedUpdate<'repo> {
    // Ordine Drop: cleanup temporanei PRIMA di rilasciare le lease.
    writes: Vec<PreparedFile>,
    removals: Vec<PathBuf>,
    // Contesto richiesto conservato, non freeze/token contro noncooperatori.
    _base_commit: gix::ObjectId,
    _target_tree: gix::ObjectId,
    _root: PathBuf,
    repository: &'repo GitRepository,
    can_publish: bool,
    target_index: gix::index::File,
    head: PreparedHead<'repo>,
    index: LockedIndex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdatePublishPhase {
    Revalidate,
    Worktree,
    Index,
    Reference,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdatePublishState {
    NotAttempted,
    Committed,
    OutcomeUnknown,
}

/// Ricevuta di una pubblicazione completata fino alla ref.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpdatePublishReceipt {
    updated_files: usize,
    removed_files: usize,
    created_directories: usize,
    removed_directories: usize,
}
impl UpdatePublishReceipt {
    pub fn updated_files(&self) -> usize {
        self.updated_files
    }
    pub fn removed_files(&self) -> usize {
        self.removed_files
    }
    pub fn created_directories(&self) -> usize {
        self.created_directories
    }
    pub fn removed_directories(&self) -> usize {
        self.removed_directories
    }
}

/// Fallimento che distingue effetti confermati, tentati e non iniziati.
/// `OutcomeUnknown` richiede una nuova ispezione del repository: non implica
/// rollback né autorizza a ripetere automaticamente l'operazione.
#[derive(Debug)]
pub struct UpdatePublishFailure {
    pub phase: UpdatePublishPhase,
    pub worktree: UpdatePublishState,
    pub index: UpdatePublishState,
    pub reference: UpdatePublishState,
    pub updated_files: usize,
    pub removed_files: usize,
    pub created_directories: usize,
    pub removed_directories: usize,
    pub cleanup_attempted: usize,
    pub cleanup_failed: usize,
    pub message: String,
}
impl std::fmt::Display for UpdatePublishFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "pubblicazione update interrotta in {:?}: {} (worktree={:?}, index={:?}, ref={:?}; file aggiornati {}, rimossi {}; cleanup {}/{})",
            self.phase,
            self.message,
            self.worktree,
            self.index,
            self.reference,
            self.updated_files,
            self.removed_files,
            self.cleanup_failed,
            self.cleanup_attempted
        )
    }
}
impl std::error::Error for UpdatePublishFailure {}
impl std::fmt::Debug for PreparedUpdate<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedUpdate").finish_non_exhaustive()
    }
}
impl PreparedUpdate<'_> {
    pub fn prepared_files(&self) -> usize {
        self.writes.len()
    }
    pub fn planned_deletions(&self) -> usize {
        self.removals.len()
    }
    /// Consuma tutti i temporanei sul worker, anche dopo un errore di cleanup.
    /// Ok copre solo close/unlink dei temporanei: Drop delle lease native non
    /// restituisce errori, non è release receipt di ref/index, join o rollback.
    /// Su errore l'owner è consumato: residui da verificare, nessun retry/replay.
    pub fn abort(self) -> Result<(), GitError> {
        let Self {
            writes,
            removals: _,
            _base_commit: _,
            _target_tree: _,
            _root: _,
            repository: _,
            can_publish: _,
            target_index: _,
            head,
            index,
        } = self;
        let (attempted, failed) = cleanup(writes);
        // Owners vivi durante TUTTI i cleanup, no commit del marker no-op.
        drop(head);
        drop(index);
        if failed == 0 {
            Ok(())
        } else {
            Err(GitError::UpdateCleanup { attempted, failed })
        }
    }

    /// Applica worktree, index e infine ref. Ogni errore riporta la fase e
    /// l'incertezza nativa: gli effetti precedenti non sono rollbackati.
    /// Operazione, cleanup e Drop vanno eseguiti sul worker.
    pub fn publish(
        self,
        cancel: Arc<AtomicBool>,
    ) -> Result<UpdatePublishReceipt, UpdatePublishFailure> {
        let Self {
            writes,
            removals,
            _base_commit: base,
            _target_tree: target,
            _root: root,
            repository,
            can_publish,
            target_index,
            head,
            mut index,
        } = self;
        let mut receipt = UpdatePublishReceipt {
            updated_files: 0,
            removed_files: 0,
            created_directories: 0,
            removed_directories: 0,
        };
        let mut worktree = UpdatePublishState::NotAttempted;
        let mut index_state = UpdatePublishState::NotAttempted;
        let mut reference = UpdatePublishState::NotAttempted;
        let extra_cleanup_attempted = Cell::new(0usize);
        let extra_cleanup_failed = Cell::new(0usize);
        let fail = |phase: UpdatePublishPhase,
                    message: String,
                    worktree: UpdatePublishState,
                    index: UpdatePublishState,
                    reference: UpdatePublishState,
                    receipt: UpdatePublishReceipt,
                    remaining: Vec<PreparedFile>| {
            let (cleanup_attempted, cleanup_failed) = cleanup(remaining);
            UpdatePublishFailure {
                phase,
                worktree,
                index,
                reference,
                updated_files: receipt.updated_files,
                removed_files: receipt.removed_files,
                created_directories: receipt.created_directories,
                removed_directories: receipt.removed_directories,
                cleanup_attempted: cleanup_attempted + extra_cleanup_attempted.get(),
                cleanup_failed: cleanup_failed + extra_cleanup_failed.get(),
                message,
            }
        };

        if !can_publish {
            return Err(fail(
                UpdatePublishPhase::Revalidate,
                "il piano è stato creato senza un commit destinazione".into(),
                worktree,
                index_state,
                reference,
                receipt,
                writes,
            ));
        }

        let touched = removals
            .iter()
            .cloned()
            .chain(writes.iter().map(|w| w._path.clone()))
            .collect::<Vec<_>>();
        let current_tree = match repository.repository.find_commit(base) {
            Ok(commit) => match commit.tree_id() {
                Ok(tree) => tree.detach(),
                Err(error) => {
                    return Err(fail(
                        UpdatePublishPhase::Revalidate,
                        error.to_string(),
                        worktree,
                        index_state,
                        reference,
                        receipt,
                        writes,
                    ));
                }
            },
            Err(error) => {
                return Err(fail(
                    UpdatePublishPhase::Revalidate,
                    error.to_string(),
                    worktree,
                    index_state,
                    reference,
                    receipt,
                    writes,
                ));
            }
        };
        if let Err(error) =
            repository.ensure_clean_for(&index, current_tree, &touched, Arc::clone(&cancel))
        {
            return Err(fail(
                UpdatePublishPhase::Revalidate,
                error.to_string(),
                worktree,
                index_state,
                reference,
                receipt,
                writes,
            ));
        }
        let changes = match repository.tree_changes(current_tree, target, false, Some(&cancel)) {
            Ok(changes) => changes,
            Err(error) => {
                return Err(fail(
                    UpdatePublishPhase::Revalidate,
                    error.to_string(),
                    worktree,
                    index_state,
                    reference,
                    receipt,
                    writes,
                ));
            }
        };
        if let Err(error) = repository.ensure_update_topology(&changes, &cancel) {
            return Err(fail(
                UpdatePublishPhase::Revalidate,
                error.to_string(),
                worktree,
                index_state,
                reference,
                receipt,
                writes,
            ));
        }
        if let Err(error) = repository.refuse_unowned_update_filters(
            &index,
            gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
            false,
            None,
            &cancel,
        ) {
            return Err(fail(
                UpdatePublishPhase::Revalidate,
                error.to_string(),
                worktree,
                index_state,
                reference,
                receipt,
                writes,
            ));
        }
        if let Err(error) = repository.refuse_unowned_update_filters(
            &target_index,
            gix::worktree::stack::state::attributes::Source::IdMapping,
            true,
            None,
            &cancel,
        ) {
            return Err(fail(
                UpdatePublishPhase::Revalidate,
                error.to_string(),
                worktree,
                index_state,
                reference,
                receipt,
                writes,
            ));
        }
        let committer = match repository.repository.committer().transpose() {
            Ok(committer) => committer,
            Err(error) => {
                return Err(fail(
                    UpdatePublishPhase::Revalidate,
                    error.to_string(),
                    worktree,
                    index_state,
                    reference,
                    receipt,
                    writes,
                ));
            }
        };
        if let Err(error) = cancelled(&cancel) {
            return Err(fail(
                UpdatePublishPhase::Revalidate,
                error.to_string(),
                worktree,
                index_state,
                reference,
                receipt,
                writes,
            ));
        }

        // Rimozioni prima dei put permettono transizioni file↔directory.
        worktree = UpdatePublishState::OutcomeUnknown;
        let mut ordered_removals = removals;
        ordered_removals.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for path in &ordered_removals {
            if let Err(error) = cancelled(&cancel) {
                return Err(fail(
                    UpdatePublishPhase::Worktree,
                    error.to_string(),
                    worktree,
                    index_state,
                    reference,
                    receipt,
                    writes,
                ));
            }
            let full = root.join(path);
            match fs::symlink_metadata(&full) {
                Ok(meta)
                    if meta.is_file() && !linklike(&meta) && !meta.permissions().readonly() => {}
                Ok(_) => {
                    return Err(fail(
                        UpdatePublishPhase::Worktree,
                        format!(
                            "rimozione rifiutata per tipo/percorso cambiato: {}",
                            path.display()
                        ),
                        worktree,
                        index_state,
                        reference,
                        receipt,
                        writes,
                    ));
                }
                Err(error) => {
                    return Err(fail(
                        UpdatePublishPhase::Worktree,
                        error.to_string(),
                        worktree,
                        index_state,
                        reference,
                        receipt,
                        writes,
                    ));
                }
            }
            if let Err(error) = fs::remove_file(full) {
                return Err(fail(
                    UpdatePublishPhase::Worktree,
                    error.to_string(),
                    worktree,
                    index_state,
                    reference,
                    receipt,
                    writes,
                ));
            }
            receipt.removed_files += 1;
        }

        // Per i payload, elimina solo directory vuote che occupano il nome
        // file destinazione; la topologia è stata rivalidata sopra.
        let pending = writes;
        let payload_paths = pending
            .iter()
            .filter_map(|write| match write.content {
                PreparedContent::Payload(_) => Some(write._path.clone()),
                PreparedContent::Atomic(_) => None,
            })
            .collect::<Vec<_>>();
        for path in &payload_paths {
            let target_path = root.join(path);
            if matches!(fs::symlink_metadata(&target_path), Ok(meta) if meta.is_dir() && !linklike(&meta))
            {
                let mut dirs = ordered_removals
                    .iter()
                    .filter_map(|removed| {
                        removed
                            .parent()
                            .filter(|parent| parent.starts_with(path))
                            .map(Path::to_path_buf)
                    })
                    .collect::<Vec<_>>();
                dirs.push(path.clone());
                dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
                dirs.dedup();
                for dir in dirs {
                    let full = root.join(&dir);
                    match fs::symlink_metadata(&full) {
                        Ok(meta) if meta.is_dir() && !linklike(&meta) => {
                            if let Err(error) = fs::remove_dir(&full) {
                                return Err(fail(
                                    UpdatePublishPhase::Worktree,
                                    error.to_string(),
                                    worktree,
                                    index_state,
                                    reference,
                                    receipt,
                                    pending,
                                ));
                            }
                            receipt.removed_directories += 1;
                        }
                        Ok(_) => {
                            return Err(fail(
                                UpdatePublishPhase::Worktree,
                                "directory destinazione cambiata".into(),
                                worktree,
                                index_state,
                                reference,
                                receipt,
                                pending,
                            ));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(fail(
                                UpdatePublishPhase::Worktree,
                                error.to_string(),
                                worktree,
                                index_state,
                                reference,
                                receipt,
                                pending,
                            ));
                        }
                    }
                }
            }
        }

        let mut pending_iter = pending.into_iter();
        while let Some(write) = pending_iter.next() {
            if let Err(error) = cancelled(&cancel) {
                let remaining = pending_iter.collect();
                return Err(fail(
                    UpdatePublishPhase::Worktree,
                    error.to_string(),
                    worktree,
                    index_state,
                    reference,
                    receipt,
                    remaining,
                ));
            }
            let full = root.join(&write._path);
            let result = match write.content {
                PreparedContent::Atomic(prepared) => prepared.commit(),
                PreparedContent::Payload(payload) => {
                    if let Err(error) = ensure_private_parents(
                        &root,
                        &write._path,
                        &mut receipt.created_directories,
                    ) {
                        extra_cleanup_attempted.set(extra_cleanup_attempted.get() + 1);
                        let cleanup_error = payload.abort().err();
                        if cleanup_error.is_some() {
                            extra_cleanup_failed.set(extra_cleanup_failed.get() + 1);
                        }
                        return Err(fail(
                            UpdatePublishPhase::Worktree,
                            cleanup_error.map_or_else(
                                || error.to_string(),
                                |cleanup| format!("{error}; cleanup payload: {cleanup}"),
                            ),
                            worktree,
                            index_state,
                            reference,
                            receipt,
                            pending_iter.collect(),
                        ));
                    }
                    let (prepared, source_cleanup) = payload.prepare_at(&full).into_results();
                    extra_cleanup_attempted.set(extra_cleanup_attempted.get() + 1);
                    if source_cleanup.is_err() {
                        extra_cleanup_failed.set(extra_cleanup_failed.get() + 1);
                    }
                    match (prepared, source_cleanup) {
                        (Ok(prepared), Ok(())) => prepared.commit(),
                        (Ok(prepared), Err(error)) => {
                            extra_cleanup_attempted.set(extra_cleanup_attempted.get() + 1);
                            let cleanup_error = prepared
                                .abort()
                                .err()
                                .map(|e| format!("; cleanup writer: {e}"))
                                .unwrap_or_default();
                            if !cleanup_error.is_empty() {
                                extra_cleanup_failed.set(extra_cleanup_failed.get() + 1);
                            }
                            return Err(fail(
                                UpdatePublishPhase::Worktree,
                                format!("cleanup payload: {error}{cleanup_error}"),
                                worktree,
                                index_state,
                                reference,
                                receipt,
                                pending_iter.collect(),
                            ));
                        }
                        (Err(error), cleanup) => {
                            let cleanup_error = cleanup
                                .err()
                                .map(|e| format!("; cleanup payload: {e}"))
                                .unwrap_or_default();
                            return Err(fail(
                                UpdatePublishPhase::Worktree,
                                format!("preparazione destinazione: {error}{cleanup_error}"),
                                worktree,
                                index_state,
                                reference,
                                receipt,
                                pending_iter.collect(),
                            ));
                        }
                    }
                }
            };
            if let Err(error) = result {
                let remaining = pending_iter.collect();
                return Err(fail(
                    UpdatePublishPhase::Worktree,
                    error.to_string(),
                    worktree,
                    index_state,
                    reference,
                    receipt,
                    remaining,
                ));
            }
            receipt.updated_files += 1;
        }
        worktree = UpdatePublishState::Committed;

        if let Err(error) = cancelled(&cancel) {
            return Err(fail(
                UpdatePublishPhase::Index,
                error.to_string(),
                worktree,
                index_state,
                reference,
                receipt,
                Vec::new(),
            ));
        }
        *index = target_index;
        index_state = UpdatePublishState::OutcomeUnknown;
        if let Err(error) = index.commit() {
            return Err(fail(
                UpdatePublishPhase::Index,
                error,
                worktree,
                index_state,
                reference,
                receipt,
                Vec::new(),
            ));
        }
        index_state = UpdatePublishState::Committed;

        if let Err(error) = cancelled(&cancel) {
            return Err(fail(
                UpdatePublishPhase::Reference,
                error.to_string(),
                worktree,
                index_state,
                reference,
                receipt,
                Vec::new(),
            ));
        }
        reference = UpdatePublishState::OutcomeUnknown;
        if let Err(error) = head.commit(committer) {
            return Err(fail(
                UpdatePublishPhase::Reference,
                error.to_string(),
                worktree,
                index_state,
                reference,
                receipt,
                Vec::new(),
            ));
        }
        Ok(receipt)
    }
}

fn ensure_private_parents(root: &Path, path: &Path, created: &mut usize) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("parent destinazione assente"))?;
    let mut current = root.to_path_buf();
    let mut relative = PathBuf::new();
    for component in parent.components() {
        relative.push(component);
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() && !linklike(&meta) && !meta.permissions().readonly() => {}
            Ok(_) => return Err(std::io::Error::other("parent destinazione non regolare")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let (phase, result) =
                    lume_core::file_write::create_private_directory(&current).into_parts();
                if phase == lume_core::file_write::DirectoryCreationPhase::Created {
                    *created += 1;
                }
                result?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
impl GitRepository {
    /// Prepara file del delta dal HEAD catturato al tree richiesto. Owners
    /// cooperativi fino Drop; target index/IdMapping costruiti una sola volta.
    /// Nessun filesystem/index/ref pubblicato, ma filters possono avere effetti.
    /// Parent mancanti/file↔directory: payload protetti nel parent esistente,
    /// senza preparare permessi o filesystem del target futuro. Nessuna mkdir
    /// o applicazione simulata: i piani tree-only restano abort-only.
    pub fn prepare_update(
        &self,
        new: gix::ObjectId,
        cancel: Arc<AtomicBool>,
    ) -> Result<PreparedUpdate<'_>, GitError> {
        self.prepare_update_inner(new, None, None, None, cancel)
    }
    /// Prepara il delta verso un commit già presente nell'ODB e tiene la
    /// transazione HEAD pronta a pubblicarlo. Non scrive worktree/index/ref.
    /// `PreparedUpdate::publish` applica worktree/index/ref con esito parziale esplicito.
    pub fn prepare_update_to(
        &self,
        new: gix::ObjectId,
        destination: gix::ObjectId,
        cancel: Arc<AtomicBool>,
    ) -> Result<PreparedUpdate<'_>, GitError> {
        self.prepare_update_inner(new, Some(destination), None, None, cancel)
    }
    pub(crate) fn prepare_update_from_snapshot(
        &self,
        new: gix::ObjectId,
        destination: gix::ObjectId,
        expected: &HeadSnapshot,
        reflog_message: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<PreparedUpdate<'_>, GitError> {
        self.prepare_update_inner(
            new,
            Some(destination),
            Some(expected),
            Some(reflog_message),
            cancel,
        )
    }
    fn prepare_update_inner(
        &self,
        new: gix::ObjectId,
        destination: Option<gix::ObjectId>,
        expected: Option<&HeadSnapshot>,
        reflog_message: Option<&str>,
        cancel: Arc<AtomicBool>,
    ) -> Result<PreparedUpdate<'_>, GitError> {
        cancelled(&cancel)?;
        if self.merge_in_progress() {
            return Err(GitError::MergeInProgress);
        }
        let current_snapshot = HeadSnapshot::capture(&self.repository)?;
        if expected.is_some_and(|expected| !expected.same_chain(&current_snapshot)) {
            return Err(fail("HEAD/branch è cambiato durante il fetch"));
        }
        let snapshot = expected.unwrap_or(&current_snapshot);
        let id = snapshot
            .id
            .ok_or_else(|| fail("HEAD non ha commit da preparare"))?;
        let index = LockedIndex::acquire(&self.repository, MissingIndex::Empty).map_err(fail)?;
        let head_target = if let Some(destination) = destination {
            let commit = self.repository.find_commit(destination).map_err(fail)?;
            let destination_tree = commit.tree_id().map_err(fail)?.detach();
            if destination_tree != new {
                return Err(fail(
                    "tree richiesto diverso dal tree del commit destinazione",
                ));
            }
            if destination != id
                && self
                    .repository
                    .merge_base(id, destination)
                    .map_err(fail)?
                    .detach()
                    != id
            {
                return Err(fail("commit destinazione non discende da HEAD"));
            }
            destination
        } else {
            id
        };
        let head = snapshot.prepare(
            &self.repository,
            head_target,
            if destination.is_some() {
                reflog_message.unwrap_or("pull: aggiornamento")
            } else {
                "preparazione senza pubblicazione"
            },
        )?;
        let old = self
            .repository
            .find_commit(id)
            .map_err(fail)?
            .tree_id()
            .map_err(fail)?
            .detach();
        let changes = self.tree_changes(old, new, false, Some(&cancel))?;
        self.ensure_update_topology(&changes, &cancel)?;
        let touched = changes
            .iter()
            .map(|change| change.path().to_path_buf())
            .collect::<Vec<_>>();
        let target_index = self.repository.index_from_tree(&new).map_err(fail)?;
        self.refuse_unowned_update_filters(
            &index,
            gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
            false,
            None,
            &cancel,
        )?;
        self.refuse_unowned_update_filters(
            &target_index,
            gix::worktree::stack::state::attributes::Source::IdMapping,
            true,
            None,
            &cancel,
        )?;
        self.ensure_clean_for(&index, old, &touched, Arc::clone(&cancel))?;
        let removed = changes
            .iter()
            .filter_map(|change| match change {
                TreeChange::Remove { path, .. } => Some(path.as_path()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        // Tutte le collocazioni controllate prima dei temp, poi rilette per file.
        for change in &changes {
            if let TreeChange::Put { path, .. } = change {
                placement(&self.work_dir, path, &removed, &cancel)?;
            }
        }
        cancelled(&cancel)?;
        let mut pipeline = self.attribute_pipeline(
            &target_index,
            gix::worktree::stack::state::attributes::Source::IdMapping,
        )?;
        let mut writes = Vec::new();
        let mut removals = Vec::new();
        let result = (|| {
            for change in &changes {
                cancelled(&cancel)?;
                let TreeChange::Put { path, after, .. } = change else {
                    if let TreeChange::Remove { path, .. } = change {
                        removals.push(path.clone());
                    }
                    continue;
                };
                placement(&self.work_dir, path, &removed, &cancel)?;
                let blob = self.repository.find_object(after.id).map_err(fail)?;
                if blob.kind != gix::objs::Kind::Blob {
                    return Err(fail("entry da preparare non riferisce un blob"));
                }
                cancelled(&cancel)?;
                let relative = git_path(path)?;
                let converted = pipeline
                    .convert_to_worktree(
                        &blob.data,
                        relative.as_ref(),
                        gix::filter::plumbing::pipeline::convert::to_worktree::Options {
                            can_delay: gix::filter::plumbing::driver::apply::Delay::Forbid,
                            ..Default::default()
                        },
                    )
                    .map_err(fail)?;
                let bytes = converted
                    .as_bytes()
                    .ok_or_else(|| fail("filtro non materializzato: nessun checkout pubblicato"))?;
                cancelled(&cancel)?;
                let (content, changed) = match placement(&self.work_dir, path, &removed, &cancel)? {
                    Placement::Atomic => {
                        destination_metadata(&self.work_dir, path)?;
                        let full = self.work_dir.join(path);
                        let expected = std::path::absolute(&full).map_err(fail)?;
                        let prepared =
                            lume_core::file_write::prepare_atomic(&full, bytes).map_err(fail)?;
                        let changed = prepared.destination() != expected;
                        #[cfg(unix)]
                        let prepared = prepared.with_executable(
                            after.mode.kind() == gix::object::tree::EntryKind::BlobExecutable,
                        );
                        (PreparedContent::Atomic(prepared), changed)
                    }
                    Placement::Payload(parent) => (
                        PreparedContent::Payload(
                            lume_core::file_write::prepare_payload(&parent, bytes).map_err(fail)?,
                        ),
                        false,
                    ),
                };
                writes.push(PreparedFile {
                    _path: path.clone(),
                    _mode: after.mode,
                    content,
                });
                if changed {
                    return Err(fail("destinazione cambiata durante prepare"));
                }
                // blob/converted cadono qui: non tutti i payload in un Vec RAM.
            }
            cancelled(&cancel)
        })();
        if let Err(error) = result {
            return Err(failed_preparation(error, writes));
        }
        Ok(PreparedUpdate {
            writes,
            removals,
            _base_commit: id,
            _target_tree: new,
            _root: self.work_dir.clone(),
            repository: self,
            can_publish: destination.is_some(),
            target_index,
            head,
            index,
        })
    }
}
