#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Lume, l'IDE desktop leggero scritto in Rust con Iced.
//!
//! Questo è il crate dell'applicazione: la finestra, i menù, i pannelli e il
//! collegamento fra loro. Il lavoro vero sta nei crate di dominio (`lume-core`,
//! `lume-syntax`, `lume-lsp`, `lume-git`, `lume-terminal`…). Per la mappa dei
//! crate e il contratto dei pannelli vedi `ARCHITECTURE.md`, per lavorarci
//! `CONTRIBUTING.md`.

mod about;
mod activity;
mod app;
mod app_icon;
mod appearance;
mod autosave;
mod breadcrumb;
mod completion;
mod completion_ui;
mod conferma_chiusura;
mod conferma_elimina;
#[cfg(feature = "mcp")]
mod consenso_mcp;
mod context_menu;
mod crash_log;
mod diag_marks;
mod diff_view;
mod dock;
mod editor_empty;
mod editor_groups;
mod editor_tab_bar;
mod environment_picker;
mod environments;
mod error_log;
mod external;
mod features;
mod file_read;
mod find_replace;
mod forge;
mod forge_panel;
mod git_actions;
mod hover;
#[cfg(feature = "sql")]
mod idea_import;
mod idle;
mod jj_actions;
mod layout;
mod lsp_mode;
mod lsp_pos;
mod lsp_set;
mod lsp_uri;
#[cfg(feature = "mcp")]
mod mcp_card;
#[cfg(feature = "mcp")]
mod mcp_gate;
#[cfg(feature = "mcp")]
mod mcp_lsp;
#[cfg(feature = "mcp")]
mod mcp_mode;
mod reader_jobs;
mod signature_help;
// `GetProcessMemoryInfo`: FFI Windows.
#[allow(unsafe_code)]
mod memory;
mod menu;
mod metrics;
mod navigazione;
mod note_bars;
mod note_links;
mod overlay_manager;
mod prompt;
mod recents;
mod rename;
mod rename_box;
mod replace;
mod replace_batch;
mod right_click;
// Verifiche d'identità file Windows tramite handle e API Win32.
#[allow(unsafe_code)]
mod save;
mod scorciatoie;
mod scritture;
mod search;
mod search_events;
mod session_restore;
mod settings_panel;
mod shell;
#[cfg(feature = "sql")]
mod sql_console;
#[cfg(feature = "sql")]
mod sql_events;
#[cfg(feature = "sql")]
mod sql_mode;
#[cfg(feature = "sql")]
mod sql_snapshot;
mod startup;
mod statusbar;
mod terminal_widget;
mod titlebar;
mod tools;
mod updates;

use app::Lume;

fn main() -> iced::Result {
    // Il ponte MCP viene avviato prima della finestra e del tracciamento
    // dell'applicazione. Il client MCP possiede questo processo fino a EOF.
    if startup::mcp_stdio_requested(std::env::args_os()) {
        std::process::exit(mcp_stdio());
    }
    // Prima riga: tutto quel che segue e' avvio da misurare.
    startup::segna_avvio();
    // Senza console (release) un panic non lascerebbe traccia: va su file.
    crash_log::installa();
    startup::traccia("crash_log installato");
    // #12: `lume.exe --prova-crash` scrive un log dei crash e esce con 101,
    // prima di aprire la finestra.
    if crash_log::prova_richiesta(std::env::args_os()) {
        panic!("{}", crash_log::MESSAGGIO_PROVA);
    }
    let applicazione = iced::application(Lume::new, Lume::update, Lume::view)
        .title("Lume")
        .theme(|app: &Lume| app.theme_state.iced.clone())
        .subscription(Lume::subscription)
        .font(lume_ui::JETBRAINS_MONO)
        .font(lume_ui::JETBRAINS_MONO_MEDIUM)
        .font(lume_ui::JETBRAINS_MONO_SEMIBOLD)
        .font(lume_ui::INTER)
        .font(lume_ui::INTER_MEDIUM)
        .font(lume_ui::INTER_SEMIBOLD)
        .default_font(lume_ui::UI)
        .window(iced::window::Settings {
            size: iced::Size::new(metrics::WINDOW_W, metrics::WINDOW_H),
            min_size: Some(iced::Size::new(940.0, 620.0)),
            decorations: false,
            // La X del sistema e Alt+F4 non chiudono da sole: arrivano come
            // `close_requests()` e passano dalla stessa domanda della X della
            // barra del titolo, o i file sporchi sparirebbero senza avviso.
            exit_on_close_request: false,
            // Barra delle applicazioni e Alt-Tab. Se la costruzione fallisce
            // si resta con l'icona di sistema: un'icona è una rifinitura, non
            // un motivo per non partire.
            icon: {
                let (dati, larghezza, altezza) = app_icon::rgba();
                iced::window::icon::from_rgba(dati, larghezza, altezza).ok()
            },
            #[cfg(target_os = "linux")]
            platform_specific: iced::window::settings::PlatformSpecific {
                application_id: "io.github.mak1jk.Lume".into(),
                ..Default::default()
            },
            ..Default::default()
        });
    startup::traccia("iced configurato, si entra nel ciclo eventi");
    let risultato = applicazione.run();
    if let Err(error) = lume_lsp::drain_process_reaper(std::time::Duration::from_secs(5)) {
        let messaggio = format!("drain finale del process reaper LSP non riuscito: {error}");
        startup::traccia(&messaggio);
        eprintln!("lume: {messaggio}");
    }
    risultato
}

#[cfg(feature = "mcp")]
fn mcp_stdio() -> i32 {
    let data_dir = lume_store::application_data_dir().map_err(|error| error.to_string());
    lume_mcp::proxy::run_stdio_proxy(std::env::args_os().skip(2), data_dir)
}

#[cfg(not(feature = "mcp"))]
fn mcp_stdio() -> i32 {
    use std::io::Write as _;
    let _ = writeln!(
        std::io::stderr(),
        "lume --mcp-stdio: questa build di Lume non include il server MCP (feature «mcp»)"
    );
    69
}
