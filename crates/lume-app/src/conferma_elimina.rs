//! La conferma «Eliminare questo file?».
//!
//! Un riquadro solo, con il nome del file e due uscite: eliminare o
//! tornare indietro. Prima dell'avvio Esc e il clic fuori annullano; durante
//! lo spostamento nascondono il riquadro, mentre il worker prosegue.

use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use lume_theme::LumeTheme;
use lume_ui::styles;

/// Le due risposte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfermaMessage {
    Elimina,
    Annulla,
}

/// Quel che la conferma mostra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Riquadro {
    /// Il nome del file da eliminare.
    pub nome: String,
    /// Comprende il contenuto della cartella.
    pub cartella: bool,
    /// L'errore dell'ultimo tentativo: resta qui finché non si risponde,
    /// perché la notifica in barra di stato sparisce e questa no.
    pub errore: Option<String>,
    /// Lo spostamento è in corso: i tasti non agiscono più.
    pub in_corso: bool,
}

/// Il testo del riquadro: che cosa succede, senza gergo.
pub fn titolo(riquadro: &Riquadro) -> String {
    format!("Eliminare «{}»?", riquadro.nome)
}

/// Il riquadro, disegnato.
pub fn view<'a>(riquadro: &Riquadro, t: &LumeTheme) -> Element<'a, ConfermaMessage> {
    let mut corpo = column![text(titolo(riquadro)).size(13).color(t.text)].spacing(8);
    corpo = corpo.push(
        text(if riquadro.cartella {
            "La cartella e tutto il suo contenuto vanno nel Cestino: li puoi recuperare da lì."
        } else {
            "Il file va nel Cestino: lo puoi recuperare da lì."
        })
        .size(11)
        .color(t.sec),
    );
    if let Some(errore) = riquadro.errore.as_ref() {
        corpo = corpo.push(text(errore.clone()).size(11).color(t.bad));
    }

    // Mentre lo spostamento è in corso i tasti non agiscono: annullare ora
    // lascerebbe il file nel Cestino senza che nessuno lo sappia.
    let mut annulla = button(text("Annulla").size(12).color(t.sec))
        .padding(Padding::from([4, 10]))
        .style(styles::hover_row(t.sec, t.hover, None, 4.0));
    let mut elimina = button(text("Elimina").size(12).color(t.bad))
        .padding(Padding::from([4, 10]))
        .style(styles::hover_row(t.bad, t.hover, None, 4.0));
    if !riquadro.in_corso {
        annulla = annulla.on_press(ConfermaMessage::Annulla);
        elimina = elimina.on_press(ConfermaMessage::Elimina);
    } else {
        corpo = corpo.push(
            text("Spostamento nel Cestino in corso… Esc nasconde il riquadro, ma non annulla lo spostamento.")
                .size(11)
                .color(t.muted),
        );
    }

    let scheda = container(
        column![
            corpo,
            row![Space::new().width(Length::Fill), annulla, elimina,]
                .spacing(6)
                .align_y(Alignment::Center),
        ]
        .spacing(8),
    )
    .width(Length::Fixed(420.0))
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
    use super::*;

    #[test]
    fn il_titolo_dice_il_nome_del_file() {
        let riquadro = Riquadro {
            nome: "appunti.md".to_owned(),
            cartella: false,
            errore: None,
            in_corso: false,
        };
        assert_eq!(titolo(&riquadro), "Eliminare «appunti.md»?");
    }
}
