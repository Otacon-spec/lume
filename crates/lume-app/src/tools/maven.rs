//! Pannello «Maven» nella barra destra, come la finestra Maven di IntelliJ
//! (piano 17). Legge `pom.xml` senza avviare Maven: progetto, ciclo di vita,
//! plugin, profili, moduli. Un clic su un goal lo esegue nel Terminale con i
//! profili spuntati. Leggerezza (D-DAP-12): nessun processo, nessun indice;
//! niente albero delle dipendenze risolte, che richiederebbe Maven.

use std::collections::BTreeSet;
use std::sync::Arc;

use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Alignment, Element, Length, Padding};
use lume_theme::LumeTheme;
use lume_ui::icons::Icon;
use lume_ui::tool::{DebugAction, Tool, ToolCtx, ToolEvent, ToolId, ToolRequest};
use lume_ui::{MONO, styles};

use crate::configurazioni_panel::CICLO_MAVEN;
use crate::esecuzione::Pom;

pub const ID: ToolId = ToolId("maven");

/// Quel che l'app sa del progetto Maven.
#[derive(Debug, Clone, Default)]
pub struct MavenView {
    pub pom: Option<Result<Pom, String>>,
    /// `./mvnw` o il percorso di `mvn`; `None` se nessuno dei due c'è.
    pub comando: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Snapshot(Arc<MavenView>),
    Esegui(Vec<String>),
    AlternaProfilo(String),
    Aggiorna,
}

pub fn snapshot_event(view: MavenView) -> ToolEvent {
    ToolEvent::new(Event::Snapshot(Arc::new(view)))
}

#[derive(Default)]
pub struct MavenTool {
    view: Arc<MavenView>,
    profili: BTreeSet<String>,
}

impl MavenTool {
    pub fn new() -> Self {
        Self::default()
    }
}

fn intestazione<'a>(t: &LumeTheme, testo: &'a str) -> Element<'a, ToolEvent> {
    container(text(testo).size(10).font(MONO).color(t.faint))
        .padding(Padding {
            top: 10.0,
            right: 12.0,
            bottom: 3.0,
            left: 12.0,
        })
        .into()
}

fn riga_goal<'a>(
    t: &LumeTheme,
    etichetta: String,
    goal: Vec<String>,
    attiva: bool,
) -> Element<'a, ToolEvent> {
    let colore = if attiva { t.text } else { t.muted };
    let contenuto = row![
        Icon::Play.view(10.0, if attiva { t.good } else { t.faint }),
        text(etichetta).size(12).font(MONO).color(colore),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let mut pulsante = button(contenuto)
        .width(Length::Fill)
        .padding(Padding::from([3, 16]))
        .style(styles::hover_row(colore, t.hover, None, 4.0));
    if attiva {
        pulsante = pulsante.on_press(ToolEvent::new(Event::Esegui(goal)));
    }
    pulsante.into()
}

impl Tool for MavenTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Maven"
    }

    fn icon(&self) -> Icon {
        Icon::Maven
    }

    fn feature(&self) -> Option<&'static str> {
        Some(crate::features::DEBUGGER_KEY)
    }

    fn update(&mut self, ev: &ToolEvent) -> Vec<ToolRequest> {
        let Some(event) = ev.downcast::<Event>() else {
            return Vec::new();
        };
        match event {
            Event::Snapshot(view) => {
                self.view = Arc::clone(view);
                // Profili spariti dal pom non restano attivi.
                if let Some(Ok(pom)) = &self.view.pom {
                    self.profili.retain(|profilo| pom.profili.contains(profilo));
                }
                Vec::new()
            }
            Event::AlternaProfilo(profilo) => {
                if !self.profili.remove(profilo) {
                    self.profili.insert(profilo.clone());
                }
                Vec::new()
            }
            Event::Esegui(goal) => vec![ToolRequest::Debug(DebugAction::MavenEsegui {
                goal: goal.clone(),
                profili: self.profili.iter().cloned().collect(),
            })],
            Event::Aggiorna => vec![ToolRequest::Debug(DebugAction::ReloadConfigs)],
        }
    }

    fn on_visibility_changed(&mut self, open: bool) -> Vec<ToolRequest> {
        // Riaperto: rilegge il pom (una lettura, su gesto).
        if open {
            vec![ToolRequest::Debug(DebugAction::ReloadConfigs)]
        } else {
            Vec::new()
        }
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        let mut corpo = column![].spacing(0);
        let aggiorna = button(Icon::Refresh.view(12.0, t.icon))
            .padding(Padding::from([3, 6]))
            .style(styles::hover_row(t.icon, t.hover2, None, 4.0))
            .on_press(ToolEvent::new(Event::Aggiorna));
        let pom = match &self.view.pom {
            None => {
                return container(
                    text("Nessun pom.xml nella radice del progetto.")
                        .size(12)
                        .color(t.muted),
                )
                .padding(12)
                .into();
            }
            Some(Err(errore)) => {
                return column![
                    row![Space::new().width(Length::Fill), aggiorna],
                    container(text(errore.as_str()).size(12).color(t.bad)).padding(12),
                ]
                .into();
            }
            Some(Ok(pom)) => pom,
        };
        let nome = pom.artifact_id.as_deref().unwrap_or("(senza artifactId)");
        let mut dettagli = Vec::new();
        if let Some(versione) = &pom.version {
            dettagli.push(versione.clone());
        }
        dettagli.push(pom.packaging.clone().unwrap_or_else(|| "jar".into()));
        corpo = corpo.push(
            container(
                row![
                    column![
                        text(nome).size(13).color(t.text),
                        text(format!(
                            "{}{}",
                            pom.group_id
                                .as_deref()
                                .map(|g| format!("{g} · "))
                                .unwrap_or_default(),
                            dettagli.join(" · ")
                        ))
                        .size(10.5)
                        .font(MONO)
                        .color(t.muted),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    aggiorna,
                ]
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([8, 12])),
        );
        let eseguibile = self.view.comando.is_some();
        corpo = corpo.push(
            container(
                text(match &self.view.comando {
                    Some(comando) => format!("con {comando}"),
                    None => {
                        "mvn non trovato su PATH e nessun ./mvnw: i goal non si eseguono".into()
                    }
                })
                .size(10.5)
                .color(if eseguibile { t.faint } else { t.warn }),
            )
            .padding(Padding::from([0, 12])),
        );
        corpo = corpo.push(intestazione(t, "CICLO DI VITA"));
        for goal in CICLO_MAVEN {
            corpo = corpo.push(riga_goal(
                t,
                goal.to_owned(),
                vec![goal.to_owned()],
                eseguibile,
            ));
        }
        if !pom.plugin.is_empty() {
            corpo = corpo.push(intestazione(t, "PLUGIN"));
            for plugin in &pom.plugin {
                let breve = plugin.rsplit(':').next().unwrap_or(plugin);
                corpo = corpo.push(
                    container(text(breve).size(12).color(t.sec)).padding(Padding::from([3, 16])),
                );
                if breve == "exec-maven-plugin" {
                    corpo = corpo.push(
                        container(riga_goal(
                            t,
                            "exec:java".into(),
                            vec!["exec:java".into()],
                            eseguibile,
                        ))
                        .padding(Padding {
                            top: 0.0,
                            right: 0.0,
                            bottom: 0.0,
                            left: 12.0,
                        }),
                    );
                }
            }
        }
        if !pom.profili.is_empty() {
            corpo = corpo.push(intestazione(t, "PROFILI"));
            for profilo in &pom.profili {
                let attivo = self.profili.contains(profilo);
                corpo = corpo.push(
                    button(
                        row![
                            text(if attivo { "☑" } else { "☐" })
                                .size(13)
                                .color(t.accent),
                            text(profilo.as_str()).size(12).color(t.text),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding(Padding::from([2, 16]))
                    .style(styles::hover_row(t.text, t.hover, None, 4.0))
                    .on_press(ToolEvent::new(Event::AlternaProfilo(profilo.clone()))),
                );
            }
        }
        if !pom.moduli.is_empty() {
            corpo = corpo.push(intestazione(t, "MODULI"));
            for modulo in &pom.moduli {
                corpo = corpo.push(
                    container(text(modulo.as_str()).size(12).color(t.sec))
                        .padding(Padding::from([3, 16])),
                );
            }
        }
        scrollable(corpo).height(Length::Fill).into()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn un_goal_porta_con_se_i_profili_spuntati() {
        let mut tool = MavenTool::new();
        tool.update(&snapshot_event(MavenView {
            pom: Some(Ok(Pom {
                profili: vec!["dev".into(), "it".into()],
                ..Pom::default()
            })),
            comando: Some("mvn".into()),
        }));
        tool.update(&ToolEvent::new(Event::AlternaProfilo("it".into())));
        let richieste = tool.update(&ToolEvent::new(Event::Esegui(vec!["test".into()])));
        assert!(matches!(
            richieste.as_slice(),
            [ToolRequest::Debug(DebugAction::MavenEsegui { goal, profili })]
                if goal == &["test"] && profili == &["it"]
        ));
        // Il profilo sparito dal pom si dimentica.
        tool.update(&snapshot_event(MavenView {
            pom: Some(Ok(Pom::default())),
            comando: Some("mvn".into()),
        }));
        assert!(tool.profili.is_empty());
    }
}
