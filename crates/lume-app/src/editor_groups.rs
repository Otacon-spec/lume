//! I gruppi dell'editor: le schede divise in due colonne affiancate.
//!
//! Modulo **puro** — solo `DocId`, niente `Workspace`, niente `iced` — quindi
//! si prova senza far girare l'app. Tutte le decisioni che il resto della
//! divisione dell'editor deve prendere («in quale gruppo apro?», «chi resta attivo dopo una
//! chiusura?») stanno qui e sono asseribili.
//!
//! **Due vincoli hanno deciso la forma.**
//!
//! 1. `indexing_slicing` è `warn` nei lint del workspace e clippy è a zero:
//!    un `Vec<EditorGroup>` con `gruppi[i]` sarebbe un warning a ogni accesso.
//!    Quindi due campi nominati, non una lista. `Option<EditorGroup>` per il
//!    secondo dice da sé che può non esserci, invece di affidarlo a una
//!    convenzione su `len()`.
//! 2. **Un documento sta in un gruppo solo.** `Workspace::open` deduplica per
//!    path e la selezione vive sul `Document`, non sul widget: lo stesso
//!    `DocId` aperto in due gruppi condividerebbe il cursore, e sarebbe un bug
//!    visibile. L'invariante semplifica anche tutto il resto — chiudere non ha
//!    bisogno di sapere il gruppo, lo si ritrova.

use lume_core::DocId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorKind {
    Code,
    Sql,
    Note(lume_notes::NoteId),
}

/// Quale dei due gruppi. Chiuso a due di proposito: il piano dichiara fuori
/// ambito la divisione in tre o la divisione verticale.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GroupId {
    #[default]
    Primary,
    Secondary,
}

impl GroupId {
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Primary => Self::Secondary,
            Self::Secondary => Self::Primary,
        }
    }
}

/// Una colonna: le sue schede in ordine, e quale è attiva.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EditorGroup {
    tabs: Vec<DocId>,
    active: Option<DocId>,
}

impl EditorGroup {
    pub fn tabs(&self) -> &[DocId] {
        &self.tabs
    }

    pub const fn active(&self) -> Option<DocId> {
        self.active
    }

    fn push(&mut self, id: DocId) {
        if !self.tabs.contains(&id) {
            self.tabs.push(id);
        }
        self.active = Some(id);
    }

    /// Toglie la scheda e sceglie chi le succede. `true` se c'era.
    ///
    /// Succede **la precedente**, non l'ultima della lista: chiudere in mezzo
    /// a cinque schede e ritrovarsi in fondo è disorientante.
    fn remove(&mut self, id: DocId) -> bool {
        let Some(index) = self.tabs.iter().position(|&tab| tab == id) else {
            return false;
        };
        self.tabs.remove(index);
        if self.active == Some(id) {
            self.active = self
                .tabs
                .get(index.saturating_sub(1))
                .or_else(|| self.tabs.last())
                .copied();
        }
        true
    }
}

/// Lo stato delle schede dell'editor: uno o due gruppi, e dove sta il fuoco.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EditorGroups {
    primary: EditorGroup,
    secondary: Option<EditorGroup>,
    focus: GroupId,
}

impl EditorGroups {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn focus(&self) -> GroupId {
        self.focus
    }

    pub const fn is_split(&self) -> bool {
        self.secondary.is_some()
    }

    pub const fn group(&self, group: GroupId) -> Option<&EditorGroup> {
        match group {
            GroupId::Primary => Some(&self.primary),
            GroupId::Secondary => self.secondary.as_ref(),
        }
    }

    /// Le schede di un gruppo; vuote se quel gruppo non c'è.
    pub fn tabs(&self, group: GroupId) -> &[DocId] {
        match self.group(group) {
            Some(group) => group.tabs(),
            None => &[],
        }
    }

    pub fn active_in(&self, group: GroupId) -> Option<DocId> {
        self.group(group).and_then(EditorGroup::active)
    }

    /// Il documento attivo **del gruppo col fuoco**: è quello che l'editor
    /// mostra e su cui agiscono Ctrl+S, la ricerca, i segnalibri.
    pub fn active(&self) -> Option<DocId> {
        self.active_in(self.focus)
    }

    pub fn group_of(&self, id: DocId) -> Option<GroupId> {
        if self.primary.tabs.contains(&id) {
            return Some(GroupId::Primary);
        }
        self.secondary
            .as_ref()
            .filter(|group| group.tabs.contains(&id))
            .map(|_| GroupId::Secondary)
    }

    pub fn contains(&self, id: DocId) -> bool {
        self.group_of(id).is_some()
    }

    /// Tutte le schede aperte, gruppo 0 prima. Serve a chi deve visitarle
    /// tutte senza sapere dove stanno: «Salva tutto», il watcher, la sessione.
    pub fn all(&self) -> impl Iterator<Item = DocId> + '_ {
        self.primary
            .tabs
            .iter()
            .copied()
            .chain(self.tabs(GroupId::Secondary).iter().copied())
    }

    fn group_mut(&mut self, group: GroupId) -> Option<&mut EditorGroup> {
        match group {
            GroupId::Primary => Some(&mut self.primary),
            GroupId::Secondary => self.secondary.as_mut(),
        }
    }

    /// Apre (o riporta in primo piano) un documento.
    ///
    /// Se è **già aperto altrove** non lo si duplica: si va a prenderlo dov'è,
    /// spostandoci anche il fuoco. È l'invariante «un documento, un gruppo»
    /// che parla: duplicarlo condividerebbe il cursore fra le due viste.
    pub fn open(&mut self, group: GroupId, id: DocId) {
        if self.activate(id) {
            return;
        }
        let group = if self.group(group).is_some() {
            group
        } else {
            GroupId::Primary
        };
        if let Some(target) = self.group_mut(group) {
            target.push(id);
        }
        self.focus = group;
    }

    /// Apre una scheda **senza attivarla e senza spostare il fuoco**.
    ///
    /// ATTENZIONE: Serve alla rinomina, e la differenza non è cosmetica: quella apre
    /// tutti i file che tocca, e con `open` una rinomina su dodici file
    /// lascerebbe l'utente sull'ultimo, che non è dov'era e non è dove voleva
    /// andare. Un documento già aperto resta dov'è, com'è.
    pub fn open_quietly(&mut self, group: GroupId, id: DocId) {
        if self.contains(id) {
            return;
        }
        let group = if self.group(group).is_some() {
            group
        } else {
            GroupId::Primary
        };
        if let Some(target) = self.group_mut(group)
            && !target.tabs.contains(&id)
        {
            target.tabs.push(id);
        }
    }

    /// Rende attiva una scheda **dov'è**, e ci porta il fuoco.
    pub fn activate(&mut self, id: DocId) -> bool {
        let Some(group) = self.group_of(id) else {
            return false;
        };
        if let Some(target) = self.group_mut(group) {
            target.active = Some(id);
        }
        self.focus = group;
        true
    }

    /// Chiude una scheda, ovunque sia. `true` se c'era.
    ///
    /// Se il gruppo 1 resta vuoto sparisce: una colonna senza schede sarebbe
    /// solo spazio rubato all'altra, e la maniglia non avrebbe niente da
    /// ridimensionare.
    pub fn close(&mut self, id: DocId) -> bool {
        let Some(group) = self.group_of(id) else {
            return false;
        };
        let removed = self.group_mut(group).is_some_and(|g| g.remove(id));
        if self
            .secondary
            .as_ref()
            .is_some_and(|group| group.tabs.is_empty())
        {
            self.secondary = None;
            self.focus = GroupId::Primary;
        }
        removed
    }

    /// «Dividi a destra»: sposta la scheda attiva nell'altro gruppo.
    ///
    /// **Sposta, non copia** — l'invariante di questo modulo. Se il gruppo col
    /// fuoco non ha nulla da dare, non succede niente: dividere per mostrare
    /// una colonna vuota non è un comando, è un buco.
    pub fn split(&mut self) -> bool {
        let Some(id) = self.active() else {
            return false;
        };
        let target = self.focus.other();
        if self.group(target).is_none() {
            self.secondary = Some(EditorGroup::default());
        }
        // solo dopo aver creato il gruppo: `move_to` rifiuta un gruppo assente
        self.move_to(target, id)
    }

    /// Sposta una scheda in un gruppo, portandoci il fuoco.
    ///
    /// Rifiuta se il gruppo di destinazione non esiste — chi vuole crearlo
    /// passa da [`EditorGroups::split`], che è il comando che lo dichiara.
    pub fn move_to(&mut self, group: GroupId, id: DocId) -> bool {
        self.move_before(group, id, None)
    }

    /// Come [`EditorGroups::move_to`], ma inserendo **prima** di una scheda.
    /// `before: None` mette in coda; una scheda che non c'è vale come coda.
    pub fn move_before(&mut self, group: GroupId, id: DocId, before: Option<DocId>) -> bool {
        let Some(from) = self.group_of(id) else {
            return false;
        };
        if self.group(group).is_none() {
            return false;
        }
        if let Some(source) = self.group_mut(from) {
            source.remove(id);
        }
        if let Some(target) = self.group_mut(group) {
            // l'indice si cerca **dopo** la rimozione: cercarlo prima
            // sposterebbe di uno ogni riordino dentro lo stesso gruppo
            match before.and_then(|before| target.tabs.iter().position(|&tab| tab == before)) {
                Some(index) => {
                    target.tabs.insert(index, id);
                    target.active = Some(id);
                }
                None => target.push(id),
            }
        }
        self.focus = group;
        // spostare l'ultima scheda fuori dal gruppo 1 lo fa sparire
        if self
            .secondary
            .as_ref()
            .is_some_and(|group| group.tabs.is_empty())
        {
            self.secondary = None;
            self.focus = GroupId::Primary;
        }
        true
    }

    /// «Chiudi divisione»: **fonde**, non butta.
    ///
    /// Il prototipo «chiude il gruppo 1». In un IDE vero vorrebbe dire far
    /// sparire documenti sporchi senza chiedere. Sotto l'invariante «un
    /// documento, un gruppo» la fusione non ha nemmeno duplicati da gestire.
    pub fn unsplit(&mut self) -> bool {
        let Some(group) = self.secondary.take() else {
            return false;
        };
        let torna_attivo = (self.focus == GroupId::Secondary)
            .then_some(group.active)
            .flatten();
        for id in group.tabs {
            self.primary.tabs.push(id);
        }
        if let Some(id) = torna_attivo.or(self.primary.active) {
            self.primary.active = Some(id);
        } else {
            self.primary.active = self.primary.tabs.last().copied();
        }
        self.focus = GroupId::Primary;
        true
    }

    /// Ricostruisce lo stato dalla sessione salvata, senza far finta che sia
    /// una sequenza di gesti: il ripristino non è una transizione.
    pub fn restore(
        &mut self,
        primary: Vec<DocId>,
        primary_active: Option<DocId>,
        secondary: Option<(Vec<DocId>, Option<DocId>)>,
        focus_secondary: bool,
    ) {
        let ripulisci = |tabs: Vec<DocId>, active: Option<DocId>| {
            let mut viste = Vec::with_capacity(tabs.len());
            for id in tabs {
                if !viste.contains(&id) {
                    viste.push(id);
                }
            }
            let active = active
                .filter(|id| viste.contains(id))
                .or_else(|| viste.last().copied());
            EditorGroup {
                tabs: viste,
                active,
            }
        };
        self.primary = ripulisci(primary, primary_active);
        // un documento in entrambi i gruppi violerebbe l'invariante: vince il
        // gruppo 0, che è quello che anche un binario vecchio sa leggere
        self.secondary = secondary
            .map(|(tabs, active)| {
                let tabs = tabs
                    .into_iter()
                    .filter(|id| !self.primary.tabs.contains(id))
                    .collect();
                ripulisci(tabs, active)
            })
            .filter(|group| !group.tabs.is_empty());
        self.focus = if focus_secondary && self.secondary.is_some() {
            GroupId::Secondary
        } else {
            GroupId::Primary
        };
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    /// Qui servono identità distinte, non documenti veri.
    fn doc(n: u64) -> DocId {
        DocId(n)
    }

    /// L'invariante che regge tutto il resto: lo stesso documento non può
    /// stare in due gruppi. Se ci stesse, le due viste condividerebbero il
    /// cursore, che vive sul `Document` e non sul widget.
    #[test]
    fn un_documento_sta_in_un_gruppo_solo() {
        let mut gruppi = EditorGroups::new();
        gruppi.open(GroupId::Primary, doc(1));
        gruppi.open(GroupId::Primary, doc(2));
        assert!(gruppi.split(), "divide portando via la scheda attiva");

        assert_eq!(gruppi.tabs(GroupId::Primary), [doc(1)]);
        assert_eq!(gruppi.tabs(GroupId::Secondary), [doc(2)]);
        assert_eq!(gruppi.focus(), GroupId::Secondary);

        // riaprirlo nel gruppo 0 non lo duplica: va a prenderlo dov'è
        gruppi.open(GroupId::Primary, doc(2));
        assert_eq!(gruppi.tabs(GroupId::Primary), [doc(1)]);
        assert_eq!(gruppi.tabs(GroupId::Secondary), [doc(2)]);
        assert_eq!(gruppi.focus(), GroupId::Secondary);
        assert_eq!(gruppi.all().count(), 2);
    }

    /// Dividere con una sola scheda lascerebbe il gruppo 0 vuoto: si divide
    /// lo stesso, ed è il gruppo 0 a restare senza. È quel che fa un IDE —
    /// il vuoto ha il suo stato, e la fusione lo ricompone.
    #[test]
    fn dividere_senza_niente_da_dare_non_fa_niente() {
        let mut gruppi = EditorGroups::new();
        assert!(!gruppi.split(), "niente da dividere");
        assert!(!gruppi.is_split());

        gruppi.open(GroupId::Primary, doc(1));
        assert!(gruppi.split());
        assert!(gruppi.tabs(GroupId::Primary).is_empty());
        assert_eq!(gruppi.tabs(GroupId::Secondary), [doc(1)]);
    }

    /// Chiudere sceglie **la precedente**, non l'ultima della lista:
    /// ritrovarsi in fondo dopo aver chiuso la terza di cinque schede è
    /// disorientante.
    #[test]
    fn chiudere_passa_alla_scheda_precedente() {
        let mut gruppi = EditorGroups::new();
        for n in 1..=4 {
            gruppi.open(GroupId::Primary, doc(n));
        }
        gruppi.activate(doc(3));

        assert!(gruppi.close(doc(3)));

        assert_eq!(gruppi.tabs(GroupId::Primary), [doc(1), doc(2), doc(4)]);
        assert_eq!(gruppi.active(), Some(doc(2)));

        // la prima non ha una precedente: succede quella che le è scivolata
        // sotto, non il fondo della lista
        gruppi.activate(doc(1));
        assert!(gruppi.close(doc(1)));
        assert_eq!(gruppi.active(), Some(doc(2)));
        assert!(
            !gruppi.close(doc(1)),
            "chiudere due volte non inventa nulla"
        );
    }

    /// Il gruppo 1 svuotato sparisce, comunque lo si svuoti: chiudendo
    /// l'ultima scheda o trascinandola via. Una colonna vuota è solo spazio
    /// rubato all'altra.
    #[test]
    fn il_gruppo_svuotato_sparisce_e_il_fuoco_torna() {
        let mut gruppi = EditorGroups::new();
        gruppi.open(GroupId::Primary, doc(1));
        gruppi.open(GroupId::Primary, doc(2));
        gruppi.split();

        assert!(gruppi.close(doc(2)));
        assert!(!gruppi.is_split());
        assert_eq!(gruppi.focus(), GroupId::Primary);
        assert_eq!(gruppi.active(), Some(doc(1)));

        gruppi.open(GroupId::Primary, doc(3));
        gruppi.split();
        assert!(gruppi.is_split());
        assert!(gruppi.move_to(GroupId::Primary, doc(3)));
        assert!(
            !gruppi.is_split(),
            "trascinata via l'ultima, il gruppo cade"
        );
        assert_eq!(gruppi.focus(), GroupId::Primary);
    }

    /// Trascinare una scheda la inserisce **dove è caduta**, dentro lo stesso
    /// gruppo come nell'altro. L'indice si cerca dopo la rimozione, o
    /// riordinare dentro un gruppo sposterebbe di uno.
    #[test]
    fn trascinare_inserisce_dove_e_caduta() {
        let mut gruppi = EditorGroups::new();
        for n in 1..=3 {
            gruppi.open(GroupId::Primary, doc(n));
        }
        // la terza davanti alla prima
        assert!(gruppi.move_before(GroupId::Primary, doc(3), Some(doc(1))));
        assert_eq!(gruppi.tabs(GroupId::Primary), [doc(3), doc(1), doc(2)]);
        assert_eq!(gruppi.active(), Some(doc(3)));

        // la prima in coda
        assert!(gruppi.move_before(GroupId::Primary, doc(3), None));
        assert_eq!(gruppi.tabs(GroupId::Primary), [doc(1), doc(2), doc(3)]);

        // ATTENZIONE: il caso che smaschera l'indice cercato troppo presto: la scheda
        // trascinata sta **prima** del bersaglio, quindi toglierla sposta
        // indietro di uno tutto quel che segue
        assert!(gruppi.move_before(GroupId::Primary, doc(1), Some(doc(3))));
        assert_eq!(gruppi.tabs(GroupId::Primary), [doc(2), doc(1), doc(3)]);

        // e attraverso i gruppi, in una posizione precisa
        gruppi.activate(doc(1));
        gruppi.split();
        gruppi.move_before(GroupId::Secondary, doc(2), None);
        assert_eq!(gruppi.tabs(GroupId::Secondary), [doc(1), doc(2)]);
        gruppi.move_before(GroupId::Secondary, doc(3), Some(doc(2)));
        assert_eq!(gruppi.tabs(GroupId::Secondary), [doc(1), doc(3), doc(2)]);
        assert!(gruppi.tabs(GroupId::Primary).is_empty());
    }

    /// «Chiudi divisione» **fonde**: far sparire schede sporche senza
    /// chiedere sarebbe il modo peggiore di chiudere una colonna.
    #[test]
    fn la_fusione_non_butta_via_niente() {
        let mut gruppi = EditorGroups::new();
        gruppi.open(GroupId::Primary, doc(1));
        gruppi.open(GroupId::Primary, doc(2));
        gruppi.split();
        gruppi.open(GroupId::Secondary, doc(3));
        assert_eq!(gruppi.focus(), GroupId::Secondary);

        assert!(gruppi.unsplit());

        assert_eq!(
            gruppi.tabs(GroupId::Primary),
            [doc(1), doc(2), doc(3)],
            "niente si perde e l'ordine resta"
        );
        assert!(!gruppi.is_split());
        assert_eq!(gruppi.focus(), GroupId::Primary);
        assert_eq!(
            gruppi.active(),
            Some(doc(3)),
            "chi aveva il fuoco resta attivo dopo la fusione"
        );
        assert!(!gruppi.unsplit(), "fondere due volte non fa niente");
    }

    /// Il ripristino non è una sequenza di gesti: si ricostruisce, e si
    /// ripulisce quel che non tornerebbe. Un binario vecchio che ha riscritto
    /// la sessione può aver messo lo stesso file in tutti e due i gruppi.
    #[test]
    fn il_ripristino_ripulisce_invece_di_fidarsi() {
        let mut gruppi = EditorGroups::new();
        gruppi.restore(
            vec![doc(1), doc(2), doc(1)],
            Some(doc(2)),
            Some((vec![doc(2), doc(3)], Some(doc(3)))),
            true,
        );

        assert_eq!(gruppi.tabs(GroupId::Primary), [doc(1), doc(2)]);
        assert_eq!(
            gruppi.tabs(GroupId::Secondary),
            [doc(3)],
            "il doppione lo tiene il gruppo 0"
        );
        assert_eq!(gruppi.focus(), GroupId::Secondary);

        // un gruppo 1 che resta vuoto dopo la ripulitura non si crea
        let mut gruppi = EditorGroups::new();
        gruppi.restore(vec![doc(1)], None, Some((vec![doc(1)], None)), true);
        assert!(!gruppi.is_split());
        assert_eq!(gruppi.focus(), GroupId::Primary);
        assert_eq!(gruppi.active(), Some(doc(1)), "un attivo si trova comunque");
    }
}
