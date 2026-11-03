//! Status bar (26px): picker ambiente e stato reale del documento attivo.

use iced::widget::{Space, button, container, row, text};
use iced::{Alignment, Background, Element, Length, Padding};
use lume_core::{Document, LineEnding};
use lume_theme::LumeTheme;

use crate::app::{Notice, NoticeKind};
use crate::environment_picker::{EnvironmentPicker, EnvironmentPickerMessage};
use crate::metrics::STATUS_H;
use lume_ui::styles;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusbarMessage {
    Environment(EnvironmentPickerMessage),
    /// «✕» su una notifica persistente.
    ChiudiNotifica,
    /// «Copia» su una notifica persistente: il testo va negli appunti.
    CopiaNotifica,
    ShowExtensions,
    McpMode(u8),
    McpCards,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusbarIntent {
    Environment(EnvironmentPickerMessage),
    ChiudiNotifica,
    CopiaNotifica,
    ShowExtensions,
    McpMode(u8),
    McpCards,
}

#[derive(Debug, Default)]
pub struct Statusbar;

/// Cosa la barra ha da dire. Un gruppo e non cinque parametri sciolti: sono
/// tutte letture dello **stesso** istante, e crescono ogni volta che una
/// funzionalità impara a parlare.
#[derive(Default)]
pub struct StatusbarInfo<'a> {
    pub branch: Option<&'a str>,
    pub document: Option<&'a Document>,
    pub watcher_error: Option<&'a str>,
    /// Perché il documento attivo è senza colori (query di evidenziazione
    /// che non compila). `None` quando tutto va.
    pub syntax_error: Option<&'a str>,
    /// Il language server, quando ha qualcosa di vero da dire.
    pub lsp: Option<String>,
    /// Stato dell'autosalvataggio della nota attiva.
    pub nota: Option<String>,
    /// Il server MCP, solo quando è acceso.
    pub mcp: Option<String>,
    /// Indice della modalità attiva, solo quando il server è acceso.
    pub mcp_mode: Option<u8>,
    /// Richieste in attesa di conferma.
    pub mcp_pending: usize,
    pub notice: Option<&'a Notice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DocumentStatus {
    line: usize,
    column: usize,
    line_ending: &'static str,
}

impl Statusbar {
    pub fn update(&mut self, message: StatusbarMessage) -> StatusbarIntent {
        match message {
            StatusbarMessage::Environment(message) => StatusbarIntent::Environment(message),
            StatusbarMessage::ChiudiNotifica => StatusbarIntent::ChiudiNotifica,
            StatusbarMessage::CopiaNotifica => StatusbarIntent::CopiaNotifica,
            StatusbarMessage::ShowExtensions => StatusbarIntent::ShowExtensions,
            StatusbarMessage::McpMode(index) => StatusbarIntent::McpMode(index),
            StatusbarMessage::McpCards => StatusbarIntent::McpCards,
        }
    }

    pub fn view<'a>(
        &'a self,
        theme: &'a LumeTheme,
        environment: &'a EnvironmentPicker,
        info: StatusbarInfo<'a>,
    ) -> Element<'a, StatusbarMessage> {
        let StatusbarInfo {
            branch,
            document,
            watcher_error,
            syntax_error,
            lsp,
            nota,
            mcp,
            mcp_mode,
            mcp_pending,
            notice,
        } = info;
        let status = document.map(document_status);
        let (encoding, indentation, line_ending, mut caret) = match status {
            Some(status) => (
                "UTF-8",
                "4 spazi",
                status.line_ending,
                format!("{}:{}", status.line, status.column),
            ),
            None => ("", "", "", String::new()),
        };
        if let Some(count) = document
            .map(|doc| doc.selections().ranges().len())
            .filter(|count| *count > 1)
        {
            caret.push_str(&format!(" · {count} selezioni"));
        }
        let mut bar = row![environment.pill(theme).map(StatusbarMessage::Environment)];
        if let Some(branch) = branch {
            // «⎇ main» (senza ↑↓: ahead/behind non è ancora calcolato)
            bar = bar.push(text(format!("⎇ {branch}")).size(11).color(theme.icon));
        }
        if let Some(label) = watcher_status_text(watcher_error) {
            bar = bar.push(text(label).size(11).color(theme.bad));
        }
        if let Some(label) = syntax_status_text(syntax_error) {
            bar = bar.push(text(label).size(11).color(theme.warn));
        }
        // Il language server, quando c'è qualcosa di vero da dire. A riposo
        // non si scrive «fermo»: sarebbe rumore permanente, non
        // informazione.
        if let Some(label) = lsp {
            bar = bar.push(text(label).size(11).color(theme.sec));
        }
        if let Some(label) = nota {
            bar = bar.push(text(label).size(11).color(theme.warn));
        }
        if let Some(label) = mcp {
            bar = bar.push(
                button(text(label).size(11).color(theme.sec))
                    .padding(0)
                    .style(lume_ui::styles::hover_row(
                        theme.sec,
                        theme.hover,
                        None,
                        3.0,
                    ))
                    .on_press(StatusbarMessage::ShowExtensions),
            );
        }
        if let Some(current) = mcp_mode {
            let mut choices = row![text("Agente").size(11).color(theme.icon)]
                .spacing(4)
                .align_y(Alignment::Center);
            for choice in mcp_choices(current) {
                let (color, background) = match (choice.active, choice.risky) {
                    (true, true) => (theme.warn, Some(theme.warn_bg)),
                    (true, false) => (theme.text, Some(theme.sel)),
                    (false, true) => (theme.warn, None),
                    (false, false) => (theme.sec, None),
                };
                choices = choices.push(
                    button(text(choice.label).size(11))
                        .padding(Padding::from([0, 4]))
                        .style(styles::hover_row(color, theme.hover, background, 3.0))
                        .on_press(StatusbarMessage::McpMode(choice.index)),
                );
            }
            bar = bar.push(choices);
        }
        if let Some(label) = mcp_pending_text(mcp_pending) {
            bar = bar.push(
                button(text(label).size(11).color(theme.warn))
                    .padding(0)
                    .style(styles::hover_row(theme.warn, theme.hover, None, 3.0))
                    .on_press(StatusbarMessage::McpCards),
            );
        }
        if let Some(notice) = notice {
            // Le notifiche normali spariscono dopo 5 s (il timer sta in
            // `Lume::notify`); quelle persistenti hanno «Copia» e «✕».
            let color = match notice.kind {
                NoticeKind::Error => theme.warn,
                NoticeKind::Info => theme.sec,
            };
            bar = bar.push(text(notice.text.as_str()).size(11).color(color));
            if notice.persistente {
                bar = bar
                    .push(
                        button(text("Copia").size(11).color(theme.sec))
                            .padding(Padding::from([0, 4]))
                            .style(lume_ui::styles::hover_row(
                                theme.sec,
                                theme.hover2,
                                None,
                                4.0,
                            ))
                            .on_press(StatusbarMessage::CopiaNotifica),
                    )
                    .push(
                        button(text("✕").size(11).color(theme.sec))
                            .padding(Padding::from([0, 4]))
                            .style(lume_ui::styles::hover_row(
                                theme.sec,
                                theme.hover2,
                                None,
                                4.0,
                            ))
                            .on_press(StatusbarMessage::ChiudiNotifica),
                    );
            }
        }
        bar = bar
            .push(Space::new().width(Length::Fill))
            .push(text(encoding).size(11).color(theme.icon))
            .push(text(indentation).size(11).color(theme.icon))
            .push(text(line_ending).size(11).color(theme.icon))
            .push(text(caret).size(11).color(theme.icon))
            .spacing(12)
            .height(Length::Fill)
            .padding(Padding::from([0, 10]))
            .align_y(Alignment::Center);
        let separator = container(Space::new())
            .width(Length::Fill)
            .height(1)
            .style({
                let border = theme.border;
                move |_| container::Style {
                    background: Some(Background::Color(border)),
                    ..container::Style::default()
                }
            });
        iced::widget::column![
            separator,
            container(bar)
                .width(Length::Fill)
                .height(STATUS_H - 1.0)
                .style({
                    let surface = theme.surface;
                    move |_| container::Style {
                        background: Some(Background::Color(surface)),
                        ..container::Style::default()
                    }
                })
        ]
        .into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpChoice {
    pub index: u8,
    pub label: &'static str,
    pub active: bool,
    pub risky: bool,
}

#[cfg(feature = "mcp")]
pub fn mcp_choices(active: u8) -> Vec<McpChoice> {
    let current = lume_mcp::Mode::from_index(active);
    lume_mcp::Mode::ALL
        .iter()
        .map(|mode| McpChoice {
            index: mode.index(),
            label: mode.label(),
            active: *mode == current,
            risky: *mode == lume_mcp::Mode::Brave,
        })
        .collect()
}

#[cfg(not(feature = "mcp"))]
pub fn mcp_choices(_active: u8) -> Vec<McpChoice> {
    Vec::new()
}

pub fn mcp_pending_text(pending: usize) -> Option<String> {
    match pending {
        0 => None,
        1 => Some("1 richiesta in attesa".to_owned()),
        n => Some(format!("{n} richieste in attesa")),
    }
}

fn watcher_status_text(error: Option<&str>) -> Option<String> {
    error.map(|error| format!("⚠ Watcher FS: {error}"))
}

fn syntax_status_text(error: Option<&str>) -> Option<String> {
    error
        .and_then(|error| error.lines().next())
        .map(|riga| format!("ATTENZIONE: evidenziazione non disponibile: {riga}"))
}

fn document_status(document: &Document) -> DocumentStatus {
    let rope = document.rope();
    let caret = document.selection().head.min(rope.len_chars());
    let zero_based_line = rope.char_to_line(caret);
    let line_start = rope.line_to_char(zero_based_line);
    DocumentStatus {
        line: zero_based_line + 1,
        column: caret.saturating_sub(line_start) + 1,
        line_ending: match document.line_ending() {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use lume_core::{DocId, Document, Range};

    use super::{
        Statusbar, StatusbarIntent, StatusbarMessage, document_status, syntax_status_text,
        watcher_status_text,
    };

    #[test]
    fn status_uses_real_caret_and_line_ending() {
        let mut document = Document::from_str("uno\r\ndue", None, DocId(1));
        document.set_selection(Range::point(6));
        let status = document_status(&document);
        assert_eq!(status.line, 2);
        assert_eq!(status.column, 3);
        assert_eq!(status.line_ending, "CRLF");
    }

    #[test]
    fn watcher_failure_has_an_always_visible_status_label() {
        assert_eq!(
            watcher_status_text(Some("accesso negato")).as_deref(),
            Some("⚠ Watcher FS: accesso negato")
        );
        assert!(watcher_status_text(None).is_none());
    }

    #[test]
    fn a_broken_highlight_query_has_a_visible_status_label() {
        assert_eq!(
            syntax_status_text(Some("Query error at 3:2. Invalid node type foo")).as_deref(),
            Some(
                "ATTENZIONE: evidenziazione non disponibile: Query error at 3:2. Invalid node type foo"
            )
        );
        assert_eq!(
            syntax_status_text(Some("prima riga\nseconda riga")).as_deref(),
            Some("ATTENZIONE: evidenziazione non disponibile: prima riga"),
            "un messaggio su più righe non deve sfondare la barra"
        );
        assert!(syntax_status_text(None).is_none());
    }

    #[test]
    fn clic_mcp_instrada_verso_estensioni() {
        assert_eq!(
            Statusbar.update(StatusbarMessage::ShowExtensions),
            StatusbarIntent::ShowExtensions
        );
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn il_selettore_ha_tre_scelte_e_una_sola_attiva() {
        let choices = super::mcp_choices(1);
        let seen: Vec<(u8, &str, bool, bool)> = choices
            .iter()
            .map(|choice| (choice.index, choice.label, choice.active, choice.risky))
            .collect();
        assert_eq!(
            seen,
            [
                (0, "Non brave", false, false),
                (1, "Auto", true, false),
                (2, "Brave", false, true),
            ]
        );
        assert!(
            super::mcp_choices(200)
                .first()
                .is_some_and(|choice| choice.active)
        );
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn il_contatore_delle_richieste_parla_solo_se_ce_ne_sono() {
        assert_eq!(super::mcp_pending_text(0), None);
        assert_eq!(
            super::mcp_pending_text(1).as_deref(),
            Some("1 richiesta in attesa")
        );
        assert_eq!(
            super::mcp_pending_text(3).as_deref(),
            Some("3 richieste in attesa")
        );
    }

    #[cfg(feature = "mcp")]
    #[test]
    fn i_tasti_del_selettore_e_il_contatore_arrivano_all_app_come_intenti() {
        assert_eq!(
            Statusbar.update(StatusbarMessage::McpMode(2)),
            StatusbarIntent::McpMode(2)
        );
        assert_eq!(
            Statusbar.update(StatusbarMessage::McpCards),
            StatusbarIntent::McpCards
        );
    }
}
