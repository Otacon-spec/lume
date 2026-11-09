#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{Chiusura, Modale, OverlayId, OverlayManager};

    #[test]
    fn informazioni_trattiene_modali_e_promuove_in_ordine() {
        let mut manager = OverlayManager::default();
        manager.open(OverlayId::About);
        manager.open(OverlayId::ConfermaElimina);
        manager.open(OverlayId::Scorciatoie);
        manager.open(OverlayId::ConfermaElimina);
        assert_eq!(manager.active(), Some(OverlayId::About));
        assert!(!manager.chiudi_se(OverlayId::Scorciatoie));
        assert_eq!(manager.chiudi().promosso, Some(OverlayId::ConfermaElimina));
        assert_eq!(manager.chiudi().promosso, Some(OverlayId::Scorciatoie));
        assert_eq!(manager.chiudi().promosso, None);
    }

    #[test]
    fn priorita_rifiuto_revoca_e_deduplicazione_preservano_fifo() {
        use super::Apertura;
        let mut m = OverlayManager::default();
        assert_eq!(m.open(OverlayId::Find), Apertura::Aperto);
        assert_eq!(
            m.open(OverlayId::About),
            Apertura::Sostituito(OverlayId::Find)
        );
        assert_eq!(m.open(OverlayId::About), Apertura::Aperto);
        assert!(matches!(m.open(OverlayId::Search), Apertura::Rifiutato(_)));
        assert_eq!(m.open(OverlayId::Scorciatoie), Apertura::InAttesa);
        assert_eq!(m.open(OverlayId::ConfermaElimina), Apertura::InAttesa);
        assert_eq!(m.open(OverlayId::Scorciatoie), Apertura::InAttesa);
        assert!(!m.chiudi_se(OverlayId::ConfermaElimina));
        assert_eq!(
            m.open(OverlayId::ConfermaChiusura),
            Apertura::Sostituito(OverlayId::About)
        );
        m.revoca_attesa(OverlayId::Scorciatoie);
        assert_eq!(m.active(), Some(OverlayId::ConfermaChiusura));
        assert_eq!(m.chiudi().promosso, Some(OverlayId::ConfermaElimina));
        assert_eq!(m.chiudi().promosso, None);
    }

    #[test]
    fn chiusura_condizionata_preserva_overlay_estranei_ed_e_idempotente() {
        let mut manager = OverlayManager::default();
        assert!(!manager.mostra(OverlayId::Menu));
        assert!(!manager.chiudi_se(OverlayId::Menu));
        assert_eq!(manager.active(), None);
        manager.open(OverlayId::About);
        assert!(manager.mostra(OverlayId::About));
        assert!(!manager.mostra(OverlayId::Menu));
        assert!(!manager.chiudi_se(OverlayId::Menu));
        assert_eq!(manager.active(), Some(OverlayId::About));
        assert!(manager.chiudi_se(OverlayId::About));
        assert!(!manager.chiudi_se(OverlayId::About));
        assert_eq!(manager.active(), None);
    }

    #[test]
    fn chiudi_restituisce_solo_il_riquadro_effettivamente_chiuso() {
        let mut manager = OverlayManager::default();
        assert_eq!(
            manager.chiudi(),
            Chiusura {
                chiuso: None,
                promosso: None
            }
        );
        manager.open(OverlayId::Rename);
        assert!(!manager.chiudi_se(OverlayId::GotoLine));
        assert_eq!(manager.active(), Some(OverlayId::Rename));
        assert_eq!(
            manager.chiudi(),
            Chiusura {
                chiuso: Some(OverlayId::Rename),
                promosso: None
            }
        );
        assert_eq!(
            manager.chiudi(),
            Chiusura {
                chiuso: None,
                promosso: None
            }
        );
    }

    #[test]
    fn ogni_modale_ammette_solo_il_proprio_e_i_passaggi_previsti() {
        use super::Ingresso::*;
        let modali = [
            (OverlayId::About, Modale::About),
            (OverlayId::ConfermaChiusura, Modale::ConfermaChiusura),
            (OverlayId::ConfermaElimina, Modale::ConfermaElimina),
            (OverlayId::Scorciatoie, Modale::Scorciatoie),
            #[cfg(feature = "mcp")]
            (OverlayId::ConsensoMcp, Modale::ConsensoMcp),
            #[cfg(feature = "mcp")]
            (OverlayId::CardMcp, Modale::CardMcp),
        ];
        let mut manager = OverlayManager::default();
        for overlay in [None, Some(OverlayId::Menu)] {
            if let Some(id) = overlay {
                manager.open(id);
            }
            for ingresso in [
                Gesto,
                Sfondo,
                Proprio(Modale::About),
                Annulla,
                Asincrono,
                Finestra,
                ChiusuraFinestra,
            ] {
                assert!(
                    manager.ammetti(ingresso).is_ok(),
                    "{overlay:?} {ingresso:?}"
                );
            }
        }
        for (id, modale) in modali {
            let mut manager = OverlayManager::default();
            manager.open(id);
            for ingresso in [
                Annulla,
                Asincrono,
                Finestra,
                ChiusuraFinestra,
                Proprio(modale),
            ] {
                assert!(manager.ammetti(ingresso).is_ok(), "{id:?} {ingresso:?}");
            }
            assert!(manager.ammetti(Sfondo).unwrap_err().avviso().is_none());
            assert!(
                manager
                    .ammetti(Gesto)
                    .unwrap_err()
                    .avviso()
                    .unwrap()
                    .contains("Rispondi prima")
            );
            for (_, altro) in modali {
                if altro != modale {
                    assert!(
                        manager
                            .ammetti(Proprio(altro))
                            .unwrap_err()
                            .avviso()
                            .is_some()
                    );
                }
            }
        }
    }

    #[test]
    fn classifica_tutti_gli_overlay() {
        let casi = [
            (OverlayId::Menu, None),
            (OverlayId::Environment, None),
            (OverlayId::Settings, None),
            (OverlayId::Search, None),
            (OverlayId::Rename, None),
            (OverlayId::Context, None),
            (OverlayId::GotoLine, None),
            (OverlayId::Albero, None),
            (OverlayId::CodeActions, None),
            (OverlayId::Forge, None),
            (OverlayId::Recents, None),
            (OverlayId::Find, None),
            (OverlayId::About, Some(Modale::About)),
            (OverlayId::ConfermaChiusura, Some(Modale::ConfermaChiusura)),
            (OverlayId::ConfermaElimina, Some(Modale::ConfermaElimina)),
            (OverlayId::Scorciatoie, Some(Modale::Scorciatoie)),
            #[cfg(feature = "mcp")]
            (OverlayId::ConsensoMcp, Some(Modale::ConsensoMcp)),
            #[cfg(feature = "mcp")]
            (OverlayId::CardMcp, Some(Modale::CardMcp)),
        ];
        for (overlay, atteso) in casi {
            assert_eq!(overlay.modale(), atteso, "{overlay:?}");
        }
        const ABOUT: Option<Modale> = OverlayId::About.modale();
        assert_eq!(ABOUT, Some(Modale::About));
    }

    #[test]
    fn opening_settings_closes_environment_and_search() {
        let mut overlays = OverlayManager::default();
        overlays.open(OverlayId::Environment);
        overlays.open(OverlayId::Search);
        overlays.open(OverlayId::Settings);
        assert_eq!(overlays.active(), Some(OverlayId::Settings));
    }

    #[test]
    fn ogni_overlay_si_apre_e_si_chiude() {
        let tutti = vec![
            OverlayId::Menu,
            OverlayId::Environment,
            OverlayId::Settings,
            OverlayId::Search,
            OverlayId::Rename,
            OverlayId::Context,
            OverlayId::GotoLine,
            OverlayId::Albero,
            OverlayId::CodeActions,
            OverlayId::Forge,
            OverlayId::Configurazioni,
            OverlayId::Recents,
            OverlayId::Find,
            OverlayId::ConfermaChiusura,
            OverlayId::ConfermaElimina,
            OverlayId::Scorciatoie,
            OverlayId::About,
            #[cfg(feature = "mcp")]
            OverlayId::ConsensoMcp,
            #[cfg(feature = "mcp")]
            OverlayId::CardMcp,
        ];
        let mut overlays = OverlayManager::default();
        assert_eq!(overlays.active(), None);
        for id in tutti {
            // Aggiungere/rimuovere una variante richiede di aggiornare questo
            // match e produce un errore di compilazione se viene dimenticata.
            match id {
                OverlayId::Menu
                | OverlayId::Environment
                | OverlayId::Settings
                | OverlayId::Search
                | OverlayId::Rename
                | OverlayId::Context
                | OverlayId::GotoLine
                | OverlayId::Albero
                | OverlayId::CodeActions
                | OverlayId::Forge
                | OverlayId::Configurazioni
                | OverlayId::Recents
                | OverlayId::Find
                | OverlayId::ConfermaChiusura
                | OverlayId::ConfermaElimina
                | OverlayId::Scorciatoie
                | OverlayId::About => {}
                #[cfg(feature = "mcp")]
                OverlayId::ConsensoMcp | OverlayId::CardMcp => {}
            }
            overlays.open(id);
            assert_eq!(overlays.active(), Some(id));
            overlays.chiudi();
            assert_eq!(overlays.active(), None);
        }
    }

    #[test]
    fn aprire_un_overlay_sostituisce_quello_aperto() {
        let mut overlays = OverlayManager::default();
        overlays.open(OverlayId::Find);
        overlays.open(OverlayId::GotoLine);
        assert_eq!(overlays.active(), Some(OverlayId::GotoLine));
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayId {
    Menu,
    Environment,
    Settings,
    Search,
    /// Il riquadro «Rinomina simbolo».
    Rename,
    /// Il menù contestuale dell'editor.
    Context,
    /// Il riquadro «Vai a riga» (Ctrl+G).
    GotoLine,
    /// Il riquadro «Nuovo file»/«Nuova cartella» dell'albero.
    Albero,
    /// L'elenco delle azioni rapide offerte dal server.
    CodeActions,
    /// Il riquadro per comporre comandi da inviare al terminale.
    Forge,
    /// La finestra «Configurazioni di esecuzione».
    Configurazioni,
    /// L'elenco dei file recenti (Ctrl+E).
    Recents,
    /// Il riquadro «Trova»/«Sostituisci» (Ctrl+F / Ctrl+R).
    Find,
    /// «Salvare le modifiche?» prima di chiudere una scheda o la finestra.
    ConfermaChiusura,
    /// «Eliminare questo file?» per un file dell'albero.
    ConfermaElimina,
    /// Elenco delle scorciatoie da tastiera (Aiuto → Scorciatoie…).
    Scorciatoie,
    About,
    /// «Avviare il server MCP per …?» all'Avvia da Estensioni.
    #[cfg(feature = "mcp")]
    ConsensoMcp,
    #[cfg(feature = "mcp")]
    CardMcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modale {
    About,
    ConfermaChiusura,
    ConfermaElimina,
    Scorciatoie,
    #[cfg(feature = "mcp")]
    ConsensoMcp,
    #[cfg(feature = "mcp")]
    CardMcp,
}

impl OverlayId {
    pub const fn modale(self) -> Option<Modale> {
        match self {
            Self::About => Some(Modale::About),
            Self::ConfermaChiusura => Some(Modale::ConfermaChiusura),
            Self::ConfermaElimina => Some(Modale::ConfermaElimina),
            Self::Scorciatoie => Some(Modale::Scorciatoie),
            #[cfg(feature = "mcp")]
            Self::ConsensoMcp => Some(Modale::ConsensoMcp),
            #[cfg(feature = "mcp")]
            Self::CardMcp => Some(Modale::CardMcp),
            Self::Menu
            | Self::Environment
            | Self::Settings
            | Self::Search
            | Self::Rename
            | Self::Context
            | Self::GotoLine
            | Self::Albero
            | Self::CodeActions
            | Self::Forge
            | Self::Configurazioni
            | Self::Recents
            | Self::Find => None,
        }
    }
}

/// Provenienza di un messaggio, indipendente dall'interfaccia e dalla persistenza.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingresso {
    Gesto,
    Sfondo,
    Proprio(Modale),
    Annulla,
    Asincrono,
    Finestra,
    ChiusuraFinestra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rifiuto {
    modale: Modale,
    silenzioso: bool,
}

impl Rifiuto {
    pub fn avviso(self) -> Option<String> {
        if self.silenzioso {
            return None;
        }
        let nome = match self.modale {
            Modale::About => "Informazioni",
            Modale::ConfermaChiusura => "Salvare le modifiche?",
            Modale::ConfermaElimina => "Eliminare questo file?",
            Modale::Scorciatoie => "Scorciatoie",
            #[cfg(feature = "mcp")]
            Modale::ConsensoMcp => "Consenso MCP",
            #[cfg(feature = "mcp")]
            Modale::CardMcp => "Richiesta MCP",
        };
        Some(format!("Rispondi prima a «{nome}»"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Apertura {
    Aperto,
    Sostituito(OverlayId),
    InAttesa,
    Rifiutato(Rifiuto),
}

/// Effetto della chiusura e dell'eventuale promozione.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chiusura {
    pub chiuso: Option<OverlayId>,
    pub promosso: Option<OverlayId>,
}

#[derive(Debug, Default)]
pub struct OverlayManager {
    active: Option<OverlayId>,
    attese: std::collections::VecDeque<OverlayId>,
}

impl OverlayManager {
    pub fn ammetti(&self, ingresso: Ingresso) -> Result<(), Rifiuto> {
        let Some(modale) = self.active.and_then(OverlayId::modale) else {
            return Ok(());
        };
        match ingresso {
            Ingresso::Annulla
            | Ingresso::Asincrono
            | Ingresso::Finestra
            | Ingresso::ChiusuraFinestra => Ok(()),
            Ingresso::Proprio(proprio) if proprio == modale => Ok(()),
            Ingresso::Sfondo => Err(Rifiuto {
                modale,
                silenzioso: true,
            }),
            Ingresso::Gesto | Ingresso::Proprio(_) => Err(Rifiuto {
                modale,
                silenzioso: false,
            }),
        }
    }

    pub fn open(&mut self, overlay: OverlayId) -> Apertura {
        if self.active == Some(overlay) {
            return Apertura::Aperto;
        }
        if let Some(modale) = self.active.and_then(OverlayId::modale) {
            if overlay == OverlayId::ConfermaChiusura {
                let precedente = self.active.replace(overlay);
                return precedente.map_or(Apertura::Aperto, Apertura::Sostituito);
            }
            if overlay.modale().is_none() {
                return Apertura::Rifiutato(Rifiuto {
                    modale,
                    silenzioso: false,
                });
            }
            if !self.attese.contains(&overlay) {
                self.attese.push_back(overlay);
            }
            return Apertura::InAttesa;
        }
        let precedente = self.active.replace(overlay);
        precedente.map_or(Apertura::Aperto, Apertura::Sostituito)
    }

    /// Ritira solo una vista in attesa, senza chiudere l'overlay attivo.
    pub fn revoca_attesa(&mut self, overlay: OverlayId) {
        self.attese.retain(|id| *id != overlay);
    }

    pub fn chiudi(&mut self) -> Chiusura {
        let chiuso = self.active.take();
        let promosso = if chiuso.is_some() {
            self.attese.pop_front()
        } else {
            None
        };
        self.active = promosso;
        Chiusura { chiuso, promosso }
    }

    pub fn mostra(&self, overlay: OverlayId) -> bool {
        self.active == Some(overlay)
    }

    pub fn chiudi_se(&mut self, overlay: OverlayId) -> bool {
        if self.mostra(overlay) {
            self.chiudi();
            true
        } else {
            false
        }
    }

    pub fn active(&self) -> Option<OverlayId> {
        self.active
    }
}
