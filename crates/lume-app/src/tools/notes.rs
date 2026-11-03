use std::path::PathBuf;

use iced::widget::{button, column, container, row, scrollable, text, text_input, tooltip};
use iced::{Alignment, Element, Length, Padding};
use lume_notes::{NoteId, NoteSummary, filter_summaries};
use lume_ui::icons::Icon;
use lume_ui::styles;
use lume_ui::tool::{NotesOrigin, Tool, ToolCtx, ToolEvent, ToolId, ToolRequest};

pub const ID: ToolId = ToolId("notes");
const ACTION_TARGET: f32 = 28.0;
const _: () = assert!(ACTION_TARGET >= 28.0);

#[derive(Debug, Clone)]
pub enum Event {
    Query(String),
    Create(NotesOrigin),
    Open(NotesOrigin, NoteId),
    Delete(NotesOrigin, NoteId),
    ChooseDirectory,
    Replace {
        summaries: Vec<NoteSummary>,
        directory: PathBuf,
        origin: NotesOrigin,
    },
    Error(String),
    /// Cambio di cartella accettato, in attesa della nuova scansione.
    Loading(PathBuf),
    Clear,
    /// Solo test: il tool consegna una fotografia del proprio stato.
    #[cfg(test)]
    Probe(std::sync::mpsc::Sender<Snapshot>),
}

/// Stato osservabile del tool dai test dell'app (`Event::Probe`).
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub summaries: usize,
    pub directory: Option<PathBuf>,
    pub loading: bool,
}

#[derive(Default)]
pub struct NotesTool {
    query: String,
    summaries: Vec<NoteSummary>,
    directory: Option<PathBuf>,
    error: Option<String>,
    origin: Option<NotesOrigin>,
    /// Cache scaricata e riscansione non ancora arrivata: la vista non deve
    /// dire «Nessun appunto» né offrire «+» su una lista che non c'è.
    loading: bool,
}

impl NotesTool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Svuota i riepiloghi restituendo la memoria, e l'errore mostrato: sono
    /// della vista. Cartella e testo di ricerca restano (non pesano nulla e
    /// la riapertura torna dove l'utente l'aveva lasciata); fino al prossimo
    /// `Replace` la vista è in caricamento.
    fn clear_view_cache(&mut self) {
        self.summaries = Vec::new();
        self.error = None;
        self.origin = None;
        self.loading = true;
    }
}

impl Tool for NotesTool {
    fn id(&self) -> ToolId {
        ID
    }
    fn title(&self) -> &'static str {
        "Appunti"
    }
    fn icon(&self) -> Icon {
        Icon::Notes
    }
    fn feature(&self) -> Option<&'static str> {
        Some("appunti")
    }

    fn on_visibility_changed(&mut self, open: bool) -> Vec<ToolRequest> {
        if open {
            return vec![ToolRequest::NotesLoad];
        }
        // Cache della vista: si libera qui, subito. All'app resta il resto.
        self.clear_view_cache();
        vec![ToolRequest::NotesUnload]
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        let search = text_input("Cerca appunti…", &self.query)
            .on_input(|value| ToolEvent::new(Event::Query(value)))
            .size(12)
            .padding(Padding::from([5.0, 9.0]))
            .style(styles::input(t));
        let mut create_button = button(text("+").size(17))
            .width(ACTION_TARGET)
            .height(ACTION_TARGET);
        if self.directory.is_some()
            && !self.loading
            && let Some(origin) = self.origin
        {
            create_button = create_button.on_press(ToolEvent::new(Event::Create(origin)));
        }
        let create = tooltip(
            create_button,
            "Nuovo appunto (Ctrl+Alt+N)",
            tooltip::Position::Top,
        );
        let mut list = column![].spacing(3);
        let filtered = filter_summaries(&self.summaries, &self.query);
        if filtered.is_empty() {
            let label = if self.loading {
                "Caricamento…"
            } else if self.summaries.is_empty() {
                "Nessun appunto"
            } else {
                "Nessun risultato"
            };
            list = list.push(container(text(label).size(11.5).color(t.muted)).padding(12));
        } else {
            for summary in filtered {
                let Some(origin) = self.origin else {
                    break;
                };
                let title = if summary.title.trim().is_empty() {
                    "Senza titolo"
                } else {
                    &summary.title
                };
                let open = button(column![
                    text(title).size(12),
                    text(&summary.preview).size(10).color(t.muted)
                ])
                .width(Length::Fill)
                .on_press(ToolEvent::new(Event::Open(origin, summary.id.clone())));
                let delete_label = format!("Elimina {title}");
                let delete = tooltip(
                    button(text("×").size(16))
                        .width(ACTION_TARGET)
                        .height(ACTION_TARGET)
                        .on_press(ToolEvent::new(Event::Delete(origin, summary.id.clone()))),
                    text(delete_label),
                    tooltip::Position::Top,
                );
                list = list.push(row![open, delete].spacing(4).align_y(Alignment::Center));
            }
        }
        let path = self
            .directory
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Caricamento…".into());
        let browse =
            button(text("Sfoglia…").size(11)).on_press(ToolEvent::new(Event::ChooseDirectory));
        let footer = column![
            text("CARTELLA PERSONALE").size(10).color(t.muted),
            text(if self.loading {
                "…".to_owned()
            } else {
                format!("{} file", self.summaries.len())
            })
            .size(11),
            text(path).size(10).color(t.muted),
            browse,
            text("Fuori dal progetto, non versionata. Un file .md per appunto.")
                .size(10)
                .color(t.muted),
        ]
        .spacing(4);
        let error: Element<'_, ToolEvent> = self.error.as_ref().map_or_else(
            || container(iced::widget::Space::new()).into(),
            |message| {
                container(text(message).size(10).color(t.bad))
                    .padding(6)
                    .into()
            },
        );
        column![
            row![search, create]
                .spacing(6)
                .padding(Padding::from([0.0, 10.0])),
            error,
            scrollable(list).height(Length::Fill),
            container(footer).padding(10),
        ]
        .height(Length::Fill)
        .into()
    }

    fn update(&mut self, event: &ToolEvent) -> Vec<ToolRequest> {
        match event.downcast::<Event>() {
            Some(Event::Query(query)) => {
                self.query.clone_from(query);
                Vec::new()
            }
            Some(Event::Create(origin)) => vec![ToolRequest::NotesCreate(*origin)],
            Some(Event::Open(origin, id)) => vec![ToolRequest::NotesOpen(*origin, id.clone())],
            Some(Event::Delete(origin, id)) => vec![ToolRequest::NotesDelete(*origin, id.clone())],
            Some(Event::ChooseDirectory) => vec![ToolRequest::NotesChooseDirectory],
            Some(Event::Replace {
                summaries,
                directory,
                origin,
            }) => {
                self.origin = Some(*origin);
                self.summaries.clone_from(summaries);
                self.directory = Some(directory.clone());
                self.error = None;
                self.loading = false;
                Vec::new()
            }
            #[cfg(test)]
            Some(Event::Probe(sink)) => {
                let _ = sink.send(Snapshot {
                    summaries: self.summaries.len(),
                    directory: self.directory.clone(),
                    loading: self.loading,
                });
                Vec::new()
            }
            Some(Event::Loading(directory)) => {
                self.clear_view_cache();
                self.directory = Some(directory.clone());
                Vec::new()
            }
            Some(Event::Clear) => {
                self.clear_view_cache();
                Vec::new()
            }
            Some(Event::Error(error)) => {
                self.error = Some(error.clone());
                // una scansione fallita non lascia il pannello in attesa
                self.loading = false;
                Vec::new()
            }
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn exposed_actions_emit_real_requests_and_delete_target_is_practical() {
        let mut tool = NotesTool::new();
        let id = NoteId::from("note-1");
        assert!(matches!(
            tool.on_visibility_changed(true).as_slice(),
            [ToolRequest::NotesLoad]
        ));
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::Create(NotesOrigin(1))))
                .as_slice(),
            [ToolRequest::NotesCreate(NotesOrigin(1))]
        ));
        assert!(
            matches!(tool.update(&ToolEvent::new(Event::Open(NotesOrigin(1), id.clone()))).as_slice(), [ToolRequest::NotesOpen(NotesOrigin(1), found)] if found == &id)
        );
        assert!(
            matches!(tool.update(&ToolEvent::new(Event::Delete(NotesOrigin(1), id.clone()))).as_slice(), [ToolRequest::NotesDelete(NotesOrigin(1), found)] if found == &id)
        );
        assert!(matches!(
            tool.update(&ToolEvent::new(Event::ChooseDirectory))
                .as_slice(),
            [ToolRequest::NotesChooseDirectory]
        ));
    }

    fn summary(id: &str) -> NoteSummary {
        NoteSummary {
            id: NoteId::from(id),
            title: format!("Titolo {id}"),
            updated_unix_ms: 1,
            preview: "anteprima".into(),
            path: PathBuf::from(format!("{id}.md")),
            error: None,
            links: Vec::new(),
        }
    }

    fn tool_popolato() -> NotesTool {
        let mut tool = NotesTool::new();
        let _ = tool.update(&ToolEvent::new(Event::Replace {
            summaries: vec![summary("a"), summary("b")],
            directory: PathBuf::from("cartella-appunti"),
            origin: NotesOrigin(1),
        }));
        let _ = tool.update(&ToolEvent::new(Event::Error("errore vecchio".into())));
        tool
    }

    #[test]
    fn la_chiusura_scarica_i_riepiloghi_ed_emette_solo_notes_unload() {
        let mut tool = tool_popolato();
        assert_eq!(tool.summaries.len(), 2);

        let requests = tool.on_visibility_changed(false);

        assert!(
            matches!(requests.as_slice(), [ToolRequest::NotesUnload]),
            "la chiusura emette esattamente NotesUnload: {requests:?}"
        );
        assert!(tool.summaries.is_empty(), "la cache della vista e' vuota");
        assert_eq!(
            tool.summaries.capacity(),
            0,
            "la memoria dei riepiloghi e' restituita, non solo svuotata"
        );
    }

    #[test]
    fn la_riapertura_emette_ancora_notes_load() {
        let mut tool = tool_popolato();
        let _ = tool.on_visibility_changed(false);
        assert!(matches!(
            tool.on_visibility_changed(true).as_slice(),
            [ToolRequest::NotesLoad]
        ));
    }

    #[test]
    fn clear_e_idempotente_e_tiene_coerente_la_cartella() {
        let mut tool = tool_popolato();
        let prima = tool.directory.clone();

        for _ in 0..2 {
            let requests = tool.update(&ToolEvent::new(Event::Clear));
            assert!(requests.is_empty(), "Clear non chiede nulla all'app");
            assert!(tool.summaries.is_empty());
            assert!(tool.error.is_none(), "l'errore appartiene alla vista");
            assert_eq!(tool.directory, prima, "la cartella scelta resta");
        }

        // Un nuovo Replace (la riapertura) ripopola senza residui.
        let _ = tool.update(&ToolEvent::new(Event::Replace {
            summaries: vec![summary("c")],
            directory: PathBuf::from("cartella-appunti"),
            origin: NotesOrigin(1),
        }));
        assert_eq!(tool.summaries.len(), 1);
    }

    #[test]
    fn dopo_clear_il_tool_e_in_caricamento_fino_al_prossimo_replace() {
        let mut tool = tool_popolato();
        assert!(!tool.loading);

        let _ = tool.update(&ToolEvent::new(Event::Clear));
        assert!(tool.loading, "dopo lo scarico non si dice «Nessun appunto»");

        let _ = tool.update(&ToolEvent::new(Event::Replace {
            summaries: Vec::new(),
            directory: PathBuf::from("cartella-appunti"),
            origin: NotesOrigin(1),
        }));
        assert!(!tool.loading, "la scansione arrivata chiude il caricamento");
    }

    #[test]
    fn la_chiusura_mette_il_tool_in_caricamento_e_un_errore_lo_chiude() {
        let mut tool = tool_popolato();
        let _ = tool.on_visibility_changed(false);
        assert!(tool.loading);
        let _ = tool.update(&ToolEvent::new(Event::Error("scansione fallita".into())));
        assert!(!tool.loading, "un errore non lascia il pannello in attesa");
        assert!(tool.error.is_some());
    }
}
