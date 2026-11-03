//! Barre contestuali mostrate sopra il testo degli Appunti.

use iced::widget::{Space, button, container, row, scrollable, text};
use iced::{Alignment, Background, Border, Element, Length, Padding};
use lume_core::DocId;
use lume_notes::{CodeLink, LinkIndex};
use lume_theme::LumeTheme;
use lume_ui::styles;
use std::path::Path;

use crate::app::Message;

pub(crate) fn conflict_bar<'a>(theme: &LumeTheme, doc: DocId) -> Element<'a, Message> {
    let background = theme.warn_bg;
    let border = theme.warn_br;
    let action = |label: &'static str, copy: bool| {
        button(text(label).size(12).color(theme.text))
            .padding(Padding::from([3, 10]))
            .style(styles::hover_row(theme.text, theme.hover, None, 4.0))
            .on_press(Message::NotesResolveConflict { doc, copia: copy })
    };
    let content = row![
        text("Questa nota è cambiata sul disco mentre la modificavi.")
            .size(12)
            .color(theme.warn),
        Space::new().width(Length::Fill),
        action("Ricarica", false),
        action("Salva copia", true),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    container(content)
        .width(Length::Fill)
        .padding(Padding::from([5, 12]))
        .style(move |_| container::Style {
            background: Some(Background::Color(background)),
            border: Border {
                color: border,
                width: 1.0,
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

const STRIP_H: f32 = 28.0;
const TITOLO_MAX: usize = 28;

pub(crate) struct Chip {
    pub(crate) label: String,
    pub(crate) on_press: Message,
    pub(crate) remove: Option<Message>,
}

pub(crate) fn range_label(da: usize, a: usize) -> String {
    if da == a {
        format!("riga {da}")
    } else {
        format!("righe {da}-{a}")
    }
}

pub(crate) fn abbrevia(testo: &str, massimo: usize) -> String {
    if testo.chars().count() <= massimo {
        return testo.to_owned();
    }
    let mut corto: String = testo.chars().take(massimo.saturating_sub(1)).collect();
    corto.push('…');
    corto
}

pub(crate) fn chips_for_code(indice: &LinkIndex, radice: &Path, file: &Path) -> Vec<Chip> {
    let Some(relativo) = lume_notes::relative_path(radice, file) else {
        return Vec::new();
    };
    indice
        .for_file(radice, &relativo)
        .iter()
        .map(|voce| {
            let titolo = if voce.title.is_empty() {
                "Senza titolo"
            } else {
                voce.title.as_str()
            };
            Chip {
                label: format!(
                    "Appunto: {} · {}",
                    abbrevia(titolo, TITOLO_MAX),
                    range_label(voce.from, voce.to)
                ),
                on_press: Message::NotesOpenLinked(voce.note.clone()),
                remove: None,
            }
        })
        .collect()
}

pub(crate) fn chips_for_note(doc: DocId, collegamenti: &[CodeLink]) -> Vec<Chip> {
    collegamenti
        .iter()
        .enumerate()
        .map(|(index, link)| {
            let percorso = link.path.to_string_lossy().replace('\\', "/");
            let righe = if link.from == link.to {
                link.from.to_string()
            } else {
                format!("{}-{}", link.from, link.to)
            };
            let simbolo = link
                .symbol
                .as_deref()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default();
            Chip {
                label: format!("{percorso}:{righe}{simbolo}"),
                on_press: Message::NotesOpenCode { doc, index },
                remove: Some(Message::NotesRemoveLink { doc, index }),
            }
        })
        .collect()
}

pub(crate) fn strip<'a>(t: &LumeTheme, chips: Vec<Chip>) -> Element<'a, Message> {
    let mut fila = row![].spacing(6).align_y(Alignment::Center);
    for chip in chips {
        let titolo = button(text(chip.label).size(11).color(t.accent))
            .padding(Padding::from([1, 8]))
            .style(styles::hover_row(t.accent, t.hover, None, 10.0))
            .on_press(chip.on_press);
        let mut gruppo = row![titolo].align_y(Alignment::Center);
        if let Some(rimuovi) = chip.remove {
            gruppo = gruppo.push(
                button(text("×").size(11).color(t.muted))
                    .padding(Padding::from([1, 5]))
                    .style(styles::hover_row(t.muted, t.hover, None, 10.0))
                    .on_press(rimuovi),
            );
        }
        fila = fila.push(gruppo);
    }
    let sfondo = t.surface;
    let bordo = t.border;
    container(
        scrollable(fila)
            .direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::new().width(3).scroller_width(3),
            ))
            .width(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fixed(STRIP_H))
    .padding(Padding::from([3, 12]))
    .style(move |_| container::Style {
        background: Some(Background::Color(sfondo)),
        border: Border {
            color: bordo,
            width: 1.0,
            ..Border::default()
        },
        ..container::Style::default()
    })
    .into()
}
