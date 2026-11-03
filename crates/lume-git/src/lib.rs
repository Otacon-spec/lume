//! Integrazione Git nativa di Lume basata su gitoxide.
//!
//! L'unica eccezione, dichiarata in
//! CONTRIBUTING.md («Eccezione: git CLI»), sta in `cli.rs`: push, cambio di
//! branch, stash apply/pop e merge con conflitti usano il `git` su PATH.

pub mod diff;
pub mod discard;
pub use diff::{DiffBase, DiffHunk, DiffRow, DiffSign, FileDiff};
pub use discard::Discarded;
mod checkout_content;
// CLI process groups and nonblocking pipe reads use narrowly wrapped libc FFI.
#[allow(unsafe_code)]
mod cli;
mod cli_ops;
mod head_write;
mod index_write;
mod merge;
mod pull;
mod stash_apply;
#[cfg(test)]
#[path = "../tests/stash_index/mod.rs"]
mod stash_index_tests;
mod tree_changes;
mod update_check;
mod update_filters;
mod update_prepare;
mod update_topology;
pub use merge::MergeOutcome;
pub use pull::{PullFailure, PullFailureStage, PullFetchState, PullOutcome};
pub use stash_apply::StashApplyOutcome;
pub use update_prepare::{
    PreparedUpdate, UpdatePublishFailure, UpdatePublishPhase, UpdatePublishReceipt,
    UpdatePublishState,
};
mod upstream;
mod worktree;
pub use cli::{CliOutput, GitCli};
pub use cli_ops::validate_ref_name;
use index_write::{LockedIndex, MissingIndex};
pub use upstream::Upstream;

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// Lock file Git-compatible: create exclusive, publish with rename, cleanup on error.
struct GitLockFile {
    path: PathBuf,
    file: Option<File>,
}

impl GitLockFile {
    fn create(path: PathBuf) -> Result<Self, GitError> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| GitError::Stash(format!("lock {}: {error}", path.display())))?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }

    fn publish(mut self, destination: &Path, bytes: &[u8]) -> Result<(), GitError> {
        let mut file = self
            .file
            .take()
            .ok_or_else(|| GitError::Stash("lock chiuso".into()))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| GitError::Stash(error.to_string()))?;
        drop(file);
        fs::rename(&self.path, destination).map_err(|error| GitError::Stash(error.to_string()))?;
        self.path.clear();
        Ok(())
    }
}

impl Drop for GitLockFile {
    fn drop(&mut self) {
        if !self.path.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

use gix::bstr::ByteSlice;
use gix::diff::index::Change as IndexChange;
use gix::status::{Item, index_worktree::iter::Summary};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeKind {
    Added,
    Removed,
    Modified,
    TypeChanged,
    Renamed,
    Copied,
    IntentToAdd,
    Conflict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileStatus {
    pub path: PathBuf,
    pub index: Option<ChangeKind>,
    pub worktree: Option<ChangeKind>,
}

impl FileStatus {
    /// Il file non è mai entrato nell'index — il gruppo «File non versionati»
    /// del pannello Commit.
    ///
    /// Sul lato worktree `ChangeKind::Added` **significa** non versionato:
    /// gitoxide lo produce solo da un `DirectoryContents` con stato
    /// `Untracked` (`gix-0.85.0/src/status/index_worktree.rs:502`). Sul lato
    /// index, invece, `Added` è un file nuovo **già in stage**, che è
    /// tracciato e va nell'altro gruppo. Da qui le due condizioni.
    pub fn is_untracked(&self) -> bool {
        self.index.is_none() && self.worktree == Some(ChangeKind::Added)
    }

    /// Il file ha almeno un hunk in stage (index diverso da HEAD).
    ///
    /// Con lo staging per hunk un file può essere *parzialmente* in stage:
    /// `index.is_some()` e `worktree.is_some()` nello stesso tempo. La
    /// casella nel pannello Commit mostra lo stato `Partial` in quel caso.
    pub fn is_staged(&self) -> bool {
        self.index.is_some_and(|kind| kind != ChangeKind::Conflict)
    }

    /// Il file è *parzialmente* in stage: alcune modifiche nell'index, altre
    /// ancora solo nel worktree. Si verifica dopo `stage_hunk` su un singolo
    /// hunk di un file con più hunk.
    pub fn is_partially_staged(&self) -> bool {
        self.is_staged() && self.worktree.is_some()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitSummary {
    pub id: String,
    pub parent_ids: Vec<String>,
    pub subject: String,
    pub body: Option<String>,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    /// Offset del fuso dell'autore in secondi (per mostrare l'ora locale del commit).
    pub author_offset: i32,
    pub committer_time: i64,
}

/// Branch locale: nome breve («main») e commit di punta.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchRef {
    pub name: String,
    pub tip: String,
}

/// Branch remoto: remote («origin»), nome breve e commit di punta.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteBranchRef {
    pub remote: String,
    pub name: String,
    pub tip: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Branches {
    /// Branch corrente (None se HEAD è detached).
    pub head: Option<String>,
    pub local: Vec<BranchRef>,
    pub remote: Vec<RemoteBranchRef>,
}

/// File toccato da un commit (diff contro il primo parent).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub kind: ChangeKind,
}

/// Una riga annotata con il commit che l'ha introdotta (git blame).
#[derive(Clone, Debug)]
pub struct BlameLine {
    /// Numero di riga 1-based nel file blamed.
    pub line_number: u32,
    /// Hash abbreviato del commit (7 caratteri).
    pub commit_id: String,
    pub author_name: String,
    /// Timestamp UNIX dell'autore.
    pub author_time: i64,
}

/// Una voce dell'area di stash: stash@{index}.
#[derive(Clone, Debug)]
pub struct StashEntry {
    /// Indice (0 = più recente).
    pub index: usize,
    /// Messaggio dello stash (es. «WIP on main: abc1234 Titolo»).
    pub message: String,
    /// OID del commit dello stash.
    pub commit_id: String,
}

#[derive(Debug, Error)]
pub enum GitError {
    #[error("impossibile aprire il repository Git: {0}")]
    Discover(String),
    #[error("repository Git bare non supportato")]
    BareRepository,
    #[error("path fuori dal worktree Git: {0}")]
    InvalidPath(PathBuf),
    #[error("impossibile aggiornare l'index Git: {0}")]
    Stage(String),
    #[error("impossibile rimuovere file dall'area di staging: {0}")]
    Unstage(String),
    #[error("impossibile leggere lo stato Git: {0}")]
    Status(String),
    #[error("il messaggio del commit è vuoto")]
    EmptyMessage,
    #[error("nessuna modifica nell'area di staging")]
    NothingStaged,
    #[error("l'index contiene conflitti non risolti")]
    ConflictedIndex,
    #[error("impossibile creare il commit Git: {0}")]
    Commit(String),
    #[error("impossibile leggere il log Git: {0}")]
    Log(String),
    #[error("impossibile leggere i branch Git: {0}")]
    Branches(String),
    #[error("riferimento Git sconosciuto: {0}")]
    UnknownReference(String),
    #[error("impossibile calcolare le modifiche del commit: {0}")]
    Diff(String),
    #[error("impossibile ripristinare il file: {0}")]
    Discard(String),
    #[error("operazione worktree rifiutata: {0}")]
    Worktree(String),
    #[error("integrazione rifiutata per modifiche locali: {0}")]
    PullDirty(String),
    #[error(
        "cleanup temporanei Git incompleto: {failed} errori su {attempted} tentativi; residui da verificare"
    )]
    UpdateCleanup { attempted: usize, failed: usize },
    #[error(
        "{source}; cleanup dei temporanei già preparati incompleto: {failed} errori su {attempted} tentativi; residui da verificare"
    )]
    UpdatePrepareCleanup {
        #[source]
        source: Box<GitError>,
        attempted: usize,
        failed: usize,
    },
    #[error("impossibile calcolare il blame: {0}")]
    Blame(String),
    #[error("operazione stash fallita: {0}")]
    Stash(String),
    #[error("fetch fallita: {0}")]
    Fetch(String),
    #[error("operazione annullata")]
    Cancelled,
    #[error("impossibile eseguire git: {0}")]
    Cli(String),
    #[error("push fallito: {0}")]
    Push(String),
    #[error("cambio di branch fallito: {0}")]
    Switch(String),
    #[error("il branch «{0}» non ha un upstream remoto supportato (richiesto refspec standard)")]
    NoUpstream(String),
    #[error("nome non valido per Git: {0}")]
    InvalidBranchName(String),
    #[error("c'è un merge in corso: concludilo o annullalo prima del commit ordinario")]
    MergeInProgress,
    #[error("non c'è un merge in corso da concludere o annullare")]
    MergeNotInProgress,
    #[error("operazione merge fallita: {0}")]
    Merge(String),
}

/// gix 0.88 non espone un tipo dedicato per tutte le interruzioni: il lettore
/// usa io::Error::other("Interrupted"), la sideband "interrupted by user",
/// l'indicizzatore un Message Retryable e la negoziazione un Message con
/// ClassificationMarker Retryable. Si cercano
/// quei segnali nelle cause, non nel testo completo del contesto. Un flag
/// tardivo da solo non deve mai nascondere un errore di rete o di disco.
fn fetch_receive_error(error: gix::Error, cancel_requested: bool) -> GitError {
    let interrupted = if cancel_requested {
        error.classify().any(|classification| {
            let cause = classification.error();
            match classification.class() {
                gix::error::Class::Io(std::io::ErrorKind::Other) => {
                    cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
                        matches!(
                            io.to_string().as_str(),
                            "Interrupted" | "interrupted by user"
                        )
                    })
                }
                gix::error::Class::Retryable => {
                    // Il marker della negoziazione classifica la propria source,
                    // non qualsiasi altro messaggio presente nell'albero errori.
                    let message = cause.downcast_ref::<gix::error::Message>().or_else(|| {
                        cause.downcast_ref::<gix::error::ClassificationMarker>()?;
                        cause.source()?.downcast_ref::<gix::error::Message>()
                    });
                    message.is_some_and(|message| {
                        matches!(
                        message.message.as_ref(),
                        "Interrupted" | "Operation interrupted"
                    ) || message.message.starts_with(
                        "We were unable to figure out what objects the server should send after ",
                    )
                    })
                }
                _ => false,
            }
        })
    } else {
        false
    };
    if interrupted {
        GitError::Cancelled
    } else {
        GitError::Fetch(error.to_string())
    }
}

pub struct GitRepository {
    repository: gix::Repository,
    work_dir: PathBuf,
}

impl GitRepository {
    pub fn discover(path: impl AsRef<Path>) -> Result<Self, GitError> {
        let repository =
            gix::discover(path.as_ref()).map_err(|error| GitError::Discover(error.to_string()))?;
        let work_dir = repository
            .workdir()
            .ok_or(GitError::BareRepository)?
            .to_path_buf();
        Ok(Self {
            repository,
            work_dir,
        })
    }

    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// Presenza di MERGE_HEAD nella vera directory Git, anche nei linked
    /// worktree. Marker malformato/link rotto o errore di accesso: rifiuto
    /// conservativo, non «nessun merge» ottenuto da is_file() == false.
    /// È un controllo prima dell'operazione, non un lock contro altri Git.
    pub fn merge_in_progress(&self) -> bool {
        match std::fs::symlink_metadata(self.repository.git_dir().join("MERGE_HEAD")) {
            Ok(_) => true,
            Err(error) => error.kind() != std::io::ErrorKind::NotFound,
        }
    }

    pub fn branch_name(&self) -> Result<Option<String>, GitError> {
        self.repository
            .head_name()
            .map(|name| name.map(|name| name.shorten().to_string()))
            .map_err(|error| GitError::Log(error.to_string()))
    }

    pub fn status(&self) -> Result<Vec<FileStatus>, GitError> {
        // Il default native usa index_or_empty (cache mtime): conserva Empty
        // se assente, ma apri il file attuale anche con timestamp uguale.
        let index = index_write::fresh_index(&self.repository, MissingIndex::Empty)
            .map_err(GitError::Status)?;
        self.status_with_index(index)
    }

    // Mapping native invariato: il caller sceglie lo snapshot, senza read
    // cached aggiuntivo né write_changes sull'index persistito.
    fn status_with_index(&self, index: gix::index::File) -> Result<Vec<FileStatus>, GitError> {
        let platform = self
            .repository
            .status(gix::progress::Discard)
            .map_err(|error| GitError::Status(error.to_string()))?
            .index(gix::worktree::IndexPersistedOrInMemory::InMemory(index))
            // Un file per riga, mai una cartella. Il default di gitoxide è
            // `Collapsed`, che emette `nuova/` invece dei file dentro — come
            // fa `git status --short` con `?? artifacts/`. Ma ogni riga del
            // pannello Commit dev'essere azionabile: la casella la mette in
            // stage, il clic ne apre il diff, «Ripristina» la cancella. Su una
            // cartella nessuna delle tre funziona. Questo sovrascrive anche
            // `status.showUntrackedFiles`: lo stato non deve dipendere dalla
            // configurazione di chi apre il repository.
            .untracked_files(gix::status::UntrackedFiles::Files);
        let entries = platform
            .into_iter(Vec::<gix::bstr::BString>::new())
            .map_err(|error| GitError::Status(error.to_string()))?;

        let mut status = BTreeMap::<PathBuf, FileStatus>::new();
        for entry in entries {
            let entry = entry.map_err(|error| GitError::Status(error.to_string()))?;
            let path = gix::path::try_from_bstr(entry.location())
                .map_err(|error| GitError::Status(error.to_string()))?
                .into_owned();
            let file = status.entry(path.clone()).or_insert(FileStatus {
                path,
                index: None,
                worktree: None,
            });
            match entry {
                Item::IndexWorktree(entry) => {
                    file.worktree = entry.summary().map(change_kind);
                }
                Item::TreeIndex(change) => {
                    file.index = Some(index_change_kind(&change));
                }
            }
        }
        Ok(status.into_values().collect())
    }

    /// Log da HEAD (vuoto se il repository non ha commit).
    pub fn log(&self, limit: usize) -> Result<Vec<CommitSummary>, GitError> {
        let head = self
            .repository
            .head()
            .map_err(|error| GitError::Log(error.to_string()))?;
        let Some(head_id) = head.id() else {
            return Ok(Vec::new());
        };
        self.log_from_id(head_id, limit)
    }

    /// Log a partire da un riferimento («HEAD», «main», «origin/main», tag).
    pub fn log_from(&self, reference: &str, limit: usize) -> Result<Vec<CommitSummary>, GitError> {
        if reference == "HEAD" {
            return self.log(limit);
        }
        let id = self
            .repository
            .find_reference(reference)
            .map_err(|_| GitError::UnknownReference(reference.to_string()))?
            .into_fully_peeled_id()
            .map_err(|error| GitError::Log(error.to_string()))?;
        self.log_from_id(id, limit)
    }

    fn log_from_id(&self, from: gix::Id<'_>, limit: usize) -> Result<Vec<CommitSummary>, GitError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let walk = from
            .ancestors()
            .all()
            .map_err(|error| GitError::Log(error.to_string()))?;

        let mut commits = Vec::new();
        for info in walk.take(limit) {
            let info = info.map_err(|error| GitError::Log(error.to_string()))?;
            let commit = info
                .object()
                .map_err(|error| GitError::Log(error.to_string()))?;
            let decoded = commit
                .decode()
                .map_err(|error| GitError::Log(error.to_string()))?;
            let author = decoded
                .author()
                .map_err(|error| GitError::Log(error.to_string()))?;
            let author_time = author
                .time()
                .map_err(|error| GitError::Log(error.to_string()))?;
            let committer_time = decoded
                .committer()
                .map_err(|error| GitError::Log(error.to_string()))?
                .time()
                .map_err(|error| GitError::Log(error.to_string()))?;
            let message = commit
                .message()
                .map_err(|error| GitError::Log(error.to_string()))?;
            commits.push(CommitSummary {
                id: info.id.to_string(),
                parent_ids: decoded.parents().map(|id| id.to_string()).collect(),
                subject: message.title.to_string(),
                body: message.body.map(|body| body.to_string()),
                author_name: author.name.to_string(),
                author_email: author.email.to_string(),
                author_time: author_time.seconds,
                author_offset: author_time.offset,
                committer_time: committer_time.seconds,
            });
        }
        Ok(commits)
    }

    /// Branch locali e remoti con il commit di punta (per l'albero del Log Git).
    pub fn branches(&self) -> Result<Branches, GitError> {
        let head = self.branch_name()?;
        let platform = self
            .repository
            .references()
            .map_err(|error| GitError::Branches(error.to_string()))?;

        let mut local = Vec::new();
        for reference in platform
            .local_branches()
            .map_err(|error| GitError::Branches(error.to_string()))?
        {
            let reference = reference.map_err(|error| GitError::Branches(error.to_string()))?;
            let name = reference.name().shorten().to_string();
            let tip = reference
                .into_fully_peeled_id()
                .map_err(|error| GitError::Branches(error.to_string()))?
                .to_string();
            local.push(BranchRef { name, tip });
        }

        let mut remote = Vec::new();
        for reference in platform
            .remote_branches()
            .map_err(|error| GitError::Branches(error.to_string()))?
        {
            let reference = reference.map_err(|error| GitError::Branches(error.to_string()))?;
            let full = reference.name().shorten().to_string();
            let Some((remote_name, name)) = full.split_once('/') else {
                continue;
            };
            if name == "HEAD" {
                continue; // «origin/HEAD» è solo un alias simbolico
            }
            let (remote_name, name) = (remote_name.to_string(), name.to_string());
            let tip = reference
                .into_fully_peeled_id()
                .map_err(|error| GitError::Branches(error.to_string()))?
                .to_string();
            remote.push(RemoteBranchRef {
                remote: remote_name,
                name,
                tip,
            });
        }
        local.sort_by(|a, b| a.name.cmp(&b.name));
        remote.sort_by(|a, b| (&a.remote, &a.name).cmp(&(&b.remote, &b.name)));
        Ok(Branches {
            head,
            local,
            remote,
        })
    }

    /// File modificati da un commit rispetto al primo parent (albero vuoto
    /// per il commit radice).
    pub fn commit_files(&self, id: &str) -> Result<Vec<ChangedFile>, GitError> {
        let oid = gix::ObjectId::from_hex(id.as_bytes())
            .map_err(|error| GitError::Diff(error.to_string()))?;
        let commit = self
            .repository
            .find_commit(oid)
            .map_err(|error| GitError::Diff(error.to_string()))?;
        let tree = commit
            .tree()
            .map_err(|error| GitError::Diff(error.to_string()))?;
        let parent_tree = match commit.parent_ids().next() {
            Some(parent) => parent
                .object()
                .map_err(|error| GitError::Diff(error.to_string()))?
                .peel_to_commit()
                .map_err(|error| GitError::Diff(error.to_string()))?
                .tree()
                .map_err(|error| GitError::Diff(error.to_string()))?,
            None => self.repository.empty_tree(),
        };

        // Il delta conserva solo leaf: le directory non sono file apribili.
        // Storia mantiene rename/copy secondo config; il checkout futuro
        // chiamerà tree_changes con rewrites=false e la propria cancellazione.
        Ok(self
            .tree_changes(parent_tree.id, tree.id, true, None)?
            .into_iter()
            .map(tree_changes::TreeChange::into_file)
            .collect())
    }

    pub fn commit(&self, message: &str) -> Result<String, GitError> {
        self.commit_with(message, false)
    }

    /// Messaggio e corpo del commit di HEAD, per precompilare la casella
    /// quando si attiva Amend. `None` se HEAD non esiste ancora.
    pub fn head_message(&self) -> Result<Option<String>, GitError> {
        let head = self
            .repository
            .head()
            .map_err(|error| GitError::Log(error.to_string()))?;
        let Some(id) = head.id() else {
            return Ok(None);
        };
        let commit = id
            .object()
            .map_err(|error| GitError::Log(error.to_string()))?
            .peel_to_commit()
            .map_err(|error| GitError::Log(error.to_string()))?;
        let message = commit
            .message_raw()
            .map_err(|error| GitError::Log(error.to_string()))?;
        Ok(Some(message.to_string()))
    }

    /// Con `amend` il commit **sostituisce** HEAD invece di aggiungersi:
    /// prende i parent di HEAD, non HEAD stesso, e conserva l'autore
    /// originale — riscrivere il messaggio non deve cambiare chi ha scritto
    /// il codice.
    ///
    /// Con `amend` non vale `NothingStaged`: correggere solo il messaggio di
    /// un commit e' un uso legittimo, anzi il piu' comune.
    pub fn commit_with(&self, message: &str, amend: bool) -> Result<String, GitError> {
        if self.merge_in_progress() {
            return Err(GitError::MergeInProgress);
        }
        if message.trim().is_empty() {
            return Err(GitError::EmptyMessage);
        }
        let head_snapshot = head_write::HeadSnapshot::capture(&self.repository)?;
        // Una sola versione fresca sotto lease fino a ref/reflog e cleanup.
        // Non scrivere l'index al termine: commit ne consuma soltanto lo stage.
        // Head fallback conserva la politica index del commit preesistente.
        let index =
            LockedIndex::acquire(&self.repository, MissingIndex::Head).map_err(GitError::Commit)?;
        if index
            .entries()
            .iter()
            .any(|entry| entry.stage() != gix::index::entry::Stage::Unconflicted)
        {
            return Err(GitError::ConflictedIndex);
        }
        let mut tree = self
            .repository
            .empty_tree()
            .edit()
            .map_err(|error| GitError::Commit(error.to_string()))?;
        for entry in index.entries() {
            // Intent-to-add registra il nome, non una versione da committare.
            // Conservare flags/index e byte del worktree, senza vuoti inventati.
            if entry
                .flags
                .contains(gix::index::entry::Flags::INTENT_TO_ADD)
            {
                continue;
            }
            let mode = entry
                .mode
                .to_tree_entry_mode()
                .ok_or_else(|| GitError::Commit("mode index non valida".into()))?;
            tree.upsert(entry.path(&index), mode.kind(), entry.id)
                .map_err(|error| GitError::Commit(error.to_string()))?;
        }
        let tree_id = tree
            .write()
            .map_err(|error| GitError::Commit(error.to_string()))?
            .detach();

        let head_id = head_snapshot.id;
        if !amend {
            let previous_tree = match head_id {
                Some(id) => self
                    .repository
                    .find_commit(id)
                    .map_err(|error| GitError::Commit(error.to_string()))?
                    .tree_id()
                    .map_err(|error| GitError::Commit(error.to_string()))?
                    .detach(),
                None => self.repository.empty_tree().id,
            };
            // Valutare staged dallo stesso albero/index leased, non da una
            // status separata o un altro snapshot della cache. No empty commit.
            if tree_id == previous_tree {
                return Err(GitError::NothingStaged);
            }
            let parents: Vec<_> = head_id.into_iter().collect();
            let reflog =
                gix::reference::log::message("commit", message.as_bytes().as_bstr(), parents.len());
            let commit_id = self
                .repository
                .new_commit(message, tree_id, parents)
                .map_err(|error| GitError::Commit(error.to_string()))?
                .id;
            head_snapshot.advance(&self.repository, commit_id, reflog)?;
            return Ok(commit_id.to_string());
        }

        // Amend senza HEAD non ha nulla da riscrivere: e' un commit normale.
        let Some(head_id) = head_id else {
            let commit_id = self
                .repository
                .new_commit(message, tree_id, Vec::<gix::ObjectId>::new())
                .map_err(|error| GitError::Commit(error.to_string()))?
                .id;
            head_snapshot.advance(
                &self.repository,
                commit_id,
                gix::reference::log::message("commit", message.as_bytes().as_bstr(), 0),
            )?;
            return Ok(commit_id.to_string());
        };
        let previous = self
            .repository
            .find_commit(head_id)
            .map_err(|error| GitError::Commit(error.to_string()))?;
        let parents: Vec<_> = previous.parent_ids().map(|id| id.detach()).collect();
        let author = previous
            .author()
            .map_err(|error| GitError::Commit(error.to_string()))?;
        let committer = self
            .repository
            .committer()
            .transpose()
            .map_err(|error| GitError::Commit(error.to_string()))?
            .unwrap_or(author);
        // Non si puo' usare `commit_as`: quello ricava il valore atteso del
        // ref dal **primo parent**, che in un amend e' il parent di HEAD e
        // non HEAD — la transazione fallirebbe sempre. Qui l'oggetto si
        // scrive a mano e il ref si sposta da HEAD al commit nuovo.
        let object = gix::objs::Commit {
            message: message.into(),
            tree: tree_id,
            author: author.into(),
            committer: committer.into(),
            encoding: None,
            parents: parents.into_iter().collect(),
            extra_headers: Vec::new(),
        };
        let commit_id = self
            .repository
            .write_object(&object)
            .map_err(|error| GitError::Commit(error.to_string()))?
            .detach();
        head_snapshot.advance(
            &self.repository,
            commit_id,
            format!("commit (amend): {}", message.lines().next().unwrap_or("")),
        )?;
        Ok(commit_id.to_string())
    }

    pub fn stage<I, P>(&self, paths: I) -> Result<(), GitError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut index =
            LockedIndex::acquire(&self.repository, MissingIndex::Empty).map_err(GitError::Stage)?;
        let mut pipeline = self
            .attribute_pipeline(
                &index,
                gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
            )
            .map_err(|error| GitError::Stage(error.to_string()))?;

        for path in paths {
            let relative = self.relative_path(path.as_ref())?;
            let git_path = gix::path::into_bstr(relative).into_owned();
            match pipeline
                .worktree_file_to_object(git_path.as_ref(), &index)
                .map_err(|error| GitError::Stage(error.to_string()))?
            {
                Some((id, kind, _metadata)) => {
                    let metadata = gix::index::fs::Metadata::from_path_no_follow(
                        &self.work_dir.join(relative),
                    )
                    .map_err(|error| GitError::Stage(error.to_string()))?;
                    let stat = gix::index::entry::Stat::from_fs(&metadata)
                        .map_err(|error| GitError::Stage(error.to_string()))?;
                    let mode = kind.into();
                    // La versione risolta è stage 0. Conservare stage 1–3
                    // dello stesso path lascerebbe il conflitto nell'index;
                    // non rimuovere le versioni degli altri file in conflitto.
                    index.remove_entries(|_, candidate, entry| {
                        candidate == git_path.as_bstr()
                            && entry.stage() != gix::index::entry::Stage::Unconflicted
                    });
                    if let Some(entry) = index.entry_mut_by_path_and_stage(
                        git_path.as_ref(),
                        gix::index::entry::Stage::Unconflicted,
                    ) {
                        entry.stat = stat;
                        entry.id = id;
                        entry.mode = mode;
                        entry.flags = gix::index::entry::Flags::empty();
                    } else {
                        index.dangerously_push_entry(
                            stat,
                            id,
                            gix::index::entry::Flags::empty(),
                            mode,
                            git_path.as_ref(),
                        );
                    }
                }
                None => {
                    index.remove_entries(|_, candidate, _| candidate == git_path.as_bstr());
                }
            }
        }

        index.sort_entries();
        index.remove_tree();
        index.commit().map_err(GitError::Stage)
    }

    pub fn unstage<I, P>(&self, paths: I) -> Result<(), GitError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut index = LockedIndex::acquire(&self.repository, MissingIndex::Head)
            .map_err(GitError::Unstage)?;
        self.unstage_index_paths(&mut index, paths)?;
        index.sort_entries();
        index.commit().map_err(GitError::Unstage)
    }

    fn unstage_index_paths<I, P>(
        &self,
        index: &mut gix::index::File,
        paths: I,
    ) -> Result<(), GitError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let head = self
            .repository
            .head()
            .map_err(|error| GitError::Unstage(error.to_string()))?;
        let head_index = match head.id() {
            Some(id) => {
                let tree_id = id
                    .object()
                    .map_err(|error| GitError::Unstage(error.to_string()))?
                    .peel_to_commit()
                    .map_err(|error| GitError::Unstage(error.to_string()))?
                    .tree_id()
                    .map_err(|error| GitError::Unstage(error.to_string()))?
                    .detach();
                Some(
                    self.repository
                        .index_from_tree(&tree_id)
                        .map_err(|error| GitError::Unstage(error.to_string()))?,
                )
            }
            None => None,
        };

        for path in paths {
            let relative = self.relative_path(path.as_ref())?;
            let git_path = gix::path::into_bstr(relative).into_owned();
            index.remove_entries(|_, candidate, _| candidate == git_path.as_bstr());
            if let Some(entry) = head_index
                .as_ref()
                .and_then(|head| head.entry_by_path(git_path.as_bstr()))
                .cloned()
            {
                index.dangerously_push_entry(
                    entry.stat,
                    entry.id,
                    entry.flags,
                    entry.mode,
                    git_path.as_bstr(),
                );
            }
        }

        Ok(())
    }

    // ──────────────────────────────────────────────────────────────────────
    // Blame
    // ──────────────────────────────────────────────────────────────────────

    /// Calcola il blame di un file rispetto a HEAD.
    ///
    /// **Il blame è di HEAD, non del worktree**: le righe non committate non
    /// hanno un commit da attribuire, quindi con un file sporco la numerazione
    /// qui e quella dell'editor divergono. Chi mostra il risultato deve dirlo,
    /// e per saperlo chiama [`GitRepository::file_is_dirty`] — non esiste
    /// nessun `BlameResult` con un campo `dirty`, che è quel che questa
    /// docstring prometteva senza che il tipo esistesse.
    pub fn blame(&self, path: &Path) -> Result<Vec<BlameLine>, GitError> {
        let head = self
            .repository
            .head()
            .map_err(|e| GitError::Blame(e.to_string()))?;
        let head_id = head
            .id()
            .ok_or_else(|| GitError::Blame("nessun commit HEAD".to_owned()))?
            .detach();

        let relative = self.relative_path(path)?;
        let git_path = gix::path::into_bstr(relative).into_owned();

        let outcome = self
            .repository
            .blame_file(
                gix::bstr::BStr::new(git_path.as_bstr()),
                head_id,
                gix::repository::blame_file::Options::default(),
            )
            .map_err(|e| GitError::Blame(e.to_string()))?;

        // Cache commit info per non fare look-up multipli.
        let mut cache: std::collections::HashMap<gix::ObjectId, (String, String, i64)> =
            std::collections::HashMap::new();

        let mut lines: Vec<BlameLine> = Vec::new();
        for entry in &outcome.entries {
            let info = if let Some(info) = cache.get(&entry.commit_id) {
                info.clone()
            } else {
                let commit = self
                    .repository
                    .find_commit(entry.commit_id)
                    .map_err(|e| GitError::Blame(e.to_string()))?;
                let decoded = commit
                    .decode()
                    .map_err(|e| GitError::Blame(e.to_string()))?;
                let author = decoded
                    .author()
                    .map_err(|e| GitError::Blame(e.to_string()))?;
                let time = author.time().map_err(|e| GitError::Blame(e.to_string()))?;
                let info = (
                    entry.commit_id.to_string(),
                    author.name.to_string(),
                    time.seconds,
                );
                cache.insert(entry.commit_id, info.clone());
                info
            };

            let range = entry.range_in_blamed_file();
            let start = range.start as u32;
            let end = range.end as u32;
            for line_idx in start..end {
                lines.push(BlameLine {
                    line_number: line_idx + 1,
                    commit_id: info.0.get(..7).unwrap_or(&info.0).to_owned(),
                    author_name: info.1.clone(),
                    author_time: info.2,
                });
            }
        }
        lines.sort_by_key(|l| l.line_number);
        Ok(lines)
    }

    /// `true` se il file ha modifiche non committate nel worktree
    /// (usato per mostrare l'avviso «blame riflette HEAD» nel pannello).
    pub fn file_is_dirty(&self, path: &Path) -> bool {
        self.status()
            .unwrap_or_default()
            .iter()
            .any(|f| f.path == self.relative_path(path).unwrap_or(path) && f.worktree.is_some())
    }

    // ──────────────────────────────────────────────────────────────────────
    // Stash
    // ──────────────────────────────────────────────────────────────────────

    /// Elenca le voci dello stash leggendo il reflog di `refs/stash`.
    /// Restituisce un vettore vuoto se refs/stash non esiste.
    pub fn stash_list(&self) -> Result<Vec<StashEntry>, GitError> {
        let stash_ref = match self
            .repository
            .try_find_reference("refs/stash")
            .map_err(|e| GitError::Stash(e.to_string()))?
        {
            Some(r) => r,
            None => return Ok(Vec::new()), // nessuno stash
        };

        let mut log_platform = stash_ref.log_iter();
        let entries_opt = log_platform
            .all()
            .map_err(|e| GitError::Stash(e.to_string()))?;

        let mut result = Vec::new();
        if let Some(entries) = entries_opt {
            // Il reflog è in ordine cronologico (più vecchio prima); invertiamo.
            let collected: Vec<_> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| (entry.message.to_string(), entry.new_oid().to_string()))
                .collect();
            for (i, (message, commit_id)) in collected.iter().rev().enumerate() {
                result.push(StashEntry {
                    index: i,
                    message: message
                        .trim_start_matches("On ")
                        .trim_end_matches('\n')
                        .to_owned(),
                    commit_id: commit_id.get(..7).unwrap_or(commit_id).to_owned(),
                });
            }
        }
        Ok(result)
    }

    /// Crea una fotografia stash dei soli file tracciati, staged + worktree.
    ///
    /// Albero worktree e secondo parent index dalla stessa versione leased,
    /// rifiuto di conflitti/ITA prima delle scritture ODB. Aggiorna refs/stash
    /// e reflog; NON ripristina ancora il worktree/index (Task7 incompleto).
    /// Hint/stat non sostituiscono i byte; helper selezionati non posseduti
    /// rifiutati, non sandbox o supporto completo/cancellabile dei filtri.
    pub fn stash_push(&self, message: Option<&str>) -> Result<(), GitError> {
        let head = self
            .repository
            .head()
            .map_err(|e| GitError::Stash(e.to_string()))?;
        let head_id = head
            .id()
            .ok_or_else(|| GitError::Stash("nessun commit HEAD".to_owned()))?
            .detach();

        // Lease cooperativa prima del read/filter/ODB fino all'esito refs/stash.
        // La cache mtime non è versione del file, né questa lease congela
        // HEAD/worktree/config/ref o garantisce transazione/rollback.
        let leased_index =
            LockedIndex::acquire(&self.repository, MissingIndex::Head).map_err(GitError::Stash)?;
        // Non omettere stage in conflitto o trattare ITA come blob approvato.
        // Preflight globale prima di pipeline/conversione/scritture ODB.
        if leased_index.entries().iter().any(|entry| {
            entry.stage() != gix::index::entry::Stage::Unconflicted
                || entry
                    .flags
                    .contains(gix::index::entry::Flags::INTENT_TO_ADD)
        }) {
            return Err(GitError::Stash(
                "snapshot stash rifiutato: conflitti o intent-to-add nell'index da conservare"
                    .into(),
            ));
        }
        // Tutti e solo i nomi tracciati della stessa versione leased. Non
        // inferire contenuto dagli hint/stat dello status. Vec è metadata,
        // non streaming o dimostrazione di un limite RAM/FD/tempo.
        let paths = leased_index
            .entries()
            .iter()
            .map(|entry| {
                let path = entry.path(&leased_index);
                let native = gix::path::try_from_bstr(path)
                    .map_err(|error| GitError::Stash(error.to_string()))?;
                self.relative_path(native.as_ref())?;
                if entry.mode.to_tree_entry_mode().is_none() {
                    return Err(GitError::Stash("mode invalida nell'index".into()));
                }
                Ok(path.to_owned())
            })
            .collect::<Result<Vec<_>, GitError>>()?;
        // Il nuovo read di tutti i tracked non deve avviare helper noti
        // non posseduti, neppure per un file che status dichiarerebbe pulito.
        // Snapshot attrs/info/config mutabili: non sandbox o freeze.
        self.refuse_unowned_update_filters(
            &leased_index,
            gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
            false,
            None,
            &std::sync::atomic::AtomicBool::new(false),
        )?;
        // Conserva anzitutto l'albero dell'index reale: Git lo usa come
        // secondo parent dello stash per distinguere staged e unstaged.
        // Poi crea una fotografia privata e vi applica le differenze del
        // worktree senza scrivere `.git/index`: «Salva stash» non pulisce il
        // worktree, ma l'oggetto prodotto resta applicabile da Git.
        let mut pipeline = self.attribute_pipeline(
            &leased_index,
            gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
        )?;
        let mut index: gix::index::File = leased_index.clone();
        let mut index_tree_editor = self
            .repository
            .empty_tree()
            .edit()
            .map_err(|e| GitError::Stash(e.to_string()))?;
        for entry in index.entries() {
            if entry.stage() != gix::index::entry::Stage::Unconflicted {
                continue;
            }
            let mode = entry
                .mode
                .to_tree_entry_mode()
                .ok_or_else(|| GitError::Stash("mode invalida nell'index".to_owned()))?;
            index_tree_editor
                .upsert(entry.path(&index), mode.kind(), entry.id)
                .map_err(|e| GitError::Stash(e.to_string()))?;
        }
        let index_tree_id = index_tree_editor
            .write()
            .map_err(|e| GitError::Stash(e.to_string()))?
            .detach();
        // Leggi anche assume-valid/skip-worktree/stat uguali: un hint non
        // autorizza a salvare byte precedenti. Niente walk dei non versionati,
        // niente secondo read dell'index. Letture/ODB iniziati non cancellabili
        // o transazione con gli altri file del worktree.
        for git_path in paths {
            let relative = gix::path::try_from_bstr(&git_path)
                .map_err(|error| GitError::Stash(error.to_string()))?;
            match pipeline
                .worktree_file_to_object(git_path.as_ref(), &index)
                .map_err(|e| GitError::Stash(e.to_string()))?
            {
                Some((id, kind, _metadata)) => {
                    let metadata = gix::index::fs::Metadata::from_path_no_follow(
                        &self.work_dir.join(relative),
                    )
                    .map_err(|e| GitError::Stash(e.to_string()))?;
                    let stat = gix::index::entry::Stat::from_fs(&metadata)
                        .map_err(|e| GitError::Stash(e.to_string()))?;
                    let mode = kind.into();
                    if let Some(entry) = index.entry_mut_by_path_and_stage(
                        git_path.as_ref(),
                        gix::index::entry::Stage::Unconflicted,
                    ) {
                        entry.stat = stat;
                        entry.id = id;
                        entry.mode = mode;
                        entry.flags = gix::index::entry::Flags::empty();
                    } else {
                        index.dangerously_push_entry(
                            stat,
                            id,
                            gix::index::entry::Flags::empty(),
                            mode,
                            git_path.as_ref(),
                        );
                    }
                }
                None => {
                    index.remove_entries(|_, candidate, _| candidate == git_path.as_bstr());
                }
            }
        }
        index.sort_entries();
        index.remove_tree();
        let mut tree_editor = self
            .repository
            .empty_tree()
            .edit()
            .map_err(|e| GitError::Stash(e.to_string()))?;
        for entry in index.entries() {
            if entry.stage() != gix::index::entry::Stage::Unconflicted {
                continue;
            }
            let mode = entry
                .mode
                .to_tree_entry_mode()
                .ok_or_else(|| GitError::Stash("mode invalida nell'index".to_owned()))?;
            tree_editor
                .upsert(entry.path(&index), mode.kind(), entry.id)
                .map_err(|e| GitError::Stash(e.to_string()))?;
        }
        let tree_id = tree_editor
            .write()
            .map_err(|e| GitError::Stash(e.to_string()))?
            .detach();

        // Messaggio dello stash.
        let branch = self.branch_name()?.unwrap_or_else(|| "HEAD".to_owned());
        let head_commit = self
            .repository
            .find_commit(head_id)
            .map_err(|e| GitError::Stash(e.to_string()))?;
        let head_subject = head_commit
            .message()
            .map_err(|e| GitError::Stash(e.to_string()))?
            .title
            .to_string();
        let short_id = head_id.to_string();
        let short_id = short_id.get(..7).unwrap_or(&short_id);
        let stash_msg = message
            .map(|m| m.to_owned())
            .unwrap_or_else(|| format!("WIP on {branch}: {short_id} {head_subject}"));

        // Commit standard della fotografia dell'index, figlio di HEAD.
        let committer = self
            .repository
            .committer()
            .ok_or_else(|| GitError::Stash("committer non configurato".to_owned()))?
            .map_err(|e| GitError::Stash(e.to_string()))?;
        let index_obj = gix::objs::Commit {
            message: format!("index on {branch}: {short_id} {head_subject}").into(),
            tree: index_tree_id,
            author: committer.into(),
            committer: committer.into(),
            encoding: None,
            parents: std::iter::once(head_id).collect(),
            extra_headers: Vec::new(),
        };
        let index_id = self
            .repository
            .write_object(&index_obj)
            .map_err(|e| GitError::Stash(e.to_string()))?
            .detach();

        // Il commit worktree usa la topologia riconosciuta da Git:
        // primo parent HEAD, secondo parent fotografia dell'index.
        let stash_obj = gix::objs::Commit {
            message: stash_msg.clone().into(),
            tree: tree_id,
            author: committer.into(),
            committer: committer.into(),
            encoding: None,
            parents: [head_id, index_id].into_iter().collect(),
            extra_headers: Vec::new(),
        };
        let stash_id = self
            .repository
            .write_object(&stash_obj)
            .map_err(|e| GitError::Stash(e.to_string()))?
            .detach();

        // Ottieni l'OID attuale di refs/stash (se esiste) per il campo
        // «previous» del reflog.
        let previous_id = self
            .repository
            .try_find_reference("refs/stash")
            .ok()
            .flatten()
            .and_then(|r| r.into_fully_peeled_id().ok())
            .map(|id| id.detach())
            .unwrap_or(gix::ObjectId::null(gix::hash::Kind::Sha1));

        // Aggiorna refs/stash con il RefLog aggiunto.
        use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
        self.repository
            .edit_reference(RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: true,
                        message: stash_msg.into(),
                    },
                    expected: if previous_id.is_null() {
                        PreviousValue::MustNotExist
                    } else {
                        PreviousValue::Any
                    },
                    new: gix::refs::Target::Object(stash_id),
                },
                name: "refs/stash"
                    .try_into()
                    .map_err(|_| GitError::Stash("nome refs/stash invalido".to_owned()))?,
                deref: false,
            })
            .map_err(|e| GitError::Stash(e.to_string()))?;

        Ok(())
    }

    /// Salva e ripulisce tracked, stage e file non versionati tramite la
    /// semantica nativa di Git. L'operazione va invocata su worker; il processo
    /// usa il contenimento disponibile in `GitCli` e resta annullabile.
    pub fn stash_save(
        &self,
        cli: &GitCli,
        message: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<bool, GitError> {
        if cancel.load(Ordering::Acquire) {
            return Err(GitError::Cancelled);
        }
        if self.merge_in_progress() {
            return Err(GitError::Stash(
                "impossibile salvare stash durante un merge".into(),
            ));
        }
        let index = self
            .repository
            .open_index()
            .map_err(|error| GitError::Stash(error.to_string()))?;
        if index.entries().iter().any(|entry| {
            entry.stage() != gix::index::entry::Stage::Unconflicted
                || entry
                    .flags
                    .contains(gix::index::entry::Flags::INTENT_TO_ADD)
        }) {
            return Err(GitError::Stash(
                "salvataggio stash rifiutato: conflitti o intent-to-add nell'index".into(),
            ));
        }
        self.refuse_unowned_update_filters(
            &index,
            gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
            false,
            None,
            cancel,
        )?;
        let previous = self
            .stash_list()?
            .first()
            .map(|entry| entry.commit_id.clone());
        let mut args = vec!["stash", "push", "--include-untracked"];
        let message_arg;
        if let Some(message) = message {
            message_arg = message.to_owned();
            args.push("-m");
            args.push(&message_arg);
        }
        let output = cli.run(&self.work_dir, &args, cancel)?;
        if output.success() {
            let current = self
                .stash_list()?
                .first()
                .map(|entry| entry.commit_id.clone());
            Ok(current != previous)
        } else {
            Err(GitError::Stash(output.failure_text()))
        }
    }

    /// Rimuove la voce stash@{`index`} dal reflog di `refs/stash`.
    ///
    /// Se è l'unico stash, cancella anche il ref.
    pub fn stash_drop(&self, index: usize) -> Result<(), GitError> {
        self.drop_stash_entry(index, None).map(|_| ())
    }

    /// Rimuove esattamente l'OID selezionato, anche se il suo indice nel
    /// reflog è cambiato mentre veniva applicato. `false` significa che un
    /// altro processo lo ha già rimosso.
    pub(crate) fn stash_drop_oid(&self, oid: &gix::ObjectId) -> Result<bool, GitError> {
        self.drop_stash_entry(0, Some(oid))
    }

    fn drop_stash_entry(
        &self,
        index: usize,
        expected_oid: Option<&gix::ObjectId>,
    ) -> Result<bool, GitError> {
        self.drop_stash_entry_with(index, expected_oid, || Ok(()))
    }

    fn drop_stash_entry_with<F>(
        &self,
        index: usize,
        expected_oid: Option<&gix::ObjectId>,
        before_ref_publish: F,
    ) -> Result<bool, GitError>
    where
        F: FnOnce() -> Result<(), GitError>,
    {
        // refs/stash e il suo reflog appartengono al repository comune, anche
        // quando l'handle è stato scoperto da un worktree collegato.
        let git_dir = self.repository.common_dir();
        let log_path = git_dir.join("logs").join("refs").join("stash");
        let log_lock_path = log_path.with_extension("lock");
        // create_new è il lock: exists() seguito da una scrittura lascia una
        // finestra TOCTOU e segue i symlink rotti.
        let log_lock = GitLockFile::create(log_lock_path.clone())?;
        // Anche uno stash push aggiunge una riga al reflog senza usare il lock
        // del reflog. Il lock della ref impedisce che quella riga venga persa
        // tra la lettura e la pubblicazione del log concatenato.
        let ref_path = git_dir.join("refs").join("stash");
        let ref_lock = GitLockFile::create(ref_path.with_extension("lock"))?;
        let contents = match fs::read(&log_path) {
            Ok(contents) => contents,
            Err(error)
                if expected_oid.is_some() && error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(false);
            }
            Err(error) => return Err(GitError::Stash(error.to_string())),
        };
        let mut lines: Vec<Vec<u8>> = contents
            .split(|&byte| byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        if expected_oid.is_none() && index >= lines.len() {
            return Err(GitError::Stash(format!(
                "stash@{{{index}}} non esiste (totale: {})",
                lines.len()
            )));
        }
        let current_oid = match self
            .repository
            .try_find_reference("refs/stash")
            .map_err(|error| GitError::Stash(error.to_string()))?
        {
            Some(reference) => reference
                .into_fully_peeled_id()
                .map_err(|error| GitError::Stash(error.to_string()))?
                .detach(),
            None if expected_oid.is_some() => return Ok(false),
            None => return Err(GitError::Stash("refs/stash non esiste".into())),
        };
        let oid_len = current_oid.to_string().len();
        let last_line = lines
            .last()
            .ok_or_else(|| GitError::Stash("reflog stash vuoto".into()))?;
        let logged_head = last_line
            .get(oid_len + 1..oid_len * 2 + 1)
            .ok_or_else(|| GitError::Stash("reflog stash malformato".into()))?;
        if gix::ObjectId::from_hex(logged_head)
            .map_err(|error| GitError::Stash(error.to_string()))?
            != current_oid
        {
            return Err(GitError::Stash(
                "refs/stash è cambiato durante la preparazione del drop".into(),
            ));
        }

        let file_row = if let Some(expected_oid) = expected_oid {
            let found = lines.iter().position(|line| {
                line.get(oid_len + 1..oid_len * 2 + 1)
                    .and_then(|bytes| gix::ObjectId::from_hex(bytes).ok())
                    .as_ref()
                    == Some(expected_oid)
            });
            let Some(row) = found else {
                return Ok(false);
            };
            row
        } else {
            lines
                .len()
                .checked_sub(index + 1)
                .ok_or_else(|| GitError::Stash("indice stash fuori range".into()))?
        };
        lines.remove(file_row);
        let mut previous = vec![b'0'; oid_len];
        for line in &mut lines {
            if line.len() < oid_len * 2 + 2 || line.get(oid_len) != Some(&b' ') {
                return Err(GitError::Stash("reflog stash malformato".into()));
            }
            line.get_mut(..oid_len)
                .ok_or_else(|| GitError::Stash("reflog stash malformato".into()))?
                .copy_from_slice(&previous);
            previous.copy_from_slice(
                line.get(oid_len + 1..oid_len * 2 + 1)
                    .ok_or_else(|| GitError::Stash("reflog stash malformato".into()))?,
            );
        }

        if lines.is_empty() {
            // Il delete transazionale di gix aggiorna insieme ref e reflog.
            // Rilascia entrambi i lock perché la transazione ne prende uno.
            drop(ref_lock);
            drop(log_lock);
            use gix::refs::transaction::{Change, PreviousValue, RefEdit, RefLog};
            return self
                .repository
                .edit_reference(RefEdit {
                    change: Change::Delete {
                        expected: PreviousValue::MustExistAndMatch(gix::refs::Target::Object(
                            current_oid,
                        )),
                        log: RefLog::AndReference,
                    },
                    name: "refs/stash"
                        .try_into()
                        .map_err(|_| GitError::Stash("nome refs/stash invalido".into()))?,
                    deref: false,
                })
                .map(|_| true)
                .map_err(|error| GitError::Stash(error.to_string()));
        }

        let mut reflog = Vec::with_capacity(contents.len());
        for line in &lines {
            reflog.extend_from_slice(line);
            reflog.push(b'\n');
        }
        let last_line = lines
            .last()
            .ok_or_else(|| GitError::Stash("reflog stash vuoto".into()))?;
        let new_oid_bytes = last_line
            .get(oid_len + 1..oid_len * 2 + 1)
            .ok_or_else(|| GitError::Stash("reflog stash malformato".into()))?;
        let new_oid = gix::ObjectId::from_hex(new_oid_bytes)
            .map_err(|error| GitError::Stash(error.to_string()))?;
        // Pubblica prima il reflog concatenato, poi la nuova punta se cambiata.
        // Se la seconda pubblicazione fallisce, ripristina il reflog soltanto
        // dopo aver verificato che refs/stash conservi ancora la punta letta
        // sotto lock. Se lo stato non è verificabile, non sovrascrive altro.
        log_lock.publish(&log_path, &reflog)?;
        if new_oid != current_oid {
            let ref_contents = format!("{new_oid}\n");
            if let Err(error) = before_ref_publish()
                .and_then(|()| ref_lock.publish(&ref_path, ref_contents.as_bytes()))
            {
                let previous_ref = format!("{current_oid}\n");
                let ref_still_previous = self
                    .repository
                    .try_find_reference("refs/stash")
                    .ok()
                    .flatten()
                    .and_then(|reference| reference.into_fully_peeled_id().ok())
                    .is_some_and(|id| id.detach() == current_oid)
                    || fs::read(&ref_path).is_ok_and(|bytes| bytes == previous_ref.as_bytes());
                if !ref_still_previous {
                    return Err(GitError::Stash(format!(
                        "refs/stash cambiata o non verificabile dopo il fallimento; reflog già aggiornato, verifica manuale richiesta: {error}"
                    )));
                }
                let restore = GitLockFile::create(log_lock_path.clone())
                    .and_then(|lock| lock.publish(&log_path, &contents));
                return match restore {
                    Ok(()) => Err(GitError::Stash(format!(
                        "refs/stash non aggiornata; reflog ripristinato dopo errore di pubblicazione: {error}"
                    ))),
                    Err(restore_error) => Err(GitError::Stash(format!(
                        "refs/stash non aggiornata; reflog pubblicato ma ripristino fallito, verifica manuale richiesta: {restore_error}; errore ref: {error}"
                    ))),
                };
            }
        }
        Ok(true)
    }

    // ──────────────────────────────────────────────────────────────────────
    // Fetch HTTPS
    // ──────────────────────────────────────────────────────────────────────

    /// `true` se la configurazione ha un remote di nome «origin».
    ///
    /// Non guarda i branch `origin/*`: un remote appena aggiunto non ne ha
    /// finché non lo si scarica, ed è proprio il caso in cui il pulsante
    /// Fetch serve.
    pub fn has_origin(&self) -> bool {
        self.repository.find_remote("origin").is_ok()
    }

    /// Esegue `fetch origin` bloccante via gix (HTTP o SSH secondo l'URL).
    ///
    /// Aggiorna i refs remoti locali e ritorna il nome del remote («origin»).
    /// Non si può annullare: la UI usa `fetch_origin_with`.
    pub fn fetch_origin(&self) -> Result<String, GitError> {
        self.fetch_origin_with(&AtomicBool::new(false))
    }

    /// Come `fetch_origin`, annullabile: alzando `cancel` (da un altro thread)
    /// gix smette di leggere il pacchetto e la funzione ritorna
    /// `GitError::Cancelled`. Un fetch interrotto può lasciare oggetti già
    /// scaricati ma **non aggiorna i refs**.
    pub fn fetch_origin_with(&self, cancel: &AtomicBool) -> Result<String, GitError> {
        self.fetch_named("origin", cancel)
    }

    /// Fetch di un remote qualsiasi, annullabile (lo usa anche il pull).
    pub fn fetch_named(&self, remote: &str, cancel: &AtomicBool) -> Result<String, GitError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(GitError::Cancelled);
        }
        let found = self
            .repository
            .find_remote(remote)
            .map_err(|e| GitError::Fetch(format!("remote «{remote}» non trovato: {e}")))?;
        let name = found
            .name()
            .map(|n| n.as_bstr().to_string())
            .unwrap_or_else(|| remote.to_owned());

        // Queste due fasi non ricevono il flag: i loro errori non possono
        // diventare «annullato» solo perché l'utente ha premuto nel frattempo.
        let prepared = found
            .connect(gix::remote::Direction::Fetch)
            .map_err(|error| GitError::Fetch(error.to_string()))?
            .prepare_fetch(gix::progress::Discard, Default::default())
            .map_err(|error| GitError::Fetch(error.to_string()))?;
        if cancel.load(Ordering::Relaxed) {
            return Err(GitError::Cancelled);
        }
        prepared
            .receive(gix::progress::Discard, cancel)
            .map_err(|error| fetch_receive_error(error, cancel.load(Ordering::Relaxed)))?;
        Ok(name)
    }

    fn relative_path<'a>(&self, path: &'a Path) -> Result<&'a Path, GitError> {
        if path.is_absolute() {
            path.strip_prefix(&self.work_dir)
                .map_err(|_| GitError::InvalidPath(path.to_path_buf()))
        } else if path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        }) {
            Err(GitError::InvalidPath(path.to_path_buf()))
        } else {
            Ok(path)
        }
    }

    /// Mette in stage un singolo hunk: aggiorna il blob nell'index applicando
    /// solo le modifiche del hunk al contenuto attuale dell'index (o HEAD se
    /// il file non è ancora in stage).
    ///
    /// L'hunk deve provenire da un `FileDiff` calcolato con
    /// `diff_worktree_file`; usarlo con diff commit produce risultati
    /// indefiniti.
    pub fn stage_hunk(&self, path: &Path, hunk: &diff::DiffHunk) -> Result<(), GitError> {
        let relative = self.relative_path(path)?;
        let git_path = gix::path::into_bstr(relative).into_owned();

        // Lock prima del caricamento, nessuna cache mtime per lo snapshot.
        let mut index =
            LockedIndex::acquire(&self.repository, MissingIndex::Head).map_err(GitError::Stage)?;

        let content = self.blob_bytes_from_index_or_head(&index, git_path.as_bstr())?;
        let new_content = apply_hunk_to_bytes(
            &content,
            &hunk.old_lines,
            &hunk.new_lines,
            hunk.old_range.start as usize,
        )
        .ok_or_else(|| {
            GitError::Stage(format!(
                "impossibile applicare l'hunk a «{}»: righe non trovate nell'index",
                relative.display()
            ))
        })?;

        self.update_index_entry(&mut index, relative, git_path.as_bstr(), &new_content)?;
        index.sort_entries();
        index.remove_tree();
        index.commit().map_err(GitError::Stage)
    }

    /// Rimuove un singolo hunk dall'area di stage: ripristina le righe
    /// originali (da HEAD) nel blob dell'index per la sezione del hunk.
    ///
    /// Se dopo la rimozione il blob nell'index coincide con HEAD, il file
    /// viene rimosso dall'index completamente (come farebbe `unstage` intero).
    pub fn unstage_hunk(&self, path: &Path, hunk: &diff::DiffHunk) -> Result<(), GitError> {
        let relative = self.relative_path(path)?;
        let git_path = gix::path::into_bstr(relative).into_owned();

        let mut index = LockedIndex::acquire(&self.repository, MissingIndex::Head)
            .map_err(GitError::Unstage)?;

        let content = self.blob_bytes_from_index_or_head(&index, git_path.as_bstr())?;
        let new_content = apply_hunk_to_bytes(
            &content,
            &hunk.new_lines,
            &hunk.old_lines,
            hunk.new_range.start as usize,
        )
        .ok_or_else(|| {
            GitError::Unstage(format!(
                "impossibile rimuovere l'hunk da «{}»: righe non trovate nell'index",
                relative.display()
            ))
        })?;

        // Se il risultato coincide con HEAD il file torna allo stato «non
        // modificato»: meglio rimuoverlo dall'index che tenere un blob
        // identico a quello del commit.
        let head_content = self
            .head_blob(git_path.as_bstr())?
            .map(|id| {
                self.repository
                    .find_object(id)
                    .map(|obj| obj.data.to_vec())
                    .map_err(|e| GitError::Unstage(e.to_string()))
            })
            .transpose()?
            .unwrap_or_default();

        if new_content == head_content {
            // Non riacquisire il lock tramite unstage(): stessa lease/snapshot.
            self.unstage_index_paths(&mut index, [path])?;
            index.sort_entries();
            return index.commit().map_err(GitError::Unstage);
        }

        self.update_index_entry(&mut index, relative, git_path.as_bstr(), &new_content)?;
        index.sort_entries();
        index.remove_tree();
        index.commit().map_err(GitError::Unstage)
    }

    // ──────────────────────────────────────────────────────────────────────
    // Helper privati per lo staging per hunk
    // ──────────────────────────────────────────────────────────────────────

    /// Legge i byte del blob per `git_path` dall'index già caricato.
    /// Se il path non è in stage, ritorna il blob di HEAD; se non c'è
    /// nemmeno in HEAD (file nuovo), ritorna `Vec::new()`.
    fn blob_bytes_from_index_or_head(
        &self,
        index: &gix::index::File,
        git_path: &gix::bstr::BStr,
    ) -> Result<Vec<u8>, GitError> {
        if let Some(entry) = index.entry_by_path(git_path) {
            let obj = self
                .repository
                .find_object(entry.id)
                .map_err(|e| GitError::Stage(e.to_string()))?;
            return Ok(obj.data.to_vec());
        }
        if let Some(blob_id) = self.head_blob(git_path)? {
            let obj = self
                .repository
                .find_object(blob_id)
                .map_err(|e| GitError::Stage(e.to_string()))?;
            return Ok(obj.data.to_vec());
        }
        Ok(Vec::new())
    }

    /// Scrive `content` come nuovo blob, poi aggiorna (rimuove il vecchio
    /// entry e aggiunge quello nuovo) nell'index già caricato. Non chiama
    /// `sort_entries()` né `write()`: lo fa il chiamante.
    fn update_index_entry(
        &self,
        index: &mut gix::index::File,
        relative: &Path,
        git_path: &gix::bstr::BStr,
        content: &[u8],
    ) -> Result<(), GitError> {
        let blob_id = self
            .repository
            .write_blob(content)
            .map_err(|e| GitError::Stage(e.to_string()))?
            .detach();

        let absolute = self.work_dir.join(relative);
        let meta = gix::index::fs::Metadata::from_path_no_follow(&absolute)
            .map_err(|e| GitError::Stage(e.to_string()))?;
        let mut stat =
            gix::index::entry::Stat::from_fs(&meta).map_err(|e| GitError::Stage(e.to_string()))?;

        // Stage/unstage per hunk: il blob nell'index può differire dal worktree
        // a parità di dimensione (es. «riga 2» → «RIGA 2»). Se riusiamo mtime e
        // size del file, gix status presume «invariato» senza rileggere i byte
        // (racy-git) e lo staging parziale sparisce. Invalida lo stat cached
        // quando i contenuti non coincidono, così Index↔Worktree resta visibile.
        let worktree_coincide = std::fs::read(&absolute)
            .ok()
            .is_some_and(|bytes| bytes.as_slice() == content);
        if !worktree_coincide {
            stat.size = u32::try_from(content.len()).unwrap_or(u32::MAX);
            stat.mtime = gix::index::entry::stat::Time { secs: 0, nsecs: 0 };
            stat.ctime = gix::index::entry::stat::Time { secs: 0, nsecs: 0 };
        }

        // Conserva la mode originale (eseguibile, symlink…) se già tracciato.
        let mode = index
            .entry_by_path(git_path)
            .map(|e| e.mode)
            .unwrap_or(gix::index::entry::Mode::FILE);

        // Rimuovi il vecchio entry (se presente) e aggiungi quello aggiornato.
        index.remove_entries(|_, candidate, _| candidate == git_path);
        index.dangerously_push_entry(
            stat,
            blob_id,
            gix::index::entry::Flags::empty(),
            mode,
            git_path,
        );

        Ok(())
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Utility per lo staging per hunk
// ────────────────────────────────────────────────────────────────────────────

/// Divide il blob (sempre a LF nell'object store) in righe senza terminatore.
/// L'eventuale riga vuota finale generata dal `\n` di chiusura viene rimossa.
fn split_content_lines(content: &[u8]) -> Vec<&[u8]> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&[u8]> = content.split(|&b| b == b'\n').collect();
    if lines.last() == Some(&(b"" as &[u8])) {
        lines.pop();
    }
    lines
}

/// Cerca la sequenza `needle` nelle `lines`, partendo da `expected` (0-based).
/// Prova prima la posizione esatta, poi cerca in un raggio di ±8 righe per
/// compensare l'offset prodotto dagli hunk già applicati.
/// Restituisce `None` se la sequenza non è presente.
fn find_hunk_position(lines: &[&[u8]], needle: &[String], expected: usize) -> Option<usize> {
    if needle.is_empty() {
        // Inserimento puro: il punto di inserimento è `expected`, clamped.
        return Some(expected.min(lines.len()));
    }
    let n = needle.len();

    let matches_at = |pos: usize| -> bool {
        let Some(end) = pos.checked_add(n) else {
            return false;
        };
        let Some(candidate) = lines.get(pos..end) else {
            return false;
        };
        needle
            .iter()
            .zip(candidate)
            .all(|(needle_line, line)| needle_line.as_bytes() == *line)
    };

    if matches_at(expected) {
        return Some(expected);
    }

    const RADIUS: usize = 8;
    let lo = expected.saturating_sub(RADIUS);
    let hi = (expected + RADIUS + 1).min(lines.len().saturating_sub(n) + 1);
    (lo..hi).find(|&pos| pos != expected && matches_at(pos))
}

/// Applica un'operazione hunk al contenuto di un blob:
/// - trova `old_lines` nell'array di righe (vicino a `expected_pos`)
/// - sostituisce la sequenza trovata con `new_lines`
/// - ricostruisce i byte con terminatori `\n`
///
/// Restituisce `None` se `old_lines` non è trovata.
fn apply_hunk_to_bytes(
    content: &[u8],
    old_lines: &[String],
    new_lines: &[String],
    expected_pos: usize,
) -> Option<Vec<u8>> {
    let lines = split_content_lines(content);
    let pos = find_hunk_position(&lines, old_lines, expected_pos)?;
    let old_count = old_lines.len();
    let before = lines.get(..pos)?;
    let after = lines.get(pos.checked_add(old_count)?..)?;

    let mut result = Vec::with_capacity(content.len());
    for line in before {
        result.extend_from_slice(line);
        result.push(b'\n');
    }
    for new_line in new_lines {
        result.extend_from_slice(new_line.as_bytes());
        result.push(b'\n');
    }
    for line in after {
        result.extend_from_slice(line);
        result.push(b'\n');
    }
    Some(result)
}

fn change_kind(summary: Summary) -> ChangeKind {
    match summary {
        Summary::Added => ChangeKind::Added,
        Summary::Removed => ChangeKind::Removed,
        Summary::Modified => ChangeKind::Modified,
        Summary::TypeChange => ChangeKind::TypeChanged,
        Summary::Renamed => ChangeKind::Renamed,
        Summary::Copied => ChangeKind::Copied,
        Summary::IntentToAdd => ChangeKind::IntentToAdd,
        Summary::Conflict => ChangeKind::Conflict,
    }
}

fn index_change_kind(change: &IndexChange) -> ChangeKind {
    match change {
        IndexChange::Addition { .. } => ChangeKind::Added,
        IndexChange::Deletion { .. } => ChangeKind::Removed,
        IndexChange::Modification {
            previous_entry_mode,
            entry_mode,
            ..
        } if previous_entry_mode != entry_mode => ChangeKind::TypeChanged,
        IndexChange::Modification { .. } => ChangeKind::Modified,
        IndexChange::Rewrite { copy: true, .. } => ChangeKind::Copied,
        IndexChange::Rewrite { copy: false, .. } => ChangeKind::Renamed,
    }
}

#[cfg(test)]
mod fetch_error_tests {
    use super::{GitError, fetch_receive_error};

    #[test]
    fn il_flag_tardivo_non_nasconde_un_errore_reale() {
        for error in [
            gix::Error::from_error(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "rete interrotta",
            )),
            gix::Error::from_error(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "disco protetto",
            )),
            gix::Error::from_error(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "interrupted by user",
            )),
            gix::Error::from_error(gix::error::retryable("errore di autenticazione")),
            gix::Error::from_error(gix::error::message("Interrupted")),
            // Un contesto Retryable non rende annullata una causa generica.
            gix::Error::from_error(gix::error::ClassificationMarker::with_source(
                gix::error::Class::Retryable,
                gix::Error::from_error(gix::error::message("Interrupted")),
            )),
        ] {
            assert!(matches!(
                fetch_receive_error(error, true),
                GitError::Fetch(_)
            ));
        }
    }

    #[test]
    fn riconosce_le_interruzioni_del_lettore_e_dell_indicizzatore() {
        for error in [
            gix::Error::from_error(std::io::Error::other("Interrupted")),
            gix::Error::from_error(gix::error::retryable("Interrupted")),
            gix::Error::from_error(gix::error::retryable("Operation interrupted")),
        ] {
            assert!(matches!(
                fetch_receive_error(error, true),
                GitError::Cancelled
            ));
        }
    }

    #[test]
    fn riconosce_il_segnale_della_negoziazione() {
        let error = gix::Error::from_error(gix::error::ClassificationMarker::with_source(
            gix::error::Class::Retryable,
            gix::error::message(
                "We were unable to figure out what objects the server should send after 2 round(s)",
            ),
        ));
        assert!(matches!(
            fetch_receive_error(error, true),
            GitError::Cancelled
        ));
    }

    #[test]
    fn riconosce_l_annullamento_della_sideband() {
        let error = gix::Error::from_error(std::io::Error::other("interrupted by user"));
        assert!(matches!(
            fetch_receive_error(error, true),
            GitError::Cancelled
        ));
    }

    #[test]
    fn senza_richiesta_di_annullamento_la_sideband_resta_un_errore() {
        let error = gix::Error::from_error(std::io::Error::other("interrupted by user"));
        let GitError::Fetch(message) = fetch_receive_error(error, false) else {
            panic!("la sideband senza richiesta di annullamento deve restare un errore");
        };
        assert!(message.contains("interrupted by user"));
    }

    #[test]
    fn senza_richiesta_di_annullamento_conserva_anche_un_errore_interrupted() {
        let error = gix::Error::from_error(std::io::Error::other("Interrupted"));
        assert!(matches!(
            fetch_receive_error(error, false),
            GitError::Fetch(_)
        ));
    }
}
