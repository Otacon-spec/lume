//! Due owner fissi per i due gruppi editor; nessun worker per scheda aperta.
use crate::{SyntaxCollector, SyntaxReceiver, SyntaxRequest, SyntaxWorker};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SyntaxLane {
    Primary,
    Secondary,
}

#[derive(Default)]
pub struct SyntaxPool {
    primary: SyntaxWorker,
    secondary: SyntaxWorker,
}
impl SyntaxPool {
    pub fn submit(&mut self, lane: SyntaxLane, request: SyntaxRequest) -> std::io::Result<u64> {
        match lane {
            SyntaxLane::Primary => self.primary.submit(request),
            SyntaxLane::Secondary => self.secondary.submit(request),
        }
    }

    pub fn revoke(&mut self, lane: SyntaxLane) -> std::io::Result<()> {
        match lane {
            SyntaxLane::Primary => self.primary.revoke(),
            SyntaxLane::Secondary => self.secondary.revoke(),
        }
    }

    pub fn receiver(&self, lane: SyntaxLane) -> SyntaxReceiver {
        match lane {
            SyntaxLane::Primary => self.primary.receiver(),
            SyntaxLane::Secondary => self.secondary.receiver(),
        }
    }

    pub fn started_threads(&self) -> usize {
        usize::from(self.primary.is_started()) + usize::from(self.secondary.is_started())
    }

    /// Entrambi gli handle vanno raccolti fuori UI, anche durante cambio radice.
    pub fn shutdown(self) -> [SyntaxCollector; 2] {
        [self.primary.shutdown(), self.secondary.shutdown()]
    }
}
