//! I blocchi di funzionalità di Lume, e il comando che li avvia e li ferma.
//!
//! Nasce da una richiesta esplicita: «le features più pesanti devono avere un
//! tasto di avvio e stop nei pannelli, per mantenere la filosofia di super
//! leggerezza». Il budget prestazionale (README) vieta processi in
//! background da inattivo; questo modulo è il posto in cui quella promessa
//! diventa una manopola invece di una speranza.
//!
//! **È un enum, non un trait.** Un trait vorrebbe `dyn` con stato eterogeneo,
//! quindi `Any` e downcast: la stessa parete che il progetto ha già rifiutato
//! per `Tool`. E non c'è comportamento condiviso da astrarre — aprire un
//! socket TDS e fare spawn di una PTY non hanno una riga in comune. Il
//! condiviso è il **vocabolario**, non il codice.
//!
//! **Due assi, da non confondere:**
//! - `enabled` — deciso nel pannello Estensioni, **persistito**. Spegnere fa
//!   sparire i pannelli della funzionalità dalle barre.
//! - *vivo* — se in questo momento tiene risorse. **Non si memorizza**: si
//!   deriva da chi le possiede davvero (`SqlWorkspace`, `TerminalTool`), o
//!   andrebbe fuori sincrono al primo percorso che qualcuno dimentica.

use std::time::Duration;

use iced::widget::{Space, button, row, text};
use iced::{Alignment, Element, Length, Padding};
use lume_theme::LumeTheme;
use lume_ui::styles;
use lume_ui::tool::ToolId;

use crate::tools;

/// Un blocco di funzionalità con backend reale.
///
/// Non c'è una variante per ogni pannello: qui stanno i blocchi che
/// l'utente riconosce come «una cosa», e che ha senso spegnere in blocco.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Feature {
    Git,
    Database,
    Terminal,
    Bookmarks,
    History,
    Notes,
    LanguageServer,
    /// Server MCP opzionale. La variante rimane nota anche nei build senza
    /// feature Cargo, come Database.
    Mcp,
    /// Debugger DAP (piano 17): una sessione per volta, avviata da una
    /// configurazione di `.lume/launch.toml`.
    Debugger,
}

impl Feature {
    /// Ogni variante, a prescindere dai cfg del build: chi riconcilia lo
    /// stato d'inattività non deve perdere il Database in un build senza la
    /// feature `sql` (il tool resta registrato e lo `SqlWorkspace` esiste).
    pub const ALL: [Feature; 9] = [
        Feature::Git,
        Feature::Database,
        Feature::Terminal,
        Feature::Bookmarks,
        Feature::History,
        Feature::Notes,
        Feature::LanguageServer,
        Feature::Mcp,
        Feature::Debugger,
    ];
}

/// Come si avvia una funzionalità.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Startable {
    /// Si accende e si spegne con un comando. Il tasto ha due versi.
    Comando,
    /// Si accende **aprendo una risorsa** — espandere un'origine dati, aprire
    /// una scheda del terminale — quindi il comando può solo **fermare**.
    ///
    /// Un «Avvia» a livello di funzionalità non saprebbe *quale* risorsa
    /// aprire né con quali credenziali: sarebbe un tasto che non può agire, e
    /// il progetto non renderizza comandi che non hanno un backend
    /// (`MENUS` in `menu.rs`).
    SoloStop,
    /// Non tiene risorse: si può solo disabilitare, non fermare.
    Nessuno,
}

/// Descrittore statico di una funzionalità.
pub struct FeatureSpec {
    pub feature: Feature,
    /// Chiave di persistenza e di `Tool::feature()`.
    pub key: &'static str,
    pub label: &'static str,
    /// I tool che spariscono dalle barre quando la funzionalità è spenta.
    pub tools: &'static [ToolId],
    /// Singolare e plurale dell'unità contata, per la riga del pannello.
    pub unit: (&'static str, &'static str),
    pub startable: Startable,
    /// `false` finché il backend non è cablato.
    ///
    /// **Non è una scelta dell'utente**, ed è per questo che non passa dalla
    /// persistenza: il giorno che diventa `true`, la funzionalità arriva anche
    /// alle sessioni salvate ieri. Salvarla fra le spente la congelerebbe
    /// invece per sempre in ogni sessione esistente.
    pub wired: bool,
    /// Dopo quanto una funzionalità **viva ma senza viste aperte** passa da
    /// Dormiente a ferma. `None` = nessun timer: non scade mai da sola.
    ///
    /// La decide la tabella d'inattività (`idle.rs`); qui c'è solo il numero,
    /// così aggiungere una funzionalità con ibernazione è una riga e non un
    /// campo nuovo in `Lume`.
    pub idle: Option<Duration>,
}

const GIT: FeatureSpec = FeatureSpec {
    feature: Feature::Git,
    key: "git",
    label: "Git",
    tools: &[tools::git_native::ID, tools::gitlog::ID],
    unit: ("", ""),
    startable: Startable::Nessuno,
    wired: true,
    idle: None,
};

#[cfg(feature = "sql")]
const DATABASE: FeatureSpec = FeatureSpec {
    feature: Feature::Database,
    key: "database",
    label: "Database",
    tools: &[tools::db::ID],
    unit: ("connessione", "connessioni"),
    startable: Startable::SoloStop,
    wired: true,
    idle: Some(crate::sql_mode::IDLE_TIMEOUT),
};

#[cfg(not(feature = "sql"))]
const DATABASE: FeatureSpec = FeatureSpec {
    feature: Feature::Database,
    key: "database",
    label: "Database",
    tools: &[],
    unit: ("connessione", "connessioni"),
    startable: Startable::SoloStop,
    wired: false,
    idle: None,
};

const TERMINALE: FeatureSpec = FeatureSpec {
    feature: Feature::Terminal,
    key: "terminale",
    label: "Terminale",
    tools: &[tools::terminal::ID],
    unit: ("sessione", "sessioni"),
    startable: Startable::SoloStop,
    wired: true,
    idle: None,
};

const SEGNALIBRI: FeatureSpec = FeatureSpec {
    feature: Feature::Bookmarks,
    key: "segnalibri",
    label: "Segnalibri",
    tools: &[tools::bookmarks::ID],
    unit: ("", ""),
    startable: Startable::Nessuno,
    wired: true,
    idle: None,
};

const CRONOLOGIA: FeatureSpec = FeatureSpec {
    feature: Feature::History,
    key: "cronologia",
    label: "Cronologia",
    tools: &[tools::history::ID],
    unit: ("", ""),
    startable: Startable::Nessuno,
    wired: true,
    idle: None,
};

const APPUNTI: FeatureSpec = FeatureSpec {
    feature: Feature::Notes,
    key: "appunti",
    label: "Appunti",
    tools: &[tools::notes::ID],
    unit: ("", ""),
    startable: Startable::Nessuno,
    wired: true,
    idle: None,
};

/// Il language server. È cablato: il dock Problemi mostra
/// diagnostica vera, e il tasto ha due versi — è l'unica funzionalità che si
/// **avvia** a comando, le altre si accendono aprendo una risorsa.
const LANGUAGE_SERVER: FeatureSpec = FeatureSpec {
    feature: Feature::LanguageServer,
    key: "lsp",
    label: "Language server",
    tools: &[tools::problems::ID, tools::structure::ID, tools::usages::ID],
    unit: ("server", "server"),
    startable: Startable::Comando,
    wired: true,
    idle: None,
};

/// Il server MCP si avvia a comando, dopo un riquadro che nomina la radice.
const MCP: FeatureSpec = FeatureSpec {
    feature: Feature::Mcp,
    key: "mcp",
    label: "Server MCP",
    tools: &[],
    unit: ("client", "client"),
    startable: Startable::Comando,
    wired: true,
    idle: None,
};

/// La tabella, per chi deve **elencare**. Chi ne vuole una sola passa da
/// [`spec`], che è un `match` e non una ricerca.
///
/// La voce `DATABASE` è protetta dalla feature `sql` (default-on): un build
/// senza `sql` non ha la riga Database nel pannello Estensioni e non registra
/// il tool (`tools/mod.rs`). Il descrittore resta leggibile per identità
/// persistita, con `wired: false` e nessun tool disponibile.
/// Chiave del debugger, condivisa con il tool Debug.
pub const DEBUGGER_KEY: &str = "debugger";

/// `SoloStop`: un «Avvia» generico non saprebbe quale programma lanciare;
/// si avvia dal pannello Debug scegliendo una configurazione. Nessuna
/// ibernazione: un programma fermo su un breakpoint è lavoro dell'utente.
const DEBUGGER: FeatureSpec = FeatureSpec {
    feature: Feature::Debugger,
    key: DEBUGGER_KEY,
    label: "Esecuzione e debug",
    tools: &[tools::debug::ID, tools::maven::ID],
    unit: ("sessione", "sessioni"),
    startable: Startable::SoloStop,
    wired: true,
    idle: None,
};

pub const FEATURES: &[&FeatureSpec] = &[
    &GIT,
    #[cfg(feature = "sql")]
    &DATABASE,
    &TERMINALE,
    &SEGNALIBRI,
    &CRONOLOGIA,
    &APPUNTI,
    &LANGUAGE_SERVER,
    #[cfg(feature = "mcp")]
    &MCP,
    &DEBUGGER,
];

/// Il descrittore di una funzionalità.
///
/// È un `match` e non un `find` sulla tabella di proposito: così la
/// copertura di ogni variante la controlla il compilatore, e non serve né
/// un `unwrap` né un ripiego che mentirebbe.
pub const fn spec(feature: Feature) -> &'static FeatureSpec {
    match feature {
        Feature::Git => &GIT,
        Feature::Database => &DATABASE,
        Feature::Terminal => &TERMINALE,
        Feature::Bookmarks => &SEGNALIBRI,
        Feature::History => &CRONOLOGIA,
        Feature::Notes => &APPUNTI,
        Feature::LanguageServer => &LANGUAGE_SERVER,
        Feature::Mcp => &MCP,
        Feature::Debugger => &DEBUGGER,
    }
}

/// Identità persistita, non elenco delle righe visibili: un cfg che nasconde
/// Database da Estensioni non può rendere anonimo un tool ancora registrato.
pub fn by_key(key: &str) -> Option<&'static FeatureSpec> {
    Feature::ALL
        .into_iter()
        .map(spec)
        .find(|spec| spec.key == key)
}

/// Chi possiede un tool, se qualcuno, anche fuori dalle barre predefinite.
pub fn feature_of(id: ToolId) -> Option<Feature> {
    Feature::ALL
        .into_iter()
        .map(spec)
        .find(|spec| spec.tools.contains(&id))
        .map(|spec| spec.feature)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureCommand {
    Start(Feature),
    Stop(Feature),
    SetEnabled(Feature, bool),
}

impl FeatureCommand {
    pub fn feature(self) -> Feature {
        match self {
            Self::Start(feature) | Self::Stop(feature) | Self::SetEnabled(feature, _) => feature,
        }
    }
}

/// Che fare quando arriva un comando. È il seam su cui si asserisce: `Task`
/// è opaco, la decisione no — stessa forma di `DiffRefresh` e `VaultAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureAction {
    Start,
    Stop,
    /// Disabilitare una funzionalità **viva** la deve prima fermare, o
    /// resterebbero socket e processi appesi a una cosa che non si vede più.
    StopThenDisable,
    Disable,
    Enable,
    Nothing,
}

/// La decisione, pura.
///
/// `live` è «tiene risorse in questo momento», derivato da chi le possiede.
pub fn feature_action(
    enabled: bool,
    live: bool,
    startable: Startable,
    command: FeatureCommand,
) -> FeatureAction {
    match command {
        // Solo chi si avvia a comando può essere avviato, e solo se è acceso
        // e non è già in piedi.
        FeatureCommand::Start(_) => {
            if startable == Startable::Comando && enabled && !live {
                FeatureAction::Start
            } else {
                FeatureAction::Nothing
            }
        }
        FeatureCommand::Stop(_) => {
            if live && startable != Startable::Nessuno {
                FeatureAction::Stop
            } else {
                FeatureAction::Nothing
            }
        }
        FeatureCommand::SetEnabled(_, want) => match (enabled, want) {
            (true, false) if live => FeatureAction::StopThenDisable,
            (true, false) => FeatureAction::Disable,
            (false, true) => FeatureAction::Enable,
            _ => FeatureAction::Nothing,
        },
    }
}

/// Il comando che una conferma sta davvero confermando.
///
/// ATTENZIONE: La conferma arriva **sempre** come `Stop(f)`: è l'unico messaggio che i
/// tre «Ferma» del chrome e del pannello sanno mandare. Ma il comando che l'ha
/// aperta poteva essere un `SetEnabled(f, false)`, e in quel caso fermare e
/// basta lascerebbe la funzionalità **accesa** — l'interruttore tornerebbe su
/// da solo dopo che l'utente ha confermato di volerlo spegnere.
///
/// Quindi la conferma **rigioca il comando in sospeso**, non quello che
/// arriva. Una conferma su un'altra funzionalità non c'entra e passa
/// invariata: la conferma è una sola per volta e quella vecchia è già scaduta.
pub fn confirm_replays(
    pending: Option<FeatureCommand>,
    incoming: FeatureCommand,
) -> FeatureCommand {
    match pending {
        Some(pending)
            if pending.feature() == incoming.feature()
                && matches!(incoming, FeatureCommand::Stop(_)) =>
        {
            pending
        }
        _ => incoming,
    }
}

/// Testo della riga nel pannello Estensioni.
///
/// **Nessuna riga mostra un numero mai misurato**: al suo posto si dice che
/// non è stata avviata. È la regola che tiene il pannello onesto, e il motivo
/// per cui il `+N ms` del prototipo non c'è — con l'avvio esplicito nessuna
/// funzionalità pesante parte al boot, quindi quel numero varrebbe `+0` per
/// tutte.
pub fn detail_line(
    spec: &FeatureSpec,
    live_count: Option<usize>,
    startup_ms: Option<u32>,
) -> String {
    let conteggio = match live_count {
        // «non avviata» sarebbe fuorviante per chi si accende **aprendo una
        // risorsa**: il terminale non si «avvia», gli si apre una scheda.
        Some(0) | None => match spec.startable {
            Startable::Nessuno => String::new(),
            Startable::SoloStop => format!("nessuna {}", spec.unit.0),
            Startable::Comando => "non avviata".to_owned(),
        },
        Some(1) => format!("1 {}", spec.unit.0),
        Some(n) => format!("{n} {}", spec.unit.1),
    };
    match startup_ms {
        Some(ms) if !conteggio.is_empty() => format!("{conteggio} · {}", startup_text(ms)),
        Some(ms) => startup_text(ms),
        None => conteggio,
    }
}

/// Il tempo d'avvio **cronometrato**, dal comando alla prima risposta utile.
///
/// Questa riga era stata rimandata, e per un motivo preciso: allora
/// non aveva un soggetto. Database e Terminale si accendono **aprendo una
/// risorsa**, quindi un «avvio N ms» per loro misurerebbe una `connect` o uno
/// spawn di PTY, cioè un'altra cosa. Il language server invece si avvia, e il
/// punto di misura è la risposta a `initialize` — dove la sessione diventa
/// `Ready`, cioè usabile.
///
/// Non si persiste: mostrarlo dopo un riavvio sarebbe spacciare la misura di
/// un'altra sessione.
fn startup_text(ms: u32) -> String {
    if ms < 1000 {
        return format!("avvio {ms} ms");
    }
    format!("avvio {},{} s", ms / 1000, (ms % 1000) / 100)
}

/// Il comando in linea nell'header del pannello (o del dock) della
/// funzionalità: è la richiesta dell'utente — «un tasto di avvio e stop nei
/// pannelli» — e sta nel **chrome della shell**, non in `Tool::view`, perché
/// lo stato pesante non vive dentro il tool.
///
/// `None` quando non c'è niente da fermare: **non si disegna un comando
/// inerte** (`MENUS` in `menu.rs`). Generica sul messaggio perché i due header
/// vivono in spazi diversi e non devono poter divergere.
/// I messaggi che il comando sa mandare. Un gruppo e non quattro parametri:
/// sono le quattro facce della **stessa** decisione, e tenerle insieme
/// impedisce di scambiarne due all'unico posto dove si scrivono.
#[derive(Debug, Clone)]
pub struct FeatureControls<M> {
    pub stop: M,
    pub confirm: M,
    pub cancel: M,
    /// `None` dove non c'è dove mandarlo: un tasto senza destinatario non si
    /// disegna, come tutto il resto in questo progetto.
    pub start: Option<M>,
    pub start_label: Option<&'static str>,
}

pub fn stop_control<'a, M: Clone + 'a>(
    feature: Feature,
    live: usize,
    confirming: bool,
    theme: &LumeTheme,
    controls: FeatureControls<M>,
) -> Option<Element<'a, M>> {
    let FeatureControls {
        stop,
        confirm,
        cancel,
        start,
        start_label,
    } = controls;
    let spec = spec(feature);
    if spec.startable == Startable::Nessuno {
        return None;
    }
    // ATTENZIONE: Chi si avvia **a comando** e non è avviato ha bisogno del verso
    // opposto, o «LSP a comando» sarebbe una riga che non può agire: il
    // pannello direbbe «non avviata» e non ci sarebbe nulla da premere. Chi
    // si accende aprendo una risorsa non lo mostra, che sarebbe un tasto
    // senza soggetto.
    if live == 0 {
        let start = start.filter(|_| spec.startable == Startable::Comando)?;
        let label = start_label.unwrap_or("Avvia");
        return Some(
            row![
                Space::new().width(Length::Fixed(2.0)),
                button(
                    row![
                        text("▶").size(8).color(theme.good),
                        text(label).size(10.5).color(theme.sec),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center)
                )
                .padding(Padding::from([2, 7]))
                .style(styles::hover_row(theme.sec, theme.hover, None, 4.0))
                .on_press(start),
            ]
            .align_y(Alignment::Center)
            .into(),
        );
    }
    if confirming {
        let unit = if live == 1 { spec.unit.0 } else { spec.unit.1 };
        return Some(
            row![
                text(format!("Chiudere {live} {unit}?"))
                    .size(11)
                    .color(theme.warn),
                button(text("Ferma").size(11).color(theme.bad))
                    .padding(Padding::from([2, 7]))
                    .style(styles::hover_row(theme.bad, theme.hover, None, 4.0))
                    .on_press(confirm),
                button(text("Annulla").size(11).color(theme.sec))
                    .padding(Padding::from([2, 7]))
                    .style(styles::hover_row(theme.sec, theme.hover, None, 4.0))
                    .on_press(cancel),
            ]
            .spacing(5)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    Some(
        row![
            Space::new().width(Length::Fixed(2.0)),
            button(
                row![
                    text("■").size(8).color(theme.bad),
                    text("Ferma").size(10.5).color(theme.sec),
                ]
                .spacing(4)
                .align_y(Alignment::Center)
            )
            .padding(Padding::from([2, 7]))
            .style(styles::hover_row(theme.sec, theme.hover, None, 4.0))
            .on_press(stop),
        ]
        .align_y(Alignment::Center)
        .into(),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn identita_database_persistita_non_dipende_dalla_riga_visibile() {
        assert_eq!(
            by_key(DATABASE.key).map(|s| s.feature),
            Some(Feature::Database)
        );
        for &tool in DATABASE.tools {
            assert_eq!(feature_of(tool), Some(Feature::Database));
        }
        assert_eq!(
            FEATURES.iter().any(|s| s.feature == Feature::Database),
            cfg!(feature = "sql")
        );
        assert!(by_key("funzionalita-inesistente").is_none());
    }

    #[test]
    fn registrazione_database_corrisponde_alla_feature_compilata() {
        let (registry, _terminal_wakeup) = tools::default_registry();
        let enabled = cfg!(feature = "sql");
        assert_eq!(registry.get(lume_ui::ToolId("db")).is_some(), enabled);
        assert_eq!(tools::persisted_id("db").is_some(), enabled);
        assert_eq!(DATABASE.wired, enabled);
        assert_eq!(DATABASE.tools.is_empty(), !enabled);
        // La preferenza persistita resta riconoscibile anche senza backend.
        assert_eq!(
            by_key("database").map(|spec| spec.feature),
            Some(Feature::Database)
        );
        assert!(registry.get(tools::project::ID).is_some());
    }

    /// Ogni funzionalità dev'essere raggiungibile dalla sua chiave e dai suoi
    /// tool: sono le due strade da cui arrivano i comandi (persistenza e
    /// barre), e una tabella incoerente si scoprirebbe solo a runtime.
    #[test]
    fn la_tabella_delle_funzionalita_e_coerente_nelle_due_direzioni() {
        for spec in FEATURES {
            assert_eq!(by_key(spec.key).map(|s| s.feature), Some(spec.feature));
            for &id in spec.tools {
                assert_eq!(feature_of(id), Some(spec.feature), "tool {id:?}");
            }
        }
        let chiavi: std::collections::BTreeSet<_> = FEATURES.iter().map(|s| s.key).collect();
        assert_eq!(chiavi.len(), FEATURES.len(), "chiavi duplicate");
    }

    /// Solo il Database ha un timer d'inattività, ed è quello dei cinque
    /// minuti di `sql_mode`. Terminale, language server, Git e le altre non
    /// scadono mai da sole: chiuderle perderebbe lavoro dell'utente o un
    /// indice costoso (vedi «Tabella d'inattività» in `ARCHITECTURE.md`).
    #[test]
    fn nessun_altra_funzionalita_ha_un_timer_d_inattivita() {
        for feature in Feature::ALL {
            let spec = spec(feature);
            assert_eq!(spec.feature, feature, "ALL e spec() devono coincidere");
            if feature == Feature::Database {
                #[cfg(feature = "sql")]
                assert_eq!(spec.idle, Some(crate::sql_mode::IDLE_TIMEOUT));
                #[cfg(feature = "sql")]
                assert_eq!(spec.idle, Some(Duration::from_secs(300)));
                #[cfg(not(feature = "sql"))]
                assert_eq!(spec.idle, None);
            } else {
                assert_eq!(spec.idle, None, "{:?} non deve avere un timer", spec.key);
            }
        }
        for spec in FEATURES {
            assert!(Feature::ALL.contains(&spec.feature));
        }
    }

    /// Il caso che perde risorse se sbagliato: spegnere dal pannello una
    /// funzionalità che sta tenendo un socket deve **prima fermarla**.
    #[test]
    fn disabilitare_una_funzionalita_viva_la_ferma_prima_di_spegnerla() {
        let cmd = FeatureCommand::SetEnabled(Feature::Database, false);
        assert_eq!(
            feature_action(true, true, Startable::SoloStop, cmd),
            FeatureAction::StopThenDisable
        );
        assert_eq!(
            feature_action(true, false, Startable::SoloStop, cmd),
            FeatureAction::Disable,
            "senza niente di vivo non c'e' nulla da fermare"
        );
    }

    /// Database e Terminale si accendono aprendo una risorsa, non da un
    /// tasto: un «Avvia» di funzionalità non saprebbe quale origine dati
    /// aprire né con quale password.
    #[test]
    fn le_funzionalita_solo_stop_non_si_avviano_da_comando() {
        assert_eq!(
            feature_action(
                true,
                false,
                Startable::SoloStop,
                FeatureCommand::Start(Feature::Database)
            ),
            FeatureAction::Nothing
        );
        assert_eq!(
            feature_action(
                true,
                false,
                Startable::Comando,
                FeatureCommand::Start(Feature::Database)
            ),
            FeatureAction::Start
        );
    }

    /// Fermare quel che non è vivo, o riaccendere quel che è già acceso, non
    /// deve produrre lavoro: i comandi arrivano anche da scorciatoie e voci
    /// di menù, dove nessuno guarda lo stato prima di premere.
    #[test]
    fn un_comando_che_non_cambia_niente_non_produce_lavoro() {
        for startable in [Startable::Comando, Startable::SoloStop, Startable::Nessuno] {
            assert_eq!(
                feature_action(
                    true,
                    false,
                    startable,
                    FeatureCommand::Stop(Feature::Terminal)
                ),
                FeatureAction::Nothing
            );
        }
        assert_eq!(
            feature_action(
                true,
                false,
                Startable::Comando,
                FeatureCommand::SetEnabled(Feature::Terminal, true)
            ),
            FeatureAction::Nothing
        );
        assert_eq!(
            feature_action(
                false,
                false,
                Startable::Comando,
                FeatureCommand::Start(Feature::Terminal)
            ),
            FeatureAction::Nothing,
            "una funzionalita' spenta non si avvia: il tasto non la resusciterebbe"
        );
    }

    /// La conferma rigioca il comando **in sospeso**, non quello che arriva.
    ///
    /// I tre «Ferma» del chrome e del pannello mandano tutti `Stop(f)`: senza
    /// questa funzione, confermare uno spegnimento fermerebbe la funzionalità
    /// lasciandola accesa, e l'interruttore tornerebbe su da solo.
    #[test]
    fn la_conferma_rigioca_il_comando_che_l_ha_aperta() {
        use Feature::{Database, Terminal};
        let spegni = FeatureCommand::SetEnabled(Terminal, false);

        assert_eq!(
            confirm_replays(Some(spegni), FeatureCommand::Stop(Terminal)),
            spegni,
            "confermare uno spegnimento spegne, non ferma soltanto"
        );
        assert_eq!(
            confirm_replays(
                Some(FeatureCommand::Stop(Terminal)),
                FeatureCommand::Stop(Terminal)
            ),
            FeatureCommand::Stop(Terminal),
            "una conferma di «Ferma» resta «Ferma»"
        );
        assert_eq!(
            confirm_replays(Some(spegni), FeatureCommand::Stop(Database)),
            FeatureCommand::Stop(Database),
            "una conferma aperta su un'altra funzionalità non c'entra"
        );
        // Solo un `Stop` è una conferma: gli altri comandi arrivano da un
        // gesto nuovo e devono passare com'è, o l'interruttore non
        // risponderebbe più finché una conferma resta aperta.
        assert_eq!(
            confirm_replays(
                Some(FeatureCommand::Stop(Terminal)),
                FeatureCommand::SetEnabled(Terminal, false)
            ),
            FeatureCommand::SetEnabled(Terminal, false)
        );
        assert_eq!(
            confirm_replays(None, FeatureCommand::Stop(Terminal)),
            FeatureCommand::Stop(Terminal)
        );
    }

    /// Una funzionalità che non ha mai tenuto risorse non deve mostrare
    /// numeri inventati — e nemmeno dire «non avviata» se non è una cosa che
    /// si avvia: il terminale si accende **aprendo una scheda**.
    #[test]
    fn una_funzionalita_mai_avviata_non_mostra_numeri() {
        let db = spec(Feature::Database);
        assert_eq!(detail_line(db, None, None), "nessuna connessione");
        assert_eq!(detail_line(db, Some(0), None), "nessuna connessione");
        assert_eq!(detail_line(db, Some(1), None), "1 connessione");
        assert_eq!(detail_line(db, Some(3), None), "3 connessioni");
        assert_eq!(
            detail_line(spec(Feature::LanguageServer), None, None),
            "non avviata",
            "questo invece si avvia davvero a comando"
        );
        assert_eq!(
            detail_line(spec(Feature::Git), None, None),
            "",
            "chi non tiene risorse non dice ne' «non avviata» ne' un conteggio"
        );
    }

    /// Il cronometro compare **solo** quando c'è stata una misura vera. È la
    /// regola che tiene onesto il pannello, ed è il motivo per cui il `+N ms`
    /// del prototipo non c'è: quello era «quanto aggiunge all'avvio dell'IDE»,
    /// e con l'avvio esplicito vale zero per tutte.
    #[test]
    fn il_cronometro_si_mostra_solo_se_e_stato_misurato() {
        let lsp = spec(Feature::LanguageServer);
        assert_eq!(
            detail_line(lsp, Some(1), Some(1234)),
            "1 server · avvio 1,2 s"
        );
        assert_eq!(
            detail_line(lsp, Some(1), Some(90)),
            "1 server · avvio 90 ms"
        );
        assert_eq!(
            detail_line(lsp, Some(0), None),
            "non avviata",
            "mai misurato: non si mostra niente"
        );
        // e chi non ha né conteggio né misura resta muto
        assert_eq!(detail_line(spec(Feature::Git), None, None), "");
    }

    /// La voce esiste solo quando il codice MCP è stato compilato.
    #[test]
    fn la_voce_mcp_esiste_solo_con_la_feature() {
        assert_eq!(
            FEATURES.iter().any(|spec| spec.feature == Feature::Mcp),
            cfg!(feature = "mcp")
        );
        let mcp = spec(Feature::Mcp);
        assert_eq!(mcp.key, "mcp");
        assert_eq!(mcp.startable, Startable::Comando);
        assert!(mcp.tools.is_empty());
        assert!(
            mcp.wired,
            "la riga ha la sua UI: Avvia, Ferma, Copia e spia"
        );
    }

    #[test]
    fn il_server_mcp_si_avvia_e_si_ferma_a_comando() {
        assert_eq!(
            feature_action(
                true,
                false,
                Startable::Comando,
                FeatureCommand::Start(Feature::Mcp)
            ),
            FeatureAction::Start
        );
        assert_eq!(
            feature_action(
                true,
                true,
                Startable::Comando,
                FeatureCommand::Stop(Feature::Mcp)
            ),
            FeatureAction::Stop
        );
        assert_eq!(
            feature_action(
                true,
                true,
                Startable::Comando,
                FeatureCommand::Start(Feature::Mcp)
            ),
            FeatureAction::Nothing
        );
    }
}
