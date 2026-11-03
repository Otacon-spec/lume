//! Percorso unico di apertura, chiusura e rivalidazione delle viste.
use super::*;
use crate::overlay_manager::{Apertura, Chiusura, Ingresso, OverlayManager, Rifiuto};

#[derive(Debug, Default)]
pub struct AppShell {
    overlays: OverlayManager,
}

impl AppShell {
    pub(super) fn ammetti(&self, ingresso: Ingresso) -> Result<(), Rifiuto> {
        self.overlays.ammetti(ingresso)
    }

    pub fn mostra(&self, overlay: OverlayId) -> bool {
        self.overlays.mostra(overlay)
    }

    pub fn active_overlay(&self) -> Option<OverlayId> {
        self.overlays.active()
    }
}

impl Lume {
    pub(super) fn apri_overlay(&mut self, overlay: OverlayId) {
        if self.shell.mostra(overlay) {
            return;
        }
        if self.uscita_in_corso && overlay.modale().is_some() {
            self.notify_persistente(
                NoticeKind::Info,
                "Chiusura in corso: ripeti l'azione se l'uscita non riesce".into(),
            );
            return;
        }
        let apertura = self.shell.overlays.open(overlay);
        match apertura {
            Apertura::Aperto | Apertura::Sostituito(_) => {
                if overlay.modale().is_some() {
                    self.layout.drag = None;
                    self.activity_bar.annulla_trascinamento();
                    self.editor_tab_bar.annulla_trascinamento();
                }
                if let Apertura::Sostituito(precedente) = apertura {
                    match precedente {
                        OverlayId::Search => {
                            if self.global_search.is_open() {
                                self.suspend_global_file_search();
                            }
                            self.reconcile_file_palette();
                        }
                        OverlayId::Find => self.find = None,
                        OverlayId::ConfermaElimina if !self.albero_eliminazione_in_corso() => {
                            self.albero_elimina = None;
                        }
                        #[cfg(feature = "mcp")]
                        OverlayId::ConsensoMcp => {
                            self.mcp_dimentica_consenso();
                            self.notify_persistente(
                                NoticeKind::Info,
                                "Consenso MCP rifiutato dalla richiesta di chiusura".into(),
                            );
                        }
                        _ => {}
                    }
                }
            }
            Apertura::InAttesa => {}
            Apertura::Rifiutato(rifiuto) => {
                if overlay == OverlayId::Find {
                    self.find = None;
                }
                if let Some(testo) = rifiuto.avviso() {
                    self.notify_persistente(NoticeKind::Info, testo);
                }
            }
        }
    }

    /// Revoca di dominio: non equivale a chiudere un riquadro visibile.
    pub(super) fn revoca_attesa_overlay(&mut self, overlay: OverlayId) {
        self.shell.overlays.revoca_attesa(overlay);
    }

    pub(super) fn invalida_attese_uscita(&mut self) {
        for id in [
            OverlayId::About,
            OverlayId::Scorciatoie,
            OverlayId::ConfermaElimina,
            OverlayId::ConfermaChiusura,
            #[cfg(feature = "mcp")]
            OverlayId::ConsensoMcp,
            #[cfg(feature = "mcp")]
            OverlayId::CardMcp,
        ] {
            self.revoca_attesa_overlay(id);
        }
    }

    fn rivalida_attese_overlay(&mut self) {
        if self.search_exit.is_some() {
            self.invalida_attese_uscita();
            return;
        }
        if self.albero_elimina.is_none() {
            self.revoca_attesa_overlay(OverlayId::ConfermaElimina);
        }
        if self.chiusura.is_none() {
            self.revoca_attesa_overlay(OverlayId::ConfermaChiusura);
        }
        #[cfg(feature = "mcp")]
        {
            if self.mcp_consenso.is_none() {
                self.revoca_attesa_overlay(OverlayId::ConsensoMcp);
            }
            if self.mcp_card_view().is_none() {
                self.revoca_attesa_overlay(OverlayId::CardMcp);
            }
        }
    }

    pub(super) fn chiudi_overlay(&mut self) -> Chiusura {
        self.rivalida_attese_overlay();
        self.shell.overlays.chiudi()
    }

    pub(super) fn chiudi_overlay_se(&mut self, overlay: OverlayId) -> bool {
        if !self.shell.mostra(overlay) {
            return false;
        }
        self.rivalida_attese_overlay();
        self.shell.overlays.chiudi_se(overlay)
    }
}
