//! Riquadro Informazioni leggibile anche quando la status bar è piena.
use crate::app::Message;
use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Alignment, Background, Border, Color, Element, Length};

pub fn view(theme: &lume_theme::LumeTheme) -> Element<'static, Message> {
    let card = container(
        column![
            text("Informazioni su Lume").size(20).color(theme.text),
            scrollable(
                text(crate::updates::informazioni())
                    .size(14)
                    .color(theme.sec)
            )
            .height(Length::Fixed(180.0)),
            row![
                button(text("Copia")).on_press(Message::CopyAbout),
                button(text("Chiudi")).on_press(Message::CloseAll),
            ]
            .spacing(12),
            text("Esc per chiudere").size(11).color(theme.faint),
        ]
        .spacing(16),
    )
    .width(Length::Fill)
    .max_width(580)
    .padding(24)
    .style({
        let background = theme.surface;
        let border = theme.border;
        move |_| container::Style {
            background: Some(Background::Color(background)),
            border: Border {
                color: border,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        }
    });
    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .padding(24)
        .style(|_| container::Style {
            background: Some(Background::Color(Color {
                a: 0.35,
                ..Color::BLACK
            })),
            ..Default::default()
        })
        .into()
}
