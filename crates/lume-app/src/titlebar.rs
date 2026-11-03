//! Title bar (38px, prototipo v2): hamburger o barra classica, nome del
//! progetto e branch corrente, pill «Cerca ovunque» centrata nello spazio
//! libero, badge ambiente e controlli finestra. Il drag della finestra parte
//! dalle zone vuote della barra.
//!
//! Zero-mock: run-config, Esegui e Debug del prototipo non hanno backend e
//! non vengono renderizzati; il branch è un'etichetta (il checkout non c'è ancora).

use iced::widget::{Space, button, container, mouse_area, row, text};
use iced::{Alignment, Background, Border, Element, Length, Padding};
use lume_theme::LumeTheme;
use lume_ui::icons::Icon;

use crate::menu::{MENUS, MenuMode, MenuState};
use crate::metrics::TITLEBAR_H;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlebarMessage {
    ToggleMenuRoot,
    OpenClassic(usize),
    HoverClassic(usize),
    Drag,
    DoubleClick,
    Search,
    Minimize,
    ToggleMaximize,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlebarIntent {
    ToggleMenuRoot,
    OpenClassic(usize),
    HoverClassic(usize),
    DragWindow,
    ToggleMaximize,
    OpenSearch,
    MinimizeWindow,
    CloseWindow,
}

#[derive(Debug, Default)]
pub struct TitlebarState;

impl TitlebarState {
    pub fn update(&mut self, message: TitlebarMessage) -> TitlebarIntent {
        match message {
            TitlebarMessage::ToggleMenuRoot => TitlebarIntent::ToggleMenuRoot,
            TitlebarMessage::OpenClassic(index) => TitlebarIntent::OpenClassic(index),
            TitlebarMessage::HoverClassic(index) => TitlebarIntent::HoverClassic(index),
            TitlebarMessage::Drag => TitlebarIntent::DragWindow,
            TitlebarMessage::DoubleClick => TitlebarIntent::ToggleMaximize,
            TitlebarMessage::Search => TitlebarIntent::OpenSearch,
            TitlebarMessage::Minimize => TitlebarIntent::MinimizeWindow,
            TitlebarMessage::ToggleMaximize => TitlebarIntent::ToggleMaximize,
            TitlebarMessage::Close => TitlebarIntent::CloseWindow,
        }
    }
}

/// Dati reali mostrati dalla barra (tutti opzionali: niente segnaposto).
#[derive(Debug, Clone, Copy, Default)]
pub struct TitlebarInfo<'a> {
    /// Nome della cartella progetto aperta.
    pub project: Option<&'a str>,
    /// Branch git corrente del progetto.
    pub branch: Option<&'a str>,
    /// Etichetta dell'ambiente selezionato (badge a destra).
    pub environment: &'a str,
}

#[derive(Debug, Default)]
pub struct Titlebar {
    state: TitlebarState,
    controls: WindowControls,
}

impl Titlebar {
    pub fn update(&mut self, message: TitlebarMessage) -> TitlebarIntent {
        self.state.update(message)
    }

    pub fn view<'a>(
        &'a self,
        theme: &'a LumeTheme,
        menu: &'a MenuState,
        info: TitlebarInfo<'a>,
    ) -> Element<'a, TitlebarMessage> {
        render(theme, menu, info, &self.controls)
    }
}

/// x di partenza della fila menù in modalità classica.
pub const CLASSIC_MENUS_X: f32 = 12.0;

/// Controlli caption per una finestra borderless (prototipo: 28×26, radius 5,
/// «✕» al hover su `bad`).
#[derive(Debug, Default)]
pub struct WindowControls;

impl WindowControls {
    fn view<'a>(&self, t: &'a LumeTheme) -> Element<'a, TitlebarMessage> {
        row![
            caption_button(t, "─", 11.0, TitlebarMessage::Minimize, false),
            caption_button(t, "□", 10.0, TitlebarMessage::ToggleMaximize, false),
            caption_button(t, "✕", 11.0, TitlebarMessage::Close, true),
        ]
        .spacing(0)
        .align_y(Alignment::Center)
        .into()
    }
}

fn caption_button<'a>(
    t: &'a LumeTheme,
    glyph: &'static str,
    size: f32,
    message: TitlebarMessage,
    close: bool,
) -> Element<'a, TitlebarMessage> {
    button(
        container(text(glyph).size(size))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    )
    .width(28)
    .height(26)
    .padding(0)
    .style({
        let (hover, bad, muted, ui) = (t.hover2, t.bad, t.muted, t.ui);
        move |_, status| {
            let active = matches!(
                status,
                iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed
            );
            iced::widget::button::Style {
                background: active.then_some(Background::Color(if close { bad } else { hover })),
                text_color: if active {
                    if close { iced::Color::WHITE } else { ui }
                } else {
                    muted
                },
                border: Border {
                    radius: 5.0.into(),
                    ..Border::default()
                },
                ..iced::widget::button::Style::default()
            }
        }
    })
    .on_press(message)
    .into()
}

fn render<'a>(
    t: &'a LumeTheme,
    menu: &'a MenuState,
    info: TitlebarInfo<'a>,
    controls: &'a WindowControls,
) -> Element<'a, TitlebarMessage> {
    let classic = menu.mode == MenuMode::Classica;

    // cluster destro: badge ambiente + controlli finestra (uguale nei due modi)
    let right = row![
        env_badge(t, info.environment),
        container(controls.view(t)).padding(Padding {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 4.0,
        }),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let base = if classic {
        // barra classica: voci menù, pill 240 centrata nello spazio libero
        let mut menus = row![].spacing(1);
        for (i, m) in MENUS.iter().enumerate() {
            let open = menu.open == Some(i);
            let label = button(text(m.name).size(12.5).color(t.ui))
                .padding(Padding::from([5.0, 9.0]))
                .style(lume_ui::styles::hover_row(
                    t.ui,
                    t.hover2,
                    open.then_some(t.hover2),
                    5.0,
                ))
                .on_press(TitlebarMessage::OpenClassic(i));
            menus = menus.push(mouse_area(label).on_enter(TitlebarMessage::HoverClassic(i)));
        }
        row![
            menus,
            container(search_pill(t, 240.0))
                .width(Length::Fill)
                .align_x(Alignment::Center),
            right,
        ]
    } else {
        let hamburger = button(Icon::Hamburger.view(15.0, t.ui))
            .width(30)
            .height(26)
            .padding(0)
            .style({
                let (hover2, open) = (t.hover2, menu.root_open);
                move |_, status| {
                    let hovered = matches!(
                        status,
                        iced::widget::button::Status::Hovered
                            | iced::widget::button::Status::Pressed
                    );
                    iced::widget::button::Style {
                        background: (open || hovered).then_some(Background::Color(hover2)),
                        border: Border {
                            radius: 5.0.into(),
                            ..Border::default()
                        },
                        ..iced::widget::button::Style::default()
                    }
                }
            })
            .on_press(TitlebarMessage::ToggleMenuRoot);

        // cluster sinistro: hamburger, nome progetto (o «lume»), pill branch
        let mut left = row![container(hamburger).align_y(Alignment::Center)]
            .spacing(8)
            .align_y(Alignment::Center);
        left = left.push(text(info.project.unwrap_or("lume")).size(12.5).color(t.ui));
        if let Some(branch) = info.branch {
            left = left.push(branch_pill(t, branch));
        }
        row![
            left,
            container(search_pill(t, 310.0))
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .padding(Padding::from([0.0, 10.0])),
            right,
        ]
    }
    .spacing(8)
    .height(Length::Fill)
    .padding(Padding {
        top: 0.0,
        right: 12.0,
        bottom: 0.0,
        left: 12.0,
    })
    .align_y(Alignment::Center);

    // il drag della finestra parte dal fondo della barra (le zone interattive
    // catturano prima i loro eventi)
    let base = mouse_area(
        container(base)
            .width(Length::Fill)
            .height(Length::Fill)
            .style({
                let surface = t.surface;
                move |_| container::Style {
                    background: Some(Background::Color(surface)),
                    ..container::Style::default()
                }
            }),
    )
    .on_press(TitlebarMessage::Drag)
    .on_double_click(TitlebarMessage::DoubleClick);

    let sep = container(Space::new())
        .width(Length::Fill)
        .height(1)
        .style({
            let border = t.border;
            move |_| container::Style {
                background: Some(Background::Color(border)),
                ..container::Style::default()
            }
        });

    iced::widget::column![
        container(base).width(Length::Fill).height(TITLEBAR_H - 1.0),
        sep
    ]
    .into()
}

/// Pill «⎇ main» (12px `sec`, padding 3×7, radius 5). Solo etichetta:
/// il cambio branch non c'è ancora.
fn branch_pill<'a>(t: &LumeTheme, branch: &'a str) -> Element<'a, TitlebarMessage> {
    container(text(format!("⎇ {branch}")).size(12).color(t.sec))
        .padding(Padding::from([3, 7]))
        .style(|_| container::Style {
            border: Border {
                radius: 5.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

/// Pill «Cerca ovunque»: 270px con kbd hint (hamburger) o 240px (classica).
fn search_pill(t: &LumeTheme, width: f32) -> Element<'_, TitlebarMessage> {
    let show_kbd = width >= 270.0;
    // ATTENZIONE: **Scostamento voluto dal prototipo.** Lì la pill è una flex col kbd
    // spinto a destra da `margin-left:auto`, quindi lente ed etichetta stanno
    // **a sinistra**. Richiesta esplicita dell'utente: testo al centro del box.
    //
    // Il centro è esatto **per costruzione**, non per calcolo: a sinistra va
    // una copia invisibile del kbd, che occupa esattamente la sua larghezza
    // perché è lo stesso widget con gli stessi testo, corpo e padding. Fra le
    // due copie, due `Fill` uguali attorno all'etichetta.
    //
    // Due tentativi buttati prima di questo, entrambi visti a schermo: il kbd
    // su un livello sovrapposto e poi la pill allargata a 290px. In tutti e
    // due l'etichetta finiva sotto il kbd («Cerca ovunqueMaiusc Maiusc»),
    // perché stimavo le larghezze del testo invece di lasciarle misurare al
    // layout. Qui non c'è niente da stimare.
    let kbd = |visibile: bool| {
        let (testo, bordo) = if visibile {
            (t.muted, t.border2)
        } else {
            (iced::Color::TRANSPARENT, iced::Color::TRANSPARENT)
        };
        container(text("Maiusc Maiusc").size(10).color(testo))
            .padding(Padding::from([1.0, 5.0]))
            .style(move |_| container::Style {
                border: Border {
                    color: bordo,
                    width: 1.0,
                    radius: 3.0.into(),
                },
                ..container::Style::default()
            })
    };

    let etichetta = row![
        Icon::Search.view(13.0, t.sec),
        text("Cerca ovunque").size(12).color(t.sec),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let inner: Element<'_, TitlebarMessage> = if show_kbd {
        row![
            kbd(false),
            Space::new().width(Length::Fill),
            etichetta,
            Space::new().width(Length::Fill),
            kbd(true),
        ]
        .align_y(Alignment::Center)
        .into()
    } else {
        row![
            Space::new().width(Length::Fill),
            etichetta,
            Space::new().width(Length::Fill),
        ]
        .align_y(Alignment::Center)
        .into()
    };
    mouse_area(
        container(inner)
            .width(width)
            .height(25)
            .padding(Padding::from([0.0, 10.0]))
            .align_y(Alignment::Center)
            .style({
                let (inset, border2) = (t.inset, t.border2);
                move |_| container::Style {
                    background: Some(Background::Color(inset)),
                    border: Border {
                        color: border2,
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..container::Style::default()
                }
            }),
    )
    .on_press(TitlebarMessage::Search)
    .interaction(iced::mouse::Interaction::Pointer)
    .into()
}

fn env_badge<'a>(t: &LumeTheme, env: &'a str) -> Element<'a, TitlebarMessage> {
    container(
        row![
            container(Space::new()).width(6).height(6).style({
                let good = t.good;
                move |_| container::Style {
                    background: Some(Background::Color(good)),
                    border: Border {
                        radius: 99.0.into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                }
            }),
            text(env).size(11).color(t.accent),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([3, 9]))
    .style({
        let sel = t.sel;
        move |_| container::Style {
            background: Some(Background::Color(sel)),
            border: Border {
                color: sel,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        }
    })
    .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{TitlebarInfo, TitlebarIntent, TitlebarMessage, TitlebarState};

    #[test]
    fn titlebar_double_click_requests_toggle_maximize() {
        assert_eq!(
            TitlebarState.update(TitlebarMessage::DoubleClick),
            TitlebarIntent::ToggleMaximize
        );
    }

    #[test]
    fn windows_caption_buttons_emit_real_window_intents() {
        for (message, expected) in [
            (TitlebarMessage::Minimize, TitlebarIntent::MinimizeWindow),
            (
                TitlebarMessage::ToggleMaximize,
                TitlebarIntent::ToggleMaximize,
            ),
            (TitlebarMessage::Close, TitlebarIntent::CloseWindow),
        ] {
            assert_eq!(TitlebarState.update(message), expected);
        }
    }

    #[test]
    fn info_senza_progetto_ne_branch_non_ha_segnaposto() {
        let info = TitlebarInfo::default();
        assert!(info.project.is_none());
        assert!(info.branch.is_none());
        assert_eq!(info.environment, "");
    }
}
