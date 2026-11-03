//! Controllo dirty/staged, non checkout né autorizzazione a pubblicare dopo
//! il ritorno: verifica filesystem/attributi/alias completa ancora distinta.
use crate::{
    GitError, GitRepository,
    head_write::HeadSnapshot,
    index_write::{LockedIndex, MissingIndex},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
impl GitRepository {
    /// Diagnostica sincrona per i percorsi relativi di un aggiornamento tree.
    /// Non esegue fetch/apply/pull. Eseguire su worker; il risultato non è una
    /// lease trasferibile né prova di assenza di gare dopo il ritorno. Le
    /// lease native index/HEAD sono mantenute durante la sola verifica.
    pub fn check_update_clean(
        &self,
        touched: &[PathBuf],
        cancel: Arc<AtomicBool>,
    ) -> Result<(), GitError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(GitError::Cancelled);
        }
        for path in touched {
            crate::worktree::check_safe_path(path)?;
        }
        if self.merge_in_progress() {
            return Err(GitError::MergeInProgress);
        }
        let snapshot = HeadSnapshot::capture(&self.repository)?;
        let id = snapshot
            .id
            .ok_or_else(|| GitError::Worktree("HEAD non ha commit da integrare".into()))?;
        let index = LockedIndex::acquire(&self.repository, MissingIndex::Empty)
            .map_err(GitError::Worktree)?;
        // No-op owner29: tiene marker di tutta la catena, MAI commit della
        // transazione Delete/Only usata come lease di verifica.
        let _head_lease = snapshot.prepare(&self.repository, id, "verifica senza aggiornamento")?;
        let tree = self
            .repository
            .find_commit(id)
            .map_err(|e| GitError::Worktree(e.to_string()))?
            .tree_id()
            .map_err(|e| GitError::Worktree(e.to_string()))?
            .detach();
        self.ensure_clean_for(&index, tree, touched, cancel)
    }
    /// Solo dirty/staged. Tutti gli staged rifiutati, non soltanto i touched;
    /// worktree dirty/ignored fuori scope preservati. Status usa copia index
    /// leased, HEAD esplicito e forza confronto contenuti per i tracked toccati.
    pub(crate) fn ensure_clean_for(
        &self,
        index: &gix::index::File,
        head_tree: gix::ObjectId,
        touched: &[PathBuf],
        cancel: Arc<AtomicBool>,
    ) -> Result<(), GitError> {
        let overlaps = |path: &std::path::Path| {
            touched
                .iter()
                .any(|p| path.starts_with(p) || p.starts_with(path))
        };
        let mut checking = index.clone();
        for (entry, path) in checking.entries_mut_with_paths() {
            if cancel.load(Ordering::Relaxed) {
                return Err(GitError::Cancelled);
            }
            if entry.stage() != gix::index::entry::Stage::Unconflicted
                || entry
                    .flags
                    .contains(gix::index::entry::Flags::INTENT_TO_ADD)
            {
                return Err(GitError::PullDirty(
                    "l'index contiene conflitti o intent-to-add da conservare".into(),
                ));
            }
            let path =
                gix::path::try_from_bstr(path).map_err(|e| GitError::Worktree(e.to_string()))?;
            if overlaps(path.as_ref()) {
                entry.flags.remove(
                    gix::index::entry::Flags::ASSUME_VALID
                        | gix::index::entry::Flags::SKIP_WORKTREE
                        | gix::index::entry::Flags::UPTODATE,
                );
                entry.stat = gix::index::entry::Stat::default();
            }
        }
        // Non usare extension TREE per inferire che le entry siano immutate.
        checking.remove_tree();
        let platform = self
            .repository
            .status(gix::progress::Discard)
            .map_err(|e| GitError::Status(e.to_string()))?
            .index(gix::worktree::IndexPersistedOrInMemory::InMemory(checking))
            .head_tree(head_tree)
            .should_interrupt_owned(Arc::clone(&cancel))
            .untracked_files(gix::status::UntrackedFiles::Files)
            .dirwalk_options(|options| {
                options.emit_ignored(Some(gix::dir::walk::EmissionMode::Matching))
            });
        let entries = platform
            .into_iter(Vec::<gix::bstr::BString>::new())
            .map_err(|e| GitError::Status(e.to_string()))?;
        for entry in entries {
            let entry = entry.map_err(|e| GitError::Status(e.to_string()))?;
            if cancel.load(Ordering::Relaxed) {
                return Err(GitError::Cancelled);
            }
            let path = gix::path::try_from_bstr(entry.location())
                .map_err(|e| GitError::Worktree(e.to_string()))?;
            let dirty = match &entry {
                gix::status::Item::TreeIndex(_) => true,
                gix::status::Item::IndexWorktree(item) => {
                    overlaps(path.as_ref())
                        && (item.summary().is_some()
                            || matches!(
                                item,
                                gix::status::index_worktree::Item::DirectoryContents { .. }
                            ))
                }
            };
            if dirty {
                return Err(GitError::PullDirty(format!(
                    "«{}» ha lavoro locale da conservare",
                    path.display()
                )));
            }
        }
        // Native iterator interrotto può finire senza tutti gli eventi: EOF
        // non è una verifica completata. Errori nativi restano errori sopra.
        if cancel.load(Ordering::Relaxed) {
            return Err(GitError::Cancelled);
        }
        Ok(())
    }
}
