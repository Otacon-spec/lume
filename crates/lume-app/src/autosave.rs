//! Macchina a stati pura per l'autosalvataggio delle note.

pub(crate) const RITARDO_MS: u64 = 600;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum StatoSalvataggio {
    #[default]
    Pulito,
    InAttesa {
        versione: u64,
    },
    InCorso {
        versione: u64,
    },
    Conflitto,
    Errore(String),
}

impl StatoSalvataggio {
    pub(crate) fn modificato(&mut self, versione: u64) -> Option<u64> {
        match self {
            Self::Conflitto | Self::InCorso { .. } => None,
            Self::Pulito | Self::InAttesa { .. } | Self::Errore(_) => {
                *self = Self::InAttesa { versione };
                Some(versione)
            }
        }
    }

    pub(crate) fn scaduto(&mut self, timer: u64, attuale: u64) -> bool {
        match self {
            Self::InAttesa { versione } if *versione == timer && timer == attuale => {
                *self = Self::InCorso { versione: timer };
                true
            }
            _ => false,
        }
    }

    /// Avvia subito una scrittura legata a una modifica dei metadati.
    pub(crate) fn avviato(&mut self, versione: u64) -> bool {
        match self {
            Self::Conflitto | Self::InCorso { .. } => false,
            Self::Pulito | Self::InAttesa { .. } | Self::Errore(_) => {
                *self = Self::InCorso { versione };
                true
            }
        }
    }

    pub(crate) fn completato(&mut self, salvata: u64, attuale: u64) -> Option<u64> {
        if !matches!(self, Self::InCorso { versione } if *versione == salvata) {
            return None;
        }
        if salvata == attuale {
            *self = Self::Pulito;
            None
        } else {
            *self = Self::InAttesa { versione: attuale };
            Some(attuale)
        }
    }

    pub(crate) fn fallito(&mut self, errore: String) {
        *self = Self::Errore(errore);
    }

    pub(crate) fn in_conflitto(&mut self) {
        *self = Self::Conflitto;
    }

    pub(crate) fn risolto(&mut self) {
        *self = Self::Pulito;
    }

    pub(crate) fn tornato_pulito(&mut self) {
        if matches!(self, Self::InAttesa { .. }) {
            *self = Self::Pulito;
        }
    }

    pub(crate) fn etichetta(&self) -> Option<String> {
        match self {
            Self::Pulito | Self::InAttesa { .. } => None,
            Self::InCorso { .. } => Some("Salvataggio…".to_owned()),
            Self::Conflitto => Some("Conflitto: la nota è cambiata sul disco".to_owned()),
            Self::Errore(error) => Some(format!("Salvataggio non riuscito: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RITARDO_MS, StatoSalvataggio};

    #[test]
    fn una_modifica_arma_un_timer_e_lo_stato_attende() {
        let mut state = StatoSalvataggio::default();
        assert_eq!(state.modificato(3), Some(3));
        assert_eq!(state, StatoSalvataggio::InAttesa { versione: 3 });
    }

    #[test]
    fn solo_l_ultimo_timer_salva() {
        let mut state = StatoSalvataggio::default();
        state.modificato(3);
        state.modificato(4);
        assert!(!state.scaduto(3, 4));
        assert_eq!(state, StatoSalvataggio::InAttesa { versione: 4 });
        assert!(state.scaduto(4, 4));
        assert_eq!(state, StatoSalvataggio::InCorso { versione: 4 });
    }

    #[test]
    fn un_timer_superato_non_salva() {
        let mut state = StatoSalvataggio::default();
        state.modificato(4);
        assert!(!state.scaduto(4, 5));
    }

    #[test]
    fn durante_il_salvataggio_non_ne_parte_un_altro() {
        let mut state = StatoSalvataggio::default();
        state.modificato(1);
        assert!(state.scaduto(1, 1));
        assert_eq!(state.modificato(2), None);
        assert!(!state.scaduto(2, 2));
    }

    #[test]
    fn completamento_pulisce_o_riarma_secondo_la_versione() {
        let mut state = StatoSalvataggio::default();
        state.modificato(1);
        state.scaduto(1, 1);
        assert_eq!(state.completato(1, 1), None);
        assert_eq!(state, StatoSalvataggio::Pulito);
        state.modificato(2);
        state.scaduto(2, 2);
        state.modificato(3);
        assert_eq!(state.completato(2, 3), Some(3));
        assert_eq!(state, StatoSalvataggio::InAttesa { versione: 3 });
    }

    #[test]
    fn conflitto_blocca_finche_non_viene_risolto() {
        let mut state = StatoSalvataggio::default();
        state.modificato(1);
        state.scaduto(1, 1);
        state.in_conflitto();
        assert_eq!(state.modificato(2), None);
        assert!(!state.scaduto(2, 2));
        state.risolto();
        assert_eq!(state, StatoSalvataggio::Pulito);
        assert_eq!(state.modificato(3), Some(3));
    }

    #[test]
    fn un_errore_permette_il_tentativo_dopo_la_modifica_successiva() {
        let mut state = StatoSalvataggio::default();
        state.modificato(1);
        state.scaduto(1, 1);
        state.fallito("disco pieno".into());
        assert_eq!(state.modificato(2), Some(2));
    }

    #[test]
    fn tornare_al_salvato_annulla_solo_l_attesa() {
        let mut state = StatoSalvataggio::InAttesa { versione: 6 };
        state.tornato_pulito();
        assert_eq!(state, StatoSalvataggio::Pulito);
        let mut saving = StatoSalvataggio::InCorso { versione: 6 };
        saving.tornato_pulito();
        assert_eq!(saving, StatoSalvataggio::InCorso { versione: 6 });
    }

    #[test]
    fn etichetta_mostra_solo_stati_esplicativi() {
        assert_eq!(StatoSalvataggio::Pulito.etichetta(), None);
        assert_eq!(StatoSalvataggio::InAttesa { versione: 1 }.etichetta(), None);
        assert_eq!(
            StatoSalvataggio::InCorso { versione: 1 }
                .etichetta()
                .as_deref(),
            Some("Salvataggio…")
        );
        assert_eq!(
            StatoSalvataggio::Conflitto.etichetta().as_deref(),
            Some("Conflitto: la nota è cambiata sul disco")
        );
        assert_eq!(
            StatoSalvataggio::Errore("disco pieno".into())
                .etichetta()
                .as_deref(),
            Some("Salvataggio non riuscito: disco pieno")
        );
    }

    #[test]
    fn un_salvataggio_immediato_parte_subito_ma_mai_sopra_un_altro() {
        let mut state = StatoSalvataggio::default();
        assert!(state.avviato(2));
        assert_eq!(state, StatoSalvataggio::InCorso { versione: 2 });
        assert!(!state.avviato(3));
        let mut pending = StatoSalvataggio::InAttesa { versione: 4 };
        assert!(pending.avviato(4));
        assert!(!pending.scaduto(4, 4));
        assert!(!StatoSalvataggio::Conflitto.avviato(5));
        assert!(StatoSalvataggio::Errore("disco pieno".into()).avviato(6));
    }

    #[test]
    fn il_ritardo_e_quello_dichiarato() {
        assert_eq!(RITARDO_MS, 600);
    }
}
