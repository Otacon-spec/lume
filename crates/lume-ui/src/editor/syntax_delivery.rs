//! Guardia di consegna da conservare per la richiesta corrente del gruppo.
use lume_syntax::{LanguageId, SyntaxCompletion, SyntaxReceiver, SyntaxRequest};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxIdentity {
    pub document: u64,
    pub epoch: u64,
    pub revision: u64,
    pub language: LanguageId,
}

pub struct SyntaxRequestGuard {
    owner: SyntaxReceiver,
    ticket: u64,
    identity: SyntaxIdentity,
    window: Range<usize>,
}

impl SyntaxRequestGuard {
    pub fn targets(&self, document: u64) -> bool {
        self.identity.document == document
    }

    pub fn new(owner: SyntaxReceiver, ticket: u64, request: &SyntaxRequest) -> Self {
        Self {
            owner,
            ticket,
            identity: SyntaxIdentity {
                document: request.document,
                epoch: request.epoch,
                revision: request.revision,
                language: request.language,
            },
            window: request.window.clone(),
        }
    }

    /// Il chiamante deve ottenere current dal workspace vivo, non dal messaggio.
    /// Un esito rifiutato non libera la richiesta corrente né la sua guardia.
    pub fn accepts(
        &self,
        owner: &SyntaxReceiver,
        completion: &SyntaxCompletion,
        current: SyntaxIdentity,
    ) -> bool {
        &self.owner == owner
            && self.ticket == completion.ticket
            && self.identity == current
            && completion.document == current.document
            && completion.epoch == current.epoch
            && completion.revision == current.revision
            && completion.result.as_ref().is_none_or(|result| {
                result.document == current.document
                    && result.epoch == current.epoch
                    && result.snapshot.covers(
                        current.language,
                        current.revision,
                        self.window.clone(),
                    )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lume_core::{DocId, Document};
    use lume_syntax::SyntaxWorker;

    #[test]
    fn old_failure_same_revision_does_not_clear_successor_request() -> Result<(), &'static str> {
        let worker = SyntaxWorker::default();
        let owner = worker.receiver();
        let document = Document::from_str("fn main() {}\n", None, DocId(1));
        let request = SyntaxRequest {
            document: 1,
            epoch: 7,
            revision: 0,
            language: LanguageId::Rust,
            source: document.rope().clone(),
            window: 0..document.rope().len_bytes(),
            edit: None,
        };
        let expected = SyntaxIdentity {
            document: 1,
            epoch: 7,
            revision: 0,
            language: LanguageId::Rust,
        };
        let guard = SyntaxRequestGuard::new(owner.clone(), 2, &request);
        let mut completion = SyntaxCompletion {
            ticket: 1,
            document: 1,
            epoch: 7,
            revision: 0,
            result: None,
        };
        assert!(!guard.accepts(&owner, &completion, expected));
        completion.ticket = 2;
        assert!(guard.accepts(&owner, &completion, expected));
        let other_worker = SyntaxWorker::default();
        assert!(!guard.accepts(&other_worker.receiver(), &completion, expected));
        for changed in [
            SyntaxIdentity {
                epoch: 8,
                ..expected
            },
            SyntaxIdentity {
                document: 2,
                ..expected
            },
            SyntaxIdentity {
                revision: 1,
                ..expected
            },
            SyntaxIdentity {
                language: LanguageId::Toml,
                ..expected
            },
        ] {
            assert!(!guard.accepts(&owner, &completion, changed));
        }
        for (language, revision, window, accepted) in [
            (LanguageId::Rust, 0, request.window.clone(), true),
            (LanguageId::Rust, 1, request.window.clone(), false),
            (LanguageId::Toml, 0, request.window.clone(), false),
            (LanguageId::Rust, 0, 0..0, false),
        ] {
            let syntax = lume_syntax::SyntaxState::new(language, document.rope().slice(..))
                .ok_or("parse")?;
            let snapshot = syntax
                .highlight_snapshot(document.rope().slice(..), revision, window)
                .ok_or("snapshot")?;
            completion.result = Some(lume_syntax::SyntaxResult {
                document: 1,
                epoch: 7,
                snapshot,
            });
            assert_eq!(guard.accepts(&owner, &completion, expected), accepted);
        }
        Ok(())
    }
}
