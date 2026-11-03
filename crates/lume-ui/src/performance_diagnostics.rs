//! Timer opt-in: solo etichette statiche, nessun testo o percorso del documento.
use std::time::Instant;

/// Il tratto misurato esclude la scrittura del risultato su stderr.
pub struct Span {
    label: &'static str,
    start: Instant,
}

impl Span {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            start: Instant::now(),
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        eprintln!(
            "LUME_PHASE label={} elapsed_us={}",
            self.label,
            elapsed.as_micros()
        );
    }
}
