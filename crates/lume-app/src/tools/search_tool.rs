//! Tool «Cerca»: ricerca reale nei file del progetto (engine in lume-core:
//! walk con .gitignore + grep). Il pannello chiede il giro
//! (`ToolRequest::RunSearch`) e l'app lo riconsegna a pezzi: `Append` per ogni
//! batch, `Done` con i totali, `Failed` se l'espressione non compila,
//! `SenzaAmbito` se non c'è niente su cui cercare.
//!
//! Da non confondere: questo è il **pannello**, cioè la vista e le
//! manopole. Il motore è `lume-core/src/search.rs`; «Cerca ovunque»
//! (`lume-app/src/search.rs`) è un'altra cosa, un overlay che trova per nome.
//!
//! Le tre manopole del prototipo — maiuscole, parola intera, espressione
//! regolare — vivono qui e viaggiano dentro la richiesta: il motore non ha
//! stato, e il pannello è l'unico posto dove la scelta dell'utente esiste.
//!
//! ATTENZIONE: **Il tasto delle maiuscole ha due posizioni, non tre.** Il default è lo
//! *smart-case* (`case_sensitive: None`): sensibile solo se il testo cercato
//! contiene maiuscole, che è quel che serve quasi sempre. Premendolo si passa
//! a «sensibile» esplicito. Forzare l'**insensibile** su un testo con
//! maiuscole non si offre: sarebbe una terza posizione da distinguere a
//! colpo d'occhio in un tasto largo tre caratteri, e si ottiene lo stesso
//! risultato scrivendo il testo in minuscolo.

use iced::widget::{Space, button, column, container, row, text, text_input};
use iced::{Alignment, Element, Length, Padding};
use lume_core::{FileMatches, SearchArea, SearchOptions};
use lume_ui::MONO;
use lume_ui::icons::Icon;
use lume_ui::styles;
use lume_ui::tool::{Tool, ToolCtx, ToolEvent, ToolId, ToolRequest};

pub const ID: ToolId = ToolId("search");

/// Id del campo di ricerca: Ctrl+Maiusc+F apre il pannello **e** ci mette il
/// cursore dentro, o sarebbe una scorciatoia che chiede un clic per servire.
pub const INPUT_ID: &str = "cerca-nei-file";

#[derive(Debug, Clone)]
pub enum Event {
    Query(String),
    /// Maiusc: smart-case ↔ sensibile.
    ToggleCase,
    ToggleWord,
    ToggleRegex,
    /// Ambito di ricerca nel progetto.
    SetScope(SearchArea),
    /// Batch incrementale di risultati (streaming dal motore di ricerca).
    Append(Vec<FileMatches>),
    /// Il motore ha finito: i totali, e se il tetto li ha troncati.
    Done {
        files: usize,
        matches: usize,
        truncated: bool,
    },
    /// Ricerca non eseguibile (di norma: espressione regolare a metà).
    Failed(String),
    /// Non c'è **niente** su cui cercare: ambito «Progetto» e nessuna cartella
    /// aperta. È una condizione diversa da «zero risultati» e va detta, o il
    /// pannello resta com'era — nessun elenco, nessun riepilogo, nessun
    /// errore — e chi cerca non ha modo di capire perché non succede niente.
    SenzaAmbito,
    /// Mostra/nasconde la riga della sostituzione.
    ToggleReplace,
    ReplaceChanged(String),
    /// Primo clic: chiede conferma dicendo quanti file tocca.
    AskReplaceAll,
    /// Conferma vincolata alla domanda corrente, non un comando libero.
    ConfirmReplaceAll(u64),
    /// La radice cambia anche quando il pannello non cambia visibilità.
    ProjectChanged {
        visible: bool,
    },
    CancelReplaceAll,
    /// L'app ha finito di sostituire: i risultati mostrati sono vecchi di un
    /// istante e la ricerca riparte da sola.
    Replaced,
    HitClicked(std::path::PathBuf, usize),
}

/// Quel che il motore ha detto a fine giro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Esito {
    pub files: usize,
    pub matches: usize,
    pub truncated: bool,
}

pub struct SearchTool {
    options: SearchOptions,
    /// Ambito: Progetto, sorgenti (`src` nel path) o solo documenti aperti.
    area: SearchArea,
    results: Vec<FileMatches>,
    /// `None` finché il giro non è finito: è anche il modo di sapere che una
    /// ricerca è in corso.
    esito: Option<Esito>,
    /// Il motivo **già ridotto a una riga**, pronto da disegnare: i due che
    /// lo scrivono hanno forme diverse (quattro righe di `regex` contro una
    /// frase nostra) e la vista non deve sapere quale dei due sta mostrando.
    errore: Option<String>,
    /// La riga di sostituzione è nascosta finché non serve: chi cerca per
    /// leggere è la maggioranza dei casi.
    replace_on: bool,
    replacement: String,
    /// Conferma in linea, come «Ripristina» nel pannello Commit.
    pending_replace: bool,
    replace_request: u64,
}

impl SearchTool {
    pub fn new() -> Self {
        Self {
            options: SearchOptions::default(),
            area: SearchArea::default(),
            results: Vec::new(),
            esito: None,
            errore: None,
            replace_on: false,
            replacement: String::new(),
            pending_replace: false,
            replace_request: 0,
        }
    }

    /// Riparte da capo con le opzioni correnti.
    ///
    /// I risultati di prima sono comunque stantii — cambiare una manopola li
    /// invalida quanto cambiare il testo — quindi si buttano subito invece di
    /// lasciarli sotto gli occhi finché non arriva il primo batch nuovo.
    fn ricerca(&mut self) -> Vec<ToolRequest> {
        self.results.clear();
        self.esito = None;
        self.errore = None;
        self.pending_replace = false;
        if self.options.needle.trim().is_empty() {
            return Vec::new();
        }
        vec![ToolRequest::RunSearch {
            options: self.options.clone(),
            area: self.area,
        }]
    }

    /// Il campo della sostituzione, e il tasto che scrive davvero.
    ///
    /// ATTENZIONE: Il tasto **non c'è** finché la ricerca non è finita. Sostituire su
    /// una lista che sta ancora arrivando vorrebbe dire riscrivere i file
    /// trovati nei primi due decimi di secondo e lasciare gli altri: un
    /// «Sostituisci tutto» che non sostituisce tutto.
    fn riga_sostituzione<'a>(&'a self, t: &lume_theme::LumeTheme) -> Element<'a, ToolEvent> {
        if self.pending_replace {
            let domanda = self.esito.map(domanda_conferma).unwrap_or_default();
            return row![
                text(domanda).size(11).color(t.warn),
                Space::new().width(Length::Fill),
                button(text("Sostituisci").size(11).color(t.bad))
                    .padding(Padding::from([3, 8]))
                    .style(styles::hover_row(t.bad, t.hover, None, 4.0))
                    .on_press(ToolEvent::new(Event::ConfirmReplaceAll(
                        self.replace_request
                    ))),
                button(text("Annulla").size(11).color(t.sec))
                    .padding(Padding::from([3, 8]))
                    .style(styles::hover_row(t.sec, t.hover, None, 4.0))
                    .on_press(ToolEvent::new(Event::CancelReplaceAll)),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into();
        }

        let campo = text_input("Sostituisci con…", &self.replacement)
            .on_input(|testo| ToolEvent::new(Event::ReplaceChanged(testo)))
            .size(12)
            .padding(Padding::from([5.0, 8.0]))
            .style(styles::input(t));
        let pronto = self.esito.is_some_and(|esito| esito.matches > 0);
        let mut tasto =
            button(
                text("Sostituisci tutto")
                    .size(11)
                    .color(if pronto { t.sec } else { t.faint }),
            )
            .padding(Padding::from([3, 8]))
            .style(styles::hover_row(
                if pronto { t.sec } else { t.faint },
                t.hover,
                None,
                4.0,
            ));
        if pronto {
            tasto = tasto.on_press(ToolEvent::new(Event::AskReplaceAll));
        }
        row![campo, tasto]
            .spacing(6)
            .align_y(Alignment::Center)
            .into()
    }
}

impl Tool for SearchTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Cerca"
    }

    fn icon(&self) -> Icon {
        Icon::Search
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;

        let input = text_input("Cerca nei file…", &self.options.needle)
            .id(INPUT_ID)
            .on_input(|q| ToolEvent::new(Event::Query(q)))
            .size(12)
            .padding(Padding::from([5.0, 8.0]))
            .style(styles::input(t));

        let manopole = row![
            chip(
                t,
                "Aa",
                self.options.case_sensitive == Some(true),
                Event::ToggleCase
            ),
            chip(t, "parola", self.options.whole_word, Event::ToggleWord),
            chip(t, ".*", self.options.regex, Event::ToggleRegex),
            chip(t, "sostituisci", self.replace_on, Event::ToggleReplace),
            Space::new().width(Length::Fill),
            chip(
                t,
                "Progetto",
                self.area == SearchArea::Progetto,
                Event::SetScope(SearchArea::Progetto),
            ),
            chip(
                t,
                "src",
                self.area == SearchArea::Sorgenti,
                Event::SetScope(SearchArea::Sorgenti),
            ),
            chip(
                t,
                "Aperti",
                self.area == SearchArea::Aperti,
                Event::SetScope(SearchArea::Aperti),
            ),
        ]
        .spacing(3)
        .align_y(Alignment::Center);

        let mut list = column![];
        for fm in &self.results {
            let name = fm
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let dir = fm
                .path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            list = list.push(
                row![
                    text(name).size(12).color(t.tree),
                    text(format!("· {dir} · {}", fm.hits.len()))
                        .size(12)
                        .color(t.muted),
                ]
                .spacing(4)
                .padding(Padding {
                    top: 6.0,
                    right: 0.0,
                    bottom: 2.0,
                    left: 0.0,
                }),
            );
            for hit in &fm.hits {
                let (line, range) = preview_around_match(&hit.line_text, &hit.range_in_line);
                let (pre, mat, post) = split_match(&line, &range);
                let hl = t.hl;
                list = list.push(
                    button(row![
                        text(pre).size(11).font(MONO).color(t.sec),
                        container(text(mat).size(11).font(MONO).color(t.warn)).style(move |_| {
                            iced::widget::container::Style {
                                background: Some(iced::Background::Color(hl)),
                                ..iced::widget::container::Style::default()
                            }
                        }),
                        text(post).size(11).font(MONO).color(t.sec),
                    ])
                    .padding(Padding {
                        top: 2.0,
                        right: 0.0,
                        bottom: 2.0,
                        left: 12.0,
                    })
                    .width(Length::Fill)
                    .style(styles::hover_row(t.sec, t.hover, None, 3.0))
                    .on_press(ToolEvent::new(Event::HitClicked(
                        fm.path.clone(),
                        hit.line_number as usize,
                    ))),
                );
            }
        }

        let mut colonna = column![input, manopole].spacing(6);
        if self.replace_on {
            colonna = colonna.push(self.riga_sostituzione(t));
        }
        if let Some(errore) = &self.errore {
            colonna = colonna.push(text(errore).size(11).color(t.bad));
        } else if let Some(esito) = self.esito {
            colonna = colonna.push(text(riepilogo(esito)).size(11).color(t.muted));
        }

        column![
            colonna,
            iced::widget::scrollable(list)
                .width(Length::Fill)
                .height(Length::Fill),
            Space::new().height(0),
        ]
        .spacing(8)
        .padding(Padding {
            top: 2.0,
            right: 12.0,
            bottom: 12.0,
            left: 12.0,
        })
        .into()
    }

    /// Chiuso: risultati liberati e ricerca fermata. Riaperto: la ricerca
    /// riparte con le stesse opzioni (senza testo `ricerca()` non chiede
    /// niente), altrimenti `esito: None` lascerebbe il pannello «in corso».
    fn on_visibility_changed(&mut self, open: bool) -> Vec<ToolRequest> {
        if open {
            return self.ricerca();
        }
        self.results = Vec::new();
        self.esito = None;
        self.errore = None;
        self.pending_replace = false;
        vec![ToolRequest::CancelSearch]
    }

    fn update(&mut self, ev: &ToolEvent) -> Vec<ToolRequest> {
        // (la riga di sostituzione sta in `SearchTool::riga_sostituzione`)
        match ev.downcast::<Event>() {
            Some(Event::Query(q)) => {
                self.options.needle = q.clone();
                self.ricerca()
            }
            Some(Event::ToggleCase) => {
                self.options.case_sensitive = match self.options.case_sensitive {
                    Some(true) => None,
                    _ => Some(true),
                };
                self.ricerca()
            }
            Some(Event::ToggleWord) => {
                self.options.whole_word = !self.options.whole_word;
                self.ricerca()
            }
            Some(Event::ToggleRegex) => {
                self.options.regex = !self.options.regex;
                self.ricerca()
            }
            Some(Event::SetScope(area)) => {
                if self.area == *area {
                    return Vec::new();
                }
                self.area = *area;
                self.ricerca()
            }
            Some(Event::ProjectChanged { visible }) => {
                self.results = Vec::new();
                self.esito = None;
                self.errore = None;
                self.pending_replace = false;
                if *visible { self.ricerca() } else { Vec::new() }
            }
            Some(Event::Append(batch)) => {
                self.pending_replace = false;
                self.results.extend(batch.iter().cloned());
                Vec::new()
            }
            Some(Event::Done {
                files,
                matches,
                truncated,
            }) => {
                self.pending_replace = false;
                self.esito = Some(Esito {
                    files: *files,
                    matches: *matches,
                    truncated: *truncated,
                });
                Vec::new()
            }
            Some(Event::Failed(motivo)) => {
                self.pending_replace = false;
                self.errore = Some(motivo_breve(motivo));
                Vec::new()
            }
            Some(Event::SenzaAmbito) => {
                self.pending_replace = false;
                self.errore = Some("Nessuna cartella aperta".to_owned());
                Vec::new()
            }
            Some(Event::ToggleReplace) => {
                self.replace_on = !self.replace_on;
                self.pending_replace = false;
                Vec::new()
            }
            Some(Event::ReplaceChanged(testo)) => {
                self.pending_replace = false;
                self.replacement = testo.clone();
                Vec::new()
            }
            Some(Event::AskReplaceAll) => {
                self.pending_replace = false;
                if self.esito.is_some_and(|esito| esito.matches > 0)
                    && let Some(next) = self.replace_request.checked_add(1)
                {
                    self.replace_request = next;
                    self.pending_replace = true;
                }
                Vec::new()
            }
            Some(Event::CancelReplaceAll) => {
                self.pending_replace = false;
                Vec::new()
            }
            Some(Event::ConfirmReplaceAll(request)) => {
                if !self.pending_replace || *request != self.replace_request {
                    return Vec::new();
                }
                self.pending_replace = false;
                // ATTENZIONE: La conferma vale per **quel** che era in elenco: se la
                // lista si è svuotata nel frattempo non si scrive niente.
                if self.results.is_empty() {
                    return Vec::new();
                }
                vec![ToolRequest::ReplaceAll {
                    matches: self.results.clone(),
                    replacement: self.replacement.clone(),
                }]
            }
            Some(Event::Replaced) => self.ricerca(),
            Some(Event::HitClicked(path, line)) => {
                vec![ToolRequest::OpenFileAt {
                    path: path.clone(),
                    line: *line,
                }]
            }
            None => Vec::new(),
        }
    }
}

/// La riga sotto il campo: il totale che il motore mandava e nessuno leggeva.
///
/// ATTENZIONE: Con `truncated` il numero è un **minimo**: dirlo «2000 risultati» a
/// secco sarebbe una bugia comoda, e sarebbe anche il numero su cui una
/// sostituzione chiederebbe conferma.
fn riepilogo(esito: Esito) -> String {
    if esito.matches == 0 {
        return "Nessun risultato".to_owned();
    }
    let quanti = if esito.truncated {
        format!("almeno {}", esito.matches)
    } else {
        esito.matches.to_string()
    };
    let occorrenze = if esito.matches == 1 {
        "occorrenza"
    } else {
        "occorrenze"
    };
    format!("{quanti} {occorrenze} in {} file", esito.files)
}

/// L'ultima riga dell'errore di compilazione: `regex` ne stampa quattro, e le
/// prime tre sono il pattern con una freccia sotto — illeggibili in una
/// colonna larga come un pannello laterale.
fn motivo_breve(errore: &str) -> String {
    let riga = errore
        .lines()
        .map(str::trim)
        .rfind(|riga| !riga.is_empty())
        .unwrap_or(errore);
    let riga = riga.strip_prefix("error: ").unwrap_or(riga);
    format!("Espressione non valida: {riga}")
}

/// La domanda della conferma: **quanti file**, come «Ripristina» nel
/// pannello Commit.
///
/// ATTENZIONE: Con un elenco troncato lo dice: chi conferma «2000 occorrenze» su un
/// progetto che ne ha diecimila deve sapere che ne restano fuori, o crederà
/// finito un lavoro appena cominciato.
fn domanda_conferma(esito: Esito) -> String {
    let occorrenze = if esito.matches == 1 {
        "1 occorrenza"
    } else {
        &format!("{} occorrenze", esito.matches)
    };
    if esito.truncated {
        format!(
            "Sostituire le {occorrenze} mostrate in {} file? Le altre restano.",
            esito.files
        )
    } else {
        format!("Sostituire {occorrenze} in {} file?", esito.files)
    }
}

fn chip<'a>(
    t: &lume_theme::LumeTheme,
    label: &'a str,
    attivo: bool,
    event: Event,
) -> Element<'a, ToolEvent> {
    let colore = if attivo { t.accent } else { t.sec };
    button(text(label).size(10.5).color(colore))
        .padding(Padding::from([2, 6]))
        .style(styles::hover_row(
            colore,
            t.hover2,
            attivo.then_some(t.sel),
            3.0,
        ))
        .on_press(ToolEvent::new(event))
        .into()
}

/// Divide la riga in (prima, match, dopo) sui byte range del match.
/// Anteprima esclusivamente visiva: conserva il range originale nel modello
/// e tronca per caratteri Unicode, non per byte.
fn preview_around_match(
    line: &str,
    range: &std::ops::Range<usize>,
) -> (String, std::ops::Range<usize>) {
    const LIMIT: usize = 512;
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    if chars.len() <= LIMIT {
        return (line.trim_end().to_owned(), range.clone());
    }
    let start_char = chars.partition_point(|(byte, _)| *byte < range.start);
    let end_char = chars.partition_point(|(byte, _)| *byte < range.end);
    let match_chars = end_char.saturating_sub(start_char).min(LIMIT);
    let context = LIMIT.saturating_sub(match_chars);
    let before = context / 2;
    let from = start_char.saturating_sub(before);
    let to = (from + LIMIT).min(chars.len());
    let from = to.saturating_sub(LIMIT).min(from);
    let start_byte = chars.get(from).map(|(byte, _)| *byte).unwrap_or(0);
    let end_byte = chars.get(to).map(|(byte, _)| *byte).unwrap_or(line.len());
    let mut preview = String::new();
    let prefix = from > 0;
    if prefix {
        preview.push('…');
    }
    let prefix_bytes = preview.len();
    preview.push_str(&line[start_byte..end_byte]);
    if to < chars.len() {
        preview.push('…');
    }
    let start = prefix_bytes + range.start.saturating_sub(start_byte);
    let end = prefix_bytes + range.end.min(end_byte).saturating_sub(start_byte);
    (preview, start..end)
}

fn split_match(line: &str, range: &std::ops::Range<usize>) -> (String, String, String) {
    let s = range.start.min(line.len());
    let e = range.end.min(line.len());
    // allinea ai confini char per non spezzare l'UTF-8
    let s = (0..=s)
        .rev()
        .find(|&i| line.is_char_boundary(i))
        .unwrap_or(0);
    let e = (e..=line.len())
        .find(|&i| line.is_char_boundary(i))
        .unwrap_or(line.len());
    (
        line[..s].to_owned(),
        line[s..e].to_owned(),
        line[e..].to_owned(),
    )
}

#[cfg(test)]
mod scope_tests;
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{Esito, Event, SearchTool, motivo_breve, preview_around_match, riepilogo};
    use lume_core::SearchArea;
    use lume_ui::tool::{Tool, ToolEvent, ToolRequest};

    fn opzioni(requests: &[ToolRequest]) -> (lume_core::SearchOptions, SearchArea) {
        match requests.first().expect("una richiesta") {
            ToolRequest::RunSearch { options, area } => (options.clone(), *area),
            altro => panic!("richiesta inattesa: {altro:?}"),
        }
    }

    /// ATTENZIONE: Le manopole non sono decorazione: ognuna deve **rifare** la ricerca
    /// con l'opzione dentro. Cambiarla senza ricercare lascerebbe a schermo i
    /// risultati di prima sotto un tasto che dice il contrario.
    #[test]
    fn ogni_manopola_rifa_la_ricerca_con_la_sua_opzione() {
        let mut tool = SearchTool::new();
        let richieste = tool.update(&ToolEvent::new(Event::Query("main".to_owned())));
        let (options, area) = opzioni(&richieste);
        assert_eq!(options, lume_core::SearchOptions::literal("main"));
        assert_eq!(
            area,
            SearchArea::Progetto,
            "di default si cerca in tutto il progetto"
        );

        let richieste = tool.update(&ToolEvent::new(Event::ToggleRegex));
        assert!(opzioni(&richieste).0.regex);
        let richieste = tool.update(&ToolEvent::new(Event::ToggleWord));
        assert!(opzioni(&richieste).0.whole_word);
        let richieste = tool.update(&ToolEvent::new(Event::SetScope(SearchArea::Aperti)));
        assert_eq!(opzioni(&richieste).1, SearchArea::Aperti);

        // il testo resta quello: si cambia il come, non il cosa
        assert_eq!(opzioni(&richieste).0.needle, "main");

        // maiuscole: due posizioni, e la seconda torna allo smart-case
        let richieste = tool.update(&ToolEvent::new(Event::ToggleCase));
        assert_eq!(opzioni(&richieste).0.case_sensitive, Some(true));
        let richieste = tool.update(&ToolEvent::new(Event::ToggleCase));
        assert_eq!(opzioni(&richieste).0.case_sensitive, None);
    }

    /// Senza testo non si cerca — nemmeno toccando una manopola: sarebbe un
    /// walk dell'albero intero per zero occorrenze.
    #[test]
    fn senza_testo_nessuna_manopola_avvia_un_giro() {
        let mut tool = SearchTool::new();
        assert!(tool.update(&ToolEvent::new(Event::ToggleRegex)).is_empty());
        assert!(
            tool.update(&ToolEvent::new(Event::Query("   ".to_owned())))
                .is_empty()
        );
        // e ripremere l'ambito già scelto non rifà niente
        let mut tool = SearchTool::new();
        let _ = tool.update(&ToolEvent::new(Event::Query("x".to_owned())));
        assert!(
            tool.update(&ToolEvent::new(Event::SetScope(SearchArea::Progetto)))
                .is_empty(),
            "l'ambito era già quello"
        );
    }

    /// I risultati di prima spariscono **subito**: lasciarli sotto opzioni
    /// che non li hanno prodotti è peggio di una lista vuota.
    #[test]
    fn cambiare_manopola_butta_i_risultati_vecchi() {
        let mut tool = SearchTool::new();
        let _ = tool.update(&ToolEvent::new(Event::Query("main".to_owned())));
        let _ = tool.update(&ToolEvent::new(Event::Append(vec![
            lume_core::FileMatches {
                path: std::path::PathBuf::from("a.rs"),
                hits: Vec::new(),
            },
        ])));
        let _ = tool.update(&ToolEvent::new(Event::Done {
            files: 1,
            matches: 1,
            truncated: false,
        }));
        assert_eq!(tool.results.len(), 1);
        assert!(tool.esito.is_some());

        let _ = tool.update(&ToolEvent::new(Event::ToggleRegex));
        assert!(tool.results.is_empty());
        assert!(tool.esito.is_none(), "e il riepilogo torna in attesa");
    }

    /// Il totale che il motore mandava e che nessuno leggeva, più la
    /// differenza fra «2000» e «almeno 2000».
    #[test]
    fn il_riepilogo_dice_quando_il_numero_e_un_minimo() {
        assert_eq!(
            riepilogo(Esito {
                files: 0,
                matches: 0,
                truncated: false
            }),
            "Nessun risultato"
        );
        assert_eq!(
            riepilogo(Esito {
                files: 3,
                matches: 12,
                truncated: false
            }),
            "12 occorrenze in 3 file"
        );
        assert_eq!(
            riepilogo(Esito {
                files: 1,
                matches: 1,
                truncated: false
            }),
            "1 occorrenza in 1 file"
        );
        assert_eq!(
            riepilogo(Esito {
                files: 40,
                matches: 2000,
                truncated: true
            }),
            "almeno 2000 occorrenze in 40 file"
        );
    }

    #[test]
    fn lanteprima_unicode_rimane_limitata_e_mantiene_il_match() {
        let line = format!("{}AGO{}", "è".repeat(400), "界".repeat(400));
        let start = line.find("AGO").unwrap();
        let (preview, range) = preview_around_match(&line, &(start..start + 3));
        assert!(preview.chars().count() <= 514, "ellissi incluse");
        assert_eq!(&preview[range], "AGO");
        assert!(preview.starts_with('…'));
        assert!(preview.ends_with('…'));
    }

    /// ATTENZIONE: Il primo clic **chiede**, non scrive: è la stessa conferma in linea
    /// di «Ripristina» nel pannello Commit, e qui pesa di più, perché
    /// riscrive file che l'utente non ha aperto.
    ///
    /// E quel che parte sono i **risultati mostrati**, non il testo cercato:
    /// chi ha confermato «in 2 file» ne intendeva due precisi.
    #[test]
    fn il_tasto_chiede_conferma_e_poi_manda_i_risultati_mostrati() {
        fn trovato(nome: &str) -> lume_core::FileMatches {
            lume_core::FileMatches {
                path: std::path::PathBuf::from(nome),
                hits: vec![lume_core::SearchHit {
                    line_number: 1,
                    line_text: "ago".into(),
                    range_in_line: 0..3,
                }],
            }
        }

        let mut tool = SearchTool::new();
        let _ = tool.update(&ToolEvent::new(Event::Query("ago".to_owned())));
        let _ = tool.update(&ToolEvent::new(Event::ReplaceChanged("AGO".to_owned())));

        // a ricerca non finita non si chiede nemmeno: sostituire su una lista
        // che sta ancora arrivando non sarebbe «tutto»
        assert!(
            tool.update(&ToolEvent::new(Event::AskReplaceAll))
                .is_empty()
        );
        assert!(!tool.pending_replace);

        let _ = tool.update(&ToolEvent::new(Event::Append(vec![
            trovato("a.rs"),
            trovato("b.rs"),
        ])));
        let _ = tool.update(&ToolEvent::new(Event::Done {
            files: 2,
            matches: 2,
            truncated: false,
        }));
        assert!(
            tool.update(&ToolEvent::new(Event::AskReplaceAll))
                .is_empty()
        );
        assert!(tool.pending_replace, "il primo clic chiede e basta");

        let richieste = tool.update(&ToolEvent::new(Event::ConfirmReplaceAll(
            tool.replace_request,
        )));
        match richieste.first().expect("una richiesta") {
            ToolRequest::ReplaceAll {
                matches,
                replacement,
            } => {
                assert_eq!(matches.len(), 2);
                assert_eq!(replacement, "AGO");
            }
            altro => panic!("richiesta inattesa: {altro:?}"),
        }
        assert!(!tool.pending_replace, "la conferma si consuma");

        // l'app ha finito: la lista mostrata è vecchia di un istante, e la
        // ricerca riparte da sola
        let richieste = tool.update(&ToolEvent::new(Event::Replaced));
        assert!(matches!(
            richieste.first(),
            Some(ToolRequest::RunSearch { .. })
        ));
        assert!(tool.results.is_empty());

        // ATTENZIONE: E su una lista vuota non parte niente: il `Done` può dire due
        // occorrenze e i batch essere stati scartati come vecchi. Confermare
        // manderebbe una sostituzione su zero file, cioè un giro a vuoto che
        // però ha già chiesto conferma all'utente.
        let _ = tool.update(&ToolEvent::new(Event::Done {
            files: 2,
            matches: 2,
            truncated: false,
        }));
        let _ = tool.update(&ToolEvent::new(Event::AskReplaceAll));
        assert!(
            tool.update(&ToolEvent::new(Event::ConfirmReplaceAll(
                tool.replace_request
            )))
            .is_empty()
        );
    }

    /// La conferma dice **quanti file**, e con l'elenco troncato dice pure
    /// che non li prende tutti.
    #[test]
    fn la_conferma_dice_quanti_file_tocca() {
        assert_eq!(
            super::domanda_conferma(Esito {
                files: 12,
                matches: 37,
                truncated: false
            }),
            "Sostituire 37 occorrenze in 12 file?"
        );
        assert_eq!(
            super::domanda_conferma(Esito {
                files: 1,
                matches: 1,
                truncated: false
            }),
            "Sostituire 1 occorrenza in 1 file?"
        );
        assert_eq!(
            super::domanda_conferma(Esito {
                files: 40,
                matches: 2000,
                truncated: true
            }),
            "Sostituire le 2000 occorrenze mostrate in 40 file? Le altre restano."
        );
    }

    /// ATTENZIONE: In modalità regex **ogni battuta intermedia è un errore**: `foo(`
    /// non compila. Una notifica per battuta sarebbe rumore, e l'errore va
    /// dove l'utente sta già guardando — sotto il campo, al posto del
    /// riepilogo. Di quattro righe di `regex` si tiene quella che dice cosa.
    /// Senza cartella aperta il pannello **lo dice**, e non con la frase
    /// della regex: «Espressione non valida: nessuna cartella aperta» sarebbe
    /// una diagnosi sbagliata su un'espressione che va benissimo.
    #[test]
    fn senza_cartella_il_pannello_lo_dice_con_parole_sue() {
        let mut tool = SearchTool::new();
        let _ = tool.update(&ToolEvent::new(Event::SenzaAmbito));
        assert_eq!(tool.errore.as_deref(), Some("Nessuna cartella aperta"));

        // ...e una ricerca nuova lo toglie di mezzo
        let _ = tool.update(&ToolEvent::new(Event::Query("ago".to_owned())));
        assert!(tool.errore.is_none());
    }

    #[test]
    fn lerrore_di_regex_si_riduce_alla_riga_che_dice_qualcosa() {
        assert_eq!(
            motivo_breve("regex parse error:\n    a(\n     ^\nerror: unclosed group"),
            "Espressione non valida: unclosed group"
        );
        assert_eq!(
            motivo_breve("boom"),
            "Espressione non valida: boom",
            "un messaggio di una riga resta intero"
        );

        // ATTENZIONE: E la riduzione avviene **entrando**, non uscendo: il campo tiene
        // il testo già pronto, perché chi lo disegna non sa se sta mostrando
        // un errore del motore o una frase nostra.
        let mut tool = SearchTool::new();
        let _ = tool.update(&ToolEvent::new(Event::Failed(
            "regex parse error:\n    a(\n     ^\nerror: unclosed group".to_owned(),
        )));
        assert_eq!(
            tool.errore.as_deref(),
            Some("Espressione non valida: unclosed group")
        );
    }

    /// Chiuso il pannello, i risultati non li guarda nessuno: si liberano (non
    /// solo si svuotano) e la ricerca in corso si ferma.
    #[test]
    fn chiudere_il_pannello_ferma_la_ricerca_e_libera_i_risultati() {
        let mut tool = SearchTool::new();
        let _ = tool.update(&ToolEvent::new(Event::Query("main".to_owned())));
        let _ = tool.update(&ToolEvent::new(Event::Append(vec![
            lume_core::FileMatches {
                path: std::path::PathBuf::from("a.rs"),
                hits: Vec::new(),
            },
        ])));

        let richieste = tool.on_visibility_changed(false);

        assert!(
            matches!(richieste.as_slice(), [ToolRequest::CancelSearch]),
            "atteso CancelSearch, trovato {richieste:?}"
        );
        assert!(tool.results.is_empty());
        assert_eq!(
            tool.results.capacity(),
            0,
            "memoria restituita, non solo svuotata"
        );
    }

    /// Review Focus: `esito: None` vuol dire «in corso». Riaperto senza
    /// rilanciare, il pannello resterebbe in corso per sempre.
    #[test]
    fn riaprire_il_pannello_rifa_la_ricerca_interrotta() {
        let mut tool = SearchTool::new();
        let _ = tool.update(&ToolEvent::new(Event::Query("main".to_owned())));
        let _ = tool.on_visibility_changed(false);

        let richieste = tool.on_visibility_changed(true);

        let (options, _) = opzioni(&richieste);
        assert_eq!(options.needle, "main");
    }

    #[test]
    fn riaprire_senza_testo_non_cerca() {
        let mut tool = SearchTool::new();
        let _ = tool.on_visibility_changed(false);
        assert!(tool.on_visibility_changed(true).is_empty());
    }

    /// La chiusura riporta il pannello a «niente in corso, niente da
    /// confermare»: un errore o una conferma di sostituzione lasciati lì
    /// riapparirebbero davanti a un elenco che non esiste più.
    #[test]
    fn chiudere_il_pannello_azzera_esito_errore_e_conferma() {
        let mut tool = SearchTool::new();
        let _ = tool.update(&ToolEvent::new(Event::Query("ago".to_owned())));
        let _ = tool.update(&ToolEvent::new(Event::Done {
            files: 1,
            matches: 2,
            truncated: false,
        }));
        let _ = tool.update(&ToolEvent::new(Event::AskReplaceAll));
        assert!(tool.pending_replace, "precondizione: conferma aperta");
        let _ = tool.update(&ToolEvent::new(Event::Failed("boom".to_owned())));
        assert!(tool.errore.is_some(), "precondizione: errore presente");

        let _ = tool.on_visibility_changed(false);

        assert!(tool.esito.is_none());
        assert!(tool.errore.is_none());
        assert!(!tool.pending_replace);
    }

    /// Le manopole e l'ambito sono scelte dell'utente, non risultati: alla
    /// riapertura la ricerca riparte **con quelle**, non con i default.
    #[test]
    fn riaprire_il_pannello_conserva_manopole_e_ambito() {
        let mut tool = SearchTool::new();
        let _ = tool.update(&ToolEvent::new(Event::Query("Main".to_owned())));
        let _ = tool.update(&ToolEvent::new(Event::ToggleWord));
        let _ = tool.update(&ToolEvent::new(Event::SetScope(SearchArea::Aperti)));
        let _ = tool.on_visibility_changed(false);

        let richieste = tool.on_visibility_changed(true);

        let (options, area) = opzioni(&richieste);
        assert_eq!(options.needle, "Main");
        assert!(options.whole_word);
        assert_eq!(area, SearchArea::Aperti);
    }
}
