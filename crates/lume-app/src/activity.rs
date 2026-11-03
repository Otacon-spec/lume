//! Activity bar (42px): rendering e stato hover/drag locali.
//!
//! Barra sinistra a due gruppi (prototipo v2): in alto i tool a pannello
//! laterale, in basso — sopra l'ingranaggio — gli strumenti del dock.
//! La barra destra ha un solo gruppo.

use iced::widget::{Space, button, column, container};
use iced::{Alignment, Element, Length, Padding, Point};
use lume_theme::LumeTheme;
use lume_ui::Registry;
use lume_ui::icons::Icon;
use lume_ui::styles;
use lume_ui::tool::{Side, ToolId, ToolKind};

use crate::metrics::ACTIVITY_W;

/// Cella 30px + gap 2 = passo verticale delle icone.
pub const CELL_PITCH: f32 = 32.0;
/// Padding verticale della barra.
pub const BAR_PAD: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconDrag {
    pub id: ToolId,
    pub active: bool,
    origin: Option<Point>,
    pub cursor: Point,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActivityBarMessage {
    Press(ToolId),
    Release,
    Hover(Option<ToolId>),
    CursorMoved(Point),
    SettingsToggle,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActivityBarIntent {
    None,
    Click(ToolId),
    Drop(IconDrag),
    ToggleSettings,
}

#[derive(Debug, Default)]
pub struct ActivityBarState {
    drag: Option<IconDrag>,
    hover: Option<ToolId>,
}

impl ActivityBarState {
    pub fn update(&mut self, message: ActivityBarMessage) -> ActivityBarIntent {
        match message {
            ActivityBarMessage::Press(id) => {
                self.drag = Some(IconDrag {
                    id,
                    active: false,
                    origin: None,
                    cursor: Point::ORIGIN,
                });
                ActivityBarIntent::None
            }
            ActivityBarMessage::Release => match self.drag.take() {
                Some(drag) if drag.active => ActivityBarIntent::Drop(drag),
                Some(drag) => ActivityBarIntent::Click(drag.id),
                None => ActivityBarIntent::None,
            },
            ActivityBarMessage::Hover(id) => {
                self.hover = id;
                ActivityBarIntent::None
            }
            ActivityBarMessage::CursorMoved(position) => {
                if let Some(drag) = &mut self.drag {
                    let origin = *drag.origin.get_or_insert(position);
                    if !drag.active && position.distance(origin) > 4.0 {
                        drag.active = true;
                    }
                    drag.cursor = position;
                }
                ActivityBarIntent::None
            }
            ActivityBarMessage::SettingsToggle => ActivityBarIntent::ToggleSettings,
        }
    }

    pub fn drag(&self) -> Option<&IconDrag> {
        self.drag.as_ref()
    }
}

/// Gruppo di destinazione di un drop sulla barra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropGroup {
    Top,
    Bottom,
}

/// Slot di inserimento calcolato dalla y del cursore (relativa alla barra).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropSlot {
    pub group: DropGroup,
    /// Indice dentro il gruppo: `n` = in coda.
    pub index: usize,
}

/// Geometria dei due gruppi: gruppo alto da `BAR_PAD`; gruppo basso ancorato
/// al fondo sopra l'ingranaggio (se presente). Nello spazio vuoto tra i due
/// vince il gruppo più vicino.
pub fn drop_slot(y: f32, bar_height: f32, n_top: usize, n_bottom: usize, gear: bool) -> DropSlot {
    let top_end = BAR_PAD + n_top as f32 * CELL_PITCH;
    let gear_block = if gear { 30.0 + 2.0 } else { 0.0 };
    let bottom_start = bar_height - BAR_PAD - gear_block - n_bottom as f32 * CELL_PITCH;
    let in_bottom = n_bottom > 0 && y >= (top_end + bottom_start) / 2.0 && y >= top_end;
    if in_bottom {
        let index = ((y - bottom_start) / CELL_PITCH).floor().max(0.0) as usize;
        DropSlot {
            group: DropGroup::Bottom,
            index: index.min(n_bottom),
        }
    } else {
        let index = ((y - BAR_PAD) / CELL_PITCH).floor().max(0.0) as usize;
        DropSlot {
            group: DropGroup::Top,
            index: index.min(n_top),
        }
    }
}

#[derive(Debug, Default)]
pub struct ActivityBar {
    state: ActivityBarState,
}

impl ActivityBar {
    pub fn annulla_trascinamento(&mut self) {
        self.state.drag = None;
    }

    pub fn update(&mut self, message: ActivityBarMessage) -> ActivityBarIntent {
        self.state.update(message)
    }

    pub fn drag(&self) -> Option<&IconDrag> {
        self.state.drag()
    }

    pub fn view<'a>(
        &'a self,
        registry: &'a Registry,
        side: Side,
        theme: &'a LumeTheme,
        settings_open: bool,
    ) -> Element<'a, ActivityBarMessage> {
        let mut items = column![].spacing(2).align_x(Alignment::Center);
        match side {
            Side::Left => {
                for id in registry.bar_of_kind(Side::Left, ToolKind::SidePanel) {
                    items = items.push(self.cell(registry, id, theme));
                }
                items = items.push(Space::new().height(Length::Fill));
                for id in registry.bar_of_kind(Side::Left, ToolKind::Dock) {
                    items = items.push(self.cell(registry, id, theme));
                }
                items = items.push(
                    button(
                        container(Icon::Settings.view(16.0, theme.muted))
                            .align_x(Alignment::Center)
                            .align_y(Alignment::Center),
                    )
                    .width(30)
                    .height(30)
                    .padding(0)
                    .style(styles::icon_cell(theme, settings_open))
                    .on_press(ActivityBarMessage::SettingsToggle),
                );
            }
            Side::Right => {
                // `registry.bar`, non `registry.right`: la barra grezza
                // contiene anche i tool delle funzionalita' spente.
                for id in registry.bar(Side::Right) {
                    items = items.push(self.cell(registry, id, theme));
                }
            }
        }

        let surface = theme.surface;
        container(items)
            .width(ACTIVITY_W)
            .height(Length::Fill)
            .padding(Padding::from([BAR_PAD, 0.0]))
            .align_x(Alignment::Center)
            .style(move |_| styles::bg(surface))
            .into()
    }

    fn cell<'a>(
        &'a self,
        registry: &'a Registry,
        id: ToolId,
        theme: &'a LumeTheme,
    ) -> Element<'a, ActivityBarMessage> {
        let Some(tool) = registry.get(id) else {
            return Space::new().into();
        };
        let active = registry.is_open(id);
        let hovered = self.state.hover == Some(id);
        let dragging = self
            .state
            .drag
            .as_ref()
            .is_some_and(|drag| drag.active && drag.id == id);
        let color = if active { theme.accent } else { theme.icon };
        let background = if dragging || hovered {
            Some(theme.hover2)
        } else if active {
            Some(theme.sel)
        } else {
            None
        };

        iced::widget::mouse_area(
            container(tool.icon().view(16.0, color))
                .width(30)
                .height(30)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .style(move |_| iced::widget::container::Style {
                    background: background.map(iced::Background::Color),
                    border: iced::Border {
                        radius: 5.0.into(),
                        ..iced::Border::default()
                    },
                    ..iced::widget::container::Style::default()
                }),
        )
        .on_press(ActivityBarMessage::Press(id))
        .on_release(ActivityBarMessage::Release)
        .on_enter(ActivityBarMessage::Hover(Some(id)))
        .on_exit(ActivityBarMessage::Hover(None))
        .interaction(iced::mouse::Interaction::Pointer)
        .into()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use iced::Point;
    use lume_ui::tool::ToolId;

    use super::{
        ActivityBarIntent, ActivityBarMessage, ActivityBarState, DropGroup, DropSlot, drop_slot,
    };

    #[test]
    fn activity_drag_is_owned_by_activity_bar() {
        let mut state = ActivityBarState::default();
        assert_eq!(
            state.update(ActivityBarMessage::Press(ToolId("test"))),
            ActivityBarIntent::None
        );
        assert!(state.drag().is_some());
        state.update(ActivityBarMessage::CursorMoved(Point::ORIGIN));
        state.update(ActivityBarMessage::CursorMoved(Point::new(5.0, 0.0)));
        assert!(state.drag().is_some_and(|drag| drag.active));
    }

    #[test]
    fn slot_di_drop_nei_due_gruppi() {
        // barra 700px, 3 icone in alto, 2 in basso + ingranaggio:
        // gruppo basso parte a 700 - 8 - 32 - 64 = 596
        let slot = |y: f32| drop_slot(y, 700.0, 3, 2, true);
        assert_eq!(
            slot(10.0),
            DropSlot {
                group: DropGroup::Top,
                index: 0
            }
        );
        assert_eq!(
            slot(50.0),
            DropSlot {
                group: DropGroup::Top,
                index: 1
            }
        );
        assert_eq!(
            slot(120.0),
            DropSlot {
                group: DropGroup::Top,
                index: 3
            },
            "in coda al gruppo alto"
        );
        assert_eq!(
            slot(598.0),
            DropSlot {
                group: DropGroup::Bottom,
                index: 0
            }
        );
        assert_eq!(
            slot(640.0),
            DropSlot {
                group: DropGroup::Bottom,
                index: 1
            }
        );
        assert_eq!(
            slot(690.0),
            DropSlot {
                group: DropGroup::Bottom,
                index: 2
            },
            "in coda al gruppo basso"
        );
        // nello spazio vuoto vince il gruppo più vicino
        assert_eq!(slot(560.0).group, DropGroup::Bottom);
        assert_eq!(slot(200.0).group, DropGroup::Top);
        // barra destra: nessun gruppo basso
        assert_eq!(
            drop_slot(650.0, 700.0, 2, 0, false),
            DropSlot {
                group: DropGroup::Top,
                index: 2
            }
        );
    }
}
