//! Nucleo dell'editor: rope, undo, selezioni, filesystem. Nessuna dipendenza UI.
//!
//! Convenzione: tutte le posizioni sono in CHAR INDEX (unità nativa di
//! ropey); i byte compaiono solo in [`AppliedEdit`], pensato per consumer
//! incrementali come tree-sitter.

pub mod canonical;
pub mod document;
pub mod error;
pub mod file_write;
pub mod fs_tree;
pub mod history;
pub mod line_ending;
pub mod movement;
pub mod multi_edit;
pub mod occurrence;
pub mod search;
pub mod selection;
pub mod selection_set;
pub mod trash;
pub mod watcher;
pub mod workspace;

pub use canonical::{canonical_path, canonical_path_lenient, path_is_within};
pub use document::{AppliedEdit, DocId, Document, PreparedReload, PreparedText, SaveSnapshot};
pub use error::CoreError;
pub use fs_tree::{FsNode, FsTree, RowSpec};
pub use history::{ChangeSet, EditKind, History, Op, Revision, Transaction};
pub use line_ending::{LineEnding, normalize, strip_bom};
pub use search::{
    FileMatches, MAX_FIND_MATCHES, RopeMatches, SEARCH_EVENT_CAPACITY, SearchArea, SearchEvents,
    SearchHit, SearchOptions, SearchOrigin, SearchRun, SearchScope, SearchUpdate, find_in_rope,
    find_in_rope_capped, spawn_search, spawn_search_owned,
};
pub use selection::{Assoc, Range};
pub use selection_set::{MAX_SELECTIONS, SelectionError, SelectionSet};
pub use trash::{TrashError, TrashOutcome, trash_path};
pub use watcher::{
    FsEvent, FsEventReceiver, FsEventSender, FsWatcher, FsWatcherCollector, fs_event_channel,
};
pub use workspace::{RebindError, Workspace};
