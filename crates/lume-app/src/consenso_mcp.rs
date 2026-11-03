//! Conferma l'avvio esplicito del server MCP nominando la radice del progetto.
//! La modalità a tre stati governa le conferme per i tool MCP.

use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use lume_theme::LumeTheme;
use lume_ui::styles;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsensoMessage {
    Avvia,
    Annulla,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Riquadro {
    pub radice: String,
}

/// Quel che un agente può fare; la modalità decide quando chiede conferma.
pub const POTERI: [&str; 7] = [
    "leggere i file del progetto, anche quelli aperti con modifiche non salvate",
    "cercare nel testo e leggere le diagnostiche del language server",
    "interrogare il language server: hover, definizioni, usi e simboli di un file",
    "leggere stato, diff, log e blame di Git",
    "aprire file nell'editor",
    "scrivere e sostituire testo nei file del progetto e rinominare simboli",
    "eseguire comandi sul tuo computer",
];

/// Modalità di partenza e limiti, mostrati prima di accendere il server.
pub const AVVISO: &str = "Si parte in Non brave: ogni scrittura e ogni comando chiede la tua conferma \
in una card. Cambi modalità dalla barra di stato: Auto lascia passare le modifiche dentro la cartella \
del progetto, Brave non chiede più niente. Scrivere sotto .git o fuori dalla cartella è sempre vietato; \
ma un comando è un programma, e in Brave può toccare qualunque file. Il server si ferma con Ferma, \
cambiando cartella o chiudendo Lume.";

pub fn titolo(riquadro: &Riquadro) -> String {
    format!("Avviare il server MCP per {}?", riquadro.radice)
}

pub fn view<'a>(riquadro: &Riquadro, t: &LumeTheme) -> Element<'a, ConsensoMessage> {
    let mut poteri = column![text("Un agente collegato potrà:").size(12).color(t.sec)].spacing(2);
    for potere in POTERI {
        poteri = poteri.push(text(format!("• {potere}")).size(12).color(t.sec));
    }
    let corpo = column![
        text(titolo(riquadro)).size(13).color(t.text),
        poteri,
        text(AVVISO).size(11).color(t.muted),
        row![
            Space::new().width(Length::Fill),
            button(text("Annulla").size(12).color(t.sec))
                .padding(Padding::from([4, 10]))
                .style(styles::hover_row(t.sec, t.hover, None, 4.0))
                .on_press(ConsensoMessage::Annulla),
            button(text("Avvia").size(12).color(t.accent_on))
                .padding(Padding::from([4, 10]))
                .style(styles::hover_row(
                    t.accent_on,
                    t.accent,
                    Some(t.accent),
                    4.0
                ))
                .on_press(ConsensoMessage::Avvia),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    ]
    .spacing(8);
    let scheda = container(corpo)
        .width(Length::Fixed(440.0))
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
    container(scheda)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .padding(Padding {
            top: 120.0,
            ..Padding::ZERO
        })
        .style(move |_| container::Style {
            background: Some(Background::Color(Color {
                a: 0.35,
                ..Color::BLACK
            })),
            ..container::Style::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn titolo_nomina_la_radice_e_i_poteri_dicono_che_si_scrive_e_si_esegue() {
        let riquadro = Riquadro {
            radice: r"C:\Progetti con spazi\è".to_owned(),
        };
        assert_eq!(
            titolo(&riquadro),
            r"Avviare il server MCP per C:\Progetti con spazi\è?"
        );
        assert!(POTERI.iter().any(|potere| potere.contains("scrivere")));
        assert!(
            POTERI
                .iter()
                .any(|potere| potere.contains("eseguire comandi"))
        );
        assert!(
            POTERI
                .iter()
                .any(|potere| potere.contains("language server"))
        );
    }

    #[test]
    fn avviso_descrive_modalita_iniziale_confini_e_comandi() {
        assert!(AVVISO.contains("Non brave"));
        assert!(AVVISO.contains(".git"));
        assert!(AVVISO.contains("Brave"));
        assert!(AVVISO.contains("un comando"));
        assert!(!AVVISO.contains("Nessuna scrittura"));
        assert!(POTERI.iter().any(|potere| potere.contains("scrivere")));
        assert!(
            POTERI
                .iter()
                .any(|potere| potere.contains("eseguire comandi"))
        );
        assert!(
            POTERI
                .iter()
                .any(|potere| potere.contains("language server"))
        );
    }
}
