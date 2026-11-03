//! Tool «Struttura»: i simboli del documento attivo, come li dichiara il
//! language server.
//!
//! **Il tool non legge `Lume`.** Le righe arrivano spinte con un
//! [`Event::Replace`], come già fanno Estensioni e Database: il `Registry`
//! espone solo `&dyn Tool` e la struttura vive accanto alla sessione LSP.
//!
//! **Cosa non c'è, e perché.**
//! - Nessun interruttore «mostra import». Il prototipo ce l'ha, e la regola
//!   di `MENUS` in `menu.rs` dice che si disegna solo quel che ha un soggetto: dei
//!   tre server che Lume sa installare — rust-analyzer, tombi, rumdl —
//!   **nessuno restituisce gli `use` fra i simboli**. Sarebbe un interruttore
//!   che non cambia niente, cioè il mock che questo lavoro toglie. Il giorno
//!   che un server li manda, il posto dove metterlo è qui.
//! - Nessun filtro né ordinamento alfabetico: il prototipo mostra l'ordine
//!   del file, che è anche l'unico che il server garantisce.

use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Alignment, Element, Length, Padding};
use lume_ui::icons::Icon;
use lume_ui::styles;
use lume_ui::tool::{Tool, ToolCtx, ToolEvent, ToolId, ToolRequest};

pub const ID: ToolId = ToolId("structure");

/// Una riga, fotografata dall'app: già appiattita, già con la sua profondità.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub detail: Option<String>,
    /// La sigla del genere di simbolo: `fn`, `struct`, `mod`… È quel che il
    /// prototipo mostra al posto di un'icona per simbolo, e costa zero.
    pub kind: &'static str,
    pub depth: usize,
    /// Riga del file, **1-based**: è quel che vuole `OpenFileAt`.
    pub line: usize,
}

#[derive(Clone, Debug)]
pub enum Event {
    Replace(Snapshot),
    Open(usize),
}

/// Quel che il pannello sa del mondo. Il percorso serve al clic: una riga
/// senza file su cui atterrare non è cliccabile, e con un `Option` il caso
/// non si può dimenticare.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub path: Option<std::path::PathBuf>,
    pub rows: Vec<Row>,
    /// Perché non c'è niente da mostrare. Vuoto = c'è.
    pub empty_reason: &'static str,
}

pub struct StructureTool {
    snapshot: Snapshot,
}

impl StructureTool {
    pub fn new() -> Self {
        Self {
            snapshot: Snapshot::default(),
        }
    }
}

impl Default for StructureTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for StructureTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Struttura"
    }

    fn icon(&self) -> Icon {
        Icon::Structure
    }

    fn feature(&self) -> Option<&'static str> {
        Some("lsp")
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        if self.snapshot.rows.is_empty() {
            let motivo = if self.snapshot.empty_reason.is_empty() {
                "Nessun simbolo in questo file."
            } else {
                self.snapshot.empty_reason
            };
            return container(text(motivo).size(12).color(t.muted))
                .padding(Padding::from([2.0, 12.0]))
                .into();
        }
        let mut list = column![].spacing(1);
        for (index, riga) in self.snapshot.rows.iter().enumerate() {
            // il rientro dice l'annidamento: 10 px per livello, come le
            // cartelle dell'albero progetto
            let rientro = 10.0 * riga.depth as f32;
            let mut contenuto = row![
                Space::new().width(Length::Fixed(rientro)),
                text(riga.kind).size(10.5).color(t.faint),
                text(riga.name.clone()).size(12).color(t.ui),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            if let Some(detail) = &riga.detail {
                contenuto = contenuto.push(
                    container(
                        text(detail.clone())
                            .size(11)
                            .color(t.muted)
                            .wrapping(text::Wrapping::None),
                    )
                    .clip(true)
                    .width(Length::Fill),
                );
            }
            let mut riga_bottone = button(contenuto)
                .width(Length::Fill)
                .padding(Padding::from([3, 6]))
                .style(styles::hover_row(t.ui, t.hover, None, 4.0));
            // senza percorso non si va da nessuna parte: il tasto resta
            // spento invece di fingere
            if self.snapshot.path.is_some() {
                riga_bottone = riga_bottone.on_press(ToolEvent::new(Event::Open(index)));
            }
            list = list.push(riga_bottone);
        }
        scrollable(container(list).width(Length::Fill).padding(pad_list()))
            .height(Length::Fill)
            .into()
    }

    fn update(&mut self, event: &ToolEvent) -> Vec<ToolRequest> {
        match event.downcast::<Event>() {
            Some(Event::Replace(snapshot)) => {
                self.snapshot = snapshot.clone();
                Vec::new()
            }
            Some(Event::Open(index)) => self
                .snapshot
                .path
                .as_ref()
                .zip(self.snapshot.rows.get(*index))
                .map(|(path, riga)| ToolRequest::OpenFileAt {
                    path: path.clone(),
                    line: riga.line,
                })
                .into_iter()
                .collect(),
            None => Vec::new(),
        }
    }
}

const fn pad_list() -> Padding {
    Padding {
        top: 2.0,
        right: 12.0,
        bottom: 12.0,
        left: 12.0,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn riga(name: &str, line: usize) -> Row {
        Row {
            name: name.to_owned(),
            detail: None,
            kind: "fn",
            depth: 0,
            line,
        }
    }

    /// Il clic apre il file alla riga del simbolo — e **solo** se un file
    /// c'è: la struttura di un documento senza path (la console SQL) non ha
    /// dove far atterrare nessuno, e un `OpenFileAt` con un percorso
    /// inventato sarebbe peggio di un tasto spento.
    #[test]
    fn il_clic_porta_alla_riga_e_senza_file_non_porta_da_nessuna_parte() {
        let mut tool = StructureTool::new();
        tool.update(&ToolEvent::new(Event::Replace(Snapshot {
            path: Some(std::path::PathBuf::from("C:/repo/src/main.rs")),
            rows: vec![riga("main", 12), riga("altro", 30)],
            empty_reason: "",
        })));
        let requests = tool.update(&ToolEvent::new(Event::Open(1)));
        assert!(matches!(
            &requests[..],
            [ToolRequest::OpenFileAt { line: 30, .. }]
        ));
        assert!(tool.update(&ToolEvent::new(Event::Open(9))).is_empty());

        tool.update(&ToolEvent::new(Event::Replace(Snapshot {
            path: None,
            rows: vec![riga("main", 12)],
            empty_reason: "",
        })));
        assert!(
            tool.update(&ToolEvent::new(Event::Open(0))).is_empty(),
            "senza file non si apre niente"
        );
    }
}
