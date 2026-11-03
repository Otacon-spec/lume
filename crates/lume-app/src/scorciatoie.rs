//! Elenco delle scorciatoie da tastiera (Aiuto → Scorciatoie…).
//!
//! Le righe dei menù derivano da `MENUS`; solo le poche scorciatoie che non
//! hanno una voce di menù sono elencate separatamente.

use std::collections::BTreeSet;

use iced::widget::{column, container, row, scrollable, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use lume_theme::LumeTheme;
use lume_ui::MONO;

use crate::menu::{Item, MENUS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Riga {
    pub azione: &'static str,
    pub tasti: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sezione {
    pub titolo: &'static str,
    pub righe: Vec<Riga>,
}

/// Scorciatoia che nessuna voce di menù porta. `funzione` nasconde la riga
/// quando la funzionalità corrispondente è spenta.
pub struct Extra {
    pub azione: &'static str,
    pub tasti: &'static str,
    pub funzione: Option<&'static str>,
}

pub const FUORI_DAI_MENU: [Extra; 5] = [
    Extra {
        azione: "Seleziona tutto",
        tasti: "Ctrl+A",
        funzione: None,
    },
    Extra {
        azione: "Ripeti (alternativa)",
        tasti: "Ctrl+Y",
        funzione: None,
    },
    Extra {
        azione: "Nuovo appunto",
        tasti: "Ctrl+Alt+N",
        funzione: Some("appunti"),
    },
    Extra {
        azione: "Cerca ovunque",
        tasti: "doppio Maiusc",
        funzione: None,
    },
    Extra {
        azione: "Chiudi il riquadro aperto",
        tasti: "Esc",
        funzione: None,
    },
];

/// Costruisce le sezioni applicando le stesse regole di visibilità del menù.
pub fn tabella(visibile: impl Fn(&Item) -> bool, spente: &BTreeSet<&'static str>) -> Vec<Sezione> {
    let mut sezioni: Vec<Sezione> = MENUS
        .iter()
        .map(|menu| Sezione {
            titolo: menu.name,
            righe: menu
                .items
                .iter()
                .filter(|voce| !voce.shortcut.is_empty() && visibile(voce))
                .map(|voce| Riga {
                    azione: voce.label,
                    tasti: voce.shortcut,
                })
                .collect(),
        })
        .filter(|sezione| !sezione.righe.is_empty())
        .collect();
    let altrove: Vec<Riga> = FUORI_DAI_MENU
        .iter()
        .filter(|extra| extra.funzione.is_none_or(|chiave| !spente.contains(chiave)))
        .map(|extra| Riga {
            azione: extra.azione,
            tasti: extra.tasti,
        })
        .collect();
    if !altrove.is_empty() {
        sezioni.push(Sezione {
            titolo: "Altrove",
            righe: altrove,
        });
    }
    sezioni
}

pub fn view<'a, M: 'a>(t: &LumeTheme, sezioni: &[Sezione]) -> Element<'a, M> {
    let mut elenco = column![].spacing(3);
    for sezione in sezioni {
        elenco = elenco.push(
            container(text(sezione.titolo).size(11).color(t.muted)).padding(Padding {
                top: 8.0,
                bottom: 2.0,
                ..Padding::ZERO
            }),
        );
        for riga in &sezione.righe {
            elenco = elenco.push(
                row![
                    text(riga.azione).size(13).color(t.text).width(Length::Fill),
                    text(riga.tasti).size(12).font(MONO).color(t.sec),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
        }
    }
    let scheda = container(
        column![
            text("Scorciatoie da tastiera").size(14).color(t.text),
            scrollable(elenco).height(Length::Fixed(380.0)),
            text("Esc per chiudere").size(11).color(t.faint),
        ]
        .spacing(10),
    )
    .width(Length::Fixed(460.0))
    .padding(16)
    .style({
        let t = t.clone();
        move |_| container::Style {
            background: Some(Background::Color(t.surface)),
            border: Border {
                color: t.border,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..container::Style::default()
        }
    });
    container(scheda)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .padding(Padding {
            top: 110.0,
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
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use std::collections::BTreeSet;

    use super::*;
    use crate::menu::MENUS;

    const README: &str = include_str!("../../../README.md");

    fn tutte() -> Vec<Sezione> {
        tabella(|_| true, &BTreeSet::new())
    }

    fn righe_attese() -> Vec<String> {
        tutte()
            .iter()
            .flat_map(|sezione| {
                sezione.righe.iter().map(move |riga| {
                    format!(
                        "| {} | {} | `{}` |",
                        sezione.titolo, riga.azione, riga.tasti
                    )
                })
            })
            .collect()
    }

    fn sezione_del_readme() -> String {
        let readme = README.replace("\r\n", "\n");
        let inizio = readme
            .find("## Scorciatoie da tastiera")
            .expect("il README non ha la sezione «## Scorciatoie da tastiera»");
        let dopo = &readme[inizio + 2..];
        let fine = dopo.find("\n## ").unwrap_or(dopo.len());
        dopo[..fine].to_owned()
    }

    #[test]
    fn ogni_scorciatoia_dei_menu_e_nella_tabella() {
        let sezioni = tutte();
        for menu in &MENUS {
            for voce in menu.items.iter().filter(|voce| !voce.shortcut.is_empty()) {
                let trovata = sezioni
                    .iter()
                    .filter(|sezione| sezione.titolo == menu.name)
                    .flat_map(|sezione| &sezione.righe)
                    .any(|riga| riga.azione == voce.label && riga.tasti == voce.shortcut);
                assert!(
                    trovata,
                    "la voce «{}» del menù {} ({}) non è nella tabella",
                    voce.label, menu.name, voce.shortcut
                );
            }
        }
    }

    #[test]
    fn nessuna_combinazione_compare_due_volte() {
        let mut viste = BTreeSet::new();
        for sezione in tutte() {
            for riga in sezione.righe {
                assert!(
                    viste.insert(riga.tasti),
                    "la combinazione {} è assegnata a due azioni (ultima: «{}»)",
                    riga.tasti,
                    riga.azione
                );
            }
        }
    }

    #[test]
    fn una_funzionalita_spenta_toglie_le_sue_righe() {
        let mut spente = BTreeSet::new();
        spente.insert("appunti");
        let senza = tabella(|voce| voce.feature != Some("terminale"), &spente);
        let azioni: Vec<&str> = senza
            .iter()
            .flat_map(|sezione| sezione.righe.iter().map(|riga| riga.azione))
            .collect();
        assert!(
            !azioni.contains(&"Terminale"),
            "Terminale è ancora nell'elenco"
        );
        assert!(
            !azioni.contains(&"Nuovo appunto"),
            "Nuovo appunto è ancora nell'elenco"
        );

        let con: Vec<&str> = tutte()
            .iter()
            .flat_map(|sezione| sezione.righe.iter().map(|riga| riga.azione))
            .collect();
        assert!(con.contains(&"Terminale"));
        assert!(con.contains(&"Nuovo appunto"));
    }

    #[test]
    fn le_funzioni_delle_righe_extra_esistono() {
        for extra in &FUORI_DAI_MENU {
            if let Some(chiave) = extra.funzione {
                assert!(
                    crate::features::by_key(chiave).is_some(),
                    "«{}» dichiara la funzione «{chiave}», che non esiste",
                    extra.azione
                );
            }
        }
    }

    #[test]
    fn il_readme_ha_una_riga_per_ogni_scorciatoia() {
        let sezione = sezione_del_readme();
        let attese = righe_attese();
        for attesa in &attese {
            assert!(
                sezione.contains(attesa.as_str()),
                "manca nel README, sezione «Scorciatoie da tastiera»: {attesa}"
            );
        }
        let righe_di_tabella = sezione
            .lines()
            .filter(|riga| riga.starts_with("| "))
            .count();
        assert_eq!(
            righe_di_tabella - 1,
            attese.len(),
            "il README ha righe in più (o in meno) rispetto alle scorciatoie del codice"
        );
    }
}
