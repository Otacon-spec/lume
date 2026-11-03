//! Il riquadro «Salvare le modifiche?»: chiudere una scheda sporca, o la
//! finestra con file sporchi, non butta via il lavoro senza chiederlo.
//!
//! Una forma sola per i due casi. Cambiano il titolo, l'elenco e le parole
//! dei tasti; la domanda è la stessa — «che ne faccio di queste modifiche?» —
//! e due aspetti diversi farebbero pensare a due domande diverse.
//!
//! Tre risposte e nessuna di più: salvare, buttare, tornare indietro. L'Esc
//! e il clic fuori dal riquadro valgono «Annulla» (passano da `CloseAll`):
//! nel dubbio non si perde niente.

use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use lume_theme::LumeTheme;
use lume_ui::styles;

/// Quanti nomi si elencano prima di dire «e altri N».
pub const MAX_NOMI: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfermaMessage {
    Salva,
    NonSalvare,
    Annulla,
}

/// Quel che cambia fra la chiusura di una scheda e quella della finestra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Riquadro {
    pub titolo: String,
    /// I file coinvolti, già nell'ordine delle schede.
    pub nomi: Vec<String>,
    /// L'ultimo salvataggio fallito: resta qui finché non si risponde, perché
    /// la notifica in barra di stato sparisce da sola e questa no.
    pub errore: Option<String>,
    pub salvataggio_in_corso: bool,
    pub etichetta_salva: &'static str,
    pub etichetta_scarta: &'static str,
}

/// I nomi da mostrare e quanti ne restano fuori.
pub fn nomi_visibili(nomi: &[String]) -> (impl Iterator<Item = &String>, usize) {
    (
        nomi.iter().take(MAX_NOMI),
        nomi.len().saturating_sub(MAX_NOMI),
    )
}

pub fn view<'a>(riquadro: &Riquadro, t: &LumeTheme) -> Element<'a, ConfermaMessage> {
    let mut corpo = column![text(riquadro.titolo.clone()).size(13).color(t.text)].spacing(8);

    // Una scheda sola ha il nome già nel titolo: ripeterlo in elenco sarebbe
    // rumore. Con la finestra l'elenco è la risposta a «quali?».
    if riquadro.nomi.len() > 1 {
        let (visibili, altri) = nomi_visibili(&riquadro.nomi);
        let mut elenco = column![].spacing(2);
        for nome in visibili {
            elenco = elenco.push(text(format!("• {nome}")).size(12).color(t.sec));
        }
        if altri > 0 {
            elenco = elenco.push(text(format!("… e altri {altri}")).size(12).color(t.muted));
        }
        corpo = corpo.push(elenco);
    }

    if riquadro.salvataggio_in_corso {
        corpo = corpo.push(text("Operazione sui file in corso. La chiusura attende l'esito; un'operazione già avviata può avere effetti che non si annullano.").size(12).color(t.sec));
    }

    if let Some(errore) = &riquadro.errore {
        corpo = corpo.push(text(errore.clone()).size(12).color(t.bad));
    }

    corpo = corpo.push(
        row![
            Space::new().width(Length::Fill),
            button(text("Annulla").size(12).color(t.sec))
                .padding(Padding::from([4, 10]))
                .style(styles::hover_row(t.sec, t.hover, None, 4.0))
                .on_press(ConfermaMessage::Annulla),
            button(
                text(riquadro.etichetta_scarta.to_owned())
                    .size(12)
                    .color(t.text)
            )
            .padding(Padding::from([4, 10]))
            .style(styles::hover_row(t.text, t.hover, None, 4.0))
            .on_press(ConfermaMessage::NonSalvare),
            button(
                text(riquadro.etichetta_salva.to_owned())
                    .size(12)
                    .color(t.accent_on)
            )
            .padding(Padding::from([4, 10]))
            .style(styles::hover_row(
                t.accent_on,
                t.accent,
                Some(t.accent),
                4.0
            ))
            .on_press(ConfermaMessage::Salva),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    );

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
    use super::{MAX_NOMI, nomi_visibili};

    /// L'elenco si tronca senza tagliare fuori il conto di quel che manca.
    #[test]
    fn elenco_lungo_si_tronca_e_dice_quanti_mancano() {
        let pochi: Vec<String> = (0..3).map(|i| format!("f{i}.rs")).collect();
        let (visibili, altri) = nomi_visibili(&pochi);
        assert_eq!(visibili.count(), 3);
        assert_eq!(altri, 0);

        let tanti: Vec<String> = (0..MAX_NOMI + 5).map(|i| format!("f{i}.rs")).collect();
        let (visibili, altri) = nomi_visibili(&tanti);
        assert_eq!(visibili.count(), MAX_NOMI);
        assert_eq!(altri, 5);
    }
}
