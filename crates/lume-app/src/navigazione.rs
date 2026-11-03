//! La cronologia di navigazione: **una pila sola per tre comandi**.
//!
//! Indietro/avanti (Ctrl+Alt+←/→) e i file recenti (Ctrl+E) escono dalla
//! stessa lista, letta in due modi: la prima muove un indice, la seconda la
//! rilegge all'indietro tenendo un file solo per percorso.
//!
//! ATTENZIONE: **Da non confondere con il tool «Cronologia»** (`tools/history.rs`),
//! che è un registro di aperture, salvataggi e commit **persistito nella
//! sessione**. Sono due cose diverse con lo stesso nome italiano: questa vive
//! in memoria e muore con la finestra, ed è giusto così — «torna dov'ero» non
//! ha senso il giorno dopo.

use std::path::{Path, PathBuf};

/// Quante posizioni si tengono. Oltre, le più vecchie cadono dalla testa.
pub const TETTO: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posizione {
    pub path: PathBuf,
    /// Riga **0-based**, come la conta `lume-core`.
    pub riga: usize,
}

impl Posizione {
    pub fn new(path: impl Into<PathBuf>, riga: usize) -> Self {
        Self {
            path: path.into(),
            riga,
        }
    }
}

/// Le posizioni visitate, con un dito su dove siamo.
#[derive(Debug, Default)]
pub struct CronologiaNavigazione {
    pila: Vec<Posizione>,
    /// Indice della posizione **corrente** dentro `pila`. Senza significato
    /// quando la pila è vuota.
    indice: usize,
}

impl CronologiaNavigazione {
    pub fn new() -> Self {
        Self::default()
    }

    /// Quante posizioni ci sono. Serve solo alle prove: la UI legge
    /// [`Self::recenti`] o [`Self::corrente`], mai il conteggio grezzo, ed è
    /// lo stesso motivo per cui non c'è un `is_empty` — «vuota» qui vuol
    /// dire `corrente() == None`, che si chiede già dove serve.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.pila.len()
    }

    /// Registra una posizione nuova.
    ///
    /// Tre regole, e ognuna toglie un fastidio vero:
    /// - la **stessa** posizione due volte di fila non si registra: muoversi
    ///   dentro la stessa riga non è navigare;
    /// - un salto nuovo **tronca la coda** dall'indice in poi, come in ogni
    ///   cronologia con avanti e indietro: aver girato in avanti e poi essere
    ///   andati altrove cancella il ramo che non si è preso;
    /// - oltre [`TETTO`] cadono le più vecchie, non le più recenti.
    pub fn push(&mut self, posizione: Posizione) {
        if self.corrente() == Some(&posizione) {
            return;
        }
        if !self.pila.is_empty() {
            self.pila.truncate(self.indice + 1);
        }
        self.pila.push(posizione);
        while self.pila.len() > TETTO {
            self.pila.remove(0);
        }
        self.indice = self.pila.len().saturating_sub(1);
    }

    pub fn corrente(&self) -> Option<&Posizione> {
        self.pila.get(self.indice)
    }

    /// Un passo indietro. `None` quando non c'è più niente dietro.
    pub fn indietro(&mut self) -> Option<&Posizione> {
        if self.indice == 0 {
            return None;
        }
        self.indice -= 1;
        self.pila.get(self.indice)
    }

    /// Un passo avanti. `None` quando si è già in cima.
    pub fn avanti(&mut self) -> Option<&Posizione> {
        if self.indice + 1 >= self.pila.len() {
            return None;
        }
        self.indice += 1;
        self.pila.get(self.indice)
    }

    /// I file distinti, dal più recente. È la stessa pila letta
    /// all'indietro: un file visitato tre volte compare una volta sola, con
    /// la riga **dell'ultima** visita.
    pub fn recenti(&self) -> Vec<&Posizione> {
        let mut visti: Vec<&Path> = Vec::new();
        let mut fuori = Vec::new();
        for posizione in self.pila.iter().rev() {
            if visti.contains(&posizione.path.as_path()) {
                continue;
            }
            visti.push(posizione.path.as_path());
            fuori.push(posizione);
        }
        fuori
    }

    /// Toglie dalla pila tutto quel che riguarda un file. Serve quando un
    /// file sparisce da sotto: «torna dov'eri» su un percorso che non esiste
    /// più è una promessa che non si può mantenere.
    pub fn forget(&mut self, path: &Path) {
        let prima = self.pila.len();
        let mut caduti_prima = 0usize;
        for (indice, posizione) in self.pila.iter().enumerate() {
            if posizione.path == path && indice <= self.indice {
                caduti_prima += 1;
            }
        }
        self.pila.retain(|posizione| posizione.path != path);
        if self.pila.len() != prima {
            self.indice = self.indice.saturating_sub(caduti_prima);
            self.indice = self.indice.min(self.pila.len().saturating_sub(1));
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{CronologiaNavigazione, Posizione, TETTO};

    fn pos(file: &str, riga: usize) -> Posizione {
        Posizione::new(format!("C:/repo/{file}"), riga)
    }

    /// ATTENZIONE: Il ramo non preso si cancella. È la regola che rende «avanti»
    /// prevedibile: senza, dopo essere tornati indietro e aver aperto
    /// qualcos'altro, «avanti» porterebbe in un posto che non c'entra più
    /// niente con quel che si stava facendo.
    #[test]
    fn un_salto_nuovo_taglia_il_ramo_che_non_si_e_preso() {
        let mut storia = CronologiaNavigazione::new();
        storia.push(pos("a.rs", 1));
        storia.push(pos("b.rs", 2));
        storia.push(pos("c.rs", 3));
        assert_eq!(storia.corrente(), Some(&pos("c.rs", 3)));

        assert_eq!(storia.indietro(), Some(&pos("b.rs", 2)));
        assert_eq!(storia.indietro(), Some(&pos("a.rs", 1)));
        assert_eq!(storia.indietro(), None, "in fondo non si va oltre");
        assert_eq!(storia.avanti(), Some(&pos("b.rs", 2)));

        storia.push(pos("d.rs", 4));
        assert_eq!(storia.len(), 3, "«c.rs» è caduto: era il ramo non preso");
        assert_eq!(storia.avanti(), None);
        assert_eq!(storia.indietro(), Some(&pos("b.rs", 2)));
    }

    /// Muoversi dentro la stessa riga non è navigare, e riempirebbe la
    /// cronologia di doppioni fino a buttare fuori quel che serve davvero.
    #[test]
    fn la_stessa_posizione_due_volte_di_fila_non_si_registra() {
        let mut storia = CronologiaNavigazione::new();
        storia.push(pos("a.rs", 1));
        storia.push(pos("a.rs", 1));
        storia.push(pos("a.rs", 1));
        assert_eq!(storia.len(), 1);
        // ma la stessa riga tornandoci **dopo** essere stati altrove sì:
        // quello è un giro, e «indietro» deve poterlo ripercorrere
        storia.push(pos("b.rs", 2));
        storia.push(pos("a.rs", 1));
        assert_eq!(storia.len(), 3);
    }

    /// I recenti sono la stessa pila letta all'indietro, **un file per
    /// percorso**: è quel che serve a Ctrl+E, e non costa una seconda
    /// struttura da tenere allineata.
    #[test]
    fn i_recenti_sono_i_file_distinti_dal_piu_recente() {
        let mut storia = CronologiaNavigazione::new();
        storia.push(pos("a.rs", 1));
        storia.push(pos("b.rs", 2));
        storia.push(pos("a.rs", 9));
        storia.push(pos("c.rs", 3));

        let recenti: Vec<(&str, usize)> = storia
            .recenti()
            .into_iter()
            .map(|posizione| {
                (
                    posizione.path.file_name().unwrap().to_str().unwrap(),
                    posizione.riga,
                )
            })
            .collect();
        assert_eq!(
            recenti,
            vec![("c.rs", 3), ("a.rs", 9), ("b.rs", 2)],
            "«a.rs» una volta sola, con la riga dell'ultima visita"
        );
    }

    /// Il tetto butta via le **più vecchie**: una cronologia che smette di
    /// registrare quando è piena sarebbe una cronologia che dimentica il
    /// presente invece del passato.
    #[test]
    fn oltre_il_tetto_cadono_le_piu_vecchie() {
        let mut storia = CronologiaNavigazione::new();
        for indice in 0..TETTO + 10 {
            storia.push(pos("a.rs", indice));
        }
        assert_eq!(storia.len(), TETTO);
        assert_eq!(storia.corrente(), Some(&pos("a.rs", TETTO + 9)));
        // e l'indice è rimasto valido: si può ancora tornare indietro
        assert_eq!(storia.indietro(), Some(&pos("a.rs", TETTO + 8)));
    }

    /// Un file che non c'è più esce dalla cronologia: «torna dov'eri» su un
    /// percorso cancellato è una promessa che non si può mantenere.
    #[test]
    fn un_file_dimenticato_esce_e_lascia_lindice_valido() {
        let mut storia = CronologiaNavigazione::new();
        storia.push(pos("a.rs", 1));
        storia.push(pos("b.rs", 2));
        storia.push(pos("a.rs", 3));
        storia.push(pos("c.rs", 4));

        storia.forget(std::path::Path::new("C:/repo/a.rs"));
        assert_eq!(storia.len(), 2);
        assert_eq!(storia.corrente(), Some(&pos("c.rs", 4)));
        assert_eq!(storia.indietro(), Some(&pos("b.rs", 2)));
        assert_eq!(storia.indietro(), None);
    }
}
