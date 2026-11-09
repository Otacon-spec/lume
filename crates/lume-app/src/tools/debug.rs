//! Pannello Debug: la vista «Workbench» del design TraceLine (piano 17).
//!
//! Tre colonne: a sinistra il **percorso di esecuzione** a cascata (frame
//! dall'esterno verso il punto di arresto, D-DAP-10) con la storia degli
//! arresti; al centro il passo scelto e il suo sorgente; a destra le schede
//! Input, Variabili, Output, Valuta, Breakpoint.
//!
//! Il pannello non possiede niente: disegna l'istantanea che l'app gli
//! manda ([`Event::Snapshot`]) e traduce i gesti in
//! [`ToolRequest::Debug`]. Zero-mock: un comando compare solo quando la
//! sessione può eseguirlo.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use iced::widget::{
    Space, button, column, container, pick_list, row, scrollable, stack, text, text_input,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use lume_dap::{StopRecord, Variable};
use lume_theme::LumeTheme;
use lume_ui::icons::Icon;
use lume_ui::tool::{DebugAction, Tool, ToolCtx, ToolEvent, ToolId, ToolKind, ToolRequest};
use lume_ui::{MONO, styles};

use crate::debug_mode::{Phase, ScopeDetail, Snippet};

pub const ID: ToolId = ToolId("debug");
/// Righe di console mandate alla vista (le ultime).
pub const VIEW_CONSOLE_LINES: usize = 500;
/// La colonna del flusso del codice: è il cuore del pannello, larga abbastanza
/// per leggere le righe eseguite.
pub const FLOW_COLUMN_W: f32 = 440.0;

/// Tutto ciò che il pannello disegna, preparato dall'app.
#[derive(Debug, Clone, Default)]
pub struct DebugView {
    pub phase: Option<Phase>,
    pub configs: Vec<String>,
    pub selected_config: usize,
    /// Perché non si può avviare (configurazione assente, gdb mancante...).
    pub setup_problem: Option<String>,
    pub setup_loaded: bool,
    /// Testo della conferma di fiducia, quando è aperta.
    pub trust_prompt: Option<String>,
    pub supports_pause: bool,
    pub position: Option<(usize, usize)>,
    pub at_latest: bool,
    pub stop: Option<StopRecord>,
    pub selected_frame: usize,
    pub snippet: Option<Snippet>,
    pub scopes: Vec<ScopeDetail>,
    pub children: HashMap<i64, (Vec<Variable>, bool)>,
    pub expanded: BTreeSet<i64>,
    pub detail_error: Option<String>,
    pub last_error: Option<String>,
    pub exit_code: Option<i64>,
    pub console: Vec<(Option<String>, String)>,
    pub console_total: usize,
    pub evaluations: Vec<(String, Result<String, String>)>,
    /// `(file, riga, verificato)`.
    pub breakpoints: Vec<(PathBuf, i64, Option<bool>)>,
    pub program: Option<String>,
}

impl DebugView {
    fn phase(&self) -> Phase {
        self.phase.unwrap_or(Phase::Ferma)
    }

    fn can_inspect(&self) -> bool {
        self.phase() == Phase::Fermo && self.at_latest
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Input,
    Variabili,
    Output,
    Valuta,
    Breakpoint,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Input,
        Tab::Variabili,
        Tab::Output,
        Tab::Valuta,
        Tab::Breakpoint,
    ];

    const fn label(self) -> &'static str {
        match self {
            Tab::Input => "Input",
            Tab::Variabili => "Variabili",
            Tab::Output => "Output",
            Tab::Valuta => "Valuta",
            Tab::Breakpoint => "Breakpoint",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Snapshot(Arc<DebugView>),
    Action(DebugAction),
    Tab(Tab),
    EvalInput(String),
    EvalSubmit,
    ConfigPicked(String),
}

pub fn snapshot_event(view: DebugView) -> ToolEvent {
    ToolEvent::new(Event::Snapshot(Arc::new(view)))
}

fn action(action: DebugAction) -> ToolEvent {
    ToolEvent::new(Event::Action(action))
}

#[derive(Default)]
pub struct DebugTool {
    view: Arc<DebugView>,
    tab: Tab,
    eval_input: String,
}

impl DebugTool {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Tool for DebugTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Debug"
    }

    fn icon(&self) -> Icon {
        Icon::Debug
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Dock
    }

    fn feature(&self) -> Option<&'static str> {
        Some(crate::features::DEBUGGER_KEY)
    }

    fn badge(&self) -> Option<String> {
        match self.view.phase() {
            Phase::Fermo => Some("in pausa".into()),
            Phase::InEsecuzione => Some("in esecuzione".into()),
            Phase::Avvio => Some("avvio".into()),
            Phase::Arresto | Phase::Ferma => None,
        }
    }

    fn live_count(&self) -> Option<usize> {
        Some(usize::from(self.view.phase().is_live()))
    }

    fn update(&mut self, ev: &ToolEvent) -> Vec<ToolRequest> {
        let Some(event) = ev.downcast::<Event>() else {
            return Vec::new();
        };
        match event {
            Event::Snapshot(view) => {
                self.view = Arc::clone(view);
                Vec::new()
            }
            Event::Action(action) => vec![ToolRequest::Debug(action.clone())],
            Event::Tab(tab) => {
                self.tab = *tab;
                Vec::new()
            }
            Event::EvalInput(value) => {
                self.eval_input.clone_from(value);
                Vec::new()
            }
            Event::EvalSubmit => {
                let expression = self.eval_input.trim().to_owned();
                if expression.is_empty() {
                    return Vec::new();
                }
                self.eval_input.clear();
                vec![ToolRequest::Debug(DebugAction::Evaluate(expression))]
            }
            Event::ConfigPicked(name) => self
                .view
                .configs
                .iter()
                .position(|config| config == name)
                .map(|index| vec![ToolRequest::Debug(DebugAction::SelectConfig(index))])
                .unwrap_or_default(),
        }
    }

    /// Chiudere il pannello non ferma la sessione (design §2): rilascia
    /// solo l'input di valutazione non inviato.
    fn on_visibility_changed(&mut self, open: bool) -> Vec<ToolRequest> {
        if !open {
            self.eval_input.clear();
            self.eval_input.shrink_to_fit();
            return Vec::new();
        }
        // Prima apertura (o progetto cambiato): legge le configurazioni.
        // Solo stat e una lettura del file, su gesto.
        if self.view.setup_loaded {
            Vec::new()
        } else {
            vec![ToolRequest::Debug(DebugAction::ReloadConfigs)]
        }
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        let view = &*self.view;
        let corpo: Element<'a, ToolEvent> = if view.stop.is_none() && !view.phase().is_live() {
            empty_state(t, view)
        } else {
            row![
                container(execution_path(t, view))
                    .width(FLOW_COLUMN_W)
                    .height(Length::Fill),
                vsep(t),
                container(selected_step(t, view))
                    .width(Length::FillPortion(3))
                    .height(Length::Fill),
                vsep(t),
                container(tabs(t, view, self.tab, &self.eval_input))
                    .width(Length::FillPortion(2))
                    .height(Length::Fill),
            ]
            .height(Length::Fill)
            .into()
        };
        column![toolbar(t, view), hsep(t), corpo]
            .height(Length::Fill)
            .into()
    }
}

// ── Barra dei comandi ───────────────────────────────────────────────

fn command_button<'a>(
    t: &LumeTheme,
    icon: Icon,
    label: &'a str,
    color: Color,
    on: DebugAction,
) -> Element<'a, ToolEvent> {
    button(
        row![icon.view(12.0, color), text(label).size(11.5).color(t.ui)]
            .spacing(5)
            .align_y(Alignment::Center),
    )
    .padding(Padding::from([3, 8]))
    .style(styles::hover_row(t.ui, t.hover2, None, 4.0))
    .on_press(action(on))
    .into()
}

fn text_button<'a>(
    t: &LumeTheme,
    label: &'a str,
    color: Color,
    on: Event,
) -> Element<'a, ToolEvent> {
    button(text(label).size(11.5).color(color))
        .padding(Padding::from([3, 8]))
        .style(styles::hover_row(color, t.hover2, None, 4.0))
        .on_press(ToolEvent::new(on))
        .into()
}

fn status_pill<'a>(t: &LumeTheme, view: &'a DebugView) -> Element<'a, ToolEvent> {
    let (dot, label) = match view.phase() {
        Phase::Ferma => match view.exit_code {
            Some(code) => (t.faint, format!("Terminato · codice {code}")),
            None => (t.faint, "Fermo".to_owned()),
        },
        Phase::Avvio => (t.warn, "Avvio…".to_owned()),
        Phase::InEsecuzione => (t.good, "In esecuzione".to_owned()),
        Phase::Fermo => {
            let motivo = view
                .stop
                .as_ref()
                .map(|stop| reason_label(&stop.reason).to_lowercase())
                .unwrap_or_default();
            (t.accent, format!("In pausa · {motivo}"))
        }
        Phase::Arresto => (t.warn, "Chiusura…".to_owned()),
    };
    container(
        row![
            text("●").size(9).color(dot),
            text(label).size(11).font(MONO).color(t.sec),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([2, 9]))
    .style(pill_style(t.inset, t.border2))
    .into()
}

fn toolbar<'a>(t: &LumeTheme, view: &'a DebugView) -> Element<'a, ToolEvent> {
    if let Some(prompt) = &view.trust_prompt {
        return container(
            row![
                Icon::Debug.view(14.0, t.warn),
                text(prompt.as_str())
                    .size(12)
                    .color(t.text)
                    .width(Length::Fill),
                command_button(
                    t,
                    Icon::Play,
                    "Fidati e avvia",
                    t.good,
                    DebugAction::TrustAndStart
                ),
                text_button(t, "Annulla", t.sec, Event::Action(DebugAction::CancelTrust)),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([6, 12]))
        .style({
            let fondo = t.warn_bg;
            move |_| styles::bg(fondo)
        })
        .into();
    }
    let mut barra = row![].spacing(4).align_y(Alignment::Center);
    match view.phase() {
        Phase::Ferma => {
            if !view.configs.is_empty() {
                let selected = view.configs.get(view.selected_config).cloned();
                barra = barra.push(
                    pick_list(view.configs.clone(), selected, |name| {
                        ToolEvent::new(Event::ConfigPicked(name))
                    })
                    .text_size(11.5)
                    .padding(Padding::from([2, 8])),
                );
                if view.setup_problem.is_none() {
                    barra = barra.push(command_button(
                        t,
                        Icon::Play,
                        "Avvia",
                        t.good,
                        DebugAction::Start,
                    ));
                }
            }
            barra = barra.push(command_button(
                t,
                Icon::Refresh,
                "Rileggi",
                t.icon,
                DebugAction::ReloadConfigs,
            ));
        }
        Phase::Fermo => {
            barra = barra
                .push(command_button(
                    t,
                    Icon::Play,
                    "Continua",
                    t.good,
                    DebugAction::Continue,
                ))
                .push(command_button(
                    t,
                    Icon::StepOver,
                    "Passo",
                    t.accent,
                    DebugAction::StepOver,
                ))
                .push(command_button(
                    t,
                    Icon::StepInto,
                    "Entra",
                    t.accent,
                    DebugAction::StepIn,
                ))
                .push(command_button(
                    t,
                    Icon::StepOut,
                    "Esci",
                    t.accent,
                    DebugAction::StepOut,
                ))
                .push(command_button(
                    t,
                    Icon::Stop,
                    "Ferma",
                    t.bad,
                    DebugAction::Stop,
                ));
        }
        Phase::InEsecuzione => {
            if view.supports_pause {
                barra = barra.push(command_button(
                    t,
                    Icon::Pause,
                    "Pausa",
                    t.warn,
                    DebugAction::Pause,
                ));
            }
            barra = barra.push(command_button(
                t,
                Icon::Stop,
                "Ferma",
                t.bad,
                DebugAction::Stop,
            ));
        }
        Phase::Avvio => {
            barra = barra.push(command_button(
                t,
                Icon::Stop,
                "Ferma",
                t.bad,
                DebugAction::Stop,
            ));
        }
        Phase::Arresto => {}
    }
    barra = barra
        .push(Space::new().width(10))
        .push(status_pill(t, view));
    if let Some(program) = &view.program {
        barra = barra.push(
            container(text(program.as_str()).size(11).font(MONO).color(t.muted))
                .padding(Padding::from([2, 9]))
                .style(pill_style(t.inset, t.border2)),
        );
    }
    barra = barra.push(Space::new().width(Length::Fill));
    if let Some(error) = &view.last_error {
        barra = barra.push(
            text(first_line(error))
                .size(11)
                .color(t.bad)
                .wrapping(text::Wrapping::None),
        );
    }
    container(barra)
        .padding(Padding::from([5, 10]))
        .width(Length::Fill)
        .into()
}

// ── Stato vuoto ─────────────────────────────────────────────────────

fn empty_state<'a>(t: &LumeTheme, view: &'a DebugView) -> Element<'a, ToolEvent> {
    let mut righe = column![].spacing(8).max_width(720);
    if !view.setup_loaded {
        righe = righe.push(
            text("Lettura di .lume/launch.toml…")
                .size(12)
                .color(t.muted),
        );
    } else if let Some(problem) = &view.setup_problem {
        righe = righe.push(text(problem.as_str()).size(12.5).color(t.text));
        righe = righe.push(
            text("Esempio di .lume/launch.toml:")
                .size(11.5)
                .color(t.muted),
        );
        righe = righe.push(
            container(
                text(
                    "[[configurazione]]\nnome = \"prova\"\nadattatore = \"gdb\"\nprogramma = \"${workspaceFolder}/target/debug/prova\"\nargomenti = []",
                )
                .size(11.5)
                .font(MONO)
                .color(t.sec),
            )
            .padding(10)
            .style(pill_style(t.inset, t.border2)),
        );
    } else {
        righe = righe.push(
            text("Scegli una configurazione e premi Avvia. Il percorso di esecuzione comparirà qui al primo arresto.")
                .size(12.5)
                .color(t.text),
        );
    }
    righe = righe.push(
        text("I breakpoint si mettono con un clic sul numero di riga o con F9 sulla riga del cursore.")
            .size(11.5)
            .color(t.muted),
    );
    container(righe)
        .padding(Padding::from([16, 18]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

// ── Percorso di esecuzione ──────────────────────────────────────────

fn reason_label(reason: &str) -> &'static str {
    match reason {
        "breakpoint" => "Breakpoint",
        "step" => "Passo",
        "exception" => "Eccezione",
        "signal" => "Segnale",
        "pause" => "Pausa",
        "entry" => "Ingresso",
        "goto" => "Salto",
        "function breakpoint" => "Breakpoint",
        "data breakpoint" => "Watchpoint",
        _ => "Arresto",
    }
}

fn execution_path<'a>(t: &LumeTheme, view: &'a DebugView) -> Element<'a, ToolEvent> {
    let mut intestazione = row![
        text("PERCORSO DI ESECUZIONE")
            .size(10.5)
            .font(MONO)
            .color(t.muted),
        Space::new().width(Length::Fill),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    if let Some((posizione, totale)) = view.position {
        intestazione = intestazione
            .push(nav_button(
                t,
                "‹",
                posizione > 1,
                DebugAction::HistoryPrevious,
            ))
            .push(
                text(format!("{posizione} / {totale}"))
                    .size(10.5)
                    .font(MONO)
                    .color(t.sec),
            )
            .push(nav_button(
                t,
                "›",
                posizione < totale,
                DebugAction::HistoryNext,
            ));
        if !view.at_latest {
            intestazione = intestazione.push(nav_button(t, "»", true, DebugAction::HistoryLatest));
        }
    }
    let mut testa = column![intestazione].spacing(3);
    if let Some(stop) = &view.stop {
        let mut sotto = format!("arresto #{} · thread {}", stop.number, stop.thread_id);
        if let Some(description) = &stop.description {
            sotto.push_str(" · ");
            sotto.push_str(description);
        }
        testa = testa.push(text(sotto).size(10.5).font(MONO).color(t.faint));
    }
    let mut elenco = column![].spacing(0);
    if let Some(stop) = &view.stop {
        let ultimo = stop.frames.len().saturating_sub(1);
        for (indice, frame) in stop.cascade() {
            let selezionato = indice == view.selected_frame;
            let (tipo, colore_tipo) = if indice == 0 {
                if stop.is_exception() {
                    ("ECCEZIONE".to_owned(), t.bad)
                } else {
                    (reason_label(&stop.reason).to_uppercase(), t.accent)
                }
            } else {
                ("CHIAMATA".to_owned(), t.faint)
            };
            let punto = if selezionato {
                t.accent
            } else if indice == 0 && stop.is_exception() {
                t.bad
            } else if indice == 0 {
                t.warn
            } else {
                t.faint
            };
            let file = frame
                .file_label()
                .unwrap_or_else(|| "senza sorgente".to_owned());
            let testo = column![
                row![
                    text(tipo).size(9.5).font(MONO).color(colore_tipo),
                    Space::new().width(Length::Fill),
                    text(file).size(9.5).font(MONO).color(t.faint),
                ]
                .align_y(Alignment::Center),
                row![
                    text(frame.name.as_str())
                        .size(12)
                        .font(MONO)
                        .color(t.func)
                        .wrapping(text::Wrapping::None),
                    text(format!(":{}", frame.line))
                        .size(11)
                        .font(MONO)
                        .color(t.muted),
                ]
                .spacing(2),
                flow_code(t, frame),
            ]
            .spacing(2)
            .width(Length::Fill);
            // La cascata: ogni chiamata rientra un po' rispetto a chi l'ha
            // fatta, così il codice eseguito si legge dall'alto in basso.
            let profondita = u16::try_from(ultimo.saturating_sub(indice)).unwrap_or(u16::MAX);
            let rientro = (f32::from(profondita) * FLOW_INDENT).min(FLOW_MAX_INDENT);
            let altezza = 36.0 + FLOW_LINE_H * frame.code.len() as f32;
            let connettore = connector(
                t,
                punto,
                indice == ultimo,
                indice == 0,
                selezionato,
                altezza,
            );
            let voce = row![connettore, Space::new().width(8.0 + rientro), testo]
                .padding(Padding::from([5, 10]));
            let sfondo = selezionato.then_some(t.sel);
            elenco = elenco.push(
                button(voce)
                    .width(Length::Fill)
                    .padding(0)
                    .style(styles::hover_row(t.text, t.hover, sfondo, 0.0))
                    .on_press(action(DebugAction::SelectFrame(indice))),
            );
        }
    }
    column![
        container(testa).padding(Padding::from([8, 12])),
        hsep(t),
        // Ancorata in fondo: il punto di arresto, il più profondo, è sempre
        // in vista; si risale per vedere chi ha chiamato.
        scrollable(elenco).height(Length::Fill).anchor_bottom(),
    ]
    .into()
}

/// Rientro per livello di chiamata nella cascata, e massimo.
const FLOW_INDENT: f32 = 12.0;
const FLOW_MAX_INDENT: f32 = 96.0;
/// Altezza di una riga di codice nel flusso.
const FLOW_LINE_H: f32 = 17.0;

/// Le righe di codice che il frame stava eseguendo: la riga del frame in
/// evidenza, il contorno attenuato.
fn flow_code<'a>(t: &LumeTheme, frame: &'a lume_dap::FrameSummary) -> Element<'a, ToolEvent> {
    let mut righe = column![].spacing(0);
    for (numero, testo) in &frame.code {
        let eseguita = *numero == frame.line;
        let riga = row![
            text(numero.to_string())
                .size(10)
                .font(MONO)
                .color(if eseguita { t.accent } else { t.faint })
                .width(28)
                .align_x(iced::alignment::Horizontal::Right),
            text(testo.trim_end())
                .size(11)
                .font(MONO)
                .color(if eseguita { t.text } else { t.muted })
                .wrapping(text::Wrapping::None),
        ]
        .spacing(8)
        .height(FLOW_LINE_H)
        .align_y(Alignment::Center);
        let fondo = eseguita.then_some(current_line_bg(t));
        righe = righe.push(
            container(riga)
                .width(Length::Fill)
                .clip(true)
                .style(move |_| container::Style {
                    background: fondo.map(Background::Color),
                    ..container::Style::default()
                }),
        );
    }
    righe.into()
}

/// Il pallino e il filo verticale che legano i frame della cascata.
fn connector<'a>(
    t: &LumeTheme,
    dot: Color,
    first: bool,
    last: bool,
    selected: bool,
    height: f32,
) -> Element<'a, ToolEvent> {
    let linea = t.border2;
    let filo = column![
        container(Space::new().width(1).height(Length::Fill))
            .style(move |_| { styles::bg(if first { Color::TRANSPARENT } else { linea }) }),
        container(Space::new().width(1).height(Length::Fill))
            .style(move |_| { styles::bg(if last { Color::TRANSPARENT } else { linea }) }),
    ]
    .height(Length::Fill)
    .align_x(Alignment::Center)
    .width(14);
    let lato = if selected { 11.0 } else { 9.0 };
    let alone = if selected { t.sel } else { Color::TRANSPARENT };
    let pallino =
        container(Space::new().width(lato).height(lato)).style(move |_| container::Style {
            background: Some(Background::Color(dot)),
            border: Border {
                radius: 99.0.into(),
                width: if selected { 3.0 } else { 0.0 },
                color: alone,
            },
            ..container::Style::default()
        });
    container(stack![
        filo,
        container(pallino)
            .width(14)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    ])
    .width(14)
    .height(height)
    .into()
}

fn nav_button<'a>(
    t: &LumeTheme,
    glyph: &'a str,
    enabled: bool,
    on: DebugAction,
) -> Element<'a, ToolEvent> {
    let colore = if enabled { t.sec } else { t.faint };
    let mut pulsante = button(text(glyph).size(13).color(colore))
        .padding(Padding::from([0, 6]))
        .style(styles::hover_row(colore, t.hover2, None, 4.0));
    if enabled {
        pulsante = pulsante.on_press(action(on));
    }
    pulsante.into()
}

// ── Passo scelto ────────────────────────────────────────────────────

fn selected_step<'a>(t: &LumeTheme, view: &'a DebugView) -> Element<'a, ToolEvent> {
    let Some(stop) = &view.stop else {
        let messaggio = match view.phase() {
            Phase::Avvio => "Avvio di gdb e del programma…",
            Phase::InEsecuzione => {
                "Il programma è in esecuzione. Si fermerà al prossimo breakpoint."
            }
            _ => "Nessun arresto.",
        };
        return container(text(messaggio).size(12).color(t.muted))
            .padding(16)
            .into();
    };
    let frame = stop.frames.get(view.selected_frame);
    let (posizione, totale) = view.position.unwrap_or((1, 1));
    let mut testa = row![
        container(
            text(format!("PASSO SCELTO · {posizione} / {totale}"))
                .size(10)
                .font(MONO)
                .color(t.accent),
        )
        .padding(Padding::from([1, 7]))
        .style(pill_style(t.sel, Color::TRANSPARENT)),
        text(format!(
            "frame {} di {}",
            stop.frames.len().saturating_sub(view.selected_frame),
            stop.frames.len()
        ))
        .size(11)
        .font(MONO)
        .color(t.faint),
        Space::new().width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if let Some(path) = frame.and_then(|frame| frame.path.clone()) {
        let line = frame
            .and_then(|frame| u32::try_from(frame.line).ok())
            .unwrap_or(1);
        testa = testa.push(command_button(
            t,
            Icon::Project,
            "Apri nell'editor",
            t.icon,
            DebugAction::OpenSource { path, line },
        ));
    }
    let titolo = match frame {
        Some(frame) => row![
            text(frame.name.as_str()).size(16).font(MONO).color(t.func),
            text(format!(" · riga {}", frame.line))
                .size(13)
                .font(MONO)
                .color(t.muted),
        ]
        .align_y(Alignment::Center),
        None => row![text("frame non disponibile").size(13).color(t.muted)],
    };
    let codice: Element<'a, ToolEvent> = match &view.snippet {
        Some(snippet) => snippet_view(t, view, snippet),
        None => container(
            column![
                text("Sorgente non disponibile").size(12.5).color(t.text),
                text(
                    "Il frame non ha un file leggibile su disco (libreria senza sorgenti o file oltre 8 MiB). Puoi continuare a fare passi: i frame senza sorgente restano nella cascata.",
                )
                .size(11.5)
                .color(t.muted),
            ]
            .spacing(6),
        )
        .padding(14)
        .into(),
    };
    column![
        container(column![testa, titolo].spacing(6)).padding(Padding::from([8, 14])),
        hsep(t),
        container(scrollable(codice).height(Length::Fill))
            .height(Length::Fill)
            .style({
                let fondo = t.inset;
                move |_| styles::bg(fondo)
            }),
    ]
    .into()
}

fn snippet_view<'a>(
    t: &LumeTheme,
    view: &'a DebugView,
    snippet: &'a Snippet,
) -> Element<'a, ToolEvent> {
    let breakpoints: BTreeSet<i64> = view
        .breakpoints
        .iter()
        .filter(|(path, _, _)| *path == snippet.path)
        .map(|(_, line, _)| *line)
        .collect();
    let mut righe = column![].spacing(0);
    for (offset, testo) in snippet.lines.iter().enumerate() {
        let numero = snippet.first_line + i64::try_from(offset).unwrap_or(0);
        let corrente = numero == snippet.line;
        let marcatore: Element<'a, ToolEvent> = if corrente {
            text("▶").size(10).color(t.good).into()
        } else if breakpoints.contains(&numero) {
            text("●").size(10).color(t.bad).into()
        } else {
            Space::new().width(10).into()
        };
        let riga = row![
            container(marcatore).width(16).align_x(Alignment::Center),
            text(numero.to_string())
                .size(11.5)
                .font(MONO)
                .color(if corrente { t.accent } else { t.faint })
                .width(40)
                .align_x(iced::alignment::Horizontal::Right),
            Space::new().width(12),
            text(testo.as_str())
                .size(12)
                .font(MONO)
                .color(if corrente { t.text } else { t.sec })
                .wrapping(text::Wrapping::None),
        ]
        .align_y(Alignment::Center)
        .height(20);
        let sfondo = corrente.then_some(current_line_bg(t));
        let bordo = if corrente { t.good } else { Color::TRANSPARENT };
        righe = righe.push(
            container(riga)
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: sfondo.map(Background::Color),
                    border: Border {
                        color: bordo,
                        width: if sfondo.is_some() { 1.0 } else { 0.0 },
                        radius: 0.0.into(),
                    },
                    ..container::Style::default()
                }),
        );
    }
    container(righe).padding(Padding::from([6, 0])).into()
}

/// Lo sfondo della riga corrente, come nel mockup: verde tenue.
pub fn current_line_bg(t: &LumeTheme) -> Color {
    Color { a: 0.14, ..t.good }
}

// ── Schede ──────────────────────────────────────────────────────────

fn tabs<'a>(
    t: &LumeTheme,
    view: &'a DebugView,
    tab: Tab,
    eval_input: &'a str,
) -> Element<'a, ToolEvent> {
    let mut intestazioni = row![].spacing(0);
    for candidata in Tab::ALL {
        let attiva = candidata == tab;
        let mut etichetta = candidata.label().to_owned();
        if candidata == Tab::Breakpoint && !view.breakpoints.is_empty() {
            etichetta = format!("{etichetta} {}", view.breakpoints.len());
        }
        let colore = if attiva { t.text } else { t.muted };
        let sottolineatura = if attiva { t.accent } else { Color::TRANSPARENT };
        intestazioni = intestazioni.push(
            button(
                column![
                    text(etichetta).size(11.5).color(colore),
                    container(Space::new().width(Length::Fill).height(2))
                        .style(move |_| styles::bg(sottolineatura)),
                ]
                .spacing(6)
                .width(Length::Shrink),
            )
            .padding(Padding {
                top: 7.0,
                right: 10.0,
                bottom: 0.0,
                left: 10.0,
            })
            .style(styles::hover_row(colore, t.hover, None, 0.0))
            .on_press(ToolEvent::new(Event::Tab(candidata))),
        );
    }
    let contenuto: Element<'a, ToolEvent> = match tab {
        Tab::Input => scopes_view(t, view, true),
        Tab::Variabili => scopes_view(t, view, false),
        Tab::Output => console_view(t, view),
        Tab::Valuta => evaluate_view(t, view, eval_input),
        Tab::Breakpoint => breakpoints_view(t, view),
    };
    column![
        container(intestazioni).padding(Padding::from([0, 6])),
        hsep(t),
        container(contenuto)
            .height(Length::Fill)
            .width(Length::Fill),
    ]
    .into()
}

fn not_live_note<'a>(t: &LumeTheme, view: &'a DebugView) -> Option<Element<'a, ToolEvent>> {
    if view.can_inspect() {
        return None;
    }
    let mut note = column![].spacing(8).padding(14);
    if view.phase() == Phase::Fermo {
        note = note
            .push(
                text("Le variabili si leggono solo sull'arresto corrente: questo è un arresto passato.")
                    .size(11.5)
                    .color(t.muted),
            )
            .push(text_button(t, "Vai all'ultimo arresto", t.accent, Event::Action(DebugAction::HistoryLatest)));
    } else {
        note = note.push(
            text("Le variabili compaiono quando il programma è in pausa.")
                .size(11.5)
                .color(t.muted),
        );
    }
    Some(note.into())
}

fn scopes_view<'a>(t: &LumeTheme, view: &'a DebugView, arguments: bool) -> Element<'a, ToolEvent> {
    if let Some(note) = not_live_note(t, view) {
        return note;
    }
    let mut elenco = column![].spacing(4).padding(Padding::from([8, 10]));
    let mut vuoto = true;
    for detail in &view.scopes {
        let is_arguments = detail.scope.presentation_hint.as_deref() == Some("arguments")
            || detail.scope.name.eq_ignore_ascii_case("arguments");
        if is_arguments != arguments {
            continue;
        }
        if !arguments {
            elenco = elenco.push(
                text(detail.scope.name.to_uppercase())
                    .size(10)
                    .font(MONO)
                    .color(t.faint),
            );
        }
        if detail.scope.expensive {
            elenco = elenco.push(text("non letto (costoso)").size(11).color(t.faint));
        }
        for variable in &detail.variables {
            vuoto = false;
            elenco = variable_rows(t, view, variable, 0, elenco);
        }
        if detail.truncated {
            elenco = elenco.push(
                text(format!("… troncate a {}", lume_dap::session::MAX_VARIABLES))
                    .size(11)
                    .color(t.warn),
            );
        }
    }
    if let Some(error) = &view.detail_error {
        elenco = elenco.push(text(error.as_str()).size(11.5).color(t.bad));
    }
    if vuoto && view.detail_error.is_none() {
        let messaggio = if arguments {
            "Questo frame non ha argomenti."
        } else {
            "Nessuna variabile locale."
        };
        elenco = elenco.push(text(messaggio).size(11.5).color(t.muted));
    }
    scrollable(elenco).height(Length::Fill).into()
}

fn variable_rows<'a>(
    t: &LumeTheme,
    view: &'a DebugView,
    variable: &'a Variable,
    depth: u16,
    mut elenco: iced::widget::Column<'a, ToolEvent>,
) -> iced::widget::Column<'a, ToolEvent> {
    let espandibile = variable.variables_reference > 0;
    let aperta = view.expanded.contains(&variable.variables_reference);
    let freccia = if !espandibile {
        " "
    } else if aperta {
        "▾"
    } else {
        "▸"
    };
    let mut contenuto = row![
        Space::new().width(f32::from(depth) * 14.0),
        text(freccia).size(10).color(t.faint).width(10),
        text(variable.name.as_str())
            .size(11.5)
            .font(MONO)
            .color(t.muted)
            .width(Length::FillPortion(2)),
        text(variable.value.as_str())
            .size(12)
            .font(MONO)
            .color(t.text)
            .width(Length::FillPortion(3))
            .wrapping(text::Wrapping::None),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if let Some(tipo) = &variable.type_name {
        contenuto = contenuto.push(text(tipo.as_str()).size(10).font(MONO).color(t.faint));
    }
    let riga = container(contenuto)
        .padding(Padding::from([5, 10]))
        .width(Length::Fill)
        .style(pill_style(t.inset, t.border));
    elenco = if espandibile {
        elenco.push(
            button(riga)
                .padding(0)
                .width(Length::Fill)
                .style(styles::hover_row(t.text, t.hover, None, 5.0))
                .on_press(action(DebugAction::ToggleVariable(
                    variable.variables_reference,
                ))),
        )
    } else {
        elenco.push(riga)
    };
    if aperta && let Some((figli, troncati)) = view.children.get(&variable.variables_reference) {
        for figlio in figli {
            elenco = variable_rows(t, view, figlio, depth.saturating_add(1), elenco);
        }
        if *troncati {
            elenco = elenco.push(text("… troncate").size(11).color(t.warn));
        }
    }
    elenco
}

fn console_view<'a>(t: &LumeTheme, view: &'a DebugView) -> Element<'a, ToolEvent> {
    let mut righe = column![].spacing(1).padding(Padding::from([8, 10]));
    if view.console.is_empty() {
        righe = righe.push(text("Nessun output.").size(11.5).color(t.muted));
    }
    for (categoria, riga) in &view.console {
        let colore = match categoria.as_deref() {
            Some("stderr") => t.bad,
            Some("console" | "important") => t.muted,
            _ => t.text,
        };
        righe = righe.push(
            text(riga.as_str())
                .size(11.5)
                .font(MONO)
                .color(colore)
                .wrapping(text::Wrapping::None),
        );
    }
    let mut piede = row![
        text(format!("{} righe", view.console_total))
            .size(10.5)
            .color(t.faint),
        Space::new().width(Length::Fill),
    ]
    .align_y(Alignment::Center);
    if view.console_total > 0 {
        piede = piede.push(text_button(
            t,
            "Svuota",
            t.sec,
            Event::Action(DebugAction::ClearConsole),
        ));
    }
    column![
        scrollable(righe).height(Length::Fill).anchor_bottom(),
        container(piede).padding(Padding::from([2, 10])),
    ]
    .into()
}

fn evaluate_view<'a>(
    t: &LumeTheme,
    view: &'a DebugView,
    eval_input: &'a str,
) -> Element<'a, ToolEvent> {
    let mut contenuto = column![].spacing(6).padding(Padding::from([8, 10]));
    if view.can_inspect() {
        contenuto = contenuto.push(
            text_input("Espressione nel frame scelto, poi Invio", eval_input)
                .on_input(|value| ToolEvent::new(Event::EvalInput(value)))
                .on_submit(ToolEvent::new(Event::EvalSubmit))
                .size(12)
                .font(MONO)
                .padding(Padding::from([5, 8]))
                .style(styles::input(t)),
        );
        contenuto = contenuto.push(
            text("Una valutazione può modificare il programma (assegnazioni, chiamate).")
                .size(10.5)
                .color(t.faint),
        );
    } else if let Some(note) = not_live_note(t, view) {
        contenuto = contenuto.push(note);
    }
    for (espressione, esito) in view.evaluations.iter().rev() {
        let (valore, colore) = match esito {
            Ok(valore) => (valore.as_str(), t.text),
            Err(errore) => (errore.as_str(), t.bad),
        };
        contenuto = contenuto.push(
            row![
                text(espressione.as_str())
                    .size(11.5)
                    .font(MONO)
                    .color(t.muted),
                text("=").size(11.5).font(MONO).color(t.faint),
                text(valore).size(12).font(MONO).color(colore),
            ]
            .spacing(8),
        );
    }
    scrollable(contenuto).height(Length::Fill).into()
}

fn breakpoints_view<'a>(t: &LumeTheme, view: &'a DebugView) -> Element<'a, ToolEvent> {
    let mut elenco = column![].spacing(2).padding(Padding::from([8, 10]));
    if view.breakpoints.is_empty() {
        elenco = elenco.push(
            text("Nessun breakpoint. Clic sul numero di riga, o F9, per metterne uno.")
                .size(11.5)
                .color(t.muted),
        );
    }
    for (path, line, verified) in &view.breakpoints {
        let (simbolo, colore, stato) = match verified {
            Some(true) => ("●", t.bad, "verificato"),
            Some(false) => ("○", t.warn, "in attesa"),
            None => ("●", t.faint, "non inviato"),
        };
        let nome = path
            .file_name()
            .map(|nome| nome.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let line_u32 = u32::try_from(*line).unwrap_or(1);
        let apri = button(
            row![
                text(simbolo).size(10).color(colore),
                text(format!("{nome}:{line}"))
                    .size(12)
                    .font(MONO)
                    .color(t.text),
                text(stato).size(10.5).color(t.faint),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding(Padding::from([3, 6]))
        .style(styles::hover_row(t.text, t.hover, None, 4.0))
        .on_press(action(DebugAction::OpenSource {
            path: path.clone(),
            line: line_u32,
        }));
        let togli = button(text("×").size(12).color(t.sec))
            .padding(Padding::from([1, 6]))
            .style(styles::hover_row(t.sec, t.hover2, None, 4.0))
            .on_press(action(DebugAction::RemoveBreakpoint {
                path: path.clone(),
                line: line_u32,
            }));
        elenco = elenco.push(row![apri, togli].align_y(Alignment::Center));
    }
    scrollable(elenco).height(Length::Fill).into()
}

// ── Piccoli pezzi ───────────────────────────────────────────────────

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}

fn pill_style(background: Color, border: Color) -> impl Fn(&iced::Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(background)),
        border: Border {
            radius: 5.0.into(),
            width: 1.0,
            color: border,
        },
        ..container::Style::default()
    }
}

fn hsep<'a>(t: &LumeTheme) -> Element<'a, ToolEvent> {
    let color = t.border;
    container(Space::new().width(Length::Fill).height(1))
        .style(move |_| styles::bg(color))
        .into()
}

fn vsep<'a>(t: &LumeTheme) -> Element<'a, ToolEvent> {
    let color = t.border;
    container(Space::new().width(1).height(Length::Fill))
        .style(move |_| styles::bg(color))
        .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn sola_azione(richieste: Vec<ToolRequest>) -> Option<DebugAction> {
        match richieste.as_slice() {
            [ToolRequest::Debug(azione)] => Some(azione.clone()),
            _ => None,
        }
    }

    #[test]
    fn i_gesti_diventano_richieste_e_la_valutazione_vuota_no() {
        let mut tool = DebugTool::new();
        assert_eq!(
            sola_azione(tool.update(&action(DebugAction::StepOver))),
            Some(DebugAction::StepOver)
        );
        assert!(tool.update(&ToolEvent::new(Event::EvalSubmit)).is_empty());
        tool.update(&ToolEvent::new(Event::EvalInput(" x + 1 ".into())));
        assert_eq!(
            sola_azione(tool.update(&ToolEvent::new(Event::EvalSubmit))),
            Some(DebugAction::Evaluate("x + 1".into()))
        );
    }

    #[test]
    fn la_prima_apertura_legge_le_configurazioni_e_la_chiusura_no() {
        let mut tool = DebugTool::new();
        assert_eq!(
            sola_azione(tool.on_visibility_changed(true)),
            Some(DebugAction::ReloadConfigs)
        );
        assert!(tool.on_visibility_changed(false).is_empty());
        tool.update(&snapshot_event(DebugView {
            setup_loaded: true,
            ..DebugView::default()
        }));
        assert!(tool.on_visibility_changed(true).is_empty());
    }

    #[test]
    fn badge_e_conteggio_seguono_la_fase() {
        let mut tool = DebugTool::new();
        assert_eq!(tool.badge(), None);
        assert_eq!(tool.live_count(), Some(0));
        tool.update(&snapshot_event(DebugView {
            phase: Some(Phase::Fermo),
            ..DebugView::default()
        }));
        assert_eq!(tool.badge().as_deref(), Some("in pausa"));
        assert_eq!(tool.live_count(), Some(1));
    }

    #[test]
    fn la_configurazione_scelta_per_nome_diventa_un_indice() {
        let mut tool = DebugTool::new();
        tool.update(&snapshot_event(DebugView {
            configs: vec!["a".into(), "b".into()],
            ..DebugView::default()
        }));
        assert_eq!(
            sola_azione(tool.update(&ToolEvent::new(Event::ConfigPicked("b".into())))),
            Some(DebugAction::SelectConfig(1))
        );
    }
}
