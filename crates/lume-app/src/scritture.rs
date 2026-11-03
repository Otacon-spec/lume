//! Il coordinatore delle scritture: decide quali mutazioni di file possono
//! avvenire adesso (piano 14, termini in `GLOSSARY.md`).
//!
//! Decide e non esegue: niente `Task`, niente I/O, niente avvisi. L'app
//! chiede con [`Coordinatore::ammetti`], chiude con
//! [`Coordinatore::rilascia`] e, in un solo punto (`Lume::update`), svuota
//! il [`Seguito`]: così il ricontrollo della chiusura in sospeso e la
//! ripartenza dei salvataggi non dipendono da chi gestisce l'esito.
use lume_core::DocId;
use std::path::PathBuf;

/// Il tipo di una mutazione di file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tipo {
    Salvataggio,
    SalvaConNome,
    Sostituisci,
    Cestino,
    EliminaAppunto,
    Git,
    Rinomina,
}

impl Tipo {
    /// Come si chiama il tipo di mutazione in un avviso all'utente.
    pub(crate) fn descrizione(self) -> &'static str {
        match self {
            Self::Salvataggio => "il salvataggio",
            Self::SalvaConNome => "«Salva con nome»",
            Self::Sostituisci => "la sostituzione nei file",
            Self::Cestino => "lo spostamento nel Cestino",
            Self::EliminaAppunto => "l'eliminazione dell'appunto",
            Self::Git => "l'operazione Git",
            Self::Rinomina => "la rinomina",
        }
    }
}

/// Chi ha chiesto un salvataggio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Causa {
    Utente,
    Automatico,
    /// Un salvataggio che fa parte di «Sostituisci nei file».
    DiSostituzione(Lease),
}

/// Una mutazione di file da ammettere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op {
    Salva {
        doc: DocId,
        versione: u64,
        causa: Causa,
    },
    SalvaConNome {
        doc: DocId,
    },
    Sostituisci {
        docs: Vec<DocId>,
        percorsi: Vec<PathBuf>,
    },
    Cestino {
        docs: Vec<DocId>,
    },
    EliminaAppunto {
        docs: Vec<DocId>,
    },
    Git,
    /// Istantanea: si ammette e basta, senza lease.
    Rinomina {
        docs: Vec<DocId>,
    },
}

/// Il permesso concesso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Permesso {
    /// La mutazione è in volo: va rilasciata con questo lease.
    Avvia(Lease),
    /// Un salvataggio in fila dietro un altro: partirà dal [`Seguito`].
    Accodato,
    /// Una mutazione istantanea: già conclusa.
    Subito,
    /// Parte della sostituzione già proprietaria: nessun lease da rilasciare.
    Continuazione,
}

/// L'identità di una scrittura in volo. `Copy`: viaggia nei `Message`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Lease(u64);

/// Una mutazione rifiutata perché un'altra è in volo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rifiuto {
    pub in_volo: Tipo,
    pub richiesto: Tipo,
    /// Un salvataggio automatico rimandato: ripartirà da solo.
    pub rimandato: bool,
}

impl Rifiuto {
    /// Il testo dell'avviso da mostrare all'utente: cosa è stato rifiutato e
    /// da cosa. Un salvataggio automatico rimandato lo dice, così l'utente sa
    /// che ripartirà da solo.
    pub(crate) fn avviso(&self) -> String {
        if self.rimandato {
            return format!(
                "{} rimandato: {} è in corso; riparte appena finisce",
                self.richiesto.descrizione(),
                self.in_volo.descrizione()
            );
        }
        format!(
            "{} non è possibile: {} è in corso",
            self.richiesto.descrizione(),
            self.in_volo.descrizione()
        )
    }
}

/// Come è finita una scrittura in volo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Esito {
    Riuscito,
    Fallito,
    /// Ammessa ma non partita (per esempio la preparazione è fallita).
    NonAvviato,
}

/// Lo stato di un documento rispetto alle scritture in volo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Presa {
    Libero,
    /// Il file viene scritto: non ricaricare, non scartare.
    InScrittura,
    /// Il file sta per sparire o cambiare: non modificare.
    Bloccato,
}

/// Cosa vede chi guarda il coordinatore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Vista {
    pub epoca: u64,
    pub in_volo: Option<Tipo>,
    pub file: Presa,
}

/// Cosa fare dopo i rilasci: lo svuota solo `Lume::update`.
#[must_use]
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Seguito {
    /// Salvataggi in fila diventati in volo.
    pub avvia: Vec<(Lease, Op)>,
    /// Documenti con un salvataggio automatico rimandato da rilanciare.
    pub rimandati: Vec<DocId>,
    /// Nessuna scrittura è più in volo dopo un rilascio.
    pub quiete: bool,
}

impl Op {
    /// La presa che la mutazione impone, i documenti che tocca e se li tocca
    /// tutti.
    fn presa(&self) -> (Presa, Vec<DocId>, bool) {
        match self {
            Self::Salva { doc, .. } | Self::SalvaConNome { doc } => {
                (Presa::InScrittura, vec![*doc], false)
            }
            Self::Sostituisci { docs, .. }
            | Self::Cestino { docs }
            | Self::EliminaAppunto { docs } => (Presa::Bloccato, docs.clone(), false),
            Self::Rinomina { docs } => (Presa::Bloccato, docs.clone(), false),
            Self::Git => (Presa::Bloccato, Vec::new(), true),
        }
    }

    fn tipo(&self) -> Tipo {
        match self {
            Self::Salva { .. } => Tipo::Salvataggio,
            Self::SalvaConNome { .. } => Tipo::SalvaConNome,
            Self::Sostituisci { .. } => Tipo::Sostituisci,
            Self::Cestino { .. } => Tipo::Cestino,
            Self::EliminaAppunto { .. } => Tipo::EliminaAppunto,
            Self::Git => Tipo::Git,
            Self::Rinomina { .. } => Tipo::Rinomina,
        }
    }
}

/// Una mutazione in volo.
#[derive(Debug)]
struct InVolo {
    lease: Lease,
    tipo: Tipo,
    /// La presa che impone ai documenti che tocca.
    presa: Presa,
    /// I documenti che tocca; vuoto con `tutti` significa «ogni documento».
    docs: Vec<DocId>,
    percorsi: Vec<PathBuf>,
    tutti: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Coordinatore {
    prossimo: u64,
    /// Avanza a ogni ammissione, estensione della presa e rilascio vero: chi
    /// tiene una vista (per esempio un ricaricamento preparato) può usare per
    /// accorgersi che le scritture sono cambiate.
    epoca: u64,
    in_volo: Option<InVolo>,
    /// I salvataggi in fila, uno per documento: la versione più recente
    /// sostituisce quella vecchia, così non si scrive due volte la stessa cosa.
    coda: Vec<Op>,
    /// Salvataggi automatici rifiutati: ripartono da soli alla quiete.
    rimandati: Vec<Op>,
    seguito: Seguito,
}

/// Accoda un salvataggio in una fila, tenendo una sola voce per documento:
/// la versione più recente sostituisce quella vecchia, così non si scrive due
/// volte la stessa cosa e non si scrive una versione sorpassata.
fn accoda_in(fila: &mut Vec<Op>, op: Op) {
    if let Op::Salva { doc, .. } = &op
        && let Some(slot) = fila
            .iter_mut()
            .find(|altro| matches!(altro, Op::Salva { doc: d, .. } if d == doc))
    {
        *slot = op;
        return;
    }
    fila.push(op);
}

/// Toglie la prima voce di una fila.
fn togli_primo(fila: &mut Vec<Op>) -> Option<Op> {
    if fila.is_empty() {
        None
    } else {
        Some(fila.remove(0))
    }
}

impl Coordinatore {
    /// Verifica il proprietario prima che il chiamante modifichi il suo stato.
    pub(crate) fn possiede(&self, lease: Lease, tipo: Tipo) -> bool {
        self.in_volo
            .as_ref()
            .is_some_and(|in_volo| in_volo.lease == lease && in_volo.tipo == tipo)
    }

    /// Aggiunge gli alias scoperti dal preflight alla stessa eliminazione.
    /// Un lease scaduto o appartenente a un altro tipo non cambia nulla.
    pub(crate) fn estendi_presa(&mut self, lease: Lease, docs: &[DocId]) -> bool {
        let Some(in_volo) = self.in_volo.as_mut() else {
            return false;
        };
        if in_volo.lease != lease || in_volo.tipo != Tipo::EliminaAppunto {
            return false;
        }
        let mut cambiata = false;
        for &doc in docs {
            if !in_volo.docs.contains(&doc) {
                in_volo.docs.push(doc);
                cambiata = true;
            }
        }
        if cambiata {
            self.epoca = self.epoca.wrapping_add(1);
        }
        true
    }

    /// Il commit o la continuazione del batch appartengono allo stesso lease.
    /// Un esito stantio non altera l'epoca né il successore.
    pub(crate) fn avanzamento_sostituzione(&mut self, lease: Lease) -> bool {
        if !self.possiede(lease, Tipo::Sostituisci) {
            return false;
        }
        self.epoca = self.epoca.wrapping_add(1);
        true
    }

    /// Evento di inizio o fine di una guardia Git non-worktree, autenticato
    /// dal chiamante tramite il ticket della guardia; non ammette un writer.
    pub(crate) fn evento_git_esterno(&mut self) {
        self.epoca = self.epoca.wrapping_add(1);
    }

    pub(crate) fn ammetti(&mut self, op: Op) -> Result<Permesso, Rifiuto> {
        let richiesto = op.tipo();
        if let Op::Salva {
            doc,
            causa: Causa::DiSostituzione(lease),
            ..
        } = &op
        {
            let corrente = self.in_volo.as_ref();
            if corrente.is_some_and(|v| {
                v.tipo == Tipo::Sostituisci && v.lease == *lease && v.docs.contains(doc)
            }) {
                return Ok(Permesso::Continuazione);
            }
            return Err(Rifiuto {
                in_volo: corrente.map_or(Tipo::Sostituisci, |v| v.tipo),
                richiesto,
                rimandato: false,
            });
        }
        if let Some(in_volo) = &self.in_volo {
            // Un salvataggio dietro un altro salvataggio si accoda.
            if in_volo.tipo == Tipo::Salvataggio && richiesto == Tipo::Salvataggio {
                // Accodare cambia la presa del documento (passa a
                // «in scrittura»): chi tiene una vista deve accorgersene.
                self.epoca = self.epoca.wrapping_add(1);
                if let Op::Salva { doc, .. } = &op {
                    self.rimandati
                        .retain(|v| !matches!(v, Op::Salva { doc: d, .. } if d == doc));
                }
                accoda_in(&mut self.coda, op);
                return Ok(Permesso::Accodato);
            }
            // Un salvataggio automatico rifiutato non si perde: riparte alla
            // quiete. Uno chiesto dall'utente no: l'utente vede l'avviso.
            let rimandato = matches!(
                &op,
                Op::Salva {
                    causa: Causa::Automatico,
                    ..
                }
            );
            if rimandato {
                self.epoca = self.epoca.wrapping_add(1);
                if let Op::Salva { doc, .. } = &op {
                    self.coda
                        .retain(|v| !matches!(v, Op::Salva { doc: d, .. } if d == doc));
                }
                accoda_in(&mut self.rimandati, op);
            }
            return Err(Rifiuto {
                in_volo: in_volo.tipo,
                richiesto,
                rimandato,
            });
        }
        // La rinomina è istantanea: si ammette e basta, senza lease e senza
        // restare in volo. L'app la esegue subito dopo l'ammissione, e il
        // documento non resta bloccato oltre il tempo della mutazione.
        if richiesto == Tipo::Rinomina {
            self.epoca = self.epoca.wrapping_add(1);
            return Ok(Permesso::Subito);
        }
        let lease = self.avvia(op.clone());
        if matches!(op, Op::Salva { .. }) {
            self.seguito.avvia.push((lease, op));
        }
        Ok(Permesso::Avvia(lease))
    }

    /// Mette una mutazione in volo e restituisce il suo lease.
    fn avvia(&mut self, op: Op) -> Lease {
        self.seguito.quiete = false;
        self.epoca = self.epoca.wrapping_add(1);
        let lease = self.nuovo_lease();
        let (presa, docs, tutti) = op.presa();
        let percorsi = match &op {
            Op::Sostituisci { percorsi, .. } => percorsi.clone(),
            _ => Vec::new(),
        };
        self.in_volo = Some(InVolo {
            lease,
            tipo: op.tipo(),
            presa,
            docs,
            percorsi,
            tutti,
        });
        lease
    }

    fn nuovo_lease(&mut self) -> Lease {
        self.prossimo = self.prossimo.wrapping_add(1);
        Lease(self.prossimo)
    }

    pub(crate) fn rilascia(&mut self, lease: Lease, esito: Esito) {
        let _ = esito;
        let Some(in_volo) = &self.in_volo else {
            return;
        };
        // Un lease stantio non tocca niente: né la quiete né l'epoca.
        if in_volo.lease != lease {
            return;
        }
        self.in_volo = None;
        self.epoca = self.epoca.wrapping_add(1);
        // Prima i salvataggi già in fila, poi quelli rimandati: ripartono da
        // soli, uno per volta, e si annunciano.
        if let Some(prossimo) = togli_primo(&mut self.coda) {
            self.riavvia(prossimo, false);
        } else if let Some(rimandato) = togli_primo(&mut self.rimandati) {
            self.riavvia(rimandato, true);
        } else {
            self.seguito.quiete = true;
        }
    }

    fn riavvia(&mut self, op: Op, da_rimandato: bool) {
        if da_rimandato && let Op::Salva { doc, .. } = &op {
            self.seguito.rimandati.push(*doc);
        }
        let lease = self.avvia(op.clone());
        self.seguito.avvia.push((lease, op));
    }

    /// Salvataggi rimasti in attesa, inclusi quelli rimandati da altre mutazioni.
    pub(crate) fn contiene_salvataggio(&self, id: DocId) -> bool {
        self.in_volo
            .as_ref()
            .is_some_and(|v| v.tipo == Tipo::Salvataggio && v.docs.contains(&id))
            || self
                .coda
                .iter()
                .chain(&self.rimandati)
                .any(|op| matches!(op, Op::Salva { doc, .. } if *doc == id))
    }

    /// Revoca solo le richieste non ancora ammesse, non il lavoro in volo.
    pub(crate) fn revoca_salvataggi(&mut self, documento: Option<DocId>) -> usize {
        let prima = self.coda.len() + self.rimandati.len();
        let conserva =
            |op: &Op| !matches!(op, Op::Salva { doc, .. } if documento.is_none_or(|id| id == *doc));
        self.coda.retain(conserva);
        self.rimandati.retain(conserva);
        let rimossi = prima - self.coda.len() - self.rimandati.len();
        if rimossi > 0 {
            self.epoca = self.epoca.wrapping_add(1);
        }
        rimossi
    }

    /// Una guardia esterna non migrata non acquista un lease. Solo gli
    /// automatici possono attendere il suo rilascio.
    pub(crate) fn rimanda_esterno(&mut self, op: Op) -> Rifiuto {
        let rimandato = matches!(
            op,
            Op::Salva {
                causa: Causa::Automatico,
                ..
            }
        );
        if rimandato {
            accoda_in(&mut self.rimandati, op);
            self.epoca = self.epoca.wrapping_add(1);
        }
        Rifiuto {
            in_volo: Tipo::Git,
            richiesto: Tipo::Salvataggio,
            rimandato,
        }
    }

    pub(crate) fn riprendi_esterno(&mut self) {
        if self.in_volo.is_none() {
            if let Some(op) = togli_primo(&mut self.coda) {
                self.riavvia(op, false);
            } else if let Some(op) = togli_primo(&mut self.rimandati) {
                self.riavvia(op, true);
            } else {
                self.seguito.quiete = true;
            }
        }
    }

    pub(crate) fn guarda(&self, doc: Option<(DocId, Option<&std::path::Path>)>) -> Vista {
        let file = match doc {
            None => Presa::Libero,
            Some((id, path)) => {
                let in_volo = self.in_volo.as_ref().map_or(Presa::Libero, |v| {
                    if v.tutti
                        || v.docs.contains(&id)
                        || path.is_some_and(|p| v.percorsi.iter().any(|target| target == p))
                    {
                        v.presa
                    } else {
                        Presa::Libero
                    }
                });
                // Anche gli automatici rimandati attendono una scrittura:
                // la lettura del documento va rinviata fino alla ripresa.
                let in_coda = if self
                    .coda
                    .iter()
                    .chain(&self.rimandati)
                    .any(|op| matches!(op, Op::Salva { doc, .. } if *doc == id))
                {
                    Presa::InScrittura
                } else {
                    Presa::Libero
                };
                in_volo.max(in_coda)
            }
        };
        Vista {
            epoca: self.epoca,
            in_volo: self.in_volo.as_ref().map(|v| v.tipo),
            file,
        }
    }

    pub(crate) fn seguito(&mut self) -> Seguito {
        std::mem::take(&mut self.seguito)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn avvia(c: &mut Coordinatore, op: Op) -> Lease {
        match c.ammetti(op) {
            Ok(Permesso::Avvia(lease)) => {
                let _ = c.seguito();
                lease
            }
            altro => panic!("atteso Avvia, ottenuto {altro:?}"),
        }
    }

    #[test]
    fn vista_include_automatico_rimandato_anche_senza_owner() {
        let mut c = Coordinatore::default();
        let doc = DocId(42);
        let rifiuto = c.rimanda_esterno(salva(doc, 1, Causa::Automatico));
        assert!(rifiuto.rimandato);
        assert_eq!(c.guarda(None).in_volo, None);
        assert_eq!(c.guarda(Some((doc, None))).file, Presa::InScrittura);
        c.riprendi_esterno();
        assert_eq!(c.guarda(None).in_volo, Some(Tipo::Salvataggio));
        assert_eq!(c.guarda(Some((doc, None))).file, Presa::InScrittura);
    }

    #[test]
    fn avanzamento_stantio_non_altera_epoca_ne_successore() {
        let mut c = Coordinatore::default();
        let vecchio = avvia(
            &mut c,
            Op::Sostituisci {
                docs: vec![],
                percorsi: vec![],
            },
        );
        assert!(c.avanzamento_sostituzione(vecchio));
        c.rilascia(vecchio, Esito::Riuscito);
        let nuovo = avvia(
            &mut c,
            Op::Sostituisci {
                docs: vec![],
                percorsi: vec![],
            },
        );
        let vista = c.guarda(None);
        assert!(!c.avanzamento_sostituzione(vecchio));
        c.rilascia(vecchio, Esito::Riuscito);
        assert_eq!(c.guarda(None), vista);
        assert!(c.possiede(nuovo, Tipo::Sostituisci));
        assert_eq!(c.seguito(), Seguito::default());
    }

    #[test]
    fn una_mutazione_in_volo_rifiuta_una_mutazione_di_altro_tipo() {
        let mut c = Coordinatore::default();
        avvia(&mut c, Op::Cestino { docs: vec![] });

        let rifiuto = c.ammetti(Op::Git).unwrap_err();

        assert_eq!(rifiuto.in_volo, Tipo::Cestino);
        assert_eq!(rifiuto.richiesto, Tipo::Git);
        assert!(!rifiuto.rimandato);
    }

    #[test]
    fn quiete_precedente_non_sopravvive_a_una_nuova_ammissione() {
        let mut c = Coordinatore::default();
        let primo = avvia(&mut c, Op::Cestino { docs: vec![] });
        c.rilascia(primo, Esito::Riuscito);
        assert!(matches!(
            c.ammetti(salva(DocId(1), 1, Causa::Utente)),
            Ok(Permesso::Avvia(_))
        ));
        assert!(!c.seguito().quiete, "la nuova scrittura non è in quiete");
    }

    #[test]
    fn il_rilascio_libera_il_posto_per_un_altra_mutazione() {
        let mut c = Coordinatore::default();
        let lease = avvia(&mut c, Op::Cestino { docs: vec![] });

        c.rilascia(lease, Esito::Riuscito);

        avvia(&mut c, Op::Git);
    }

    #[test]
    fn un_lease_stantio_non_libera_la_mutazione_in_volo() {
        let mut c = Coordinatore::default();
        let vecchio = avvia(&mut c, Op::Cestino { docs: vec![] });
        c.rilascia(vecchio, Esito::Riuscito);
        avvia(&mut c, Op::Git);

        c.rilascia(vecchio, Esito::Riuscito);

        let rifiuto = c.ammetti(Op::Cestino { docs: vec![] }).unwrap_err();
        assert_eq!(rifiuto.in_volo, Tipo::Git);
    }

    #[test]
    fn la_quiete_arriva_una_volta_sola_dopo_l_ultimo_rilascio() {
        let mut c = Coordinatore::default();
        let lease = avvia(&mut c, Op::Git);
        assert!(!c.seguito().quiete, "ancora in volo");

        c.rilascia(lease, Esito::Fallito);

        assert!(c.seguito().quiete);
        assert!(!c.seguito().quiete, "il seguito si svuota");
    }

    #[test]
    fn la_rinomina_e_istantanea_e_non_resta_in_volo() {
        let mut c = Coordinatore::default();
        let prima = c.guarda(None);

        assert_eq!(
            c.ammetti(Op::Rinomina { docs: vec![] }),
            Ok(Permesso::Subito)
        );
        let dopo = c.guarda(None);
        assert_eq!(dopo.epoca, prima.epoca + 1);
        assert_eq!(dopo.in_volo, None);
        assert_eq!(dopo.file, Presa::Libero);
        assert_eq!(c.seguito(), Seguito::default());

        avvia(&mut c, Op::Git);
    }

    #[test]
    fn rinomina_rifiutata_non_altera_lease_ne_coda_dei_salvataggi() {
        let mut c = Coordinatore::default();
        let primo = avvia(&mut c, salva(DocId(1), 1, Causa::Utente));
        let secondo = salva(DocId(2), 2, Causa::Utente);
        assert_eq!(c.ammetti(secondo.clone()), Ok(Permesso::Accodato));
        let prima = c.guarda(None);
        assert_eq!(
            c.ammetti(Op::Rinomina {
                docs: vec![DocId(2)]
            })
            .unwrap_err()
            .in_volo,
            Tipo::Salvataggio
        );
        assert_eq!(c.guarda(None), prima);
        c.rilascia(primo, Esito::Riuscito);
        let seguito = c.seguito();
        assert_eq!(seguito.avvia.len(), 1);
        assert_eq!(seguito.avvia.first().map(|(_, op)| op), Some(&secondo));
        assert!(!seguito.quiete);
    }

    #[test]
    fn l_epoca_avanza_a_ogni_ammissione_e_a_ogni_rilascio_vero() {
        let mut c = Coordinatore::default();
        let iniziale = c.guarda(None).epoca;

        let lease = avvia(&mut c, Op::Git);
        let dopo_ammissione = c.guarda(None).epoca;
        assert!(dopo_ammissione > iniziale, "l'ammissione avanza l'epoca");

        c.rilascia(lease, Esito::Riuscito);
        let dopo_rilascio = c.guarda(None).epoca;
        assert!(
            dopo_rilascio > dopo_ammissione,
            "il rilascio vero avanza l'epoca"
        );

        c.rilascia(lease, Esito::Riuscito);
        assert_eq!(
            c.guarda(None).epoca,
            dopo_rilascio,
            "un lease stantio non tocca l'epoca"
        );
    }

    fn uno_e_due() -> (DocId, DocId) {
        (DocId(1), DocId(2))
    }

    #[test]
    fn estensione_degli_alias_autentica_lease_deduplica_e_avanza_solo_se_cambia() {
        let mut c = Coordinatore::default();
        let (uno, due) = uno_e_due();
        let vecchio = avvia(&mut c, Op::EliminaAppunto { docs: vec![uno] });
        c.rilascia(vecchio, Esito::NonAvviato);
        let _ = c.seguito();
        let attuale = avvia(&mut c, Op::EliminaAppunto { docs: vec![uno] });
        let epoca = c.guarda(None).epoca;
        assert!(!c.estendi_presa(vecchio, &[due]));
        assert_eq!(c.guarda(None).epoca, epoca);
        assert_eq!(c.guarda(Some((due, None))).file, Presa::Libero);
        assert!(c.estendi_presa(attuale, &[uno, due, due]));
        let estesa = c.guarda(None).epoca;
        assert_eq!(estesa, epoca + 1);
        assert_eq!(c.guarda(Some((due, None))).file, Presa::Bloccato);
        assert!(c.estendi_presa(attuale, &[due, uno]));
        assert_eq!(c.guarda(None).epoca, estesa);
        assert!(!c.seguito().quiete);
        c.rilascia(attuale, Esito::Riuscito);
        let _ = c.seguito();
        let git = avvia(&mut c, Op::Git);
        let epoca_git = c.guarda(None).epoca;
        assert!(!c.estendi_presa(git, &[due]));
        assert_eq!(c.guarda(None).epoca, epoca_git);
    }

    #[test]
    fn la_presa_riguarda_solo_i_documenti_toccati() {
        let mut c = Coordinatore::default();
        let (uno, due) = uno_e_due();

        let lease = avvia(&mut c, Op::Cestino { docs: vec![uno] });

        assert_eq!(
            c.guarda(Some((uno, None))).file,
            Presa::Bloccato,
            "il documento che va nel Cestino è bloccato"
        );
        assert_eq!(
            c.guarda(Some((due, None))).file,
            Presa::Libero,
            "un altro documento resta libero"
        );
        assert_eq!(c.guarda(None).in_volo, Some(Tipo::Cestino));

        c.rilascia(lease, Esito::Riuscito);
        assert_eq!(c.guarda(Some((uno, None))).file, Presa::Libero);
    }

    #[test]
    fn ogni_tipo_dichiara_la_sua_presa() {
        let (uno, due) = uno_e_due();
        // Regola 4 del piano: si blocca solo il documento toccato, il resto
        // dell'editor resta usabile. L'unica eccezione è Git, che riscrive
        // l'albero e quindi li tocca tutti.
        let casi = [
            (
                Op::Cestino { docs: vec![uno] },
                Presa::Bloccato,
                Presa::Libero,
            ),
            (
                Op::EliminaAppunto { docs: vec![uno] },
                Presa::Bloccato,
                Presa::Libero,
            ),
            (
                Op::Sostituisci {
                    docs: vec![uno],
                    percorsi: vec![],
                },
                Presa::Bloccato,
                Presa::Libero,
            ),
            (
                Op::SalvaConNome { doc: uno },
                Presa::InScrittura,
                Presa::Libero,
            ),
            (
                Op::Salva {
                    doc: uno,
                    versione: 7,
                    causa: Causa::Utente,
                },
                Presa::InScrittura,
                Presa::Libero,
            ),
            (Op::Git, Presa::Bloccato, Presa::Bloccato),
        ];
        for (op, attesa, attesa_altro) in casi {
            let mut c = Coordinatore::default();
            let tipo = op.tipo();
            let _lease = avvia(&mut c, op);
            assert_eq!(
                c.guarda(Some((uno, None))).file,
                attesa,
                "presa sbagliata per {tipo:?}"
            );
            assert_eq!(
                c.guarda(Some((due, None))).file,
                attesa_altro,
                "presa sbagliata per {tipo:?} su un documento non elencato"
            );
        }
    }

    fn salva(doc: DocId, versione: u64, causa: Causa) -> Op {
        Op::Salva {
            doc,
            versione,
            causa,
        }
    }

    #[test]
    fn i_salvataggi_si_accodano_fra_loro_e_la_coda_deduplica_per_documento() {
        let mut c = Coordinatore::default();
        let (uno, due) = uno_e_due();
        let primo = avvia(&mut c, salva(uno, 1, Causa::Utente));

        assert_eq!(
            c.ammetti(salva(uno, 2, Causa::Utente)),
            Ok(Permesso::Accodato)
        );
        assert_eq!(
            c.ammetti(salva(due, 1, Causa::Utente)),
            Ok(Permesso::Accodato)
        );
        // il secondo salvataggio di `uno` sostituisce il primo in coda: vale
        // la versione più recente, non due scritture della stessa roba
        assert_eq!(
            c.ammetti(salva(uno, 3, Causa::Utente)),
            Ok(Permesso::Accodato)
        );
        // durante i salvataggi le altre mutazioni sono rifiutate
        let rifiuto = c.ammetti(Op::Git).unwrap_err();
        assert_eq!(rifiuto.in_volo, Tipo::Salvataggio);
        // e il documento in coda risulta già in scrittura
        assert_eq!(
            c.guarda(Some((due, None))).file,
            Presa::InScrittura,
            "un documento in coda è in scrittura"
        );

        c.rilascia(primo, Esito::Riuscito);
        let seguito = c.seguito();
        assert_eq!(seguito.avvia.len(), 1, "parte un salvataggio per volta");
        assert!(!seguito.quiete, "la coda non è vuota");

        let Some((_, primo_op)) = seguito.avvia.first() else {
            panic!("atteso un salvataggio avviato");
        };
        let Op::Salva { doc, versione, .. } = primo_op else {
            panic!("atteso un salvataggio");
        };
        assert_eq!(*doc, uno);
        assert_eq!(*versione, 3, "in coda resta la versione più recente");
    }

    #[test]
    fn la_quiete_arriva_solo_quando_la_coda_e_vuota() {
        let mut c = Coordinatore::default();
        let (uno, due) = uno_e_due();
        let primo = avvia(&mut c, salva(uno, 1, Causa::Utente));
        assert_eq!(
            c.ammetti(salva(due, 1, Causa::Utente)),
            Ok(Permesso::Accodato)
        );

        c.rilascia(primo, Esito::Riuscito);
        let Some(secondo) = c.seguito().avvia.first().map(|(lease, _)| *lease) else {
            panic!("atteso il secondo salvataggio");
        };
        assert!(!c.seguito().quiete, "il secondo è in volo");

        c.rilascia(secondo, Esito::Riuscito);
        assert!(c.seguito().quiete, "coda vuota: quiete");
    }

    #[test]
    fn un_salvataggio_automatico_rifiutato_riparte_alla_quiete() {
        let mut c = Coordinatore::default();
        let (uno, _) = uno_e_due();
        let git = avvia(&mut c, Op::Git);

        let rifiuto = c.ammetti(salva(uno, 4, Causa::Automatico)).unwrap_err();
        assert_eq!(rifiuto.in_volo, Tipo::Git);
        assert!(
            rifiuto.rimandato,
            "il salvataggio automatico è rimandato, non perso"
        );
        assert!(
            c.seguito().avvia.is_empty(),
            "non riparte mentre Git è in volo"
        );

        c.rilascia(git, Esito::Riuscito);

        let seguito = c.seguito();
        assert_eq!(seguito.avvia.len(), 1, "riparte alla quiete");
        assert_eq!(seguito.rimandati, vec![uno], "e lo dice, per l'avviso");
        assert!(!seguito.quiete, "ripartito: la quiete non è ancora qui");
    }

    #[test]
    fn un_salvataggio_di_sostituzione_e_un_salvataggio_come_gli_altri() {
        let mut c = Coordinatore::default();
        let (uno, _) = uno_e_due();
        let _sostituisci = avvia(
            &mut c,
            Op::Sostituisci {
                docs: vec![uno],
                percorsi: vec![],
            },
        );

        let rifiuto = c
            .ammetti(salva(uno, 2, Causa::DiSostituzione(Lease(9))))
            .unwrap_err();

        assert_eq!(rifiuto.in_volo, Tipo::Sostituisci);
        assert!(
            !rifiuto.rimandato,
            "un salvataggio di sostituzione non è un automatico da rimandare"
        );
    }

    #[test]
    fn continuazione_della_sostituzione_autentica_documento_e_lease_senza_quiete() {
        let mut c = Coordinatore::default();
        let (uno, due) = uno_e_due();
        let lease = avvia(
            &mut c,
            Op::Sostituisci {
                docs: vec![uno],
                percorsi: vec![],
            },
        );
        assert!(
            c.ammetti(salva(due, 2, Causa::DiSostituzione(lease)))
                .is_err()
        );
        assert!(
            c.ammetti(salva(uno, 2, Causa::DiSostituzione(Lease(999))))
                .is_err()
        );
        assert!(
            c.ammetti(salva(uno, 2, Causa::DiSostituzione(lease)))
                .is_ok()
        );
        assert!(c.possiede(lease, Tipo::Sostituisci));
        assert!(!c.seguito().quiete);
        assert!(c.ammetti(Op::Git).is_err());
        c.rilascia(lease, Esito::Riuscito);
        assert!(c.seguito().quiete);
        assert!(
            c.ammetti(salva(uno, 2, Causa::DiSostituzione(lease)))
                .is_err()
        );
    }

    #[test]
    fn continuazione_con_lease_di_altro_tipo_non_diventa_salvataggio_in_coda() {
        let mut c = Coordinatore::default();
        let (uno, _) = uno_e_due();
        let lease = avvia(&mut c, salva(uno, 1, Causa::Utente));
        assert!(
            c.ammetti(salva(uno, 2, Causa::DiSostituzione(lease)))
                .is_err()
        );
        c.rilascia(lease, Esito::Riuscito);
        let seguito = c.seguito();
        assert!(seguito.avvia.is_empty());
        assert!(seguito.quiete);
    }

    #[test]
    fn presa_di_sostituzione_riconosce_anche_il_percorso_gia_validato() {
        let mut c = Coordinatore::default();
        let (uno, due) = uno_e_due();
        let target = PathBuf::from("progetto/chiuso.txt");
        let lease = avvia(
            &mut c,
            Op::Sostituisci {
                docs: vec![uno],
                percorsi: vec![target.clone()],
            },
        );
        assert_eq!(c.guarda(Some((due, Some(&target)))).file, Presa::Bloccato);
        assert_eq!(
            c.guarda(Some((
                due,
                Some(std::path::Path::new("progetto/altro.txt"))
            )))
            .file,
            Presa::Libero
        );
        c.rilascia(lease, Esito::Riuscito);
        assert_eq!(c.guarda(Some((due, Some(&target)))).file, Presa::Libero);
    }

    #[test]
    fn un_salvataggio_utente_rifiutato_non_viene_rimandato() {
        let mut c = Coordinatore::default();
        let (uno, _) = uno_e_due();
        let _git = avvia(&mut c, Op::Git);

        let rifiuto = c.ammetti(salva(uno, 4, Causa::Utente)).unwrap_err();

        assert!(
            !rifiuto.rimandato,
            "un salvataggio dell'utente non riparte da solo"
        );
    }

    #[test]
    fn il_rifiuto_sa_dire_chi_lo_ha_bloccato() {
        let mut c = Coordinatore::default();
        let _git = avvia(&mut c, Op::Git);

        let rifiuto = c.ammetti(Op::Cestino { docs: vec![] }).unwrap_err();
        let avviso = rifiuto.avviso();

        assert!(avviso.contains("Cestino"), "{avviso}");
        assert!(avviso.contains("Git"), "{avviso}");
        assert!(!avviso.contains("rimandato"), "{avviso}");

        let rimandato = c
            .ammetti(salva(DocId(1), 2, Causa::Automatico))
            .unwrap_err()
            .avviso();
        assert!(rimandato.contains("rimandato"), "{rimandato}");
        assert!(rimandato.contains("riparte"), "{rimandato}");
    }

    #[test]
    fn i_rimandati_deduplicano_per_documento_come_la_coda() {
        let mut c = Coordinatore::default();
        let (uno, _) = uno_e_due();
        let git = avvia(&mut c, Op::Git);

        let _ = c.ammetti(salva(uno, 2, Causa::Automatico));
        let _ = c.ammetti(salva(uno, 3, Causa::Automatico));
        let _ = c.ammetti(salva(uno, 5, Causa::Automatico));

        c.rilascia(git, Esito::Riuscito);

        let seguito = c.seguito();
        assert_eq!(seguito.avvia.len(), 1, "una sola ripresa per documento");
        let Some((_, ripreso)) = seguito.avvia.first() else {
            panic!("atteso il salvataggio ripreso");
        };
        let Op::Salva { versione, .. } = ripreso else {
            panic!("atteso un salvataggio");
        };
        assert_eq!(*versione, 5, "riprende la versione più recente");
        assert_eq!(seguito.rimandati, vec![uno]);
    }

    #[test]
    fn revoca_per_documento_e_globale_non_libera_il_lease_in_volo() {
        let mut c = Coordinatore::default();
        let git = avvia(&mut c, Op::Git);
        let _ = c.ammetti(salva(DocId(1), 1, Causa::Automatico));
        let _ = c.ammetti(salva(DocId(2), 2, Causa::Automatico));
        let _ = c.ammetti(salva(DocId(3), 3, Causa::Automatico));
        c.rilascia(git, Esito::Riuscito);
        let primo = c.seguito().avvia.first().unwrap().0;
        assert_eq!(
            c.ammetti(salva(DocId(4), 4, Causa::Utente)),
            Ok(Permesso::Accodato)
        );
        assert_eq!(c.revoca_salvataggi(Some(DocId(2))), 1);
        assert_eq!(c.revoca_salvataggi(Some(DocId(1))), 0);
        assert!(c.possiede(primo, Tipo::Salvataggio));
        assert_eq!(c.revoca_salvataggi(None), 2);
        c.rilascia(git, Esito::Riuscito);
        assert!(c.possiede(primo, Tipo::Salvataggio));
        c.rilascia(primo, Esito::Riuscito);
        let seguito = c.seguito();
        assert!(seguito.avvia.is_empty());
        assert!(seguito.quiete);
    }

    #[test]
    fn accodare_avanza_l_epoca_perche_cambia_la_presa() {
        let mut c = Coordinatore::default();
        let (uno, _) = uno_e_due();
        let _primo = avvia(&mut c, salva(uno, 1, Causa::Utente));
        let prima = c.guarda(Some((uno, None))).epoca;

        assert_eq!(
            c.ammetti(salva(DocId(2), 1, Causa::Utente)),
            Ok(Permesso::Accodato)
        );

        assert!(c.guarda(None).epoca > prima, "accodare cambia la presa");
        assert_eq!(
            c.guarda(Some((DocId(2), None))).file,
            Presa::InScrittura,
            "il documento in coda è in scrittura"
        );
    }
}
