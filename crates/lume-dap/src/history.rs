//! La storia degli arresti di una sessione (D-DAP-10).
//!
//! A ogni evento `stopped` il client chiede lo stack e lo registra qui. La
//! vista «Execution Path» mostra la cascata dei frame dell'arresto scelto e si
//! sposta fra gli arresti con Prev/Next. Non è una traccia registrata: sono
//! solo i punti in cui il programma si è fermato.
//!
//! Limiti (design §6): al più [`MAX_STOPS`] arresti e [`MAX_RETAINED_FRAMES`]
//! frame in tutto; si scartano i più vecchi.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::protocol::StackFrame;

/// Frame chiesti per arresto (`stackTrace.levels`).
pub const STACK_LEVELS: usize = 50;
pub const MAX_STOPS: usize = 100;
pub const MAX_RETAINED_FRAMES: usize = 500;
/// Righe di codice conservate per frame (la riga eseguita e il contorno).
pub const MAX_CODE_LINES: usize = 5;
/// Caratteri conservati per riga di codice.
pub const MAX_CODE_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSummary {
    pub name: String,
    pub path: Option<PathBuf>,
    pub source_name: Option<String>,
    pub line: i64,
    pub column: i64,
    /// Le righe di codice attorno a `line` (numero, testo), per il flusso
    /// del codice; vuoto se il sorgente non è leggibile.
    pub code: Vec<(i64, String)>,
}

impl FrameSummary {
    pub fn from_frame(frame: &StackFrame) -> Self {
        let source = frame.source.as_ref();
        Self {
            name: frame.name.clone(),
            path: source
                .and_then(|source| source.path.as_ref())
                .map(PathBuf::from),
            source_name: source.and_then(|source| source.name.clone()),
            line: frame.line,
            column: frame.column,
            code: Vec::new(),
        }
    }

    /// Il nome breve del file, se il frame ha un sorgente.
    pub fn file_label(&self) -> Option<String> {
        self.source_name.clone().or_else(|| {
            self.path
                .as_deref()
                .and_then(Path::file_name)
                .map(|nome| nome.to_string_lossy().into_owned())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopRecord {
    /// Numero progressivo dell'arresto nella sessione (da 1).
    pub number: u64,
    pub reason: String,
    pub description: Option<String>,
    pub thread_id: i64,
    /// Dal più interno (dove si è fermato) al più esterno, come in DAP.
    pub frames: Vec<FrameSummary>,
}

impl StopRecord {
    /// La cascata dall'esterno verso l'interno: `main` in alto, il punto di
    /// arresto in fondo.
    pub fn cascade(&self) -> impl Iterator<Item = (usize, &FrameSummary)> {
        self.frames.iter().enumerate().rev()
    }

    pub fn top(&self) -> Option<&FrameSummary> {
        self.frames.first()
    }

    /// Arresto dovuto a un'eccezione o a un segnale.
    pub fn is_exception(&self) -> bool {
        matches!(self.reason.as_str(), "exception" | "signal")
    }
}

#[derive(Debug, Default, Clone)]
pub struct StopHistory {
    stops: VecDeque<StopRecord>,
    cursor: Option<usize>,
    next_number: u64,
    retained_frames: usize,
}

impl StopHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.stops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stops.is_empty()
    }

    /// Registra un arresto e lo seleziona. Restituisce il suo numero.
    pub fn push(
        &mut self,
        reason: String,
        description: Option<String>,
        thread_id: i64,
        frames: &[StackFrame],
    ) -> u64 {
        self.push_with_code(reason, description, thread_id, frames, Vec::new())
    }

    /// Come [`Self::push`], con le righe di codice di ogni frame (stesso
    /// ordine di `frames`), tagliate a [`MAX_CODE_LINES`] e [`MAX_CODE_CHARS`].
    pub fn push_with_code(
        &mut self,
        reason: String,
        description: Option<String>,
        thread_id: i64,
        frames: &[StackFrame],
        code: Vec<Vec<(i64, String)>>,
    ) -> u64 {
        self.next_number += 1;
        let mut code = code.into_iter();
        let frames: Vec<FrameSummary> = frames
            .iter()
            .take(STACK_LEVELS)
            .map(|frame| {
                let mut summary = FrameSummary::from_frame(frame);
                summary.code = code
                    .next()
                    .unwrap_or_default()
                    .into_iter()
                    .take(MAX_CODE_LINES)
                    .map(|(numero, testo)| (numero, testo.chars().take(MAX_CODE_CHARS).collect()))
                    .collect();
                summary
            })
            .collect();
        self.retained_frames += frames.len();
        self.stops.push_back(StopRecord {
            number: self.next_number,
            reason,
            description,
            thread_id,
            frames,
        });
        while self.stops.len() > MAX_STOPS
            || (self.retained_frames > MAX_RETAINED_FRAMES && self.stops.len() > 1)
        {
            if let Some(scartato) = self.stops.pop_front() {
                self.retained_frames -= scartato.frames.len();
            }
        }
        self.cursor = self.stops.len().checked_sub(1);
        self.next_number
    }

    pub fn current(&self) -> Option<&StopRecord> {
        self.cursor.and_then(|indice| self.stops.get(indice))
    }

    /// L'ultimo arresto: dove il programma è fermo adesso.
    pub fn latest(&self) -> Option<&StopRecord> {
        self.stops.back()
    }

    pub fn is_at_latest(&self) -> bool {
        self.cursor.is_some() && self.cursor == self.stops.len().checked_sub(1)
    }

    /// Posizione 1-based e totale, per «2 / 8».
    pub fn position(&self) -> Option<(usize, usize)> {
        self.cursor.map(|indice| (indice + 1, self.stops.len()))
    }

    pub fn select_previous(&mut self) -> bool {
        match self.cursor {
            Some(indice) if indice > 0 => {
                self.cursor = Some(indice - 1);
                true
            }
            _ => false,
        }
    }

    pub fn select_next(&mut self) -> bool {
        match self.cursor {
            Some(indice) if indice + 1 < self.stops.len() => {
                self.cursor = Some(indice + 1);
                true
            }
            _ => false,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &StopRecord> {
        self.stops.iter()
    }

    /// Le righe di `path` in cui il programma si è fermato, per evidenziarle
    /// nell'editor come «eseguite».
    pub fn lines_hit(&self, path: &Path) -> BTreeSet<i64> {
        self.stops
            .iter()
            .filter_map(StopRecord::top)
            .filter(|frame| frame.path.as_deref() == Some(path))
            .map(|frame| frame.line)
            .collect()
    }

    pub fn clear(&mut self) {
        self.stops.clear();
        self.cursor = None;
        self.retained_frames = 0;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::protocol::Source;

    fn frame(name: &str, line: i64) -> StackFrame {
        StackFrame {
            id: line,
            name: name.into(),
            source: Some(Source {
                name: Some("prova.c".into()),
                path: Some("/p/prova.c".into()),
                source_reference: None,
            }),
            line,
            column: 0,
            instruction_pointer_reference: None,
        }
    }

    #[test]
    fn cascata_dall_esterno_e_navigazione() {
        let mut storia = StopHistory::new();
        storia.push(
            "breakpoint".into(),
            None,
            1,
            &[frame("quadrato", 4), frame("somma", 11), frame("main", 18)],
        );
        storia.push(
            "step".into(),
            None,
            1,
            &[frame("quadrato", 5), frame("somma", 11), frame("main", 18)],
        );
        assert_eq!(storia.position(), Some((2, 2)));
        assert!(storia.is_at_latest());
        let nomi: Vec<_> = storia
            .current()
            .unwrap()
            .cascade()
            .map(|(_, f)| f.name.clone())
            .collect();
        assert_eq!(nomi, ["main", "somma", "quadrato"]);
        assert!(storia.select_previous());
        assert!(!storia.select_previous());
        assert_eq!(storia.position(), Some((1, 2)));
        assert!(!storia.is_at_latest());
        assert!(storia.select_next());
        assert!(!storia.select_next());
        assert_eq!(
            storia
                .lines_hit(Path::new("/p/prova.c"))
                .into_iter()
                .collect::<Vec<_>>(),
            [4, 5]
        );
    }

    #[test]
    fn limiti_di_arresti_e_frame() {
        let mut storia = StopHistory::new();
        for numero in 0..(MAX_STOPS + 10) {
            storia.push(
                "step".into(),
                None,
                1,
                &[frame("f", i64::try_from(numero).unwrap())],
            );
        }
        assert_eq!(storia.len(), MAX_STOPS);
        assert_eq!(storia.iter().next().unwrap().number, 11);
        let profondo: Vec<_> = (0..200).map(|riga| frame("f", riga)).collect();
        storia.push("step".into(), None, 1, &profondo);
        assert_eq!(storia.latest().unwrap().frames.len(), STACK_LEVELS);
        let trattenuti: usize = storia.iter().map(|stop| stop.frames.len()).sum();
        assert!(trattenuti <= MAX_RETAINED_FRAMES);
        assert_eq!(storia.position().unwrap().0, storia.len());
    }

    #[test]
    fn eccezione_e_segnale() {
        let mut storia = StopHistory::new();
        storia.push("signal".into(), Some("SIGSEGV".into()), 1, &[frame("f", 1)]);
        assert!(storia.current().unwrap().is_exception());
        storia.clear();
        assert!(storia.is_empty());
        assert_eq!(storia.position(), None);
    }
}
