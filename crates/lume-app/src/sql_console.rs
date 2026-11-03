//! La console `console.sql`: barra, editor, griglia risultati, barra di stato.
//!
//! L'editor **è quello vero**: il testo della console vive in un `Document`
//! senza path, quindi rope, undo, selezione ed evidenziazione SQL arrivano
//! gratis da `lume-ui`. Qui si disegna solo ciò che l'editor non è.
//!
//! In modalità confronto i riquadri sono due ma la query è **una**: è il senso
//! della funzione — stessa interrogazione su due motori, celle divergenti
//! evidenziate. Il prototipo dà a ogni riquadro un testo suo; quella variante
//! non serve al caso d'uso e costerebbe un secondo documento.

use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Alignment, Color, Element, Length, Padding};
use lume_sql::{ColumnInfo, ResultSet, Value};
use lume_theme::LumeTheme;
use lume_ui::styles;

use crate::app::{Lume, Message};
use crate::sql_mode::{
    PAGE_SIZE, Pane, PaneStatus, RESULT_NOT_RESTORED, SortDirection, compare_values,
};

pub fn grid_scroll_id(index: usize) -> iced::widget::Id {
    match index {
        0 => iced::widget::Id::new("sql-grid-0"),
        1 => iced::widget::Id::new("sql-grid-1"),
        _ => iced::widget::Id::new("sql-grid-0"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SqlMessage {
    /// Esegue la query di tutti i riquadri attivi (Ctrl+↵ o «Esegui»).
    Run,
    ToggleCompare,
    ToggleSyncScroll,
    SwapPanes,
    ToggleConnectionMenu(usize),
    SelectConnection(usize, String),
    SelectResultSet(usize, usize),
    /// Ordina il riquadro per la colonna indicata; ripremendo inverte.
    Sort(usize, String),
    PreviousPage(usize),
    NextPage(usize),
    /// Scroll verticale di un riquadro (per sincronizzare A e B).
    GridScrolled {
        pane: usize,
        offset_y: f32,
    },
    /// Abbandona il risultato atteso. Su Azure SQL la query in corso viene
    /// interrotta anche sul server (Attention TDS) e la connessione si
    /// ricrea alla richiesta successiva; su Oracle la query può continuare
    /// sul server e la connessione resta occupata finché il driver torna.
    DiscardResult,
    ContinueResult(usize),
    StopResult(usize),
}

pub fn view<'a>(app: &'a Lume, theme: &'a LumeTheme) -> Element<'a, Message> {
    let workspace = app.sql_workspace();
    let compare = workspace.is_some_and(|w| w.compare);
    let sync_scroll = workspace.is_some_and(|w| w.sync_scroll);

    let mut toolbar = row![
        icon_button("▶", theme.good, theme, SqlMessage::Run),
        separator(theme),
        toggle_button("◫ Confronta", compare, theme, SqlMessage::ToggleCompare),
    ]
    .spacing(4)
    .padding(Padding::from([4.0, 10.0]))
    .align_y(Alignment::Center);
    if workspace.is_some_and(|w| {
        w.active_panes()
            .iter()
            .any(|pane| pane.status == PaneStatus::Running)
    }) {
        toolbar = toolbar.push(separator(theme)).push(icon_button(
            "Scarta risultato",
            theme.warn,
            theme,
            SqlMessage::DiscardResult,
        ));
    }
    if let Some(workspace) = workspace {
        for (index, pane) in workspace.active_panes().iter().enumerate() {
            if pane.status == PaneStatus::Paused {
                toolbar = toolbar
                    .push(separator(theme))
                    .push(icon_button(
                        "Continua",
                        theme.good,
                        theme,
                        SqlMessage::ContinueResult(index),
                    ))
                    .push(icon_button(
                        "Ferma acquisizione",
                        theme.warn,
                        theme,
                        SqlMessage::StopResult(index),
                    ));
            }
        }
    }
    if compare {
        toolbar = toolbar
            .push(toggle_button(
                "⇅ Scroll sincronizzato",
                sync_scroll,
                theme,
                SqlMessage::ToggleSyncScroll,
            ))
            .push(icon_button("⇄", theme.sec, theme, SqlMessage::SwapPanes));
    }

    let panes: Vec<&Pane> = workspace
        .map(|w| w.active_panes().iter().collect())
        .unwrap_or_default();

    let mut grids = row![].spacing(0);
    if panes.is_empty() {
        grids = grids.push(
            container(
                text("Nessuna origine dati collegata.")
                    .size(12)
                    .color(theme.muted),
            )
            .center_x(Length::Fill)
            .padding(20),
        );
    }
    for (index, pane) in panes.iter().enumerate() {
        let other = compare.then(|| panes.get(1 - index).copied()).flatten();
        grids = grids.push(pane_view(app, theme, index, pane, compare, other));
    }

    let surface = theme.surface;
    column![
        container(toolbar)
            .width(Length::Fill)
            .style(move |_| styles::bg(surface)),
        app.editor_element_raw(),
        container(grids).height(Length::Fill),
    ]
    .into()
}

fn pane_view<'a>(
    app: &'a Lume,
    theme: &'a LumeTheme,
    index: usize,
    pane: &'a Pane,
    compare: bool,
    other: Option<&'a Pane>,
) -> Element<'a, Message> {
    let profile = pane
        .connection
        .as_deref()
        .and_then(|id| app.sql_profile(id));
    let label = profile.map_or("nessuna connessione", |p| p.label.as_str());
    let dialect = profile.map_or("", |p| p.engine.dialect());

    let mut header = row![].spacing(6).align_y(Alignment::Center);
    if compare {
        header = header.push(
            text(if index == 0 { "A" } else { "B" })
                .size(10.5)
                .color(theme.sec),
        );
    }
    header = header
        .push(
            button(
                row![
                    text(label).size(12).color(theme.ui),
                    text("▾").size(9).color(theme.faint)
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([2, 6]))
            .style(styles::hover_row(theme.ui, theme.hover, None, 4.0))
            .on_press(Message::Sql(SqlMessage::ToggleConnectionMenu(index))),
        )
        .push(Space::new().width(Length::Fill))
        .push(pager(theme, index, pane));

    let mut body = column![
        container(header)
            .width(Length::Fill)
            .padding(Padding::from([4.0, 10.0])),
    ];
    if pane.connection_menu_open {
        body = body.push(connection_menu(app, theme, index, pane));
    }
    if pane.result_set_count() > 1 {
        let mut tabs = row![].spacing(4);
        for result_index in 0..pane.result_set_count() {
            let selected = pane.selected_result_set() == result_index;
            tabs = tabs.push(
                button(
                    text(format!("Risultato {}", result_index + 1))
                        .size(10.5)
                        .color(if selected { theme.ui } else { theme.muted }),
                )
                .padding(Padding::from([2, 7]))
                .style(styles::hover_row(
                    theme.ui,
                    theme.hover,
                    selected.then_some(theme.hover2),
                    4.0,
                ))
                .on_press(Message::Sql(SqlMessage::SelectResultSet(
                    index,
                    result_index,
                ))),
            );
        }
        body = body.push(container(tabs).padding(Padding::from([2.0, 10.0])));
    }
    body.push(grid(app, theme, index, pane, compare, other))
        .push(status_bar(theme, pane, dialect))
        .width(Length::Fill)
        .into()
}

/// «‹ 1 / N ›» del prototipo. Le frecce si spengono ai bordi invece di
/// sparire, così la barra non balla.
fn pager<'a>(theme: &'a LumeTheme, index: usize, pane: &'a Pane) -> Element<'a, Message> {
    let pages = pane
        .results
        .as_ref()
        .map(|set| set.row_count().div_ceil(PAGE_SIZE).max(1))
        .unwrap_or(1);
    let current = pane.page + 1;
    let previous = arrow(
        theme,
        "‹",
        pane.page > 0,
        index,
        SqlMessage::PreviousPage(index),
    );
    let next = arrow(
        theme,
        "›",
        current < pages,
        index,
        SqlMessage::NextPage(index),
    );
    row![
        previous,
        text(format!("{current} / {pages}"))
            .size(11)
            .color(theme.sec),
        next,
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

fn arrow<'a>(
    theme: &'a LumeTheme,
    glyph: &'a str,
    enabled: bool,
    _pane: usize,
    message: SqlMessage,
) -> Element<'a, Message> {
    let color = if enabled { theme.ui } else { theme.faint };
    let mut widget = button(text(glyph).size(12).color(color))
        .padding(Padding::from([1, 5]))
        .style(styles::hover_row(color, theme.hover, None, 4.0));
    if enabled {
        widget = widget.on_press(Message::Sql(message));
    }
    widget.into()
}

/// Elenco delle connessioni disponibili per il riquadro.
fn connection_menu<'a>(
    app: &'a Lume,
    theme: &'a LumeTheme,
    index: usize,
    pane: &'a Pane,
) -> Element<'a, Message> {
    let mut list = column![].spacing(1);
    for profile in app.sql_profiles() {
        let selected = pane.connection.as_deref() == Some(profile.id.as_str());
        list = list.push(
            button(
                row![
                    text(&profile.label).size(12).color(theme.ui),
                    Space::new().width(Length::Fill),
                    text(&profile.host).size(10.5).color(theme.muted),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding(Padding::from([3, 8]))
            .style(styles::hover_row(
                theme.ui,
                theme.hover,
                selected.then_some(theme.hover2),
                4.0,
            ))
            .on_press(Message::Sql(SqlMessage::SelectConnection(
                index,
                profile.id.clone(),
            ))),
        );
    }
    container(list)
        .width(Length::Fill)
        .padding(Padding::from([2.0, 10.0]))
        .into()
}

/// La griglia. Larghezze da contenuto come nel prototipo (`max-content`),
/// limitate perché una colonna di testo lungo non spinga fuori le altre.
fn grid<'a>(
    app: &'a Lume,
    theme: &'a LumeTheme,
    index: usize,
    pane: &'a Pane,
    compare: bool,
    other: Option<&'a Pane>,
) -> Element<'a, Message> {
    let Some(results) = pane.results.as_ref() else {
        let placeholder = match &pane.status {
            PaneStatus::Error(message) if message == RESULT_NOT_RESTORED => message.as_str(),
            _ => "Premi Esegui o Ctrl+↵ per lanciare la query",
        };
        return container(text(placeholder).size(12).color(theme.muted))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
    };

    let other_set = other.and_then(|pane| pane.results.as_ref());
    let other_columns = other_set.map(|set| set.columns.as_slice());
    let widths = column_widths(results);
    let mut header = row![].spacing(0);
    for (ci, info) in results.columns.iter().enumerate() {
        let arrow = match pane.sort.as_ref() {
            Some(sort) if sort.column == info.name => match sort.direction {
                SortDirection::Ascending => " ↑",
                SortDirection::Descending => " ↓",
            },
            _ => "",
        };
        let exclusive = compare && column_missing_from(&info.name, other_columns);
        header = header.push(
            button(
                row![
                    text(&info.name).size(11.5).color(theme.ui),
                    text(&info.type_name).size(10).color(theme.muted),
                    text(arrow).size(10).color(theme.accent),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .width(widths.get(ci).copied().unwrap_or(120.0))
            .padding(Padding::from([3, 8]))
            .style(styles::hover_row(
                theme.ui,
                theme.hover,
                exclusive.then_some(theme.warn_bg),
                0.0,
            ))
            .on_press(Message::Sql(SqlMessage::Sort(index, info.name.clone()))),
        );
    }

    let sync_scroll = app
        .sql_workspace()
        .is_some_and(|workspace| workspace.sync_scroll);
    let mut body = column![].spacing(0);
    for slot in 0..pane.page_len() {
        let Some(cells) = pane
            .row_index_at(slot)
            .and_then(|row| results.rows.get(row))
        else {
            continue;
        };
        let other_cells = other
            .and_then(|pane| {
                pane.row_index_at(slot)
                    .and_then(|row| pane.results.as_ref()?.rows.get(row))
            })
            .map(Vec::as_slice);
        let highlights = compare_cells(&results.columns, cells, other_columns, other_cells);
        let mut line = row![].spacing(0);
        for (ci, value) in cells.iter().enumerate() {
            let highlight = compare && highlights.get(ci).copied().unwrap_or(false);
            let warn_bg = theme.warn_bg;
            line = line.push(
                container(
                    text(pane.text_at(slot, ci).unwrap_or_default())
                        .size(11.5)
                        .color(cell_color(theme, value))
                        .wrapping(text::Wrapping::None),
                )
                .clip(true)
                .width(widths.get(ci).copied().unwrap_or(120.0))
                .padding(Padding::from([3, 8]))
                .style(move |_| {
                    if highlight {
                        container::Style {
                            background: Some(iced::Background::Color(warn_bg)),
                            ..container::Style::default()
                        }
                    } else {
                        container::Style::default()
                    }
                }),
            );
        }
        body = body.push(line);
    }

    let surface = theme.surface;
    let scroll_id = grid_scroll_id(index);
    let mut scroller = scrollable(body).height(Length::Fill).id(scroll_id);
    if compare && sync_scroll {
        scroller = scroller.on_scroll(move |viewport| {
            Message::Sql(SqlMessage::GridScrolled {
                pane: index,
                offset_y: viewport.absolute_offset().y,
            })
        });
    }
    column![
        container(header)
            .width(Length::Fill)
            .style(move |_| styles::bg(surface)),
        scroller,
    ]
    .height(Length::Fill)
    .into()
}

fn column_missing_from(name: &str, other_columns: Option<&[ColumnInfo]>) -> bool {
    other_columns.is_some_and(|columns| columns.iter().all(|column| column.name != name))
}

fn compare_cells(
    columns: &[ColumnInfo],
    row: &[Value],
    other_columns: Option<&[ColumnInfo]>,
    other_row: Option<&[Value]>,
) -> Vec<bool> {
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            if column_missing_from(&column.name, other_columns) {
                return true;
            }
            let Some(other_columns) = other_columns else {
                return false;
            };
            let Some(left) = row.get(index) else {
                return false;
            };
            let right = other_columns
                .iter()
                .position(|candidate| candidate.name == column.name)
                .and_then(|ci| other_row.and_then(|row| row.get(ci)));
            cell_differs(left, right)
        })
        .collect()
}

fn cell_differs(left: &Value, right: Option<&Value>) -> bool {
    match right {
        Some(right) => compare_values(Some(left), Some(right)) != std::cmp::Ordering::Equal,
        None => true,
    }
}

fn status_bar<'a>(theme: &'a LumeTheme, pane: &'a Pane, dialect: &'a str) -> Element<'a, Message> {
    let (dot, label, color) = match &pane.status {
        PaneStatus::Idle => ("○", "In attesa".to_owned(), theme.faint),
        // «Scarta risultato» in toolbar, non «Interrompi»: i driver non
        // espongono un break; il gesto onesto abbandona solo l'attesa in UI.
        PaneStatus::Running => ("◐", "In esecuzione…".to_owned(), theme.sec),
        PaneStatus::Paused => (
            "◐",
            "Budget raggiunto: acquisizione in pausa".to_owned(),
            theme.warn,
        ),
        PaneStatus::Ok => ("●", "OK".to_owned(), theme.good),
        PaneStatus::Error(message) => ("●", message.clone(), theme.bad),
    };
    let surface = theme.surface;
    let rows = pane
        .results
        .as_ref()
        .map(|set| format!("{} righe", set.row_count()))
        .unwrap_or_default();
    let elapsed = pane
        .results
        .as_ref()
        .map(|set| format!("{} ms", set.elapsed.as_millis()))
        .unwrap_or_default();
    let memory = if pane.accounted_bytes > 0 {
        format!(
            "{} MiB contabilizzati",
            pane.accounted_bytes / (1024 * 1024)
        )
    } else {
        String::new()
    };
    let incomplete = if pane.incomplete {
        "risultato incompleto"
    } else {
        ""
    };
    container(
        row![
            text(dot).size(9).color(color),
            text(label).size(11).color(theme.sec),
            Space::new().width(Length::Fill),
            text(rows).size(11).color(theme.muted),
            text(memory).size(11).color(theme.muted),
            text(incomplete).size(11).color(theme.warn),
            text(elapsed).size(11).color(theme.muted),
            text(dialect).size(11).color(theme.muted),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([3.0, 10.0]))
    .style(move |_| styles::bg(surface))
    .into()
}

/// Larghezza per colonna, dal contenuto della pagina più l'intestazione.
fn column_widths(results: &ResultSet) -> Vec<f32> {
    const CHAR: f32 = 6.6;
    const MIN: f32 = 70.0;
    const MAX: f32 = 320.0;
    results
        .columns
        .iter()
        .enumerate()
        .map(|(ci, info)| {
            let header = info.name.chars().count() + info.type_name.chars().count() + 4;
            let widest = results
                .rows
                .iter()
                .take(PAGE_SIZE)
                .filter_map(|row| row.get(ci))
                .map(|value| value.to_string().chars().count())
                .max()
                .unwrap_or(0);
            (header.max(widest) as f32 * CHAR + 16.0).clamp(MIN, MAX)
        })
        .collect()
}

fn cell_color(theme: &LumeTheme, value: &Value) -> Color {
    match value {
        Value::Null => theme.faint,
        Value::Int(_) | Value::Float(_) => theme.num,
        Value::Text(_) => theme.string,
        _ => theme.text,
    }
}

fn icon_button<'a>(
    glyph: &'a str,
    color: Color,
    theme: &'a LumeTheme,
    message: SqlMessage,
) -> Element<'a, Message> {
    button(text(glyph).size(12).color(color))
        .padding(Padding::from([3, 7]))
        .style(styles::hover_row(color, theme.hover, None, 4.0))
        .on_press(Message::Sql(message))
        .into()
}

fn toggle_button<'a>(
    label: &'a str,
    active: bool,
    theme: &'a LumeTheme,
    message: SqlMessage,
) -> Element<'a, Message> {
    let color = if active { theme.accent_on } else { theme.sec };
    let background = active.then_some(theme.sel);
    button(text(label).size(11.5).color(color))
        .padding(Padding::from([3, 9]))
        .style(styles::hover_row(color, theme.hover, background, 4.0))
        .on_press(Message::Sql(message))
        .into()
}

fn separator<'a>(theme: &'a LumeTheme) -> Element<'a, Message> {
    let rule = theme.border2;
    container(Space::new().width(1).height(14))
        .style(move |_| styles::bg(rule))
        .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use std::time::Duration;

    use lume_sql::ColumnInfo;

    use super::*;

    fn results() -> ResultSet {
        ResultSet {
            columns: vec![
                ColumnInfo {
                    name: "id".into(),
                    type_name: "int".into(),
                },
                ColumnInfo {
                    name: "email".into(),
                    type_name: "text".into(),
                },
            ],
            rows: vec![
                vec![Value::Int(3), Value::Text("c@x".into())],
                vec![Value::Int(1), Value::Null],
                vec![Value::Int(2), Value::Text("a@x".into())],
            ],
            elapsed: Duration::from_millis(42),
        }
    }

    #[test]
    fn il_confronto_preserva_l_ordine_dei_tipi_e_non_tocca_il_result_set() {
        let set = results();
        assert_eq!(
            compare_values(Some(&set.rows[1][0]), Some(&set.rows[2][0])),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            set.rows[0][0],
            Value::Int(3),
            "le righe restano nell'ordine del server"
        );
    }

    #[test]
    fn i_null_finiscono_in_fondo_in_ordine_crescente() {
        let set = results();
        assert_eq!(
            compare_values(Some(&set.rows[2][1]), Some(&set.rows[1][1])),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn le_larghezze_restano_nei_limiti() {
        let set = results();
        for width in column_widths(&set) {
            assert!((70.0..=320.0).contains(&width), "larghezza {width}");
        }
    }

    #[test]
    fn compare_cells_segna_i_valori_diversi_sulla_stessa_colonna() {
        let left = results();
        let right = ResultSet {
            columns: left.columns.clone(),
            rows: vec![
                vec![Value::Int(3), Value::Text("altro".into())],
                vec![Value::Int(1), Value::Null],
                vec![Value::Int(9), Value::Text("a@x".into())],
            ],
            elapsed: Duration::from_millis(1),
        };
        assert_eq!(
            compare_cells(
                &left.columns,
                &left.rows[0],
                Some(&right.columns),
                Some(&right.rows[0]),
            ),
            vec![false, true]
        );
        assert_eq!(
            compare_cells(
                &left.columns,
                &left.rows[2],
                Some(&right.columns),
                Some(&right.rows[2]),
            ),
            vec![true, false]
        );
    }

    #[test]
    fn compare_cells_tratta_null_e_valore_come_diversi() {
        let columns = vec![ColumnInfo {
            name: "email".into(),
            type_name: "text".into(),
        }];
        assert_eq!(
            compare_cells(
                &columns,
                &[Value::Null],
                Some(&columns),
                Some(&[Value::Text("c@x".into())]),
            ),
            vec![true]
        );
        assert_eq!(
            compare_cells(
                &columns,
                &[Value::Null],
                Some(&columns),
                Some(&[Value::Null]),
            ),
            vec![false]
        );
    }

    #[test]
    fn compare_cells_evidenzia_una_colonna_solo_da_una_parte() {
        let left_columns = vec![
            ColumnInfo {
                name: "id".into(),
                type_name: "int".into(),
            },
            ColumnInfo {
                name: "solo_a".into(),
                type_name: "text".into(),
            },
        ];
        let right_columns = vec![ColumnInfo {
            name: "id".into(),
            type_name: "int".into(),
        }];
        assert_eq!(
            compare_cells(
                &left_columns,
                &[Value::Int(1), Value::Text("x".into())],
                Some(&right_columns),
                Some(&[Value::Int(1)]),
            ),
            vec![false, true]
        );
        assert!(column_missing_from("solo_a", Some(&right_columns)));
        assert!(!column_missing_from("id", Some(&right_columns)));
        let only_b = vec![ColumnInfo {
            name: "solo_b".into(),
            type_name: "text".into(),
        }];
        assert_eq!(
            compare_cells(
                &only_b,
                &[Value::Text("y".into())],
                Some(&left_columns),
                Some(&[Value::Int(1), Value::Text("x".into())]),
            ),
            vec![true]
        );
    }
}
