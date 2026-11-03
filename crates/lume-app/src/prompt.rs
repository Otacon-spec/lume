//! Un riquadro con una riga da scrivere: il guscio condiviso di «Rinomina
//! simbolo» e «Vai a riga».
//!
//! Due clienti, una forma sola. Non è economia di righe: sono due comandi che
//! chiedono la stessa cosa nello stesso momento del gesto, e se avessero due
//! aspetti diversi la differenza sembrerebbe voler dire qualcosa.
//!
//! ATTENZIONE: Il prototipo àncora il campo **sopra il simbolo**, e non si può fare da
//! qui: la geometria del caret vive nel `Tree` del widget dell'editor e non è
//! raggiungibile da `view()` (`sync` e `x_at_col_cached` in `lume-ui/src/editor/widget.rs`). Ancorarlo vuol
//! dire cambiare l'API del widget — è un passo di widget, non di LSP, ed è
//! rimandato con quel motivo.

use iced::widget::{Space, button, column, container, row, text, text_input};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use lume_theme::LumeTheme;
use lume_ui::styles;

/// L'id del campo, per l'autofocus. Uno solo: i due riquadri non sono mai
/// aperti insieme, perché l'overlay è uno.
pub const INPUT_ID: &str = "prompt-input";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptMessage {
    Changed(String),
    Confirm,
    Cancel,
}

/// Quel che cambia fra i due usi.
pub struct Prompt<'a> {
    pub title: String,
    pub placeholder: &'a str,
    pub value: &'a str,
    /// Riga sotto il campo: cosa succederà, o cosa non succederà.
    pub hint: String,
    /// Il tasto di conferma è premibile?
    pub ready: bool,
    pub confirm_label: &'a str,
}

pub fn view<'a>(prompt: &Prompt<'_>, t: &LumeTheme) -> Element<'a, PromptMessage> {
    let campo = text_input(prompt.placeholder, prompt.value)
        .id(INPUT_ID)
        .on_input(PromptMessage::Changed)
        // ATTENZIONE: `on_submit` non è comodità: fa **catturare** l'Invio al campo, e
        // il listener globale salta gli eventi catturati. Senza, l'Invio
        // finirebbe anche alla ricerca globale.
        .on_submit(PromptMessage::Confirm)
        .size(13)
        .padding(Padding::from([6, 8]))
        .style({
            let t = t.clone();
            move |_, _| text_input::Style {
                background: Background::Color(t.bg),
                border: Border {
                    color: t.border,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                icon: t.icon,
                placeholder: t.muted,
                value: t.text,
                selection: t.sel,
            }
        });

    let mut conferma = button(text(prompt.confirm_label.to_owned()).size(12).color(t.text))
        .padding(Padding::from([4, 10]))
        .style(styles::hover_row(t.text, t.hover, None, 4.0));
    if prompt.ready {
        conferma = conferma.on_press(PromptMessage::Confirm);
    }

    let scheda = container(
        column![
            text(prompt.title.clone()).size(12).color(t.sec),
            campo,
            // L'hint sta su riga propria: nella riga dei tasti un percorso
            // lungo spingeva «Annulla» e «Conferma» fuori dal riquadro, e il
            // tasto di conferma spariva con lui.
            text(prompt.hint.clone()).size(11).color(t.muted),
            row![
                Space::new().width(Length::Fill),
                button(text("Annulla").size(12).color(t.sec))
                    .padding(Padding::from([4, 10]))
                    .style(styles::hover_row(t.sec, t.hover, None, 4.0))
                    .on_press(PromptMessage::Cancel),
                conferma,
            ]
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

/// «Vai a riga»: legge il numero che l'utente ha scritto.
///
/// ATTENZIONE: Le righe si contano **da 1** per chi le legge e da 0 per il testo, e la
/// conversione va fatta in un posto solo. `0` e il vuoto non sono righe: il
/// riquadro resta com'è invece di saltare all'inizio del file, che sarebbe
/// una risposta a una domanda che non è stata fatta.
pub fn riga_richiesta(testo: &str) -> Option<usize> {
    let numero: usize = testo.trim().parse().ok()?;
    numero.checked_sub(1)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::riga_richiesta;

    /// Da 1 per la gente, da 0 per il testo — e quel che non è un numero non
    /// diventa la riga zero per ripiego.
    #[test]
    fn vai_a_riga_conta_da_uno_e_rifiuta_quel_che_non_e_una_riga() {
        assert_eq!(riga_richiesta("1"), Some(0));
        assert_eq!(riga_richiesta("  42 "), Some(41));
        assert_eq!(riga_richiesta("0"), None, "la riga zero non esiste");
        assert_eq!(riga_richiesta(""), None);
        assert_eq!(riga_richiesta("dodici"), None);
        assert_eq!(riga_richiesta("-3"), None);
        assert_eq!(riga_richiesta("2.5"), None);
    }
}
