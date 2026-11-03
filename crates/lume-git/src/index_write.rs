//! Lease cooperativa index: lock prima del read, non soltanto nel persist.
//! I/O e Drop sul worker dell'operazione Git. Non blocca writer che ignorano
//! index.lock né HEAD/branch/config/worktree; non è un transaction manager.
use std::{
    fs,
    io::{BufWriter, Write},
    ops::{Deref, DerefMut},
};

pub(crate) enum MissingIndex {
    Empty,
    Head,
}

/// Carica dal disco senza la cache mtime del Repository, che non distingue
/// due versioni reali con timestamp uguale. Fallback scelto dal caller.
pub(crate) fn fresh_index(
    repo: &gix::Repository,
    missing: MissingIndex,
) -> Result<gix::index::File, String> {
    match fs::symlink_metadata(repo.index_path()) {
        Ok(_) => repo.open_index().map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if matches!(missing, MissingIndex::Head) {
                let head = repo.head().map_err(|error| error.to_string())?;
                if let Some(id) = head.id() {
                    let tree = id
                        .object()
                        .map_err(|error| error.to_string())?
                        .peel_to_commit()
                        .map_err(|error| error.to_string())?
                        .tree_id()
                        .map_err(|error| error.to_string())?
                        .detach();
                    return repo
                        .index_from_tree(&tree)
                        .map_err(|error| error.to_string());
                }
            }
            Ok(gix::index::File::from_state(
                gix::index::State::new(repo.object_hash()),
                repo.index_path(),
            ))
        }
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) struct LockedIndex {
    index: gix::index::File,
    lock: gix::lock::File,
}
impl LockedIndex {
    pub(crate) fn acquire(repo: &gix::Repository, missing: MissingIndex) -> Result<Self, String> {
        let lock = gix::lock::File::acquire_to_update_resource(
            repo.index_path(),
            gix::lock::acquire::Fail::Immediately,
            None,
        )
        .map_err(|error| error.to_string())?;
        let index = fresh_index(repo, missing)?;
        Ok(Self { index, lock })
    }
    /// Encoder/lock nativi gix: stessa pubblicazione dell'index precedente,
    /// ma lock già posseduto dal primo read. Non richiamare File::write(), che
    /// proverebbe ad acquisire il medesimo lock una seconda volta.
    pub(crate) fn commit(mut self) -> Result<(), String> {
        self.index.remove_tree();
        {
            // Stesso buffer 64 KiB dell'encoder File::write() di gix.
            let mut output = BufWriter::with_capacity(64 * 1024, &mut self.lock);
            self.index
                .write_to(&mut output, gix::index::write::Options::default())
                .map_err(|error| error.to_string())?;
            output.flush().map_err(|error| error.to_string())?;
        }
        self.lock
            .with_mut(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        self.lock.commit().map_err(|error| error.to_string())?;
        Ok(())
    }
}
impl Deref for LockedIndex {
    type Target = gix::index::File;
    fn deref(&self) -> &Self::Target {
        &self.index
    }
}
impl DerefMut for LockedIndex {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.index
    }
}
