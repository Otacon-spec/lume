//! «Ripristina»: operazione distruttiva, confermata dal chiamante.
//!
//! File tracciato: blob dell'index attraverso filtri reali e writer protetto
//! del core; non versionato: cancellazione di un singolo file regolare.
//! Prima si controllano tutti i nomi/tipi/link/readonly, poi si elaborano
//! payload uno alla volta. Directory, link e gitlink non vengono indovinati.
//! Non è rollback multi-file né lock contro Git/FS esterni: un errore dopo
//! scritture già concluse le dichiara e lascia verificare il repository.
use crate::{
    GitError, GitRepository,
    index_write::{LockedIndex, MissingIndex},
    worktree::{destination_metadata, git_path},
};
use gix::bstr::ByteSlice;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Discarded {
    /// Riportato al contenuto dell'index.
    Restored(PathBuf),
    /// Non versionato: cancellato dal disco.
    Deleted(PathBuf),
    /// Non c'era niente da buttare via.
    Unchanged(PathBuf),
}

impl GitRepository {
    pub fn discard<I, P>(&self, paths: I) -> Result<Vec<Discarded>, GitError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        // Esclusione cooperativa prima della lettura e di ogni effetto sul
        // worktree. Conserva Head fallback, non riscrive/persiste index.
        let index = LockedIndex::acquire(&self.repository, MissingIndex::Head)
            .map_err(GitError::Discard)?;
        let mut selected = Vec::new();
        let mut seen = BTreeSet::new();
        for path in paths {
            let relative = self.relative_path(path.as_ref())?.to_path_buf();
            if !seen.insert(relative.clone()) {
                return Err(GitError::Worktree(
                    "percorso duplicato nel ripristino".into(),
                ));
            }
            let rela = git_path(&relative)?;
            // entry_by_path sceglie stage 2, oppure None se manca ours:
            // un conflitto non è un blob unico né un file non versionato.
            use gix::index::entry::Stage;
            if [Stage::Base, Stage::Ours, Stage::Theirs]
                .into_iter()
                .any(|stage| {
                    index
                        .entry_by_path_and_stage(rela.as_bstr(), stage)
                        .is_some()
                })
            {
                return Err(GitError::ConflictedIndex);
            }
            if let Some(entry) = index.entry_by_path(rela.as_bstr()) {
                // ITA registra un nome, non un payload staged da ripristinare.
                // Rifiuta tutta la selezione prima del primo effetto sul disco.
                if entry
                    .flags
                    .contains(gix::index::entry::Flags::INTENT_TO_ADD)
                {
                    return Err(GitError::Worktree(
                        "intent-to-add nell'index: nessun contenuto staged da ripristinare".into(),
                    ));
                }
                if !matches!(
                    entry.mode,
                    gix::index::entry::Mode::FILE | gix::index::entry::Mode::FILE_EXECUTABLE
                ) {
                    return Err(GitError::Worktree(
                        "link simbolico/submodule nell'index: ripristino non supportato".into(),
                    ));
                }
            }
            if destination_metadata(&self.work_dir, &relative)?.is_some() {
                // Non tronca; verifica accesso effettivo prima del primo file.
                // Il drop del handle è sullo stesso worker, non sulla UI.
                fs::OpenOptions::new()
                    .write(true)
                    .open(self.work_dir.join(&relative))
                    .map_err(|e| GitError::Discard(e.to_string()))?;
            }
            selected.push(relative);
        }
        if selected.is_empty() {
            return Ok(Vec::new());
        }
        let mut pipeline = self
            .attribute_pipeline(
                &index,
                gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
            )
            .map_err(|e| GitError::Discard(e.to_string()))?;
        let mut done = Vec::new();
        for relative in selected {
            let result = (|| -> Result<Discarded, GitError> {
                let absolute = self.work_dir.join(&relative);
                let rela = git_path(&relative)?;
                let metadata = destination_metadata(&self.work_dir, &relative)?;
                let Some(entry) = index.entry_by_path(rela.as_bstr()) else {
                    return if metadata.is_some() {
                        fs::remove_file(&absolute).map_err(|e| GitError::Discard(e.to_string()))?;
                        Ok(Discarded::Deleted(relative.clone()))
                    } else {
                        Ok(Discarded::Unchanged(relative.clone()))
                    };
                };
                let blob = self
                    .repository
                    .find_object(entry.id)
                    .map_err(|e| GitError::Discard(e.to_string()))?;
                if blob.kind != gix::objs::Kind::Blob {
                    return Err(GitError::Discard(
                        "l'entry file dell'index non punta a un blob".into(),
                    ));
                }
                let converted = pipeline
                    .convert_to_worktree(
                        &blob.data,
                        rela.as_bstr(),
                        gix::filter::plumbing::pipeline::convert::to_worktree::Options {
                            can_delay: gix::filter::plumbing::driver::apply::Delay::Forbid,
                            ..Default::default()
                        },
                    )
                    .map_err(|e| GitError::Discard(e.to_string()))?;
                let bytes = converted.as_bytes().ok_or_else(|| {
                    GitError::Discard(
                        "il filtro configurato non restituisce contenuto materializzato".into(),
                    )
                })?;
                // Ricontrolla dopo i filtri, che possono anche avviare helper.
                destination_metadata(&self.work_dir, &relative)?;
                if let Some(parent) = absolute.parent() {
                    fs::create_dir_all(parent).map_err(|e| GitError::Discard(e.to_string()))?;
                }
                destination_metadata(&self.work_dir, &relative)?;
                let prepared = lume_core::file_write::prepare_atomic(&absolute, bytes)
                    .map_err(|e| GitError::Discard(e.to_string()))?;
                if prepared.destination()
                    != std::path::absolute(&absolute)
                        .map_err(|e| GitError::Discard(e.to_string()))?
                {
                    return Err(GitError::Worktree(
                        "la destinazione è cambiata durante la preparazione".into(),
                    ));
                }
                #[cfg(unix)]
                let prepared = prepared
                    .with_executable(entry.mode == gix::index::entry::Mode::FILE_EXECUTABLE);
                destination_metadata(&self.work_dir, &relative)?;
                prepared
                    .commit()
                    .map_err(|e| GitError::Discard(e.to_string()))?;
                Ok(Discarded::Restored(relative.clone()))
            })();
            match result {
                Ok(outcome) => done.push(outcome),
                Err(error) => {
                    return Err(GitError::Discard(format!(
                        "{error}; ripristino interrotto dopo {} percorsi conclusi; nessun rollback, esito dei percorsi da verificare",
                        done.len()
                    )));
                }
            }
        }
        Ok(done)
    }
}
