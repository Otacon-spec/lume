//! Evidenziazione incrementale con tree-sitter: classi semantiche, non colori.

pub mod language;
mod pool;
mod processor;
mod snapshot;
pub mod syntax;
pub mod token;
mod worker;

pub use language::{LanguageId, LanguageSpec, detect, spec, spec_error};
pub use pool::{SyntaxLane, SyntaxPool};
pub use processor::{SyntaxProcessor, SyntaxRequest, SyntaxResult};
pub use snapshot::{FoldRegion, HighlightSnapshot};
pub use syntax::{InputEditRaw, SyntaxState};
pub use token::TokenClass;
pub use worker::{SyntaxCollector, SyntaxCompletion, SyntaxReceiver, SyntaxWorker};
