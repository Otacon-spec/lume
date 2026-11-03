use std::collections::HashMap;

use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, Range, TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentItem, TextDocumentSyncKind, Uri, VersionedTextDocumentIdentifier,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentChange {
    Full(String),
    Incremental {
        range: Range,
        range_length: Option<u32>,
        text: String,
        resulting_text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DocumentError {
    #[error("document is already owned by this session: {0:?}")]
    AlreadyOpen(Uri),
    #[error("document is not owned by this session: {0:?}")]
    NotOpen(Uri),
    #[error("document synchronization is disabled")]
    SyncDisabled,
    #[error("document version space exhausted: {0:?}")]
    VersionExhausted(Uri),
}

#[derive(Debug, Clone)]
struct OpenDocument {
    language_id: String,
    version: i32,
    text: String,
}

/// Stato nuovo posseduto dal caller, ancora non applicato allo store.
pub(crate) struct PreparedDocumentChange {
    uri: Uri,
    document: OpenDocument,
}

#[derive(Debug, Clone)]
pub struct DocumentStore {
    sync_kind: TextDocumentSyncKind,
    documents: HashMap<Uri, OpenDocument>,
}

impl DocumentStore {
    pub fn new(sync_kind: TextDocumentSyncKind) -> Self {
        Self {
            sync_kind,
            documents: HashMap::new(),
        }
    }

    pub(crate) fn set_sync_kind(&mut self, sync_kind: TextDocumentSyncKind) {
        self.sync_kind = sync_kind;
    }

    pub fn open(
        &mut self,
        uri: Uri,
        language_id: String,
        text: String,
    ) -> Result<DidOpenTextDocumentParams, DocumentError> {
        let (params, prepared) = self.prepare_open(uri, language_id, text)?;
        self.commit_change(prepared);
        Ok(params)
    }

    pub(crate) fn prepare_open(
        &self,
        uri: Uri,
        language_id: String,
        text: String,
    ) -> Result<(DidOpenTextDocumentParams, PreparedDocumentChange), DocumentError> {
        if self.sync_kind == TextDocumentSyncKind::NONE {
            return Err(DocumentError::SyncDisabled);
        }
        if self.documents.contains_key(&uri) {
            return Err(DocumentError::AlreadyOpen(uri));
        }
        let document = OpenDocument {
            language_id: language_id.clone(),
            version: 1,
            text: text.clone(),
        };
        let prepared = PreparedDocumentChange {
            uri: uri.clone(),
            document,
        };
        Ok((
            DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri,
                    language_id,
                    version: 1,
                    text,
                },
            },
            prepared,
        ))
    }

    pub fn change(
        &mut self,
        uri: &Uri,
        change: DocumentChange,
    ) -> Result<DidChangeTextDocumentParams, DocumentError> {
        let (params, prepared) = self.prepare_change(uri, change)?;
        self.commit_change(prepared);
        Ok(params)
    }

    pub(crate) fn prepare_change(
        &self,
        uri: &Uri,
        change: DocumentChange,
    ) -> Result<(DidChangeTextDocumentParams, PreparedDocumentChange), DocumentError> {
        if self.sync_kind == TextDocumentSyncKind::NONE {
            return Err(DocumentError::SyncDisabled);
        }
        let document = self
            .documents
            .get(uri)
            .ok_or_else(|| DocumentError::NotOpen(uri.clone()))?;
        let version = document
            .version
            .checked_add(1)
            .ok_or_else(|| DocumentError::VersionExhausted(uri.clone()))?;
        let (text, incremental) = match change {
            DocumentChange::Full(text) => (text, None),
            DocumentChange::Incremental {
                range,
                range_length,
                text,
                resulting_text,
            } => (
                resulting_text,
                Some(TextDocumentContentChangeEvent {
                    range: Some(range),
                    range_length,
                    text,
                }),
            ),
        };
        let content_change = if self.sync_kind == TextDocumentSyncKind::INCREMENTAL {
            incremental.unwrap_or_else(|| TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.clone(),
            })
        } else {
            TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.clone(),
            }
        };
        let prepared = PreparedDocumentChange {
            uri: uri.clone(),
            document: OpenDocument {
                language_id: document.language_id.clone(),
                version,
                text,
            },
        };
        Ok((
            DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                content_changes: vec![content_change],
            },
            prepared,
        ))
    }

    pub(crate) fn commit_change(&mut self, prepared: PreparedDocumentChange) {
        self.documents.insert(prepared.uri, prepared.document);
    }

    /// Replace the tracked text, sending a minimal UTF-16 ranged edit when the
    /// server negotiated incremental synchronization.
    pub fn change_text(
        &mut self,
        uri: &Uri,
        new_text: &str,
    ) -> Result<DidChangeTextDocumentParams, DocumentError> {
        let (params, prepared) = self.prepare_change_text(uri, new_text)?;
        self.commit_change(prepared);
        Ok(params)
    }

    pub(crate) fn prepare_change_text(
        &self,
        uri: &Uri,
        new_text: &str,
    ) -> Result<(DidChangeTextDocumentParams, PreparedDocumentChange), DocumentError> {
        self.prepare_change_text_owned(uri, new_text.to_owned())
    }

    pub(crate) fn prepare_change_text_owned(
        &self,
        uri: &Uri,
        new_text: String,
    ) -> Result<(DidChangeTextDocumentParams, PreparedDocumentChange), DocumentError> {
        if self.sync_kind != TextDocumentSyncKind::INCREMENTAL {
            return self.prepare_change(uri, DocumentChange::Full(new_text));
        }
        let old_text = &self
            .documents
            .get(uri)
            .ok_or_else(|| DocumentError::NotOpen(uri.clone()))?
            .text;
        let prefix = common_prefix_bytes(old_text.as_bytes(), new_text.as_bytes());
        // The common byte prefix can end inside a scalar. Move it back to a
        // UTF-8 boundary before deriving LSP positions.
        let mut prefix = prefix;
        while !old_text.is_char_boundary(prefix) {
            prefix -= 1;
        }

        let old_tail = &old_text[prefix..];
        let new_tail = &new_text[prefix..];
        let matching_suffix = common_suffix_utf8(old_tail, new_tail);
        let old_end = old_text.len() - matching_suffix;
        let new_end = new_text.len() - matching_suffix;
        let start = lsp_position(&old_text[..prefix]);
        let end = lsp_position(&old_text[..old_end]);
        let removed = &old_text[prefix..old_end];
        let replacement = &new_text[prefix..new_end];
        self.prepare_change(
            uri,
            DocumentChange::Incremental {
                range: Range::new(start, end),
                range_length: u32::try_from(removed.encode_utf16().count()).ok(),
                text: replacement.to_owned(),
                resulting_text: new_text,
            },
        )
    }

    pub fn save(
        &self,
        uri: &Uri,
        include_text: bool,
    ) -> Result<DidSaveTextDocumentParams, DocumentError> {
        let document = self
            .documents
            .get(uri)
            .ok_or_else(|| DocumentError::NotOpen(uri.clone()))?;
        Ok(DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            text: include_text.then(|| document.text.clone()),
        })
    }

    pub fn close(&mut self, uri: &Uri) -> Result<DidCloseTextDocumentParams, DocumentError> {
        let params = self.prepare_close(uri)?;
        self.commit_close(uri);
        Ok(params)
    }

    pub(crate) fn prepare_close(
        &self,
        uri: &Uri,
    ) -> Result<DidCloseTextDocumentParams, DocumentError> {
        if !self.documents.contains_key(uri) {
            return Err(DocumentError::NotOpen(uri.clone()));
        }
        Ok(DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
        })
    }

    pub(crate) fn commit_close(&mut self, uri: &Uri) {
        self.documents.remove(uri);
    }

    pub fn reopen_documents(&self) -> Vec<DidOpenTextDocumentParams> {
        let mut documents = self
            .documents
            .iter()
            .map(|(uri, document)| DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: document.language_id.clone(),
                    version: document.version,
                    text: document.text.clone(),
                },
            })
            .collect::<Vec<_>>();
        documents.sort_by(|left, right| {
            left.text_document
                .uri
                .as_str()
                .cmp(right.text_document.uri.as_str())
        });
        documents
    }
}

// Comparazioni safe di blocchi: la scansione scalare resta solo nel blocco
// che contiene la differenza. I confini UTF8 si ripristinano prima del delta.
#[allow(clippy::indexing_slicing)] // matched + 64 <= min(len), matched <= min(len)
fn common_prefix_bytes(a: &[u8], b: &[u8]) -> usize {
    let limit = a.len().min(b.len());
    let mut matched = 0;
    while limit - matched >= 64 && a[matched..matched + 64] == b[matched..matched + 64] {
        matched += 64;
    }
    matched
        + a[matched..]
            .iter()
            .zip(&b[matched..])
            .take_while(|(x, y)| x == y)
            .count()
}

#[allow(clippy::indexing_slicing)] // matched <= min(len), blocco solo se restano 64 byte
fn common_suffix_utf8(a: &str, b: &str) -> usize {
    let limit = a.len().min(b.len());
    let mut matched = 0;
    while limit - matched >= 64
        && a.as_bytes()[a.len() - matched - 64..a.len() - matched]
            == b.as_bytes()[b.len() - matched - 64..b.len() - matched]
    {
        matched += 64;
    }
    matched += a.as_bytes()[..a.len() - matched]
        .iter()
        .rev()
        .zip(b.as_bytes()[..b.len() - matched].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    while !a.is_char_boundary(a.len() - matched) || !b.is_char_boundary(b.len() - matched) {
        matched -= 1;
    }
    matched
}

fn lsp_position(prefix: &str) -> lsp_types::Position {
    let line = memchr::memchr_iter(b'\n', prefix.as_bytes()).count();
    // LF is ASCII: the offset immediately after it is a UTF-8 boundary.
    let tail =
        memchr::memrchr(b'\n', prefix.as_bytes()).map_or(prefix, |offset| &prefix[offset + 1..]);
    let character = utf16_units(tail);
    lsp_types::Position::new(
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(character).unwrap_or(u32::MAX),
    )
}

fn utf16_units(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.encode_utf16().count()
    }
}

#[cfg(test)]
mod prepared_tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn preparazione_abbandonata_non_modifica_byte_versioni_o_altri_documenti() {
        let a: Uri = "file:///a.rs".parse().unwrap();
        let b: Uri = "file:///b.rs".parse().unwrap();
        let mut store = DocumentStore::new(TextDocumentSyncKind::INCREMENTAL);
        store
            .open(a.clone(), "rust".into(), "a😀z\n".into())
            .unwrap();
        store
            .open(b.clone(), "rust".into(), "estraneo".repeat(1000))
            .unwrap();
        let before = store.reopen_documents();
        let other_ptr = store.documents[&b].text.as_ptr();
        let (params, prepared) = store.prepare_change_text(&a, "a漢z\n").unwrap();
        assert_eq!(params.text_document.version, 2);
        assert_eq!(params.content_changes[0].range_length, Some(2));
        assert_eq!(store.reopen_documents(), before);
        drop(prepared);
        assert_eq!(store.reopen_documents(), before);
        let (_, prepared) = store.prepare_change_text(&a, "a漢z\n").unwrap();
        store.commit_change(prepared);
        assert_eq!(store.documents[&a].version, 2);
        assert_eq!(store.documents[&a].text, "a漢z\n");
        assert_eq!(store.documents[&b].text.as_ptr(), other_ptr);
        assert_eq!(store.documents[&b].version, 1);
    }

    #[test]
    fn preparazione_full_e_errori_preservano_store() {
        let a: Uri = "file:///a.rs".parse().unwrap();
        let mut store = DocumentStore::new(TextDocumentSyncKind::FULL);
        store.open(a.clone(), "rust".into(), "old".into()).unwrap();
        let (params, prepared) = store.prepare_change_text(&a, "new").unwrap();
        assert_eq!(params.content_changes[0].range, None);
        assert_eq!(params.content_changes[0].text, "new");
        drop(prepared);
        assert_eq!(store.documents[&a].text, "old");
        store.documents.get_mut(&a).unwrap().version = i32::MAX;
        assert!(matches!(
            store.prepare_change_text(&a, "new"),
            Err(DocumentError::VersionExhausted(_))
        ));
        assert_eq!(store.documents[&a].text, "old");
        store.sync_kind = TextDocumentSyncKind::NONE;
        assert!(matches!(
            store.prepare_change_text(&a, "new"),
            Err(DocumentError::SyncDisabled)
        ));
    }
    #[test]
    fn testo_owned_preparato_trasferisce_allocazione_senza_modificare_store() {
        for sync in [
            TextDocumentSyncKind::INCREMENTAL,
            TextDocumentSyncKind::FULL,
        ] {
            let mut store = DocumentStore::new(sync);
            let uri: Uri = "file:///owned.rs".parse().unwrap();
            store
                .open(uri.clone(), "rust".into(), "prima 😀\n".into())
                .unwrap();
            let text = "dopo 漢\n".to_owned();
            let allocation = text.as_ptr();
            let (_, prepared) = store.prepare_change_text_owned(&uri, text).unwrap();
            assert_eq!(prepared.document.text.as_ptr(), allocation);
            assert_eq!(store.documents[&uri].text, "prima 😀\n");
            assert_eq!(store.documents[&uri].version, 1);
            store.commit_change(prepared);
            assert_eq!(store.documents[&uri].text, "dopo 漢\n");
            assert_eq!(store.documents[&uri].version, 2);
        }
    }

    #[test]
    fn posizioni_e_lunghezza_utf16_equivalgono_oracolo_scalare() {
        let mut corpus = vec![String::new()];
        let mut level = vec![String::new()];
        for _ in 0..4 {
            level = level
                .iter()
                .flat_map(|s| ["a", "é", "漢", "😀", "\r", "\n"].map(|c| format!("{s}{c}")))
                .collect();
            corpus.extend(level.clone());
        }
        for n in [63, 64, 65, 127, 128, 129, 4096] {
            corpus.push(format!("{}😀\n{}é", "a".repeat(n), "b".repeat(n)));
        }
        for text in corpus {
            assert_eq!(
                utf16_units(&text),
                text.chars().map(char::len_utf16).sum::<usize>()
            );
            let mut expected = lsp_types::Position::new(0, 0);
            for (offset, ch) in text.char_indices() {
                assert_eq!(lsp_position(&text[..offset]), expected);
                if ch == '\n' {
                    expected.line += 1;
                    expected.character = 0;
                } else {
                    expected.character += ch.len_utf16() as u32;
                }
            }
            assert_eq!(lsp_position(&text), expected);
        }
    }

    #[test]
    fn confronto_blocchi_equivale_oracolo_utf8() {
        let mut corpus = vec![String::new()];
        let mut level = vec![String::new()];
        for _ in 0..3 {
            level = level
                .iter()
                .flat_map(|s| ["a", "b", "é", "漢", "😀", "\n"].map(|c| format!("{s}{c}")))
                .collect();
            corpus.extend(level.clone());
        }
        for n in [63, 64, 65, 127, 128, 129] {
            corpus.push(format!("{}😀", "a".repeat(n)));
            corpus.push(format!("b{}漢", "a".repeat(n)));
        }
        for a in &corpus {
            for b in &corpus {
                let raw = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
                assert_eq!(common_prefix_bytes(a.as_bytes(), b.as_bytes()), raw);
                let oracle: usize = a
                    .chars()
                    .rev()
                    .zip(b.chars().rev())
                    .take_while(|(x, y)| x == y)
                    .map(|(c, _)| c.len_utf8())
                    .sum();
                assert_eq!(common_suffix_utf8(a, b), oracle, "{a:?} / {b:?}");
            }
        }
    }

    #[test]
    #[ignore = "diagnostica release opt-in, non budget prodotto"]
    fn misura_prepare_change_multi_documenti() {
        let baseline = std::env::var("LUME_MISURA_ROLLBACK").unwrap() == "clone-store";
        let owned = std::env::var("LUME_MISURA_OWNED").is_ok_and(|value| value == "1");
        let corpus = std::env::var("LUME_MISURA_CORPUS").unwrap_or_else(|_| "ascii".into());
        let position = std::env::var("LUME_MISURA_POSIZIONE").unwrap_or_else(|_| "start".into());
        assert!(matches!(corpus.as_str(), "ascii" | "unicode"));
        assert!(matches!(position.as_str(), "start" | "middle" | "end"));
        for mib in [1, 8, 31] {
            let mut store = DocumentStore::new(TextDocumentSyncKind::INCREMENTAL);
            let mut uris = Vec::new();
            let bytes = mib * 1024 * 1024;
            let mut payload = if corpus == "unicode" {
                "é界😀\n".repeat(bytes / "é界😀\n".len())
            } else {
                "x".repeat(bytes)
            };
            payload.push_str(&"x".repeat(bytes - payload.len()));
            let mut offset = match position.as_str() {
                "middle" => bytes / 2,
                "end" => bytes - 1,
                _ => 0,
            };
            while !payload.is_char_boundary(offset) {
                offset -= 1;
            }
            let end = offset + payload[offset..].chars().next().unwrap().len_utf8();
            for n in 0..4 {
                let uri: Uri = format!("file:///measure-{n}.rs").parse().unwrap();
                store
                    .open(uri.clone(), "rust".into(), payload.clone())
                    .unwrap();
                uris.push(uri);
            }
            let mut samples = Vec::new();
            for n in 0..7 {
                let mut changed = payload.clone();
                changed.replace_range(offset..end, if n % 2 == 0 { "a" } else { "b" });
                let before = &store.documents[&uris[0]].text;
                let old_end = offset + before[offset..].chars().next().unwrap().len_utf8();
                let scalar_position = |text: &str| {
                    text.chars()
                        .fold(lsp_types::Position::new(0, 0), |mut p, c| {
                            if c == '\n' {
                                p.line += 1;
                                p.character = 0;
                            } else {
                                p.character += c.len_utf16() as u32;
                            }
                            p
                        })
                };
                let expected_range = lsp_types::Range::new(
                    scalar_position(&before[..offset]),
                    scalar_position(&before[..old_end]),
                );
                let expected_length = before[offset..old_end].encode_utf16().count() as u32;
                // Input owned rappresenta la String già materializzata dall'attore.
                let input = owned.then(|| changed.clone());
                let started = std::time::Instant::now();
                let previous = baseline.then(|| store.clone());
                let (params, prepared) = match input {
                    Some(text) => store.prepare_change_text_owned(&uris[0], text),
                    None => store.prepare_change_text(&uris[0], &changed),
                }
                .unwrap();
                store.commit_change(prepared);
                std::hint::black_box(previous.as_ref());
                drop(previous);
                samples.push(started.elapsed().as_micros());
                assert_eq!(params.text_document.version, n + 2);
                assert_eq!(params.content_changes[0].range, Some(expected_range));
                assert_eq!(
                    params.content_changes[0].range_length,
                    Some(expected_length)
                );
                assert_eq!(
                    params.content_changes[0].text,
                    if n % 2 == 0 { "a" } else { "b" }
                );
                assert_eq!(store.documents[&uris[0]].text, changed);
                for uri in &uris[1..] {
                    assert_eq!(store.documents[uri].text, payload);
                    assert_eq!(store.documents[uri].version, 1);
                }
            }
            samples.sort_unstable();
            eprintln!(
                "LSP_PREPARE_MULTI:clone_store={baseline},owned_text={owned},corpus={corpus},position={position},docs=4,mib_per_doc={mib},samples=7,median_us={},max_us={},release={}",
                samples[3],
                samples[6],
                !cfg!(debug_assertions)
            );
        }
    }
}
