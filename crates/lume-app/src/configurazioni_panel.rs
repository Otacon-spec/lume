//! Finestra «Configurazioni di esecuzione» (stile IntelliJ, piano 17):
//! elenco raggruppato per tipo a sinistra, modulo a destra, + / duplica /
//! rimuovi. Salva riscrive `.lume/launch.toml`.
//!
//! Lo stato è una bozza di testi: i campi non si riformattano mentre si
//! scrive; la conversione e la validazione avvengono al salvataggio.

use iced::widget::{
    Space, button, column, container, mouse_area, opaque, row, scrollable, text, text_input,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use lume_theme::LumeTheme;
use lume_ui::{MONO, styles};

use crate::configurazioni::{
    Configurazione, Tipo, dividi_ambiente, dividi_argomenti, dividi_elenco, unisci_ambiente,
    unisci_argomenti,
};

/// Goal del ciclo di vita di Maven offerti a un clic.
pub const CICLO_MAVEN: [&str; 7] = [
    "clean", "validate", "compile", "test", "package", "verify", "install",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bozza {
    pub tipo: Tipo,
    pub nome: String,
    pub programma: String,
    pub argomenti: String,
    pub cartella: String,
    pub ambiente: String,
    pub goal: String,
    pub profili: String,
    pub fermati_all_inizio: bool,
    adattatore: Option<String>,
    richiesta: Option<String>,
}

impl Bozza {
    pub fn da(configurazione: &Configurazione) -> Self {
        Self {
            tipo: configurazione.tipo,
            nome: configurazione.nome.clone(),
            programma: configurazione.programma.clone(),
            argomenti: unisci_argomenti(&configurazione.argomenti),
            cartella: configurazione.cartella.clone().unwrap_or_default(),
            ambiente: unisci_ambiente(&configurazione.ambiente),
            goal: configurazione.goal.join(" "),
            profili: configurazione.profili.join(", "),
            fermati_all_inizio: configurazione.fermati_all_inizio,
            adattatore: configurazione.adattatore.clone(),
            richiesta: configurazione.richiesta.clone(),
        }
    }

    pub fn configurazione(&self) -> Result<Configurazione, String> {
        let configurazione = Configurazione {
            nome: self.nome.trim().to_owned(),
            tipo: self.tipo,
            adattatore: match self.tipo {
                Tipo::Applicazione => self.adattatore.clone().or_else(|| Some("gdb".into())),
                _ => None,
            },
            richiesta: self.richiesta.clone(),
            programma: match self.tipo {
                Tipo::Maven => String::new(),
                _ => self.programma.trim().to_owned(),
            },
            argomenti: dividi_argomenti(&self.argomenti),
            cartella: Some(self.cartella.trim().to_owned()).filter(|c| !c.is_empty()),
            ambiente: dividi_ambiente(&self.ambiente)?,
            goal: match self.tipo {
                Tipo::Maven => dividi_elenco(&self.goal),
                _ => Vec::new(),
            },
            profili: match self.tipo {
                Tipo::Maven => dividi_elenco(&self.profili),
                _ => Vec::new(),
            },
            fermati_all_inizio: self.tipo == Tipo::Applicazione && self.fermati_all_inizio,
        };
        match configurazione.problema() {
            Some(problema) => Err(problema),
            None => Ok(configurazione),
        }
    }

    fn ha_goal(&self, goal: &str) -> bool {
        dividi_elenco(&self.goal).iter().any(|g| g == goal)
    }

    fn alterna_goal(&mut self, goal: &str) {
        let mut goal_attuali = dividi_elenco(&self.goal);
        if let Some(posizione) = goal_attuali.iter().position(|g| g == goal) {
            goal_attuali.remove(posizione);
        } else {
            // In ordine di ciclo di vita, come li mostra Maven.
            let rango = |g: &str| {
                CICLO_MAVEN
                    .iter()
                    .position(|c| *c == g)
                    .unwrap_or(usize::MAX)
            };
            let dove = goal_attuali
                .iter()
                .position(|g| rango(g) > rango(goal))
                .unwrap_or(goal_attuali.len());
            goal_attuali.insert(dove, goal.to_owned());
        }
        self.goal = goal_attuali.join(" ");
    }
}

#[derive(Debug, Clone, Default)]
pub struct Editor {
    pub voci: Vec<Bozza>,
    pub scelta: Option<usize>,
    pub modificato: bool,
    pub errore: Option<String>,
    /// C'è un `pom.xml` (per suggerire il tipo Maven).
    pub maven: bool,
}

impl Editor {
    pub fn nuovo(configurazioni: &[Configurazione], maven: bool) -> Self {
        let voci: Vec<Bozza> = configurazioni.iter().map(Bozza::da).collect();
        Self {
            scelta: (!voci.is_empty()).then_some(0),
            voci,
            modificato: false,
            errore: None,
            maven,
        }
    }

    fn corrente(&mut self) -> Option<&mut Bozza> {
        self.scelta.and_then(|indice| self.voci.get_mut(indice))
    }

    fn nome_libero(&self, base: &str) -> String {
        let esiste = |nome: &str| self.voci.iter().any(|voce| voce.nome.trim() == nome);
        if !esiste(base) {
            return base.to_owned();
        }
        (2..)
            .map(|numero| format!("{base} ({numero})"))
            .find(|nome| !esiste(nome))
            .unwrap_or_else(|| base.to_owned())
    }

    /// Tutte le configurazioni, o il primo problema con il nome di chi lo ha.
    pub fn configurazioni(&self) -> Result<Vec<Configurazione>, String> {
        let mut nomi = std::collections::BTreeSet::new();
        self.voci
            .iter()
            .map(|voce| {
                let configurazione = voce
                    .configurazione()
                    .map_err(|problema| format!("«{}»: {problema}", voce.nome.trim()))?;
                if !nomi.insert(configurazione.nome.clone()) {
                    return Err(format!(
                        "due configurazioni si chiamano «{}»",
                        configurazione.nome
                    ));
                }
                Ok(configurazione)
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub enum Messaggio {
    Seleziona(usize),
    Aggiungi(Tipo),
    Duplica,
    Rimuovi,
    Nome(String),
    Programma(String),
    Argomenti(String),
    Cartella(String),
    Ambiente(String),
    Goal(String),
    AlternaGoal(&'static str),
    Profili(String),
    Fermati(bool),
    Salva,
    ApriTesto,
    Chiudi,
}

/// Che cosa deve fare l'app dopo un messaggio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Azione {
    Nessuna,
    Salva(Vec<Configurazione>),
    ApriTesto,
    Chiudi,
}

pub fn aggiorna(editor: &mut Editor, messaggio: Messaggio) -> Azione {
    let cambia = |editor: &mut Editor, f: &dyn Fn(&mut Bozza)| {
        if let Some(bozza) = editor.corrente() {
            f(bozza);
            editor.modificato = true;
            editor.errore = None;
        }
    };
    match messaggio {
        Messaggio::Seleziona(indice) => {
            if indice < editor.voci.len() {
                editor.scelta = Some(indice);
            }
        }
        Messaggio::Aggiungi(tipo) => {
            let nome = editor.nome_libero(match tipo {
                Tipo::Applicazione => "Applicazione",
                Tipo::Maven => "Maven",
                Tipo::Comando => "Comando",
            });
            editor
                .voci
                .push(Bozza::da(&Configurazione::nuova(tipo, nome)));
            editor.scelta = Some(editor.voci.len() - 1);
            editor.modificato = true;
        }
        Messaggio::Duplica => {
            if let Some(bozza) = editor.scelta.and_then(|i| editor.voci.get(i)).cloned() {
                let nome = editor.nome_libero(bozza.nome.trim());
                editor.voci.push(Bozza { nome, ..bozza });
                editor.scelta = Some(editor.voci.len() - 1);
                editor.modificato = true;
            }
        }
        Messaggio::Rimuovi => {
            if let Some(indice) = editor.scelta.filter(|i| *i < editor.voci.len()) {
                editor.voci.remove(indice);
                editor.scelta = if editor.voci.is_empty() {
                    None
                } else {
                    Some(indice.min(editor.voci.len() - 1))
                };
                editor.modificato = true;
            }
        }
        Messaggio::Nome(v) => cambia(editor, &|b| b.nome.clone_from(&v)),
        Messaggio::Programma(v) => cambia(editor, &|b| b.programma.clone_from(&v)),
        Messaggio::Argomenti(v) => cambia(editor, &|b| b.argomenti.clone_from(&v)),
        Messaggio::Cartella(v) => cambia(editor, &|b| b.cartella.clone_from(&v)),
        Messaggio::Ambiente(v) => cambia(editor, &|b| b.ambiente.clone_from(&v)),
        Messaggio::Goal(v) => cambia(editor, &|b| b.goal.clone_from(&v)),
        Messaggio::AlternaGoal(goal) => cambia(editor, &|b| b.alterna_goal(goal)),
        Messaggio::Profili(v) => cambia(editor, &|b| b.profili.clone_from(&v)),
        Messaggio::Fermati(v) => cambia(editor, &|b| b.fermati_all_inizio = v),
        Messaggio::Salva => match editor.configurazioni() {
            Ok(configurazioni) => return Azione::Salva(configurazioni),
            Err(problema) => editor.errore = Some(problema),
        },
        Messaggio::ApriTesto => return Azione::ApriTesto,
        Messaggio::Chiudi => return Azione::Chiudi,
    }
    Azione::Nessuna
}

// ── Vista ───────────────────────────────────────────────────────────

fn pulsante<'a>(
    t: &LumeTheme,
    etichetta: &'a str,
    colore: Color,
    messaggio: Messaggio,
) -> Element<'a, Messaggio> {
    button(text(etichetta).size(12).color(colore))
        .padding(Padding::from([4, 10]))
        .style(styles::hover_row(colore, t.hover2, None, 4.0))
        .on_press(messaggio)
        .into()
}

fn campo<'a>(
    t: &LumeTheme,
    etichetta: &'a str,
    segnaposto: &'a str,
    valore: &'a str,
    messaggio: fn(String) -> Messaggio,
    mono: bool,
) -> Element<'a, Messaggio> {
    let mut input = text_input(segnaposto, valore)
        .on_input(messaggio)
        .on_submit(Messaggio::Salva)
        .size(12.5)
        .padding(Padding::from([5, 8]))
        .style(styles::input(t));
    if mono {
        input = input.font(MONO);
    }
    column![text(etichetta).size(11).color(t.muted), input]
        .spacing(4)
        .into()
}

fn elenco<'a>(t: &LumeTheme, editor: &'a Editor) -> Element<'a, Messaggio> {
    let mut colonna = column![].spacing(1);
    for tipo in Tipo::ALL {
        let voci: Vec<(usize, &Bozza)> = editor
            .voci
            .iter()
            .enumerate()
            .filter(|(_, voce)| voce.tipo == tipo)
            .collect();
        if voci.is_empty() {
            continue;
        }
        colonna = colonna.push(
            container(
                text(tipo.etichetta().to_uppercase())
                    .size(10)
                    .font(MONO)
                    .color(t.faint),
            )
            .padding(Padding {
                top: 8.0,
                right: 8.0,
                bottom: 2.0,
                left: 8.0,
            }),
        );
        for (indice, voce) in voci {
            let scelta = editor.scelta == Some(indice);
            let nome = if voce.nome.trim().is_empty() {
                "(senza nome)"
            } else {
                voce.nome.as_str()
            };
            colonna = colonna.push(
                button(
                    text(nome)
                        .size(12.5)
                        .color(t.text)
                        .wrapping(text::Wrapping::None),
                )
                .width(Length::Fill)
                .padding(Padding::from([4, 14]))
                .style(styles::hover_row(
                    t.text,
                    t.hover,
                    scelta.then_some(t.sel),
                    4.0,
                ))
                .on_press(Messaggio::Seleziona(indice)),
            );
        }
    }
    if editor.voci.is_empty() {
        colonna = colonna.push(
            container(
                text("Nessuna configurazione. Aggiungine una con i pulsanti qui sotto.")
                    .size(11.5)
                    .color(t.muted),
            )
            .padding(10),
        );
    }
    let mut aggiungi = column![text("Aggiungi").size(10.5).color(t.faint)].spacing(2);
    for tipo in Tipo::ALL {
        if tipo == Tipo::Maven && !editor.maven {
            continue;
        }
        aggiungi = aggiungi.push(pulsante(
            t,
            match tipo {
                Tipo::Applicazione => "+ Applicazione",
                Tipo::Maven => "+ Maven",
                Tipo::Comando => "+ Comando",
            },
            t.accent,
            Messaggio::Aggiungi(tipo),
        ));
    }
    let mut azioni = row![].spacing(2);
    if editor.scelta.is_some() {
        azioni = azioni
            .push(pulsante(t, "Duplica", t.sec, Messaggio::Duplica))
            .push(pulsante(t, "Rimuovi", t.bad, Messaggio::Rimuovi));
    }
    column![scrollable(colonna).height(Length::Fill), aggiungi, azioni,]
        .spacing(8)
        .padding(8)
        .into()
}

fn modulo<'a>(t: &LumeTheme, bozza: &'a Bozza) -> Element<'a, Messaggio> {
    let mut campi = column![
        row![
            text(bozza.tipo.etichetta())
                .size(11)
                .font(MONO)
                .color(t.accent),
            Space::new().width(Length::Fill),
        ],
        campo(
            t,
            "Nome",
            "come compare nel widget Esegui",
            &bozza.nome,
            Messaggio::Nome,
            false
        ),
    ]
    .spacing(10);
    match bozza.tipo {
        Tipo::Applicazione | Tipo::Comando => {
            let segnaposto = if bozza.tipo == Tipo::Applicazione {
                "${workspaceFolder}/target/debug/programma"
            } else {
                "python3, npm, ${workspaceFolder}/script.sh …"
            };
            campi = campi
                .push(campo(
                    t,
                    "Programma",
                    segnaposto,
                    &bozza.programma,
                    Messaggio::Programma,
                    true,
                ))
                .push(campo(
                    t,
                    "Argomenti",
                    "separati da spazi; \"tra virgolette\" se ne hanno",
                    &bozza.argomenti,
                    Messaggio::Argomenti,
                    true,
                ));
        }
        Tipo::Maven => {
            let mut chip = row![].spacing(4);
            for goal in CICLO_MAVEN {
                let acceso = bozza.ha_goal(goal);
                let colore = if acceso { t.accent } else { t.sec };
                chip = chip.push(
                    button(text(goal).size(11.5).font(MONO).color(colore))
                        .padding(Padding::from([3, 8]))
                        .style(styles::hover_row(
                            colore,
                            t.hover2,
                            acceso.then_some(t.sel),
                            4.0,
                        ))
                        .on_press(Messaggio::AlternaGoal(goal)),
                );
            }
            campi = campi
                .push(column![text("Ciclo di vita").size(11).color(t.muted), chip].spacing(4))
                .push(campo(
                    t,
                    "Goal",
                    "es. clean package  o  exec:java",
                    &bozza.goal,
                    Messaggio::Goal,
                    true,
                ))
                .push(campo(
                    t,
                    "Profili",
                    "separati da virgola",
                    &bozza.profili,
                    Messaggio::Profili,
                    true,
                ))
                .push(campo(
                    t,
                    "Opzioni",
                    "es. -DskipTests -Dexec.args=\"a b\"",
                    &bozza.argomenti,
                    Messaggio::Argomenti,
                    true,
                ));
        }
    }
    campi = campi
        .push(campo(
            t,
            "Cartella di lavoro",
            "vuota = radice del progetto",
            &bozza.cartella,
            Messaggio::Cartella,
            true,
        ))
        .push(campo(
            t,
            "Variabili d'ambiente",
            "CHIAVE=valore; ALTRA=valore",
            &bozza.ambiente,
            Messaggio::Ambiente,
            true,
        ));
    if bozza.tipo == Tipo::Applicazione {
        let segno = if bozza.fermati_all_inizio {
            "☑"
        } else {
            "☐"
        };
        campi = campi.push(
            button(
                row![
                    text(segno).size(13).color(t.accent),
                    text("Fermati all'inizio di main (debug)")
                        .size(12)
                        .color(t.text)
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([3, 4]))
            .style(styles::hover_row(t.text, t.hover, None, 4.0))
            .on_press(Messaggio::Fermati(!bozza.fermati_all_inizio)),
        );
    }
    let nota = match bozza.tipo {
        Tipo::Applicazione => "▶ esegue il programma nel Terminale; 🐞 lo avvia con gdb.",
        Tipo::Maven => "▶ esegue mvn (o ./mvnw) nel Terminale. Maven parte solo quando premi ▶.",
        Tipo::Comando => {
            "▶ esegue il comando nel Terminale. Nessuna shell: argomenti come scritti."
        }
    };
    campi = campi.push(text(nota).size(11).color(t.faint));
    scrollable(container(campi).padding(Padding::from([12, 16])))
        .height(Length::Fill)
        .into()
}

pub fn view<'a>(editor: &'a Editor, t: &LumeTheme) -> Element<'a, Messaggio> {
    let destra: Element<'a, Messaggio> = match editor.scelta.and_then(|i| editor.voci.get(i)) {
        Some(bozza) => modulo(t, bozza),
        None => container(
            text("Scegli o aggiungi una configurazione.")
                .size(12)
                .color(t.muted),
        )
        .padding(16)
        .into(),
    };
    let separatore = {
        let colore = t.border;
        container(Space::new().width(1).height(Length::Fill)).style(move |_| styles::bg(colore))
    };
    let mut piede = row![].spacing(6).align_y(Alignment::Center);
    piede = match &editor.errore {
        Some(errore) => piede.push(
            text(errore.as_str())
                .size(11.5)
                .color(t.bad)
                .width(Length::Fill),
        ),
        None => piede.push(
            text(if editor.modificato {
                "Modifiche non salvate"
            } else {
                ".lume/launch.toml"
            })
            .size(11)
            .color(t.muted)
            .width(Length::Fill),
        ),
    };
    piede = piede
        .push(pulsante(t, "Apri come testo", t.sec, Messaggio::ApriTesto))
        .push(pulsante(t, "Annulla", t.sec, Messaggio::Chiudi))
        .push(pulsante(t, "Salva", t.good, Messaggio::Salva));
    let card = container(
        column![
            text("Configurazioni di esecuzione").size(13).color(t.text),
            row![
                container(elenco(t, editor)).width(230).height(Length::Fill),
                separatore,
                container(destra).width(Length::Fill).height(Length::Fill),
            ]
            .height(Length::Fill),
            piede,
        ]
        .spacing(10),
    )
    .width(Length::Fixed(820.0))
    .height(Length::Fixed(540.0))
    .padding(Padding::from([12, 14]))
    .style({
        let (surface, border) = (t.surface, t.border);
        move |_| container::Style {
            background: Some(Background::Color(surface)),
            border: Border {
                color: border,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..container::Style::default()
        }
    });
    mouse_area(
        container(opaque(card))
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
    .on_press(Messaggio::Chiudi)
    .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn aggiungere_modificare_e_salvare() {
        let mut editor = Editor::nuovo(&[], true);
        assert_eq!(
            aggiorna(&mut editor, Messaggio::Aggiungi(Tipo::Maven)),
            Azione::Nessuna
        );
        aggiorna(&mut editor, Messaggio::AlternaGoal("clean"));
        aggiorna(&mut editor, Messaggio::Profili("dev, it".into()));
        aggiorna(&mut editor, Messaggio::Argomenti("-DskipTests".into()));
        let Azione::Salva(configurazioni) = aggiorna(&mut editor, Messaggio::Salva) else {
            panic!("doveva salvare");
        };
        assert_eq!(configurazioni[0].goal, ["clean", "package"]);
        assert_eq!(configurazioni[0].profili, ["dev", "it"]);
        assert_eq!(configurazioni[0].argomenti, ["-DskipTests"]);
    }

    #[test]
    fn nomi_duplicati_e_campi_mancanti_bloccano_il_salvataggio() {
        let mut editor = Editor::nuovo(&[], false);
        aggiorna(&mut editor, Messaggio::Aggiungi(Tipo::Comando));
        assert_eq!(aggiorna(&mut editor, Messaggio::Salva), Azione::Nessuna);
        assert!(editor.errore.as_deref().unwrap().contains("programma"));
        aggiorna(&mut editor, Messaggio::Programma("python3".into()));
        aggiorna(&mut editor, Messaggio::Duplica);
        assert_eq!(editor.voci[1].nome, "Comando (2)");
        aggiorna(&mut editor, Messaggio::Nome("Comando".into()));
        assert_eq!(aggiorna(&mut editor, Messaggio::Salva), Azione::Nessuna);
        assert!(
            editor
                .errore
                .as_deref()
                .unwrap()
                .contains("due configurazioni")
        );
        aggiorna(&mut editor, Messaggio::Rimuovi);
        assert!(
            matches!(aggiorna(&mut editor, Messaggio::Salva), Azione::Salva(c) if c.len() == 1)
        );
    }

    #[test]
    fn i_goal_restano_in_ordine_di_ciclo_di_vita() {
        let mut bozza = Bozza::da(&Configurazione::nuova(Tipo::Maven, "m".into()));
        bozza.alterna_goal("clean");
        bozza.alterna_goal("install");
        bozza.alterna_goal("compile");
        assert_eq!(bozza.goal, "clean compile package install");
        bozza.alterna_goal("package");
        assert_eq!(bozza.goal, "clean compile install");
    }
}
