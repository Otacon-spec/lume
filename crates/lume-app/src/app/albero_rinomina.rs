//! Rinominare un file o una cartella dell'albero di progetto.
//!
//! Rinominare cambia l'identità di un file che può essere **aperto**: il
//! buffer, la scheda e la URI del language server devono seguire il nuovo
//! nome, altrimenti il prossimo salvataggio ricreerebbe il file vecchio.
//!
//! Per questo la rinomina è un passo dedicato e non una voce accanto a
//! «Crea»: qui si rinomina su disco, si riallineano i documenti aperti e si
//! avvisa il server, e ogni fallimento dice che cosa è successo.

use std::path::{Path, PathBuf};

/// Il riquadro aperto: che cosa si rinomina e con quale nome nuovo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Richiesta {
    /// Il file o la cartella da rinominare.
    pub percorso: PathBuf,
    /// Il nome attuale: il campo parte da qui, e il nome nuovo deve essere
    /// diverso.
    pub nome_attuale: String,
    /// Il nome nuovo scritto dall'utente.
    pub valore: String,
    /// L'errore dell'ultimo tentativo: resta nel riquadro finché il nome non
    /// cambia, perché la notifica in barra di stato sparisce da sola.
    pub errore: Option<String>,
}

impl Richiesta {
    /// Il riquadro, nella forma che il prompt dell'app sa disegnare.
    pub fn prompt(&self) -> crate::prompt::Prompt<'_> {
        crate::prompt::Prompt {
            title: format!("Rinomina «{}»", self.nome_attuale),
            placeholder: "nuovo nome",
            value: &self.valore,
            hint: self
                .errore
                .clone()
                .unwrap_or_else(|| format!("in {}", self.cartella().display())),
            ready: self.errore.is_none() && nome_diverso(&self.valore, &self.nome_attuale).is_ok(),
            confirm_label: "Rinomina",
        }
    }

    /// La cartella che contiene il file rinominato.
    pub fn cartella(&self) -> PathBuf {
        self.percorso
            .parent()
            .map(|cartella| cartella.to_path_buf())
            .unwrap_or_default()
    }
}

/// Il nome nuovo è accettato?
///
/// Le stesse regole della creazione — un nome solo, senza separatori e senza
/// «..» — e in più **diverso dall'attuale**: rinominare un file in se stesso
/// non è un'operazione, è un errore da non fare.
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

/// Il nome nuovo è diverso dall'attuale?
pub fn nome_diverso(nome: &str, attuale: &str) -> Result<(), String> {
    nome_valido(nome)?;
    if nome.trim() == attuale.trim() {
        return Err("il nome nuovo deve essere diverso da quello attuale".to_owned());
    }
    Ok(())
}

/// Rinomina il file o la cartella dentro `radice`.
///
/// La rinomina è **sulla stessa cartella**: il nome nuovo non può cambiare il
/// percorso. Un nome già preso è un errore, non una sovrascrittura.
///
/// Riuscita la rinomina, la cartella viene sincronizzata sul disco: il nome
/// nuovo sopravvive a una perdita di corrente. Un errore di sincronizzazione
/// non annulla la rinomina, già avvenuta, e torna come avviso.
pub fn rinomina(
    radice: &Path,
    percorso: &Path,
    nome: &str,
) -> Result<(PathBuf, Option<String>), String> {
    let attuale = percorso
        .file_name()
        .map(|nome| nome.to_string_lossy().into_owned())
        .unwrap_or_default();
    nome_diverso(nome, &attuale)?;
    if !lume_core::path_is_within(radice, percorso) {
        return Err("il file non è dentro il progetto".to_owned());
    }
    let cartella = percorso.parent().ok_or("il percorso non ha una cartella")?;
    let nuovo = nome.trim();
    let destinazione = cartella.join(nuovo);
    if destinazione == percorso {
        return Err("il nome nuovo deve essere diverso da quello attuale".to_owned());
    }
    // Il solo cambio di maiuscole (a.txt -> A.txt) su un filesystem che non
    // distingue maiuscole: il destinatario "esiste gia'" per il filesystem ma
    // e' lo stesso file, e rinominarlo e' lecito.
    let solo_maiuscole = !destinazione.exists()
        && percorso
            .file_name()
            .map(|nome| nome.to_string_lossy().eq_ignore_ascii_case(nuovo))
            .unwrap_or(false)
        && cartella
            .join(nuovo)
            .metadata()
            .map(|meta| meta.is_file())
            .unwrap_or(false);
    if solo_maiuscole {
        return std::fs::rename(percorso, &destinazione)
            .map(|()| sincronizza_cartella(cartella, destinazione))
            .map_err(|errore| format!("«{nuovo}»: {errore}"));
    }
    if destinazione.exists() {
        return Err(format!("«{nuovo}» esiste già"));
    }
    // `rename` su Linux **sovrascrive** il destinatario: il controllo qui
    // sopra lascia la finestra in cui un altro processo crea il file. Il link
    // duro non sovrascrive e fallisce con AlreadyExists, quindi la rinomina
    // resta atomica e senza sovrascritture; l'originale viene poi rimosso.
    match std::fs::hard_link(percorso, &destinazione) {
        Ok(()) => {}
        Err(errore) if errore.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(format!("«{nuovo}» esiste già"));
        }
        Err(errore) => {
            return Err(format!("«{nuovo}»: {errore}"));
        }
    }
    if let Err(errore) = std::fs::remove_file(percorso) {
        // il destinatario c'è già: si rimuove, per non lasciare due nomi
        // per lo stesso file.
        let _ = std::fs::remove_file(&destinazione);
        return Err(format!("«{nuovo}»: {errore}"));
    }
    Ok(sincronizza_cartella(cartella, destinazione))
}

/// Il testo per la barra di stato dopo una rinomina riuscita, se c'è
/// qualcosa da dire: la cartella non sincronizzata, i documenti non
/// riallineati.
fn avviso_rinomina(
    percorso: &Path,
    avviso_cartella: Option<String>,
    non_riallineati: &[String],
) -> Option<String> {
    let mut avvisi: Vec<String> = avviso_cartella.into_iter().collect();
    if !non_riallineati.is_empty() {
        avvisi.push(format!(
            "questi documenti non sono stati riallineati: {}",
            non_riallineati.join(", ")
        ));
    }
    (!avvisi.is_empty()).then(|| {
        format!(
            "«{}» rinominato, ma {}",
            percorso.display(),
            avvisi.join("; ")
        )
    })
}

fn sincronizza_cartella(cartella: &Path, destinazione: PathBuf) -> (PathBuf, Option<String>) {
    let avviso = lume_core::file_write::sync_directory(cartella)
        .map(|errore| format!("la cartella non è stata sincronizzata sul disco: {errore}"));
    (destinazione, avviso)
}

impl super::Lume {
    /// Apre il riquadro di rinomina per il file dell'albero.
    ///
    /// Fuori dal progetto la richiesta è rifiutata con avviso: l'albero non è
    /// il filesystem. Il campo parte dal nome attuale, e il pulsante è attivo
    /// solo con un nome **diverso**: rinominare un file in sé stesso non è
    /// un'operazione.
    pub(super) fn apri_rinomina_albero(
        &mut self,
        soggetto: Option<&crate::context_menu::Soggetto>,
    ) -> super::Task<super::Message> {
        let Some(crate::context_menu::Soggetto::Albero { path, is_dir }) = soggetto else {
            return self.notify_error("Il comando non ha un file dell'albero".to_owned());
        };
        if *is_dir {
            return self.notify_error("Il comando vale su un file".to_owned());
        }
        let Some(radice) = self.project_root.clone() else {
            return self.notify_error("Nessun progetto aperto: non c'è cosa rinominare".to_owned());
        };
        if !lume_core::path_is_within(&radice, path) {
            return self.notify_error("Il file non è dentro il progetto".to_owned());
        }
        let Some(nome_attuale) = path
            .file_name()
            .map(|nome| nome.to_string_lossy().into_owned())
        else {
            return self.notify_error("Il percorso non ha un nome".to_owned());
        };
        // I due riquadri dell'albero non possono stare aperti insieme: quello
        // di rinomina ha la precedenza nel disegno, e senza questo la
        // creazione resterebbe nascosta.
        self.albero = None;
        self.albero_rinomina = Some(super::albero_rinomina::Richiesta {
            percorso: path.clone(),
            nome_attuale: nome_attuale.clone(),
            valore: nome_attuale,
            errore: None,
        });
        self.apri_overlay(super::OverlayId::Albero);
        iced::widget::operation::focus(iced::widget::Id::new(crate::prompt::INPUT_ID))
    }

    /// Il riquadro di rinomina, se è aperto.
    pub(crate) fn albero_rinomina_prompt(&self) -> Option<crate::prompt::Prompt<'_>> {
        self.albero_rinomina
            .as_ref()
            .map(|richiesta| richiesta.prompt())
    }

    /// La risposta al riquadro: il nome cambia, si annulla, o si conferma.
    pub(super) fn apply_albero_rinomina_message(
        &mut self,
        message: &crate::prompt::PromptMessage,
    ) -> super::Task<super::Message> {
        use crate::prompt::PromptMessage;
        match message {
            PromptMessage::Changed(testo) => {
                if let Some(richiesta) = self.albero_rinomina.as_mut() {
                    richiesta.valore = testo.clone();
                    richiesta.errore = None;
                }
                super::Task::none()
            }
            PromptMessage::Cancel => {
                self.albero_rinomina = None;
                self.chiudi_overlay_se(super::OverlayId::Albero);
                super::Task::none()
            }
            PromptMessage::Confirm => self.rinomina_albero(),
        }
    }

    /// Rinomina il file su disco e riallinea chi lo aveva aperto.
    ///
    /// I documenti aperti sul vecchio percorso vengono legati al nuovo: senza
    /// questo, il prossimo salvataggio ricreerebbe il file vecchio. Al server
    /// LSP la URI vecchia viene chiusa e la nuova aperta, perché Lume non
    /// invia `workspace/didRenameFiles`.
    fn rinomina_albero(&mut self) -> super::Task<super::Message> {
        let Some(richiesta) = self.albero_rinomina.clone() else {
            return super::Task::none();
        };
        let Some(radice) = self.project_root.clone() else {
            return self.notify_error("Nessun progetto aperto: non c'è cosa rinominare".to_owned());
        };
        let percorso = richiesta.percorso.clone();
        let nome = richiesta.valore.clone();
        // I documenti da riallineare sono identificati **prima** della
        // rinomina: dopo, il vecchio percorso non esiste più e il confronto
        // canonico vale None, che abbina anche documenti estranei il cui file
        // non c'è più sul disco. Qui il confronto è sul Path del modello,
        // come fa l'indice del workspace.
        let sul_file: Vec<lume_core::DocId> = self
            .replace_mapping()
            .iter()
            .filter(|(_, aperto)| *aperto == percorso)
            .map(|(id, _)| *id)
            .collect();
        let preflight = super::albero_rinomina::nome_diverso(&nome, &richiesta.nome_attuale)
            .and_then(|()| {
                if !lume_core::path_is_within(&radice, &percorso) {
                    Err("il file non è dentro il progetto".to_owned())
                } else if percorso.parent().is_none() || percorso.file_name().is_none() {
                    Err("il percorso non ha una cartella o un nome".to_owned())
                } else {
                    Ok(())
                }
            });
        if let Err(errore) = preflight {
            if let Some(richiesta) = self.albero_rinomina.as_mut() {
                richiesta.errore = Some(errore.clone());
            }
            return self.notify_error(errore);
        }
        if self.git_operazione.is_some() {
            return self.notify_error(
                crate::scritture::Rifiuto {
                    in_volo: crate::scritture::Tipo::Git,
                    richiesto: crate::scritture::Tipo::Rinomina,
                    rimandato: false,
                }
                .avviso(),
            );
        }
        match self.scritture.ammetti(crate::scritture::Op::Rinomina {
            docs: sul_file.clone(),
        }) {
            Ok(crate::scritture::Permesso::Subito) => {}
            Err(rifiuto) => return self.notify_error(rifiuto.avviso()),
            Ok(_) => return self.notify_error("Rinomina non ammessa dal coordinatore".to_owned()),
        }
        let (destinazione, avviso_cartella) =
            match super::albero_rinomina::rinomina(&radice, &percorso, &nome) {
                Ok(esito) => esito,
                Err(errore) => {
                    // l'errore resta nel riquadro: la notifica sparisce, il
                    // riquadro no.
                    if let Some(richiesta) = self.albero_rinomina.as_mut() {
                        richiesta.errore = Some(errore.clone());
                    }
                    return self.notify_error(errore);
                }
            };

        // I documenti aperti sul vecchio percorso seguono il nuovo nome.
        //
        // Il rebind puo' fallire (percorso gia' preso da un altro
        // documento): in quel caso il documento resta sul vecchio percorso e
        // la sua URI e' gia' stata chiusa. Si raccoglie l'errore e si va
        // avanti con gli altri documenti: uscire qui lascerebbe meta'
        // riallineati, l'albero non aggiornato e la sessione non persistita.
        let mut reale = Vec::new();
        for id in sul_file {
            // Chiude la URI vecchia e riapre quella nuova sul server.
            self.lsp_did_close(id);
            match self.workspace.rebind_path(id, destinazione.clone()) {
                Ok(()) => {
                    // L'impronta del file era quella del percorso vecchio:
                    // senza aggiornarla il salvataggio successivo sarebbe
                    // rifiutato («impronta non disponibile») e il testo non
                    // arriverebbe mai sul nome nuovo.
                    if let Ok(Some(impronta)) =
                        lume_core::file_write::fingerprint_if_regular(&destinazione)
                    {
                        self.disk_fingerprints.insert(id, impronta);
                    }
                    self.lsp_did_open(id)
                }
                Err(errore) => reale.push(format!("«{}»: {errore}", self.nome_documento(id))),
            }
        }
        self.albero_rinomina = None;
        self.chiudi_overlay_se(super::OverlayId::Albero);
        // l'albero si aggiorna, e con lui lo scope del watcher
        let cartella = richiesta.cartella();
        self.refresh_project_tree(std::slice::from_ref(&cartella));
        let sessione = self.persist_session();
        // Il riquadro e' gia' chiuso: l'avviso va in barra di stato, e
        // dice che cosa non e' stato riallineato o sincronizzato.
        let Some(messaggio) = avviso_rinomina(&percorso, avviso_cartella, &reale) else {
            return sessione;
        };
        super::Task::batch([sessione, self.notify_error(messaggio)])
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn il_nome_nuovo_deve_essere_diverso_dall_attuale() {
        let errore = nome_diverso("prova.txt", "prova.txt").unwrap_err();
        assert!(errore.contains("diverso"), "{errore}");
    }

    #[test]
    fn il_nome_nuovo_non_puo_uscire_dalla_cartella() {
        assert!(nome_valido("a/b").is_err());
        assert!(nome_valido("a\\b").is_err());
        assert!(nome_valido("..").is_err());
        assert!(nome_valido("").is_err());
        assert!(nome_valido("prova.txt").is_ok());
    }

    #[test]
    fn rinominare_su_disco_il_file_nel_progetto() {
        let radice = tempfile::tempdir().unwrap();
        let file = radice.path().join("prima.txt");
        std::fs::write(&file, b"contenuto").unwrap();

        let (destinazione, avviso) = rinomina(radice.path(), &file, "dopo.txt").unwrap();

        assert_eq!(destinazione, radice.path().join("dopo.txt"));
        assert!(avviso.is_none(), "{avviso:?}");
        assert!(!file.exists(), "il nome vecchio non esiste piu'");
        assert_eq!(std::fs::read(&destinazione).unwrap(), b"contenuto");
    }

    #[test]
    fn un_nome_gia_presente_non_e_sovrapposto() {
        let radice = tempfile::tempdir().unwrap();
        let file = radice.path().join("prima.txt");
        let altro = radice.path().join("dopo.txt");
        std::fs::write(&file, b"contenuto").unwrap();
        std::fs::write(&altro, b"altro").unwrap();

        let errore = rinomina(radice.path(), &file, "dopo.txt").unwrap_err();

        assert!(errore.contains("esiste già"), "{errore}");
        assert_eq!(std::fs::read(&file).unwrap(), b"contenuto");
        assert_eq!(std::fs::read(&altro).unwrap(), b"altro");
    }

    #[test]
    fn fuori_progetto_la_rinomina_e_rifiutata() {
        let radice = tempfile::tempdir().unwrap();
        let fuori = tempfile::tempdir().unwrap();
        let file = fuori.path().join("fuori.txt");
        std::fs::write(&file, b"contenuto").unwrap();

        let errore = rinomina(radice.path(), &file, "dentro.txt").unwrap_err();

        assert!(errore.contains("non è dentro il progetto"), "{errore}");
        assert!(file.exists(), "il file fuori progetto e' intatto");
    }

    #[test]
    fn l_avviso_della_cartella_non_sincronizzata_arriva_insieme_ai_riallineamenti() {
        let percorso = Path::new("/progetto/prima.txt");
        assert_eq!(avviso_rinomina(percorso, None, &[]), None);
        let cartella = Some("la cartella non è stata sincronizzata sul disco: EIO".to_owned());
        let testo = avviso_rinomina(percorso, cartella.clone(), &[]).unwrap();
        assert!(
            testo.contains("rinominato, ma la cartella non è stata sincronizzata"),
            "{testo}"
        );
        let testo = avviso_rinomina(percorso, cartella, &["«a»: occupato".to_owned()]).unwrap();
        assert!(testo.contains("sincronizzata sul disco: EIO"), "{testo}");
        assert!(
            testo.contains("non sono stati riallineati: «a»: occupato"),
            "{testo}"
        );
    }
}
