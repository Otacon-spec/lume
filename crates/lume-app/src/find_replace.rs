//! Stato di «Trova» (Ctrl+F) e «Sostituisci» (Ctrl+R) nel documento aperto.
//!
//! Puro: non tocca mai `Document` né `Rope` in scrittura. Legge un `&Rope`
//! solo quando query o manopole cambiano — mai per frame, come il budget
//! prestazionale richiede. Chi possiede il documento (`Lume`, in `app.rs`)
//! applica `FindIntent` con `Document::apply(Transaction)`, cosicché Annulla
//! e Ripeti funzionino gratis: nessuna scrittura passa da qui.

use lume_core::{DocId, RopeMatches, SearchOptions, find_in_rope_capped};

#[derive(Debug, Clone)]
pub struct FindState {
    pub doc: DocId,
    pub options: SearchOptions,
    pub replace_mode: bool,
    pub replacement: String,
    pub matches: Vec<std::ops::Range<usize>>,
    /// Oltre `lume_core::MAX_FIND_MATCHES` ce ne sono altri: il contatore
    /// lo mostra col «+» e «Sostituisci tutto» lo dice (#8).
    pub truncated: bool,
    pub current: Option<usize>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindMessage {
    QueryChanged(String),
    ReplacementChanged(String),
    ToggleCase,
    ToggleWord,
    ToggleRegex,
    ToggleReplaceMode,
    Next,
    Previous,
    Replace,
    ReplaceAll,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindIntent {
    None,
    Closed,
    JumpTo(std::ops::Range<usize>),
    ReplaceOne(std::ops::Range<usize>, String),
    ReplaceAll {
        ranges: Vec<std::ops::Range<usize>>,
        replacement: String,
        truncated: bool,
    },
}

impl FindState {
    pub fn new(doc: DocId, initial_query: String, replace_mode: bool, rope: &ropey::Rope) -> Self {
        let options = SearchOptions::literal(initial_query);
        let (trovati, error) = run(&options, rope);
        Self {
            doc,
            current: (!trovati.ranges.is_empty()).then_some(0),
            matches: trovati.ranges,
            truncated: trovati.truncated,
            options,
            replace_mode,
            replacement: String::new(),
            error,
        }
    }

    /// Ricalcola match, troncamento ed errore sul rope attuale. Unico punto
    /// in cui `matches` e `truncated` cambiano: non possono sfasarsi.
    fn ricalcola(&mut self, rope: &ropey::Rope) {
        let (trovati, error) = run(&self.options, rope);
        self.matches = trovati.ranges;
        self.truncated = trovati.truncated;
        self.error = error;
    }

    pub fn update(&mut self, message: FindMessage, rope: &ropey::Rope) -> FindIntent {
        match message {
            FindMessage::QueryChanged(q) => {
                self.options.needle = q;
                self.recompute(rope)
            }
            FindMessage::ReplacementChanged(r) => {
                self.replacement = r;
                FindIntent::None
            }
            FindMessage::ToggleCase => {
                self.options.case_sensitive = match self.options.case_sensitive {
                    Some(true) => None,
                    _ => Some(true),
                };
                self.recompute(rope)
            }
            FindMessage::ToggleWord => {
                self.options.whole_word = !self.options.whole_word;
                self.recompute(rope)
            }
            FindMessage::ToggleRegex => {
                self.options.regex = !self.options.regex;
                self.recompute(rope)
            }
            FindMessage::ToggleReplaceMode => {
                self.replace_mode = !self.replace_mode;
                FindIntent::None
            }
            FindMessage::Next => self.step(1),
            FindMessage::Previous => self.step(-1),
            FindMessage::Replace => {
                // Ricalcola prima di sostituire: se il documento è cambiato da
                // fuori (es. un tasto arrivato all'editor, un reload, un edit LSP),
                // `self.matches` potrebbe non corrispondere più al rope attuale.
                self.ricalcola(rope);
                self.current = self
                    .current
                    .filter(|&i| i < self.matches.len())
                    .or_else(|| (!self.matches.is_empty()).then_some(0));
                match self.current.and_then(|i| self.matches.get(i)) {
                    Some(range) => FindIntent::ReplaceOne(range.clone(), self.replacement.clone()),
                    None => FindIntent::None,
                }
            }
            FindMessage::ReplaceAll => {
                self.ricalcola(rope);
                if self.matches.is_empty() {
                    self.current = None;
                    FindIntent::None
                } else {
                    FindIntent::ReplaceAll {
                        ranges: self.matches.clone(),
                        replacement: self.replacement.clone(),
                        truncated: self.truncated,
                    }
                }
            }
            FindMessage::Close => FindIntent::Closed,
        }
    }

    /// Ricalcola i match e riparte dal primo — lo stesso comportamento di
    /// «cerca mentre digiti» del pannello Cerca nei file.
    fn recompute(&mut self, rope: &ropey::Rope) -> FindIntent {
        self.ricalcola(rope);
        self.current = (!self.matches.is_empty()).then_some(0);
        match self.current.and_then(|i| self.matches.get(i)) {
            Some(range) => FindIntent::JumpTo(range.clone()),
            None => FindIntent::None,
        }
    }

    /// Dopo una sostituzione singola andata a buon fine, ricalcola i match
    /// sul rope aggiornato e sceglie come "corrente" il primo che comincia
    /// da `da_posizione` in poi (con l'a capo se non ce n'è uno più avanti)
    /// — così «Sostituisci» premuto di nuovo avanza all'occorrenza
    /// successiva invece di ripetere sempre la prima del file. Ritorna il
    /// range su cui spostare la selezione, se c'è ancora un match.
    pub fn recompute_dopo_sostituzione(
        &mut self,
        rope: &ropey::Rope,
        da_posizione: usize,
    ) -> Option<std::ops::Range<usize>> {
        self.ricalcola(rope);
        self.current = self
            .matches
            .iter()
            .position(|m| m.start >= da_posizione)
            .or_else(|| (!self.matches.is_empty()).then_some(0));
        self.current.and_then(|i| self.matches.get(i)).cloned()
    }

    /// `delta` è +1 (Successivo) o -1 (Precedente); gira con l'a capo.
    fn step(&mut self, delta: i32) -> FindIntent {
        if self.matches.is_empty() {
            return FindIntent::None;
        }
        let len = self.matches.len() as i32;
        let base = self.current.unwrap_or(0) as i32;
        let next = (base + delta).rem_euclid(len) as usize;
        self.current = Some(next);
        #[allow(clippy::indexing_slicing)]
        // next è sempre < matches.len() per costruzione: rem_euclid garantisce 0..len
        FindIntent::JumpTo(self.matches[next].clone())
    }
}

fn run(options: &SearchOptions, rope: &ropey::Rope) -> (RopeMatches, Option<String>) {
    match find_in_rope_capped(rope, options) {
        Ok(trovati) => (trovati, None),
        Err(e) => (RopeMatches::default(), Some(e)),
    }
}

/// «corrente/totale», col «+» quando il totale è un minimo: dire «5000» a
/// secco su un file che ne ha di più sarebbe la stessa bugia comoda che
/// `riepilogo` in `tools/search_tool.rs` evita.
pub(crate) fn contatore(state: &FindState) -> String {
    let piu = if state.truncated { "+" } else { "" };
    match (state.current, state.matches.len()) {
        (_, 0) => "0/0".to_owned(),
        (Some(i), n) => format!("{}/{n}{piu}", i + 1),
        (None, n) => format!("0/{n}{piu}"),
    }
}

pub const INPUT_ID: &str = "trova-nel-file";

pub fn view<'a>(
    state: &'a FindState,
    t: &'a lume_theme::LumeTheme,
) -> iced::Element<'a, FindMessage> {
    use iced::widget::{button, column, container, row, text, text_input};
    use iced::{Alignment, Length, Padding};

    let conteggio = contatore(state);

    let query_row = row![
        text_input("Trova…", &state.options.needle)
            .id(INPUT_ID)
            .on_input(FindMessage::QueryChanged)
            .on_submit(FindMessage::Next)
            .size(12.5)
            .padding(Padding::from([5.0, 8.0]))
            .style(lume_ui::styles::input(t))
            .width(Length::Fixed(200.0)),
        chip(
            t,
            "Aa",
            state.options.case_sensitive == Some(true),
            FindMessage::ToggleCase
        ),
        chip(
            t,
            "parola",
            state.options.whole_word,
            FindMessage::ToggleWord
        ),
        chip(t, ".*", state.options.regex, FindMessage::ToggleRegex),
        text(conteggio).size(11).color(t.muted),
        button(text("↑").size(11).color(t.sec))
            .style(lume_ui::styles::hover_row(t.sec, t.hover2, None, 4.0))
            .on_press(FindMessage::Previous),
        button(text("↓").size(11).color(t.sec))
            .style(lume_ui::styles::hover_row(t.sec, t.hover2, None, 4.0))
            .on_press(FindMessage::Next),
        button(
            text(if state.replace_mode { "▾" } else { "▸" })
                .size(11)
                .color(t.sec)
        )
        .style(lume_ui::styles::hover_row(t.sec, t.hover2, None, 4.0))
        .on_press(FindMessage::ToggleReplaceMode),
        button(text("✕").size(11).color(t.sec))
            .style(lume_ui::styles::hover_row(t.sec, t.hover2, None, 4.0))
            .on_press(FindMessage::Close),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let mut colonna = column![query_row].spacing(4);

    if let Some(errore) = &state.error {
        colonna = colonna.push(text(errore).size(11).color(t.bad));
    }

    if state.replace_mode {
        colonna = colonna.push(
            row![
                text_input("Sostituisci con…", &state.replacement)
                    .on_input(FindMessage::ReplacementChanged)
                    .on_submit(FindMessage::Replace)
                    .size(12.5)
                    .padding(Padding::from([5.0, 8.0]))
                    .style(lume_ui::styles::input(t))
                    .width(Length::Fixed(200.0)),
                button(text("Sostituisci").size(11).color(t.sec))
                    .style(lume_ui::styles::hover_row(t.sec, t.hover2, None, 4.0))
                    .on_press(FindMessage::Replace),
                button(text("Sostituisci tutto").size(11).color(t.sec))
                    .style(lume_ui::styles::hover_row(t.sec, t.hover2, None, 4.0))
                    .on_press(FindMessage::ReplaceAll),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
    }

    container(colonna)
        .padding(10)
        .style(move |_| iced::widget::container::Style {
            background: Some(iced::Background::Color(t.surface)),
            border: iced::Border {
                color: t.border,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// Manopola on/off — stessa faccia di `tools/search_tool.rs::chip`, ma per
/// `FindMessage`: non condivisibile senza generalizzare quella, e i due
/// pannelli restano piccoli abbastanza da non doverlo fare.
fn chip<'a>(
    t: &lume_theme::LumeTheme,
    label: &'a str,
    attivo: bool,
    message: FindMessage,
) -> iced::Element<'a, FindMessage> {
    use iced::Padding;
    use iced::widget::{button, text};

    let colore = if attivo { t.accent } else { t.sec };
    button(text(label).size(10.5).color(colore))
        .padding(Padding::from([2, 6]))
        .style(lume_ui::styles::hover_row(
            colore,
            t.hover2,
            attivo.then_some(t.sel),
            3.0,
        ))
        .on_press(message)
        .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use lume_core::DocId;
    use ropey::Rope;

    fn stato(testo: &str, query: &str) -> (FindState, Rope) {
        let rope = Rope::from_str(testo);
        let stato = FindState::new(DocId(1), query.to_owned(), false, &rope);
        (stato, rope)
    }

    #[test]
    fn aprire_con_una_query_calcola_subito_i_match_e_seleziona_il_primo() {
        let (stato, _rope) = stato("uno due uno", "uno");
        assert_eq!(stato.matches, vec![0..3, 8..11]);
        assert_eq!(stato.current, Some(0));
    }

    #[test]
    fn query_vuota_non_ha_match_ne_corrente() {
        let (stato, _rope) = stato("uno due uno", "");
        assert!(stato.matches.is_empty());
        assert_eq!(stato.current, None);
    }

    #[test]
    fn digitare_ricalcola_e_riparte_dal_primo_match() {
        let (mut stato, rope) = stato("alfa beta alfa", "beta");
        assert_eq!(stato.matches, vec![5..9]);
        let intento = stato.update(FindMessage::QueryChanged("alfa".into()), &rope);
        assert_eq!(stato.matches, vec![0..4, 10..14]);
        assert_eq!(stato.current, Some(0));
        assert_eq!(intento, FindIntent::JumpTo(0..4));
    }

    #[test]
    fn successivo_e_precedente_girano_con_a_capo() {
        let (mut stato, rope) = stato("a a a", "a");
        assert_eq!(stato.matches.len(), 3);
        assert_eq!(stato.current, Some(0));
        assert_eq!(
            stato.update(FindMessage::Next, &rope),
            FindIntent::JumpTo(2..3)
        );
        assert_eq!(stato.current, Some(1));
        assert_eq!(
            stato.update(FindMessage::Next, &rope),
            FindIntent::JumpTo(4..5)
        );
        assert_eq!(
            stato.update(FindMessage::Next, &rope),
            FindIntent::JumpTo(0..1),
            "dall'ultimo si torna al primo"
        );
        assert_eq!(
            stato.update(FindMessage::Previous, &rope),
            FindIntent::JumpTo(4..5),
            "dal primo, indietro, si va all'ultimo"
        );
    }

    #[test]
    fn manopole_ricalcolano_e_preservano_la_query() {
        let (mut stato, rope) = stato("Main main", "main");
        assert_eq!(
            stato.matches.len(),
            2,
            "smart-case: minuscolo trova entrambi"
        );
        let _ = stato.update(FindMessage::ToggleCase, &rope);
        assert_eq!(stato.options.case_sensitive, Some(true));
        assert_eq!(
            stato.matches,
            vec![5..9],
            "solo il minuscolo, ora sensibile"
        );
    }

    #[test]
    fn regex_non_valida_riempie_error_e_svuota_i_match() {
        let (mut stato, rope) = stato("testo", "testo");
        let _ = stato.update(FindMessage::ToggleRegex, &rope);
        let _ = stato.update(FindMessage::QueryChanged("(".into()), &rope);
        assert!(stato.matches.is_empty());
        assert!(stato.error.is_some());
    }

    #[test]
    fn toggle_replace_mode_apre_e_chiude_il_campo_sostituisci() {
        let (mut stato, rope) = stato("uno", "uno");
        assert!(!stato.replace_mode);
        let _ = stato.update(FindMessage::ToggleReplaceMode, &rope);
        assert!(stato.replace_mode);
    }

    #[test]
    fn replace_produce_lintento_con_solo_il_match_corrente() {
        let (mut stato, rope) = stato("uno due uno", "uno");
        let _ = stato.update(FindMessage::ReplacementChanged("tre".into()), &rope);
        let intento = stato.update(FindMessage::Replace, &rope);
        assert_eq!(intento, FindIntent::ReplaceOne(0..3, "tre".to_owned()));
    }

    #[test]
    fn replace_senza_match_e_un_none_non_un_panic() {
        let (mut stato, rope) = stato("niente qui", "assente");
        assert_eq!(stato.update(FindMessage::Replace, &rope), FindIntent::None);
    }

    #[test]
    fn replace_all_porta_tutti_i_match_e_la_sostituzione() {
        let (mut stato, rope) = stato("a a a", "a");
        let _ = stato.update(FindMessage::ReplacementChanged("b".into()), &rope);
        let intento = stato.update(FindMessage::ReplaceAll, &rope);
        assert_eq!(
            intento,
            FindIntent::ReplaceAll {
                ranges: vec![0..1, 2..3, 4..5],
                replacement: "b".to_owned(),
                truncated: false,
            }
        );
    }

    #[test]
    fn replace_all_oltre_il_tetto_dice_che_ha_troncato() {
        let testo = "a ".repeat(lume_core::MAX_FIND_MATCHES + 1);
        let (mut stato, rope) = stato(&testo, "a");
        assert!(stato.truncated);
        let FindIntent::ReplaceAll {
            ranges, truncated, ..
        } = stato.update(FindMessage::ReplaceAll, &rope)
        else {
            panic!("atteso ReplaceAll");
        };
        assert_eq!(ranges.len(), lume_core::MAX_FIND_MATCHES);
        assert!(truncated);
    }

    #[test]
    fn il_contatore_dice_quando_i_match_sono_di_piu() {
        let testo = "a ".repeat(lume_core::MAX_FIND_MATCHES + 1);
        let (troncato, _) = stato(&testo, "a");
        assert_eq!(
            contatore(&troncato),
            format!("1/{}+", lume_core::MAX_FIND_MATCHES)
        );
        let (esatto, _) = stato("a a a", "a");
        assert_eq!(contatore(&esatto), "1/3");
        let (vuoto, _) = stato("niente", "assente");
        assert_eq!(contatore(&vuoto), "0/0");
    }

    #[test]
    fn replace_all_senza_match_non_produce_intento() {
        let (mut stato, rope) = stato("niente qui", "assente");
        assert_eq!(
            stato.update(FindMessage::ReplaceAll, &rope),
            FindIntent::None,
            "zero match: nessun gradino di undo vuoto"
        );
    }

    #[test]
    fn sostituisci_ricalcola_contro_il_rope_attuale_non_usa_match_stantii() {
        let vecchio = Rope::from_str("uno due uno");
        let mut stato = FindState::new(DocId(1), "uno".to_owned(), false, &vecchio);
        let _ = stato.update(FindMessage::ReplacementChanged("tre".into()), &vecchio);
        // il documento cambia altrove (es. un tasto arrivato all'editor invece
        // che al riquadro) senza che FindState lo sappia ancora
        let nuovo = Rope::from_str("zero uno due uno");
        let intento = stato.update(FindMessage::Replace, &nuovo);
        // il match usato deve essere quello vero nel rope nuovo (5..8), non
        // quello stantio del rope vecchio (0..3, che nel rope nuovo cadrebbe
        // dentro "zero " e sostituirebbe testo sbagliato)
        assert_eq!(intento, FindIntent::ReplaceOne(5..8, "tre".to_owned()));
    }

    #[test]
    fn recompute_dopo_sostituzione_avanza_alla_prossima_occorrenza() {
        let rope = Rope::from_str("a a a");
        let mut stato = FindState::new(DocId(1), "a".to_owned(), false, &rope);
        stato.current = Some(1); // l'utente ha navigato alla "a" di mezzo (2..3)
        // simulazione: quella "a" è stata sostituita con "bb" -> "a bb a"
        let dopo = Rope::from_str("a bb a");
        let prossimo = stato.recompute_dopo_sostituzione(&dopo, 2 + 2);
        assert_eq!(prossimo, Some(5..6));
        assert_eq!(stato.current, Some(1));
    }

    #[test]
    fn recompute_dopo_sostituzione_gira_con_a_capo_se_non_ce_n_e_uno_dopo() {
        let rope = Rope::from_str("a a");
        let mut stato = FindState::new(DocId(1), "a".to_owned(), false, &rope);
        stato.current = Some(1); // ultima occorrenza (2..3)
        // sostituita con "bb" -> "a bb"
        let dopo = Rope::from_str("a bb");
        let prossimo = stato.recompute_dopo_sostituzione(&dopo, 2 + 2);
        // nessun match con start >= 4 in "a bb" (solo 0..1) -> torna al primo
        assert_eq!(prossimo, Some(0..1));
        assert_eq!(stato.current, Some(0));
    }

    #[test]
    fn close_chiude_e_basta() {
        let (mut stato, rope) = stato("uno", "uno");
        assert_eq!(stato.update(FindMessage::Close, &rope), FindIntent::Closed);
    }

    /// Non verifica l'albero Iced (nessuno lo fa altrove nel repo — vedi
    /// `search.rs`/`structure.rs`, che testano solo la logica), ma garantisce
    /// che costruire la vista non vada in panic su nessuna combinazione di
    /// stato plausibile: lista vuota, errore, modalità Sostituisci.
    #[test]
    fn la_vista_non_va_in_panic_in_nessuna_combinazione_di_stato() {
        let rope = Rope::from_str("uno due uno");
        // `LumeTheme::builtin()` (in `lume-theme/src/tokens.rs`) restituisce i 5 temi
        // embedded, Officina per primo: nessun test nel repo costruisce
        // ancora un `LumeTheme` a mano, questo è il modo reale di averne uno.
        let tema = lume_theme::LumeTheme::builtin()
            .into_iter()
            .next()
            .expect("almeno il tema Officina deve essere valido");
        for query in ["", "uno", "("] {
            for replace_mode in [false, true] {
                let mut stato = FindState::new(DocId(1), query.to_owned(), replace_mode, &rope);
                if query == "(" {
                    let _ = stato.update(FindMessage::ToggleRegex, &rope);
                }
                let _ = view(&stato, &tema);
            }
        }
    }
}
