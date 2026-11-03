//! La tabella d'inattività: **un solo posto** in cui si decide quando una
//! funzionalità passa da «viva e in uso» a «viva e dormiente», e quando il
//! suo timer smette di contare.
//!
//! Prima c'erano due campi fatti a mano solo per SQL (`sql_closed_at`,
//! `sql_idle_epoch`). Qui il meccanismo è generico e guidato da
//! `FeatureSpec::idle`: una funzionalità con `idle: None` non arma mai
//! nulla, una con `Some(durata)` diventa **Dormiente** quando ha risorse
//! vive e nessuna vista che le usa.
//!
//! **È una funzione pura.** `reconcile` non legge l'orologio né l'app: le
//! arrivano i tre fatti (risorsa viva, in uso, istante) e decide. Il
//! risultato si prova senza aprire una finestra né aspettare cinque minuti.
//!
//! **L'epoca è il token di generazione del timer.** `Instant` non è `Hash`,
//! quindi non può essere la chiave della subscription; l'epoca sì, e
//! cambia a **ogni** ingresso e a ogni uscita dalla dormienza. Un timer
//! armato in un'epoca vecchia, anche se il suo messaggio è già in volo,
//! non combacia più e `accepts` lo respinge.
//!
//! Non si salva lo stato «vivo»: lo dice chi possiede la risorsa, e arriva
//! qui come parametro (`feature_holds_resources`: per il Database l'esistenza
//! dello `SqlWorkspace`, per le altre `feature_live_count`).
//!
//! **Contratto con `stop_feature`.** Lo stream del timer invia un solo
//! messaggio e finisce: un timer nuovo nasce solo se cambia l'epoca. Una
//! funzionalità con `idle: Some(_)` deve quindi avere in `stop_feature` un
//! ramo che libera davvero qualcosa e porta `feature_holds_resources` a
//! falso; altrimenti resterebbe Dormiente per sempre.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::features::Feature;

/// Un timer da armare: ciò che serve alla subscription per nascere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IdleArm {
    pub(crate) epoch: u64,
    pub(crate) timeout: Duration,
}

/// Per ogni funzionalità: `None` = nessun conto alla rovescia; `Some(t)` =
/// Dormiente dall'istante `t`. Il secondo campo è l'epoca, che non torna
/// mai indietro e sopravvive alla dormienza.
#[derive(Debug, Default)]
pub(crate) struct IdleTable {
    states: BTreeMap<Feature, (Option<Instant>, u64)>,
}

impl IdleTable {
    /// Allinea lo stato al mondo. Idempotente.
    ///
    /// Restituisce `Some(arm)` **solo sul fronte** in cui la funzionalità
    /// diventa Dormiente (risorsa viva, nessuna vista in uso, `timeout`
    /// presente): è il momento in cui chi chiama può fare il lavoro «una
    /// volta sola alla chiusura». Chiamarla di nuovo mentre è già Dormiente
    /// restituisce `None` e non tocca l'epoca; per conoscere i timer in
    /// corso si usa [`IdleTable::dormant`].
    ///
    /// Esce dalla dormienza (incrementando l'epoca) quando la funzionalità
    /// torna in uso **o** smette di essere viva: senza risorsa non c'è nulla
    /// da far dormire, e un timer sarebbe una subscription accesa per niente.
    pub(crate) fn reconcile(
        &mut self,
        feature: Feature,
        live: bool,
        in_use: bool,
        now: Instant,
        timeout: Option<Duration>,
    ) -> Option<IdleArm> {
        let should_sleep = live && !in_use && timeout.is_some();
        // Una funzionalità mai addormentata non occupa una voce.
        if !should_sleep && !self.states.contains_key(&feature) {
            return None;
        }
        let (closed_at, epoch) = self.states.entry(feature).or_insert((None, 0));
        match (closed_at.is_some(), should_sleep) {
            (false, true) => {
                *epoch += 1;
                *closed_at = Some(now);
                timeout.map(|timeout| IdleArm {
                    epoch: *epoch,
                    timeout,
                })
            }
            (true, false) => {
                *epoch += 1;
                *closed_at = None;
                None
            }
            _ => None,
        }
    }

    /// Il messaggio di scadenza `(feature, epoch)` vale ancora?
    ///
    /// Tre condizioni insieme: la funzionalità è Dormiente, l'epoca è quella
    /// corrente, e nel frattempo nessuno l'ha riaperta (`in_use` è letto
    /// **adesso** dal chiamante, non dal momento dell'armamento: il messaggio
    /// può essere rimasto in coda dietro un click).
    pub(crate) fn accepts(&self, feature: Feature, epoch: u64, in_use: bool) -> bool {
        !in_use
            && self
                .states
                .get(&feature)
                .is_some_and(|&(closed_at, current)| closed_at.is_some() && current == epoch)
    }

    /// I timer che devono esistere adesso: `(funzionalità, epoca)` per ogni
    /// Dormiente. Da qui nasce `Subscription::batch`, e solo da qui.
    pub(crate) fn dormant(&self) -> impl Iterator<Item = (Feature, u64)> + '_ {
        self.states
            .iter()
            .filter(|(_, (closed_at, _))| closed_at.is_some())
            .map(|(&feature, &(_, epoch))| (feature, epoch))
    }

    /// La funzionalità è Dormiente? Serve ai test: la produzione itera
    /// [`IdleTable::dormant`].
    #[cfg(test)]
    pub(crate) fn is_dormant(&self, feature: Feature) -> bool {
        self.states
            .get(&feature)
            .is_some_and(|(closed_at, _)| closed_at.is_some())
    }

    /// L'epoca corrente. Serve ai test; il chiamante di produzione la riceve
    /// da [`IdleArm`] e da [`IdleTable::dormant`].
    #[cfg(test)]
    pub(crate) fn epoch(&self, feature: Feature) -> u64 {
        self.states.get(&feature).map_or(0, |&(_, epoch)| epoch)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    const CINQUE_MINUTI: Option<Duration> = Some(Duration::from_secs(5 * 60));

    fn db(table: &mut IdleTable, live: bool, in_use: bool, now: Instant) -> Option<IdleArm> {
        table.reconcile(Feature::Database, live, in_use, now, CINQUE_MINUTI)
    }

    #[test]
    fn a_freddo_non_c_e_niente_da_far_dormire() {
        let mut table = IdleTable::default();
        let t0 = Instant::now();
        // Ferma: né viva né in uso, e anche «viva» se l'app dice che non lo è
        assert!(db(&mut table, false, false, t0).is_none());
        assert!(!table.is_dormant(Feature::Database));
        assert_eq!(table.dormant().count(), 0);
        assert!(!table.accepts(Feature::Database, 0, false));
        assert!(!table.accepts(Feature::Database, 1, false));
    }

    #[test]
    fn in_uso_e_attiva_e_non_arma_nulla() {
        let mut table = IdleTable::default();
        assert!(db(&mut table, true, true, Instant::now()).is_none());
        assert!(!table.is_dormant(Feature::Database));
        assert_eq!(table.dormant().count(), 0);
    }

    #[test]
    fn la_prima_chiusura_arma_un_timer_una_volta_sola() {
        let mut table = IdleTable::default();
        let t0 = Instant::now();
        db(&mut table, true, true, t0);
        let arm = db(&mut table, true, false, t0).unwrap();
        assert_eq!(arm.timeout, Duration::from_secs(300));
        assert!(table.is_dormant(Feature::Database));
        assert_eq!(
            table.dormant().collect::<Vec<_>>(),
            vec![(Feature::Database, arm.epoch)]
        );
        // idempotente: nessun nuovo fronte, nessuna nuova epoca
        assert!(db(&mut table, true, false, t0 + Duration::from_secs(10)).is_none());
        assert_eq!(table.epoch(Feature::Database), arm.epoch);
    }

    #[test]
    fn riapertura_invalida_il_timer_precedente() {
        let mut table = IdleTable::default();
        let t0 = Instant::now();
        let primo = db(&mut table, true, false, t0).unwrap();
        assert!(table.accepts(Feature::Database, primo.epoch, false));
        db(&mut table, true, true, t0 + Duration::from_secs(30));
        assert!(!table.is_dormant(Feature::Database));
        assert!(
            table.epoch(Feature::Database) > primo.epoch,
            "anche la riapertura avanza l'epoca"
        );
        assert!(!table.accepts(Feature::Database, primo.epoch, false));
        let secondo = db(&mut table, true, false, t0 + Duration::from_secs(31)).unwrap();
        assert_ne!(primo.epoch, secondo.epoch);
        assert!(
            secondo.epoch > primo.epoch,
            "le epoche non tornano indietro"
        );
        assert!(!table.accepts(Feature::Database, primo.epoch, false));
        assert!(table.accepts(Feature::Database, secondo.epoch, false));
    }

    #[test]
    fn la_scadenza_corrente_vale_solo_se_nessuno_l_ha_riaperta() {
        let mut table = IdleTable::default();
        let arm = db(&mut table, true, false, Instant::now()).unwrap();
        assert!(table.accepts(Feature::Database, arm.epoch, false));
        assert!(
            !table.accepts(Feature::Database, arm.epoch, true),
            "epoca giusta ma la vista e' tornata in uso: il messaggio e' in ritardo"
        );
    }

    #[test]
    fn perdere_la_risorsa_spegne_il_conto_alla_rovescia() {
        let mut table = IdleTable::default();
        let t0 = Instant::now();
        let arm = db(&mut table, true, false, t0).unwrap();
        // «Ferma» a mano: la risorsa cade mentre e' Dormiente
        assert!(db(&mut table, false, false, t0 + Duration::from_secs(5)).is_none());
        assert!(!table.is_dormant(Feature::Database));
        assert_eq!(table.dormant().count(), 0);
        assert!(!table.accepts(Feature::Database, arm.epoch, false));
    }

    #[test]
    fn senza_timeout_non_si_arma_mai() {
        let mut table = IdleTable::default();
        let t0 = Instant::now();
        for feature in [Feature::Terminal, Feature::LanguageServer, Feature::Git] {
            assert!(
                table.reconcile(feature, true, false, t0, None).is_none(),
                "{feature:?}: viva, senza viste, ma senza timeout resta Attiva"
            );
            assert!(!table.is_dormant(feature));
            assert!(!table.accepts(feature, 0, false));
            assert!(!table.accepts(feature, 1, false));
        }
        assert_eq!(table.dormant().count(), 0);
    }

    #[test]
    fn le_funzionalita_sono_indipendenti() {
        let mut table = IdleTable::default();
        let t0 = Instant::now();
        let arm = db(&mut table, true, false, t0).unwrap();
        assert!(
            table
                .reconcile(Feature::Terminal, true, false, t0, None)
                .is_none()
        );
        assert!(table.accepts(Feature::Database, arm.epoch, false));
        assert!(!table.accepts(Feature::Terminal, arm.epoch, false));
    }
}
