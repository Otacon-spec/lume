//! Stato vuoto dell'editor; codice e tab hanno widget dedicati.

use iced::widget::{container, text};
use iced::{Alignment, Element, Length};
use lume_theme::LumeTheme;

use crate::app::Message;

pub fn empty_state(t: &LumeTheme) -> Element<'_, Message> {
    container(
        text("Apri una cartella (File → Apri…) per iniziare")
            .size(12.5)
            .color(t.muted),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .into()
}
