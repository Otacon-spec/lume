use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Alignment, Background, Border, Element, Length, Padding};
use lume_theme::LumeTheme;

use crate::appearance::{IconThemeRow, IconThemeState};
use crate::metrics::{ACTIVITY_W, STATUS_H};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsMessage {
    Open,
    Dismiss,
    ColorThemePicked(usize),
    IconThemePicked(String),
    RescanIconThemes,
    /// Il selettore dei temi icone sta per diventare visibile: serve la
    /// scansione completa dei pacchetti esterni (una sola volta).
    AppearanceRequested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsIntent {
    None,
    SelectColorTheme(usize),
    SelectIconTheme(String),
    RescanIconThemes,
    ScanIconThemes,
}

#[derive(Debug, Default)]
pub struct ColorThemeSelector {
    selected: usize,
}

#[derive(Debug)]
pub struct IconThemeSelector {
    selected: String,
}

impl Default for IconThemeSelector {
    fn default() -> Self {
        Self {
            selected: "material".to_owned(),
        }
    }
}

#[derive(Debug, Default)]
pub struct SettingsPanel {
    open: bool,
    color_theme: ColorThemeSelector,
    icon_theme: IconThemeSelector,
}

impl SettingsPanel {
    pub fn update(&mut self, message: SettingsMessage) -> SettingsIntent {
        match message {
            SettingsMessage::Open => {
                self.open = true;
                SettingsIntent::None
            }
            SettingsMessage::Dismiss => {
                self.open = false;
                SettingsIntent::None
            }
            SettingsMessage::ColorThemePicked(index) => {
                self.color_theme.selected = index;
                SettingsIntent::SelectColorTheme(index)
            }
            SettingsMessage::IconThemePicked(id) => {
                self.icon_theme.selected.clone_from(&id);
                SettingsIntent::SelectIconTheme(id)
            }
            SettingsMessage::RescanIconThemes => SettingsIntent::RescanIconThemes,
            SettingsMessage::AppearanceRequested => SettingsIntent::ScanIconThemes,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    #[cfg(test)]
    pub fn selected_color_theme(&self) -> usize {
        self.color_theme.selected
    }

    pub fn set_selected_color_theme(&mut self, index: usize) {
        self.color_theme.selected = index;
    }

    pub fn set_selected_icon_theme(&mut self, id: impl Into<String>) {
        self.icon_theme.selected = id.into();
    }

    pub fn view<'a>(
        &'a self,
        current_theme: &'a LumeTheme,
        themes: &'a [LumeTheme],
        icon_themes: &'a IconThemeState,
        persistence_error: Option<&'a str>,
    ) -> Element<'a, SettingsMessage> {
        let mut rows = column![
            container(text("TEMA COLORI").size(10.5).color(current_theme.sec)).padding(Padding {
                top: 6.0,
                right: 10.0,
                bottom: 8.0,
                left: 10.0,
            })
        ];
        for (index, theme) in themes.iter().enumerate() {
            let active = index == self.color_theme.selected;
            let (swatch, swatch_background) = (theme.accent, theme.bg);
            rows = rows.push(
                button(
                    row![
                        container(Space::new())
                            .width(13)
                            .height(13)
                            .style(move |_| container::Style {
                                background: Some(Background::Color(swatch)),
                                border: Border {
                                    color: swatch_background,
                                    width: 2.5,
                                    radius: 99.0.into(),
                                },
                                ..container::Style::default()
                            }),
                        text(theme.name.as_str())
                            .size(12.5)
                            .color(current_theme.text),
                        Space::new().width(Length::Fill),
                        text(if active { "✓" } else { "" })
                            .size(11)
                            .color(current_theme.accent),
                    ]
                    .spacing(9)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding(Padding {
                    top: 6.0,
                    right: 10.0,
                    bottom: 6.0,
                    left: 10.0,
                })
                .style(lume_ui::styles::hover_row(
                    current_theme.text,
                    current_theme.hover2,
                    active.then_some(current_theme.hover2),
                    5.0,
                ))
                .on_press(SettingsMessage::ColorThemePicked(index)),
            );
        }
        rows = rows.push(
            container(text("ICONE FILE").size(10.5).color(current_theme.sec)).padding(Padding {
                top: 12.0,
                right: 10.0,
                bottom: 8.0,
                left: 10.0,
            }),
        );
        for icon_row in icon_themes.rows() {
            rows = rows.push(self.icon_theme.row(icon_row, current_theme));
        }
        rows = rows.push(
            button(text("Rileggi pacchetti esterni").size(11.5))
                .width(Length::Fill)
                .padding(Padding {
                    top: 6.0,
                    right: 10.0,
                    bottom: 6.0,
                    left: 10.0,
                })
                .style(lume_ui::styles::hover_row(
                    current_theme.text,
                    current_theme.hover2,
                    None,
                    5.0,
                ))
                .on_press(SettingsMessage::RescanIconThemes),
        );
        if let Some(error) = persistence_error {
            rows = rows.push(
                container(
                    text(format!("Non salvato: {error}"))
                        .size(10.5)
                        .color(current_theme.bad),
                )
                .padding(10),
            );
        }
        let popup = container(scrollable(rows).height(480))
            .width(200)
            .padding(5)
            .style(|_| lume_ui::styles::popup_small(current_theme));
        // il dock vive nella colonna centrale: il popup si ancora alla status bar
        let bottom = STATUS_H + 8.0;
        column![
            Space::new().height(Length::Fill),
            row![Space::new().width(ACTIVITY_W + 8.0), popup],
            Space::new().height(bottom),
        ]
        .into()
    }
}

impl IconThemeSelector {
    fn row<'a>(
        &'a self,
        icon_theme: IconThemeRow,
        current_theme: &'a LumeTheme,
    ) -> Element<'a, SettingsMessage> {
        let selected = icon_theme.id == self.selected;
        let state = if selected && icon_theme.is_active {
            "✓"
        } else if icon_theme.is_preferred {
            "!"
        } else if icon_theme.is_active {
            "↩"
        } else {
            ""
        };
        let mut content = row![
            text(icon_theme.name.clone())
                .size(12.0)
                .color(current_theme.text)
        ]
        .spacing(7)
        .align_y(Alignment::Center);
        if let Some(preview) = icon_theme.preview {
            content = content
                .push(Space::new().width(Length::Fill))
                .push(preview.folder.view(14.0, current_theme.sec))
                .push(preview.folder_open.view(14.0, current_theme.sec))
                .push(preview.java.view(14.0, current_theme.sec))
                .push(preview.generic_file.view(14.0, current_theme.sec))
                .push(text(state).size(11).color(current_theme.accent));
            return button(content)
                .width(Length::Fill)
                .padding(Padding {
                    top: 6.0,
                    right: 10.0,
                    bottom: 6.0,
                    left: 10.0,
                })
                .style(lume_ui::styles::hover_row(
                    current_theme.text,
                    current_theme.hover2,
                    selected.then_some(current_theme.hover2),
                    5.0,
                ))
                .on_press(SettingsMessage::IconThemePicked(icon_theme.id))
                .into();
        }

        content = content
            .push(Space::new().width(Length::Fill))
            .push(text("non disponibile").size(10.0).color(current_theme.bad));
        column![
            container(content).padding(Padding {
                top: 6.0,
                right: 10.0,
                bottom: 2.0,
                left: 10.0,
            }),
            container(
                text(
                    icon_theme
                        .error
                        .unwrap_or_else(|| "errore sconosciuto".to_owned())
                )
                .size(9.5)
                .color(current_theme.sec)
            )
            .padding(Padding {
                top: 0.0,
                right: 10.0,
                bottom: 6.0,
                left: 10.0,
            })
        ]
        .into()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{SettingsIntent, SettingsMessage, SettingsPanel};

    #[test]
    fn settings_owns_color_selection_and_open_state() {
        let mut panel = SettingsPanel::default();
        assert_eq!(panel.update(SettingsMessage::Open), SettingsIntent::None);
        assert!(panel.is_open());
        assert_eq!(
            panel.update(SettingsMessage::ColorThemePicked(2)),
            SettingsIntent::SelectColorTheme(2)
        );
        assert_eq!(panel.selected_color_theme(), 2);
        assert_eq!(
            panel.update(SettingsMessage::IconThemePicked("catppuccin-mocha".into())),
            SettingsIntent::SelectIconTheme("catppuccin-mocha".into())
        );
    }

    #[test]
    fn first_appearance_request_asks_for_the_external_theme_scan() {
        let mut panel = SettingsPanel::default();
        assert_eq!(
            panel.update(SettingsMessage::AppearanceRequested),
            SettingsIntent::ScanIconThemes
        );
    }
}
