//! Motore sincrono da possedere esclusivamente sul worker, senza riferimenti UI.
use crate::{HighlightSnapshot, InputEditRaw, LanguageId, SyntaxState};
use ropey::Rope;
use std::{
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub struct SyntaxRequest {
    pub document: u64,
    pub epoch: u64,
    pub revision: u64,
    pub language: LanguageId,
    pub source: Rope,
    pub window: Range<usize>,
    /// Revisione base ed edit: opzionale quando la coda salta revisioni.
    pub edit: Option<(u64, InputEditRaw)>,
}

#[derive(Clone, Debug)]
pub struct SyntaxResult {
    pub document: u64,
    pub epoch: u64,
    pub snapshot: HighlightSnapshot,
}

struct Parsed {
    document: u64,
    epoch: u64,
    revision: u64,
    source: Rope,
    state: SyntaxState,
}

/// Conserva al massimo un parser/albero/testo, appartenenti alla stessa revisione.
/// Non avvia thread: l'owner dell'app deve eseguirlo e raccoglierlo fuori UI.
#[derive(Default)]
pub struct SyntaxProcessor {
    parsed: Option<Parsed>,
}

impl SyntaxProcessor {
    /// Da eseguire sul worker: libera parser/tree/testo della scheda revocata.
    pub fn clear(&mut self) {
        self.parsed = None;
    }

    pub fn process(&mut self, request: SyntaxRequest, cancel: &AtomicBool) -> Option<SyntaxResult> {
        if cancel.load(Ordering::Acquire)
            || request.window.start > request.window.end
            || request.window.end > request.source.len_bytes()
        {
            return None;
        }
        let matching = self.parsed.as_mut().filter(|p| {
            p.document == request.document
                && p.epoch == request.epoch
                && p.state.lang() == request.language
        });
        let reused = if let Some(parsed) = matching {
            if parsed.revision == request.revision && parsed.source == request.source {
                true
            } else if let Some((base, edit)) = request.edit.filter(|(base, edit)| {
                *base == parsed.revision && valid_edit(&parsed.source, &request.source, edit)
            }) {
                let _ = base;
                if !parsed
                    .state
                    .apply_edit_cancellable(&edit, request.source.slice(..), cancel)
                {
                    return None;
                }
                parsed.revision = request.revision;
                parsed.source = request.source.clone();
                true
            } else {
                false
            }
        } else {
            false
        };
        if !reused {
            // Libera anche l'albero del documento precedente prima della parse
            // completa: nessuna doppia cache di alberi durante cambio documento.
            self.parsed = None;
            let state =
                SyntaxState::new_cancellable(request.language, request.source.slice(..), cancel)?;
            self.parsed = Some(Parsed {
                document: request.document,
                epoch: request.epoch,
                revision: request.revision,
                source: request.source.clone(),
                state,
            });
        }
        let parsed = self.parsed.as_ref()?;
        let snapshot = parsed.state.highlight_snapshot_cancellable(
            parsed.source.slice(..),
            parsed.revision,
            request.window,
            cancel,
        )?;
        Some(SyntaxResult {
            document: parsed.document,
            epoch: parsed.epoch,
            snapshot,
        })
    }
}

fn boundary(rope: &Rope, byte: usize) -> Option<usize> {
    let character = rope.try_byte_to_char(byte).ok()?;
    (rope.char_to_byte(character) == byte).then_some(character)
}

fn point(rope: &Rope, byte: usize) -> (usize, usize) {
    let line = rope.byte_to_line(byte);
    (line, byte - rope.line_to_byte(line))
}

/// Verifica sul worker che l'edit descriva davvero i due Rope: edit saltati,
/// coordinate UTF-8 errate o basi obsolete richiedono parse completa.
fn valid_edit(old: &Rope, new: &Rope, edit: &InputEditRaw) -> bool {
    if edit.start_byte > edit.old_end_byte || edit.start_byte > edit.new_end_byte {
        return false;
    }
    let Some(start_old) = boundary(old, edit.start_byte) else {
        return false;
    };
    let Some(end_old) = boundary(old, edit.old_end_byte) else {
        return false;
    };
    let Some(start_new) = boundary(new, edit.start_byte) else {
        return false;
    };
    let Some(end_new) = boundary(new, edit.new_end_byte) else {
        return false;
    };
    edit.start_point == point(old, edit.start_byte)
        && edit.start_point == point(new, edit.start_byte)
        && edit.old_end_point == point(old, edit.old_end_byte)
        && edit.new_end_point == point(new, edit.new_end_byte)
        && old.slice(..start_old) == new.slice(..start_new)
        && old.slice(end_old..) == new.slice(end_new..)
}

#[cfg(test)]
mod diagnostics {
    use super::*;

    #[test]
    #[ignore = "diagnostica release validazione Rope, non budget prodotto"]
    fn large_rope_validation_cost() -> Result<(), &'static str> {
        use std::{hint::black_box, time::Instant};
        let prefix = "// città 漢字 😀\nfn main() { println!(\"é\"); }\n";
        let old = Rope::from_str(&format!(
            "{prefix}\n{}",
            "// padding ascii per corpus\n".repeat(320_000)
        ));
        let same = old.clone();
        let mut new = old.clone();
        new.insert(old.byte_to_char(prefix.len()), "a");
        let edit = InputEditRaw {
            start_byte: prefix.len(),
            old_end_byte: prefix.len(),
            new_end_byte: prefix.len() + 1,
            start_point: (2, 0),
            old_end_point: (2, 0),
            new_end_point: (2, 1),
        };
        for mode in ["same_revision", "validate_edit"] {
            let mut samples = Vec::new();
            for sample in 0..101 {
                let start = Instant::now();
                let valid = if mode == "same_revision" {
                    black_box(&old) == black_box(&same)
                } else {
                    valid_edit(black_box(&old), black_box(&new), black_box(&edit))
                };
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                assert!(black_box(valid));
                if sample > 0 {
                    samples.push(elapsed);
                }
            }
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "ROPE_VALIDATION mode={mode} bytes={} samples={} median_ms={:.6} p95_ms={:.6}",
                old.len_bytes(),
                samples.len(),
                samples.get(50).ok_or("median sample")?,
                samples.get(95).ok_or("p95 sample")?
            );
        }
        Ok(())
    }
}
