//! Viste: composizione dell'area centrale, delle colonne, dei pannelli e dei dock.
//!
//! Metodi di `Lume`: lo stato e i messaggi restano in `app.rs`.

use super::*;
use iced::widget::column;

impl Lume {
    #[cfg(feature = "mcp")]
    pub(crate) fn mcp_card_view(&self) -> Option<crate::mcp_card::CardView> {
        self.mcp
            .as_ref()
            .and_then(|mcp| crate::mcp_card::CardView::of(&mcp.gate))
    }

    pub fn view(&self) -> Element<'_, Message> {
        #[cfg(feature = "performance-diagnostics")]
        let _phase = lume_ui::performance_diagnostics::Span::new("app.view");
        // Il primo passaggio di qui è il momento che il budget chiama
        // «finestra interattiva»: prima non c'è niente da disegnare. Dopo la
        // prima volta è un confronto atomico e basta.
        crate::startup::segna_primo_frame();
        self.shell.view(self)
    }

    /// Vista codice: EditorView sul documento attivo, placeholder altrimenti.
    /// Cosa occupa l'area centrale. Nel prototipo è una precedenza, non un
    /// tipo di tab: `showDiff` vince su tutto, poi `showSql` se la tab attiva
    /// è `console.sql`, poi l'editor.
    pub(super) fn central_view(&self) -> CentralView {
        if self.diff.is_some() {
            return CentralView::Diff;
        }
        // ATTENZIONE: **del gruppo 0**, non di quello col fuoco. Con la console a
        // sinistra e il fuoco a destra, `active_doc()` è la scheda di destra:
        // la precedenza sarebbe caduta su `Editor` e la console si sarebbe
        // mostrata come testo grezzo, senza barra né griglia.
        #[cfg(feature = "sql")]
        if self.sql_console.is_some()
            && self
                .groups
                .active_in(crate::editor_groups::GroupId::Primary)
                == self.sql_console
        {
            return CentralView::Sql;
        }
        CentralView::Editor
    }

    /// Chi occupa l'area centrale **di un gruppo**.
    ///
    /// Il gruppo 1 è **sempre** editor: è la lettura del prototipo, ed è anche
    /// il flusso utile — il file a destra, il suo diff a sinistra. Diff e
    /// console SQL sono viste del gruppo 0 e non si duplicano.
    ///
    /// È la funzione su cui si asserisce: quel che esce da
    /// `group_central_view` è un `Element`, cioè opaco.
    pub(super) fn central_view_of(&self, group: crate::editor_groups::GroupId) -> CentralView {
        if group == crate::editor_groups::GroupId::Secondary {
            return CentralView::Editor;
        }
        self.central_view()
    }

    pub(super) fn group_central_view(
        &self,
        group: crate::editor_groups::GroupId,
    ) -> Element<'_, Message> {
        match self.central_view_of(group) {
            CentralView::Diff => match self.diff.as_ref() {
                Some(diff) => crate::diff_view::view(diff, self.theme()),
                None => self.editor_of(group),
            },
            #[cfg(feature = "sql")]
            CentralView::Sql => crate::sql_console::view(self, self.theme()),
            CentralView::Editor => self.editor_of(group),
        }
    }

    /// L'editor vero e proprio. Separato perché la console SQL lo incorpora:
    /// il suo testo è un `Document` come gli altri.
    #[cfg(feature = "sql")]
    pub(crate) fn editor_element_raw(&self) -> Element<'_, Message> {
        // il gruppo è fissato per lo stesso motivo di `central_view`: la
        // console vive nel gruppo 0, e il suo editor è quello di sinistra
        // anche mentre il fuoco è nella colonna di destra
        self.editor_of(crate::editor_groups::GroupId::Primary)
    }

    /// L'editor di un gruppo. I comandi che ne escono portano **il gruppo**:
    /// così battere in una colonna ci sposta il fuoco senza che serva un
    /// gesto apposta, e senza intercettare i click con un `mouse_area` che
    /// starebbe fra l'utente e l'editor.
    pub(super) fn editor_of(&self, group: crate::editor_groups::GroupId) -> Element<'_, Message> {
        let active = self.groups.active_in(group);
        if let Some((id, doc)) = active.and_then(|id| self.workspace.get(id).map(|doc| (id, doc))) {
            let syntax = self.syntax.get(&id);
            let highlights: &[std::ops::Range<usize>] = self
                .find
                .as_ref()
                .filter(|f| active == Some(f.doc))
                .map(|f| f.matches.as_slice())
                .unwrap_or(&[]);
            let marks: &[lume_ui::editor::DiagnosticMark] =
                self.doc_marks.get(&id).map_or(&[], Vec::as_slice);
            let blame = self.editor_blame_cache.get(&id).map(Vec::as_slice);
            let note_ranges: &[std::ops::Range<usize>] = self
                .notes
                .as_ref()
                .and_then(|notes| notes.marcatori.get(&id))
                .map_or(&[], Vec::as_slice);
            let fold_spans: &[lume_ui::editor::FoldSpan] = self
                .fold_projections
                .get(&id)
                .map_or(&[], lume_ui::editor::FoldProjection::spans);
            let overlay = EditorOverlay {
                marks,
                blame,
                popup: self.completion_popup(id),
                complete_enabled: self
                    .lsp
                    .holding(id)
                    .is_some_and(crate::lsp_mode::LspWorkspace::can_complete),
                hover: self.hover_card(id),
                hover_enabled: self
                    .lsp
                    .holding(id)
                    .is_some_and(crate::lsp_mode::LspWorkspace::can_hover),
                note_ranges,
                fold_spans,
            };
            let lane = match group {
                crate::editor_groups::GroupId::Primary => lume_syntax::SyntaxLane::Primary,
                crate::editor_groups::GroupId::Secondary => lume_syntax::SyntaxLane::Secondary,
            };
            let epoch = self.git_root_epoch;
            let snapshot = self
                .syntax_snapshots
                .get(&lane)
                .filter(|(document, root, snapshot)| {
                    *document == id
                        && *root == epoch
                        && syntax.is_some_and(|language| snapshot.language() == language.lang())
                })
                .map(|(_, _, snapshot)| snapshot);
            let view =
                EditorView::from_snapshot(doc, snapshot, self.theme(), highlights, move |cmd| {
                    Message::EditorIn(group, cmd)
                })
                .overlay(overlay)
                .focus_request(self.editor_focus_epoch, self.groups.focus() == group);
            let view = if let Some(language) = syntax {
                view.syntax_context(epoch, self.syntax_pool.receiver(lane), language.lang())
                    .on_syntax_viewport(move |revision, window| {
                        Message::SyntaxViewport(super::syntax::ViewportRequest {
                            group,
                            document: id,
                            epoch,
                            revision,
                            window,
                        })
                    })
            } else {
                view
            };
            return Element::new(view);
        }
        editor_empty::empty_state(self.theme())
    }

    /// L'area editor intera: uno o due gruppi con la maniglia in mezzo.
    pub(crate) fn editor_area<'a>(&'a self, shell: &'a AppShell) -> Element<'a, Message> {
        use crate::editor_groups::GroupId;
        let primary = self.editor_group_view(shell, GroupId::Primary);
        if !self.groups.is_split() {
            return primary;
        }
        let (a, b) = self.split_portions();
        row![
            container(primary).width(Length::FillPortion(a)),
            self.vsep(),
            self.split_handle(),
            container(self.editor_group_view(shell, GroupId::Secondary))
                .width(Length::FillPortion(b)),
        ]
        .height(Length::Fill)
        .into()
    }

    /// Il rapporto in porzioni intere: `FillPortion` non prende frazioni, e
    /// il layout non deve conoscere i pixel.
    pub(super) fn split_portions(&self) -> (u16, u16) {
        let primo = (self.layout.split_ratio * 1000.0).round().clamp(1.0, 999.0) as u16;
        (primo, 1000 - primo)
    }

    /// Un gruppo: la sua barra schede e la sua area centrale.
    pub(super) fn editor_group_view<'a>(
        &'a self,
        shell: &'a AppShell,
        group: crate::editor_groups::GroupId,
    ) -> Element<'a, Message> {
        let bg = self.theme().bg;
        let mut column = column![
            shell.tab_bar(self, group),
            self.hsep(),
            crate::breadcrumb::view(self.theme(), &self.crumbs_of(group)),
        ];
        if let Some(doc) = self
            .groups
            .active_in(group)
            .filter(|doc| self.conflitto_della_scheda(*doc))
        {
            column = column.push(crate::note_bars::conflict_bar(self.theme(), doc));
        }
        if let Some(strip) = self.striscia_collegamenti(group) {
            column = column.push(strip);
        }
        container(column.push(self.group_central_view(group)))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| styles::bg(bg))
            .into()
    }

    /// Maniglia verticale fra i due gruppi.
    pub(crate) fn split_handle(&self) -> Element<'_, Message> {
        let accent_dim = self.theme().accent_dim;
        mouse_area(hover(
            Space::new().width(HANDLE).height(Length::Fill),
            container(Space::new())
                .width(HANDLE)
                .height(Length::Fill)
                .style(move |_| styles::bg(accent_dim)),
        ))
        .on_press(Message::DragStart(HandleKind::Split))
        .interaction(mouse::Interaction::ResizingHorizontally)
        .into()
    }

    /// Larghezza stimata dell'area editor: serve solo a convertire il
    /// trascinamento della maniglia in una frazione. Le due barre attività e
    /// il pannello aperto sono l'unica cosa che le sta accanto.
    pub(super) fn editor_width(&self) -> f32 {
        let pannello = if self.registry.active_panel.is_some() {
            self.layout.panel_width + crate::metrics::HANDLE + 2.0
        } else {
            0.0
        };
        (self.layout.window.width - 2.0 * crate::metrics::ACTIVITY_W - pannello).max(1.0)
    }

    // ------------------------------------------------------------- helpers

    pub(crate) fn tool_view(&self, id: ToolId) -> Element<'_, Message> {
        let Some(tool) = self.registry.get(id) else {
            return Space::new().into();
        };
        let ctx = ToolCtx {
            theme: self.theme(),
            active_path: self.active_path(),
            body_height: (tool.kind() == ToolKind::Dock)
                .then_some(self.registry.dock_height - crate::metrics::DOCK_HEADER_H - 1.0),
            file_icons: Some(&self.icon_themes),
            open_diff: self.open_diff_ref(),
        };
        tool.view(&ctx).map(move |ev| Message::Tool(id, ev))
    }

    pub(crate) fn side_panel(&self, id: ToolId) -> Element<'_, Message> {
        let t = self.theme();
        let title = self
            .registry
            .get(id)
            .map(|tool| tool.title().to_uppercase())
            .unwrap_or_default();
        let control = crate::features::feature_of(id).and_then(|feature| {
            crate::features::stop_control(
                feature,
                self.feature_live_count(feature).unwrap_or(0),
                self.feature_stop_pending
                    .is_some_and(|pending| pending.feature() == feature),
                t,
                crate::features::FeatureControls {
                    stop: Message::Feature(crate::features::FeatureCommand::Stop(feature)),
                    confirm: Message::FeatureStopConfirmed(feature),
                    cancel: Message::FeatureStopCancelled,
                    start: self
                        .feature_can_start(feature)
                        .then(|| Message::Feature(crate::features::FeatureCommand::Start(feature))),
                    start_label: (feature == crate::features::Feature::LanguageServer)
                        .then(|| crate::lsp_mode::lsp_start_label(self.lsp_active()))
                        .flatten(),
                },
            )
        });
        let mut header = row![
            text(title).size(10.5).color(t.sec),
            Space::new().width(Length::Fill),
        ];
        if let Some(control) = control {
            header = header.push(control);
        }
        let header = header
            .push(
                button(text("×").size(14).color(t.muted).line_height(1.0))
                    .padding(0)
                    .style(styles::hover_row(t.muted, t.surface, None, 3.0))
                    .on_press(Message::ClosePanel),
            )
            .spacing(6)
            .padding(Padding {
                top: 8.0,
                right: 12.0,
                bottom: 6.0,
                left: 12.0,
            })
            .align_y(Alignment::Center);

        let bg = t.bg;
        container(column![header, self.tool_view(id)])
            .width(self.layout.panel_width)
            .height(Length::Fill)
            .style(move |_| styles::bg(bg))
            .into()
    }

    /// Separatore verticale 1px (bordi strutturali del prototipo).
    pub(crate) fn vsep(&self) -> Element<'_, Message> {
        let border = self.theme().border;
        container(Space::new())
            .width(1)
            .height(Length::Fill)
            .style(move |_| styles::bg(border))
            .into()
    }

    pub(crate) fn hsep(&self) -> Element<'_, Message> {
        let border = self.theme().border;
        container(Space::new())
            .width(Length::Fill)
            .height(1)
            .style(move |_| styles::bg(border))
            .into()
    }

    /// Maniglia verticale del pannello laterale (col-resize, hover accent-dim).
    pub(crate) fn panel_handle(&self) -> Element<'_, Message> {
        let accent_dim = self.theme().accent_dim;
        mouse_area(hover(
            Space::new().width(HANDLE).height(Length::Fill),
            container(Space::new())
                .width(HANDLE)
                .height(Length::Fill)
                .style(move |_| styles::bg(accent_dim)),
        ))
        .on_press(Message::DragStart(HandleKind::Panel))
        .interaction(mouse::Interaction::ResizingHorizontally)
        .into()
    }

    /// Dock: header (titolo, badge, «—») + corpo del tool attivo.
    pub(crate) fn dock_view(&self, id: ToolId) -> Element<'_, Message> {
        let t = self.theme();
        let (title, badge) = self
            .registry
            .get(id)
            .map(|tool| (tool.title(), tool.badge()))
            .unwrap_or(("", None));
        let control = crate::features::feature_of(id).and_then(|feature| {
            crate::features::stop_control(
                feature,
                self.feature_live_count(feature).unwrap_or(0),
                self.feature_stop_pending
                    .is_some_and(|pending| pending.feature() == feature),
                t,
                crate::features::FeatureControls {
                    stop: DockMessage::FeatureStop(feature),
                    confirm: DockMessage::FeatureStopConfirmed(feature),
                    cancel: DockMessage::FeatureStopCancelled,
                    start: self
                        .feature_can_start(feature)
                        .then_some(DockMessage::FeatureStart(feature)),
                    start_label: (feature == crate::features::Feature::LanguageServer)
                        .then(|| crate::lsp_mode::lsp_start_label(self.lsp_active()))
                        .flatten(),
                },
            )
        });
        let bg = t.bg;
        container(column![
            self.dock
                .header(t, title, badge, control)
                .map(Message::Dock),
            self.hsep(),
            self.tool_view(id),
        ])
        .width(Length::Fill)
        .height(self.registry.dock_height)
        .style(move |_| styles::bg(bg))
        .into()
    }

    /// Maniglia orizzontale sopra il dock (row-resize).
    pub(crate) fn dock_handle(&self) -> Element<'_, Message> {
        let accent_dim = self.theme().accent_dim;
        mouse_area(hover(
            Space::new().width(Length::Fill).height(HANDLE),
            container(Space::new())
                .width(Length::Fill)
                .height(HANDLE)
                .style(move |_| styles::bg(accent_dim)),
        ))
        .on_press(Message::DragStart(HandleKind::Dock))
        .interaction(mouse::Interaction::ResizingVertically)
        .into()
    }

    /// Strisce invisibili ai bordi per il resize della finestra borderless.
    pub(crate) fn resize_grips(&self) -> Element<'_, Message> {
        use iced::window::Direction;
        let grip = |w: Length, h: Length, dir: Direction, cursor: mouse::Interaction| {
            mouse_area(Space::new().width(w).height(h))
                .on_press(Message::WindowResizeEdge(dir))
                .interaction(cursor)
        };
        let g = 4.0;
        let edges = column![
            // riga superiore: angolo NW, bordo N, angolo NE
            row![
                grip(
                    g.into(),
                    g.into(),
                    Direction::NorthWest,
                    mouse::Interaction::ResizingDiagonallyDown
                ),
                grip(
                    Length::Fill,
                    g.into(),
                    Direction::North,
                    mouse::Interaction::ResizingVertically
                ),
                grip(
                    g.into(),
                    g.into(),
                    Direction::NorthEast,
                    mouse::Interaction::ResizingDiagonallyUp
                ),
            ],
            // fascia centrale: bordo W, spazio (non interattivo), bordo E
            row![
                grip(
                    g.into(),
                    Length::Fill,
                    Direction::West,
                    mouse::Interaction::ResizingHorizontally
                ),
                Space::new().width(Length::Fill).height(Length::Fill),
                grip(
                    g.into(),
                    Length::Fill,
                    Direction::East,
                    mouse::Interaction::ResizingHorizontally
                ),
            ]
            .height(Length::Fill),
            // riga inferiore: angolo SW, bordo S, angolo SE
            row![
                grip(
                    g.into(),
                    g.into(),
                    Direction::SouthWest,
                    mouse::Interaction::ResizingDiagonallyUp
                ),
                grip(
                    Length::Fill,
                    g.into(),
                    Direction::South,
                    mouse::Interaction::ResizingVertically
                ),
                grip(
                    g.into(),
                    g.into(),
                    Direction::SouthEast,
                    mouse::Interaction::ResizingDiagonallyDown
                ),
            ],
        ];
        edges.into()
    }
}
