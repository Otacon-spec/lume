use iced::widget::{Space, button, column, container, mouse_area, row, stack, text};
use iced::{Background, Border, Element, Length, Padding, Point};
use lume_ui::styles;
use lume_ui::tool::Side;

pub(crate) use crate::app::AppShell;
use crate::app::{Lume, Message};
use crate::editor_tab_bar::EditorTab;
use crate::overlay_manager::OverlayId;
use crate::statusbar::StatusbarMessage;

impl AppShell {
    /// La barra schede di **un gruppo**: solo le sue schede, e l'attiva è
    /// quella del gruppo — non quella col fuoco, o la colonna senza fuoco
    /// mostrerebbe tutte le schede spente.
    pub fn tab_bar<'a>(
        &'a self,
        app: &'a Lume,
        group: crate::editor_groups::GroupId,
    ) -> Element<'a, Message> {
        let attiva = app.groups.active_in(group);
        let tabs = app.groups.tabs(group).iter().filter_map(move |&id| {
            app.workspace.get(id).map(|document| EditorTab {
                id,
                kind: app.editor_kind(id),
                // lo stesso nome del riquadro «Salvare le modifiche?»: la
                // domanda deve parlare della scheda che si vede
                name: app.nome_documento(id),
                is_dirty: document.is_dirty(),
                is_active: attiva == Some(id),
                path: document.path().map(std::path::Path::to_path_buf),
            })
        });
        app.editor_tab_bar
            .view(group, tabs, app.theme(), &app.icon_themes)
            .map(Message::EditorTabBar)
    }

    pub fn view<'a>(&'a self, app: &'a Lume) -> Element<'a, Message> {
        let theme = app.theme();
        let background = theme.bg;
        let editor = app.editor_area(self);

        let panel = app
            .registry
            .active_panel
            .map(|id| (id, app.registry.side_of(id)));
        // colonna centrale: [pannello | editor] sopra, dock sotto (prototipo v2)
        let mut middle = row![];
        if let Some((id, Side::Left)) = panel {
            middle = middle.push(app.side_panel(id));
            middle = middle.push(app.vsep());
            middle = middle.push(app.panel_handle());
        }
        middle = middle.push(editor);
        if let Some((id, Side::Right)) = panel {
            middle = middle.push(app.panel_handle());
            middle = middle.push(app.vsep());
            middle = middle.push(app.side_panel(id));
        }
        let mut central = column![middle.height(Length::Fill)];
        if let Some(id) = app.registry.active_dock {
            central = central.push(app.dock_handle());
            central = central.push(app.hsep());
            central = central.push(app.dock_view(id));
        }
        let central = row![
            app.activity_bar
                .view(
                    &app.registry,
                    Side::Left,
                    theme,
                    app.settings_panel.is_open(),
                )
                .map(Message::ActivityBar),
            app.vsep(),
            central.width(Length::Fill),
            app.vsep(),
            app.activity_bar
                .view(
                    &app.registry,
                    Side::Right,
                    theme,
                    app.settings_panel.is_open(),
                )
                .map(Message::ActivityBar),
        ];

        let titlebar = app
            .titlebar
            .view(theme, app.menu_bar.state(), app.titlebar_info())
            .map(Message::Titlebar);
        let mut root = column![titlebar, central.height(Length::Fill)];
        let active_document = app.active_doc().and_then(|id| app.workspace.get(id));
        root = root.push(
            app.statusbar
                .view(
                    theme,
                    &app.environment_picker,
                    crate::statusbar::StatusbarInfo {
                        branch: app.branch.as_deref(),
                        document: active_document,
                        watcher_error: app.watcher_error(),
                        syntax_error: app.active_syntax_error(),
                        lsp: crate::lsp_mode::lsp_status_text(app.lsp_focus()),
                        nota: app.etichetta_nota_attiva(),
                        mcp: app.mcp_status_text(),
                        mcp_mode: app.mcp_mode_index(),
                        mcp_pending: app.mcp_pending(),
                        notice: app.notice.as_ref().map(|(_, notice)| notice),
                    },
                )
                .map(Message::Statusbar),
        );
        let base = container(root)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| styles::bg(background));

        let mut layers = stack![base];
        if self.active_overlay().is_some() {
            layers = layers.push(
                mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
                    .on_press(Message::CloseAll),
            );
        }
        match self.active_overlay() {
            Some(OverlayId::Menu) => {
                let overlay_titlebar = app
                    .titlebar
                    .view(theme, app.menu_bar.state(), app.titlebar_info())
                    .map(Message::Titlebar);
                layers = layers.push(column![
                    overlay_titlebar,
                    app.menu_bar
                        .overlay(theme, app.menu_checks())
                        .map(Message::MenuBar)
                ]);
            }
            Some(OverlayId::Environment) => {
                layers = layers.push(
                    app.environment_picker
                        .popup(theme)
                        .map(|message| Message::Statusbar(StatusbarMessage::Environment(message))),
                );
            }
            Some(OverlayId::Settings) => {
                layers = layers.push(
                    app.settings_panel
                        .view(
                            theme,
                            &app.theme_state.list,
                            &app.icon_themes,
                            app.persistence_error.as_deref(),
                        )
                        .map(Message::Settings),
                );
            }
            Some(OverlayId::Context) => {
                if let Some(dove) = app.context_menu.as_ref() {
                    let voci = crate::context_menu::items(&dove.soggetto, app.menu_checks());
                    layers = layers.push(
                        iced::widget::pin(crate::context_menu::view(theme, &voci, |act| {
                            Message::MenuAction(act)
                        }))
                        .position(Point::new(dove.x, dove.y)),
                    );
                }
            }
            Some(OverlayId::Rename) => {
                if let Some(riquadro) = &app.rename {
                    layers = layers
                        .push(crate::prompt::view(&riquadro.prompt(), theme).map(Message::Rename));
                }
            }
            Some(OverlayId::GotoLine) => {
                layers = layers
                    .push(crate::prompt::view(&app.goto_prompt(), theme).map(Message::GotoLine));
            }
            Some(OverlayId::Albero) => {
                // L'albero ha due riquadri: creare e rinominare. Se c'e' la
                // rinomina aperta, e' quella che si disegna.
                if let Some(prompt) = app.albero_rinomina_prompt() {
                    layers = layers
                        .push(crate::prompt::view(&prompt, theme).map(Message::AlberoRinomina));
                } else if let Some(prompt) = app.albero_prompt() {
                    layers = layers.push(crate::prompt::view(&prompt, theme).map(Message::Albero));
                }
            }
            Some(OverlayId::CodeActions) => {
                if !app.code_actions.is_empty() {
                    let mut col = column![].spacing(2);
                    for (i, azione) in app.code_actions.iter().enumerate() {
                        let titolo = match azione {
                            lsp_types::CodeActionOrCommand::CodeAction(a) => a.title.clone(),
                            lsp_types::CodeActionOrCommand::Command(c) => c.title.clone(),
                        };
                        col = col.push(
                            button(text(titolo).size(12).color(theme.text))
                                .width(Length::Fill)
                                .padding(Padding::from([4, 10]))
                                .style(styles::hover_row(theme.text, theme.hover, None, 4.0))
                                .on_press(Message::CodeActionChosen(i)),
                        );
                    }
                    let pannello = container(col)
                        .width(Length::Fixed(360.0))
                        .padding(Padding::from([8, 8]))
                        .style({
                            let t = theme.clone();
                            move |_| container::Style {
                                background: Some(Background::Color(t.surface)),
                                border: Border {
                                    color: t.border,
                                    width: 1.0,
                                    radius: 6.0.into(),
                                },
                                ..container::Style::default()
                            }
                        });
                    layers =
                        layers.push(iced::widget::pin(pannello).position(Point::new(200.0, 150.0)));
                }
            }
            Some(OverlayId::Forge) => {
                if let Some(config) = app.forge_config.as_ref() {
                    layers = layers.push(
                        crate::forge_panel::view(config, app.forge_can_run(), theme)
                            .map(Message::Forge),
                    );
                }
            }
            Some(OverlayId::Configurazioni) => {
                if let Some(editor) = app.configurazioni_editor.as_ref() {
                    layers = layers.push(
                        crate::configurazioni_panel::view(editor, theme)
                            .map(Message::Configurazioni),
                    );
                }
            }
            Some(OverlayId::ConfermaChiusura) => {
                if let Some(riquadro) = app.riquadro_chiusura() {
                    layers = layers.push(
                        crate::conferma_chiusura::view(&riquadro, theme)
                            .map(Message::ConfermaChiusura),
                    );
                }
            }
            Some(OverlayId::ConfermaElimina) => {
                if let Some(riquadro) = app.riquadro_elimina() {
                    layers = layers.push(
                        crate::conferma_elimina::view(&riquadro, theme).map(Message::AlberoElimina),
                    );
                }
            }
            Some(OverlayId::About) => layers = layers.push(crate::about::view(theme)),
            Some(OverlayId::Scorciatoie) => {
                let checks = app.menu_checks();
                let sezioni = crate::scorciatoie::tabella(
                    |voce| crate::menu::item_visible(voce, checks),
                    checks.disabled_features,
                );
                layers = layers.push(crate::scorciatoie::view(theme, &sezioni));
            }
            #[cfg(feature = "mcp")]
            Some(OverlayId::ConsensoMcp) => {
                if let Some(riquadro) = app.riquadro_consenso_mcp() {
                    layers = layers.push(
                        crate::consenso_mcp::view(&riquadro, theme).map(Message::ConsensoMcp),
                    );
                }
            }
            #[cfg(feature = "mcp")]
            Some(OverlayId::CardMcp) => {
                if let Some(card) = app.mcp_card_view() {
                    layers = layers.push(crate::mcp_card::view(&card, theme).map(Message::McpCard));
                }
            }
            Some(OverlayId::Recents) => {
                layers = layers.push(
                    crate::recents::view(theme, &app.recents_rows(), app.recents_selected)
                        .map(Message::Recents),
                );
            }
            Some(OverlayId::Find) => {
                if let Some(trova) = &app.find {
                    layers = layers.push(
                        container(crate::find_replace::view(trova, theme).map(Message::Find))
                            .width(Length::Fill)
                            .align_x(iced::alignment::Horizontal::Right)
                            .padding(12),
                    );
                }
            }
            Some(OverlayId::Search) => {
                layers = layers.push(
                    app.global_search
                        .view(theme, &app.icon_themes)
                        .map(Message::GlobalSearch),
                );
            }
            None => {}
        }
        if let Some(drag) = app.activity_bar.drag()
            && drag.active
            && let Some(tool) = app.registry.get(drag.id)
        {
            let ghost_color = iced::Color {
                a: 0.7,
                ..theme.accent
            };
            layers = layers.push(
                iced::widget::pin(tool.icon().view::<Message>(16.0, ghost_color))
                    .position(Point::new(drag.cursor.x - 8.0, drag.cursor.y - 8.0)),
            );
        }
        if let Some(drag) = app.editor_tab_bar.drag()
            && drag.active
            && app.workspace.get(drag.id).is_some()
        {
            // Stessa etichetta di barra e conferma: conserva il numero del
            // file nuovo e il titolo Appunti anche durante il trascinamento.
            let nome = app.nome_documento(drag.id);
            let ghost = iced::Color {
                a: 0.75,
                ..theme.accent
            };
            layers = layers.push(
                iced::widget::pin(
                    iced::widget::text(nome)
                        .size(12)
                        .color(ghost)
                        .wrapping(iced::widget::text::Wrapping::None),
                )
                .position(Point::new(drag.cursor.x + 10.0, drag.cursor.y - 8.0)),
            );
        }
        layers = layers.push(app.resize_grips());
        layers.into()
    }
}
