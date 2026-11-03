//! Visualizzatore di diff nell'area centrale.
//!
//! Intestazione `file · base · +N · −M · Unificato · ×` e righe con **due
//! grondaie** (numero riga vecchio, numero riga nuovo), colonna segno e
//! testo, su fondo verde o rosso, come nel prototipo.
//!
//! **Affiancato non si disegna**: sarebbe un secondo motore di
//! visualizzazione, non un'opzione di questo — stessa regola di Accantona.

use std::path::Path;

use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Alignment, Background, Color, Element, Length, Padding};
use lume_git::{DiffBase, DiffHunk, DiffRow, DiffSign, FileDiff};
use lume_theme::LumeTheme;
use lume_ui::MONO;
use lume_ui::styles;

use crate::app::Message;

/// Larghezza delle due grondaie, come nel prototipo (36px l'una).
const GUTTER: f32 = 36.0;

/// Pill dell'interruttore fra le due basi.
///
/// Quella attiva riusa lo stile della pill «Unificato» accanto; quella spenta è
/// un bottone piatto. Nessun token nuovo.
fn base_pill<'a>(
    etichetta: &'a str,
    base: DiffBase,
    attiva: bool,
    t: &'a LumeTheme,
) -> Element<'a, Message> {
    if attiva {
        container(text(etichetta).size(11).color(t.accent_on))
            .padding(Padding::from([2, 8]))
            .style(move |_| container::Style {
                background: Some(Background::Color(t.sel)),
                border: iced::Border {
                    radius: 5.0.into(),
                    width: 1.0,
                    color: t.border2,
                },
                ..container::Style::default()
            })
            .into()
    } else {
        button(text(etichetta).size(11).color(t.muted))
            .padding(Padding::from([2, 8]))
            .style(styles::hover_row(t.muted, t.hover, None, 5.0))
            .on_press(Message::SwitchDiffBase(base))
            .into()
    }
}

/// Le due pill affiancate, o niente su un diff di commit.
fn base_switch<'a>(
    corrente: &DiffBase,
    commutabile: bool,
    t: &'a LumeTheme,
) -> Element<'a, Message> {
    if !commutabile {
        return Space::new().width(0).into();
    }
    row![
        base_pill(
            "Locali",
            DiffBase::Worktree,
            *corrente == DiffBase::Worktree,
            t
        ),
        base_pill("In stage", DiffBase::Index, *corrente == DiffBase::Index, t),
    ]
    .spacing(2)
    .align_y(Alignment::Center)
    .into()
}

pub fn view<'a>(diff: &'a FileDiff, t: &'a LumeTheme) -> Element<'a, Message> {
    let name = diff.path.to_string_lossy().replace('\\', "/");
    // Su worktree e index l'interruttore dice già in quale vista siamo, quindi
    // l'etichetta testuale sparirebbe ripetendo la pill accesa. Su un commit
    // non c'è niente da commutare e resta l'etichetta con l'hash.
    let commutabile = matches!(diff.base, DiffBase::Worktree | DiffBase::Index);
    let header = row![
        text(name).size(12).color(t.text),
        text(if commutabile {
            String::new()
        } else {
            format!("· {}", diff.base.label())
        })
        .size(12)
        .color(t.muted),
        text(format!("+{}", diff.added))
            .size(11)
            .font(MONO)
            .color(t.good),
        text(format!("−{}", diff.removed))
            .size(11)
            .font(MONO)
            .color(t.bad),
        Space::new().width(Length::Fill),
        base_switch(&diff.base, commutabile, t),
        container(text("Unificato").size(11).color(t.accent_on))
            .padding(Padding::from([2, 8]))
            .style(move |_| container::Style {
                background: Some(Background::Color(t.sel)),
                border: iced::Border {
                    radius: 5.0.into(),
                    width: 1.0,
                    color: t.border2,
                },
                ..container::Style::default()
            }),
        button(text("×").size(14).color(t.muted))
            .padding(Padding::from([0, 6]))
            .style(styles::hover_row(t.muted, t.hover, None, 4.0))
            .on_press(Message::CloseDiff),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let mut body = column![].spacing(0);
    if diff.binary {
        body = body.push(
            container(
                text("File binario: nessuna differenza da mostrare.")
                    .size(12)
                    .color(t.muted),
            )
            .padding(12),
        );
    } else if diff.rows.is_empty() {
        body =
            body.push(container(text("Nessuna differenza.").size(12).color(t.muted)).padding(12));
    }
    // Ogni base porta la sua azione: «Locali» mette in stage, «In stage»
    // toglie, un commit non fa niente.
    let hunk_ctx: Option<(&Path, &[DiffHunk], HunkAction)> = match &diff.base {
        DiffBase::Worktree => Some((&diff.path, &diff.hunks, HunkAction::Stage)),
        DiffBase::Index => Some((&diff.path, &diff.hunks, HunkAction::Unstage)),
        DiffBase::Commit(_) => None,
    };
    for row_data in &diff.rows {
        body = body.push(diff_row(row_data, t, hunk_ctx));
    }
    if diff.truncated {
        body = body.push(
            container(
                text("Diff troppo lungo: righe successive non mostrate.")
                    .size(11)
                    .color(t.warn),
            )
            .padding(Padding::from([6, 14])),
        );
    }

    column![
        container(header)
            .padding(Padding {
                top: 0.0,
                right: 14.0,
                bottom: 8.0,
                left: 14.0
            })
            .width(Length::Fill),
        scrollable(body).height(Length::Fill).width(Length::Fill),
    ]
    .spacing(6)
    .padding(Padding::from([10, 0]))
    .into()
}

/// Cosa fa il pulsante per hunk, che dipende dalla base del diff.
///
/// Le due basi partizionano il cambiamento: un hunk sta in «Locali» **o** in
/// «In stage», mai in tutte e due. Da qui l'azione è univoca, e il pulsante
/// applica sempre esattamente quel che la riga mostra.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HunkAction {
    Stage,
    Unstage,
}

/// Disegna una singola riga del diff.
///
/// `hunk_ctx` è `Some((path, hunks, azione))` per i diff del worktree e
/// dell'index: in quel caso le righe Added/Removed portano il pulsante che
/// mette in stage o toglie dallo stage l'hunk. Su un diff di commit è `None` —
/// non c'è niente da mettere in stage in un commit già fatto.
fn diff_row<'a>(
    row_data: &'a DiffRow,
    t: &'a LumeTheme,
    hunk_ctx: Option<(&'a Path, &'a [DiffHunk], HunkAction)>,
) -> Element<'a, Message> {
    match row_data {
        DiffRow::Gap { skipped } => {
            let label = if *skipped == 0 {
                "⋯".to_owned()
            } else {
                format!("⋯ {skipped} righe")
            };
            container(text(label).size(11).font(MONO).color(t.faint))
                .padding(Padding {
                    top: 1.0,
                    right: 0.0,
                    bottom: 1.0,
                    left: GUTTER * 2.0,
                })
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(Background::Color(t.inset)),
                    ..container::Style::default()
                })
                .into()
        }
        DiffRow::Line {
            old_line,
            new_line,
            sign,
            text: line,
            hunk_idx,
        } => {
            let (background, sign_color, text_color, mark) = match sign {
                DiffSign::Added => (Some(t.good_bg), t.good, t.text, "+"),
                // Il rosso di fondo del prototipo è `rgba(212,112,105,.14)`,
                // cioè il colore «bad» molto diluito.
                DiffSign::Removed => (Some(faded(t.bad)), t.bad, t.text, "-"),
                DiffSign::Context => (None, t.faint, t.tree, ""),
            };

            // Il pulsante per hunk sta sulle righe che appartengono a un hunk
            // (Added o Removed), e solo dove l'azione ha senso.
            let stage_btn: Option<Element<'a, Message>> = match (sign, hunk_ctx, hunk_idx) {
                (DiffSign::Added | DiffSign::Removed, Some((path, hunks, action)), Some(idx)) => {
                    hunks.get(*idx).map(|hunk| {
                        let path = path.to_path_buf();
                        let hunk = hunk.clone();
                        let (glifo, tinta, message) = match action {
                            HunkAction::Stage => {
                                ("≡+", t.accent_on, Message::StageHunk { path, hunk })
                            }
                            HunkAction::Unstage => {
                                ("≡−", t.warn, Message::UnstageHunk { path, hunk })
                            }
                        };
                        button(text(glifo).size(10).font(MONO).color(tinta))
                            .padding(Padding::from([1, 5]))
                            .style(styles::hover_row(tinta, t.hover, None, 3.0))
                            .on_press(message)
                            .into()
                    })
                }
                _ => None,
            };

            let mut line_row = row![
                gutter(*old_line, t),
                gutter(*new_line, t),
                text(mark).size(12.5).font(MONO).color(sign_color).width(14),
                text(line)
                    .size(12.5)
                    .font(MONO)
                    .color(text_color)
                    .wrapping(text::Wrapping::None),
            ]
            .align_y(Alignment::Center);

            if let Some(btn) = stage_btn {
                line_row = line_row.push(Space::new().width(Length::Fill)).push(btn);
            }

            let mut cell = container(line_row).width(Length::Fill);
            if let Some(background) = background {
                cell = cell.style(move |_| container::Style {
                    background: Some(Background::Color(background)),
                    ..container::Style::default()
                });
            }
            cell.into()
        }
    }
}

fn gutter<'a>(number: Option<u32>, t: &'a LumeTheme) -> Element<'a, Message> {
    let label = number.map(|n| n.to_string()).unwrap_or_default();
    container(text(label).size(11).font(MONO).color(t.faint))
        .width(GUTTER)
        .padding(Padding {
            top: 0.0,
            right: 8.0,
            bottom: 0.0,
            left: 0.0,
        })
        .align_x(Alignment::End)
        .into()
}

/// Il fondo delle righe rimosse: «bad» al 14% di opacità, come nel prototipo.
fn faded(color: Color) -> Color {
    Color { a: 0.14, ..color }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::faded;

    #[test]
    fn il_fondo_delle_rimozioni_e_il_rosso_diluito_del_prototipo() {
        let bad = iced::Color::from_rgb8(212, 112, 105);
        let background = faded(bad);
        assert!((background.a - 0.14).abs() < f32::EPSILON);
        assert_eq!(
            (background.r, background.g, background.b),
            (bad.r, bad.g, bad.b)
        );
    }
}
