//! Dock unificato in fondo alla colonna centrale (prototipo v2): un solo
//! tool alla volta (Terminale, Log Git, Problemi), header 34px con titolo,
//! badge opzionale e «—» che chiude. Il corpo è la view del tool attivo.

use iced::font::Weight;
use iced::widget::{Space, button, container, row, text};
use iced::{Alignment, Border, Element, Length, Padding};
use lume_theme::LumeTheme;
use lume_ui::ui_font;

use crate::metrics::DOCK_HEADER_H;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockMessage {
    /// «—» nell'header: chiude il dock (equivale a `ToolRequest::CloseSelf`).
    Close,
    /// «Ferma» sulla funzionalità del tool nel dock.
    FeatureStart(crate::features::Feature),
    FeatureStop(crate::features::Feature),
    FeatureStopConfirmed(crate::features::Feature),
    FeatureStopCancelled,
}

#[derive(Debug, Default)]
pub struct Dock;

impl Dock {
    /// Header del dock: titolo 12px/500, badge pill 9.5px (se presente),
    /// «—» a destra. Il bordo inferiore lo aggiunge la shell (hsep).
    pub fn header<'a>(
        &self,
        theme: &'a LumeTheme,
        title: &'a str,
        badge: Option<String>,
        control: Option<Element<'a, DockMessage>>,
    ) -> Element<'a, DockMessage> {
        let mut bar = row![
            text(title)
                .size(12)
                .font(ui_font(Weight::Medium))
                .color(theme.text)
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if let Some(badge) = badge {
            let (bg, fg) = (theme.hover2, theme.sec);
            bar = bar.push(
                container(text(badge).size(9.5).color(fg))
                    .padding(Padding::from([1, 6]))
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(bg)),
                        border: Border {
                            radius: 99.0.into(),
                            ..Border::default()
                        },
                        ..container::Style::default()
                    }),
            );
        }
        bar = bar.push(Space::new().width(Length::Fill));
        if let Some(control) = control {
            bar = bar.push(control);
        }
        bar = bar.push(
            button(text("—").size(12).color(theme.sec))
                .padding(Padding::from([0, 6]))
                .style(close_style(theme))
                .on_press(DockMessage::Close),
        );
        let surface = theme.bg;
        container(bar)
            .width(Length::Fill)
            .height(DOCK_HEADER_H)
            .padding(Padding::from([0, 12]))
            .align_y(Alignment::Center)
            .style(move |_| lume_ui::styles::bg(surface))
            .into()
    }
}

/// «—»: testo `sec`, al hover `text`; nessuno sfondo (come il prototipo).
fn close_style(theme: &LumeTheme) -> impl Fn(&iced::Theme, button::Status) -> button::Style {
    let (sec, hovered) = (theme.sec, theme.text);
    move |_, status| button::Style {
        background: None,
        text_color: match status {
            button::Status::Hovered | button::Status::Pressed => hovered,
            _ => sec,
        },
        ..button::Style::default()
    }
}
