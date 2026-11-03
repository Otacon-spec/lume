//! Operazioni sull'albero di progetto: creare un file o una cartella.
//!
//! Creare è il caso semplice: un nome nuovo non ha buffer sporchi, né
//! schede aperte, né URI LSP da riallineare. Rinominare ed eliminare non
//! sono qui: cambiano l'identità di un file che può essere aperto e meritano
//! un passo dedicato (Cestino, conferma, riallineamento delle URI).
//!
//! Il nome che l'utente scrive non può uscire dalla cartella scelta: niente
//! separatori, niente «..», niente nomi vuoti.

use std::path::{Path, PathBuf};

/// Cosa chiede l'utente all'albero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Azione {
    NuovoFile,
    NuovaCartella,
}

impl Azione {
    /// Il titolo del riquadro.
    pub fn titolo(self) -> &'static str {
        match self {
            Azione::NuovoFile => "Nuovo file",
            Azione::NuovaCartella => "Nuova cartella",
        }
    }

    /// Il testo del campo: che cosa ci si scrive.
    pub fn segnaposto(self) -> &'static str {
        match self {
            Azione::NuovoFile => "nome del file",
            Azione::NuovaCartella => "nome della cartella",
        }
    }
}

/// Il riquadro aperto: l'azione, la cartella e il nome scritto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Richiesta {
    pub azione: Azione,
    pub cartella: PathBuf,
    pub valore: String,
    /// L'errore dell'ultimo tentativo: resta nel riquadro finché il nome non
    /// cambia, perché la notifica in barra di stato sparisce da sola.
    pub errore: Option<String>,
}

impl Richiesta {
    /// Il riquadro, nella forma che il prompt dell'app sa disegnare.
    pub fn prompt(&self) -> crate::prompt::Prompt<'_> {
        crate::prompt::Prompt {
            title: self.azione.titolo().to_owned(),
            placeholder: self.azione.segnaposto(),
            value: &self.valore,
            hint: self
                .errore
                .clone()
                .unwrap_or_else(|| format!("in {}", self.cartella.display())),
            ready: self.errore.is_none() && nome_valido(&self.valore).is_ok(),
            confirm_label: "Crea",
        }
    }
}

/// Il nome è accettato?
///
/// Un nome non vuoto, senza separatori di percorso e senza «..»: il file
/// nasce **nella** cartella scelta, non fuori.
pub fn nome_valido(nome: &str) -> Result<(), String> {
    if nome.trim().is_empty() {
        return Err("il nome non può essere vuoto".to_owned());
    }
    if nome.contains('/') || nome.contains('\\') {
        return Err("il nome non può contenere separatori di percorso".to_owned());
    }
    if nome.trim() == "." || nome.trim() == ".." {
        return Err("il nome non può essere «.» o «..»".to_owned());
    }
    Ok(())
}

/// Crea il file o la cartella dentro `cartella`.
///
/// `radice` è la root del progetto: fuori da lì la richiesta è rifiutata,
/// perché l'albero non è il filesystem. Un nome già preso è un errore, non
/// una sovrascrittura.
pub fn crea(radice: &Path, cartella: &Path, azione: Azione, nome: &str) -> Result<PathBuf, String> {
    nome_valido(nome)?;
    let nome = nome.trim();
    if !cartella.starts_with(radice) {
        return Err("la cartella non è dentro il progetto".to_owned());
    }
    let percorso = cartella.join(nome);
    let esito = match azione {
        Azione::NuovoFile => lume_core::file_write::crea_file(&percorso),
        Azione::NuovaCartella => lume_core::file_write::crea_cartella(&percorso),
    };
    esito.map_err(|errore| match errore.kind() {
        std::io::ErrorKind::AlreadyExists => format!("«{nome}» esiste già"),
        std::io::ErrorKind::PermissionDenied => format!("«{nome}»: permesso negato"),
        _ => format!("«{nome}»: {errore}"),
    })?;
    Ok(percorso)
}

impl super::Lume {
    /// Apre il riquadro per la cartella del soggetto: una riga dell'albero.
    pub(super) fn apri_albero_azione(
        &mut self,
        azione: Azione,
        soggetto: Option<&crate::context_menu::Soggetto>,
    ) -> super::Task<super::Message> {
        let Some(crate::context_menu::Soggetto::Albero { path, is_dir }) = soggetto else {
            return self.notify_error("Il comando non ha una cartella dell'albero".to_owned());
        };
        if !*is_dir {
            return self.notify_error("Il comando vale su una cartella".to_owned());
        }
        self.open_albero_azione(azione, path.clone())
    }

    /// Apre il riquadro «Nuovo file»/«Nuova cartella» nella cartella scelta.
    ///
    /// Fuori dal progetto la richiesta è rifiutata con un avviso: l'albero
    /// non è il filesystem.
    pub(super) fn open_albero_azione(
        &mut self,
        azione: Azione,
        cartella: PathBuf,
    ) -> super::Task<super::Message> {
        let Some(radice) = self.project_root.clone() else {
            return self.notify_error("Nessun progetto aperto: non c'è dove creare".to_owned());
        };
        if !cartella.starts_with(&radice) {
            return self.notify_error("La cartella non è dentro il progetto".to_owned());
        }
        self.albero = Some(Richiesta {
            azione,
            cartella,
            valore: String::new(),
            errore: None,
        });
        self.apri_overlay(super::OverlayId::Albero);
        iced::widget::operation::focus(iced::widget::Id::new(crate::prompt::INPUT_ID))
    }

    /// Il riquadro aperto, se c'è.
    pub(crate) fn albero_prompt(&self) -> Option<crate::prompt::Prompt<'_>> {
        self.albero.as_ref().map(|richiesta| richiesta.prompt())
    }

    /// La risposta al riquadro: il nome cambia, si annulla, o si conferma.
    pub(super) fn apply_albero_message(
        &mut self,
        message: &crate::prompt::PromptMessage,
    ) -> super::Task<super::Message> {
        use crate::prompt::PromptMessage;
        match message {
            PromptMessage::Changed(testo) => {
                if let Some(richiesta) = self.albero.as_mut() {
                    richiesta.valore = testo.clone();
                    richiesta.errore = None;
                }
                super::Task::none()
            }
            PromptMessage::Cancel => {
                self.albero = None;
                self.chiudi_overlay_se(super::OverlayId::Albero);
                super::Task::none()
            }
            PromptMessage::Confirm => {
                let Some(richiesta) = self.albero.clone() else {
                    return super::Task::none();
                };
                let Some(radice) = self.project_root.clone() else {
                    // l'errore resta anche nel riquadro: la notifica in
                    // barra di stato sparisce, il riquadro no.
                    let errore = "Nessun progetto aperto: non c'è dove creare".to_owned();
                    if let Some(richiesta) = self.albero.as_mut() {
                        richiesta.errore = Some(errore.clone());
                    }
                    return self.notify_error(errore);
                };
                match crea(
                    &radice,
                    &richiesta.cartella,
                    richiesta.azione,
                    &richiesta.valore,
                ) {
                    Ok(percorso) => {
                        self.albero = None;
                        self.chiudi_overlay_se(super::OverlayId::Albero);
                        // l'albero si aggiorna, e con lui lo scope del watcher
                        self.refresh_project_tree(std::slice::from_ref(&richiesta.cartella));
                        self.notify(
                            super::NoticeKind::Info,
                            format!("Creato {}", percorso.display()),
                        )
                    }
                    Err(errore) => {
                        // l'errore resta nel riquadro: la notifica sparisce, il
                        // riquadro no.
                        if let Some(richiesta) = self.albero.as_mut() {
                            richiesta.errore = Some(errore.clone());
                        }
                        self.notify_error(errore)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn il_nome_non_puo_uscire_dalla_cartella() {
        assert!(nome_valido("prova.txt").is_ok());
        assert!(nome_valido("").is_err());
        assert!(nome_valido("   ").is_err());
        assert!(nome_valido("a/b").is_err());
        assert!(nome_valido("a\\b").is_err());
        assert!(nome_valido("..").is_err());
        assert!(nome_valido(".").is_err());
    }

    #[test]
    fn crea_il_file_nella_cartella_e_non_fuori() {
        let radice = tempfile::tempdir().unwrap();
        let cartella = radice.path().join("src");
        std::fs::create_dir(&cartella).unwrap();

        let percorso = crea(radice.path(), &cartella, Azione::NuovoFile, "nuovo.txt").unwrap();
        assert!(percorso.is_file());
        assert_eq!(percorso, cartella.join("nuovo.txt"));

        // fuori dalla root: rifiutato, e niente scritto.
        let fuori = tempfile::tempdir().unwrap();
        let errore = crea(radice.path(), fuori.path(), Azione::NuovoFile, "prova.txt").unwrap_err();
        assert!(errore.contains("non è dentro il progetto"), "{errore}");
        assert!(!fuori.path().join("prova.txt").exists());
    }

    #[test]
    fn un_nome_gia_presente_non_e_una_sovrascrittura() {
        let radice = tempfile::tempdir().unwrap();
        let percorso = radice.path().join("gia.txt");
        std::fs::write(&percorso, "contenuto").unwrap();

        let errore = crea(radice.path(), radice.path(), Azione::NuovoFile, "gia.txt").unwrap_err();
        assert!(errore.contains("esiste già"), "{errore}");
        assert_eq!(std::fs::read(&percorso).unwrap(), b"contenuto");
    }

    #[test]
    fn crea_anche_una_cartella() {
        let radice = tempfile::tempdir().unwrap();
        let percorso = crea(
            radice.path(),
            radice.path(),
            Azione::NuovaCartella,
            "moduli",
        )
        .unwrap();
        assert!(percorso.is_dir());
    }
}
