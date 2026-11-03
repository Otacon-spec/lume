use iced::widget::{button, column, container, row, text};
use iced::{Element, Length, Padding};
use lsp_types::DiagnosticSeverity;
use lume_lsp::{Problem, ProblemId};
use lume_ui::MONO;
use lume_ui::icons::Icon;
use lume_ui::styles;
use lume_ui::tool::{Tool, ToolCtx, ToolEvent, ToolId, ToolKind, ToolRequest};

use crate::lsp_uri::uri_to_path;

pub const ID: ToolId = ToolId("problems");

#[derive(Debug, Clone)]
pub enum Event {
    Replace(Vec<Problem>),
    Open(ProblemId),
}

/// Le diagnostiche vere. ATTENZIONE: Questa riga era `allow(dead_code)`: nessun
/// chiamante in produzione, dock sempre vuoto, badge mai acceso — l'ultimo
/// mock del progetto. Ora la chiama `Lume::sync_problems_tool`.
pub fn replace_event(problems: Vec<Problem>) -> ToolEvent {
    ToolEvent::new(Event::Replace(problems))
}

pub struct ProblemsTool {
    problems: Vec<Problem>,
}

impl ProblemsTool {
    pub fn new() -> Self {
        Self {
            problems: Vec::new(),
        }
    }
}

impl Default for ProblemsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for ProblemsTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Problemi"
    }

    /// Appartiene al language server: spento quello dal pannello Estensioni,
    /// il dock sparisce dalle barre e dai menù insieme alla sua scorciatoia.
    /// Le righe non sono un guscio: arrivano dai
    /// `publishDiagnostics` di un server vero.
    fn feature(&self) -> Option<&'static str> {
        Some("lsp")
    }

    fn icon(&self) -> Icon {
        Icon::Problems
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Dock
    }

    /// Badge nell'header del dock: numero di problemi (nessuno se zero).
    fn badge(&self) -> Option<String> {
        (!self.problems.is_empty()).then(|| self.problems.len().to_string())
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let theme = ctx.theme;
        let mut list = column![].spacing(1);
        if self.problems.is_empty() {
            list = list.push(
                container(text("Nessun problema").size(12).color(theme.muted))
                    .padding(Padding::from([10.0, 12.0])),
            );
        }
        for problem in &self.problems {
            let severity = match problem.severity {
                Some(DiagnosticSeverity::ERROR) => ("E", theme.bad),
                Some(DiagnosticSeverity::WARNING) => ("W", theme.warn),
                Some(DiagnosticSeverity::INFORMATION) => ("I", theme.sec),
                _ => ("H", theme.muted),
            };
            let location = format!(
                "{}:{}:{}",
                display_name(problem),
                problem.range.start.line + 1,
                problem.range.start.character + 1
            );
            let content = row![
                text(severity.0).size(11).font(MONO).color(severity.1),
                text(&problem.message).size(12).color(theme.text),
                text(location).size(11).font(MONO).color(theme.muted),
                text(problem.server_id.as_str()).size(10).color(theme.faint),
            ]
            .spacing(8)
            .width(Length::Fill)
            .padding(Padding::from([4.0, 8.0]));
            if uri_to_path(&problem.uri).is_some() {
                list = list.push(
                    button(content)
                        .width(Length::Fill)
                        .style(styles::hover_row(theme.text, theme.hover, None, 3.0))
                        .on_press(ToolEvent::new(Event::Open(problem.id))),
                );
            } else {
                list = list.push(container(content).width(Length::Fill));
            }
        }
        // riga di riepilogo (prototipo: «cargo check · 1 avviso, 0 errori»)
        let (errors, warnings) =
            self.problems
                .iter()
                .fold((0usize, 0usize), |(e, w), p| match p.severity {
                    Some(DiagnosticSeverity::ERROR) => (e + 1, w),
                    Some(DiagnosticSeverity::WARNING) => (e, w + 1),
                    _ => (e, w),
                });
        let summary = if self.problems.is_empty() {
            "Nessun problema".to_string()
        } else {
            format!("{errors} errori, {warnings} avvisi")
        };
        column![
            iced::widget::scrollable(list)
                .width(Length::Fill)
                .height(Length::Fill),
            container(text(summary).size(11.5).color(theme.muted))
                .padding(Padding::from([6.0, 12.0])),
        ]
        .into()
    }

    fn update(&mut self, event: &ToolEvent) -> Vec<ToolRequest> {
        match event.downcast::<Event>() {
            Some(Event::Replace(problems)) => {
                self.problems = problems.clone();
                Vec::new()
            }
            Some(Event::Open(id)) => self
                .problems
                .iter()
                .find(|problem| problem.id == *id)
                .and_then(|problem| {
                    uri_to_path(&problem.uri).map(|path| ToolRequest::OpenFileAt {
                        path,
                        line: problem.range.start.line as usize + 1,
                    })
                })
                .into_iter()
                .collect(),
            None => Vec::new(),
        }
    }
}

fn display_name(problem: &Problem) -> String {
    uri_to_path(&problem.uri)
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| problem.uri.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use lsp_types::{DiagnosticSeverity, Position, Range};
    use lume_lsp::{Problem, ProblemId, ServerId};
    use lume_ui::tool::{Tool, ToolEvent, ToolRequest};

    use super::{Event, ProblemsTool, replace_event};

    fn problem(uri: &str) -> Problem {
        Problem {
            id: ProblemId(7),
            server_id: ServerId::new("fixture"),
            uri: uri.parse().expect("problem URI"),
            range: Range::new(Position::new(4, 2), Position::new(4, 5)),
            severity: Some(DiagnosticSeverity::ERROR),
            code: None,
            source: None,
            message: "real diagnostic".into(),
            related: Vec::new(),
        }
    }

    #[test]
    fn file_problem_opens_the_real_path_and_one_based_line() {
        let mut tool = ProblemsTool::new();
        tool.update(&replace_event(vec![problem(
            "file:///C:/workspace/main.rs",
        )]));
        let requests = tool.update(&ToolEvent::new(Event::Open(ProblemId(7))));
        assert!(matches!(
            requests.as_slice(),
            [ToolRequest::OpenFileAt { path, line: 5 }]
                if path.ends_with("workspace/main.rs") || path.ends_with(r"workspace\main.rs")
        ));
    }

    #[test]
    fn non_file_and_malformed_uris_are_not_navigable() {
        let mut tool = ProblemsTool::new();
        tool.update(&replace_event(vec![problem("untitled:fixture")]));
        assert!(
            tool.update(&ToolEvent::new(Event::Open(ProblemId(7))))
                .is_empty()
        );
        // la codifica rotta la copre `lsp_uri`, che è dove vive la
        // conversione: qui basta che una URI non-file non apra niente
    }
}
