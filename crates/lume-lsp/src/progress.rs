//! Throttling per le notifiche di lavoro del language server.
use lsp_types::{ProgressParams, ProgressParamsValue, WorkDoneProgress};
use std::time::{Duration, Instant};
const DISTANZA_MINIMA: Duration = Duration::from_millis(100);
#[derive(Debug, Default)]
pub struct ProgressThrottle {
    ultimo_inoltro: Option<Instant>,
}
impl ProgressThrottle {
    pub fn admit(&mut self, params: &ProgressParams, now: Instant) -> bool {
        let ProgressParamsValue::WorkDone(WorkDoneProgress::Report(_)) = &params.value else {
            self.ultimo_inoltro = Some(now);
            return true;
        };
        if self
            .ultimo_inoltro
            .is_some_and(|ultimo| now.saturating_duration_since(ultimo) < DISTANZA_MINIMA)
        {
            return false;
        }
        self.ultimo_inoltro = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::ProgressThrottle;
    use lsp_types::{
        NumberOrString, ProgressParams, ProgressParamsValue, WorkDoneProgress,
        WorkDoneProgressBegin, WorkDoneProgressEnd, WorkDoneProgressReport,
    };
    use std::time::{Duration, Instant};
    fn con(value: WorkDoneProgress) -> ProgressParams {
        ProgressParams {
            token: NumberOrString::String("indexing".to_owned()),
            value: ProgressParamsValue::WorkDone(value),
        }
    }
    fn inizio() -> ProgressParams {
        con(WorkDoneProgress::Begin(WorkDoneProgressBegin {
            title: "Indexing".to_owned(),
            ..Default::default()
        }))
    }
    fn passo(percentuale: u32) -> ProgressParams {
        con(WorkDoneProgress::Report(WorkDoneProgressReport {
            percentage: Some(percentuale),
            ..Default::default()
        }))
    }
    fn fine() -> ProgressParams {
        con(WorkDoneProgress::End(WorkDoneProgressEnd::default()))
    }
    #[test]
    fn inizio_e_fine_passano_sempre_i_passi_al_massimo_ogni_cento_millisecondi() {
        let t0 = Instant::now();
        let mut filtro = ProgressThrottle::default();
        assert!(filtro.admit(&inizio(), t0));
        assert!(!filtro.admit(&passo(10), t0 + Duration::from_millis(10)));
        assert!(filtro.admit(&passo(20), t0 + Duration::from_millis(150)));
        assert!(!filtro.admit(&passo(30), t0 + Duration::from_millis(160)));
        assert!(filtro.admit(&fine(), t0 + Duration::from_millis(161)));
    }
}
