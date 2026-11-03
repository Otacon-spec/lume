//! Libreria di style-fn: ogni funzione prende i token del tema e restituisce
//! uno stile iced. Il chrome non usa mai il Theme iced, sempre i token.

use iced::border::Radius;
use iced::widget::{button, container, text, text_input, tooltip};
use iced::{Background, Border, Color, Element, Padding, Shadow, Vector};
use lume_theme::LumeTheme;

pub fn bg(color: Color) -> container::Style {
    container::Style {
        background: Some(Background::Color(color)),
        ..container::Style::default()
    }
}

/// Superficie con un solo bordo attivo (bordi 1px del prototipo: le barre
/// hanno border-bottom/top/left/right singoli — iced ha bordi uniformi,
/// quindi il "bordo singolo" si realizza con un quad separato; qui restituiamo
/// solo il fondo).
pub fn surface(t: &LumeTheme) -> container::Style {
    bg(t.surface)
}

pub fn inset(t: &LumeTheme) -> container::Style {
    bg(t.inset)
}

/// Popup dei menù: bg surface, bordo border2, radius 9, ombra del prototipo.
pub fn popup(t: &LumeTheme) -> container::Style {
    container::Style {
        background: Some(Background::Color(t.surface)),
        border: Border {
            color: t.border2,
            width: 1.0,
            radius: Radius::from(9.0),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.55),
            offset: Vector::new(0.0, 14.0),
            blur_radius: 36.0,
        },
        ..container::Style::default()
    }
}

/// Popup piccoli (env, tema): radius 8, ombra 0 12 32 .5.
pub fn popup_small(t: &LumeTheme) -> container::Style {
    container::Style {
        background: Some(Background::Color(t.surface)),
        border: Border {
            color: t.border2,
            width: 1.0,
            radius: Radius::from(8.0),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: Vector::new(0.0, 12.0),
            blur_radius: 32.0,
        },
        ..container::Style::default()
    }
}

/// Un suggerimento sopra un controllo: riquadro `surface` con bordo `border2`.
pub fn with_hint<'a, M: 'a>(
    t: &LumeTheme,
    content: impl Into<Element<'a, M>>,
    title: &'static str,
) -> Element<'a, M> {
    let (fill, line, ink) = (t.surface, t.border2, t.text);
    let label = container(text(title).size(11).color(ink)).padding(Padding::from([3, 7]));
    tooltip(content, label, tooltip::Position::Top)
        .gap(4)
        .style(move |_| container::Style {
            background: Some(Background::Color(fill)),
            border: Border {
                color: line,
                width: 1.0,
                radius: iced::border::Radius::from(5.0),
            },
            ..container::Style::default()
        })
        .into()
}

/// Pannello dell'overlay «Cerca ovunque»: radius 10, ombra 0 24 64 .6.
pub fn overlay_panel(t: &LumeTheme) -> container::Style {
    container::Style {
        background: Some(Background::Color(t.surface)),
        border: Border {
            color: t.border2,
            width: 1.0,
            radius: Radius::from(10.0),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.6),
            offset: Vector::new(0.0, 24.0),
            blur_radius: 64.0,
        },
        ..container::Style::default()
    }
}

/// Bottone "riga hoverable" generico: trasparente, hover con colore dato.
/// `radius` e colori testo espliciti; usato per righe di menù/alberi/liste.
pub fn hover_row(
    text: Color,
    hover_bg: Color,
    active_bg: Option<Color>,
    radius: f32,
) -> impl Fn(&iced::Theme, button::Status) -> button::Style {
    move |_, status| {
        let background = match status {
            button::Status::Hovered | button::Status::Pressed => Some(Background::Color(hover_bg)),
            _ => active_bg.map(Background::Color),
        };
        button::Style {
            background,
            text_color: text,
            border: Border {
                radius: Radius::from(radius),
                ..Border::default()
            },
            ..button::Style::default()
        }
    }
}

/// Cella icona 30×30 della activity bar.
pub fn icon_cell(
    t: &LumeTheme,
    active: bool,
) -> impl Fn(&iced::Theme, button::Status) -> button::Style + use<> {
    let (bg_idle, hover2) = (if active { Some(t.sel) } else { None }, t.hover2);
    move |_, status| {
        let background = match status {
            button::Status::Hovered | button::Status::Pressed => Some(Background::Color(hover2)),
            _ => bg_idle.map(Background::Color),
        };
        button::Style {
            background,
            border: Border {
                radius: Radius::from(5.0),
                ..Border::default()
            },
            ..button::Style::default()
        }
    }
}

/// Input testuale del prototipo: bg inset, bordo border2 (focus accent-dim), radius 5.
pub fn input(
    t: &LumeTheme,
) -> impl Fn(&iced::Theme, text_input::Status) -> text_input::Style + use<> {
    let t = t.clone();
    move |_, status| {
        let border_color = match status {
            text_input::Status::Focused { .. } | text_input::Status::Hovered => t.accent_dim,
            _ => t.border2,
        };
        text_input::Style {
            background: Background::Color(t.inset),
            border: Border {
                color: border_color,
                width: 1.0,
                radius: Radius::from(5.0),
            },
            icon: t.icon,
            placeholder: t.muted,
            value: t.text,
            selection: t.sel,
        }
    }
}
