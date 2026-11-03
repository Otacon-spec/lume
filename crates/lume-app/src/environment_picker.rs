//! Selettore ambiente nella status bar: pill + popup verso l'alto (290px).
//! I profili sono reali (Locale + distro WSL rilevate, vedi `environments`);
//! la scelta determina la shell dei nuovi terminali.

use iced::widget::{Space, button, column, container, mouse_area, row, text};
use iced::{Alignment, Background, Border, Element, Length, Padding};
use lume_store::{EnvironmentProfile, LOCAL_ENVIRONMENT_ID};
use lume_theme::LumeTheme;

use crate::environments::{detail_for, local_profile};
use crate::metrics::STATUS_H;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentPickerMessage {
    Toggle,
    Select(usize),
    Dismiss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentPickerIntent {
    None,
    Selected(usize),
}

#[derive(Debug)]
pub struct EnvironmentPicker {
    open: bool,
    profiles: Vec<EnvironmentProfile>,
    selected: String,
}

impl Default for EnvironmentPicker {
    fn default() -> Self {
        Self {
            open: false,
            profiles: vec![local_profile()],
            selected: LOCAL_ENVIRONMENT_ID.into(),
        }
    }
}

impl EnvironmentPicker {
    pub fn update(&mut self, message: EnvironmentPickerMessage) -> EnvironmentPickerIntent {
        match message {
            EnvironmentPickerMessage::Toggle => {
                self.open = !self.open;
                EnvironmentPickerIntent::None
            }
            EnvironmentPickerMessage::Select(index) => {
                if let Some(profile) = self.profiles.get(index) {
                    self.selected = profile.id.clone();
                }
                self.open = false;
                EnvironmentPickerIntent::Selected(self.selected_index())
            }
            EnvironmentPickerMessage::Dismiss => {
                self.open = false;
                EnvironmentPickerIntent::None
            }
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Sostituisce i profili; se quello selezionato sparisce si torna a Locale.
    pub fn set_profiles(&mut self, profiles: Vec<EnvironmentProfile>) {
        self.profiles = if profiles.is_empty() {
            vec![local_profile()]
        } else {
            profiles
        };
        if !self.profiles.iter().any(|p| p.id == self.selected) {
            self.selected = LOCAL_ENVIRONMENT_ID.into();
        }
    }

    /// Seleziona per id (sessione ripristinata); id sconosciuto = Locale.
    pub fn select_id(&mut self, id: &str) {
        self.selected = if self.profiles.iter().any(|p| p.id == id) {
            id.to_string()
        } else {
            LOCAL_ENVIRONMENT_ID.into()
        };
    }

    pub fn profiles(&self) -> &[EnvironmentProfile] {
        &self.profiles
    }

    fn selected_index(&self) -> usize {
        self.profiles
            .iter()
            .position(|p| p.id == self.selected)
            .unwrap_or(0)
    }

    pub fn selected_profile(&self) -> &EnvironmentProfile {
        self.profiles
            .iter()
            .find(|p| p.id == self.selected)
            .or_else(|| self.profiles.first())
            .unwrap_or_else(|| {
                // set_profiles garantisce almeno Locale; fallback difensivo
                unreachable_profile()
            })
    }

    pub fn selected_id(&self) -> &str {
        &self.selected_profile().id
    }

    pub fn label(&self) -> &str {
        &self.selected_profile().label
    }

    pub fn pill<'a>(&'a self, theme: &'a LumeTheme) -> Element<'a, EnvironmentPickerMessage> {
        let label = self.label();
        let open = self.open;
        mouse_area(
            container(
                row![
                    container(Space::new()).width(6).height(6).style({
                        let good = theme.good;
                        move |_| container::Style {
                            background: Some(Background::Color(good)),
                            border: Border {
                                radius: 99.0.into(),
                                ..Border::default()
                            },
                            ..container::Style::default()
                        }
                    }),
                    text(format!("{label} ▴")).size(11).color(theme.accent),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([2, 8]))
            .style({
                let hover = theme.hover2;
                move |_| container::Style {
                    background: open.then_some(Background::Color(hover)),
                    border: Border {
                        radius: 4.0.into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                }
            }),
        )
        .on_press(EnvironmentPickerMessage::Toggle)
        .interaction(iced::mouse::Interaction::Pointer)
        .into()
    }

    pub fn popup<'a>(&'a self, theme: &'a LumeTheme) -> Element<'a, EnvironmentPickerMessage> {
        let header = container(text("AMBIENTE DI SVILUPPO").size(10.5).color(theme.sec))
            .width(Length::Fill)
            .padding(Padding {
                top: 9.0,
                right: 14.0,
                bottom: 9.0,
                left: 14.0,
            });
        let mut rows = column![header, separator(theme)];
        for (index, profile) in self.profiles.iter().enumerate() {
            let active = profile.id == self.selected;
            // Locale e WSL rilevate sono raggiungibili: dot verde
            let dot_color = theme.good;
            let name_color = if active { theme.text } else { theme.ui };
            rows = rows.push(
                button(
                    row![
                        container(Space::new()).width(7).height(7).style(move |_| {
                            container::Style {
                                background: Some(Background::Color(dot_color)),
                                border: Border {
                                    radius: 99.0.into(),
                                    ..Border::default()
                                },
                                ..container::Style::default()
                            }
                        }),
                        text(profile.label.as_str()).size(12.5).color(name_color),
                        Space::new().width(Length::Fill),
                        text(detail_for(profile)).size(11).color(theme.muted),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding(Padding {
                    top: 8.0,
                    right: 14.0,
                    bottom: 8.0,
                    left: 14.0,
                })
                .style(lume_ui::styles::hover_row(
                    name_color,
                    theme.hover,
                    active.then_some(theme.hover2),
                    0.0,
                ))
                .on_press(EnvironmentPickerMessage::Select(index)),
            );
        }
        let popup = container(rows)
            .width(290)
            .style(|_| lume_ui::styles::popup_small(theme));
        column![
            Space::new().height(Length::Fill),
            row![Space::new().width(10.0), popup],
            Space::new().height(STATUS_H + 8.0),
        ]
        .into()
    }
}

fn unreachable_profile() -> &'static EnvironmentProfile {
    static LOCAL: std::sync::LazyLock<EnvironmentProfile> = std::sync::LazyLock::new(local_profile);
    &LOCAL
}

fn separator(theme: &LumeTheme) -> Element<'_, EnvironmentPickerMessage> {
    let border = theme.border;
    container(Space::new())
        .width(Length::Fill)
        .height(1)
        .style(move |_| lume_ui::styles::bg(border))
        .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use lume_store::{EnvironmentKind, EnvironmentProfile};

    use super::{EnvironmentPicker, EnvironmentPickerIntent, EnvironmentPickerMessage};

    #[test]
    fn environment_open_select_and_dismiss_are_local() {
        let mut picker = EnvironmentPicker::default();
        assert_eq!(
            picker.update(EnvironmentPickerMessage::Toggle),
            EnvironmentPickerIntent::None
        );
        assert!(picker.is_open());
        assert_eq!(
            picker.update(EnvironmentPickerMessage::Select(0)),
            EnvironmentPickerIntent::Selected(0)
        );
        assert!(!picker.is_open());
        assert_eq!(picker.label(), "Locale");
    }

    #[test]
    fn profili_reali_selezione_per_id_e_fallback_a_locale() {
        let mut picker = EnvironmentPicker::default();
        picker.set_profiles(vec![
            crate::environments::local_profile(),
            EnvironmentProfile {
                id: "wsl:Ubuntu".into(),
                label: "WSL · Ubuntu".into(),
                kind: EnvironmentKind::Wsl,
            },
        ]);
        assert_eq!(
            picker.update(EnvironmentPickerMessage::Select(1)),
            EnvironmentPickerIntent::Selected(1)
        );
        assert_eq!(picker.label(), "WSL · Ubuntu");
        assert_eq!(picker.selected_id(), "wsl:Ubuntu");
        // id sconosciuto (sessione vecchia): torna a Locale
        picker.select_id("ssh:vps");
        assert_eq!(picker.selected_id(), "local");
        // la distro sparisce: torna a Locale
        picker.select_id("wsl:Ubuntu");
        picker.set_profiles(vec![crate::environments::local_profile()]);
        assert_eq!(picker.label(), "Locale");
    }
}
