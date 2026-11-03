//! Il riquadro Forge.

use crate::forge::{Family, Kind, RunConfig};
use iced::widget::{Space, button, column, container, mouse_area, row, text, text_input};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use lume_theme::LumeTheme;
use lume_ui::{MONO, styles};

pub const TARGET_ID: &str = "forge-bersaglio";
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeMessage {
    Family(Family),
    Kind(Kind),
    Target(String),
    Args(String),
    Release(bool),
    Features(String),
    Run,
    Close,
}

pub fn edit(config: &mut RunConfig, message: ForgeMessage) {
    match message {
        ForgeMessage::Family(family) if family != config.family => {
            *config = RunConfig::blank(family)
        }
        ForgeMessage::Kind(kind)
            if config.family.kinds().iter().any(|(k, _, _)| *k == kind) && kind != config.kind =>
        {
            config.kind = kind;
            config.target.clear();
        }
        ForgeMessage::Target(v) => config.target = v,
        ForgeMessage::Args(v) => config.args = v,
        ForgeMessage::Release(v) => config.release = v,
        ForgeMessage::Features(v) => config.features = v,
        ForgeMessage::Family(_)
        | ForgeMessage::Kind(_)
        | ForgeMessage::Run
        | ForgeMessage::Close => {}
    }
}
fn chip<'a>(
    t: &LumeTheme,
    label: &str,
    active: bool,
    message: ForgeMessage,
) -> Element<'a, ForgeMessage> {
    let color = if active { t.accent } else { t.sec };
    button(text(label.to_owned()).size(11.5).color(color))
        .padding(Padding::from([3, 8]))
        .style(styles::hover_row(
            color,
            t.hover2,
            active.then_some(t.sel),
            4.0,
        ))
        .on_press(message)
        .into()
}
pub fn view<'a>(config: &RunConfig, can_run: bool, t: &LumeTheme) -> Element<'a, ForgeMessage> {
    let problem = config.problem();
    let runnable = problem.is_none() && can_run;
    let mut families = column![].spacing(4);
    for group in Family::ALL.chunks(4) {
        let mut line = row![].spacing(4);
        for family in group {
            line = line.push(chip(
                t,
                family.name(),
                *family == config.family,
                ForgeMessage::Family(*family),
            ));
        }
        families = families.push(line);
    }
    let mut kinds = row![].spacing(4);
    for (kind, label, _) in config.family.kinds() {
        kinds = kinds.push(chip(
            t,
            label,
            *kind == config.kind,
            ForgeMessage::Kind(*kind),
        ));
    }
    let placeholder = config
        .family
        .kinds()
        .iter()
        .find(|(k, _, _)| *k == config.kind)
        .map_or("", |(_, _, p)| *p);
    let target = text_input(placeholder, &config.target)
        .id(TARGET_ID)
        .on_input(ForgeMessage::Target)
        .on_submit(ForgeMessage::Run)
        .size(13)
        .padding(Padding::from([6, 8]))
        .style(styles::input(t));
    let args = text_input("argomenti (facoltativi)", &config.args)
        .on_input(ForgeMessage::Args)
        .on_submit(ForgeMessage::Run)
        .size(13)
        .padding(Padding::from([6, 8]))
        .style(styles::input(t));
    let mut fields = column![
        text(config.family.target_label(config.kind))
            .size(11)
            .color(t.muted),
        target,
        text("Argomenti").size(11).color(t.muted),
        args
    ]
    .spacing(4);
    if config.family == Family::Rust {
        let features = text_input("feature di Cargo, separate da virgola", &config.features)
            .on_input(ForgeMessage::Features)
            .on_submit(ForgeMessage::Run)
            .size(13)
            .padding(Padding::from([6, 8]))
            .style(styles::input(t));
        fields = fields.push(
            row![
                chip(
                    t,
                    "release",
                    config.release,
                    ForgeMessage::Release(!config.release)
                ),
                features
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
    }
    let preview = container(text(config.command()).size(12).font(MONO).color(t.text))
        .width(Length::Fill)
        .padding(Padding::from([6, 8]))
        .style({
            let t = t.clone();
            move |_| container::Style {
                background: Some(Background::Color(t.bg)),
                border: Border {
                    color: t.border,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..container::Style::default()
            }
        });
    let note = if !can_run {
        "Il terminale è spento: riaccendilo dalle Impostazioni per eseguire.".to_owned()
    } else if let Some(p) = problem {
        p
    } else {
        "Va nel terminale attivo, nella cartella in cui si trova.".to_owned()
    };
    let mut run = button(text("Esegui").size(12).color(t.text))
        .padding(Padding::from([4, 10]))
        .style(styles::hover_row(t.text, t.hover, None, 4.0));
    if runnable {
        run = run.on_press(ForgeMessage::Run);
    }
    let card = container(
        column![
            text("Forge · configurazione di esecuzione")
                .size(12)
                .color(t.sec),
            families,
            text(config.family.description()).size(11).color(t.muted),
            kinds,
            fields,
            preview,
            row![
                text(note).size(11).color(t.muted),
                Space::new().width(Length::Fill),
                button(text("Annulla").size(12).color(t.sec))
                    .padding(Padding::from([4, 10]))
                    .style(styles::hover_row(t.sec, t.hover, None, 4.0))
                    .on_press(ForgeMessage::Close),
                run
            ]
            .spacing(6)
            .align_y(Alignment::Center)
        ]
        .spacing(8),
    )
    .width(Length::Fixed(520.0))
    .padding(Padding::from([12, 14]))
    .style({
        let t = t.clone();
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
    mouse_area(
        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(|_| container::Style {
                background: Some(Background::Color(Color {
                    a: 0.35,
                    ..Color::BLACK
                })),
                ..container::Style::default()
            }),
    )
    .on_press(ForgeMessage::Close)
    .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{ForgeMessage, edit, view};
    use crate::forge::{Family, Kind, RunConfig};

    fn scrivi(config: &mut RunConfig, messaggi: Vec<ForgeMessage>) {
        for messaggio in messaggi {
            edit(config, messaggio);
        }
    }

    #[test]
    fn cambiare_famiglia_riparte_da_una_configurazione_vuota() {
        let mut c = RunConfig::blank(Family::Rust);
        scrivi(
            &mut c,
            vec![
                ForgeMessage::Target("lume".into()),
                ForgeMessage::Release(true),
                ForgeMessage::Features("sql".into()),
                ForgeMessage::Family(Family::Node),
            ],
        );
        assert_eq!(c, RunConfig::blank(Family::Node));
    }
    #[test]
    fn riscegliere_la_stessa_famiglia_non_cancella_quel_che_hai_scritto() {
        let mut c = RunConfig::blank(Family::Rust);
        scrivi(
            &mut c,
            vec![
                ForgeMessage::Target("lume".into()),
                ForgeMessage::Family(Family::Rust),
            ],
        );
        assert_eq!(c.target, "lume");
    }
    #[test]
    fn cambiare_tipo_svuota_il_bersaglio_e_rifiuta_un_tipo_di_un_altra_famiglia() {
        let mut c = RunConfig::blank(Family::Rust);
        scrivi(
            &mut c,
            vec![
                ForgeMessage::Target("lume".into()),
                ForgeMessage::Kind(Kind::Test),
            ],
        );
        assert_eq!(c.kind, Kind::Test);
        assert_eq!(c.target, "");
        scrivi(
            &mut c,
            vec![
                ForgeMessage::Target("remote::".into()),
                ForgeMessage::Kind(Kind::Pytest),
            ],
        );
        assert_eq!(c.kind, Kind::Test);
        assert_eq!(c.target, "remote::");
    }
    #[test]
    fn scrivere_nei_campi_compone_il_comando_dell_anteprima() {
        let mut c = RunConfig::blank(Family::Rust);
        scrivi(
            &mut c,
            vec![
                ForgeMessage::Target("lume".into()),
                ForgeMessage::Args("--port 8080".into()),
                ForgeMessage::Release(true),
                ForgeMessage::Features("sql".into()),
            ],
        );
        assert_eq!(
            c.command(),
            "cargo run --release --features sql --bin lume -- --port 8080"
        );
        assert!(c.is_runnable());
    }
    #[test]
    fn run_e_close_non_toccano_la_configurazione() {
        let mut c = RunConfig::blank(Family::Go);
        c.target = "./cmd/server".into();
        let prima = c.clone();
        scrivi(&mut c, vec![ForgeMessage::Run, ForgeMessage::Close]);
        assert_eq!(c, prima);
    }
    #[test]
    fn il_riquadro_si_costruisce_per_ogni_famiglia() {
        let tema = lume_theme::LumeTheme::default();
        for family in Family::ALL {
            let vuota = RunConfig::blank(family);
            let _ = view(&vuota, true, &tema);
            let _ = view(&vuota, false, &tema);
        }
    }
}
