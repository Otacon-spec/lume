//! Rifiuto preventivo degli helper comando non ancora posseduti da Task5.
//! Attributi/config nativi, non parser custom, sandbox o freeze anti-TOCTOU.
use crate::{GitError, GitRepository};
use std::sync::atomic::{AtomicBool, Ordering};
impl GitRepository {
    /// Selezione native sugli index espliciti: il solo driver configurato ma
    /// non selezionato non blocca. Built-in CRLF/encoding/ident restano nativi.
    /// only_path limita la selezione al file richiesto dal checkout byte-only;
    /// None controlla l'intero index per la preparazione Task5.
    /// Copre i driver noti in questo snapshot; worktree attrs/info/config sono
    /// mutabili, non prova che un noncooperatore non li cambi successivamente.
    pub(crate) fn refuse_unowned_update_filters(
        &self,
        index: &gix::index::State,
        source: gix::worktree::stack::state::attributes::Source,
        to_worktree: bool,
        only_path: Option<&gix::bstr::BStr>,
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(GitError::Cancelled);
        }
        let options = gix::filter::Pipeline::options(&self.repository)
            .map_err(|e| GitError::Worktree(e.to_string()))?;
        if options.drivers.is_empty() {
            return Ok(());
        }
        let mut cache = self
            .repository
            .attributes_only(index, source)
            .map_err(|e| GitError::Worktree(e.to_string()))?;
        let mut attrs = gix::attrs::search::Outcome::default();
        attrs.initialize_with_selection(&Default::default(), ["filter"]);
        for entry in index.entries() {
            if cancel.load(Ordering::Relaxed) {
                return Err(GitError::Cancelled);
            }
            if only_path.is_some_and(|path| entry.path(index) != path) {
                continue;
            }
            attrs.reset();
            cache
                .at_entry(entry.path(index), None)
                .map_err(|e| GitError::Worktree(e.to_string()))?
                .matching_attributes(&mut attrs);
            let Some(matched) = attrs.iter_selected().next() else {
                continue;
            };
            let Some(name) = matched.assignment.state.as_bstr() else {
                continue;
            };
            if options.drivers.iter().any(|d| {
                let driver_name: &gix::bstr::BStr = d.name.as_ref();
                driver_name == name
                    && (d.process.is_some()
                        || if to_worktree {
                            d.smudge.is_some()
                        } else {
                            d.clean.is_some()
                        })
            }) {
                return Err(GitError::Worktree("filtro comando selezionato: conversione rifiutata prima di avviare helper non posseduti".into()));
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(GitError::Cancelled);
        }
        Ok(())
    }
}
