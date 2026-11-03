//! Tool «Estensioni»: l'elenco delle funzionalità pesanti, con l'interruttore
//! che le abilita e il comando che le ferma.
//!
//! Nasce dalla richiesta esplicita dell'utente — «l'utente può scegliere le
//! features pesanti da disabilitare» — ed è il gemello a elenco del tasto che
//! sta nell'header di ogni pannello.
//!
//! **Il tool non legge `Lume`.** Il `Registry` espone solo `&dyn Tool`, e lo
//! stato pesante (socket SQL, sessioni PTY) vive sull'app: le righe arrivano
//! qui **spinte** con un [`Event::Replace`], come già fa il pannello
//! Database. È la stessa parete che regge il resto dei pannelli, e qui lavora a
//! nostro favore: il pannello non può mentire su un numero che non possiede.
//!
//! **Cosa non c'è, e perché.**
//! - Nessun «Marketplace»: Docker, AI, HTTP, Python, Markdown e TOML del
//!   prototipo non hanno backend. Elencarli come «non installate» con un
//!   tasto Installa sarebbe il mock più grande di tutto il prototipo.
//! - Nessun `+N ms` all'avvio dell'IDE: con l'avvio esplicito **nessuna**
//!   funzionalità pesante parte al boot, quindi quel numero varrebbe `+0` per
//!   tutte. La colonna non ha più contenuto, ed è la conferma che
//!   l'architettura funziona.
//! - La riga «Picco RAM processo» c'è solo dove esiste un backend di misura.
//!   Misura il picco del **solo processo Lume**, non del Job Object, quindi i
//!   language server figli non sono contati. Si aggiorna solo a richiesta col
//!   tasto ↺: un timer periodico qui sarebbe lavoro da fermo.
//! - Solo le funzionalità **cablate** (`FeatureSpec::wired`): una riga che non
//!   può avviare niente sarebbe esattamente il mock che questo lavoro toglie.

use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use lume_ui::icons::Icon;
use lume_ui::styles;
use lume_ui::tool::{Tool, ToolCtx, ToolEvent, ToolId, ToolRequest};

use crate::features::{self, Feature};

pub const ID: ToolId = ToolId("ext");

/// Una riga, fotografata dall'app. Porta solo quel che il pannello **non può
/// sapere da solo**: il resto (etichetta, unità, avviabilità) lo legge dalla
/// tabella statica, così non c'è un secondo posto da tenere allineato.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub feature: Feature,
    pub enabled: bool,
    /// Risorse vive adesso. `None` = la funzionalità non sa contarle.
    pub live: Option<usize>,
    /// Il «Ferma» di questa riga sta aspettando una conferma.
    pub confirming: bool,
    /// Quanto ci ha messo ad avviarsi, **in questa sessione**. `None` = non
    /// misurato, e allora non si mostra: nessuna riga porta un numero mai
    /// preso davvero.
    pub startup_ms: Option<u32>,
    /// Premere «Avvia» adesso farebbe qualcosa? Il pannello non può saperlo:
    /// per il language server dipende dal fatto che il binario si risolva, e
    /// quello lo sa solo l'app.
    pub can_start: bool,
    /// Sostituisce la riga generica di `detail_line` quando c'è uno stato
    /// più preciso (es. «non installato» vs «non avviata»).
    pub detail_override: Option<String>,
    /// Etichetta del tasto d'avvio quando la funzionalità è ferma.
    pub start_label: Option<&'static str>,
    /// Tasti «Copia» (etichetta, testo), mostrati solo a server vivo.
    pub copies: Vec<(&'static str, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguageServerSection {
    pub name: String,
    pub version: String,
    pub detail: String,
    pub can_download: bool,
    pub downloading: bool,
}

#[derive(Clone, Debug)]
pub enum Event {
    Replace {
        rows: Vec<Row>,
        language_server: Option<LanguageServerSection>,
    },
    Toggle(Feature),
    Stop(Feature),
    Start(Feature),
    CancelStop,
    Copy(String),
    // Aggiorna il picco di memoria visualizzato nel pannello.
    PeakMemoryUpdated(Option<u64>),
    // Pulsante «Aggiorna RAM».
    RefreshPeakMemory,
}

pub struct ExtensionsTool {
    rows: Vec<Row>,
    language_server: Option<LanguageServerSection>,
    // Picco processo in byte (None = non misurato o backend assente).
    peak_memory_bytes: Option<u64>,
    // Vero solo dove il sistema espone davvero una misura.
    memory_backend: bool,
}

impl ExtensionsTool {
    pub fn new() -> Self {
        Self::with_memory_backend(crate::memory::HAS_BACKEND)
    }

    pub fn with_memory_backend(memory_backend: bool) -> Self {
        Self {
            rows: Vec::new(),
            language_server: None,
            peak_memory_bytes: None,
            memory_backend,
        }
    }

    /// La riga del picco di memoria esiste solo dove c'è una misura.
    pub fn shows_ram_row(&self) -> bool {
        self.memory_backend
    }
}

impl Default for ExtensionsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for ExtensionsTool {
    fn id(&self) -> ToolId {
        ID
    }

    fn title(&self) -> &'static str {
        "Estensioni"
    }

    fn icon(&self) -> Icon {
        Icon::Extensions
    }

    /// Nucleo: è il pannello da cui si riaccende tutto il resto, e non deve
    /// poter spegnere sé stesso.
    fn feature(&self) -> Option<&'static str> {
        None
    }

    fn view<'a>(&'a self, ctx: &ToolCtx<'a>) -> Element<'a, ToolEvent> {
        let t = ctx.theme;
        let mut list = column![].spacing(2);
        if let Some(server) = &self.language_server {
            list = list.push(language_server_section(server, t));
        }
        for riga in &self.rows {
            let spec = features::spec(riga.feature);
            let dettaglio = riga
                .detail_override
                .clone()
                .unwrap_or_else(|| features::detail_line(spec, riga.live, riga.startup_ms));

            let mut testa = row![
                text(spec.label)
                    .size(12)
                    .color(if riga.enabled { t.ui } else { t.faint }),
                Space::new().width(Length::Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            testa = testa.push(switch(
                riga.enabled,
                t,
                ToolEvent::new(Event::Toggle(riga.feature)),
            ));

            // «Ferma» e la sua conferma mandano lo **stesso** comando: l'app
            // vede la conferma già in sospeso e la seconda volta ferma davvero.
            // Un messaggio a parte sarebbe un secondo percorso da tenere
            // allineato al primo, per la stessa decisione.
            let comando = riga.enabled.then(|| {
                features::stop_control(
                    riga.feature,
                    riga.live.unwrap_or(0),
                    riga.confirming,
                    t,
                    features::FeatureControls {
                        stop: ToolEvent::new(Event::Stop(riga.feature)),
                        confirm: ToolEvent::new(Event::Stop(riga.feature)),
                        cancel: ToolEvent::new(Event::CancelStop),
                        start: riga
                            .can_start
                            .then(|| ToolEvent::new(Event::Start(riga.feature))),
                        start_label: riga.start_label,
                    },
                )
            });
            let comando = comando.flatten();
            let dettaglio = if riga.enabled {
                dettaglio
            } else {
                "disattivata".to_owned()
            };

            let mut scheda_riga = column![testa].spacing(3);
            // Niente riga vuota sotto il nome: Git, Segnalibri e Cronologia
            // non tengono risorse e non hanno né dettaglio né comando.
            if !dettaglio.is_empty() || comando.is_some() {
                let mut piede = row![
                    text(dettaglio).size(11).color(t.muted),
                    Space::new().width(Length::Fill),
                ]
                .spacing(6)
                .align_y(Alignment::Center);
                if let Some(comando) = comando {
                    piede = piede.push(comando);
                }
                scheda_riga = scheda_riga.push(piede);
            }
            if riga.enabled && !riga.copies.is_empty() {
                let mut copie = row![].spacing(6);
                for (etichetta, testo) in &riga.copies {
                    copie = copie.push(
                        button(text(*etichetta).size(11).color(t.sec))
                            .padding(Padding::from([2, 6]))
                            .style(styles::hover_row(t.sec, t.hover, Some(t.border), 4.0))
                            .on_press(ToolEvent::new(Event::Copy(testo.clone()))),
                    );
                }
                scheda_riga = scheda_riga.push(copie);
            }

            list = list.push(
                container(scheda_riga)
                    .width(Length::Fill)
                    .padding(Padding::from([5, 6]))
                    .style(scheda(t)),
            );
        }
        // ── Riga picco di memoria ───────────────────────────────
        if self.shows_ram_row() {
            let ram_label = picco_ram_label(self.peak_memory_bytes);
            let ram_row = container(
                column![
                    row![
                        text("Picco RAM processo").size(12).color(t.ui),
                        Space::new().width(Length::Fill),
                        button(text("↺").size(11).color(t.sec))
                            .padding(Padding::from([1, 5]))
                            .style(styles::hover_row(t.sec, t.hover, None, 4.0))
                            .on_press(ToolEvent::new(Event::RefreshPeakMemory)),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                    text(ram_label).size(11).color(t.muted),
                ]
                .spacing(3),
            )
            .width(Length::Fill)
            .padding(Padding::from([5, 6]))
            .style(scheda(t));
            list = list.push(ram_row);
        }

        scrollable(container(list).width(Length::Fill).padding(pad_list()))
            .height(Length::Fill)
            .into()
    }

    fn update(&mut self, event: &ToolEvent) -> Vec<ToolRequest> {
        match event.downcast::<Event>() {
            Some(Event::Replace {
                rows,
                language_server,
            }) => {
                self.rows.clone_from(rows);
                self.language_server.clone_from(language_server);
                Vec::new()
            }
            // Lo stato vero è dell'app: il pannello chiede il **contrario di
            // quel che ha mostrato**, non scrive niente da sé. E se quella
            // riga non c'è, non manda niente: inventare un verso sarebbe
            // peggio del silenzio.
            Some(Event::Toggle(feature)) => self
                .rows
                .iter()
                .find(|row| row.feature == *feature)
                .map(|row| ToolRequest::SetFeatureEnabled {
                    feature: features::spec(*feature).key,
                    enabled: !row.enabled,
                })
                .into_iter()
                .collect(),
            Some(Event::Stop(feature)) => {
                vec![ToolRequest::StopFeature(features::spec(*feature).key)]
            }
            Some(Event::Start(feature)) => {
                vec![ToolRequest::StartFeature(features::spec(*feature).key)]
            }
            Some(Event::CancelStop) => vec![ToolRequest::CancelStopFeature],
            Some(Event::Copy(testo)) => vec![ToolRequest::WriteClipboard(testo.clone())],
            // Picco di memoria
            Some(Event::PeakMemoryUpdated(bytes)) => {
                self.peak_memory_bytes = *bytes;
                Vec::new()
            }
            Some(Event::RefreshPeakMemory) => {
                vec![ToolRequest::RefreshPeakMemory]
            }
            None => Vec::new(),
        }
    }
}

/// «—» finché non c'è una misura; «0 KB» è una misura, non un'assenza.
fn picco_ram_label(bytes: Option<u64>) -> String {
    bytes.map_or_else(|| "—".to_owned(), format_bytes)
}

/// L'interruttore del prototipo: pista + pomello, senza widget nuovi.
///
/// Acceso usa gli **stessi token della casella spuntata** del pannello Git
/// (`git_native.rs::check_box`): fondo `sel`, bordo `accent`. Prima la pista
/// era `accent` piena col pomello `accent_on` — due azzurri vicini, quindi una
/// pillola satura in cui il pomello non si distingueva e l'interruttore non
/// sembrava un interruttore. Il pomello ora è `text`, il colore più chiaro del
/// tema: è lui a dover saltare all'occhio, perché è lui che dice se è acceso.
fn switch<'a>(on: bool, t: &lume_theme::LumeTheme, message: ToolEvent) -> Element<'a, ToolEvent> {
    let (pista, bordo, pomello) = if on {
        (t.sel, t.accent, t.text)
    } else {
        (t.inset, t.faint, t.sec)
    };
    let corpo = row![
        Space::new().width(Length::Fixed(if on { 11.0 } else { 2.0 })),
        pillola(pomello, 9.0, 9.0),
    ]
    .align_y(Alignment::Center);
    button(
        container(corpo)
            .width(24)
            .height(13)
            .align_y(Alignment::Center)
            .style(move |_| container::Style {
                background: Some(Background::Color(pista)),
                border: Border {
                    color: bordo,
                    width: 1.0,
                    radius: 6.5.into(),
                },
                ..container::Style::default()
            }),
    )
    .padding(0)
    .style(styles::hover_row(t.ui, Color::TRANSPARENT, None, 6.5))
    .on_press(message)
    .into()
}

/// Il pomello dell'interruttore: un rettangolo tondo, come `dot` in
/// `git_native.rs`.
fn pillola<'a>(colore: Color, larghezza: f32, altezza: f32) -> Element<'a, ToolEvent> {
    container(Space::new())
        .width(larghezza)
        .height(altezza)
        .style(move |_| container::Style {
            background: Some(Background::Color(colore)),
            border: Border {
                radius: 99.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

/// La scheda di una riga: fondo `surface`, bordo `border2`, r5 — la stessa
/// forma delle pill del pannello Commit.
fn scheda(t: &lume_theme::LumeTheme) -> impl Fn(&iced::Theme) -> container::Style + use<> {
    let (bg, border) = (t.surface, t.border2);
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        border: Border {
            radius: 5.0.into(),
            width: 1.0,
            color: border,
        },
        ..container::Style::default()
    }
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

/// Formatta i byte in una stringa leggibile (MB con una cifra decimale).
fn format_bytes(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes >= MB {
        let mb = bytes as f64 / MB as f64;
        format!("{mb:.1} MB")
    } else {
        let kb = bytes / 1024;
        format!("{kb} KB")
    }
}

fn language_server_section<'a>(
    server: &LanguageServerSection,
    t: &lume_theme::LumeTheme,
) -> Element<'a, ToolEvent> {
    let mut testa = row![
        text("Server linguaggio").size(11).color(t.muted),
        Space::new().width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if server.can_download && !server.downloading {
        testa = testa.push(
            button(
                row![
                    text("⬇").size(8).color(t.good),
                    text("Scarica").size(10.5).color(t.sec),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([2, 7]))
            .style(styles::hover_row(t.sec, t.hover, None, 4.0))
            .on_press(ToolEvent::new(Event::Start(
                crate::features::Feature::LanguageServer,
            ))),
        );
    }
    if server.downloading {
        testa = testa.push(
            button(text("Annulla").size(10.5).color(t.sec))
                .padding(Padding::from([2, 7]))
                .style(styles::hover_row(t.sec, t.hover, None, 4.0))
                .on_press(ToolEvent::new(Event::Stop(
                    crate::features::Feature::LanguageServer,
                ))),
        );
    }
    let version = if server.version.is_empty() {
        String::new()
    } else {
        format!(" · {}", server.version)
    };
    container(
        column![
            testa,
            text(format!("{}{}", server.name, version))
                .size(12)
                .color(t.ui),
            text(server.detail.clone()).size(11).color(t.muted),
        ]
        .spacing(3),
    )
    .width(Length::Fill)
    .padding(Padding::from([5, 6]))
    .style(scheda(t))
    .into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::features::Startable;

    fn riga(feature: Feature, enabled: bool, live: Option<usize>) -> Row {
        Row {
            feature,
            enabled,
            live,
            confirming: false,
            startup_ms: None,
            can_start: true,
            detail_override: None,
            start_label: None,
            copies: Vec::new(),
        }
    }

    #[test]
    fn copia_manda_il_testo_agli_appunti() {
        let mut tool = ExtensionsTool::new();
        let richieste = tool.update(&ToolEvent::new(Event::Copy("claude mcp add …".to_owned())));
        assert!(
            matches!(sola(richieste), ToolRequest::WriteClipboard(testo) if testo == "claude mcp add …")
        );
    }

    /// La sola richiesta prodotta, se ce n'è una sola. `ToolRequest` non è
    /// `PartialEq` (porta `PathBuf` e segreti), quindi si asserisce con
    /// `matches!` invece che con l'uguaglianza.
    fn sola(richieste: Vec<ToolRequest>) -> ToolRequest {
        assert_eq!(richieste.len(), 1, "una richiesta, non {richieste:?}");
        richieste.into_iter().next().unwrap()
    }

    /// L'interruttore chiede **il contrario di quel che mostra**, non uno
    /// stato che si è calcolato da sé: lo stato vero è dell'app, e un pannello
    /// che tenesse la propria copia andrebbe fuori sincrono al primo comando
    /// arrivato da un'altra strada (l'header del pannello, per esempio).
    #[test]
    fn l_interruttore_chiede_il_contrario_di_cio_che_mostra() {
        let mut tool = ExtensionsTool::new();
        tool.update(&ToolEvent::new(Event::Replace {
            rows: vec![
                riga(Feature::Database, true, Some(2)),
                riga(Feature::Terminal, false, Some(0)),
            ],
            language_server: None,
        }));

        assert!(matches!(
            sola(tool.update(&ToolEvent::new(Event::Toggle(Feature::Database)))),
            ToolRequest::SetFeatureEnabled {
                feature: "database",
                enabled: false,
            }
        ));
        assert!(matches!(
            sola(tool.update(&ToolEvent::new(Event::Toggle(Feature::Terminal)))),
            ToolRequest::SetFeatureEnabled {
                feature: "terminale",
                enabled: true,
            }
        ));
        assert!(
            tool.update(&ToolEvent::new(Event::Toggle(Feature::Git)))
                .is_empty(),
            "una riga che il pannello non mostra non produce comandi"
        );
    }

    /// «Ferma» non decide niente: passa la chiave e lascia all'app la
    /// conferma. Il pannello non sa se un figlio è vivo.
    #[test]
    fn fermare_manda_solo_la_chiave() {
        let mut tool = ExtensionsTool::new();
        tool.update(&ToolEvent::new(Event::Replace {
            rows: vec![riga(Feature::Terminal, true, Some(3))],
            language_server: None,
        }));

        assert!(matches!(
            sola(tool.update(&ToolEvent::new(Event::Stop(Feature::Terminal)))),
            ToolRequest::StopFeature("terminale")
        ));
        assert!(matches!(
            sola(tool.update(&ToolEvent::new(Event::CancelStop))),
            ToolRequest::CancelStopFeature
        ));
    }

    /// Una funzionalità che non tiene risorse non mostra un comando inerte:
    /// stessa regola di `MENUS` in `menu.rs`, e il motivo per cui `stop_control`
    /// restituisce `Option`.
    ///
    /// ATTENZIONE: Con una sola eccezione, ed è quella che rende «LSP a comando» una
    /// cosa e non una riga di testo: chi si avvia **a comando** e non è
    /// avviato mostra il verso opposto. Senza, il language server avrebbe un
    /// pannello che dice «non avviata» e niente da premere — il mock che
    /// questo lavoro serve a togliere.
    #[test]
    fn le_funzionalita_leggere_non_mostrano_ferma() {
        let tema = lume_theme::LumeTheme::default();
        let comando = |feature, live: usize| {
            features::stop_control(
                feature,
                live,
                false,
                &tema,
                features::FeatureControls {
                    stop: ToolEvent::new(Event::CancelStop),
                    confirm: ToolEvent::new(Event::CancelStop),
                    cancel: ToolEvent::new(Event::CancelStop),
                    start: Some(ToolEvent::new(Event::Start(feature))),
                    start_label: None,
                },
            )
            .is_some()
        };
        for (feature, live, atteso) in [
            (Feature::Git, 0, false),
            (Feature::Bookmarks, 0, false),
            (Feature::Database, 0, false),
            (Feature::Database, 2, true),
            (Feature::Terminal, 1, true),
            // il verso opposto: fermo **e** avviabile a comando
            (Feature::LanguageServer, 0, true),
            (Feature::LanguageServer, 1, true),
            (Feature::Mcp, 0, true),
            (Feature::Mcp, 1, true),
        ] {
            assert_eq!(comando(feature, live), atteso, "{feature:?} con {live}");
        }
        // e senza un messaggio d'avvio non si disegna comunque niente: un
        // tasto che non ha dove mandare la richiesta è inerte per definizione
        assert!(
            features::stop_control(
                Feature::LanguageServer,
                0,
                false,
                &tema,
                features::FeatureControls {
                    stop: ToolEvent::new(Event::CancelStop),
                    confirm: ToolEvent::new(Event::CancelStop),
                    cancel: ToolEvent::new(Event::CancelStop),
                    start: None,
                    start_label: None,
                },
            )
            .is_none()
        );
        assert_eq!(
            features::spec(Feature::Git).startable,
            Startable::Nessuno,
            "Git non tiene processi: solo l'interruttore"
        );
    }

    /// Zero-mock: una piattaforma senza misura non ha la riga (né il pulsante ↺).
    #[test]
    fn senza_backend_la_riga_ram_non_si_disegna() {
        assert!(!ExtensionsTool::with_memory_backend(false).shows_ram_row());
        assert!(ExtensionsTool::with_memory_backend(true).shows_ram_row());
    }

    #[test]
    fn l_etichetta_del_picco_distingue_assente_da_misurato() {
        assert_eq!(picco_ram_label(None), "—");
        assert_eq!(picco_ram_label(Some(0)), "0 KB");
        assert_eq!(picco_ram_label(Some(512 * 1024)), "512 KB");
        assert_eq!(picco_ram_label(Some(3 * 1024 * 1024)), "3.0 MB");
    }

    #[test]
    fn il_picco_letto_dall_app_arriva_al_pannello() {
        let mut tool = ExtensionsTool::with_memory_backend(true);
        assert!(
            tool.update(&ToolEvent::new(Event::PeakMemoryUpdated(Some(
                5 * 1024 * 1024
            ))))
            .is_empty()
        );
        assert_eq!(tool.peak_memory_bytes, Some(5 * 1024 * 1024));
        tool.update(&ToolEvent::new(Event::PeakMemoryUpdated(None)));
        assert_eq!(tool.peak_memory_bytes, None);
    }
}
