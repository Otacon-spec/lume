//! Conversione blob→worktree, non apply/preflight né lease di pubblicazione.
use crate::{GitError, GitRepository, worktree::git_path};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

impl GitRepository {
    /// Legge un file regolare del tree e restituisce i byte di checkout.
    /// Il risultato NON autorizza a scrivere: nessun file/index/ref aggiornato
    /// qui, ma filtri configurati possono avviare helper con effetti propri.
    /// Operazione bloccante su worker, cancellabile fra fasi, non durante I/O
    /// o helper nativo già iniziati. Vec/blob/index non sono limiti RAM.
    pub fn checkout_content(
        &self,
        tree: gix::ObjectId,
        relative: &Path,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, GitError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(GitError::Cancelled);
        }
        let path = git_path(relative)?;
        let index = self
            .repository
            .index_from_tree(&tree)
            .map_err(|e| GitError::Worktree(e.to_string()))?;
        let entry = index.entry_by_path(path.as_ref()).ok_or_else(|| {
            GitError::Worktree("il tree non contiene un file regolare al percorso richiesto".into())
        })?;
        if !matches!(
            entry.mode,
            gix::index::entry::Mode::FILE | gix::index::entry::Mode::FILE_EXECUTABLE
        ) {
            return Err(GitError::Worktree(
                "checkout di link/submodule non supportato".into(),
            ));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(GitError::Cancelled);
        }
        // Solo il file richiesto: driver selezionati da altri file del tree
        // non autorizzano helper qui, ma neppure impediscono questa lettura.
        self.refuse_unowned_update_filters(
            &index,
            gix::worktree::stack::state::attributes::Source::IdMapping,
            true,
            Some(path.as_ref()),
            cancel,
        )?;
        let blob = self
            .repository
            .find_object(entry.id)
            .map_err(|e| GitError::Worktree(e.to_string()))?;
        if blob.kind != gix::object::Kind::Blob {
            return Err(GitError::Worktree(
                "entry file non riferisce un blob".into(),
            ));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(GitError::Cancelled);
        }
        // IdMapping non riprende attributi rimossi nel tree dal worktree
        // vecchio. Attributi globali/info restano policy native gix.
        let mut pipeline = self.attribute_pipeline(
            &index,
            gix::worktree::stack::state::attributes::Source::IdMapping,
        )?;
        let converted = pipeline
            .convert_to_worktree(
                &blob.data,
                path.as_ref(),
                gix::filter::plumbing::pipeline::convert::to_worktree::Options {
                    can_delay: gix::filter::plumbing::driver::apply::Delay::Forbid,
                    ..Default::default()
                },
            )
            .map_err(|e| GitError::Worktree(e.to_string()))?;
        let bytes = converted.as_bytes().ok_or_else(|| {
            GitError::Worktree(
                "il filtro non restituisce contenuto materializzato; nessun checkout pubblicato"
                    .into(),
            )
        })?;
        if cancel.load(Ordering::Relaxed) {
            return Err(GitError::Cancelled);
        }
        Ok(bytes.to_vec())
    }

    /// Una sola costruzione native da index esplicito. Stage conserva
    /// WorktreeThenIdMapping; blob di un nuovo tree usa IdMapping, non la
    /// pipeline generica che riapre/cachea index e preferisce il worktree.
    pub(crate) fn attribute_pipeline(
        &self,
        index: &gix::index::State,
        source: gix::worktree::stack::state::attributes::Source,
    ) -> Result<gix::filter::Pipeline<'_>, GitError> {
        let cache = self
            .repository
            .attributes_only(index, source)
            .map_err(|e| GitError::Worktree(e.to_string()))?;
        gix::filter::Pipeline::new(&self.repository, cache.detach())
            .map_err(|e| GitError::Worktree(e.to_string()))
    }
}
#[cfg(test)]
mod tests;
