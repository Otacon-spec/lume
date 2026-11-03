//! Tool «Segnalibri»: righe reali `file:riga — anteprima`, create con F11
//! sulla riga corrente del documento attivo e persistite nella sessione.
//! La lista appartiene all'app (che la salva nello snapshot): il tool la
//! mostra e chiede apertura o rimozione tramite `ToolRequest`.

use std::path::Path;

use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Alignment, Element, Length, Padding};
use lume_store::BookmarkRecord;
use lume_ui::icons::Icon;
use lume_ui::styles;
use lume_ui::tool::{Tool, ToolCtx, ToolEvent, ToolId, ToolRequest};

pub const ID: ToolId = ToolId("bookmarks");

#[derive(Clone, Debug)]
pub enum Event {
    Replace(Vec<BookmarkRecord>),
    Open(usize),
    Remove(usize),
}

pub struct BookmarksTool {
    items: Vec<BookmarkRecord>,
}

impl BookmarksTool {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }
}

impl Default for BookmarksTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Aggiunge il segnalibro o lo toglie se esiste già (stesso file e riga).
/// Restituisce true se è stato aggiunto.
pub fn toggle(items: &mut Vec<BookmarkRecord>, bookmark: BookmarkRecord) -> bool {
    match items
        .iter()
        .position(|b| b.path == bookmark.path && b.line == bookmark.line)
    {
        Some(index) => {
            items.remove(index);
            false
        }
        None => {
            items.push(bookmark);
            true
        }
    }
}

/// Nome file per le righe («main.rs»).
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

impl Tool for BookmarksTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Segnalibri"
    }

    fn icon(&self) -> Icon {
        Icon::Bookmarks
    }

    fn feature(&self) -> Option<&'static str> {
        Some("segnalibri")
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        if self.items.is_empty() {
            return container(
                text("Nessun segnalibro. F11 aggiunge la riga corrente.")
                    .size(12)
                    .color(t.muted),
            )
            .padding(Padding::from([2.0, 12.0]))
            .into();
        }
        let mut list = column![].spacing(2);
        for (index, bookmark) in self.items.iter().enumerate() {
            let location = format!("{}:{}", display_name(&bookmark.path), bookmark.line);
            let open = button(
                row![
                    text(location).size(12).color(t.ui),
                    container(
                        text(format!("— {}", bookmark.preview))
                            .size(12)
                            .color(t.muted)
                            .wrapping(text::Wrapping::None)
                    )
                    .clip(true)
                    .width(Length::Fill),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding(Padding::from([3, 6]))
            .style(styles::hover_row(t.ui, t.hover, None, 4.0))
            .on_press(ToolEvent::new(Event::Open(index)));
            let remove = button(text("×").size(12).color(t.faint))
                .padding(Padding::from([2, 6]))
                .style(styles::hover_row(t.faint, t.hover, None, 4.0))
                .on_press(ToolEvent::new(Event::Remove(index)));
            list = list.push(
                row![open, remove, Space::new().width(4)]
                    .spacing(2)
                    .align_y(Alignment::Center),
            );
        }
        scrollable(container(list).width(Length::Fill).padding(pad_list()))
            .height(Length::Fill)
            .into()
    }

    fn update(&mut self, event: &ToolEvent) -> Vec<ToolRequest> {
        match event.downcast::<Event>() {
            Some(Event::Replace(items)) => {
                self.items = items.clone();
                Vec::new()
            }
            Some(Event::Open(index)) => self
                .items
                .get(*index)
                .map(|bookmark| ToolRequest::OpenFileAt {
                    path: bookmark.path.clone(),
                    line: bookmark.line,
                })
                .into_iter()
                .collect(),
            Some(Event::Remove(index)) => vec![ToolRequest::RemoveBookmark(*index)],
            None => Vec::new(),
        }
    }
}

/// Padding del prototipo per le liste dei pannelli laterali: 2 sopra, 12 ai lati e sotto.
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
    use std::path::PathBuf;

    use super::*;

    fn bookmark(line: usize) -> BookmarkRecord {
        BookmarkRecord {
            path: PathBuf::from("C:/repo/src/main.rs"),
            line,
            preview: "fn main() {".into(),
        }
    }

    #[test]
    fn toggle_aggiunge_e_toglie_per_file_e_riga() {
        let mut items = Vec::new();
        assert!(toggle(&mut items, bookmark(7)));
        assert!(toggle(&mut items, bookmark(9)));
        assert_eq!(items.len(), 2);
        assert!(!toggle(&mut items, bookmark(7)));
        assert_eq!(items, vec![bookmark(9)]);
    }

    #[test]
    fn apertura_e_rimozione_passano_dall_app() {
        let mut tool = BookmarksTool::new();
        tool.update(&ToolEvent::new(Event::Replace(vec![bookmark(7)])));
        let requests = tool.update(&ToolEvent::new(Event::Open(0)));
        assert!(matches!(
            &requests[..],
            [ToolRequest::OpenFileAt { path, line: 7 }] if path == &PathBuf::from("C:/repo/src/main.rs")
        ));
        assert!(tool.update(&ToolEvent::new(Event::Open(3))).is_empty());
        let requests = tool.update(&ToolEvent::new(Event::Remove(0)));
        assert!(matches!(&requests[..], [ToolRequest::RemoveBookmark(0)]));
        assert_eq!(display_name(Path::new("C:/repo/src/main.rs")), "main.rs");
    }
}
