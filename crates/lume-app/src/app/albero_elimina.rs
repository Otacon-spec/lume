//! Eliminare un file o una cartella dell'albero: la conferma e lo spostamento nel Cestino.
//!
//! Eliminare è l'azione che cambia l'identità di un file: se il file è
//! aperto in una scheda, il buffer continua a esistere mentre il file non c'è
//! più, e un salvataggio successivo lo ricreerebbe. Per questo prima si
//! chiede conferma, e un file aperto con modifiche non salvate non si elimina:
//! si dice perché, e il buffer resta.
//!
//! Il Cestino è la destinazione: `lume_core::trash_path` sposta il file dove
//! l'utente lo ritrova. Se il Cestino non è raggiungibile, non si cancella
//! niente: la cancellazione definitiva è un'altra azione, e non questa.

use std::path::{Path, PathBuf};

/// La conferma: cosa verrà eliminato e da dove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Richiesta {
    /// Il file da eliminare.
    pub percorso: PathBuf,
    /// Il nome mostrato all'utente: il percorso per intero è rumoroso.
    pub nome: String,
    /// La conferma comprende anche tutto il contenuto della cartella.
    pub cartella: bool,
    /// L'errore dell'ultimo tentativo, se c'è stato.
    pub errore: Option<String>,
    /// Lo spostamento è in corso: i tasti non agiscono più.
    pub in_corso: bool,
    /// Il permesso appartiene soltanto al worker avviato per questa richiesta.
    pub(crate) lease: Option<crate::scritture::Lease>,
    /// Le schede aperte sul file, calcolate **prima** dello spostamento.
    ///
    /// Dopo lo spostamento il file non esiste più e il confronto sul
    /// percorso canonico non basta più: senza questa lista una scheda
    /// aperta con un altro percorso resterebbe aperta, e un suo salvataggio
    /// ricreerebbe il file.
    pub schede: Vec<lume_core::DocId>,
}

/// Il file è dentro il progetto? Fuori, l'albero non è il filesystem.
///
/// Il confronto è canonico: una sottocartella symlink che punta fuori dal
/// progetto porta a un file che **non** è del progetto, anche se il suo
/// percorso scritto lo sembra.
pub fn dentro_progetto(radice: &Path, percorso: &Path) -> Result<(), String> {
    let fuori = || "il file non è dentro il progetto".to_owned();
    if percorso == radice || !percorso.starts_with(radice) {
        return Err(fuori());
    }
    if percorso
        .components()
        .any(|componente| componente.as_os_str() == "..")
    {
        return Err(fuori());
    }
    let radice_canonica = radice.canonicalize().map_err(|_| fuori())?;
    let cartella = percorso.parent().ok_or_else(fuori)?;
    let cartella_canonica = cartella.canonicalize().map_err(|_| fuori())?;
    if !cartella_canonica.starts_with(&radice_canonica) {
        return Err(fuori());
    }
    Ok(())
}

/// La conferma di un file non autorizza il contenuto di una directory che
/// un altro processo abbia creato nel frattempo sotto lo stesso nome.
fn verifica_tipo(percorso: &Path, cartella: bool) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(percorso).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink()
        || !(metadata.is_file() || metadata.is_dir())
        || metadata.is_dir() != cartella
    {
        return Err(
            "L’elemento è cambiato o è un link: chiudi la conferma e ripeti l’operazione".into(),
        );
    }
    Ok(())
}

/// Risolve anche un percorso il cui ultimo tratto non esiste più: gli
/// antenati esistenti conservano il legame degli alias con la cartella.
fn risolvi_antenati(percorso: &Path) -> Option<PathBuf> {
    for antenato in percorso.ancestors() {
        if let Ok(mut canonico) = antenato.canonicalize() {
            canonico.push(percorso.strip_prefix(antenato).ok()?);
            return Some(canonico);
        }
    }
    None
}

/// I documenti aperti che puntano a quel file, con le modifiche non salvate.
///
/// Il confronto è sul percorso canonico: un link simbolico o un percorso
/// diverso che punta allo stesso file sono lo stesso file.
pub fn aperti_sul_file(
    aperti: &[(lume_core::DocId, PathBuf)],
    percorso: &Path,
) -> Vec<lume_core::DocId> {
    let obiettivo = risolvi_antenati(percorso);
    let cartella = std::fs::symlink_metadata(percorso).is_ok_and(|m| m.is_dir());
    aperti
        .iter()
        .filter(|(_, aperto)| match (&obiettivo, risolvi_antenati(aperto)) {
            (Some(a), Some(b)) => {
                a == &b || (cartella && (b.starts_with(a) || aperto.starts_with(percorso)))
            }
            _ => aperto == percorso || (cartella && aperto.starts_with(percorso)),
        })
        .map(|(id, _)| *id)
        .collect()
}

/// Rifiuta l'eliminazione se il file è aperto con modifiche non salvate.
///
/// Il testo dice perché e cosa fare: perdere le modifiche non è un dettaglio
/// che si lascia all'utente di scoprire dopo.
pub fn rifiuta_se_sporco(
    docs: &[lume_core::DocId],
    sporchi: &dyn Fn(lume_core::DocId) -> bool,
) -> Result<(), String> {
    if docs.iter().any(|id| sporchi(*id)) {
        return Err(
            "un documento interessato ha modifiche non salvate: salvalo o chiudi il buffer prima di eliminare"
                .to_owned(),
        );
    }
    Ok(())
}

impl super::Lume {
    /// Apre la conferma di eliminazione per il file dell'albero.
    ///
    /// Fuori dal progetto la richiesta è rifiutata con avviso: l'albero non è
    /// il filesystem. Un file aperto con modifiche non salvate non si elimina:
    /// il buffer resterebbe senza file, e un salvataggio successivo lo
    /// ricreerebbe.
    pub(super) fn apri_elimina_albero(
        &mut self,
        soggetto: Option<&crate::context_menu::Soggetto>,
    ) -> super::Task<super::Message> {
        let Some(crate::context_menu::Soggetto::Albero { path, is_dir }) = soggetto else {
            return self.notify_error("Il comando non ha un file dell'albero".to_owned());
        };
        if self.scritture.guarda(None).in_volo.is_some() || self.git_operazione.is_some() {
            return self
                .notify_error("Una scrittura è in corso: attendi prima di eliminare".into());
        }
        let Some(radice) = self.project_root.clone() else {
            return self.notify_error("Nessun progetto aperto: non c'è cosa eliminare".to_owned());
        };
        if let Err(errore) = super::albero_elimina::dentro_progetto(&radice, path) {
            return self.notify_error(errore);
        }
        if let Err(errore) = verifica_tipo(path, *is_dir) {
            return self.notify_error(errore);
        }
        let aperti = self.replace_mapping();
        let sporchi = |id: lume_core::DocId| {
            self.chiede_salvataggio(id)
                || self
                    .restored_tab_commands
                    .get(&id)
                    .is_some_and(|q| !q.is_empty())
                || self
                    .restored_tab_actions
                    .get(&id)
                    .is_some_and(|q| !q.is_empty())
        };
        let sul_file = super::albero_elimina::aperti_sul_file(&aperti, path);
        if let Err(errore) = super::albero_elimina::rifiuta_se_sporco(&sul_file, &sporchi) {
            return self.notify_error(errore);
        }
        let nome = path
            .file_name()
            .map(|nome| nome.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.albero_elimina = Some(super::albero_elimina::Richiesta {
            percorso: path.clone(),
            nome,
            cartella: *is_dir,
            errore: None,
            in_corso: false,
            lease: None,
            schede: sul_file,
        });
        self.apri_overlay(super::OverlayId::ConfermaElimina);
        super::Task::none()
    }

    /// Lo spostamento è in corso: i pulsanti della richiesta non lo annullano.
    pub(super) fn albero_eliminazione_in_corso(&self) -> bool {
        self.albero_elimina
            .as_ref()
            .is_some_and(|richiesta| richiesta.in_corso)
    }

    /// Il riquadro di conferma, se è aperto.
    pub(crate) fn riquadro_elimina(&self) -> Option<crate::conferma_elimina::Riquadro> {
        self.albero_elimina
            .as_ref()
            .map(|richiesta| crate::conferma_elimina::Riquadro {
                nome: richiesta.nome.clone(),
                cartella: richiesta.cartella,
                errore: richiesta.errore.clone(),
                in_corso: richiesta.in_corso,
            })
    }

    /// La risposta alla conferma: si elimina, o si torna indietro.
    pub(super) fn apply_albero_elimina_message(
        &mut self,
        message: &crate::conferma_elimina::ConfermaMessage,
    ) -> super::Task<super::Message> {
        // La richiesta conservata dopo una sostituzione non autorizza una
        // risposta alla conferma invisibile. Un'operazione già avviata mantiene
        // invece l'avviso e la gestione del lease previsti dai rami sotto.
        if !self.shell.mostra(super::OverlayId::ConfermaElimina)
            && !self.albero_eliminazione_in_corso()
        {
            return self.notify(
                super::NoticeKind::Info,
                "La conferma del Cestino non è più visibile".into(),
            );
        }
        match message {
            crate::conferma_elimina::ConfermaMessage::Annulla => {
                if self.albero_elimina.as_ref().is_some_and(|r| r.in_corso) {
                    // Lo spostamento è già partito: annullare qui lascerebbe
                    // il file nel Cestino senza che nessuno lo sappia. Si
                    // aspetta l'esito.
                    return self.notify(
                        super::NoticeKind::Info,
                        "Spostamento nel Cestino già avviato: attendi l'esito".into(),
                    );
                }
                self.albero_elimina = None;
                self.chiudi_overlay_se(super::OverlayId::ConfermaElimina);
                super::Task::none()
            }
            crate::conferma_elimina::ConfermaMessage::Elimina => {
                if self.albero_elimina.as_ref().is_some_and(|r| r.in_corso) {
                    // Doppio clic: lo spostamento è già in corso.
                    return self.notify(
                        super::NoticeKind::Info,
                        "Spostamento nel Cestino già avviato: attendi l'esito".into(),
                    );
                }
                self.elimina_albero()
            }
        }
    }

    /// Sposta il file nel Cestino e chiude le schede che lo mostravano.
    ///
    /// Il lavoro è su disco e su un altro volume: va fuori dal ciclo della
    /// interfaccia, e il risultato torna come messaggio.
    fn elimina_albero(&mut self) -> super::Task<super::Message> {
        let Some(richiesta) = self.albero_elimina.clone() else {
            return super::Task::none();
        };
        if self.scritture.guarda(None).in_volo.is_some() || self.git_operazione.is_some() {
            let errore = "Una scrittura è in corso: attendi prima di eliminare".to_owned();
            if let Some(stato) = self.albero_elimina.as_mut() {
                stato.errore = Some(errore.clone());
            }
            return self.notify_error(errore);
        }
        let Some(radice) = self.project_root.clone() else {
            return self.notify_error("Nessun progetto aperto: non c'è cosa eliminare".to_owned());
        };
        if let Err(errore) = super::albero_elimina::dentro_progetto(&radice, &richiesta.percorso) {
            return self.notify_error(errore);
        }
        if let Err(errore) = verifica_tipo(&richiesta.percorso, richiesta.cartella) {
            if let Some(stato) = self.albero_elimina.as_mut() {
                stato.errore = Some(errore.clone());
            }
            return self.notify_error(errore);
        }
        let aperti = self.replace_mapping();
        let sporchi = |id: lume_core::DocId| {
            self.chiede_salvataggio(id)
                || self
                    .restored_tab_commands
                    .get(&id)
                    .is_some_and(|q| !q.is_empty())
                || self
                    .restored_tab_actions
                    .get(&id)
                    .is_some_and(|q| !q.is_empty())
        };
        let sul_file = super::albero_elimina::aperti_sul_file(&aperti, &richiesta.percorso);
        if let Err(errore) = super::albero_elimina::rifiuta_se_sporco(&sul_file, &sporchi) {
            // l'errore resta nel riquadro: la notifica sparisce, il riquadro no.
            if let Some(richiesta) = self.albero_elimina.as_mut() {
                richiesta.errore = Some(errore.clone());
            }
            return self.notify_error(errore);
        }
        let lease = match self.scritture.ammetti(crate::scritture::Op::Cestino {
            docs: sul_file.clone(),
        }) {
            Ok(crate::scritture::Permesso::Avvia(lease)) => lease,
            Err(rifiuto) => {
                let errore = rifiuto.avviso();
                if let Some(stato) = self.albero_elimina.as_mut() {
                    stato.errore = Some(errore.clone());
                }
                return self.notify_error(errore);
            }
            Ok(_) => return self.notify_error("Spostamento nel Cestino non avviato".into()),
        };
        // Le letture già preparate non possono aprire o ricaricare file
        // mentre la loro identità viene rimossa dall'albero.
        // Da qui in poi lo spostamento e' in corso: i tasti non agiscono
        // piu' e l'esito torna a questa richiesta, non a uno stato che
        // l'utente puo' azzerare nel frattempo.
        let Some(stato) = self.albero_elimina.as_mut() else {
            self.scritture
                .rilascia(lease, crate::scritture::Esito::NonAvviato);
            return self
                .notify_error("Spostamento nel Cestino non avviato: richiesta assente".into());
        };
        stato.in_corso = true;
        stato.errore = None;
        stato.lease = Some(lease);
        // Le schede sono quelle di adesso: una aperta fra la conferma e
        // il clic deve chiudersi con le altre.
        stato.schede = sul_file;
        let richiesta_per_il_messaggio = stato.clone();
        let percorso = richiesta.percorso.clone();
        let percorso_per_l_errore = percorso.clone();
        let cartella = richiesta.cartella;
        super::Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    verifica_tipo(&percorso, cartella).map_err(|errore| {
                        lume_core::TrashError::Io {
                            path: percorso.clone(),
                            source: std::io::Error::other(errore),
                        }
                    })?;
                    lume_core::trash_path(&percorso)
                })
                .await
                .unwrap_or_else(|errore| {
                    Err(lume_core::TrashError::Io {
                        path: percorso_per_l_errore,
                        source: std::io::Error::other(errore.to_string()),
                    })
                })
            },
            move |esito| super::Message::AlberoEliminato(richiesta_per_il_messaggio, esito),
        )
    }

    /// Il riquadro torna aperto con l'errore, o l'overlay si chiude.
    ///
    /// Senza questo, un errore lasciava l'overlay attivo ma senza riquadro da
    /// disegnare: la notifica in barra di stato sparisce dopo qualche secondo
    /// e l'errore andava con lei.
    fn riporta_errore_elimina(
        &mut self,
        mut richiesta: super::albero_elimina::Richiesta,
        errore: String,
    ) {
        richiesta.errore = Some(errore);
        richiesta.in_corso = false;
        richiesta.lease = None;
        self.albero_elimina = Some(richiesta);
        // La chiusura in sospeso conserva intenzione ed errore; altrimenti
        // si riapre la conferma se era stata nascosta con Esc.
        if !self.shell.mostra(super::OverlayId::ConfermaElimina) {
            self.apri_overlay(super::OverlayId::ConfermaElimina);
        }
    }

    /// Il file è nel Cestino: le schede che lo mostravano chiudono.
    ///
    /// Le schede sono quelle calcolate **prima** dello spostamento: adesso il
    /// file non esiste più e il confronto sul percorso canonico non basta più.
    pub(super) fn albero_eliminato(
        &mut self,
        richiesta: super::albero_elimina::Richiesta,
        esito: Result<lume_core::TrashOutcome, lume_core::TrashError>,
    ) -> super::Task<super::Message> {
        let Some(lease) = richiesta.lease else {
            return super::Task::none();
        };
        if !richiesta.in_corso
            || self.albero_elimina.as_ref() != Some(&richiesta)
            || !self
                .scritture
                .possiede(lease, crate::scritture::Tipo::Cestino)
        {
            return super::Task::none();
        }
        let esito_lease = match &esito {
            Ok(lume_core::TrashOutcome::Trashed) => crate::scritture::Esito::Riuscito,
            _ => crate::scritture::Esito::Fallito,
        };
        if matches!(&esito, Ok(lume_core::TrashOutcome::Trashed)) {
            for doc in &richiesta.schede {
                self.scritture.revoca_salvataggi(Some(*doc));
            }
        }
        self.scritture.rilascia(lease, esito_lease);
        let Some(stato) = self.albero_elimina.take() else {
            return self
                .notify_error("Risultato del Cestino senza richiesta: controlla i file".into());
        };
        match esito {
            Ok(lume_core::TrashOutcome::Trashed) => {
                self.revoca_attesa_overlay(super::OverlayId::ConfermaElimina);
                self.chiudi_overlay_se(super::OverlayId::ConfermaElimina);
                // le schede che mostravano il file non possono più
                // salvarlo: un salvataggio lo ricreerebbe.
                for doc in &richiesta.schede {
                    self.close_tab(*doc);
                }
                // l'albero si aggiorna, e con lui lo scope del watcher
                let cartella = richiesta
                    .percorso
                    .parent()
                    .map(|cartella| cartella.to_path_buf())
                    .unwrap_or_default();
                self.refresh_project_tree(std::slice::from_ref(&cartella));
                self.notify(
                    super::NoticeKind::Info,
                    format!("Spostato nel Cestino {}", richiesta.percorso.display()),
                )
            }
            Ok(lume_core::TrashOutcome::NeedsPermanentConfirmation) => {
                let errore =
                    "Il Cestino non è disponibile: nessun elemento è stato eliminato".to_owned();
                if let Some(chiusura) = self.chiusura.as_mut() {
                    chiusura.errore = Some(errore.clone());
                    self.close_after_save = None;
                }
                self.riporta_errore_elimina(stato, errore.clone());
                self.notify_error(errore)
            }
            Err(errore) => {
                let testo = errore.to_string();
                if let Some(chiusura) = self.chiusura.as_mut() {
                    chiusura.errore = Some(testo.clone());
                    self.close_after_save = None;
                }
                self.riporta_errore_elimina(stato, testo.clone());
                self.notify_error(testo)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn fuori_progetto_l_eliminazione_e_rifiutata() {
        let radice = tempfile::tempdir().unwrap();
        let fuori = tempfile::tempdir().unwrap();
        let errore = dentro_progetto(radice.path(), &fuori.path().join("a.txt")).unwrap_err();
        assert!(errore.contains("non è dentro il progetto"), "{errore}");
    }

    #[test]
    fn dentro_progetto_l_eliminazione_e_ammessa() {
        let radice = tempfile::tempdir().unwrap();
        // il confronto e' canonico: la cartella deve esistere davvero.
        std::fs::create_dir(radice.path().join("src")).unwrap();
        let dentro = dentro_progetto(radice.path(), &radice.path().join("src/a.txt"));
        assert!(dentro.is_ok());
    }

    // Su Windows un symlink richiede privilegi: la prova resta Unix.
    #[cfg(unix)]
    #[test]
    fn una_cartella_symlink_che_esce_dal_progetto_non_e_ammessa() {
        let radice = tempfile::tempdir().unwrap();
        let fuori = tempfile::tempdir().unwrap();
        std::fs::write(fuori.path().join("segreto.txt"), b"segreto").unwrap();
        // la cartella dentro il progetto e' un link a quella fuori
        let link = radice.path().join("scorciatoia");
        std::os::unix::fs::symlink(fuori.path(), &link).unwrap();
        let errore = dentro_progetto(radice.path(), &link.join("segreto.txt")).unwrap_err();
        assert!(errore.contains("non è dentro il progetto"), "{errore}");
    }

    #[test]
    fn un_file_sporco_non_si_elimina() {
        let sporchi = |id: lume_core::DocId| id.0 == 7;
        let errore = rifiuta_se_sporco(&[lume_core::DocId(7)], &sporchi).unwrap_err();
        assert!(errore.contains("modifiche non salvate"), "{errore}");
    }

    #[test]
    fn un_file_pulito_si_elimina() {
        let sporchi = |_id: lume_core::DocId| false;
        assert!(rifiuta_se_sporco(&[lume_core::DocId(7)], &sporchi).is_ok());
    }

    #[test]
    fn il_confronto_e_sul_file_canonico() {
        let cartella = tempfile::tempdir().unwrap();
        let file = cartella.path().join("a.txt");
        std::fs::write(&file, b"contenuto").unwrap();
        // lo stesso file raggiunto con un percorso diverso
        let altro = cartella.path().join(".").join("a.txt");
        let aperti = vec![(lume_core::DocId(1), altro)];
        let trovati = aperti_sul_file(&aperti, &file);
        assert_eq!(trovati, vec![lume_core::DocId(1)]);
    }
}
