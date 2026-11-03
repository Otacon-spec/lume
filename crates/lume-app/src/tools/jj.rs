//! Pannello Jujutsu opzionale. Chiudendo il pannello, snapshot ed errori
//! vengono scartati; `jj` parte solo in risposta a un'azione esplicita.
use std::path::PathBuf;

use iced::widget::{button, column, container, row, scrollable, text, text_input, tooltip};
use iced::{Alignment, Element, Length, Padding};
use lume_jj::JjSnapshot;
use lume_ui::icons::Icon;
use lume_ui::tool::{JjAction, Tool, ToolCtx, ToolEvent, ToolId, ToolRequest};
use lume_ui::{MONO, styles};

pub const ID: ToolId = ToolId("jj");
const ACTION_TARGET: f32 = 28.0;
const _: () = assert!(ACTION_TARGET >= 28.0);

#[derive(Debug, Clone)]
pub enum Event {
    Message(String),
    Refresh,
    Describe,
    New,
    Push,
    Cancel,
    SetWorkspace(Option<PathBuf>),
    Loaded(Result<JjSnapshot, String>),
    ActionDone { action: JjAction, ok: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Refresh,
    New,
    Push,
    Describe,
    Cancel,
}

pub struct JjTool {
    locate: fn() -> bool,
    workspace: Option<PathBuf>,
    jj_found: bool,
    open: bool,
    snapshot: Option<JjSnapshot>,
    loading: bool,
    busy: Option<&'static str>,
    error: Option<String>,
    draft: String,
}

fn jj_on_path() -> bool {
    lume_jj::JjCli::locate().is_some()
}

impl JjTool {
    pub fn new() -> Self {
        Self::with_locator(jj_on_path)
    }

    pub fn with_locator(locate: fn() -> bool) -> Self {
        Self {
            locate,
            workspace: None,
            jj_found: false,
            open: false,
            snapshot: None,
            loading: false,
            busy: None,
            error: None,
            draft: String::new(),
        }
    }

    pub fn controls(&self) -> Vec<Control> {
        if !self.jj_found || self.workspace.is_none() {
            return Vec::new();
        }
        if self.loading || self.busy.is_some() {
            return vec![Control::Cancel];
        }
        let mut controls = vec![Control::Refresh, Control::New, Control::Push];
        if !self.draft.trim().is_empty() {
            controls.push(Control::Describe);
        }
        controls
    }

    fn can_act(&self) -> bool {
        self.open
            && self.jj_found
            && self.workspace.is_some()
            && !self.loading
            && self.busy.is_none()
    }

    fn start_load(&mut self) -> Vec<ToolRequest> {
        if !self.can_act() {
            return Vec::new();
        }
        self.loading = true;
        self.error = None;
        vec![ToolRequest::JjLoad]
    }

    fn start_action(&mut self, action: JjAction) -> Vec<ToolRequest> {
        if !self.can_act() {
            return Vec::new();
        }
        self.busy = Some(action.label());
        self.error = None;
        vec![ToolRequest::JjAction(action)]
    }

    fn control_button<'a>(control: Control) -> Element<'a, ToolEvent> {
        let (label, tip, event) = match control {
            Control::Refresh => ("Aggiorna", "Rilegge cambiamenti e stato", Event::Refresh),
            Control::New => (
                "Nuovo",
                "jj new: nuovo cambiamento sopra la copia di lavoro",
                Event::New,
            ),
            Control::Push => ("Push", "jj git push: i bookmark tracciati", Event::Push),
            Control::Describe => (
                "Descrivi",
                "jj describe con il messaggio scritto",
                Event::Describe,
            ),
            Control::Cancel => ("Annulla", "Interrompe jj", Event::Cancel),
        };
        tooltip(
            button(text(label).size(11))
                .height(ACTION_TARGET)
                .on_press(ToolEvent::new(event)),
            tip,
            tooltip::Position::Top,
        )
        .into()
    }
}

impl Default for JjTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for JjTool {
    fn id(&self) -> ToolId {
        ID
    }
    fn title(&self) -> &'static str {
        "Jujutsu"
    }
    fn icon(&self) -> Icon {
        Icon::Jujutsu
    }
    fn feature(&self) -> Option<&'static str> {
        None
    }

    fn on_visibility_changed(&mut self, open: bool) -> Vec<ToolRequest> {
        if open {
            self.open = true;
            self.jj_found = (self.locate)();
            return self.start_load();
        }
        self.open = false;
        let cancel = self.loading && self.busy.is_none();
        self.loading = false;
        self.snapshot = None;
        self.error = None;
        if cancel {
            vec![ToolRequest::JjCancel]
        } else {
            Vec::new()
        }
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        if !self.jj_found {
            return container(column![text("jj non è nel PATH").size(12),
                text("Questo progetto è un workspace Jujutsu. Per usarlo serve il programma jj: installalo e riapri il pannello.")
                    .size(11).color(t.muted)].spacing(6)).padding(12).into();
        }
        let root = self
            .workspace
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let controls = self.controls();
        let mut buttons = row![].spacing(6).align_y(Alignment::Center);
        for control in controls.iter().filter(|c| **c != Control::Describe) {
            buttons = buttons.push(Self::control_button(*control));
        }
        let mut message = row![
            text_input("Messaggio del cambiamento…", &self.draft)
                .on_input(|value| ToolEvent::new(Event::Message(value)))
                .size(12)
                .padding(Padding::from([5.0, 9.0]))
                .style(styles::input(t))
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if controls.contains(&Control::Describe) {
            message = message.push(Self::control_button(Control::Describe));
        }
        let mut body = column![].spacing(4);
        if let Some(label) = self.busy {
            body = body.push(text(format!("{label}…")).size(11).color(t.muted));
        } else if self.loading {
            body = body.push(text("Caricamento…").size(11).color(t.muted));
        }
        if let Some(error) = &self.error {
            body = body.push(text(error.clone()).size(11).color(t.bad));
        }
        if let Some(snapshot) = &self.snapshot {
            body = body.push(
                text(snapshot.status.clone())
                    .size(11)
                    .font(MONO)
                    .color(t.muted),
            );
            for entry in &snapshot.entries {
                let marker = if entry.working_copy { "@" } else { "○" };
                let change: String = entry.change.chars().take(8).collect();
                let description = if entry.description.trim().is_empty() {
                    "(senza descrizione)"
                } else {
                    &entry.description
                };
                let mut line = row![
                    text(format!("{marker} {change}"))
                        .size(11)
                        .font(MONO)
                        .color(t.muted),
                    text(description).size(12)
                ]
                .spacing(8);
                if !entry.bookmarks.is_empty() {
                    line = line.push(text(entry.bookmarks.clone()).size(10).color(t.accent));
                }
                if entry.conflict {
                    line = line.push(text("conflitto").size(10).color(t.bad));
                }
                body = body.push(line);
            }
        }
        column![
            container(column![text(root).size(10).color(t.muted), buttons, message].spacing(8))
                .padding(10),
            scrollable(container(body).padding(Padding::from([0.0, 10.0]))).height(Length::Fill)
        ]
        .height(Length::Fill)
        .into()
    }

    fn update(&mut self, event: &ToolEvent) -> Vec<ToolRequest> {
        match event.downcast::<Event>() {
            Some(Event::Message(value)) => {
                self.draft.clone_from(value);
                Vec::new()
            }
            Some(Event::Refresh) => self.start_load(),
            Some(Event::Describe) => {
                if self.draft.trim().is_empty() {
                    return Vec::new();
                }
                self.start_action(JjAction::Describe(self.draft.clone()))
            }
            Some(Event::New) => self.start_action(JjAction::New(
                (!self.draft.trim().is_empty()).then(|| self.draft.clone()),
            )),
            Some(Event::Push) => self.start_action(JjAction::Push),
            Some(Event::Cancel) => {
                if self.loading || self.busy.is_some() {
                    vec![ToolRequest::JjCancel]
                } else {
                    Vec::new()
                }
            }
            Some(Event::SetWorkspace(workspace)) => {
                self.workspace.clone_from(workspace);
                self.snapshot = None;
                self.error = None;
                self.loading = false;
                self.busy = None;
                self.draft.clear();
                if self.open {
                    self.start_load()
                } else {
                    Vec::new()
                }
            }
            Some(Event::Loaded(result)) => {
                self.loading = false;
                if !self.open {
                    return Vec::new();
                }
                match result {
                    Ok(snapshot) => {
                        self.snapshot = Some(snapshot.clone());
                        self.error = None;
                    }
                    Err(error) => {
                        self.snapshot = None;
                        self.error = Some(error.clone());
                    }
                }
                Vec::new()
            }
            Some(Event::ActionDone { action, ok }) => {
                self.busy = None;
                if !ok {
                    return Vec::new();
                }
                let sent = match action {
                    JjAction::Describe(m) | JjAction::New(Some(m)) => Some(m),
                    JjAction::New(None) | JjAction::Push => None,
                };
                if sent.is_some_and(|message| message == &self.draft) {
                    self.draft.clear();
                }
                if self.open {
                    self.start_load()
                } else {
                    Vec::new()
                }
            }
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use lume_jj::JjSnapshot;
    use lume_ui::tool::{JjAction, Tool, ToolEvent, ToolRequest};
    use std::{cell::Cell, path::PathBuf};

    thread_local! { static FOUND: Cell<bool> = const { Cell::new(true) }; }
    fn found() -> bool {
        FOUND.with(Cell::get)
    }
    fn tool(jj_found: bool, workspace: bool) -> JjTool {
        FOUND.with(|f| f.set(jj_found));
        let mut tool = JjTool::with_locator(found);
        if workspace {
            let _ = ev(
                &mut tool,
                Event::SetWorkspace(Some(PathBuf::from("/prova/ws"))),
            );
        }
        tool
    }
    fn ev(tool: &mut JjTool, event: Event) -> Vec<ToolRequest> {
        tool.update(&ToolEvent::new(event))
    }
    fn snapshot() -> JjSnapshot {
        JjSnapshot {
            entries: Vec::new(),
            status: "clean".into(),
        }
    }
    fn pronto() -> JjTool {
        let mut tool = tool(true, true);
        assert!(matches!(
            tool.on_visibility_changed(true).as_slice(),
            [ToolRequest::JjLoad]
        ));
        let _ = ev(&mut tool, Event::Loaded(Ok(snapshot())));
        tool
    }
    #[test]
    fn identita() {
        let t = tool(true, false);
        assert_eq!(t.id(), ID);
        assert_eq!(t.title(), "Jujutsu");
        assert_eq!(t.icon(), Icon::Jujutsu);
        assert_eq!(t.feature(), None);
    }
    #[test]
    fn senza_jj_nessun_pulsante_ne_richiesta() {
        let mut t = tool(false, true);
        assert!(t.on_visibility_changed(true).is_empty());
        assert!(t.controls().is_empty());
        assert!(ev(&mut t, Event::Push).is_empty());
    }
    #[test]
    fn senza_workspace_nessun_controllo() {
        let mut t = tool(true, false);
        assert!(t.on_visibility_changed(true).is_empty());
        assert!(t.controls().is_empty());
    }
    #[test]
    fn apertura_e_caricamento_singolo() {
        let mut t = tool(true, true);
        assert!(matches!(
            t.on_visibility_changed(true).as_slice(),
            [ToolRequest::JjLoad]
        ));
        assert!(ev(&mut t, Event::Refresh).is_empty());
        assert_eq!(t.controls(), vec![Control::Cancel]);
    }
    #[test]
    fn stato_pronto_mostra_controlli() {
        let t = pronto();
        assert!(t.snapshot.is_some());
        assert_eq!(
            t.controls(),
            vec![Control::Refresh, Control::New, Control::Push]
        );
    }
    #[test]
    fn errore_caricamento_puo_essere_riprovato() {
        let mut t = tool(true, true);
        let _ = t.on_visibility_changed(true);
        let _ = ev(&mut t, Event::Loaded(Err("boom".into())));
        assert_eq!(t.error.as_deref(), Some("boom"));
        assert!(matches!(
            ev(&mut t, Event::Refresh).as_slice(),
            [ToolRequest::JjLoad]
        ));
    }
    #[test]
    fn describe_richiede_testo_e_una_sola_azione() {
        let mut t = pronto();
        assert!(ev(&mut t, Event::Describe).is_empty());
        let _ = ev(&mut t, Event::Message(" msg ".into()));
        assert!(
            matches!(ev(&mut t, Event::Describe).as_slice(), [ToolRequest::JjAction(JjAction::Describe(m))] if m == " msg ")
        );
        assert!(ev(&mut t, Event::Push).is_empty());
    }
    #[test]
    fn new_trasporta_messaggio_opzionale() {
        let mut t = pronto();
        assert!(matches!(
            ev(&mut t, Event::New).as_slice(),
            [ToolRequest::JjAction(JjAction::New(None))]
        ));
    }
    #[test]
    fn azione_riuscita_ricarica_fallita_no() {
        let mut t = pronto();
        let _ = ev(&mut t, Event::Push);
        assert!(matches!(
            ev(
                &mut t,
                Event::ActionDone {
                    action: JjAction::Push,
                    ok: true
                }
            )
            .as_slice(),
            [ToolRequest::JjLoad]
        ));
        let _ = ev(&mut t, Event::Loaded(Ok(snapshot())));
        let _ = ev(&mut t, Event::Push);
        assert!(
            ev(
                &mut t,
                Event::ActionDone {
                    action: JjAction::Push,
                    ok: false
                }
            )
            .is_empty()
        );
    }
    #[test]
    fn chiusura_scarta_snapshot_e_annulla_solo_load() {
        let mut t = tool(true, true);
        let _ = t.on_visibility_changed(true);
        assert!(matches!(
            t.on_visibility_changed(false).as_slice(),
            [ToolRequest::JjCancel]
        ));
        let _ = ev(&mut t, Event::Loaded(Err("cancel".into())));
        assert!(t.error.is_none());
        let mut ready = pronto();
        assert!(ready.on_visibility_changed(false).is_empty());
        assert!(ready.snapshot.is_none());
    }
    #[test]
    fn cambio_workspace_azzera_bozza() {
        let mut t = pronto();
        let _ = ev(&mut t, Event::Message("draft".into()));
        assert!(matches!(
            ev(&mut t, Event::SetWorkspace(Some(PathBuf::from("/altro")))).as_slice(),
            [ToolRequest::JjLoad]
        ));
        assert!(t.snapshot.is_none());
        assert!(t.draft.is_empty());
    }

    #[test]
    fn la_bozza_si_svuota_solo_se_corrisponde_all_azione_inviata() {
        let mut t = pronto();
        let _ = ev(&mut t, Event::Message("uno".into()));
        let _ = ev(&mut t, Event::Describe);
        let _ = ev(&mut t, Event::Message("due".into()));
        let _ = ev(
            &mut t,
            Event::ActionDone {
                action: JjAction::Describe("uno".into()),
                ok: true,
            },
        );
        assert_eq!(t.draft, "due");
        let mut t = pronto();
        let _ = ev(&mut t, Event::Message("uno".into()));
        let _ = ev(&mut t, Event::Describe);
        let _ = ev(
            &mut t,
            Event::ActionDone {
                action: JjAction::Describe("uno".into()),
                ok: true,
            },
        );
        assert!(t.draft.is_empty());
    }

    #[test]
    fn annullamento_solo_se_ce_un_lavoro_in_corso() {
        let mut t = pronto();
        assert!(ev(&mut t, Event::Cancel).is_empty());
        let _ = ev(&mut t, Event::Refresh);
        assert!(matches!(
            ev(&mut t, Event::Cancel).as_slice(),
            [ToolRequest::JjCancel]
        ));
    }

    #[test]
    fn riapertura_rileva_di_nuovo_jj() {
        let mut t = tool(false, true);
        assert!(t.on_visibility_changed(true).is_empty());
        let _ = t.on_visibility_changed(false);
        FOUND.with(|flag| flag.set(true));
        assert!(matches!(
            t.on_visibility_changed(true).as_slice(),
            [ToolRequest::JjLoad]
        ));
    }

    #[test]
    fn rimuovere_workspace_azzera_i_controlli() {
        let mut t = pronto();
        let _ = ev(&mut t, Event::SetWorkspace(None));
        assert!(t.controls().is_empty());
        assert!(t.snapshot.is_none());
    }
}
