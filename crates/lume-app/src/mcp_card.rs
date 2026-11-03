use iced::widget::{button, column, container, row, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use lume_mcp::{AppQuery, confine::to_slash};
use lume_theme::LumeTheme;

use crate::mcp_gate::Gate;

pub const PREVIEW_LINES: usize = 6;
pub const CLIP_CHARS: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardMessage {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardView {
    pub position: (usize, usize),
    pub client: String,
    pub title: String,
    pub lines: Vec<String>,
}

impl CardView {
    pub fn of(gate: &Gate) -> Option<Self> {
        let waiting = gate.front()?;
        Some(Self {
            position: gate.position()?,
            client: waiting.client.clone(),
            title: waiting.title.clone(),
            lines: waiting.lines.clone(),
        })
    }
}

pub fn clip(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let clipped: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{clipped}…")
    } else {
        clipped
    }
}

pub fn describe(query: &AppQuery) -> (String, Vec<String>) {
    match query {
        AppQuery::WriteFile { relative, content } => {
            let lines: Vec<&str> = content.lines().collect();
            let mut preview = vec![format!(
                "{} caratteri, {} righe",
                content.chars().count(),
                lines.len()
            )];
            preview.extend(
                lines
                    .iter()
                    .take(PREVIEW_LINES)
                    .map(|line| clip(line, CLIP_CHARS)),
            );
            if lines.len() > PREVIEW_LINES {
                preview.push("… altre righe".to_owned());
            }
            (format!("vuole scrivere {}", to_slash(relative)), preview)
        }
        AppQuery::ReplaceText {
            relative,
            old_text,
            new_text,
            all,
        } => {
            let mut lines = vec![
                format!("Cerca: «{}»", clip(old_text, CLIP_CHARS)),
                format!("Sostituisce con: «{}»", clip(new_text, CLIP_CHARS)),
            ];
            if *all {
                lines.push("Tutte le occorrenze".to_owned());
            }
            (format!("vuole modificare {}", to_slash(relative)), lines)
        }
        AppQuery::RunCommand {
            program,
            args,
            cwd,
            timeout_secs,
        } => {
            let mut command = vec![display_arg(program)];
            command.extend(args.iter().map(|arg| display_arg(arg)));
            let directory = if cwd.as_os_str().is_empty() {
                "la radice del progetto".to_owned()
            } else {
                to_slash(cwd)
            };
            ("vuole eseguire un comando".to_owned(), vec![clip(&command.join(" "), CLIP_CHARS), format!("Cartella: {directory}"), format!("Limite di tempo: {timeout_secs} s"), "Il comando può modificare file ovunque sul computer, anche fuori dal progetto.".to_owned()])
        }
        AppQuery::Rename {
            relative,
            line,
            column,
            new_name,
        } => (
            format!("vuole rinominare in {}", to_slash(relative)),
            vec![
                format!("Posizione: riga {line}, colonna {column}"),
                format!("Nuovo nome: {}", clip(new_name, CLIP_CHARS)),
            ],
        ),
        AppQuery::OpenInEditor { relative, line } => (
            format!("vuole aprire {}", to_slash(relative)),
            line.map(|line| vec![format!("Riga: {line}")])
                .unwrap_or_default(),
        ),
        AppQuery::Buffer { relative } => (format!("vuole leggere {}", to_slash(relative)), vec![]),
        AppQuery::Diagnostics { relative, .. } => (
            "vuole leggere le diagnostiche".to_owned(),
            relative
                .as_deref()
                .map(|path| vec![to_slash(path)])
                .unwrap_or_default(),
        ),
        AppQuery::Hover {
            relative,
            line,
            column,
        } => (
            format!("vuole leggere l'hover di {}", to_slash(relative)),
            vec![format!("Riga {line}, colonna {column}")],
        ),
        AppQuery::Definition {
            relative,
            line,
            column,
        } => (
            format!("vuole trovare la definizione in {}", to_slash(relative)),
            vec![format!("Riga {line}, colonna {column}")],
        ),
        AppQuery::References {
            relative,
            line,
            column,
        } => (
            format!("vuole trovare gli utilizzi in {}", to_slash(relative)),
            vec![format!("Riga {line}, colonna {column}")],
        ),
        AppQuery::DocumentSymbols { relative } => (
            format!("vuole leggere i simboli di {}", to_slash(relative)),
            vec![],
        ),
        AppQuery::OpenFiles => ("vuole leggere i file aperti".to_owned(), vec![]),
        AppQuery::ActiveFile => ("vuole leggere il file attivo".to_owned(), vec![]),
    }
}

const RENAME_FILES_SHOWN: usize = 8;

fn plurale(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

pub fn describe_rename(new_name: &str, files: &[(String, usize)]) -> (String, Vec<String>) {
    let total: usize = files.iter().map(|(_, occurrences)| *occurrences).sum();
    let title = format!(
        "vuole rinominare in «{}»: {} in {}",
        clip(new_name, 60),
        plurale(total, "occorrenza", "occorrenze"),
        plurale(files.len(), "file", "file"),
    );
    let mut lines: Vec<String> = files
        .iter()
        .take(RENAME_FILES_SHOWN)
        .map(|(path, occurrences)| {
            format!(
                "{}: {}",
                clip(path, CLIP_CHARS),
                plurale(*occurrences, "occorrenza", "occorrenze")
            )
        })
        .collect();
    if files.len() > RENAME_FILES_SHOWN {
        lines.push(format!(
            "… e altri {} file",
            files.len() - RENAME_FILES_SHOWN
        ));
    }
    lines.push(
        "Le modifiche entrano nei buffer dell'editor: niente viene salvato su disco.".to_owned(),
    );
    (title, lines)
}

fn display_arg(arg: &str) -> String {
    if arg.chars().any(char::is_whitespace) {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_owned()
    }
}

pub fn view<'a>(card: &CardView, theme: &LumeTheme) -> Element<'a, CardMessage> {
    let mut details = column![
        text(format!("{} vuole {}", card.client, card.title))
            .size(14)
            .color(theme.text)
    ]
    .spacing(6);
    if card.position.1 > 1 {
        details = details.push(
            text(format!(
                "Richiesta {} di {}",
                card.position.0, card.position.1
            ))
            .size(11)
            .color(theme.muted),
        );
    }
    for line in &card.lines {
        details = details.push(text(line.clone()).size(12).color(theme.sec));
    }
    let actions = row![
        button(text("Nega").size(12).color(theme.sec))
            .padding(Padding::from([5, 12]))
            .on_press(CardMessage::Deny),
        button(text("Consenti").size(12).color(theme.accent_on))
            .padding(Padding::from([5, 12]))
            .on_press(CardMessage::Allow),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let panel = container(
        column![
            details,
            row![iced::widget::Space::new().width(Length::Fill), actions]
                .align_y(Alignment::Center)
        ]
        .spacing(12),
    )
    .width(Length::Fixed(520.0))
    .padding(Padding::from([14, 16]))
    .style({
        let theme = theme.clone();
        move |_| iced::widget::container::Style {
            background: Some(Background::Color(theme.surface)),
            border: Border {
                color: theme.border,
                width: 1.0,
                radius: 7.0.into(),
            },
            ..Default::default()
        }
    });
    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| iced::widget::container::Style {
            background: Some(Background::Color(Color {
                a: 0.42,
                ..Color::BLACK
            })),
            ..Default::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use lume_mcp::AppQuery;
    use std::path::PathBuf;

    #[test]
    fn clip_taglia_in_caratteri_e_non_spezza_un_accento() {
        assert_eq!(clip("ciao", 10), "ciao");
        assert_eq!(clip("perché", 5), "perch…");
        assert_eq!(clip("èèèèè", 3), "èèè…");
        assert_eq!(clip("", 3), "");
    }

    #[test]
    fn la_card_di_una_scrittura_mostra_percorso_dimensione_e_anteprima() {
        let query = AppQuery::WriteFile {
            relative: PathBuf::from("src").join("a.rs"),
            content: "riga uno\nriga due\n".to_owned(),
        };
        let (title, lines) = describe(&query);
        assert_eq!(title, "vuole scrivere src/a.rs");
        assert_eq!(lines[0], "18 caratteri, 2 righe");
        assert!(lines.contains(&"riga uno".to_owned()));
    }

    #[test]
    fn preview_for_large_content_is_bounded() {
        let query = AppQuery::WriteFile {
            relative: "large.txt".into(),
            content: format!("{}\n", "è".repeat(5000)).repeat(50),
        };
        let (_, lines) = describe(&query);
        assert!(lines.len() <= 1 + PREVIEW_LINES + 1);
        assert!(
            lines
                .iter()
                .all(|line| line.chars().count() <= CLIP_CHARS + 1)
        );
    }

    #[test]
    fn la_card_di_una_sostituzione_mostra_prima_e_dopo() {
        let query = AppQuery::ReplaceText {
            relative: "b.txt".into(),
            old_text: "vecchio".into(),
            new_text: "nuovo".into(),
            all: true,
        };
        let (title, lines) = describe(&query);
        assert_eq!(title, "vuole modificare b.txt");
        assert_eq!(
            lines,
            [
                "Cerca: «vecchio»",
                "Sostituisce con: «nuovo»",
                "Tutte le occorrenze"
            ]
        );
    }

    #[test]
    fn la_card_della_rinomina_elenca_file_e_limita_la_coda() {
        let one = describe_rename("main", &[("main.rs".to_owned(), 1)]);
        assert_eq!(one.0, "vuole rinominare in «main»: 1 occorrenza in 1 file");
        assert_eq!(one.1[0], "main.rs: 1 occorrenza");
        assert_eq!(
            one.1.last().unwrap(),
            "Le modifiche entrano nei buffer dell'editor: niente viene salvato su disco."
        );

        let many: Vec<(String, usize)> = (0..12).map(|n| (format!("f{n}.rs"), n + 1)).collect();
        let (title, lines) = describe_rename("principale", &many);
        assert_eq!(
            title,
            "vuole rinominare in «principale»: 78 occorrenze in 12 file"
        );
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0], "f0.rs: 1 occorrenza");
        assert_eq!(lines[1], "f1.rs: 2 occorrenze");
        assert_eq!(lines[8], "… e altri 4 file");
    }

    #[test]
    fn la_card_comando_avverte_che_la_radice_non_limita_il_comando() {
        let query = AppQuery::RunCommand {
            program: "cargo".into(),
            args: vec!["test".into(), "--all".into()],
            cwd: PathBuf::new(),
            timeout_secs: 60,
        };
        let (title, lines) = describe(&query);
        assert_eq!(title, "vuole eseguire un comando");
        assert_eq!(lines[0], "cargo test --all");
        assert_eq!(lines[1], "Cartella: la radice del progetto");
        assert_eq!(lines[2], "Limite di tempo: 60 s");
        assert!(lines[3].contains("ovunque"));
    }
}
