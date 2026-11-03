//! Tool «Database»: albero connessioni → tabelle → colonne del prototipo v3.
//!
//! Come Segnalibri e Cronologia, l'albero **appartiene all'app**: espandere un
//! nodo non è solo stato di vista, richiede di interrogare lo schema, e il
//! `Registry` espone solo `&dyn Tool` (niente downcast). Il tool disegna e
//! chiede via `ToolRequest`; chi possiede socket e cache è `Lume`, così la
//! chiusura del pannello può davvero liberarli (vedi `sql_is_open`).

use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Element, Length, Padding};
use lume_ui::icons::Icon;
use lume_ui::styles;
use lume_ui::tool::{Tool, ToolCtx, ToolEvent, ToolId, ToolRequest};

pub const ID: ToolId = ToolId("db");

/// Testo dell'avviso sotto una connessione non cifrata. Dice cosa si rischia,
/// non solo «attenzione»: la password e i dati viaggiano senza protezione.
pub(crate) const AVVISO_IN_CHIARO: &str =
    "connessione non cifrata: password e dati viaggiano in chiaro";

fn avviso(connection: &ConnectionView) -> Option<&'static str> {
    connection.cleartext.then_some(AVVISO_IN_CHIARO)
}

/// Stato di una connessione, per il pallino di stato della riga.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    /// Nessun socket: è lo stato normale a pannello chiuso e da freddo.
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnView {
    pub name: String,
    pub type_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableView {
    pub name: String,
    pub expanded: bool,
    /// Vuoto finché lo schema non è stato caricato per questa tabella.
    pub columns: Vec<ColumnView>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionView {
    pub id: String,
    /// Etichetta del prototipo: «azure · prod».
    pub label: String,
    /// Motore leggibile: «SQL Server», «Oracle».
    pub engine: String,
    pub host: String,
    pub expanded: bool,
    pub state: ConnectionState,
    pub tables: Vec<TableView>,
    /// Questa connessione sta aspettando la password: sotto la riga compare
    /// il campo. Uno solo per volta, perché a chiederla è l'app.
    pub asking_secret: bool,
    /// Cosa chiedere: una password SQL o il client secret del service
    /// principal. Sono due cose diverse e l'utente le tiene in due posti
    /// diversi.
    pub secret_label: String,
    /// Connessione Oracle senza TLS (`tcp://` scritto dall'utente): la riga
    /// mostra un avviso rosso. Lo calcola l'app con `lume_sql::oracle::is_cleartext`.
    pub cleartext: bool,
}

#[derive(Clone, Debug)]
pub enum Event {
    Replace(Vec<ConnectionView>),
    ToggleConnection(usize),
    ToggleTable(usize, usize),
    /// Doppio clic su una tabella: apre la console con un SELECT pronto.
    OpenTable(usize, usize),
    /// Bottone «Nuova console di query» della barra del pannello.
    OpenConsole,
    NewConnection,
    ImportIdea,
    ConfirmIdea,
    IdeaUser(usize, String),
    CancelIdea,
    IdeaPreview {
        profiles: Vec<(String, String)>,
        skipped: Vec<String>,
    },
    RefreshSchema,
    /// Battuta nel campo della password. Resta **dentro il tool**: solo la
    /// conferma la manda all'app. Il valore è un `Redacted`: il `Debug`
    /// derivato di `Event` non può stamparlo.
    SecretTyped(lume_ui::tool::Redacted),
    SecretConfirmed,
    SecretCancelled,
    /// Campo del modulo «nuova origine dati».
    DraftTyped(DraftField, String),
    DraftToggledOracle,
    DraftToggledServicePrincipal,
    DraftConfirmed,
    DraftCancelled,
    DeleteConnection(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftField {
    Label,
    Host,
    Database,
    User,
    TenantId,
    ClientId,
}

struct IdeaPreview {
    profiles: Vec<(String, String)>,
    skipped: Vec<String>,
}

pub struct DbTool {
    connections: Vec<ConnectionView>,
    idea_preview: Option<IdeaPreview>,
    /// Password in corso di digitazione. `iced::widget::text_input` vuole un
    /// `&str` vivo a ogni frame, quindi la bozza esiste in chiaro finché si
    /// digita; sta in un `Redacted`, che la azzera a `Drop`, e si svuota
    /// appena confermata, annullata o quando il pannello si chiude. Non viene
    /// mai salvata. `iced` tiene comunque una copia propria dentro il widget:
    /// vedi «Limiti noti» del piano.
    secret_draft: lume_ui::tool::Redacted,
    /// Modulo «nuova origine dati» aperto. `None` = chiuso.
    draft: Option<ToolSqlDraft>,
}

/// Alias locale per non ripetere il percorso: è lo stesso tipo che viaggia
/// nella `ToolRequest`, così la conferma non deve ricopiarne i campi.
type ToolSqlDraft = lume_ui::tool::SqlConnectionDraft;

impl DbTool {
    pub fn new() -> Self {
        Self {
            connections: Vec::new(),
            idea_preview: None,
            secret_draft: lume_ui::tool::Redacted::default(),
            draft: None,
        }
    }
}

impl Default for DbTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for DbTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Database"
    }

    fn icon(&self) -> Icon {
        Icon::Db
    }

    fn feature(&self) -> Option<&'static str> {
        Some("database")
    }

    /// Chiuso il pannello, la password digitata a metà non deve restare in
    /// memoria: al riaprirlo il campo è vuoto e si riscrive.
    fn on_visibility_changed(&mut self, open: bool) -> Vec<ToolRequest> {
        if !open {
            self.secret_draft = lume_ui::tool::Redacted::default();
        }
        Vec::new()
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        let toolbar = row![
            tool_button("+", t.icon, t.hover, Event::NewConnection),
            tool_button("▶", t.icon, t.hover, Event::OpenConsole),
            tool_button("⟳", t.icon, t.hover, Event::RefreshSchema),
            tool_button("Importa da .idea", t.icon, t.hover, Event::ImportIdea),
        ]
        .spacing(2)
        .padding(Padding::from([2.0, 10.0]))
        .align_y(Alignment::Center);

        let mut toolbar = column![toolbar];
        if let Some(IdeaPreview { profiles, skipped }) = &self.idea_preview {
            let mut preview = column![
                text(format!(
                    ".idea: {} profili importabili, {} esclusi",
                    profiles.len(),
                    skipped.len()
                ))
                .size(12)
                .color(t.sec)
            ]
            .spacing(5);
            for (index, (label, user)) in profiles.iter().enumerate() {
                preview = preview.push(text(label).size(11).color(t.sec)).push(
                    iced::widget::text_input("Utente database (obbligatorio)", user)
                        .size(12)
                        .on_input(move |user| ToolEvent::new(Event::IdeaUser(index, user))),
                );
            }
            for reason in skipped {
                preview = preview.push(text(reason).size(11).color(t.warn));
            }
            toolbar = toolbar.push(scrollable(preview).height(Length::Fixed(220.0)));
            let ready =
                !profiles.is_empty() && profiles.iter().all(|(_, user)| !user.trim().is_empty());
            toolbar = toolbar.push(
                row![
                    button(text("Importa profili").size(12))
                        .on_press_maybe(ready.then_some(ToolEvent::new(Event::ConfirmIdea))),
                    button(text("Annulla").size(12)).on_press(ToolEvent::new(Event::CancelIdea)),
                ]
                .spacing(6),
            );
        }

        if self.connections.is_empty() && self.draft.is_none() {
            return column![
                toolbar,
                container(
                    text("Nessuna origine dati. «+» ne aggiunge una.")
                        .size(12)
                        .color(t.muted),
                )
                .padding(Padding::from([2.0, 12.0])),
            ]
            .into();
        }

        let mut list = column![].spacing(1);
        if let Some(draft) = &self.draft {
            list = list.push(draft_form(t, draft));
        }
        for (ci, connection) in self.connections.iter().enumerate() {
            list = list.push(connection_row(t, ci, connection));
            if let Some(testo) = avviso(connection) {
                list = list.push(avviso_row(t, testo));
            }
            if connection.asking_secret {
                list = list.push(secret_prompt(t, connection, self.secret_draft.as_str()));
            }
            if !connection.expanded {
                continue;
            }
            if connection.tables.is_empty() {
                list = list.push(leaf_row(t, 40.0, "nessuna tabella", "", t.muted));
                continue;
            }
            for (ti, table) in connection.tables.iter().enumerate() {
                list = list.push(table_row(t, ci, ti, table));
                if !table.expanded {
                    continue;
                }
                for column_view in &table.columns {
                    list = list.push(leaf_row(
                        t,
                        58.0,
                        &column_view.name,
                        &column_view.type_name,
                        t.sec,
                    ));
                }
            }
        }

        column![
            toolbar,
            scrollable(container(list).width(Length::Fill).padding(pad_list()))
                .height(Length::Fill),
        ]
        .into()
    }

    fn update(&mut self, event: &ToolEvent) -> Vec<ToolRequest> {
        match event.downcast::<Event>() {
            Some(Event::Replace(connections)) => {
                self.connections = connections.clone();
                Vec::new()
            }
            Some(Event::ToggleConnection(index)) => {
                vec![ToolRequest::SqlToggleConnection(*index)]
            }
            Some(Event::ToggleTable(connection, table)) => {
                vec![ToolRequest::SqlToggleTable(*connection, *table)]
            }
            Some(Event::OpenTable(connection, table)) => self
                .connections
                .get(*connection)
                .and_then(|c| c.tables.get(*table).map(|t| (c, t)))
                .map(|(c, t)| ToolRequest::SqlOpenConsole {
                    connection: Some(c.id.clone()),
                    table: Some(t.name.clone()),
                })
                .into_iter()
                .collect(),
            Some(Event::OpenConsole) => vec![ToolRequest::SqlOpenConsole {
                connection: None,
                table: None,
            }],
            Some(Event::ImportIdea) => vec![ToolRequest::SqlImportIdea],
            Some(Event::ConfirmIdea) => vec![ToolRequest::SqlConfirmIdea],
            Some(Event::CancelIdea) => vec![ToolRequest::SqlCancelIdea],
            Some(Event::IdeaUser(index, user)) => vec![ToolRequest::SqlIdeaUser {
                index: *index,
                user: user.clone(),
            }],
            Some(Event::IdeaPreview { profiles, skipped }) => {
                self.idea_preview =
                    (!profiles.is_empty() || !skipped.is_empty()).then(|| IdeaPreview {
                        profiles: profiles.clone(),
                        skipped: skipped.clone(),
                    });
                Vec::new()
            }
            Some(Event::NewConnection) => {
                // Il modulo vive nel tool: finché non si conferma, non c'è
                // niente da dire all'app.
                self.draft = Some(ToolSqlDraft::default());
                Vec::new()
            }
            Some(Event::DraftTyped(field, value)) => {
                if let Some(draft) = self.draft.as_mut() {
                    let target = match field {
                        DraftField::Label => &mut draft.label,
                        DraftField::Host => &mut draft.host,
                        DraftField::Database => &mut draft.database,
                        DraftField::User => &mut draft.user,
                        DraftField::TenantId => &mut draft.tenant_id,
                        DraftField::ClientId => &mut draft.client_id,
                    };
                    target.clone_from(value);
                }
                Vec::new()
            }
            Some(Event::DraftToggledOracle) => {
                if let Some(draft) = self.draft.as_mut() {
                    draft.oracle = !draft.oracle;
                    // Il service principal è di Entra: su Oracle non esiste.
                    if draft.oracle {
                        draft.service_principal = false;
                    }
                }
                Vec::new()
            }
            Some(Event::DraftToggledServicePrincipal) => {
                if let Some(draft) = self.draft.as_mut()
                    && !draft.oracle
                {
                    draft.service_principal = !draft.service_principal;
                }
                Vec::new()
            }
            Some(Event::DraftConfirmed) => match self.draft.take() {
                Some(draft) if draft_is_complete(&draft) => {
                    vec![ToolRequest::SqlCreateConnection(draft)]
                }
                // Incompleto: si rimette il modulo com'era invece di
                // buttare via quello che l'utente ha già scritto.
                other => {
                    self.draft = other;
                    Vec::new()
                }
            },
            Some(Event::DraftCancelled) => {
                self.draft = None;
                Vec::new()
            }
            Some(Event::DeleteConnection(index)) => {
                vec![ToolRequest::SqlDeleteConnection(*index)]
            }
            Some(Event::RefreshSchema) => vec![ToolRequest::SqlRefreshSchema],
            Some(Event::SecretTyped(text)) => {
                self.secret_draft = text.clone();
                Vec::new()
            }
            Some(Event::SecretConfirmed) => {
                // `take` e non `clone`: la copia in chiaro dentro il tool
                // finisce qui, appena il segreto è partito verso l'app.
                let secret = std::mem::take(&mut self.secret_draft);
                if secret.is_empty() {
                    Vec::new()
                } else {
                    vec![ToolRequest::SqlSecretSubmitted(secret)]
                }
            }
            Some(Event::SecretCancelled) => {
                self.secret_draft = lume_ui::tool::Redacted::default();
                vec![ToolRequest::SqlSecretCancelled]
            }
            None => Vec::new(),
        }
    }
}

/// I tooltip del prototipo non ci sono: l'app non ha ancora un primitivo
/// tooltip condiviso, e inventarne uno qui non è lavoro di questo giro.
fn tool_button<'a>(
    glyph: &'a str,
    color: iced::Color,
    hover: iced::Color,
    event: Event,
) -> Element<'a, ToolEvent> {
    button(text(glyph).size(12).color(color))
        .padding(Padding::from([2, 5]))
        .style(styles::hover_row(color, hover, None, 4.0))
        .on_press(ToolEvent::new(event))
        .into()
}

/// Riga di connessione: chevron + pallino di stato + etichetta + motore.
fn connection_row<'a>(
    t: &lume_theme::LumeTheme,
    index: usize,
    connection: &'a ConnectionView,
) -> Element<'a, ToolEvent> {
    let (dot, dot_color) = match &connection.state {
        ConnectionState::Connected => ("●", t.good),
        ConnectionState::Connecting => ("◐", t.warn),
        ConnectionState::Error(_) => ("●", t.bad),
        ConnectionState::Disconnected => ("○", t.faint),
    };
    let chevron = if connection.expanded { "▾" } else { "▸" };
    let inner = button(
        row![
            Space::new().width(6),
            text(chevron).size(10).color(t.faint),
            text(dot).size(9).color(dot_color),
            container(
                text(&connection.label)
                    .size(12)
                    .color(t.tree)
                    .wrapping(text::Wrapping::None)
            )
            .clip(true)
            .width(Length::Fill),
            text(&connection.engine).size(10.5).color(t.muted),
        ]
        .spacing(5)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([3, 6]))
    .style(styles::hover_row(t.tree, t.hover, None, 4.0))
    .on_press(ToolEvent::new(Event::ToggleConnection(index)));

    // «×» fuori dal bottone della riga: annidarli non funziona, e comunque
    // sono due gesti diversi.
    row![
        container(inner).width(Length::Fill),
        tool_button("×", t.faint, t.bad, Event::DeleteConnection(index)),
    ]
    .align_y(Alignment::Center)
    .into()
}

/// Riga rossa sotto la connessione. Va a capo se il pannello è stretto:
/// tagliarla (`Wrapping::None`) nasconderebbe proprio la parte che conta.
fn avviso_row<'a>(t: &lume_theme::LumeTheme, testo: &'static str) -> Element<'a, ToolEvent> {
    container(text(testo).size(10.5).color(t.bad))
        .width(Length::Fill)
        .padding(Padding {
            top: 0.0,
            right: 8.0,
            bottom: 2.0,
            left: 22.0,
        })
        .into()
}

/// Riga tabella: clic espande le colonne, doppio clic apre la console.
fn table_row<'a>(
    t: &lume_theme::LumeTheme,
    connection: usize,
    index: usize,
    table: &'a TableView,
) -> Element<'a, ToolEvent> {
    let chevron = if table.expanded { "▾" } else { "▸" };
    let meta = if table.columns.is_empty() {
        String::new()
    } else {
        format!("{} col", table.columns.len())
    };
    let inner = button(
        row![
            Space::new().width(22),
            text(chevron).size(10).color(t.faint),
            container(
                text(&table.name)
                    .size(12)
                    .color(t.sec)
                    .wrapping(text::Wrapping::None)
            )
            .clip(true)
            .width(Length::Fill),
            text(meta).size(10.5).color(t.muted),
        ]
        .spacing(5)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([3, 6]))
    .style(styles::hover_row(t.sec, t.hover, None, 4.0))
    .on_press(ToolEvent::new(Event::ToggleTable(connection, index)));

    mouse_area(inner)
        .on_double_click(ToolEvent::new(Event::OpenTable(connection, index)))
        .into()
}

/// Un profilo è utilizzabile solo se ha tutto il necessario per connettersi.
/// Confermarne uno a metà creerebbe una riga che fallisce ogni volta senza
/// dire perché.
fn draft_is_complete(draft: &ToolSqlDraft) -> bool {
    let identity = if draft.service_principal {
        !draft.tenant_id.trim().is_empty() && !draft.client_id.trim().is_empty()
    } else {
        !draft.user.trim().is_empty()
    };
    !draft.label.trim().is_empty()
        && !draft.host.trim().is_empty()
        && !draft.database.trim().is_empty()
        && identity
}

/// Modulo «nuova origine dati», in linea in cima all'albero.
fn draft_form<'a>(t: &lume_theme::LumeTheme, draft: &'a ToolSqlDraft) -> Element<'a, ToolEvent> {
    let field = |placeholder: &'a str, value: &'a str, which: DraftField| {
        iced::widget::text_input(placeholder, value)
            .size(12)
            .padding(Padding::from([3, 6]))
            .on_input(move |text| ToolEvent::new(Event::DraftTyped(which, text)))
            .on_submit(ToolEvent::new(Event::DraftConfirmed))
    };
    let engine = if draft.oracle { "Oracle" } else { "SQL Server" };
    let mut form = column![
        row![
            tool_button(engine, t.tree, t.hover, Event::DraftToggledOracle),
            Space::new().width(Length::Fill),
            tool_button("✓", t.good, t.hover, Event::DraftConfirmed),
            tool_button("×", t.muted, t.hover, Event::DraftCancelled),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
        field(
            "etichetta, es. azure · prod",
            &draft.label,
            DraftField::Label
        ),
        field(
            if draft.oracle {
                "host[:1521] (tcp:// = in chiaro)"
            } else {
                "server.database.windows.net"
            },
            &draft.host,
            DraftField::Host,
        ),
        field(
            if draft.oracle {
                "service name"
            } else {
                "database"
            },
            &draft.database,
            DraftField::Database,
        ),
    ]
    .spacing(3);

    // Il service principal è di Entra ID: su Oracle il pulsante non compare
    // perché non c'è nulla dietro.
    if !draft.oracle {
        let label = if draft.service_principal {
            "service principal"
        } else {
            "login SQL"
        };
        form = form.push(
            row![tool_button(
                label,
                t.tree,
                t.hover,
                Event::DraftToggledServicePrincipal
            )]
            .align_y(Alignment::Center),
        );
    }
    form = if draft.service_principal {
        form.push(field("tenant id", &draft.tenant_id, DraftField::TenantId))
            .push(field("client id", &draft.client_id, DraftField::ClientId))
    } else {
        form.push(field("utente", &draft.user, DraftField::User))
    };

    let surface = t.surface;
    container(form)
        .padding(Padding::from([6, 8]))
        .width(Length::Fill)
        .style(move |_| styles::bg(surface))
        .into()
}

/// Campo per la password, sotto la riga della connessione che la chiede.
///
/// In linea e non in una finestra modale perché l'app non ha un primitivo di
/// modale, e perché così si vede **di quale** connessione si parla: la riga è
/// subito sopra.
fn secret_prompt<'a>(
    t: &lume_theme::LumeTheme,
    connection: &'a ConnectionView,
    draft: &str,
) -> Element<'a, ToolEvent> {
    let field = iced::widget::text_input(&connection.secret_label, draft)
        .secure(true)
        .size(12)
        .padding(Padding::from([3, 6]))
        .on_input(|text| ToolEvent::new(Event::SecretTyped(lume_ui::tool::Redacted::new(text))))
        .on_submit(ToolEvent::new(Event::SecretConfirmed));
    container(
        row![
            Space::new().width(22.0),
            field,
            tool_button("↵", t.icon, t.hover, Event::SecretConfirmed),
            tool_button("×", t.muted, t.hover, Event::SecretCancelled),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([2, 6]))
    .into()
}

/// Riga foglia non interattiva (colonna, o «nessuna tabella»).
fn leaf_row<'a>(
    t: &lume_theme::LumeTheme,
    indent: f32,
    label: &'a str,
    meta: &'a str,
    color: iced::Color,
) -> Element<'a, ToolEvent> {
    container(
        row![
            Space::new().width(indent),
            container(
                text(label)
                    .size(12)
                    .color(color)
                    .wrapping(text::Wrapping::None)
            )
            .clip(true)
            .width(Length::Fill),
            text(meta).size(10.5).color(t.muted),
        ]
        .spacing(5)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([3, 6]))
    .into()
}

/// Padding del prototipo per le liste dei pannelli laterali.
const fn pad_list() -> Padding {
    Padding {
        top: 2.0,
        right: 12.0,
        bottom: 12.0,
        left: 12.0,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn connection() -> ConnectionView {
        ConnectionView {
            id: "azure-prod".into(),
            label: "azure · prod".into(),
            engine: "SQL Server".into(),
            host: "acme.database.windows.net".into(),
            expanded: true,
            state: ConnectionState::Connected,
            tables: vec![TableView {
                name: "users".into(),
                expanded: false,
                columns: Vec::new(),
            }],
            asking_secret: false,
            secret_label: "password di lume".into(),
            cleartext: false,
        }
    }

    #[test]
    fn l_avviso_in_chiaro_compare_solo_per_le_connessioni_non_cifrate() {
        let mut in_chiaro = connection();
        in_chiaro.cleartext = true;
        assert_eq!(avviso(&in_chiaro), Some(AVVISO_IN_CHIARO));
        assert_eq!(avviso(&connection()), None);
        // L'avviso dice cosa succede, non solo «attenzione».
        assert!(AVVISO_IN_CHIARO.contains("password"));
        assert!(AVVISO_IN_CHIARO.contains("in chiaro"));
    }

    #[test]
    fn il_segreto_esce_dal_tool_una_volta_sola_e_non_ci_resta() {
        let mut tool = DbTool::new();
        tool.update(&ToolEvent::new(Event::Replace(vec![connection()])));

        assert!(
            tool.update(&ToolEvent::new(Event::SecretTyped(
                lume_ui::tool::Redacted::new("p4ss".into())
            )))
            .is_empty(),
            "digitare non deve mandare la password nella coda dei messaggi"
        );
        let requests = tool.update(&ToolEvent::new(Event::SecretConfirmed));
        assert!(matches!(
            requests.as_slice(),
            [ToolRequest::SqlSecretSubmitted(secret)]
                if secret.clone().expose() == "p4ss"
        ));
        assert!(
            tool.secret_draft.is_empty(),
            "confermata, la copia in chiaro dentro il tool deve sparire"
        );
        assert!(
            tool.update(&ToolEvent::new(Event::SecretConfirmed))
                .is_empty(),
            "una seconda conferma non deve rimandare nulla"
        );
    }

    #[test]
    fn annullare_svuota_il_campo_e_avvisa_lapp() {
        let mut tool = DbTool::new();
        tool.update(&ToolEvent::new(Event::SecretTyped(
            lume_ui::tool::Redacted::new("p4ss".into()),
        )));
        let requests = tool.update(&ToolEvent::new(Event::SecretCancelled));
        assert!(matches!(
            requests.as_slice(),
            [ToolRequest::SqlSecretCancelled]
        ));
        assert!(tool.secret_draft.is_empty());
    }

    #[test]
    fn espandere_passa_dall_app_perche_serve_lo_schema() {
        let mut tool = DbTool::new();
        tool.update(&ToolEvent::new(Event::Replace(vec![connection()])));
        assert!(matches!(
            &tool.update(&ToolEvent::new(Event::ToggleConnection(0)))[..],
            [ToolRequest::SqlToggleConnection(0)]
        ));
        assert!(matches!(
            &tool.update(&ToolEvent::new(Event::ToggleTable(0, 0)))[..],
            [ToolRequest::SqlToggleTable(0, 0)]
        ));
    }

    #[test]
    fn il_doppio_clic_su_una_tabella_apre_la_console_con_quella_connessione() {
        let mut tool = DbTool::new();
        tool.update(&ToolEvent::new(Event::Replace(vec![connection()])));
        let requests = tool.update(&ToolEvent::new(Event::OpenTable(0, 0)));
        match &requests[..] {
            [ToolRequest::SqlOpenConsole { connection, table }] => {
                assert_eq!(connection.as_deref(), Some("azure-prod"));
                assert_eq!(table.as_deref(), Some("users"));
            }
            other => panic!("atteso SqlOpenConsole, trovato {other:?}"),
        }
        // indice fuori range: nessuna richiesta, non un panic
        assert!(
            tool.update(&ToolEvent::new(Event::OpenTable(0, 9)))
                .is_empty()
        );
    }

    #[test]
    fn la_barra_apre_una_console_senza_connessione_scelta() {
        let mut tool = DbTool::new();
        let requests = tool.update(&ToolEvent::new(Event::OpenConsole));
        match &requests[..] {
            [ToolRequest::SqlOpenConsole { connection, table }] => {
                assert!(connection.is_none() && table.is_none());
            }
            other => panic!("atteso SqlOpenConsole, trovato {other:?}"),
        }
    }

    #[test]
    fn la_password_non_compare_nel_debug() {
        let evento = Event::SecretTyped(lume_ui::tool::Redacted::new("p4ss-segretissima".into()));
        let testo = format!("{evento:?}");
        assert!(
            !testo.contains("p4ss"),
            "l'evento stampa la password: {testo}"
        );

        let richiesta = ToolRequest::SqlSecretSubmitted(lume_ui::tool::Redacted::new(
            "p4ss-segretissima".into(),
        ));
        let testo = format!("{richiesta:?}");
        assert!(
            !testo.contains("p4ss"),
            "la richiesta stampa la password: {testo}"
        );

        // Anche l'evento avvolto nel guscio opaco del tool.
        let avvolto = ToolEvent::new(Event::SecretTyped(lume_ui::tool::Redacted::new(
            "p4ss-segretissima".into(),
        )));
        assert!(!format!("{avvolto:?}").contains("p4ss"));
    }

    #[test]
    fn chiudere_il_pannello_svuota_la_bozza_della_password() {
        let mut tool = DbTool::new();
        tool.update(&ToolEvent::new(Event::SecretTyped(
            lume_ui::tool::Redacted::new("p4ss".into()),
        )));

        assert!(
            tool.on_visibility_changed(true).is_empty(),
            "aprire il pannello non tocca la bozza"
        );
        assert!(!tool.secret_draft.is_empty());

        assert!(tool.on_visibility_changed(false).is_empty());
        assert!(
            tool.secret_draft.is_empty(),
            "chiuso il pannello la password digitata non deve restare in memoria"
        );
        assert!(
            tool.update(&ToolEvent::new(Event::SecretConfirmed))
                .is_empty(),
            "una conferma dopo la chiusura non deve mandare nulla"
        );
    }
}
