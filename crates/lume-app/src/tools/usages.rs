//! Dock «Trova utilizzi»: dove il simbolo sotto il cursore compare, secondo
//! il language server.
//!
//! **Il tool non legge `Lume`**: i gruppi arrivano spinti con un
//! [`Event::Replace`], come Problemi e Struttura.
//!
//! **Niente anteprima della riga, e non è una dimenticanza.** Il prototipo
//! mostra il testo di ogni occorrenza; per farlo davvero bisognerebbe
//! **leggere dal disco** ogni file coinvolto — una ricerca su un simbolo
//! molto usato ne tocca decine — e leggerlo dal thread della UI, che è
//! esattamente il genere di lavoro che questo progetto toglie. Mostrarla solo
//! per i file già aperti darebbe righe di due qualità diverse nella stessa
//! lista, che è peggio che non averla. `file:riga:colonna` è azionabile: un
//! clic e ci si è.

use std::path::PathBuf;

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Element, Length, Padding};
use lume_ui::MONO;
use lume_ui::icons::Icon;
use lume_ui::styles;
use lume_ui::tool::{Tool, ToolCtx, ToolEvent, ToolId, ToolKind, ToolRequest};

pub const ID: ToolId = ToolId("usages");

/// Le occorrenze in un file solo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// Come mostrarlo: relativo alla radice quando si può.
    pub label: String,
    pub path: Option<PathBuf>,
    /// Riga e colonna, **1-based**: è quel che si mostra e quel che vuole
    /// `OpenFileAt`.
    pub hits: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Il simbolo cercato, per l'intestazione.
    pub symbol: String,
    pub groups: Vec<Group>,
    /// Cosa dire quando non ci sono gruppi.
    pub status: String,
}

impl Snapshot {
    pub fn total(&self) -> usize {
        self.groups.iter().map(|group| group.hits.len()).sum()
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    Replace(Snapshot),
    Open(usize, usize),
}

pub struct UsagesTool {
    snapshot: Snapshot,
}

impl UsagesTool {
    pub fn new() -> Self {
        Self {
            snapshot: Snapshot::default(),
        }
    }
}

impl Default for UsagesTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for UsagesTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Utilizzi"
    }

    fn icon(&self) -> Icon {
        Icon::Usages
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Dock
    }

    fn feature(&self) -> Option<&'static str> {
        Some("lsp")
    }

    fn badge(&self) -> Option<String> {
        let totale = self.snapshot.total();
        (totale > 0).then(|| totale.to_string())
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        let mut list = column![].spacing(1);
        if self.snapshot.groups.is_empty() {
            let stato = if self.snapshot.status.is_empty() {
                "Alt+F7 cerca gli utilizzi del simbolo sotto il cursore."
            } else {
                self.snapshot.status.as_str()
            };
            list = list.push(
                container(text(stato).size(12).color(t.muted)).padding(Padding::from([10.0, 12.0])),
            );
        }
        for (indice_gruppo, gruppo) in self.snapshot.groups.iter().enumerate() {
            list = list.push(
                container(
                    row![
                        text(gruppo.label.clone()).size(12).color(t.sec),
                        text(format!("({})", gruppo.hits.len()))
                            .size(11)
                            .color(t.faint),
                    ]
                    .spacing(6),
                )
                .padding(Padding::from([6.0, 12.0])),
            );
            for (indice_riga, (riga, colonna)) in gruppo.hits.iter().enumerate() {
                let contenuto = row![
                    text(format!("{riga}:{colonna}"))
                        .size(11)
                        .font(MONO)
                        .color(t.muted),
                ]
                .padding(Padding::from([3.0, 28.0]))
                .width(Length::Fill);
                if gruppo.path.is_some() {
                    list = list.push(
                        button(contenuto)
                            .width(Length::Fill)
                            .style(styles::hover_row(t.text, t.hover, None, 3.0))
                            .on_press(ToolEvent::new(Event::Open(indice_gruppo, indice_riga))),
                    );
                } else {
                    list = list.push(container(contenuto).width(Length::Fill));
                }
            }
        }
        let riepilogo = if self.snapshot.groups.is_empty() {
            String::new()
        } else {
            let totale = self.snapshot.total();
            let file = self.snapshot.groups.len();
            // il cursore può non essere su una parola: allora il conteggio
            // resta, il nome no. Un «» fra virgolette sarebbe peggio di niente
            if self.snapshot.symbol.is_empty() {
                format!("{totale} in {file} file")
            } else {
                format!("«{}» — {totale} in {file} file", self.snapshot.symbol)
            }
        };
        column![
            scrollable(list).width(Length::Fill).height(Length::Fill),
            container(text(riepilogo).size(11.5).color(t.muted))
                .padding(Padding::from([6.0, 12.0])),
        ]
        .into()
    }

    fn update(&mut self, event: &ToolEvent) -> Vec<ToolRequest> {
        match event.downcast::<Event>() {
            Some(Event::Replace(snapshot)) => {
                self.snapshot = snapshot.clone();
                Vec::new()
            }
            Some(Event::Open(gruppo, riga)) => self
                .snapshot
                .groups
                .get(*gruppo)
                .and_then(|gruppo| {
                    let (linea, _) = gruppo.hits.get(*riga)?;
                    Some(ToolRequest::OpenFileAt {
                        path: gruppo.path.clone()?,
                        line: *linea,
                    })
                })
                .into_iter()
                .collect(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn snapshot() -> Snapshot {
        Snapshot {
            symbol: "mian".to_owned(),
            groups: vec![
                Group {
                    label: "src/main.rs".to_owned(),
                    path: Some(PathBuf::from("C:/repo/src/main.rs")),
                    hits: vec![(3, 4), (9, 12)],
                },
                Group {
                    label: "un-file-fuori-dal-disco".to_owned(),
                    path: None,
                    hits: vec![(1, 1)],
                },
            ],
            status: String::new(),
        }
    }

    /// Il badge conta le **occorrenze**, non i file: è il numero che serve a
    /// decidere se guardare. E un gruppo senza percorso — un URI che non è un
    /// file su disco — non è cliccabile invece di aprire qualcosa a caso.
    #[test]
    fn il_badge_conta_le_occorrenze_e_i_file_senza_percorso_non_si_aprono() {
        let mut tool = UsagesTool::new();
        assert_eq!(tool.badge(), None, "a freddo non c'è niente da contare");
        tool.update(&ToolEvent::new(Event::Replace(snapshot())));
        assert_eq!(tool.badge(), Some("3".to_owned()));

        let requests = tool.update(&ToolEvent::new(Event::Open(0, 1)));
        assert!(matches!(
            &requests[..],
            [ToolRequest::OpenFileAt { line: 9, .. }]
        ));
        assert!(
            tool.update(&ToolEvent::new(Event::Open(1, 0))).is_empty(),
            "senza percorso non si apre niente"
        );
        assert!(tool.update(&ToolEvent::new(Event::Open(5, 0))).is_empty());
    }
}
