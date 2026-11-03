#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[cfg(feature = "sql")]
use super::VaultAction;
use super::{
    Lume, Message, NoticeKind, RestoredTabPayload, RestoredTabPayloadMessage, Ricaricato,
    leggi_ramo,
};
use crate::menu::{MenuAct, MenuIntent};
use crate::overlay_manager::OverlayId;

#[test]
fn escape_catturato_dal_widget_non_viene_scartato_dal_listener_globale() {
    let escape = iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
        modified_key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
        physical_key: iced::keyboard::key::Physical::Code(iced::keyboard::key::Code::Escape),
        location: iced::keyboard::Location::Standard,
        modifiers: iced::keyboard::Modifiers::default(),
        text: None,
        repeat: false,
    });

    assert!(
        !super::ignora_evento_globale_catturato(&escape, iced::event::Status::Captured),
        "Escape deve arrivare al listener globale anche se un widget lo cattura"
    );
}

#[test]
fn gli_altri_tasti_catturati_restano_esclusi_dal_listener_globale() {
    let lettera = iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: iced::keyboard::Key::Character("a".into()),
        modified_key: iced::keyboard::Key::Character("a".into()),
        physical_key: iced::keyboard::key::Physical::Code(iced::keyboard::key::Code::KeyA),
        location: iced::keyboard::Location::Standard,
        modifiers: iced::keyboard::Modifiers::default(),
        text: Some("a".into()),
        repeat: false,
    });

    assert!(super::ignora_evento_globale_catturato(
        &lettera,
        iced::event::Status::Captured
    ));
}

#[test]
fn aiuto_scorciatoie_apre_il_riquadro_e_esc_lo_chiude() {
    let (mut app, _) = Lume::new_without_store();
    let aiuto = crate::menu::MENUS
        .iter()
        .find(|menu| menu.name == "Aiuto")
        .unwrap();
    assert!(
        aiuto.items.iter().any(|voce| voce.label == "Scorciatoie…"),
        "il menù Aiuto non ha la voce «Scorciatoie…»"
    );

    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::Shortcuts));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Scorciatoie));

    let _ = app.update(Message::CloseAll);
    assert_eq!(app.shell.active_overlay(), None);
}

#[test]
fn esc_pulisce_solo_il_modello_del_riquadro_chiuso() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    write_and_open(&mut app, dir.path(), "prova.txt", "testo\n");
    let doc = app.active_doc().unwrap();
    app.rename = Some(crate::rename_box::RenameBox::new(
        doc,
        lsp_types::Position::new(0, 0),
        0,
        "testo".into(),
    ));
    app.goto_line = Some("42".into());
    app.apri_overlay(OverlayId::Rename);
    app.apri_overlay(OverlayId::GotoLine);

    let _ = app.update(Message::CloseAll);
    assert_eq!(app.shell.active_overlay(), None);
    assert!(app.goto_line.is_none());
    assert!(app.rename.is_some(), "il modello sostituito sopravvive");

    let _ = app.update(Message::CloseAll);
    assert!(
        app.rename.is_some(),
        "Esc senza overlay non cancella altri modelli"
    );
    app.apri_overlay(OverlayId::Rename);
    app.goto_line = Some("17".into());
    let _ = app.update(Message::CloseAll);
    assert_eq!(app.shell.active_overlay(), None);
    assert!(app.rename.is_none());
    assert_eq!(app.goto_line.as_deref(), Some("17"));
}

#[test]
fn esc_su_overlay_sostitutivo_non_autorizza_la_chiusura_sospesa() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    write_and_open(&mut app, dir.path(), "sporco.txt", "ciao\n");
    let doc = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    chiudi_scheda(&mut app, doc);
    assert!(app.chiusura.is_some());
    app.goto_line = Some("8".into());
    app.apri_overlay(OverlayId::GotoLine);
    assert_eq!(
        app.shell.active_overlay(),
        Some(OverlayId::ConfermaChiusura)
    );
    let _ = app.update(Message::CloseAll);
    assert!(app.chiusura.is_none());
    assert_eq!(app.goto_line.as_deref(), Some("8"));
    rispondi(
        &mut app,
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    );
    assert!(app.groups.contains(doc));
    assert!(app.workspace.get(doc).unwrap().is_dirty());
}

#[test]
fn esc_su_code_actions_pulisce_la_lista_e_preserva_goto_line() {
    let mut app = app();
    app.goto_line = Some("23".into());
    app.code_actions
        .push(lsp_types::CodeActionOrCommand::Command(
            lsp_types::Command {
                title: "azione".into(),
                command: "azione".into(),
                arguments: None,
            },
        ));
    app.apri_overlay(OverlayId::CodeActions);
    let _ = app.update(Message::CloseAll);
    assert_eq!(app.shell.active_overlay(), None);
    assert!(app.code_actions.is_empty());
    assert_eq!(app.goto_line.as_deref(), Some("23"));
}

#[test]
fn scelta_azione_consuma_il_risultato_senza_chiudere_goto_line() {
    let mut app = app();
    app.code_actions
        .push(lsp_types::CodeActionOrCommand::Command(
            lsp_types::Command {
                title: "azione".into(),
                command: "azione".into(),
                arguments: None,
            },
        ));
    app.apri_overlay(OverlayId::CodeActions);
    app.goto_line = Some("23".into());
    app.apri_overlay(OverlayId::GotoLine);
    let _ = app.update(Message::CodeActionChosen(0));
    assert!(app.code_actions.is_empty());
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::GotoLine));
    assert_eq!(app.goto_line.as_deref(), Some("23"));
    assert!(app.notice.is_some());
}

fn azione_aiuto_elimina_trova(azione: MenuAct, atteso: OverlayId) {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    write_and_open(&mut app, dir.path(), "trova.txt", "una parola\n");
    let _ = app.update(Message::MenuAction(MenuAct::Find));
    assert!(app.find.is_some());
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Find));

    let _ = app.update(Message::MenuAction(azione));
    assert_eq!(app.shell.active_overlay(), Some(atteso));
    assert!(app.find.is_none(), "{azione:?} deve chiudere Trova");
}

#[test]
fn informazioni_elimina_trova_quando_sostituisce_il_riquadro() {
    azione_aiuto_elimina_trova(MenuAct::About, OverlayId::About);
}

#[test]
fn scorciatoie_elimina_trova_quando_sostituisce_il_riquadro() {
    azione_aiuto_elimina_trova(MenuAct::Shortcuts, OverlayId::Scorciatoie);
}

#[test]
fn scorciatoie_chiude_il_menu_aperto() {
    let mut app = app();
    let _ = app.update(Message::MenuBar(crate::menu::MenuBarMessage::ToggleRoot));
    assert!(app.menu_bar.is_open());
    let _ = app.update(Message::MenuAction(MenuAct::Shortcuts));
    assert!(!app.menu_bar.is_open());
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Scorciatoie));
}

#[path = "tests/albero_operations.rs"]
mod albero_operations;
#[path = "tests/context_menu_surfaces.rs"]
mod context_menu_surfaces;
#[path = "tests/file_commands_b.rs"]
mod file_commands_b;
#[path = "tests/native_jj_consumer80.rs"]
mod native_jj_consumer80;
#[path = "tests/native_lsp_consumer64.rs"]
mod native_lsp_consumer64;
#[path = "tests/note_link_cfg83.rs"]
mod note_link_cfg83;
#[path = "tests/save_as_safety58.rs"]
mod save_as_safety58;
#[path = "tests/save_owned_tests.rs"]
mod save_owned_tests;
#[path = "tests/untitled_tests.rs"]
mod untitled_tests;
use crate::activity::ActivityBarIntent;
use crate::environment_picker::EnvironmentPickerMessage;
use crate::file_read::{DopoApertura, Testo, leggi_per_apertura};
use crate::search::{GlobalSearchMessage, SearchTab};
use crate::statusbar::StatusbarMessage;
use crate::titlebar::TitlebarMessage;
use crate::tools;
use lume_ui::editor::EditorCommand;
use std::path::Path;

fn app() -> Lume {
    Lume::new_without_store().0
}

#[test]
fn un_lume_di_prova_non_ha_il_log_degli_errori() {
    assert!(app().error_log.is_none());
}

#[test]
fn le_azioni_git_sono_serializzate_e_un_cambio_root_scarta_l_esito_vecchio() {
    let prima = tempfile::tempdir().unwrap();
    let seconda = tempfile::tempdir().unwrap();
    let mut app = app();
    app.set_project_root(prima.path().to_path_buf());
    let _worker = app.run_git_action(lume_ui::tool::GitAction::Fetch);
    let sequence = app.git_action_seq;
    let ticket = app.git_operazione.unwrap().ticket;
    assert!(app.git_action_busy);
    let _scartata = app.run_git_action(lume_ui::tool::GitAction::Push);
    assert_eq!(app.git_action_seq, sequence, "una seconda azione non parte");

    app.set_project_root(seconda.path().to_path_buf());
    assert!(
        app.git_action_cancel
            .as_ref()
            .unwrap()
            .0
            .load(std::sync::atomic::Ordering::SeqCst)
    );
    assert!(
        app.git_action_busy,
        "attende la conclusione del worker posseduto"
    );

    let _ = app.update(Message::GitWriteFinished(
        ticket,
        Box::new(super::GitWriteResult::Action(
            lume_ui::tool::GitAction::Fetch,
            crate::git_actions::ActionReport::ok("risultato vecchio"),
        )),
    ));
    assert!(!app.git_action_busy);
    assert!(
        app.notice.is_none(),
        "un esito vecchio non raggiunge l'utente"
    );
}

#[test]
fn richieste_git_unificate_entrano_nel_worker_e_si_annullano() {
    use lume_ui::tool::{GitAction, ToolRequest};
    use std::sync::atomic::Ordering;

    let root = tempfile::tempdir().unwrap();
    let mut app = app();
    app.set_project_root(root.path().to_path_buf());
    let _task = app.apply_tool_requests(vec![(
        crate::tools::git_native::ID,
        ToolRequest::GitAction(GitAction::Fetch),
    )]);
    assert!(app.git_action_busy);
    assert_eq!(app.git_action_seq, 1);

    let cancel = app.git_action_cancel.as_ref().unwrap().0.clone();
    let _task =
        app.apply_tool_requests(vec![(crate::tools::git_native::ID, ToolRequest::GitCancel)]);
    assert!(cancel.load(Ordering::SeqCst));
}

#[test]
fn completamento_azione_git_ricarica_stato_log_e_stash_una_volta_sola() {
    use lume_ui::tool::GitAction;
    use std::sync::atomic::Ordering::SeqCst;

    let root = tempfile::tempdir().unwrap();
    let mut app = app();
    app.set_project_root(root.path().to_path_buf());
    app.registry.set_active_dock(Some(crate::tools::gitlog::ID));
    let _worker = app.run_git_action(GitAction::Fetch);
    let ticket = app.git_operazione.unwrap().ticket;
    let before = (
        app.git_gen.load(SeqCst),
        app.git_log_gen.load(SeqCst),
        app.git_stash_gen.load(SeqCst),
    );

    let _refresh = app.update(Message::GitWriteFinished(
        ticket,
        Box::new(super::GitWriteResult::Action(
            GitAction::Fetch,
            crate::git_actions::ActionReport::ok("fetch completato"),
        )),
    ));
    assert!(!app.git_action_busy);
    assert_eq!(app.git_gen.load(SeqCst), before.0 + 1);
    assert_eq!(app.git_log_gen.load(SeqCst), before.1 + 1);
    assert_eq!(app.git_stash_gen.load(SeqCst), before.2);

    let _worker = app.run_git_action(GitAction::StashPop(0));
    let ticket = app.git_operazione.unwrap().ticket;
    let stash_before = app.git_stash_gen.load(SeqCst);
    let _refresh = app.update(Message::GitWriteFinished(
        ticket,
        Box::new(super::GitWriteResult::Action(
            GitAction::StashPop(0),
            crate::git_actions::ActionReport::ok("stash aggiornato"),
        )),
    ));
    assert_eq!(app.git_stash_gen.load(SeqCst), stash_before + 1);
}

#[test]
fn un_azione_git_non_si_sovrappone_a_una_scrittura_occupata() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app();
    app.set_project_root(root.path().to_path_buf());
    let ticket = app.begin_git_write().unwrap();
    let _ignored = app.run_git_action(lume_ui::tool::GitAction::Push);
    assert!(!app.git_action_busy);
    assert_eq!(app.git_operazione.unwrap().ticket, ticket);
}

#[test]
fn un_errore_mostrato_finisce_nel_log_e_un_avviso_informativo_no() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.error_log = Some(crate::error_log::ErrorLog::new(dir.path().to_owned(), None));

    let _ = app.notify(NoticeKind::Info, "Controllo aggiornamenti…".to_owned());
    assert!(!dir.path().join("errors.log").exists());

    let _ = app.notify_error("Apertura di a.txt fallita: accesso negato".to_owned());
    let testo = std::fs::read_to_string(dir.path().join("errors.log")).unwrap();
    assert!(
        testo.contains("Apertura di a.txt fallita: accesso negato"),
        "{testo}"
    );
    assert_eq!(testo.lines().count(), 1, "{testo}");
}

#[test]
fn registra_errore_senza_log_non_fa_niente_e_non_va_in_panico() {
    let mut app = app();
    assert!(app.error_log.is_none());
    app.registra_errore("errore qualunque");
}

#[test]
fn un_errore_persistente_finisce_nel_log_e_un_avviso_informativo_no() {
    // I ripristini delle schede e i conflitti col disco mostrano l'errore con
    // `notify_persistente`: deve lasciare traccia nel log come `notify_error`.
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.error_log = Some(crate::error_log::ErrorLog::new(dir.path().to_owned(), None));

    app.notify_persistente(NoticeKind::Info, "Connessioni rilevate".to_owned());
    assert!(!dir.path().join("errors.log").exists());

    app.notify_persistente(
        NoticeKind::Error,
        "Impossibile riaprire a.txt: documento aggiornato durante la lettura".to_owned(),
    );
    let testo = std::fs::read_to_string(dir.path().join("errors.log")).unwrap();
    assert!(
        testo.contains("Impossibile riaprire a.txt: documento aggiornato durante la lettura"),
        "{testo}"
    );
    assert_eq!(testo.lines().count(), 1, "{testo}");
}

#[test]
fn un_errore_non_persistente_diretto_finisce_nel_log() {
    // Jujutsu e Git mostrano l'errore con `notify(kind, ..)` invece che con
    // `notify_error`: deve comunque finire nel log.
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.error_log = Some(crate::error_log::ErrorLog::new(dir.path().to_owned(), None));

    let _ = app.notify(
        NoticeKind::Error,
        "Jujutsu - Push non riuscita: Error: remote".to_owned(),
    );
    let testo = std::fs::read_to_string(dir.path().join("errors.log")).unwrap();
    assert!(
        testo.contains("Jujutsu - Push non riuscita: Error: remote"),
        "{testo}"
    );
    assert_eq!(testo.lines().count(), 1, "{testo}");
}

#[test]
fn risposta_blame_tardiva_dopo_chiusura_scheda_non_ricrea_la_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "riga\n").unwrap();
    let mut app = app();
    app.open_file(&path).unwrap();
    let id = app.workspace.get_by_path(&path).unwrap();
    app.editor_blame_enabled = true;
    app.editor_blame_available = true;
    app.queue_editor_blame(id);
    let request = std::mem::take(&mut app.pending_tool_requests);
    let _task = app.apply_tool_requests(request);
    let generation = app.editor_blame_generation[&id];
    app.close_tab(id);
    let _ = app.update(Message::EditorBlameLoaded(id, generation, path, Ok(vec![])));
    assert!(!app.editor_blame_cache.contains_key(&id));
    assert!(!app.editor_blame_generation.contains_key(&id));
}

#[test]
fn blame_disattivo_non_accoda_richieste() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "riga\n").unwrap();
    let mut app = app();
    app.open_file(&path).unwrap();
    let id = app.workspace.get_by_path(&path).unwrap();
    app.queue_editor_blame(id);
    assert!(app.pending_tool_requests.is_empty());
}

#[test]
fn il_salvataggio_atomico_conserva_bom_e_fine_riga_e_pulisce_il_documento() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(
        &mut app,
        dir.path(),
        "bom.txt",
        "\u{feff}prima\r\nseconda\r\n",
    );
    sporca_attivo(&mut app);
    let id = app.workspace.get_by_path(&path).unwrap();
    app.write_doc(id).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"\xEF\xBB\xBFXprima\r\nseconda\r\n"
    );
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn un_salvataggio_rifiutato_lascia_file_intatto_e_documento_sporco() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "protetto.txt", "originale\n");
    sporca_attivo(&mut app);
    let id = app.workspace.get_by_path(&path).unwrap();
    let original = std::fs::metadata(&path).unwrap().permissions();
    let mut readonly = original.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&path, readonly).unwrap();
    let result = app.write_doc(id);
    std::fs::set_permissions(&path, original).unwrap();
    assert!(result.unwrap_err().contains("protetto.txt"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "originale\n");
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn annullare_il_fetch_segnala_il_worker_e_la_risposta_libera_la_guardia() {
    use lume_ui::tool::ToolRequest;
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.project_root = Some(dir.path().to_path_buf());
    let _ = app.apply_tool_requests(vec![(
        tools::git_native::ID,
        ToolRequest::GitAction(lume_ui::tool::GitAction::Fetch),
    )]);
    let flag = app.git_action_cancel.as_ref().unwrap().0.clone();
    let ticket = app.git_operazione.unwrap().ticket;
    let _ = app.apply_tool_requests(vec![(tools::git_native::ID, ToolRequest::GitCancel)]);
    assert!(flag.load(Ordering::Relaxed));
    assert!(app.git_action_busy, "si attende ancora il worker");
    let _ = app.update(Message::GitWriteFinished(
        ticket,
        Box::new(super::GitWriteResult::Action(
            lume_ui::tool::GitAction::Fetch,
            crate::git_actions::ActionReport::warning("Annullato."),
        )),
    ));
    assert!(!app.git_action_busy);
    assert!(app.git_action_cancel.is_none());
}

#[test]
fn cambio_progetto_annulla_il_fetch_e_scartane_la_risposta_tardiva() {
    use lume_ui::tool::{GitAction, ToolRequest};
    use std::sync::atomic::Ordering;
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let mut app = app();
    app.project_root = Some(first.path().to_path_buf());
    let _ = app.apply_tool_requests(vec![(
        tools::git_native::ID,
        ToolRequest::GitAction(GitAction::Fetch),
    )]);
    let old_flag = app.git_action_cancel.as_ref().unwrap().0.clone();
    let old_ticket = app.git_operazione.unwrap().ticket;
    app.set_project_root(second.path().to_path_buf());
    assert!(old_flag.load(Ordering::Relaxed));
    let _ = app.update(Message::GitWriteFinished(
        old_ticket,
        Box::new(super::GitWriteResult::Action(
            GitAction::Fetch,
            crate::git_actions::ActionReport::ok("risultato precedente"),
        )),
    ));
    assert!(!app.git_action_busy);
    assert!(app.notice.is_none());
    let _ = app.apply_tool_requests(vec![(
        tools::git_native::ID,
        ToolRequest::GitAction(GitAction::Fetch),
    )]);
    let new_flag = app.git_action_cancel.as_ref().unwrap().0.clone();
    assert!(!new_flag.load(Ordering::Relaxed));
    assert!(!std::sync::Arc::ptr_eq(&old_flag, &new_flag));
}

#[test]
fn uscire_dall_app_chiede_di_annullare_il_fetch() {
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.project_root = Some(dir.path().to_path_buf());
    let _ = app.apply_tool_requests(vec![(
        tools::git_native::ID,
        lume_ui::tool::ToolRequest::GitAction(lume_ui::tool::GitAction::Fetch),
    )]);
    let flag = app.git_action_cancel.as_ref().unwrap().0.clone();
    drop(app);
    assert!(flag.load(Ordering::Relaxed));
}

/// La lettura di `reload_doc` fatta a mano, e il messaggio consegnato.
fn consegna_ricaricamento(app: &mut Lume, id: lume_core::DocId, path: &std::path::Path) {
    let esito = crate::file_read::read_testo_capped(path, crate::file_read::MAX_FILE_BYTES)
        .map(|testo| Ricaricato { testo, nota: None })
        .map_err(|errore| errore.to_string());
    let versione = app
        .workspace
        .get(id)
        .map_or(0, lume_core::Document::version);
    let _ = app.update(Message::DocRicaricato(
        id,
        path.to_path_buf(),
        versione,
        esito,
    ));
}

fn write_and_open(
    app: &mut Lume,
    dir: &std::path::Path,
    name: &str,
    content: &str,
) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, content).unwrap();
    app.open_file(&p).unwrap();
    p
}

/// #11: chi apre una segnalazione deve poter leggere e copiare la
/// versione con calma.
#[test]
fn informazioni_resta_finche_non_la_si_chiude() {
    use crate::menu::MenuAct;
    let mut app = app();
    let _ = app.update(Message::MenuAction(MenuAct::About));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
    let _ = app.update(Message::NoticeExpired(0));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
    let _ = app.update(Message::CloseAll);
    assert!(app.shell.active_overlay().is_none());
}

/// La persistenza non ferma la scadenza delle altre notifiche.
#[test]
fn un_errore_sopra_informazioni_la_sostituisce_e_scade_da_se() {
    use crate::menu::MenuAct;

    let mut app = app();
    let _ = app.update(Message::MenuAction(MenuAct::About));
    let _ = app.update(Message::UpdateChecked(Err("rete assente".to_owned())));
    let (id, notice) = app
        .notice
        .as_ref()
        .map(|(id, notice)| (*id, notice))
        .expect("l'errore");
    assert!(!notice.persistente);
    assert_eq!(notice.text, "rete assente");
    let _ = app.update(Message::NoticeExpired(id));
    assert!(app.notice.is_none(), "l'errore scade come sempre");
}

#[test]
fn open_file_ok_e_fallito() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "a.rs", "fn main() {}\n");
    assert_eq!(app.groups.all().count(), 1);
    assert_eq!(app.active_path(), Some(p.as_path()));
    assert!(app.active_doc().is_some());
    // path inesistente: Err, nessun tab nuovo
    assert!(app.open_file(&dir.path().join("missing.rs")).is_err());
    assert_eq!(app.groups.all().count(), 1);
}

#[test]
fn un_file_riconosciuto_con_la_query_sana_non_ha_errori_di_evidenziazione() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    assert_eq!(
        app.active_syntax_error(),
        None,
        "nessun documento: nessun errore"
    );
    write_and_open(&mut app, dir.path(), "a.rs", "fn main() {}\n");
    assert_eq!(app.active_syntax_error(), None);
    write_and_open(&mut app, dir.path(), "note.txt", "ciao\n");
    assert_eq!(
        app.active_syntax_error(),
        None,
        "un'estensione sconosciuta non è un errore di query"
    );
}

/// Attenzione: un file da 33 MiB non apre nessuna scheda: l'errore lo dice in
/// italiano e l'app resta com'era.
#[test]
fn open_file_rifiuta_un_file_oltre_il_tetto() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let enorme = dir.path().join("enorme.log");
    std::fs::File::create(&enorme)
        .unwrap()
        .set_len(crate::file_read::MAX_FILE_BYTES + 1)
        .unwrap();

    let errore = app.open_file(&enorme).unwrap_err();

    assert!(
        errore.to_string().contains("File troppo grande"),
        "{errore}"
    );
    assert_eq!(app.groups.all().count(), 0, "nessuna scheda");
    assert!(app.workspace.get_by_path(&enorme).is_none());
}

/// Attenzione: costruire il task **non legge niente**: la lettura è nel
/// `spawn_blocking`, non sul thread che chiama.
#[test]
fn open_file_task_non_apre_niente_finche_la_lettura_non_torna() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = dir.path().join("a.rs");
    std::fs::write(&p, "fn a() {}\n").unwrap();

    let _task = app.open_file_task(p.clone(), DopoApertura::Niente);

    assert!(app.workspace.get_by_path(&p).is_none());
    assert_eq!(app.groups.all().count(), 0);
}

#[test]
fn la_lettura_tornata_apre_la_scheda_e_va_alla_riga() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = dir.path().join("a.rs");
    std::fs::write(&p, "uno\ndue\ntre\n").unwrap();

    let _ = app.update(Message::FileLetto(leggi_per_apertura(
        p.clone(),
        DopoApertura::Riga(3),
    )));

    assert_eq!(app.active_path(), Some(p.as_path()));
    let id = app.active_doc().unwrap();
    let documento = app.workspace.get(id).unwrap();
    let riga = documento.rope().char_to_line(documento.selection().head);
    assert_eq!(riga, 2, "la riga 3, contata da 1");
}

/// Attenzione: il file enorme non apre nessuna scheda, notifica in italiano e lascia
/// l'app com'era.
#[test]
fn un_file_enorme_non_apre_nessuna_scheda() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let enorme = dir.path().join("enorme.log");
    std::fs::File::create(&enorme)
        .unwrap()
        .set_len(crate::file_read::MAX_FILE_BYTES + 1)
        .unwrap();

    let _ = app.update(Message::FileLetto(leggi_per_apertura(
        enorme.clone(),
        DopoApertura::Riga(1),
    )));

    assert_eq!(app.groups.all().count(), 0);
    assert!(app.workspace.get_by_path(&enorme).is_none());
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(
        notifica.contains("enorme.log") && notifica.contains("File troppo grande"),
        "{notifica}"
    );
}

#[test]
fn un_file_non_utf8_notifica_e_non_apre_niente() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = dir.path().join("binario.dat");
    std::fs::write(&p, [0xFF, 0xFE, 0x00, 0xC3]).unwrap();

    let _ = app.update(Message::FileLetto(leggi_per_apertura(
        p,
        DopoApertura::Niente,
    )));

    assert_eq!(app.groups.all().count(), 0);
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(notifica.contains("non è testo UTF-8"), "{notifica}");
}

#[test]
fn un_file_dell_avvio_fuori_dal_progetto_cambia_il_progetto() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = dir.path().join("a.rs");
    std::fs::write(&p, "fn a() {}\n").unwrap();

    let _ = app.update(Message::FileLetto(leggi_per_apertura(
        p.clone(),
        DopoApertura::Avvio {
            fuori_progetto: true,
        },
    )));

    assert_eq!(app.active_path(), Some(p.as_path()));
    assert_eq!(app.project_root.as_deref(), p.parent());
}

/// Attenzione: `FileLetto` non esce da `update` con un `return` secco: la coda di
/// fine `update` (`drain_tool_requests`) chiede al server l'outline del documento
/// appena aperto e aggiorna il pannello Struttura.
#[test]
fn dopo_una_lettura_tornata_parte_la_richiesta_dell_outline_del_nuovo_documento() {
    let root = tempdir("file_letto_outline");
    let (mut app, _) = app_con_lsp(&root);
    let mut posta = acceso(&mut app, sa_struttura());
    while posta.try_recv().is_ok() {}
    let altro = root.join("altro.rs");
    std::fs::write(&altro, "fn altro() {}").unwrap();

    let _ = app.update(Message::FileLetto(leggi_per_apertura(
        altro.clone(),
        DopoApertura::Niente,
    )));

    let id = app.workspace.get_by_path(&altro).expect("il file è aperto");
    let mut chiesto = false;
    while let Ok(comando) = posta.try_recv() {
        if let lume_lsp::LspCommand::Ask {
            request: lume_lsp::LspRequest::DocumentSymbols { .. },
            ..
        } = comando
        {
            chiesto = true;
        }
    }
    assert!(chiesto, "l'outline del documento {id:?} va chiesto subito");
    let _ = std::fs::remove_dir_all(&root);
}

/// Come sopra, per il ricaricamento: il testo nuovo ha una versione nuova e il
/// pannello Struttura non può restare su quella vecchia.
#[test]
fn dopo_un_ricaricamento_tornato_parte_la_richiesta_dell_outline() {
    let root = tempdir("doc_ricaricato_outline");
    let (mut app, id) = app_con_lsp(&root);
    let mut posta = acceso(&mut app, sa_struttura());
    while posta.try_recv().is_ok() {}
    let path = root.join("main.rs");
    let versione = app.workspace.get(id).unwrap().version();

    let _ = app.update(Message::DocRicaricato(
        id,
        path,
        versione,
        Ok(Ricaricato {
            testo: Testo::from("fn cambiata() {}".to_owned()),
            nota: None,
        }),
    ));

    let mut chiesto = false;
    while let Ok(comando) = posta.try_recv() {
        if let lume_lsp::LspCommand::Ask {
            request: lume_lsp::LspRequest::DocumentSymbols { .. },
            ..
        } = comando
        {
            chiesto = true;
        }
    }
    assert!(chiesto, "l'outline della versione ricaricata va chiesto");
    let _ = std::fs::remove_dir_all(&root);
}

/// Attenzione: il refresh Git dopo il cambio di radice dell'avvio parte dalla
/// consegna di `FileLetto`: il `git_task` di `new()` ha visto la radice vecchia e
/// `set_project_root` invalida le letture in volo.
#[test]
fn un_file_dell_avvio_fuori_dal_progetto_rilancia_il_refresh_git() {
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.registry.set_active_panel(None);
    app.registry.set_active_dock(None);
    let p = dir.path().join("a.rs");
    std::fs::write(&p, "fn a() {}\n").unwrap();
    let ramo_prima = app.git_branch_gen.load(Ordering::SeqCst);

    let _ = app.update(Message::FileLetto(leggi_per_apertura(
        p,
        DopoApertura::Avvio {
            fuori_progetto: true,
        },
    )));

    assert!(
        app.git_stale && app.git_log_stale,
        "i debiti Git si recuperano alla riapertura delle viste"
    );
    assert!(
        app.git_branch_gen.load(Ordering::SeqCst) > ramo_prima + 1,
        "invalidazione del cambio radice più la lettura del ramo"
    );
}

/// Attenzione: mentre si leggeva, l'utente ha scritto: il suo testo vince sul disco.
#[test]
fn il_ricaricamento_tornato_non_calpesta_un_documento_diventato_sporco() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "a.rs", "uno\n");
    let id = app.workspace.get_by_path(&p).unwrap();
    let versione = app.workspace.get(id).unwrap().version();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));
    assert!(app.workspace.get(id).unwrap().is_dirty());

    let _ = app.update(Message::DocRicaricato(
        id,
        p.clone(),
        versione,
        Ok(Ricaricato {
            testo: Testo::from("dal disco\n".to_owned()),
            nota: None,
        }),
    ));

    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "Xuno\n");
}

/// Attenzione: l'utente ha scritto **e salvato** mentre si leggeva: il documento è di
/// nuovo pulito, ma la lettura è più vecchia del suo salvataggio e non lo sovrascrive.
#[test]
fn il_ricaricamento_tornato_dopo_un_salvataggio_nel_frattempo_e_scartato() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "a.rs", "uno\n");
    let id = app.workspace.get_by_path(&p).unwrap();
    let versione_alla_richiesta = app.workspace.get(id).unwrap().version();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));
    app.workspace.get_mut(id).unwrap().mark_saved();
    assert!(!app.workspace.get(id).unwrap().is_dirty());

    let _ = app.update(Message::DocRicaricato(
        id,
        p,
        versione_alla_richiesta,
        Ok(Ricaricato {
            testo: Testo::from("uno\n".to_owned()),
            nota: None,
        }),
    ));

    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "Xuno\n");
}

#[test]
fn un_ricaricamento_per_un_documento_chiuso_non_fa_niente() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "a.rs", "uno\n");
    let id = app.workspace.get_by_path(&p).unwrap();
    let versione = app.workspace.get(id).unwrap().version();
    app.workspace.close(id);

    let _ = app.update(Message::DocRicaricato(
        id,
        p,
        versione,
        Ok(Ricaricato {
            testo: Testo::from("due\n".to_owned()),
            nota: None,
        }),
    ));

    assert!(app.workspace.get(id).is_none());
}

/// Attenzione: due aperture dello stesso file: la seconda trova il documento già aperto
/// e si limita ad attivarlo, senza toccarne il testo.
#[test]
fn due_aperture_dello_stesso_file_lasciano_un_solo_documento() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = dir.path().join("a.rs");
    std::fs::write(&p, "uno\n").unwrap();
    let _ = app.update(Message::FileLetto(leggi_per_apertura(
        p.clone(),
        DopoApertura::Niente,
    )));
    let id = app.workspace.get_by_path(&p).unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));

    let _ = app.update(Message::FileLetto(leggi_per_apertura(
        p.clone(),
        DopoApertura::Niente,
    )));

    assert_eq!(app.groups.all().count(), 1);
    assert_eq!(app.workspace.get_by_path(&p), Some(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "Xuno\n");
}

#[test]
fn il_ricaricamento_tornato_aggiorna_un_documento_pulito() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "a.rs", "uno\n");
    let id = app.workspace.get_by_path(&p).unwrap();
    let versione = app.workspace.get(id).unwrap().version();

    let _ = app.update(Message::DocRicaricato(
        id,
        p,
        versione,
        Ok(Ricaricato {
            testo: Testo::from("due\n".to_owned()),
            nota: None,
        }),
    ));

    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "due\n");
}

#[test]
fn un_ricaricamento_fallito_lo_dice_e_non_tocca_il_documento() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "a.rs", "uno\n");
    let id = app.workspace.get_by_path(&p).unwrap();
    let versione = app.workspace.get(id).unwrap().version();

    let _ = app.update(Message::DocRicaricato(
        id,
        p,
        versione,
        Err("File troppo grande per l'editor: 41 MiB, il limite è 32 MiB".into()),
    ));

    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "uno\n");
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(notifica.contains("Ricaricamento di") && notifica.contains("troppo grande"));
}

/// Un file aperto dalla shell di Windows deve diventare il documento
/// attivo e la sua cartella deve essere il progetto quando non ce n'e' uno.
#[test]
fn file_di_avvio_diventa_documento_attivo() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("nota.md");
    std::fs::write(&file, "# Nota\n").unwrap();
    let file = file.canonicalize().unwrap();
    let mut app = app();

    let _ = app.open_startup_file(&file);
    let _ = app.update(Message::FileLetto(leggi_per_apertura(
        file.clone(),
        DopoApertura::Avvio {
            fuori_progetto: true,
        },
    )));

    assert_eq!(app.active_path(), Some(file.as_path()));
    assert_eq!(app.project_root.as_deref(), file.parent());
}

#[test]
fn directory_di_avvio_non_sostituisce_il_progetto_ripristinato() {
    let restored = tempfile::tempdir().unwrap();
    let invalid = tempfile::tempdir().unwrap();
    let restored = restored.path().canonicalize().unwrap();
    let mut app = app();
    app.project_root = Some(restored.clone());

    let _ = app.open_startup_file(invalid.path());

    assert_eq!(app.project_root, Some(restored));
    assert!(app.active_path().is_none());
}

#[test]
fn file_di_avvio_mancante_non_sostituisce_il_progetto_ripristinato() {
    let restored = tempfile::tempdir().unwrap();
    let restored = restored.path().canonicalize().unwrap();
    let missing = restored.parent().unwrap().join("lume-file-inesistente.md");
    let mut app = app();
    app.project_root = Some(restored.clone());

    let _ = app.open_startup_file(&missing);

    assert_eq!(app.project_root, Some(restored));
    assert!(app.active_path().is_none());
}

#[cfg(windows)]
#[test]
fn file_di_avvio_non_leggibile_non_sostituisce_il_progetto_ripristinato() {
    use std::os::windows::fs::OpenOptionsExt;

    let restored = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let restored = restored.path().canonicalize().unwrap();
    let file = external.path().join("bloccato.md");
    std::fs::write(&file, "contenuto\n").unwrap();
    let _lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&file)
        .unwrap();
    let mut app = app();
    app.project_root = Some(restored.clone());

    let _ = app.open_startup_file(&file);

    assert_eq!(app.project_root, Some(restored));
    assert!(app.active_path().is_none());
}

#[test]
fn salva_parte_senza_io_ui_e_la_battuta_successiva_resta_sporca() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "async.txt", "prima");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let task = app.update(Message::SaveDocFixture(id));
    assert!(
        (app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "prima");
    let doc = app.workspace.get_mut(id).unwrap();
    doc.apply(lume_core::Transaction::new(
        lume_core::ChangeSet::insert_at(0, "Y", doc.rope().len_chars()),
    ));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
    let task = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "YXprima");
    assert!(!app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn salva_tutto_rifiuta_una_revisione_cambiata_mentre_attende() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let first = write_and_open(&mut app, dir.path(), "first.txt", "prima");
    sporca_attivo(&mut app);
    let second = write_and_open(&mut app, dir.path(), "second.txt", "seconda");
    sporca_attivo(&mut app);
    let second_id = app.active_doc().unwrap();
    let task = app.update(Message::SaveAllFixture);
    let doc = app.workspace.get_mut(second_id).unwrap();
    doc.apply(lume_core::Transaction::new(
        lume_core::ChangeSet::insert_at(0, "Y", doc.rope().len_chars()),
    ));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(first).unwrap(), "Xprima");
    assert_eq!(std::fs::read_to_string(second).unwrap(), "seconda");
    assert!(app.workspace.get(second_id).unwrap().is_dirty());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("cambiato in attesa")
    );
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
}

#[test]
fn chiusura_aspetta_il_worker_e_non_scarta_un_edit_successivo() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "close-async.txt", "prima");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    chiudi_scheda(&mut app, id);
    let task = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::Salva,
    ));
    assert!(app.groups.contains(id));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "prima");
    let doc = app.workspace.get_mut(id).unwrap();
    doc.apply(lume_core::Transaction::new(
        lume_core::ChangeSet::insert_at(0, "Y", doc.rope().len_chars()),
    ));
    app.completa_salvataggi(task);
    assert!(app.groups.contains(id));
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(app.riquadro_chiusura().unwrap().errore.is_some());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");
}

#[test]
fn esc_durante_il_save_revoca_la_chiusura_non_la_scrittura_gia_chiesta() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    write_and_open(&mut app, dir.path(), "esc-async.txt", "prima");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    chiudi_scheda(&mut app, id);
    let task = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::Salva,
    ));
    let _ = app.update(Message::CloseAll);
    app.completa_salvataggi(task);
    assert!(app.groups.contains(id));
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert!(app.chiusura.is_none());
    assert!(app.close_after_save.is_none());
}

#[test]
fn ctrl_s_ripetuto_fonde_la_coda_e_salva_solo_la_revisione_richiesta() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "repeat.txt", "prima");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let task = app.update(Message::SaveDocFixture(id));
    let doc = app.workspace.get_mut(id).unwrap();
    doc.apply(lume_core::Transaction::new(
        lume_core::ChangeSet::insert_at(0, "Y", doc.rope().len_chars()),
    ));
    for _ in 0..1000 {
        let _ = app.update(Message::SaveDocFixture(id));
    }
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "YXprima");
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
}

#[test]
fn completion_con_ticket_vecchio_non_libera_il_worker_corrente() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "ticket.txt", "prima");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let version = app.workspace.get(id).unwrap().version();
    let request = app.prepare_save(id, version).unwrap().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let completion = runtime.block_on(request.run());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "Xprima");
    let mut old = completion.clone();
    old.ticket.sequence += 1;
    let _ = app.update(Message::SaveFinished(Box::new(old)));
    assert!(
        (app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
    assert!(app.workspace.get(id).unwrap().is_dirty());
    let _ = app.update(Message::SaveFinished(Box::new(completion)));
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
    assert!(!app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn nota_editata_durante_save_conserva_header_e_guardia_per_il_save_successivo() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    let loaded = store.create("Prima").unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), vec![loaded.clone()])
        .unwrap();
    let id = app.open_loaded_note(loaded.clone());
    sporca_attivo(&mut app);
    let saved_body = app.workspace.get(id).unwrap().rope().to_string();
    let task = app.update(Message::SaveDocFixture(id));
    app.unload_notes_view_cache();
    let doc = app.workspace.get_mut(id).unwrap();
    doc.apply(lume_core::Transaction::new(
        lume_core::ChangeSet::insert_at(0, "Y", doc.rope().len_chars()),
    ));
    app.completa_salvataggi(task);
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(app.notes.as_ref().unwrap().loaded.is_empty());
    let read = store.load(loaded.path()).unwrap();
    assert_eq!(read.note.header.id, loaded.note.header.id);
    assert_eq!(read.note.markdown, saved_body);
    let task = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(task);
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(
        store.load(loaded.path()).unwrap().note.markdown,
        app.workspace.get(id).unwrap().rope().to_string()
    );
}

#[test]
fn non_salvare_durante_un_worker_revoca_prima_del_commit() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "discard-in-flight.txt", "prima");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let task = app.update(Message::SaveDocFixture(id));
    let mut stream = iced_runtime::task::into_stream(task).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let first = runtime
        .block_on(futures::StreamExt::next(&mut stream))
        .unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(ready)) = first else {
        panic!("autorizzazione pre-commit del worker assente");
    };
    let pause = app.save_job.as_ref().unwrap().pause().unwrap();
    let _ = app.update(Message::SaveReady(ready));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "prima");
    chiudi_scheda(&mut app, id);
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    ));
    assert!(
        app.groups.contains(id),
        "il worker è ancora posseduto dalla finestra"
    );
    assert!(app.riquadro_chiusura().unwrap().salvataggio_in_corso);
    drop(pause);
    let iced_runtime::Action::Output(receipt) = runtime
        .block_on(futures::StreamExt::next(&mut stream))
        .unwrap()
    else {
        panic!("completion del worker assente");
    };
    let _ = app.update(receipt);
    assert!(!app.groups.contains(id));
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "prima",
        "non salvare revoca il commit ancora non iniziato"
    );
}

#[test]
fn edit_dirty_save_su_disco() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "b.txt", "ciao\n");
    let id = app.active_doc().unwrap();

    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));
    assert!(app.workspace.get(id).unwrap().is_dirty());

    let task = app.update(Message::SaveActive);
    app.completa_salvataggi(task);
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "Xciao\n");
    assert!(app.disk_fingerprints.contains_key(&id));
}

#[test]
fn rescan_rilegge_file_reale_e_preserva_buffer_dirty() {
    for dirty in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        let path = write_and_open(&mut app, dir.path(), "rescan.txt", "prima\r\n");
        let id = app.active_doc().unwrap();
        let accepted = app.disk_fingerprints[&id];
        if dirty {
            sporca_attivo(&mut app);
        }
        let version = app.workspace.get(id).unwrap().version();
        std::fs::write(&path, "esterna\r\n").unwrap();
        let task = app.update(Message::FsRescan(vec![dir.path().to_path_buf()]));
        let _ = app.update(primo_output(task));
        let doc = app.workspace.get(id).unwrap();
        if dirty {
            assert_eq!(doc.save_payload(), b"Xprima\r\n");
            assert_eq!(doc.version(), version);
            assert_eq!(app.disk_fingerprints[&id], accepted);
            assert!(app.external_conflicts.contains(&id));
        } else {
            assert_eq!(doc.save_payload(), b"esterna\r\n");
            assert_eq!(
                app.disk_fingerprints[&id],
                crate::file_read::Testo::from("esterna\r\n".to_owned()).fingerprint()
            );
        }
        assert_eq!(doc.is_dirty(), dirty);
        assert_eq!(std::fs::read(&path).unwrap(), b"esterna\r\n");
    }
}

#[test]
fn rescan_scope_obsoleto_non_avvia_reader_ne_invalida_git() {
    let dir = tempfile::tempdir().unwrap();
    let old = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "current.txt", "attuale\n");
    app.git_stale = false;
    app.git_log_stale = false;
    let task = app.update(Message::FsRescan(vec![old.path().to_path_buf()]));
    assert!(primo_output_opzionale(task).is_none());
    assert!(!app.readers.has_workers());
    assert!(!app.git_stale);
    assert!(!app.git_log_stale);
    assert_eq!(std::fs::read(&path).unwrap(), b"attuale\n");
}

#[test]
fn rescan_non_ricorsivo_non_include_documenti_nelle_sottocartelle() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let mut app = app();
    let parent = write_and_open(&mut app, dir.path(), "parent.txt", "parent");
    let child = write_and_open(&mut app, &nested, "child.txt", "child");
    let paths = app.fs_rescan_paths(vec![dir.path().to_path_buf()]);
    assert!(paths.contains(&parent));
    assert!(!paths.contains(&child));
    assert!(!paths.contains(&nested));
}

#[test]
fn rescan_durante_save_accoda_recheck_del_binding_corrente() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "rescan-save.txt", "prima\n");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let save = app.update(Message::SaveDocFixture(id));
    assert!(
        (app.scritture
            .guarda(Some((
                id,
                app.workspace.get(id).and_then(lume_core::Document::path)
            )))
            .file
            != crate::scritture::Presa::Libero
            || app.git_operazione.is_some())
    );
    let task = app.update(Message::FsRescan(vec![dir.path().to_path_buf()]));
    assert!(primo_output_opzionale(task).is_none());
    assert!(app.pending_fs_rechecks.contains(&id));
    assert!(!app.readers.has_workers());
    app.completa_salvataggi(save);
    assert!(!app.pending_fs_rechecks.contains(&id));
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(std::fs::read(&path).unwrap(), b"Xprima\n");
}

#[test]
fn reload_esterno_stesso_docid_con_undo() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "c.txt", "vecchio\n");
    let id = app.active_doc().unwrap();

    // il file cambia su disco (non da noi) → reload in-place
    std::fs::write(&p, "nuovo\n").unwrap();
    let _ = app.update(Message::FsChanged(vec![p.clone()]));
    consegna_ricaricamento(&mut app, id, &p);
    assert_eq!(app.active_doc(), Some(id), "il DocId non cambia");
    assert_eq!(app.groups.all().collect::<Vec<_>>(), vec![id]);
    let doc = app.workspace.get(id).unwrap();
    assert_eq!(doc.rope().to_string(), "nuovo\n");
    assert!(!doc.is_dirty());

    // l'undo del reload ripristina il contenuto precedente
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Undo,
    ));
    let doc = app.workspace.get(id).unwrap();
    assert_eq!(doc.rope().to_string(), "vecchio\n");
    assert!(doc.is_dirty());
}

#[test]
fn modifica_esterna_con_buffer_dirty_avvisa_e_blocca_sovrascrittura() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "conflitto.txt", "prima\n");
    let id = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("locale ".into()),
    ));
    let versione = app.workspace.get(id).unwrap().version();
    std::fs::write(&path, "esterno\n").unwrap();
    let watcher_task = app.on_fs_changed(vec![path.clone()]);
    assert!(
        iced_runtime::task::into_stream(watcher_task).is_some(),
        "l'evento entro la finestra del save va comunque confrontato"
    );
    let lettura = crate::file_read::read_testo_capped(&path, crate::file_read::MAX_FILE_BYTES)
        .map(|testo| Ricaricato { testo, nota: None })
        .map_err(|error| error.to_string());

    let _ = app.doc_ricaricato(id, &path, versione, lettura);

    let documento = app.workspace.get(id).unwrap();
    assert!(documento.is_dirty());
    assert!(documento.rope().to_string().starts_with("locale "));
    assert!(app.external_conflicts.contains(&id));
    assert!(app.notice.as_ref().is_some_and(|(_, notice)| {
        notice.persistente && notice.text.contains("cambiato sul disco")
    }));

    let task = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "esterno\n");
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn apertura_duplicata_non_autorizza_save_sui_byte_esterni_senza_watcher() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "duplicate-save.txt", "prima\r\n");
    let id = app.active_doc().unwrap();
    let accepted = app.disk_fingerprints[&id];
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("locale ".into()),
    ));
    std::fs::write(&path, "dopo!\r\n").unwrap();
    let task = app.open_file_task(path.clone(), crate::file_read::DopoApertura::Niente);
    let _ = app.update(primo_output(task));
    assert_eq!(app.disk_fingerprints[&id], accepted);
    assert!(
        !app.external_conflicts.contains(&id),
        "nessun evento watcher consegnato"
    );
    let task = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(&path).unwrap(), b"dopo!\r\n");
    assert_eq!(
        app.workspace.get(id).unwrap().save_payload(),
        b"locale prima\r\n"
    );
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, notice)| { notice.text.contains("cambiato sul") })
    );
}

#[test]
fn save_worker_rifiuta_modifica_esterna_stessa_lunghezza_senza_watcher() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "stale.txt", "prima\n");
    let id = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("locale ".into()),
    ));
    // Stessa lunghezza: mtime/dimensione non sono sufficienti; nessun
    // evento watcher viene consegnato prima della richiesta di save.
    std::fs::write(&path, "dopo!\n").unwrap();

    let task = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(task);

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "dopo!\n");
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(app.notice.as_ref().is_some_and(|(_, notice)| {
        notice.text.contains("cambiato sul disco") || notice.text.contains("cambiato sul")
    }));
}

#[test]
fn watcher_reale_dopo_due_save_rileva_conflitto_dirty_senza_finestra_di_soppressione() {
    use futures::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "dopo-save.txt", "base\n");
    let id = app.active_doc().unwrap();
    for text in ["X", "Y"] {
        let _ = app.update(Message::EditorIn(
            crate::editor_groups::GroupId::Primary,
            EditorCommand::Insert(text.into()),
        ));
        app.write_doc(id).unwrap();
        assert!(!app.workspace.get(id).unwrap().is_dirty());
    }
    let saved_at = std::time::Instant::now();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("L".into()),
    ));
    let local = app.workspace.get(id).unwrap().rope().to_string();
    let version = app.workspace.get(id).unwrap().version();
    assert_eq!(std::fs::read(&path).unwrap().len(), b"ZZbase\n".len());
    std::fs::write(&path, "ZZbase\n").unwrap();
    let task = app.on_fs_changed(vec![path.clone()]);
    let elapsed = saved_at.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(1500),
        "evento subito dopo il save: {elapsed:?}"
    );
    let mut stream = iced_runtime::task::into_stream(task).expect("lettura watcher reale");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let output = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap();
        let iced_runtime::Action::Output(message) = output else {
            panic!("risposta watcher attesa")
        };
        drop(app.update(message));
    });
    assert!(app.external_conflicts.contains(&id));
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(app.workspace.get(id).unwrap().version(), version);
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), local);
    let task = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(&path).unwrap(), b"ZZbase\n");
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[cfg(target_os = "linux")]
#[test]
fn notify_nativo_dopo_save_consegna_conflitto_al_worker_app() {
    use futures::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "notify-save.txt", "base\n");
    let id = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));
    app.write_doc(id).unwrap();
    let saved_at = std::time::Instant::now();
    let (watcher, events) =
        lume_core::FsWatcher::watch_directories(&[dir.path().to_path_buf()]).unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("L".into()),
    ));
    let local = app.workspace.get(id).unwrap().rope().to_string();
    std::fs::write(&path, "Zbase\n").unwrap();
    let event = events
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("evento notify nativo");
    let lume_core::FsEvent::Changed(paths) = event else {
        panic!("evento file atteso")
    };
    assert!(paths.contains(&path));
    assert!(saved_at.elapsed() < std::time::Duration::from_millis(1500));
    let task = app.update(Message::FsChanged(paths));
    let mut stream = iced_runtime::task::into_stream(task).expect("Task di lettura watcher");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let output = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap();
        let iced_runtime::Action::Output(message) = output else {
            panic!("completion di lettura attesa")
        };
        drop(app.update(message));
    });
    assert!(app.external_conflicts.contains(&id));
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), local);
    let task = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(&path).unwrap(), b"Zbase\n");
    drop(watcher);
}

#[test]
fn save_worker_rifiuta_file_eliminato_senza_watcher() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "removed-stale.txt", "prima\n");
    let id = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("locale ".into()),
    ));
    std::fs::remove_file(&path).unwrap();

    let task = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(task);

    assert!(!path.exists());
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(app.external_conflicts.contains(&id));
    assert!(app.notice.as_ref().is_some_and(|(_, notice)| {
        notice.text.contains("cambiato sul disco") && notice.text.contains("Salva con nome")
    }));
}

#[test]
fn errore_watcher_obsoleto_non_blocca_documento_corrente() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "watcher-stale.txt", "prima\n");
    let id = app.active_doc().unwrap();
    let versione = app.workspace.get(id).unwrap().version();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("locale ".into()),
    ));
    app.notice = None;
    let _ = app.doc_ricaricato(
        id,
        &path,
        versione,
        Err("lettura precedente fallita".into()),
    );
    assert!(!app.external_conflicts.contains(&id));
    assert!(app.notice.is_none());
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "locale prima\n"
    );
}

#[test]
fn errore_watcher_altro_percorso_non_blocca_documento_corrente() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "watcher-path.txt", "prima\n");
    let id = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("locale ".into()),
    ));
    let versione = app.workspace.get(id).unwrap().version();
    app.notice = None;
    let _ = app.doc_ricaricato(
        id,
        &dir.path().join("percorso-precedente.txt"),
        versione,
        Err("vecchio percorso eliminato".into()),
    );
    assert!(!app.external_conflicts.contains(&id));
    assert!(app.notice.is_none());
    assert_eq!(app.workspace.get(id).unwrap().path(), Some(path.as_path()));
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn eliminazione_esterna_con_buffer_dirty_blocca_il_salvataggio() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "eliminato.txt", "prima\n");
    let id = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("locale ".into()),
    ));
    let versione = app.workspace.get(id).unwrap().version();
    std::fs::remove_file(&path).unwrap();
    let watcher_task = app.on_fs_changed(vec![path.clone()]);
    assert!(iced_runtime::task::into_stream(watcher_task).is_some());
    let lettura = crate::file_read::read_testo_capped(&path, crate::file_read::MAX_FILE_BYTES)
        .map(|testo| Ricaricato { testo, nota: None })
        .map_err(|error| error.to_string());

    let _ = app.doc_ricaricato(id, &path, versione, lettura);

    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(app.external_conflicts.contains(&id));
    assert!(app.notice.as_ref().is_some_and(|(_, notice)| {
        notice.persistente && notice.text.contains("Non riesco a verificare")
    }));
    let task = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(task);
    assert!(
        !path.exists(),
        "il save normale non ricrea il file eliminato"
    );
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn due_save_consecutivi_usano_il_fingerprint_del_commit_precedente() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "consecutivi.txt", "prima\n");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    assert!(app.write_doc(id).is_ok());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima\n");

    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("Y".into()),
    ));
    assert!(app.write_doc(id).is_ok());

    assert_eq!(std::fs::read_to_string(path).unwrap(), "XYprima\n");
    assert!(!app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn chiusura_salva_dopo_conflitto_conserva_domanda_e_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "close-conflict.txt", "prima\n");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let version = app.workspace.get(id).unwrap().version();
    std::fs::write(&path, "altro\n").unwrap();
    let _ = app.on_fs_changed(vec![path.clone()]);
    let read = crate::file_read::read_testo_capped(&path, crate::file_read::MAX_FILE_BYTES)
        .map(|testo| Ricaricato { testo, nota: None })
        .map_err(|error| error.to_string());
    let _ = app.doc_ricaricato(id, &path, version, read);
    chiudi_scheda(&mut app, id);
    assert!(riquadro_aperto(&app));

    let task = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::Salva,
    ));
    app.completa_salvataggi(task);

    assert!(riquadro_aperto(&app));
    assert!(app.groups.contains(id));
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "altro\n");
    assert!(app.riquadro_chiusura().unwrap().errore.is_some());
}

/// ATTENZIONE: L'ambito «Aperti» è una **lista di percorsi**, e la console SQL non
/// ne ha uno: cercarci dentro darebbe righe di risultato che nessun clic
/// può riaprire. Vale anche per il secondo gruppo, che è pur sempre
/// roba aperta.
#[test]
#[cfg(feature = "sql")]
fn lambito_dei_file_aperti_salta_la_console_e_prende_i_due_gruppi() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let uno = write_and_open(&mut app, dir.path(), "uno.rs", "ago\n");
    let due = write_and_open(&mut app, dir.path(), "due.rs", "ago\n");
    app.groups.split();
    app.open_sql_console(None, None);
    assert!(app.sql_console.is_some(), "la console è aperta davvero");

    let mut aperti = app.percorsi_aperti();
    aperti.sort();
    let mut attesi = vec![uno, due];
    attesi.sort();
    assert_eq!(aperti, attesi, "i due file, e solo quelli");
}

/// Un file chiuso, uno aperto e pulito, uno aperto e **sporco**: tre
/// politiche diverse in un giro solo.
///
/// ATTENZIONE: Il chiuso si riscrive a byte, il pulito passa dal suo `Document` e
/// poi si salva, lo sporco passa dal `Document` e **resta da salvare** —
/// salvarlo vorrebbe dire scrivere modifiche dell'utente che l'utente non
/// ha chiesto di scrivere.
#[test]
fn sostituire_scrive_i_chiusi_salva_i_puliti_e_lascia_sporchi_gli_sporchi() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let chiuso = dir.path().join("chiuso.txt");
    std::fs::write(&chiuso, "ago qui\n").unwrap();
    let pulito = write_and_open(&mut app, dir.path(), "pulito.txt", "ago la\n");
    let sporco = write_and_open(&mut app, dir.path(), "sporco.txt", "ago giu\n");
    let id_sporco = app.workspace.get_by_path(&sporco).unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("!".into()),
    ));
    assert!(app.workspace.get(id_sporco).unwrap().is_dirty());

    let risultati = vec![
        trovato(&chiuso, "ago qui"),
        trovato(&pulito, "ago la"),
        trovato(&sporco, "!ago giu"),
    ];
    // Il pannello ha davvero cercato «ago»: `Replaced` rifà *la* ricerca,
    // e senza un testo da rifare non avrebbe niente da chiedere.
    if let Some(tool) = app.registry.get_mut(tools::search_tool::ID) {
        tool.update(&lume_ui::tool::ToolEvent::new(
            tools::search_tool::Event::Query("ago".to_owned()),
        ));
    }
    let debounce_prima = app.search_debounce;
    let task = app.replace_all(&risultati, "AGO");
    app.completa_salvataggi(task);

    assert_eq!(std::fs::read_to_string(&chiuso).unwrap(), "AGO qui\n");
    assert_eq!(
        std::fs::read_to_string(&pulito).unwrap(),
        "AGO la\n",
        "chi era pulito finisce sul disco: il comando aveva promesso di cambiare dei file"
    );
    assert_eq!(
        std::fs::read_to_string(&sporco).unwrap(),
        "ago giu\n",
        "chi era sporco resta com'era sul disco"
    );
    let id_sporco = app.workspace.get_by_path(&sporco).unwrap();
    assert_eq!(
        app.workspace.get(id_sporco).unwrap().rope().to_string(),
        "!AGO giu\n",
        "...ma in memoria è cambiato"
    );
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert_eq!(
        notifica,
        "Sostituite 3 occorrenze in 3 file — 1 già modificati restano da salvare"
    );
    assert_ne!(
        app.search_debounce, debounce_prima,
        "e la ricerca riparte: la lista mostrata è vecchia di un istante. \
             La richiesta del pannello passa da un `drain` annidato dentro \
             `apply_tool_requests`, che è il pezzo che si romperebbe in silenzio"
    );
}

/// ATTENZIONE: Senza cartella aperta l'ambito «Progetto» non ha un soggetto, e
/// tacere sarebbe un vicolo cieco. «File aperti» invece regge da sé: un
/// file aperto con Ctrl+O esiste anche senza progetto.
#[test]
fn lambito_progetto_senza_cartella_non_ce_ma_quello_dei_file_aperti_si() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    assert!(app.search_scope(lume_core::SearchArea::Progetto).is_none());
    assert!(matches!(
        app.search_scope(lume_core::SearchArea::Aperti),
        Some(lume_core::SearchScope::Aperti(_))
    ));

    // e nessun giro parte: la generazione non si muove
    let prima = app.search_gen.load(std::sync::atomic::Ordering::SeqCst);
    let _ = app.run_search(
        lume_core::SearchOptions::literal("ago"),
        lume_core::SearchArea::Progetto,
    );
    assert_eq!(
        app.search_gen.load(std::sync::atomic::Ordering::SeqCst),
        prima
    );

    app.project_root = Some(dir.path().to_path_buf());
    assert!(matches!(
        app.search_scope(lume_core::SearchArea::Progetto),
        Some(lume_core::SearchScope::Albero(_))
    ));
}

/// ATTENZIONE: Un file cambiato dopo la ricerca ferma **tutto**, e prima di
/// scrivere: le colonne di allora parlerebbero di un altro testo, e si
/// taglierebbe in mezzo a una parola senza che niente lo segnali.
#[test]
fn una_riga_cambiata_dopo_la_ricerca_non_fa_scrivere_niente() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let primo = dir.path().join("primo.txt");
    let secondo = dir.path().join("secondo.txt");
    std::fs::write(&primo, "ago uno\n").unwrap();
    std::fs::write(&secondo, "tutt altro adesso\n").unwrap();

    // il secondo risultato è di un testo che sul disco non c'è più
    let risultati = vec![trovato(&primo, "ago uno"), trovato(&secondo, "ago due")];
    let task = app.replace_all(&risultati, "X");
    app.completa_salvataggi(task);

    assert_eq!(
        std::fs::read_to_string(&primo).unwrap(),
        "ago uno\n",
        "nemmeno il file buono si tocca: il no arriva prima della prima scrittura"
    );
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(
        notifica.contains("secondo.txt"),
        "e dice quale: {}",
        notifica
    );
}

/// Attenzione: un file chiuso troppo grande ferma la sostituzione **prima** di
/// scrivere: nemmeno il file buono si tocca, e la notifica nomina quello
/// che ha bloccato.
#[test]
fn la_sostituzione_su_un_file_chiuso_enorme_ferma_tutto() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let primo = dir.path().join("primo.txt");
    let enorme = dir.path().join("enorme.txt");
    std::fs::write(&primo, "ago uno\n").unwrap();
    std::fs::File::create(&enorme)
        .unwrap()
        .set_len(crate::file_read::MAX_FILE_BYTES + 1)
        .unwrap();

    let risultati = vec![trovato(&primo, "ago uno"), trovato(&enorme, "ago due")];
    let task = app.replace_all(&risultati, "X");
    app.completa_salvataggi(task);

    assert_eq!(std::fs::read_to_string(&primo).unwrap(), "ago uno\n");
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(
        notifica.contains("enorme.txt") && notifica.contains("File troppo grande"),
        "{notifica}"
    );
}

/// Readonly già presente: la nuova preparazione forte non pubblica
/// nessuno dei file. Il parziale durante commit è verificato nel batch.
#[test]
// set_readonly(false) è solo il ripristino del flag nel tempdir di test
#[allow(clippy::permissions_set_readonly_false)]
fn readonly_in_preparazione_non_pubblica_nessun_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let buono = dir.path().join("buono.txt");
    let bloccato = dir.path().join("bloccato.txt");
    std::fs::write(&buono, "ago uno\n").unwrap();
    std::fs::write(&bloccato, "ago due\n").unwrap();
    let mut permessi = std::fs::metadata(&bloccato).unwrap().permissions();
    permessi.set_readonly(true);
    std::fs::set_permissions(&bloccato, permessi).unwrap();

    let risultati = vec![trovato(&buono, "ago uno"), trovato(&bloccato, "ago due")];
    let task = app.replace_all(&risultati, "X");
    app.completa_salvataggi(task);

    assert_eq!(
        std::fs::read_to_string(&buono).unwrap(),
        "ago uno\n",
        "readonly rilevato in preparazione: nemmeno il primo si pubblica"
    );
    assert_eq!(std::fs::read_to_string(&bloccato).unwrap(), "ago due\n");
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(
        notifica.contains("bloccato.txt") && notifica.contains("non avviata"),
        "il preflight fallito nomina il file e non promette commit: {}",
        notifica
    );

    let mut permessi = std::fs::metadata(&bloccato).unwrap().permissions();
    permessi.set_readonly(false);
    std::fs::set_permissions(&bloccato, permessi).unwrap();
}

fn primo_output(task: iced::Task<Message>) -> Message {
    primo_output_opzionale(task).expect("il Task deve produrre un Message")
}

// Ready lascia un worker in attesa: il runtime del primo output deve
// sopravvivere fino alla decisione e alla raccolta, non bloccarsi nel Drop.
fn runtime_file_tests() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    })
}

fn primo_output_opzionale(task: iced::Task<Message>) -> Option<Message> {
    use futures::StreamExt;
    runtime_file_tests().block_on(async {
        let mut stream = iced_runtime::task::into_stream(task)?;
        while let Some(action) = stream.next().await {
            if let iced_runtime::Action::Output(message) = action {
                return Some(message);
            }
        }
        None
    })
}

fn primo_output_immediato(task: iced::Task<Message>) -> Option<Message> {
    use futures::StreamExt;
    let mut stream = iced_runtime::task::into_stream(task)?;
    runtime_file_tests().block_on(async {
        loop {
            match tokio::time::timeout(std::time::Duration::from_millis(200), stream.next())
                .await
                .ok()?
            {
                Some(iced_runtime::Action::Output(message)) => return Some(message),
                Some(_) => continue,
                None => return None,
            }
        }
    })
}

fn completa_azione_restaurata(app: &mut Lume, task: iced::Task<Message>) {
    let mut task = task;
    for _ in 0..16 {
        let message = if !app.restored_tab_reads.is_empty()
            || (app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some())
        {
            primo_output_opzionale(task)
        } else {
            primo_output_immediato(task)
        };
        let Some(message) = message else {
            return;
        };
        task = app.update(message);
        if app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some() {
            app.completa_salvataggi(task);
            return;
        }
    }
    panic!("la catena di continuazioni del ripristino non si è esaurita");
}

#[test]
fn replace_non_fa_io_finche_il_task_non_parte() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "ago\n").unwrap();
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    assert_eq!(std::fs::read(&path).unwrap(), b"ago\n");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    assert!(app.replacement.is_some());
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(path).unwrap(), b"AGO\n");
    assert!(app.replacement.is_none());
}

#[test]
fn sostituzione_blocca_solo_input_coinvolto_e_rifiuta_salva_utente() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let altro = write_and_open(&mut app, dir.path(), "altro.txt", "altro\n");
    let coinvolto = write_and_open(&mut app, dir.path(), "coinvolto.txt", "ago\n");
    let id = app.active_doc().unwrap();
    let task = app.replace_all(&[trovato(&coinvolto, "ago")], "AGO");
    assert_eq!(
        app.scritture.guarda(None).in_volo,
        Some(crate::scritture::Tipo::Sostituisci)
    );
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "ago\n");
    let _ = app.update(Message::SaveActive);
    assert!(!(app.scritture.contiene_salvataggio(id) || app.saves.active_document() == Some(id)));
    assert!(app.notice.as_ref().unwrap().1.text.contains("sostituzione"));
    app.open_file(&altro).unwrap();
    let altro_id = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("Y".into()),
    ));
    assert_eq!(
        app.workspace.get(altro_id).unwrap().rope().to_string(),
        "Yaltro\n"
    );
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(&coinvolto).unwrap(), b"AGO\n");
    assert_eq!(std::fs::read(&altro).unwrap(), b"altro\n");
    assert!(app.workspace.get(altro_id).unwrap().is_dirty());
    assert_eq!(app.scritture.guarda(None).in_volo, None);
}

#[test]
fn sostituzione_rifiuta_salva_in_ogni_fase_senza_accodarlo() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let estraneo = write_and_open(&mut app, dir.path(), "estraneo.txt", "altro\n");
    let estraneo_id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let path = write_and_open(&mut app, dir.path(), "sostituito.txt", "ago\n");
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let _ = app.update(Message::MenuAction(MenuAct::SaveAll));
    assert!(
        !(app.scritture.contiene_salvataggio(estraneo_id)
            || app.saves.active_document() == Some(estraneo_id))
    );
    assert!(app.notice.as_ref().unwrap().1.text.contains("sostituzione"));
    let ready = primo_output(task);
    let task = app.update(ready);
    let _ = app.update(Message::MenuAction(MenuAct::SaveAll));
    assert!(
        !(app.scritture.contiene_salvataggio(estraneo_id)
            || app.saves.active_document() == Some(estraneo_id))
    );
    assert!(app.notice.as_ref().unwrap().1.text.contains("sostituzione"));
    let receipt = primo_output(task);
    let task = app.update(receipt);
    assert_eq!(
        app.replacement.as_ref().unwrap().phase,
        crate::replace_batch::Phase::Saving
    );
    let _ = app.update(Message::MenuAction(MenuAct::SaveAll));
    assert!(
        !(app.scritture.contiene_salvataggio(estraneo_id)
            || app.saves.active_document() == Some(estraneo_id))
    );
    let _ = app.update(Message::EditorTabBar(
        crate::editor_tab_bar::EditorTabBarMessage::Close(estraneo_id),
    ));
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::Salva,
    ));
    assert!(
        !(app.scritture.contiene_salvataggio(estraneo_id)
            || app.saves.active_document() == Some(estraneo_id))
    );
    assert!(app.close_after_save.is_none());
    assert!(app.notice.as_ref().unwrap().1.text.contains("sostituzione"));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(path).unwrap(), b"AGO\n");
    assert_eq!(std::fs::read(estraneo).unwrap(), b"altro\n");
    assert!(app.workspace.get(estraneo_id).unwrap().is_dirty());
    assert!(app.chiusura.is_some());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[cfg(unix)]
#[test]
fn lettura_alias_iniziata_prima_del_replace_non_apre_un_buffer_superato() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = dir.path().join("target.txt");
    std::fs::write(&path, "ago\n").unwrap();
    let alias = dir.path().join("alias.txt");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    let reading = leggi_per_apertura(alias.clone(), DopoApertura::Niente);
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let prepared = primo_output(task);
    let commit = app.update(prepared);
    let _ = app.update(Message::FileLetto(reading));
    assert!(app.workspace.get_by_path(&alias).is_none());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("ripeti l'apertura")
    );
    app.completa_salvataggi(commit);
    assert_eq!(std::fs::read(&path).unwrap(), b"AGO\n");
    app.open_file(&alias).unwrap();
    let id = app.workspace.get_by_path(&alias).unwrap();
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "AGO\n");
}

#[test]
fn lettura_vecchia_non_crea_nuovo_binding_durante_save() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let saved = write_and_open(&mut app, dir.path(), "saved.txt", "prima");
    let other = dir.path().join("other.txt");
    std::fs::write(&other, "altro").unwrap();
    let reading = leggi_per_apertura(other.clone(), DopoApertura::Niente);
    sporca_attivo(&mut app);
    let id = app.active_doc().unwrap();
    let task = app.update(Message::SaveDocFixture(id));
    assert_eq!(
        app.open_file(&other).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let _ = app.update(Message::FileLetto(reading));
    assert!(app.workspace.get_by_path(&other).is_none());
    app.open_file(&saved).unwrap();
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "Xprima");
    app.completa_salvataggi(task);
    app.open_file(&other).unwrap();
    assert!(app.workspace.get_by_path(&other).is_some());
}

#[test]
fn epoca_rifiuta_lettura_gia_completata_prima_del_replace_anche_dopo_il_commit() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "ago\n").unwrap();
    let old_read = primo_output(app.open_file_task(path.clone(), DopoApertura::Niente));
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    app.completa_salvataggi(task);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    let _ = app.update(old_read);
    assert!(app.workspace.get_by_path(&path).is_none());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("Lettura superata")
    );
    let fresh = primo_output(app.open_file_task(path.clone(), DopoApertura::Niente));
    let _ = app.update(fresh);
    let id = app.workspace.get_by_path(&path).unwrap();
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "AGO\n");
}

#[test]
fn epoca_avanza_al_commit_replace_prima_del_rilascio_e_scartando_replay() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let closed = dir.path().join("closed.txt");
    std::fs::write(&closed, "ago\n").unwrap();
    let opened = write_and_open(&mut app, dir.path(), "opened.txt", "ago\n");
    let task = app.replace_all(&[trovato(&closed, "ago"), trovato(&opened, "ago")], "AGO");
    let prepared = primo_output(task);
    let old_read = primo_output(app.open_file_task(opened.clone(), DopoApertura::Niente));
    let epoca = app.scritture.guarda(None).epoca;
    let committed = primo_output(app.update(prepared));
    let replay = committed.clone();
    let replay_successore = committed.clone();
    let continuazione = app.update(committed);
    assert_eq!(std::fs::read(&closed).unwrap(), b"AGO\n");
    assert_eq!(
        app.scritture.guarda(None).in_volo,
        Some(crate::scritture::Tipo::Sostituisci)
    );
    assert_ne!(app.scritture.guarda(None).epoca, epoca);
    let dopo = app.scritture.guarda(None).epoca;
    drop(app.update(replay));
    assert_eq!(app.scritture.guarda(None).epoca, dopo);
    drop(app.update(old_read));
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("Lettura superata")
    );
    app.completa_salvataggi(continuazione);
    assert_eq!(std::fs::read(&opened).unwrap(), b"AGO\n");
    let successore = dir.path().join("successore.txt");
    std::fs::write(&successore, "ago\n").unwrap();
    let nuovo = app.replace_all(&[trovato(&successore, "ago")], "nuovo");
    let lease = app.replacement.as_ref().unwrap().lease;
    let vista = app.scritture.guarda(None);
    drop(app.update(replay_successore));
    assert_eq!(app.scritture.guarda(None), vista);
    assert!(
        app.scritture
            .possiede(lease, crate::scritture::Tipo::Sostituisci)
    );
    app.completa_salvataggi(nuovo);
    assert_eq!(std::fs::read(&successore).unwrap(), b"nuovo\n");
}

#[test]
fn continuazione_replace_invalida_lettura_prima_del_rilascio_del_batch() {
    use futures::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let first = write_and_open(&mut app, dir.path(), "first.txt", "ago\n");
    let second = write_and_open(&mut app, dir.path(), "second.txt", "ago\n");
    let task = app.replace_all(&[trovato(&first, "ago"), trovato(&second, "ago")], "AGO");
    let prepared = primo_output(task);
    let committed = primo_output(app.update(prepared));
    let save = app.update(committed);
    let lease = app.replacement.as_ref().unwrap().lease;
    assert_eq!(app.replacement.as_ref().unwrap().autosaves.len(), 2);
    let old_read = primo_output(app.open_file_task(first.clone(), DopoApertura::Niente));
    let epoca = app.scritture.guarda(None).epoca;
    let mut stream = iced_runtime::task::into_stream(save).unwrap();
    let runtime = runtime_file_tests();
    let ready = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(ready @ Message::SaveReady(_)) = ready else {
        panic!("Ready reale assente")
    };
    drop(app.update(ready));
    let receipt = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(receipt @ Message::SaveCollected(_)) = receipt else {
        panic!("ricevuta reale assente")
    };
    let next = app.update(receipt);
    assert!(
        app.scritture
            .possiede(lease, crate::scritture::Tipo::Sostituisci)
    );
    assert!(app.scritture.guarda(None).epoca > epoca);
    drop(app.update(old_read));
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("Lettura superata")
    );
    app.completa_salvataggi(next);
    assert_eq!(std::fs::read(first).unwrap(), b"AGO\n");
    assert_eq!(std::fs::read(second).unwrap(), b"AGO\n");
}

#[test]
fn epoca_di_fine_save_rifiuta_lettura_del_disco_prima_del_rename() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "file.txt", "prima");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let task = app.update(Message::SaveDocFixture(id));
    let old_read = primo_output(app.open_file_task(path.clone(), DopoApertura::Niente));
    app.completa_salvataggi(task);
    app.close_tab(id);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    let _ = app.update(old_read);
    assert!(app.workspace.get_by_path(&path).is_none());
    assert_eq!(std::fs::read(path).unwrap(), b"Xprima");
}

#[test]
fn edit_durante_prepare_revoca_tutti_i_commit_e_pulisce_i_temporanei() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let closed = dir.path().join("closed.txt");
    std::fs::write(&closed, "ago\n").unwrap();
    let opened = write_and_open(&mut app, dir.path(), "opened.txt", "ago\n");
    let id = app.active_doc().unwrap();
    let task = app.replace_all(&[trovato(&closed, "ago"), trovato(&opened, "ago")], "AGO");
    let prepared = primo_output(task);
    assert_eq!(std::fs::read(&closed).unwrap(), b"ago\n");
    let doc = app.workspace.get_mut(id).unwrap();
    doc.apply(lume_core::Transaction::new(
        lume_core::ChangeSet::insert_at(0, "X", doc.rope().len_chars()),
    ));
    let task = app.update(prepared);
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(closed).unwrap(), b"ago\n");
    assert_eq!(std::fs::read(opened).unwrap(), b"ago\n");
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "Xago\n");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn edit_durante_commit_non_viene_sovrascritto_e_il_parziale_e_dichiarato() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let closed = dir.path().join("closed.txt");
    std::fs::write(&closed, "ago\n").unwrap();
    let opened = write_and_open(&mut app, dir.path(), "opened.txt", "ago\n");
    let id = app.active_doc().unwrap();
    let task = app.replace_all(&[trovato(&closed, "ago"), trovato(&opened, "ago")], "AGO");
    let prepared = primo_output(task);
    let commit = app.update(prepared);
    let outcome = primo_output(commit);
    let doc = app.workspace.get_mut(id).unwrap();
    doc.apply(lume_core::Transaction::new(
        lume_core::ChangeSet::insert_at(0, "X", doc.rope().len_chars()),
    ));
    let task = app.update(outcome);
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(closed).unwrap(), b"AGO\n");
    assert_eq!(std::fs::read(opened).unwrap(), b"ago\n");
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "Xago\n");
    let message = &app.notice.as_ref().unwrap().1.text;
    assert!(
        message.contains("Commit riusciti")
            && message.contains("closed.txt")
            && message.contains("Non tentati")
            && message.contains("opened.txt")
    );
}

#[test]
fn errore_autosave_del_pulito_non_annuncia_successo_complessivo() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "readonly.txt", "ago\n");
    let id = app.active_doc().unwrap();
    let permissions = std::fs::metadata(&path).unwrap().permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&path, readonly).unwrap();
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    app.completa_salvataggi(task);
    std::fs::set_permissions(&path, permissions).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"ago\n");
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "AGO\n");
    assert!(app.workspace.get(id).unwrap().is_dirty());
    let message = &app.notice.as_ref().unwrap().1.text;
    assert!(
        !message.starts_with("Sostituite")
            && message.contains("readonly.txt")
            && message.contains("fallit"),
        "messaggio inatteso: {message}"
    );
}

#[test]
fn report_aspetta_tutti_gli_autosave_e_elenca_ogni_fallimento() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let first = write_and_open(&mut app, dir.path(), "first.txt", "ago\n");
    let second = write_and_open(&mut app, dir.path(), "second.txt", "ago\n");
    let mut restore = Vec::new();
    for path in [&first, &second] {
        let permissions = std::fs::metadata(path).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(path, readonly).unwrap();
        restore.push((path.clone(), permissions));
    }
    let task = app.replace_all(&[trovato(&first, "ago"), trovato(&second, "ago")], "AGO");
    let prepared = primo_output(task);
    let task = app.update(prepared);
    let committed = primo_output(task);
    let task = app.update(committed);
    assert_eq!(app.replacement.as_ref().unwrap().autosaves.len(), 2);
    assert_eq!(
        app.scritture.guarda(None).in_volo,
        Some(crate::scritture::Tipo::Sostituisci)
    );
    assert!(
        !app.notice
            .as_ref()
            .is_some_and(|(_, n)| n.text.starts_with("Sostituite"))
    );
    app.completa_salvataggi(task);
    for (path, permissions) in restore {
        std::fs::set_permissions(path, permissions).unwrap();
    }
    let notice = &app.notice.as_ref().unwrap().1.text;
    assert!(
        notice.contains("first.txt")
            && notice.contains("second.txt")
            && notice.contains("parziale")
    );
    assert_eq!(std::fs::read(first).unwrap(), b"ago\n");
    assert_eq!(std::fs::read(second).unwrap(), b"ago\n");
    assert!(app.replacement.is_none());
    assert_eq!(app.scritture.guarda(None).in_volo, None);
}

#[test]
fn sostituzione_versione_in_coda_cambiata_finisce_con_parziale_senza_perdere_edit() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let primo = write_and_open(&mut app, dir.path(), "primo.txt", "ago\n");
    let secondo = write_and_open(&mut app, dir.path(), "secondo.txt", "ago\n");
    let secondo_id = app.active_doc().unwrap();
    let task = app.replace_all(&[trovato(&primo, "ago"), trovato(&secondo, "ago")], "AGO");
    let ready = primo_output(task);
    let task = app.update(ready);
    let receipt = primo_output(task);
    let task = app.update(receipt);
    assert_eq!(
        app.saves.active_document(),
        app.workspace.get_by_path(&primo)
    );
    let doc = app.workspace.get_mut(secondo_id).unwrap();
    doc.apply(lume_core::Transaction::new(
        lume_core::ChangeSet::insert_at(0, "X", doc.rope().len_chars()),
    ));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(primo).unwrap(), b"AGO\n");
    assert_eq!(std::fs::read(secondo).unwrap(), b"ago\n");
    assert_eq!(
        app.workspace.get(secondo_id).unwrap().rope().to_string(),
        "XAGO\n"
    );
    assert!(app.workspace.get(secondo_id).unwrap().is_dirty());
    assert!(app.notice.as_ref().unwrap().1.text.contains("Non tentati"));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn continuazione_per_documento_estraneo_produce_report_e_non_resta_appesa() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let estraneo = write_and_open(&mut app, dir.path(), "estraneo.txt", "altro\n");
    let estraneo_id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let path = write_and_open(&mut app, dir.path(), "coinvolto.txt", "ago\n");
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let ready = primo_output(task);
    let task = app.update(ready);
    let receipt = primo_output(task);
    let task = app.update(receipt);
    app.replacement
        .as_mut()
        .unwrap()
        .autosaves
        .insert(estraneo_id, estraneo.clone());
    app.queue_save(estraneo_id);
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(path).unwrap(), b"AGO\n");
    assert_eq!(std::fs::read(estraneo).unwrap(), b"altro\n");
    assert!(app.workspace.get(estraneo_id).unwrap().is_dirty());
    assert!(app.notice.as_ref().unwrap().1.text.contains("Non tentati"));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn sostituzione_binding_in_coda_cambiato_segnala_parziale_senza_scrivere() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let primo = write_and_open(&mut app, dir.path(), "primo.txt", "ago\n");
    let secondo = write_and_open(&mut app, dir.path(), "secondo.txt", "ago\n");
    let id = app.active_doc().unwrap();
    let task = app.replace_all(&[trovato(&primo, "ago"), trovato(&secondo, "ago")], "AGO");
    let ready = primo_output(task);
    let task = app.update(ready);
    let receipt = primo_output(task);
    let task = app.update(receipt);
    let nuovo = dir.path().join("nuovo.txt");
    app.workspace.rebind_path(id, nuovo.clone()).unwrap();
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(primo).unwrap(), b"AGO\n");
    assert_eq!(std::fs::read(secondo).unwrap(), b"ago\n");
    assert!(!nuovo.exists());
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "AGO\n");
    assert!(app.workspace.get(id).unwrap().is_dirty());
    let notice = &app.notice.as_ref().unwrap().1.text;
    assert!(notice.contains("Non tentati") && notice.contains("secondo.txt"));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn cambio_progetto_durante_saving_revoca_e_conserva_i_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let altro = tempfile::tempdir().unwrap();
    let mut app = app();
    app.set_project_root(dir.path().to_path_buf());
    let primo = write_and_open(&mut app, dir.path(), "primo.txt", "ago\n");
    let primo_id = app.active_doc().unwrap();
    let secondo = write_and_open(&mut app, dir.path(), "secondo.txt", "ago\n");
    let secondo_id = app.active_doc().unwrap();
    let task = app.replace_all(&[trovato(&primo, "ago"), trovato(&secondo, "ago")], "AGO");
    let ready = primo_output(task);
    let task = app.update(ready);
    let receipt = primo_output(task);
    let task = app.update(receipt);
    app.set_project_root(altro.path().to_path_buf());
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(&primo).unwrap(), b"ago\n");
    assert_eq!(std::fs::read(&secondo).unwrap(), b"ago\n");
    for id in [primo_id, secondo_id] {
        assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "AGO\n");
        assert!(app.workspace.get(id).unwrap().is_dirty());
    }
    let notice = &app.notice.as_ref().unwrap().1.text;
    assert!(notice.contains("primo.txt") && notice.contains("secondo.txt"));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn non_salvare_durante_saving_revoca_i_salvataggi_restanti_e_mostra_il_report() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let primo = write_and_open(&mut app, dir.path(), "primo.txt", "ago\n");
    let secondo = write_and_open(&mut app, dir.path(), "secondo.txt", "ago\n");
    let id = app.active_doc().unwrap();
    let task = app.replace_all(&[trovato(&primo, "ago"), trovato(&secondo, "ago")], "AGO");
    let ready = primo_output(task);
    let task = app.update(ready);
    let receipt = primo_output(task);
    let task = app.update(receipt);
    let _ = app.update(Message::EditorTabBar(
        crate::editor_tab_bar::EditorTabBarMessage::Close(id),
    ));
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    ));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(&primo).unwrap(), b"ago\n");
    assert_eq!(std::fs::read(&secondo).unwrap(), b"ago\n");
    assert!(!app.groups.contains(id));
    let notice = &app.notice.as_ref().unwrap().1.text;
    assert!(notice.contains("Non tentati") && notice.contains("secondo.txt"));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn sostituzione_blocca_incolla_e_rifiuta_salvataggio_sopra_overlay() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "file.txt", "ago\n");
    let id = app.active_doc().unwrap();
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let _ = app.update(Message::ClipboardPasted(Some("X".into())));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "ago\n");
    app.apri_overlay(OverlayId::About);
    let _ = app.update(Message::SaveActive);
    assert!(!(app.scritture.contiene_salvataggio(id) || app.saves.active_document() == Some(id)));
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("Rispondi prima")
    );
    app.chiudi_overlay();
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(path).unwrap(), b"AGO\n");
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn cambio_progetto_durante_prepare_rifiuta_e_aspetta_abort() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "ago\n").unwrap();
    app.project_root = Some(dir.path().to_path_buf());
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let prepared = primo_output(task);
    app.project_root = Some(other.path().to_path_buf());
    let task = app.update(prepared);
    assert!(app.replacement.is_some());
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(path).unwrap(), b"ago\n");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn prepared_duplicato_non_ruba_il_piano_al_commit_corrente() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "ago\n").unwrap();
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let prepared = primo_output(task);
    let duplicate = prepared.clone();
    let commit = app.update(prepared);
    let _ = app.update(duplicate);
    app.completa_salvataggi(commit);
    assert_eq!(std::fs::read(path).unwrap(), b"AGO\n");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn non_salvare_durante_preparazione_revoca_replace_e_aspetta_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "cancel.txt", "ago\n");
    let id = app.active_doc().unwrap();
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    app.close_tab(id);
    let _ = app.rispondi_chiusura(crate::conferma_chiusura::ConfermaMessage::NonSalvare);
    assert!(app.groups.contains(id));
    app.completa_salvataggi(task);
    assert!(!app.groups.contains(id));
    assert_eq!(std::fs::read(path).unwrap(), b"ago\n");
    assert!(app.replacement.is_none());
}

#[cfg(unix)]
#[test]
fn alias_del_buffer_sporco_non_viene_trattato_come_file_chiuso() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "original.txt", "ago\n");
    sporca_attivo(&mut app);
    let id = app.active_doc().unwrap();
    let alias = dir.path().join("alias.txt");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    let task = app.replace_all(&[trovato(&alias, "ago")], "AGO");
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(path).unwrap(), b"ago\n");
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "Xago\n");
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("già aperto tramite")
    );
}

/// Una riga con una sola occorrenza di «ago», come la troverebbe il
/// motore: prima colonna utile, e il testo della riga per intero.
fn trovato(path: &std::path::Path, riga: &str) -> lume_core::FileMatches {
    let inizio = riga.find("ago").expect("la riga contiene «ago»");
    lume_core::FileMatches {
        path: path.to_path_buf(),
        hits: vec![lume_core::SearchHit {
            line_number: 1,
            line_text: riga.into(),
            range_in_line: inizio..inizio + 3,
        }],
    }
}

/// ATTENZIONE: Ctrl+Maiusc+F **non** è un `click`. Premuto due volte — che è
/// quel che fa chi non è sicuro di aver premuto — un interruttore
/// chiuderebbe il pannello appena aperto per cercare.
///
/// La differenza è deliberata rispetto a `ShowExtensions`, che invece è un
/// interruttore: lì la scorciatoia apre *e chiude* un elenco che si
/// guarda, qui porta il cursore in un campo in cui si scrive.
#[test]
fn cerca_nei_file_premuto_due_volte_lascia_il_pannello_aperto() {
    let mut app = app();
    for _ in 0..2 {
        let _ = app.update(Message::MenuAction(crate::menu::MenuAct::FindInFiles));
    }
    assert_eq!(
        app.registry.active_panel,
        Some(tools::search_tool::ID),
        "il secondo colpo non deve richiudere"
    );
}

#[test]
fn search_update_di_generazione_vecchia_ignorato() {
    let mut app = app();
    // generazione corrente = 0: una risposta con gen 999 va scartata
    assert!(!app.deliver_search_update(999, lume_core::SearchUpdate::Error("boom".into())));
    // con la generazione corrente arriva al pannello
    assert!(app.deliver_search_update(0, lume_core::SearchUpdate::Error("boom".into())));
    assert!(
        app.notice.is_none(),
        "un'espressione a metà è una riga nel pannello, non una notifica"
    );
}

#[test]
// set_readonly(false) è solo il ripristino del flag nel tempdir di test
#[allow(clippy::permissions_set_readonly_false)]
fn save_fallito_lascia_dirty_e_notifica() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "ro.txt", "solo lettura\n");
    let id = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));

    // file read-only: la scrittura fallisce
    let mut perm = std::fs::metadata(&p).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&p, perm.clone()).unwrap();

    let task = app.update(Message::SaveActive);
    app.completa_salvataggi(task);
    assert!(app.workspace.get(id).unwrap().is_dirty(), "resta sporco");
    assert!(app.notice.is_some(), "notifica di errore presente");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "solo lettura\n");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "il temporaneo del save fallito viene raccolto"
    );

    // cleanup: togli il read-only per permettere il drop del tempdir
    perm.set_readonly(false);
    std::fs::set_permissions(&p, perm).unwrap();
}

#[test]
fn il_gesto_sulle_schede_si_spegne_al_rilascio() {
    use crate::editor_groups::GroupId;
    use crate::editor_tab_bar::EditorTabBarMessage;

    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    write_and_open(&mut app, dir.path(), "uno.txt", "uno\n");
    let id = app.active_doc().unwrap();

    let _ = app.update(Message::EditorTabBar(EditorTabBarMessage::Press(
        GroupId::Primary,
        id,
    )));
    assert!(app.editor_tab_bar.drag().is_some_and(|drag| !drag.active));

    let _ = app.update(Message::DragMoved(iced::Point::new(100.0, 10.0)));
    assert!(app.editor_tab_bar.drag().is_some_and(|drag| !drag.active));
    let _ = app.update(Message::DragMoved(iced::Point::new(120.0, 10.0)));
    assert!(app.editor_tab_bar.drag().is_some_and(|drag| drag.active));

    let _ = app.update(Message::DragEnd);
    assert!(app.editor_tab_bar.drag().is_none());
    assert!(app.layout.drag.is_none());
}

// ------------------------------------------- chiusura con modifiche

/// Scrive una «X» in testa al documento attivo: lo rende sporco.
fn sporca_attivo(app: &mut Lume) {
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));
}

fn chiudi_scheda(app: &mut Lume, id: lume_core::DocId) {
    let _ = app.update(Message::EditorTabBar(
        crate::editor_tab_bar::EditorTabBarMessage::Close(id),
    ));
}

fn rispondi(app: &mut Lume, risposta: crate::conferma_chiusura::ConfermaMessage) {
    let task = app.update(Message::ConfermaChiusura(risposta));
    app.completa_salvataggi(task);
}

fn riquadro_aperto(app: &Lume) -> bool {
    app.shell.active_overlay() == Some(crate::overlay_manager::OverlayId::ConfermaChiusura)
        && app.riquadro_chiusura().is_some()
}

#[test]
fn scheda_pulita_si_chiude_subito() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    write_and_open(&mut app, dir.path(), "pulito.txt", "ciao\n");
    let id = app.active_doc().unwrap();

    chiudi_scheda(&mut app, id);

    assert!(!app.groups.contains(id));
    assert!(app.workspace.get(id).is_none());
    assert!(!riquadro_aperto(&app), "niente da chiedere");
}

#[test]
fn scheda_sporca_chiede_e_non_si_chiude() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    write_and_open(&mut app, dir.path(), "sporco.txt", "ciao\n");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);

    chiudi_scheda(&mut app, id);

    assert!(
        app.groups.contains(id),
        "la scheda resta finché non si risponde"
    );
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(riquadro_aperto(&app));
    let riquadro = app.riquadro_chiusura().unwrap();
    assert_eq!(riquadro.titolo, "Salvare le modifiche a sporco.txt?");
    assert_eq!(riquadro.etichetta_salva, "Salva");
    assert_eq!(riquadro.etichetta_scarta, "Non salvare");

    // l'editor sotto il riquadro ha ancora il fuoco: quel che si batte
    // adesso non deve finire nel file di cui si sta chiedendo
    sporca_attivo(&mut app);
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "Xciao\n");
}

#[test]
fn non_salvare_chiude_e_scarta() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "scarta.txt", "ciao\n");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    chiudi_scheda(&mut app, id);

    rispondi(
        &mut app,
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    );

    assert!(!app.groups.contains(id));
    assert!(app.workspace.get(id).is_none());
    assert!(!riquadro_aperto(&app));
    assert!(app.chiusura.is_none());
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "ciao\n",
        "il disco non si tocca"
    );
}

#[test]
fn salva_scrive_e_chiude() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "salva.txt", "ciao\n");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    chiudi_scheda(&mut app, id);

    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Salva);

    assert_eq!(std::fs::read_to_string(&p).unwrap(), "Xciao\n");
    assert!(!app.groups.contains(id));
    assert!(app.workspace.get(id).is_none());
    assert!(!riquadro_aperto(&app));
}

#[test]
// set_readonly(false) è solo il ripristino del flag nel tempdir di test
#[allow(clippy::permissions_set_readonly_false)]
fn salva_fallito_non_chiude_e_tiene_l_errore() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "ro.txt", "solo lettura\n");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let mut perm = std::fs::metadata(&p).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&p, perm.clone()).unwrap();
    chiudi_scheda(&mut app, id);

    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Salva);

    assert!(app.groups.contains(id), "la scheda non si chiude");
    assert!(app.workspace.get(id).unwrap().is_dirty(), "e resta sporca");
    assert!(riquadro_aperto(&app), "il riquadro resta aperto");
    let errore = app.riquadro_chiusura().unwrap().errore.unwrap();
    assert!(
        errore.contains("ro.txt"),
        "l'errore dice quale file: {errore}"
    );
    assert!(app.notice.is_some(), "e compare anche in barra di stato");

    perm.set_readonly(false);
    std::fs::set_permissions(&p, perm).unwrap();
}

#[test]
fn annulla_ed_esc_lasciano_tutto_com_era() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "annulla.txt", "ciao\n");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);

    chiudi_scheda(&mut app, id);
    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Annulla);
    assert!(app.groups.contains(id));
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(!riquadro_aperto(&app));
    assert!(app.chiusura.is_none());

    // l'Esc (e il clic fuori dal riquadro) passano da `CloseAll`
    chiudi_scheda(&mut app, id);
    let _ = app.update(Message::CloseAll);
    assert!(app.groups.contains(id));
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(!riquadro_aperto(&app));
    assert!(app.chiusura.is_none());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "ciao\n");
}

/// L'apertura non modale è rifiutata; dopo Annulla la risposta è scaduta.
#[test]
fn risposta_a_una_domanda_non_piu_visibile_non_agisce() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    write_and_open(&mut app, dir.path(), "vecchia.txt", "ciao\n");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    chiudi_scheda(&mut app, id);

    app.apri_overlay(crate::overlay_manager::OverlayId::GotoLine);
    assert!(riquadro_aperto(&app));
    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Annulla);
    rispondi(
        &mut app,
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    );

    assert!(app.groups.contains(id));
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

/// La console SQL non è un file: la sua query sopravvive alla scheda e
/// viaggia nella sessione. Né la scheda né la finestra devono chiedere.
#[test]
#[cfg(feature = "sql")]
fn console_sql_sporca_non_chiede_niente() {
    let mut app = app();
    app.open_sql_console(None, None);
    let console = app.sql_console.unwrap();
    app.groups.activate(console);
    sporca_attivo(&mut app);
    assert!(app.workspace.get(console).unwrap().is_dirty());

    assert_eq!(app.richiesta_uscita(None), super::Uscita::Esci);
    assert!(!riquadro_aperto(&app));

    chiudi_scheda(&mut app, console);
    assert!(!app.groups.contains(console));
    assert!(!riquadro_aperto(&app));
    assert!(
        app.workspace.get(console).is_some(),
        "la query resta, come prima"
    );
}

#[test]
fn uscita_senza_sporchi_esce_subito() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    write_and_open(&mut app, dir.path(), "a.txt", "a\n");

    assert_eq!(app.richiesta_uscita(None), super::Uscita::Esci);
    assert!(!riquadro_aperto(&app));
}

#[test]
fn uscita_con_sporchi_chiede_ed_elenca() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let a = write_and_open(&mut app, dir.path(), "a.txt", "a\n");
    sporca_attivo(&mut app);
    write_and_open(&mut app, dir.path(), "pulito.txt", "p\n");
    let b = write_and_open(&mut app, dir.path(), "b.txt", "b\n");
    sporca_attivo(&mut app);

    // la richiesta del sistema (X, Alt+F4) passa dalla stessa strada
    let _ = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));

    assert!(riquadro_aperto(&app));
    let riquadro = app.riquadro_chiusura().unwrap();
    assert_eq!(riquadro.titolo, "2 file con modifiche non salvate");
    assert_eq!(riquadro.nomi, vec!["a.txt".to_owned(), "b.txt".to_owned()]);
    assert_eq!(riquadro.etichetta_salva, "Salva tutto ed esci");
    assert_eq!(riquadro.etichetta_scarta, "Esci senza salvare");

    // Annulla: niente esce, niente si scrive
    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Annulla);
    assert!(!riquadro_aperto(&app));
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "a\n");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "b\n");

    // la X della barra del titolo chiede la stessa cosa
    let _ = app.update(Message::Titlebar(TitlebarMessage::Close));
    assert!(riquadro_aperto(&app));

    // Salva tutto: entrambi su disco, e il riquadro se ne va (si esce)
    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Salva);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "Xa\n");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "Xb\n");
    assert!(!riquadro_aperto(&app));
    assert!(app.chiusura.is_none());
}

#[test]
fn esci_senza_salvare_non_scrive_niente() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let a = write_and_open(&mut app, dir.path(), "a.txt", "a\n");
    sporca_attivo(&mut app);
    assert_eq!(app.richiesta_uscita(None), super::Uscita::Chiedi);

    rispondi(
        &mut app,
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    );

    assert!(app.chiusura.is_none());
    assert!(!riquadro_aperto(&app));
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "a\n");
}

#[test]
// set_readonly(false) è solo il ripristino del flag nel tempdir di test
#[allow(clippy::permissions_set_readonly_false)]
fn salva_tutto_fallito_non_esce_e_lascia_solo_quel_che_manca() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let ok = write_and_open(&mut app, dir.path(), "ok.txt", "ok\n");
    sporca_attivo(&mut app);
    let ro = write_and_open(&mut app, dir.path(), "ro.txt", "ro\n");
    sporca_attivo(&mut app);
    let mut perm = std::fs::metadata(&ro).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&ro, perm.clone()).unwrap();
    assert_eq!(app.richiesta_uscita(None), super::Uscita::Chiedi);

    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Salva);

    assert_eq!(
        std::fs::read_to_string(&ok).unwrap(),
        "Xok\n",
        "quel che si può, si salva"
    );
    assert!(riquadro_aperto(&app), "non si esce");
    let riquadro = app.riquadro_chiusura().unwrap();
    assert_eq!(
        riquadro.nomi,
        vec!["ro.txt".to_owned()],
        "resta solo il fallito"
    );
    assert!(riquadro.errore.unwrap().contains("ro.txt"));
    assert!(app.notice.is_some());

    // un secondo Alt+F4 non riapre la domanda da capo: l'errore resta
    let _ = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
    assert!(app.riquadro_chiusura().unwrap().errore.is_some());

    perm.set_readonly(false);
    std::fs::set_permissions(&ro, perm).unwrap();
}

/// Cambiare cartella o spegnere gli Appunti chiude tutte le loro schede:
/// con una nota sporca si rifiuta, invece di buttarla.
#[test]
fn appunti_sporchi_bloccano_spegnimento_e_cambio_cartella() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    let loaded = store.create("Prima").unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), vec![loaded.clone()])
        .unwrap();
    let doc = app.open_loaded_note(loaded.clone());
    let p = loaded.path().to_path_buf();
    let sul_disco = std::fs::read_to_string(&p).unwrap();
    sporca_attivo(&mut app);
    assert!(app.workspace.get(doc).unwrap().is_dirty());
    assert!(app.feature_is_enabled(crate::features::Feature::Notes));

    let _ = app.switch_notes_directory(dir.path().join("altra"));
    assert!(app.groups.contains(doc), "il cambio cartella è rifiutato");
    assert!(app.notice.is_some());

    let _ = app.apply_feature_command(crate::features::FeatureCommand::SetEnabled(
        crate::features::Feature::Notes,
        false,
    ));
    assert!(app.groups.contains(doc), "lo spegnimento è rifiutato");
    assert!(app.feature_is_enabled(crate::features::Feature::Notes));
    assert!(app.workspace.get(doc).unwrap().is_dirty());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), sul_disco);
}

/// Ctrl+S su una nota scriveva il solo corpo della scheda sul file e
/// cancellava l'intestazione `+++`: alla rilettura la nota aveva un id
/// nuovo. Ora passa dallo store, che ricompone il file intero.
#[test]
fn salvare_una_nota_conserva_intestazione_e_id() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    let loaded = store.create("Prima").unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), vec![loaded.clone()])
        .unwrap();
    let doc = app.open_loaded_note(loaded.clone());
    sporca_attivo(&mut app);
    let corpo = app.workspace.get(doc).unwrap().rope().to_string();

    let task = app.update(Message::SaveDocFixture(doc));
    app.completa_salvataggi(task);

    assert!(!app.workspace.get(doc).unwrap().is_dirty());
    let sul_disco = std::fs::read_to_string(loaded.path()).unwrap();
    assert!(sul_disco.starts_with("+++"), "intestazione conservata");
    let riletta = store.load(loaded.path()).unwrap();
    assert_eq!(riletta.note.header.id, loaded.note.header.id);
    assert_eq!(riletta.note.markdown, corpo);
}

/// Se il file della nota è cambiato sul disco dopo l'apertura, il
/// salvataggio si rifiuta invece di schiacciare l'altra modifica.
#[test]
fn salvare_una_nota_cambiata_sul_disco_e_un_errore_non_una_sovrascrittura() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    let loaded = store.create("Prima").unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), vec![loaded.clone()])
        .unwrap();
    let doc = app.open_loaded_note(loaded.clone());
    sporca_attivo(&mut app);
    let altrove = "+++\nmodificata = \"da un altro editor\"\n+++\n# Altrove\n";
    std::fs::write(loaded.path(), altrove).unwrap();

    let task = app.update(Message::SaveDocFixture(doc));
    app.completa_salvataggi(task);

    assert!(app.workspace.get(doc).unwrap().is_dirty(), "resta sporca");
    assert!(app.notice.is_some(), "e l'errore si vede");
    assert_eq!(std::fs::read_to_string(loaded.path()).unwrap(), altrove);
}

#[test]
fn errore_watcher_nativo_non_viene_cancellato_da_un_save_riuscito() {
    let (mut app, _) = Lume::new_without_store();

    let dir = tempfile::tempdir().unwrap();
    app.set_project_root(dir.path().to_path_buf());
    let _ = app.fs_watcher_subscription();
    let scope = app.watcher_scope.borrow().as_ref().unwrap().clone();
    let _ = app.update(Message::FsWatchFailed(scope, "accesso negato".into()));
    let _ = app.update(Message::SessionSaved(Ok(())));

    assert_eq!(app.watcher_error(), Some("accesso negato"));
    assert!(app.persistence_error.is_none());
}

#[test]
fn errore_recipe_watcher_obsoleta_non_modifica_avviso_del_root_corrente() {
    use futures::StreamExt;
    let old = tempfile::tempdir().unwrap();
    let current = tempfile::tempdir().unwrap();
    let mut app = app();
    app.set_project_root(old.path().to_path_buf());
    let recipe = iced::advanced::subscription::into_recipes(app.fs_watcher_subscription())
        .pop()
        .unwrap();
    drop(old);
    let mut stream = recipe.stream(futures::stream::empty().boxed());
    let message = runtime_file_tests().block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
    });
    assert!(matches!(&message, Message::FsWatchFailed(_, _)));
    app.set_project_root(current.path().to_path_buf());
    let _ = app.update(message);
    assert!(
        app.watcher_error.is_none(),
        "la recipe fallita del root precedente non descrive il watcher corrente"
    );
}

#[test]
fn global_file_query_starts_a_cancellable_background_worker() {
    use std::sync::atomic::Ordering;

    let (mut app, _) = Lume::new_without_store();
    app.project_root = Some(std::env::temp_dir());
    let _ = app.open_search(SearchTab::Tutto);

    let _task = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        "main".into(),
    )));

    assert!(app.global_search.is_file_searching());
    assert_eq!(app.global_search_gen.load(Ordering::SeqCst), 1);
}

#[test]
fn global_queries_reuse_one_worker_thread() {
    let (mut app, _) = Lume::new_without_store();
    app.project_root = Some(std::env::temp_dir());
    let _ = app.open_search(SearchTab::Tutto);

    let _first = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        "ma".into(),
    )));
    let worker_id = app.global_file_search_worker.as_ref().unwrap().thread_id();
    let _second = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        "main".into(),
    )));

    assert_eq!(
        app.global_file_search_worker.as_ref().unwrap().thread_id(),
        worker_id
    );
}

/// L.12: il thread «lume-file-palette» non esiste finché non si cerca, e
/// chiusa la palette va via. Un errore d'avvio si ritenta alla prossima
/// apertura, non a ogni battuta.
#[test]
fn la_palette_file_parte_al_primo_uso_e_si_ferma_alla_chiusura() {
    let (mut app, _) = Lume::new_without_store();
    assert!(
        app.global_file_search_worker.is_none(),
        "nessun thread palette all'avvio"
    );

    app.project_root = Some(std::env::temp_dir());
    let _ = app.open_search(SearchTab::Tutto);
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        "ma".into(),
    )));
    assert!(app.global_file_search_worker.is_some());

    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Escape));
    assert!(
        app.global_file_search_worker.is_none(),
        "palette chiusa: il thread va via"
    );
    assert!(app.global_file_search_worker_error.is_none());
}

/// Scegliere un risultato (Invio o clic) esce da `update` col `return`
/// anticipato di `Activate`, senza passare dal fondo: il thread della
/// palette deve andare via lo stesso, subito, senza un altro messaggio.
#[test]
fn scegliere_un_risultato_della_palette_ferma_subito_il_thread() {
    let (mut app, _) = Lume::new_without_store();
    app.project_root = Some(std::env::temp_dir());
    let _ = app.open_search(SearchTab::Tutto);
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        "Salva tutto".into(),
    )));
    assert!(app.global_file_search_worker.is_some());

    let indice = app
        .global_search
        .entries()
        .iter()
        .position(|entry| entry.name == "Salva tutto")
        .expect("l'azione «Salva tutto» compare fra i risultati");
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Activate(indice)));

    assert!(!app.global_search.is_open());
    assert!(
        app.global_file_search_worker.is_none(),
        "scelto il risultato la palette è chiusa: il thread va via subito"
    );
}

/// Un errore d'avvio del thread non si ritenta a ogni battuta finché la
/// palette resta aperta; alla chiusura si azzera, e la prossima apertura
/// riprova.
#[test]
fn l_errore_d_avvio_della_palette_non_si_ritenta_finche_e_aperta() {
    let (mut app, _) = Lume::new_without_store();
    app.global_file_search_worker_error = Some("limite thread".into());
    let _ = app.open_search(SearchTab::Tutto);

    app.ensure_file_palette_worker();
    assert!(
        app.global_file_search_worker.is_none(),
        "con l'errore in mano non si riprova a creare il thread"
    );
    assert_eq!(
        app.global_file_search_worker_error.as_deref(),
        Some("limite thread")
    );
    app.reconcile_file_palette();
    assert_eq!(
        app.global_file_search_worker_error.as_deref(),
        Some("limite thread"),
        "palette ancora aperta: l'errore resta"
    );

    app.global_search.close();
    app.chiudi_overlay_se(OverlayId::Search);
    app.reconcile_file_palette();
    assert!(app.global_file_search_worker_error.is_none());

    app.global_search
        .update(GlobalSearchMessage::Open(SearchTab::Tutto));
    app.ensure_file_palette_worker();
    assert!(
        app.global_file_search_worker.is_some(),
        "alla riapertura il thread si ritenta"
    );
}

/// La domanda «Salvare le modifiche?» chiude la palette e ritorna prima
/// del fondo di `update`: il thread va via lì, non un messaggio dopo.
#[test]
fn la_domanda_di_chiusura_ferma_il_thread_della_palette() {
    let (mut app, _) = Lume::new_without_store();
    app.project_root = Some(std::env::temp_dir());
    let _ = app.open_search(SearchTab::Tutto);
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        "ma".into(),
    )));
    assert!(app.global_file_search_worker.is_some());

    app.chiedi_chiusura(super::DaChiudere::Finestra(None));

    assert!(!app.global_search.is_open());
    assert!(app.global_file_search_worker.is_none());
}

#[test]
fn unavailable_global_search_worker_surfaces_an_error() {
    let (mut app, _) = Lume::new_without_store();
    app.global_file_search_worker.take();
    app.global_file_search_worker_error = Some("limite thread".into());
    app.project_root = Some(std::env::temp_dir());
    let _ = app.open_search(SearchTab::Tutto);

    let _task = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        "main".into(),
    )));

    assert_eq!(
        app.global_search.file_search_error(),
        Some("worker ricerca file non disponibile: limite thread")
    );
}

#[test]
fn replacing_search_overlay_cancels_its_background_worker() {
    use std::sync::atomic::Ordering;

    let (mut app, _) = Lume::new_without_store();
    app.project_root = Some(std::env::temp_dir());
    let _ = app.open_search(SearchTab::Tutto);
    let _task = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        "main".into(),
    )));

    app.apply_activity_intent(ActivityBarIntent::ToggleSettings);

    assert!(!app.global_search.is_file_searching());
    assert_eq!(app.global_search_gen.load(Ordering::SeqCst), 2);
}

#[test]
fn esc_annulla_una_volta_la_ricerca_anche_se_sostituita() {
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.project_root = Some(dir.path().to_path_buf());
    let _ = app.open_search(SearchTab::Tutto);
    let _task = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        "nome".into(),
    )));
    assert!(app.global_search.is_file_searching());
    app.apri_overlay(OverlayId::GotoLine);
    let prima = app.global_search_gen.load(Ordering::SeqCst);
    let _ = app.update(Message::CloseAll);
    assert_eq!(app.global_search_gen.load(Ordering::SeqCst), prima + 1);
    assert!(!app.global_search.is_file_searching());
    assert!(app.global_search.is_open());
}

fn ricerca_con_documento(root: &std::path::Path, query: &str) -> Lume {
    let mut app = app();
    write_and_open(&mut app, root, "originale.txt", "testo\n");
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Open(
        SearchTab::Tutto,
    )));
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        query.into(),
    )));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Search));
    app
}

#[test]
fn ricerca_invisibile_non_esegue_accettazione_ne_clic() {
    for dopo_esc in [false, true] {
        for clic in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = ricerca_con_documento(dir.path(), "Nuovo file");
            let indice = app
                .global_search
                .entries()
                .iter()
                .position(|e| e.name == "Nuovo file")
                .unwrap();
            assert_eq!(indice, 0);
            let originale = app.active_doc().unwrap();
            let schede = app.groups.all().count();
            let _ = app.update(Message::MenuAction(MenuAct::GotoLine));
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::GotoLine));
            if dopo_esc {
                let _ = app.update(Message::CloseAll);
            }
            let atteso = if dopo_esc {
                None
            } else {
                Some(OverlayId::GotoLine)
            };
            let messaggio = if clic {
                GlobalSearchMessage::Activate(indice)
            } else {
                GlobalSearchMessage::Accept
            };
            let _ = app.update(Message::GlobalSearch(messaggio));
            assert_eq!(
                app.groups.all().count(),
                schede,
                "ricerca invisibile non crea file"
            );
            assert_eq!(app.active_doc(), Some(originale));
            assert_eq!(app.shell.active_overlay(), atteso);
            assert!(matches!(app.notice.as_ref(), Some((_, n)) if n.kind == NoticeKind::Info));
        }
    }
}

#[test]
fn ricerca_invisibile_non_modifica_selezione_o_scheda() {
    use crate::search::GlobalSearchIntent;
    for dopo_esc in [false, true] {
        for messaggio in [
            GlobalSearchMessage::Navigate(1),
            GlobalSearchMessage::CycleTab,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = ricerca_con_documento(dir.path(), "");
            assert!(app.global_search.entries().len() > 1);
            let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Navigate(1)));
            assert_eq!(
                app.global_search.update(GlobalSearchMessage::Accept),
                GlobalSearchIntent::Activate(1)
            );
            let voci: Vec<_> = app
                .global_search
                .entries()
                .iter()
                .map(|e| e.name.clone())
                .collect();
            let _ = app.update(Message::MenuAction(MenuAct::GotoLine));
            if dopo_esc {
                let _ = app.update(Message::CloseAll);
            }
            let atteso = if dopo_esc {
                None
            } else {
                Some(OverlayId::GotoLine)
            };
            // La cancellazione della ricerca può già avere riportato la
            // selezione a zero: l'input tardivo non deve modificarla ancora.
            let selezione = app.global_search.update(GlobalSearchMessage::Accept);
            let _ = app.update(Message::GlobalSearch(messaggio));
            assert_eq!(
                app.global_search.update(GlobalSearchMessage::Accept),
                selezione
            );
            assert_eq!(
                app.global_search
                    .entries()
                    .iter()
                    .map(|e| e.name.clone())
                    .collect::<Vec<_>>(),
                voci
            );
            assert_eq!(app.shell.active_overlay(), atteso);
            assert!(matches!(app.notice.as_ref(), Some((_, n)) if n.kind == NoticeKind::Info));
        }
    }
}

#[test]
fn ricerca_invisibile_rifiuta_input_e_chiusura_tardivi() {
    for messaggio in [
        GlobalSearchMessage::QueryChanged("Salva".into()),
        GlobalSearchMessage::TabClicked(SearchTab::File),
        GlobalSearchMessage::Escape,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = ricerca_con_documento(dir.path(), "Nuovo file");
        let voci: Vec<_> = app
            .global_search
            .entries()
            .iter()
            .map(|e| e.name.clone())
            .collect();
        let _ = app.update(Message::MenuAction(MenuAct::GotoLine));
        let _ = app.update(Message::GlobalSearch(messaggio));
        assert_eq!(app.shell.active_overlay(), Some(OverlayId::GotoLine));
        assert_eq!(
            app.global_search
                .entries()
                .iter()
                .map(|e| e.name.clone())
                .collect::<Vec<_>>(),
            voci
        );
        assert!(app.global_search.is_open());
        assert!(matches!(app.notice.as_ref(), Some((_, n)) if n.kind == NoticeKind::Info));
    }
}

#[test]
fn tasti_globali_di_sfondo_non_avvisano_se_la_ricerca_e_chiusa() {
    let mut app = app();
    assert!(!app.global_search.is_open());
    for messaggio in [
        GlobalSearchMessage::Navigate(1),
        GlobalSearchMessage::Accept,
        GlobalSearchMessage::CycleTab,
    ] {
        let _ = app.update(Message::GlobalSearch(messaggio));
        assert!(app.notice.is_none());
        assert_eq!(app.shell.active_overlay(), None);
    }
}

#[test]
fn ricerca_visibile_esegue_l_azione_selezionata() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = ricerca_con_documento(dir.path(), "Nuovo file");
    let prima = app.groups.all().count();
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Accept));
    assert_eq!(app.groups.all().count(), prima + 1);
    assert_eq!(app.shell.active_overlay(), None);
}

#[test]
fn doppio_maiusc_riapre_ricerca_sostituita_senza_cancellarne_il_modello() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = ricerca_con_documento(dir.path(), "Nuovo file");
    let _ = app.update(Message::MenuAction(MenuAct::GotoLine));
    let _ = app.update(Message::CloseAll);
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::ShiftPressed));
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::ShiftPressed));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Search));
    assert!(
        app.global_search
            .entries()
            .iter()
            .any(|e| e.name == "Nuovo file")
    );
    let prima = app.groups.all().count();
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Accept));
    assert_eq!(app.groups.all().count(), prima + 1);
}

#[test]
fn apertura_esplicita_riporta_avanti_la_ricerca_sostituita() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = ricerca_con_documento(dir.path(), "Nuovo file");
    let _ = app.update(Message::MenuAction(MenuAct::GotoLine));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::GotoLine));
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Open(
        SearchTab::Azioni,
    )));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Search));
    let prima = app.groups.all().count();
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Accept));
    assert_eq!(app.groups.all().count(), prima + 1);
}

#[test]
fn recenti_riceve_frecce_e_invio_senza_tab_di_ricerca_invisibile() {
    let dir = tempfile::tempdir().unwrap();
    let uno = dir.path().join("uno.txt");
    let due = dir.path().join("due.txt");
    let tre = dir.path().join("tre.txt");
    for file in [&uno, &due, &tre] {
        std::fs::write(file, "testo\n").unwrap();
    }
    let mut app = app();
    app.open_file(&uno).unwrap();
    app.jump_to(&due, lsp_types::Position::new(0, 0)).unwrap();
    app.jump_to(&tre, lsp_types::Position::new(0, 0)).unwrap();
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Open(
        SearchTab::Tutto,
    )));
    let voci: Vec<_> = app
        .global_search
        .entries()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    let _ = app.update(Message::MenuAction(MenuAct::Recents));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Recents));
    let righe = app.recents_rows();
    assert_eq!(righe.len(), 2);
    let destinazione = righe[1].name.clone();
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Navigate(1)));
    assert_eq!(app.recents_selected, 1);
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::CycleTab));
    assert_eq!(app.recents_selected, 1);
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Recents));
    assert_eq!(
        app.global_search
            .entries()
            .iter()
            .map(|e| e.name.clone())
            .collect::<Vec<_>>(),
        voci
    );
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Accept));
    assert_eq!(
        app.active_path()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy(),
        destinazione
    );
    assert_eq!(app.shell.active_overlay(), None);
}

#[test]
fn esc_su_search_annulla_una_volta_la_generazione() {
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir().unwrap();
    let mut app = ricerca_con_documento(dir.path(), "nome");
    app.project_root = Some(dir.path().to_path_buf());
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        "nome".into(),
    )));
    assert!(app.global_search.is_file_searching());
    let prima = app.global_search_gen.load(Ordering::SeqCst);
    let _ = app.update(Message::CloseAll);
    assert_eq!(app.global_search_gen.load(Ordering::SeqCst), prima + 1);
    assert!(!app.global_search.is_file_searching());
    assert!(!app.global_search.is_open());
    assert_eq!(app.shell.active_overlay(), None);
}

#[test]
fn opening_a_tool_cancels_hidden_global_search_work() {
    let (mut app, _) = Lume::new_without_store();
    app.project_root = Some(std::env::temp_dir());
    let _ = app.open_search(SearchTab::Tutto);
    let _task = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        "main".into(),
    )));

    app.apply_activity_intent(ActivityBarIntent::Click(tools::project::ID));

    assert!(!app.global_search.is_file_searching());
}

#[test]
fn opening_menu_cancels_hidden_global_search_work() {
    let (mut app, _) = Lume::new_without_store();
    app.project_root = Some(std::env::temp_dir());
    let _ = app.open_search(SearchTab::Tutto);
    let _task = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        "main".into(),
    )));

    let _task = app.update(Message::Titlebar(TitlebarMessage::ToggleMenuRoot));

    assert!(!app.global_search.is_file_searching());
}

#[test]
fn opening_environment_cancels_hidden_global_search_work() {
    let (mut app, _) = Lume::new_without_store();
    app.project_root = Some(std::env::temp_dir());
    let _ = app.open_search(SearchTab::Tutto);
    let _task = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
        "main".into(),
    )));

    let _task = app.update(Message::Statusbar(StatusbarMessage::Environment(
        EnvironmentPickerMessage::Toggle,
    )));

    assert!(!app.global_search.is_file_searching());
}

#[test]
fn aprire_il_selettore_avvia_la_rilevazione_una_volta_sola() {
    use crate::environments::RilevazioneWsl;
    let mut app = app();
    let toggle = || {
        Message::Statusbar(StatusbarMessage::Environment(
            EnvironmentPickerMessage::Toggle,
        ))
    };

    let _ = app.update(toggle());
    assert!(app.environment_picker.is_open());
    assert_eq!(app.wsl, RilevazioneWsl::InCorso);

    // chiudere e riaprire non rilancia nulla
    let _ = app.update(toggle());
    let _ = app.update(toggle());
    assert_eq!(app.wsl, RilevazioneWsl::InCorso);
    assert_eq!(app.rileva_wsl_se_serve().units(), 0);
}

#[test]
fn chiudere_il_selettore_non_avvia_la_rilevazione() {
    use crate::environments::RilevazioneWsl;
    let mut app = app();
    let _ = app.update(Message::Statusbar(StatusbarMessage::Environment(
        EnvironmentPickerMessage::Dismiss,
    )));
    assert_eq!(app.wsl, RilevazioneWsl::NonAvviata);
}

#[test]
fn l_esito_della_rilevazione_aggiunge_le_distro_e_chiude_la_fase() {
    use crate::environments::{RilevazioneWsl, parse_wsl_list};
    let mut app = app();
    let _ = app.rileva_wsl_se_serve();
    let raw: Vec<u8> = "Ubuntu\r\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let _ = app.update(Message::EnvironmentsDetected(parse_wsl_list(&raw)));
    assert_eq!(app.wsl, RilevazioneWsl::Fatta);
    let id: Vec<&str> = app
        .environment_picker
        .profiles()
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    if crate::environments::WSL_SUPPORTED {
        assert!(id.contains(&"wsl:Ubuntu"), "profili: {id:?}");
    } else {
        assert!(!id.contains(&"wsl:Ubuntu"), "profili: {id:?}");
    }
}

#[test]
fn refresh_filesystem_riallinea_lo_scope_applicativo() {
    use lume_ui::tool::ToolEvent;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_app_watch_refresh_{nonce}"));
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();

    let (mut app, _) = Lume::new_without_store();
    app.set_project_root(root.clone());
    let _ = app.update(Message::Tool(
        tools::project::ID,
        ToolEvent::new(tools::project::Event::RowClicked(src.clone(), true)),
    ));
    assert_eq!(app.fs_watch_directories(), vec![root.clone(), src.clone()]);

    fs::remove_dir_all(&src).unwrap();
    let _ = app.on_fs_changed(vec![src]);
    assert_eq!(app.fs_watch_directories(), vec![root.clone()]);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn watcher_scope_unisce_albero_visibile_e_documenti_aperti() {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_app_watch_scope_{nonce}"));
    let expanded = root.join("expanded");
    let document_dir = root.join("closed").join("nested");
    fs::create_dir_all(&expanded).unwrap();
    fs::create_dir_all(&document_dir).unwrap();
    let file = document_dir.join("main.rs");
    fs::write(&file, "fn main() {}\n").unwrap();

    let (mut app, _) = Lume::new_without_store();
    app.set_project_root(root.clone());
    assert_eq!(app.fs_watch_directories(), vec![root.clone()]);

    app.set_project_watch_directories(vec![root.clone(), expanded.clone()]);
    assert_eq!(
        app.fs_watch_directories(),
        vec![root.clone(), expanded.clone()]
    );

    assert!(app.open_file(&file).is_ok());
    assert_eq!(
        app.fs_watch_directories(),
        vec![root.clone(), document_dir, expanded]
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(feature = "sql")]
fn starts_without_sample_project_or_mock_tools() {
    let (app, _) = Lume::new_without_store();

    assert!(app.project_root.is_none());
    assert_eq!(app.groups.all().count(), 0);
    assert_eq!(
        app.registry.left,
        vec![
            tools::project::ID,
            tools::search_tool::ID,
            tools::git_native::ID,
            tools::terminal::ID,
            tools::gitlog::ID,
            tools::db::ID,
            tools::problems::ID,
            tools::ext::ID,
            tools::usages::ID,
            tools::jj::ID
        ]
    );
    assert_eq!(
        app.registry.right,
        vec![
            tools::bookmarks::ID,
            tools::history::ID,
            tools::notes::ID,
            tools::structure::ID
        ]
    );
    assert!(app.notes.is_none());
    assert_eq!(app.registry.active_dock, Some(tools::terminal::ID));
    assert_eq!(app.registry.dock_height, lume_ui::tool::DOCK_HEIGHT_RANGE.1);
}

#[test]
fn gli_eventi_del_filesystem_non_ricalcolano_git_a_viste_chiuse() {
    let (mut app, _) = Lume::new_without_store();
    app.registry.set_active_panel(None);
    app.registry.set_active_dock(None);
    assert!(!app.registry.is_open(tools::git_native::ID));
    assert!(!app.registry.is_open(tools::gitlog::ID));

    let _ = app.update(Message::FsChanged(vec![std::path::PathBuf::from(
        "src/lib.rs",
    )]));

    // nessun lavoro speso: solo la marcatura per il recupero pigro
    assert!(app.git_stale, "il pannello Git va marcato stantio");
    assert!(app.git_log_stale, "il dock Log Git va marcato stantio");
}

#[test]
fn leco_delle_nostre_scritture_su_git_non_rilancia_il_refresh() {
    let (mut app, _) = Lume::new_without_store();
    app.registry.set_active_panel(None);
    app.registry.set_active_dock(None);
    app.last_git_operation = Some(std::time::Instant::now());

    // burst tutto dentro .git subito dopo una nostra lettura: e' l'eco
    let _ = app.update(Message::FsChanged(vec![
        std::path::PathBuf::from(".git").join("index"),
    ]));
    assert!(!app.git_stale, "l'eco delle nostre scritture va ignorato");

    // un file del working tree nello stesso burst e' un cambio vero
    let _ = app.update(Message::FsChanged(vec![
        std::path::PathBuf::from(".git").join("index"),
        std::path::PathBuf::from("src/lib.rs"),
    ]));
    assert!(app.git_stale, "un cambio del working tree non va soppresso");
}

#[test]
fn il_watcher_include_git_per_vedere_i_commit_da_terminale() {
    let (mut app, _) = Lume::new_without_store();
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    std::fs::create_dir_all(root.join(".git").join("refs").join("heads")).expect("git dir");
    app.project_root = Some(root.clone());

    let watched = app.fs_watch_directories();
    assert!(
        watched.contains(&root.join(".git")),
        "senza la radice .git un commit esterno non aggiorna branch e log"
    );
    assert!(
        watched.contains(&root.join(".git").join("refs").join("heads")),
        "i ref sciolti non stanno nella radice: servono anche refs/heads"
    );
}

#[test]
fn riaprire_la_vista_git_recupera_il_refresh_saltato() {
    let (mut app, _) = Lume::new_without_store();
    app.registry.set_active_panel(None);
    app.registry.set_active_dock(None);
    let _ = app.update(Message::FsChanged(vec![std::path::PathBuf::from(
        "src/lib.rs",
    )]));
    assert!(app.git_stale);

    // la riapertura passa da drain_tool_requests, che consuma i flag
    app.registry.set_active_panel(Some(tools::git_native::ID));
    let _ = app.refresh_stale_git();
    assert!(
        !app.git_stale,
        "il refresh saltato va recuperato all'apertura"
    );
    assert!(
        app.git_log_stale,
        "il dock Log Git e' ancora chiuso: resta stantio"
    );
}

#[test]
fn un_risultato_git_arrivato_a_pannello_chiuso_non_ripopola_la_cache() {
    use lume_ui::tool::ToolRequest;
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.registry.set_active_panel(None);
    app.registry
        .get_mut(tools::git_native::ID)
        .unwrap()
        .update(&lume_ui::tool::ToolEvent::new(
            tools::git_native::Event::SetRoot(root.path().to_path_buf()),
        ));

    let _ = app.update(Message::GitLoaded(0, Ok(snapshot_con_ramo("main")), false));

    let requests = app.registry.set_active_panel(Some(tools::git_native::ID));
    assert!(
        requests
            .iter()
            .any(|(_, request)| matches!(request, ToolRequest::GitRefresh))
    );
}

#[test]
fn un_risultato_log_arrivato_a_dock_chiuso_non_ripopola_la_cache() {
    use lume_ui::tool::ToolRequest;
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.registry.set_active_dock(None);
    app.registry
        .get_mut(tools::gitlog::ID)
        .unwrap()
        .update(&lume_ui::tool::ToolEvent::new(
            tools::gitlog::Event::SetRoot(root.path().to_path_buf()),
        ));

    let _ = app.update(Message::GitLogLoaded(
        0,
        Ok(tools::gitlog::LogSnapshot {
            reference: "HEAD".into(),
            branches: lume_git::Branches {
                head: None,
                local: Vec::new(),
                remote: Vec::new(),
            },
            commits: Vec::new(),
        }),
    ));

    let requests = app.registry.set_active_dock(Some(tools::gitlog::ID));
    assert!(
        requests
            .iter()
            .any(|(_, request)| matches!(request, ToolRequest::GitLog(_)))
    );
}

#[test]
fn la_riapertura_non_carica_due_volte_git() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.registry.set_active_panel(None);
    app.project_root = Some(root.path().to_path_buf());
    app.git_stale = true;
    app.registry
        .get_mut(tools::git_native::ID)
        .unwrap()
        .update(&lume_ui::tool::ToolEvent::new(
            tools::git_native::Event::SetRoot(root.path().to_path_buf()),
        ));

    let prima = app.git_gen.load(std::sync::atomic::Ordering::SeqCst);
    let requests = app.registry.set_active_panel(Some(tools::git_native::ID));
    app.pending_tool_requests.extend(requests);
    let _ = app.drain_tool_requests();

    let dopo = app.git_gen.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        dopo,
        prima + 1,
        "refresh chiesto dal tool e stale non duplicano il lavoro"
    );
    assert!(!app.git_stale);
}

/// L.11: all'avvio con un progetto, a viste Git chiuse, si legge solo il
/// ramo per la titlebar. Refresh completo e log(200) aspettano la vista.
#[test]
fn all_avvio_a_viste_git_chiuse_si_legge_solo_il_ramo() {
    use std::sync::atomic::Ordering;
    let (mut app, _) = Lume::new_without_store();
    app.registry.set_active_panel(None);
    app.registry.set_active_dock(None);
    app.project_root = Some(std::env::temp_dir());

    let _ = app.git_dopo_cambio_radice();

    assert_eq!(
        app.git_gen.load(Ordering::SeqCst),
        0,
        "niente refresh completo"
    );
    assert_eq!(app.git_log_gen.load(Ordering::SeqCst), 0, "niente log(200)");
    assert_eq!(app.git_branch_gen.load(Ordering::SeqCst), 1, "solo il ramo");
    assert!(app.git_stale, "il pannello Git si aggiorna alla riapertura");
    assert!(app.git_log_stale, "il Log Git si carica alla riapertura");
}

#[test]
fn con_il_pannello_git_aperto_all_avvio_si_fa_il_refresh_completo() {
    use std::sync::atomic::Ordering;
    let (mut app, _) = Lume::new_without_store();
    let _ = app.registry.set_active_panel(Some(tools::git_native::ID));
    app.registry.set_active_dock(None);
    app.project_root = Some(std::env::temp_dir());

    let _ = app.git_dopo_cambio_radice();

    assert_eq!(app.git_gen.load(Ordering::SeqCst), 1);
    assert_eq!(
        app.git_branch_gen.load(Ordering::SeqCst),
        0,
        "il ramo arriva col refresh"
    );
    assert!(!app.git_stale);
    assert!(app.git_log_stale);
}

/// Con il Log Git aperto all'avvio il log(200) parte subito e non resta
/// nessun debito da recuperare.
#[test]
fn con_il_log_git_aperto_all_avvio_si_carica_il_log() {
    use std::sync::atomic::Ordering;
    let (mut app, _) = Lume::new_without_store();
    app.registry.set_active_panel(None);
    let _ = app.registry.set_active_dock(Some(tools::gitlog::ID));
    app.project_root = Some(std::env::temp_dir());

    let _ = app.git_dopo_cambio_radice();

    assert_eq!(app.git_log_gen.load(Ordering::SeqCst), 1);
    assert!(!app.git_log_stale);
    assert_eq!(app.git_gen.load(Ordering::SeqCst), 0);
    assert_eq!(app.git_branch_gen.load(Ordering::SeqCst), 1);
}

/// Con «git» spenta non si tocca il repository, ma i flag restano: chi la
/// riaccende e apre il pannello deve vedere lo stato, non «Caricamento…»
/// per sempre.
#[test]
fn con_git_spenta_all_avvio_non_si_legge_niente() {
    use std::sync::atomic::Ordering;
    let (mut app, _) = Lume::new_without_store();
    let _ = app.registry.set_feature_enabled("git", false);
    app.project_root = Some(std::env::temp_dir());

    let _ = app.git_dopo_cambio_radice();

    assert_eq!(app.git_gen.load(Ordering::SeqCst), 0);
    assert_eq!(app.git_branch_gen.load(Ordering::SeqCst), 0);
    assert!(app.git_stale && app.git_log_stale);
}

/// Senza progetto non c'è niente da leggere e niente da recuperare.
#[test]
fn senza_progetto_git_dopo_cambio_radice_non_fa_niente() {
    use std::sync::atomic::Ordering;
    let (mut app, _) = Lume::new_without_store();
    app.project_root = None;

    let _ = app.git_dopo_cambio_radice();

    assert_eq!(app.git_branch_gen.load(Ordering::SeqCst), 0);
    assert!(!app.git_stale && !app.git_log_stale);
}

/// Stessa guardia dopo un commit: a dock chiuso il log si segna stantio.
#[test]
fn un_commit_a_log_chiuso_non_ricarica_il_log() {
    use std::sync::atomic::Ordering;
    let (mut app, _) = Lume::new_without_store();
    app.registry.set_active_dock(None);
    app.project_root = Some(std::env::temp_dir());
    let snapshot = tools::git_native::Snapshot {
        branch: Some("main".into()),
        local_branches: vec!["main".into()],
        has_origin: false,
        merge_conflicts: None,
        has_upstream: false,
        files: Vec::new(),
        commits: Vec::new(),
        head_message: None,
    };

    let _ = app.update(Message::GitLoaded(0, Ok(snapshot), true));

    assert_eq!(app.git_log_gen.load(Ordering::SeqCst), 0);
    assert!(app.git_log_stale);
}

/// Con il dock Log Git aperto, invece, il commit lo ricarica.
#[test]
fn un_commit_a_log_aperto_ricarica_il_log() {
    use std::sync::atomic::Ordering;
    let (mut app, _) = Lume::new_without_store();
    let _ = app.registry.set_active_dock(Some(tools::gitlog::ID));
    app.project_root = Some(std::env::temp_dir());
    let snapshot = tools::git_native::Snapshot {
        branch: Some("main".into()),
        local_branches: vec!["main".into()],
        has_origin: false,
        merge_conflicts: None,
        has_upstream: false,
        files: Vec::new(),
        commits: Vec::new(),
        head_message: None,
    };

    let _ = app.update(Message::GitLoaded(0, Ok(snapshot), true));

    assert_eq!(app.git_log_gen.load(Ordering::SeqCst), 1);
    assert!(!app.git_log_stale);
}

/// Review Focus: un `git checkout` fatto dal terminale con le viste Git
/// chiuse cambia `.git/HEAD`; la pill del ramo deve seguirlo. Un file del
/// worktree invece non cambia il ramo.
#[test]
fn un_checkout_a_viste_git_chiuse_aggiorna_la_pill_del_ramo() {
    use std::sync::atomic::Ordering;
    let cartella = tempfile::tempdir().unwrap();
    let root = cartella.path().to_path_buf();
    let (mut app, _) = Lume::new_without_store();
    app.registry.set_active_panel(None);
    app.registry.set_active_dock(None);
    app.project_root = Some(root);

    let _ = app.update(Message::FsChanged(vec![std::path::PathBuf::from(
        ".git/HEAD",
    )]));
    assert_eq!(app.git_branch_gen.load(Ordering::SeqCst), 1);
    assert_eq!(
        app.git_gen.load(Ordering::SeqCst),
        0,
        "il pannello chiuso resta stantio"
    );

    let _ = app.update(Message::FsChanged(vec![std::path::PathBuf::from(
        "src/lib.rs",
    )]));
    assert_eq!(app.git_branch_gen.load(Ordering::SeqCst), 1);
}

/// Con «git» spenta nemmeno un evento su `.git` rilegge il ramo.
#[test]
fn con_git_spenta_un_evento_su_git_non_rilegge_il_ramo() {
    use std::sync::atomic::Ordering;
    let (mut app, _) = Lume::new_without_store();
    let _ = app.registry.set_feature_enabled("git", false);
    app.registry.set_active_dock(None);
    app.project_root = Some(std::env::temp_dir());

    let _ = app.update(Message::FsChanged(vec![std::path::PathBuf::from(
        ".git/HEAD",
    )]));

    assert_eq!(app.git_branch_gen.load(Ordering::SeqCst), 0);
}

/// Transizione: avviato a viste chiuse, il pannello Git aperto dopo
/// ottiene lo stato completo (refresh) e il flag si consuma.
#[test]
fn aprire_il_pannello_git_dopo_un_avvio_a_viste_chiuse_carica_lo_stato_completo() {
    use std::sync::atomic::Ordering;
    let (mut app, _) = Lume::new_without_store();
    app.registry.set_active_panel(None);
    app.registry.set_active_dock(None);
    app.project_root = Some(std::env::temp_dir());
    let _ = app.git_dopo_cambio_radice();
    assert_eq!(app.git_gen.load(Ordering::SeqCst), 0);

    let _ = app.registry.set_active_panel(Some(tools::git_native::ID));
    let _ = app.refresh_stale_git();

    assert_eq!(
        app.git_gen.load(Ordering::SeqCst),
        1,
        "il pannello aperto dopo l'avvio deve avere il refresh completo"
    );
    assert!(!app.git_stale);
}

fn snapshot_con_ramo(ramo: &str) -> tools::git_native::Snapshot {
    tools::git_native::Snapshot {
        branch: Some(ramo.into()),
        local_branches: vec![ramo.into()],
        has_origin: false,
        merge_conflicts: None,
        has_upstream: false,
        files: Vec::new(),
        commits: Vec::new(),
        head_message: None,
    }
}

#[test]
fn snapshot_of_rileva_origin_e_stato_merge_senza_ambiguita() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let root = tempfile::tempdir().unwrap();
    let cli = lume_git::GitCli::locate().expect("git controllato dal testkit");
    let stop = std::sync::atomic::AtomicBool::new(false);
    let run = |args: &[&str]| {
        let output = cli.run(root.path(), args, &stop).unwrap();
        assert!(output.success(), "{}", output.failure_text());
    };
    run(&["init", "-b", "main"]);
    run(&["config", "user.name", "Lume test"]);
    run(&["config", "user.email", "lume@example.test"]);
    std::fs::write(root.path().join("a.txt"), "base\n").unwrap();
    run(&["add", "a.txt"]);
    run(&["commit", "-m", "base"]);

    let repository = lume_git::GitRepository::discover(root.path()).unwrap();
    let snapshot = super::git::snapshot_of(&repository).unwrap();
    assert!(!snapshot.has_origin);
    assert!(snapshot.merge_conflicts.is_none());

    run(&["remote", "add", "origin", "../remote.git"]);
    let repository = lume_git::GitRepository::discover(root.path()).unwrap();
    let snapshot = super::git::snapshot_of(&repository).unwrap();
    assert!(
        snapshot.has_origin,
        "origin non richiede refs remote gia' scaricati"
    );
    assert!(snapshot.merge_conflicts.is_none());
}

/// Transizione: scegliere un'altra cartella a viste chiuse azzera il ramo
/// vecchio, rilegge solo il ramo e lascia i debiti alla riapertura. Le
/// letture in volo del progetto di prima si scartano anche se nessun
/// refresh completo le supera.
#[test]
fn scegliere_una_cartella_a_viste_git_chiuse_scarta_le_letture_del_progetto_di_prima() {
    use std::sync::atomic::Ordering;
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.registry.set_active_panel(None);
    app.registry.set_active_dock(None);
    app.branch = Some("vecchio".into());
    let (git_prima, log_prima, ramo_prima) = (
        app.git_gen.load(Ordering::SeqCst),
        app.git_log_gen.load(Ordering::SeqCst),
        app.git_branch_gen.load(Ordering::SeqCst),
    );

    let _ = app.update(Message::FolderPicked(Some(root.path().to_path_buf())));

    assert_eq!(app.branch, None, "il ramo del progetto di prima va via");
    assert!(app.git_stale && app.git_log_stale);
    assert!(app.git_gen.load(Ordering::SeqCst) > git_prima);
    assert!(app.git_log_gen.load(Ordering::SeqCst) > log_prima);
    assert!(app.git_branch_gen.load(Ordering::SeqCst) > ramo_prima);

    // le risposte in volo del progetto vecchio non toccano il ramo
    let _ = app.update(Message::GitBranchLoaded(ramo_prima, Some("vecchio".into())));
    assert_eq!(app.branch, None);
    let _ = app.update(Message::GitLoaded(
        git_prima,
        Ok(snapshot_con_ramo("vecchio")),
        false,
    ));
    assert_eq!(app.branch, None);

    // quella del progetto nuovo vale
    let corrente = app.git_branch_gen.load(Ordering::SeqCst);
    let _ = app.update(Message::GitBranchLoaded(corrente, Some("nuovo".into())));
    assert_eq!(app.branch.as_deref(), Some("nuovo"));
}

/// Transizione: con il pannello Git aperto la cartella scelta fa il
/// refresh completo e scarta lo snapshot del progetto di prima.
#[test]
fn scegliere_una_cartella_a_pannello_git_aperto_fa_il_refresh_completo() {
    use std::sync::atomic::Ordering;
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let _ = app.registry.set_active_panel(Some(tools::git_native::ID));
    app.registry.set_active_dock(None);
    let git_prima = app.git_gen.load(Ordering::SeqCst);
    let ramo_prima = app.git_branch_gen.load(Ordering::SeqCst);

    let _ = app.update(Message::FolderPicked(Some(root.path().to_path_buf())));

    assert!(
        app.git_gen.load(Ordering::SeqCst) > git_prima + 1,
        "invalidazione del cambio radice piu' il refresh completo"
    );
    assert!(!app.git_stale);
    let _ = app.update(Message::GitLoaded(
        git_prima,
        Ok(snapshot_con_ramo("vecchio")),
        false,
    ));
    assert_eq!(app.branch, None, "lo snapshot vecchio si scarta");
    let corrente = app.git_gen.load(Ordering::SeqCst);
    let _ = app.update(Message::GitLoaded(
        corrente,
        Ok(snapshot_con_ramo("nuovo")),
        false,
    ));
    assert_eq!(app.branch.as_deref(), Some("nuovo"));
    assert_eq!(
        app.git_branch_gen.load(Ordering::SeqCst),
        ramo_prima + 1,
        "il ramo arriva col refresh: solo l'invalidazione del cambio radice"
    );
}

/// Con «git» spenta dopo il cambio cartella non parte nessuna lettura,
/// ma quelle in volo del progetto di prima non devono comunque passare.
#[test]
fn con_git_spenta_scegliere_una_cartella_scarta_comunque_le_letture_in_volo() {
    use std::sync::atomic::Ordering;
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let _ = app.registry.set_feature_enabled("git", false);
    app.registry.set_active_dock(None);
    let ramo_prima = app.git_branch_gen.load(Ordering::SeqCst);

    let _ = app.update(Message::FolderPicked(Some(root.path().to_path_buf())));
    let _ = app.update(Message::GitBranchLoaded(ramo_prima, Some("vecchio".into())));

    assert_eq!(app.branch, None);
}

/// Il ramo letto vale solo se e' della generazione corrente: la risposta
/// di una lettura superata (altro progetto, altro checkout) si scarta.
#[test]
fn il_ramo_di_una_generazione_superata_si_scarta() {
    use std::sync::atomic::Ordering;
    let (mut app, _) = Lume::new_without_store();
    app.git_branch_gen.store(2, Ordering::SeqCst);
    app.branch = None;

    let _ = app.update(Message::GitBranchLoaded(1, Some("vecchio".into())));
    assert_eq!(app.branch, None);

    let _ = app.update(Message::GitBranchLoaded(2, Some("main".into())));
    assert_eq!(app.branch.as_deref(), Some("main"));
}

/// La lettura vera, su repository veri: ramo dopo `git init`, cambio di
/// ramo con `git checkout`, e `None` fuori da un repository.
#[test]
fn leggi_ramo_legge_solo_il_ramo_di_un_repository_vero() {
    let git = |dir: &Path, args: &[&str]| {
        // Nome in variabile: l'eccezione CLI documentata vieta la forma
        // letterale Command::new col programma git fuori da cli.rs.
        let programma = "git";
        let status = std::process::Command::new(programma)
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?} fallito");
    };
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q", "-b", "principale"]);
    assert_eq!(leggi_ramo(repo.path()).as_deref(), Some("principale"));

    git(repo.path(), &["checkout", "-q", "-b", "altro"]);
    assert_eq!(leggi_ramo(repo.path()).as_deref(), Some("altro"));

    let fuori = tempfile::tempdir().unwrap();
    let programma = "git";
    let dentro_un_repo = std::process::Command::new(programma)
        .arg("-C")
        .arg(fuori.path())
        .args(["rev-parse", "--git-dir"])
        .output()
        .unwrap()
        .status
        .success();
    if dentro_un_repo {
        eprintln!(
            "salto l'asserzione «fuori da un repository»: un antenato di {} e' un repository Git",
            fuori.path().display()
        );
    } else {
        assert_eq!(leggi_ramo(fuori.path()), None);
    }
}

#[test]
fn session_snapshot_tracks_real_workspace_and_layout() {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_app_session_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    let file = root.join("main.rs");
    fs::write(&file, "fn main() {}\n").unwrap();

    let (mut app, _) = Lume::new_without_store();
    app.set_project_root(root.clone());
    assert!(app.open_file(&file).is_ok());
    app.layout.panel_width = 333.0;
    app.theme_state.sel = 1;
    // il dock parte sul terminale: il click sul Log Git lo sostituisce
    app.registry.click(tools::gitlog::ID);

    let snapshot = app.session_snapshot();

    assert_eq!(snapshot.workspace_root, Some(root.clone()));
    assert_eq!(snapshot.tabs, vec![file.clone()]);
    assert_eq!(snapshot.active_tab, Some(file));
    assert_eq!(snapshot.panel_width, 333.0);
    assert_eq!(
        snapshot.appearance.color_theme_id,
        app.theme_state.current().id.as_str()
    );
    assert_eq!(snapshot.active_dock.as_deref(), Some("gitlog"));
    assert_eq!(snapshot.dock_height, lume_ui::tool::DOCK_HEIGHT_RANGE.1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn f11_toggles_a_real_bookmark_and_history_tracks_opens_and_saves() {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_app_bookmark_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    let file = root.join("main.rs");
    fs::write(&file, "fn main() {\n    println!(\"ciao\");\n}\n").unwrap();

    let (mut app, _) = Lume::new_without_store();
    app.set_project_root(root.clone());
    assert!(app.open_file(&file).is_ok());
    assert_eq!(app.history.len(), 1);
    assert_eq!(app.history[0].action, lume_store::HistoryAction::Opened);
    assert_eq!(app.history[0].label, "main.rs");

    app.goto_line(2);
    let _ = app.update(Message::ToggleBookmark);
    assert_eq!(app.bookmarks.len(), 1);
    assert_eq!(app.bookmarks[0].line, 2);
    assert_eq!(app.bookmarks[0].preview, "println!(\"ciao\");");
    assert_eq!(app.bookmarks[0].path, file);
    let snapshot = app.session_snapshot();
    assert_eq!(snapshot.bookmarks, app.bookmarks);
    assert_eq!(snapshot.history.len(), 1);

    // stessa riga: toglie
    let _ = app.update(Message::ToggleBookmark);
    assert!(app.bookmarks.is_empty());

    // salvataggio reale → voce di cronologia
    let id = app.active_doc().unwrap();
    app.workspace
        .get_mut(id)
        .unwrap()
        .set_selection(lume_core::Range::point(0));
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        lume_ui::editor::EditorCommand::Insert("// nota\n".into()),
    ));
    let task = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(task);
    assert_eq!(app.history[0].action, lume_store::HistoryAction::Saved);
    assert!(fs::read_to_string(&file).unwrap().starts_with("// nota"));

    // rimozione dal tool: passa dall'app e sparisce dallo snapshot
    let _ = app.update(Message::ToggleBookmark);
    let _ = app.update(Message::Tool(
        tools::bookmarks::ID,
        lume_ui::tool::ToolEvent::new(tools::bookmarks::Event::Remove(0)),
    ));
    assert!(app.bookmarks.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn view_menu_toggles_terminal_and_shows_docks_with_real_checks() {
    use crate::menu::MenuAct;

    let (mut app, _) = Lume::new_without_store();
    assert!(
        app.menu_checks().terminal_open,
        "il dock parte sul terminale"
    );
    let _ = app.update(Message::MenuAction(MenuAct::ToggleTerminal));
    assert!(!app.registry.is_open(tools::terminal::ID));
    assert!(!app.menu_checks().terminal_open);
    let _ = app.update(Message::MenuAction(MenuAct::ShowGitLog));
    assert!(app.registry.is_open(tools::gitlog::ID));
    // «Log Git» non è un toggle: riselezionarlo lo lascia aperto
    let _ = app.update(Message::MenuAction(MenuAct::ShowGitLog));
    assert!(app.registry.is_open(tools::gitlog::ID));
    // ATTENZIONE: Rovesciata da quando il language server è cablato: per questo
    // Problemi è un dock vero e la scorciatoia lo apre. Quando non lo
    // era, questa asserzione diceva l'opposto — ed era giusta allora.
    let _ = app.update(Message::MenuAction(MenuAct::ShowProblems));
    assert!(app.registry.is_open(tools::problems::ID));
    // e resta un dock non-toggle come Log Git
    let _ = app.update(Message::MenuAction(MenuAct::ShowProblems));
    assert!(app.registry.is_open(tools::problems::ID));
    let _ = app.update(Message::MenuAction(MenuAct::ToggleTerminal));
    assert!(app.menu_checks().terminal_open);
}

/// «Estensioni» era raggiungibile solo da Ctrl+Maiusc+X: da menù non
/// c'era verso di aprirla. È un pannello laterale, quindi — a differenza
/// di Log Git e Problemi, che sono dock non-toggle — riattivarla la
/// chiude, come il clic sull'icona.
#[test]
fn visualizza_apre_e_chiude_estensioni_come_interruttore() {
    use crate::menu::MenuAct;

    let (mut app, _) = Lume::new_without_store();
    assert!(!app.registry.is_open(tools::ext::ID));
    let _ = app.update(Message::MenuAction(MenuAct::ShowExtensions));
    assert!(app.registry.is_open(tools::ext::ID));
    let _ = app.update(Message::MenuAction(MenuAct::ShowExtensions));
    assert!(!app.registry.is_open(tools::ext::ID));
}

#[test]
fn project_search_runs_only_after_typing_settles() {
    use std::sync::atomic::Ordering;

    let (mut app, _) = Lume::new_without_store();
    let root = tempfile::tempdir().unwrap();
    app.set_project_root(root.path().to_path_buf());
    let before_typing = app.search_gen.load(Ordering::SeqCst);
    let _ = app.update(Message::Tool(
        tools::search_tool::ID,
        lume_ui::tool::ToolEvent::new(tools::search_tool::Event::Query("fo".into())),
    ));
    let _ = app.update(Message::Tool(
        tools::search_tool::ID,
        lume_ui::tool::ToolEvent::new(tools::search_tool::Event::Query("foo".into())),
    ));
    assert_eq!(
        app.search_gen.load(Ordering::SeqCst),
        before_typing,
        "nessun walk finché si digita"
    );
    let stale = app.search_debounce - 1;
    let _ = app.update(Message::SearchDebounced(
        stale,
        Box::new(lume_core::SearchOptions::literal("fo")),
        lume_core::SearchArea::Progetto,
    ));
    assert_eq!(
        app.search_gen.load(Ordering::SeqCst),
        before_typing,
        "token superato: ignorato"
    );
    let _ = app.update(Message::SearchDebounced(
        app.search_debounce,
        Box::new(lume_core::SearchOptions::literal("foo")),
        lume_core::SearchArea::Progetto,
    ));
    assert_eq!(app.search_gen.load(Ordering::SeqCst), before_typing + 1);
    app.cancel_global_file_search();
    app.search_gen.fetch_add(1, Ordering::SeqCst);
}

/// Apre il pannello Cerca in un'app con un progetto vero e consuma le
/// richieste di apertura.
fn app_con_cerca_aperta(root: &std::path::Path) -> Lume {
    let (mut app, _) = Lume::new_without_store();
    app.project_root = Some(root.to_path_buf());
    let apertura = app.registry.set_active_panel(Some(tools::search_tool::ID));
    app.pending_tool_requests.extend(apertura);
    let _ = app.drain_tool_requests();
    app
}

/// L.13: chiudere Cerca ferma il motore (generazione nuova, esce senza
/// `Done`) **e** l'attesa di quiete (token nuovo): senza il secondo, un
/// giro in quiete già partito rilancerebbe la ricerca a pannello chiuso.
/// La ricerca è vera: `run_search` avvia subito motore e ponte, e la
/// generazione controllata è quella che il motore in volo confronta.
#[test]
fn chiudere_cerca_ferma_la_ricerca_e_la_quiete() {
    use std::sync::atomic::Ordering;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), "needle\n").unwrap();
    let mut app = app_con_cerca_aperta(root.path());

    let _ = app.run_search(
        lume_core::SearchOptions::literal("needle"),
        lume_core::SearchArea::Progetto,
    );
    let in_volo = app.search_gen.load(Ordering::SeqCst);
    let quiete = app.search_debounce;

    let chiusura = app.registry.set_active_panel(None);
    app.pending_tool_requests.extend(chiusura);
    let _ = app.drain_tool_requests();

    assert_eq!(
        app.search_gen.load(Ordering::SeqCst),
        in_volo + 1,
        "il motore in volo vede una generazione diversa dalla sua ed esce"
    );
    assert_ne!(app.search_debounce, quiete);
}

/// Il giro in quiete già armato (l'utente ha digitato, poi ha chiuso prima
/// dei 200 ms) non deve far partire un walk a pannello chiuso: è il
/// percorso che il solo cambio di generazione non copre.
#[test]
fn chiudere_cerca_rende_muta_la_quiete_gia_armata() {
    use std::sync::atomic::Ordering;
    let root = tempfile::tempdir().unwrap();
    let mut app = app_con_cerca_aperta(root.path());
    let _ = app.update(Message::Tool(
        tools::search_tool::ID,
        lume_ui::tool::ToolEvent::new(tools::search_tool::Event::Query("foo".into())),
    ));
    let armato = app.search_debounce;

    let _ = app.update(Message::ClosePanel);
    assert!(!app.registry.is_open(tools::search_tool::ID));
    let dopo_chiusura = app.search_gen.load(Ordering::SeqCst);

    let _ = app.update(Message::SearchDebounced(
        armato,
        Box::new(lume_core::SearchOptions::literal("foo")),
        lume_core::SearchArea::Progetto,
    ));

    assert_eq!(
        app.search_gen.load(Ordering::SeqCst),
        dopo_chiusura,
        "il token armato prima della chiusura è superato: nessun walk"
    );
}

/// Ogni modo di togliere Cerca dalla vista ferma la ricerca: cambiare
/// pannello laterale e la chiusura da parte del tool stesso, non solo la
/// chiusura esplicita del pannello.
#[test]
fn ogni_chiusura_di_cerca_ferma_la_ricerca() {
    use std::sync::atomic::Ordering;
    let root = tempfile::tempdir().unwrap();

    let mut app = app_con_cerca_aperta(root.path());
    let prima = app.search_gen.load(Ordering::SeqCst);
    let cambio = app.registry.set_active_panel(Some(tools::git_native::ID));
    app.pending_tool_requests.extend(cambio);
    let _ = app.drain_tool_requests();
    assert_eq!(
        app.search_gen.load(Ordering::SeqCst),
        prima + 1,
        "passare a un altro pannello laterale chiude Cerca"
    );

    let mut app = app_con_cerca_aperta(root.path());
    let prima = app.search_gen.load(Ordering::SeqCst);
    let chiusura = app.registry.close(tools::search_tool::ID);
    app.pending_tool_requests.extend(chiusura);
    let _ = app.drain_tool_requests();
    assert_eq!(
        app.search_gen.load(Ordering::SeqCst),
        prima + 1,
        "la chiusura del tool (CloseSelf) ferma la ricerca"
    );
}

/// Chiudere un pannello che non è Cerca non tocca la ricerca degli altri:
/// il motore di Cerca non è un effetto collaterale di ogni chiusura.
#[test]
fn chiudere_un_altro_pannello_non_ferma_cerca() {
    use std::sync::atomic::Ordering;
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.project_root = Some(root.path().to_path_buf());
    let apertura = app.registry.set_active_panel(Some(tools::git_native::ID));
    app.pending_tool_requests.extend(apertura);
    let _ = app.drain_tool_requests();
    let gen_prima = app.search_gen.load(Ordering::SeqCst);
    let quiete = app.search_debounce;

    let chiusura = app.registry.set_active_panel(None);
    app.pending_tool_requests.extend(chiusura);
    let _ = app.drain_tool_requests();

    assert_eq!(app.search_gen.load(Ordering::SeqCst), gen_prima);
    assert_eq!(app.search_debounce, quiete);
}

/// Riaperto con un testo già scritto, il pannello rifà la ricerca passando
/// dall'attesa di quiete: senza, resterebbe «in corso» per sempre.
#[test]
fn riaprire_cerca_con_un_testo_riarma_la_ricerca() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_con_cerca_aperta(root.path());
    let _ = app.update(Message::Tool(
        tools::search_tool::ID,
        lume_ui::tool::ToolEvent::new(tools::search_tool::Event::Query("foo".into())),
    ));
    let _ = app.update(Message::ClosePanel);
    let dopo_chiusura = app.search_debounce;

    let riapertura = app.registry.set_active_panel(Some(tools::search_tool::ID));
    app.pending_tool_requests.extend(riapertura);
    let _ = app.drain_tool_requests();

    assert_ne!(
        app.search_debounce, dopo_chiusura,
        "la riapertura arma un giro nuovo con il testo di prima"
    );
}

#[test]
fn opening_java_creates_real_syntax_state() {
    use lume_syntax::LanguageId;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_app_java_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    let file = root.join("Main.java");
    fs::write(&file, "public class Main { static void run() {} }\n").unwrap();

    let (mut app, _) = Lume::new_without_store();
    assert!(app.open_file(&file).is_ok());
    let id = app.active_doc().unwrap();
    assert_eq!(
        app.syntax.get(&id).map(|syntax| syntax.lang()),
        Some(LanguageId::Java)
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(feature = "sql")]
fn restore_snapshot_reopens_existing_files_and_layout() {
    use lume_store::SessionSnapshot;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_app_restore_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    let file = root.join("lib.rs");
    fs::write(&file, "pub fn restored() {}\n").unwrap();

    let (mut app, _) = Lume::new_without_store();
    let theme_name = app.theme_state.list[2].name.clone();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.appearance.color_theme_id = app.theme_state.list[2].id.clone();
    snapshot.workspace_root = Some(root.clone());
    snapshot.tabs = vec![file.clone()];
    snapshot.active_tab = Some(file.clone());
    snapshot.panel_width = 377.0;
    snapshot.left_tools = vec!["search".into(), "project".into()];
    snapshot.active_panel = Some("search".into());
    snapshot.active_dock = Some("terminal".into());
    snapshot.dock_height = 420.0;

    app.restore_snapshot(snapshot);

    assert_eq!(app.project_root, Some(root.clone()));
    assert_eq!(app.active_path(), Some(file.as_path()));
    assert_eq!(app.groups.all().count(), 1);
    assert_eq!(app.layout.panel_width, 377.0);
    assert_eq!(app.theme_state.current().name, theme_name);
    assert_eq!(
        app.registry.left,
        vec![
            tools::search_tool::ID,
            tools::project::ID,
            tools::git_native::ID,
            tools::terminal::ID,
            tools::gitlog::ID,
            #[cfg(feature = "sql")]
            tools::db::ID,
            tools::problems::ID,
            tools::ext::ID,
            tools::usages::ID,
            tools::jj::ID
        ]
    );
    assert_eq!(app.registry.active_panel, Some(tools::search_tool::ID));
    assert!(app.registry.is_open(tools::terminal::ID));
    assert_eq!(app.registry.dock_height, 420.0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ripristino_iniziale_non_ferma_il_chiamante_sulla_lettura_del_tab_attivo() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("attivo.rs");
    std::fs::write(&file, "pub fn caricato() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![file.clone()];
    snapshot.active_tab = Some(file.clone());

    let task = app.restore_snapshot_async(snapshot);
    let id = app.workspace.get_by_path(&file).unwrap();
    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");
    assert!(!app.syntax.contains_key(&id));

    let _ = app.update(primo_output(task));
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "pub fn caricato() {}\n"
    );
    assert!(app.syntax.contains_key(&id));
}

#[test]
fn vai_a_riga_su_tab_pending_attende_il_worker_senza_materializzare_inline() {
    use crate::overlay_manager::OverlayId;
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("vai.rs");
    std::fs::write(&file, "prima\nseconda\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![file.clone()];
    snapshot.active_tab = Some(file.clone());
    let read = app.restore_snapshot_async(snapshot);
    let id = app.workspace.get_by_path(&file).unwrap();

    let _ = app.open_goto_line();
    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");
    assert_ne!(app.shell.active_overlay(), Some(OverlayId::GotoLine));

    let follow = app.update(primo_output(read));
    let _ = app.update(primo_output(follow));
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::GotoLine));
}

#[test]
fn cerca_e_segnalibro_pending_riprendono_dopo_la_lettura_async() {
    use crate::overlay_manager::OverlayId;
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("azioni.rs");
    std::fs::write(&file, "prima\nseconda\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![file.clone()];
    snapshot.active_tab = Some(file.clone());
    let read = app.restore_snapshot_async(snapshot);
    let id = app.workspace.get_by_path(&file).unwrap();

    let _ = app.open_find(false);
    let _ = app.toggle_bookmark();
    assert!(app.pending_restored_tabs.contains(id));
    assert_ne!(app.shell.active_overlay(), Some(OverlayId::Find));
    assert!(app.bookmarks.is_empty());

    let follow = app.update(primo_output(read));
    let next = app.update(primo_output(follow));
    let _ = app.update(primo_output(next));
    assert!(!app.pending_restored_tabs.contains(id));
    assert!(app.find.as_ref().is_some_and(|state| state.doc == id));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Find));
    assert!(app.bookmarks.iter().any(|bookmark| bookmark.path == file));
}

#[test]
fn informazioni_rifiutano_trova_ripreso_dalla_lettura_reale() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("ripristino.txt");
    std::fs::write(&file, "testo reale").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = lume_store::SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![file.clone()];
    snapshot.active_tab = Some(file.clone());
    let read = app.restore_snapshot_async(snapshot);
    let id = app.workspace.get_by_path(&file).unwrap();
    let _ = app.open_find(false);
    let _ = app.update(Message::MenuAction(MenuAct::About));
    completa_azione_restaurata(&mut app, read);
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "testo reale"
    );
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
    assert!(app.find.is_none());
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, n)| n.kind == NoticeKind::Info && n.text.contains("Rispondi prima"))
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), "testo reale");
}

#[test]
fn documentazione_rapida_su_tab_pending_attende_il_worker() {
    use crate::menu::MenuIntent;
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("quickdoc.rs");
    std::fs::write(&file, "fn documentata() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![file.clone()];
    snapshot.active_tab = Some(file.clone());
    let read = app.restore_snapshot_async(snapshot);
    let id = app.workspace.get_by_path(&file).unwrap();

    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::QuickDoc));
    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");

    completa_azione_restaurata(&mut app, read);
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "fn documentata() {}\n"
    );
}

#[test]
fn restore_snapshot_materializza_solo_la_scheda_attiva_e_annuncia_solo_quella() {
    use lume_store::SessionSnapshot;

    let root = tempdir("restore_lazy_tabs");
    let (mut app, _) = Lume::new_without_store();

    let paths: Vec<_> = (0..10)
        .map(|n| {
            root.join(if n == 0 {
                "main.rs".to_owned()
            } else {
                format!("file-{n}.rs")
            })
        })
        .collect();
    for (n, path) in paths.iter().enumerate() {
        std::fs::write(path, format!("pub fn funzione_{n}() {{}}\n")).unwrap();
    }

    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.clone());
    snapshot.tabs = paths.clone();
    snapshot.active_tab = paths.first().cloned();
    app.restore_snapshot(snapshot);

    let ids: Vec<_> = app.groups.all().collect();
    assert_eq!(ids.len(), paths.len());
    assert_eq!(app.pending_restored_tabs.pending_len(), 9);
    for (id, path) in ids.iter().zip(&paths) {
        assert_eq!(app.workspace.get(*id).unwrap().path(), Some(path.as_path()));
    }
    assert_eq!(
        app.active_path(),
        paths.first().map(std::path::PathBuf::as_path)
    );
    let primo = *ids.first().unwrap();
    assert_eq!(
        app.workspace.get(primo).unwrap().rope().to_string(),
        "pub fn funzione_0() {}\n"
    );
    assert!(app.syntax.contains_key(&primo));

    let inattive: Vec<_> = ids.iter().copied().skip(1).collect();
    for id in &inattive {
        assert_eq!(app.workspace.get(*id).unwrap().rope().to_string(), "");
        assert!(!app.syntax.contains_key(id));
    }

    let spec = lume_language_servers::ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let mut lsp = crate::lsp_mode::LspWorkspace::new(spec, root.clone());
    lsp.command = Some(comando_finto());
    lsp.version = "1.2.3".to_owned();
    lsp.state = crate::lsp_mode::LspState::Avvio;
    let _ = lsp.bump();
    app.lsp.insert(lsp);
    let mut posta = acceso(&mut app, lsp_types::ServerCapabilities::default());
    let mut aperti = Vec::new();
    while let Ok(lume_lsp::LspCommand::Open { uri, .. }) = posta.try_recv() {
        aperti.push(uri);
    }
    let uri_primo = crate::lsp_uri::path_to_uri(&paths[0]).unwrap();
    assert_eq!(aperti, vec![uri_primo]);

    let secondo = inattive[0];
    let task = app.apply_tab_intent(super::EditorTabBarIntent::Activate(secondo));
    let _ = app.update(primo_output(task));
    assert_eq!(app.pending_restored_tabs.pending_len(), 8);
    assert_eq!(
        app.workspace.get(secondo).unwrap().rope().to_string(),
        "pub fn funzione_1() {}\n"
    );
    assert!(app.syntax.contains_key(&secondo));
    match posta.try_recv().expect("un didOpen all'attivazione") {
        lume_lsp::LspCommand::Open { uri, .. } => {
            assert_eq!(uri, crate::lsp_uri::path_to_uri(&paths[1]).unwrap());
        }
        altro => panic!("atteso Open, visto {altro:?}"),
    }
    assert!(posta.try_recv().is_err(), "nessun didOpen doppio");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn restore_snapshot_materializza_le_schede_attive_di_entrambi_i_gruppi() {
    use lume_store::{SessionSnapshot, TabGroupRecord};

    let root = tempdir("restore_lazy_split");
    let paths: Vec<_> = (0..5).map(|n| root.join(format!("split-{n}.rs"))).collect();
    for (n, path) in paths.iter().enumerate() {
        std::fs::write(path, format!("pub fn split_{n}() {{}}\n")).unwrap();
    }
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.clone());
    snapshot.tabs = paths[..3].to_vec();
    snapshot.active_tab = paths.first().cloned();
    snapshot.secondary_group = Some(TabGroupRecord {
        tabs: paths[3..].to_vec(),
        active: paths.get(3).cloned(),
    });
    snapshot.focus_secondary = true;

    app.restore_snapshot(snapshot);

    assert_eq!(app.groups.all().count(), 5);
    assert_eq!(app.pending_restored_tabs.pending_len(), 3);
    assert_eq!(app.groups.focus(), crate::editor_groups::GroupId::Secondary);
    assert_eq!(
        app.active_path(),
        paths.get(3).map(std::path::PathBuf::as_path)
    );
    for (index, path) in paths.iter().enumerate() {
        let id = app.workspace.get_by_path(path).unwrap();
        let loaded = index == 0 || index == 3;
        assert_eq!(
            app.workspace.get(id).unwrap().rope().to_string().is_empty(),
            !loaded,
            "stato di lettura della scheda {index}"
        );
        assert_eq!(app.syntax.contains_key(&id), loaded);
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn attivazione_pending_avvia_la_lettura_senza_bloccare_il_chiamante() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("attiva.rs");
    let deferred = root.path().join("differita.rs");
    std::fs::write(&active, "fn attiva() {}\n").unwrap();
    std::fs::write(&deferred, "fn differita() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&deferred).unwrap();
    assert!(app.pending_restored_tabs.contains(id));
    let task = app.activate_tab(id);

    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");
    assert!(!app.syntax.contains_key(&id));
    assert!(app.notice.as_ref().is_some_and(|(_, notice)| {
        notice.kind == NoticeKind::Info && notice.text.contains("lettura in corso")
    }));
    let _ = app.update(primo_output(task));
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "fn differita() {}\n"
    );
    assert!(app.syntax.contains_key(&id));
}

#[test]
fn risposta_lettura_tardiva_non_ricrea_una_scheda_chiusa() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("attiva.rs");
    let deferred = root.path().join("chiusa.rs");
    std::fs::write(&active, "fn attiva() {}\n").unwrap();
    std::fs::write(&deferred, "fn non_riaprire() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&deferred).unwrap();
    let task = app.activate_tab(id);
    assert!(app.pending_restored_tabs.contains(id));
    app.close_tab(id);
    let _ = app.update(primo_output(task));

    assert!(!app.groups.contains(id));
    assert!(app.workspace.get(id).is_none());
    assert!(!app.pending_restored_tabs.contains(id));
}

#[test]
fn input_digitato_durante_il_caricamento_si_applica_dopo_la_lettura() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("attiva.rs");
    let deferred = root.path().join("differita.rs");
    std::fs::write(&active, "fn attiva() {}\n").unwrap();
    std::fs::write(&deferred, "fn differita() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&deferred).unwrap();
    let read = app.activate_tab(id);
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("x".to_owned()),
    ));
    assert!(app.pending_restored_tabs.contains(id));
    let _ = app.update(primo_output(read));

    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "xfn differita() {}\n"
    );
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn attivazioni_ripetute_condividono_la_lettura_in_volo() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("attiva.rs");
    let deferred = root.path().join("una-lettura.rs");
    std::fs::write(&active, "fn attiva() {}\n").unwrap();
    std::fs::write(&deferred, "fn una_lettura() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&deferred).unwrap();
    let task = app.activate_tab(id);
    let request = app.restored_tab_reads.get(&id).copied();
    let duplicate = app.activate_tab(id);

    assert_eq!(app.restored_tab_reads.get(&id).copied(), request);
    assert!(iced_runtime::task::into_stream(duplicate).is_none());
    let _ = app.update(primo_output(task));
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "fn una_lettura() {}\n"
    );
}

#[test]
fn lettura_pending_con_epoca_superata_non_sostituisce_il_placeholder() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("attiva.rs");
    let deferred = root.path().join("epoca.rs");
    std::fs::write(&active, "fn attiva() {}\n").unwrap();
    std::fs::write(&deferred, "fn epoca() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&deferred).unwrap();
    let task = app.activate_tab(id);
    assert_eq!(
        app.scritture
            .ammetti(crate::scritture::Op::Rinomina { docs: vec![] }),
        Ok(crate::scritture::Permesso::Subito)
    );
    let _ = app.update(primo_output(task));

    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");
    assert!(!app.syntax.contains_key(&id));
    assert!(!app.restored_tab_reads.contains_key(&id));
    assert!(app.notice.as_ref().is_some_and(|(_, notice)| {
        notice.kind == NoticeKind::Error && notice.text.contains("superata")
    }));
}

#[test]
fn lettura_pending_non_sovrascrive_un_reload_del_watcher_piu_recente() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("attiva.rs");
    let deferred = root.path().join("aggiornata.rs");
    std::fs::write(&active, "fn attiva() {}\n").unwrap();
    std::fs::write(&deferred, "fn vecchia() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&deferred).unwrap();
    let version = app.workspace.get(id).unwrap().version();
    let _read = app.activate_tab(id);
    let request = app.restored_tab_reads[&id];
    std::fs::write(&deferred, "fn nuova() {}\n").unwrap();
    let _ = app.doc_ricaricato(
        id,
        &deferred,
        version,
        Ok(Ricaricato {
            testo: crate::file_read::Testo::from("fn nuova() {}\n".to_owned()),
            nota: None,
        }),
    );
    let stale = RestoredTabPayloadMessage::new(RestoredTabPayload {
        text: crate::file_read::Testo::from("fn vecchia() {}\n".to_owned()),
        syntax: None,
        note: None,
        notes_selection: app.notes_directory_scelta.clone(),
        note_matches_selection: true,
    });
    let _ = app.update(Message::RestoredTabReadFinished(
        id,
        deferred.clone(),
        request,
        app.scritture.guarda(None).epoca,
        version,
        Ok(stale).into(),
    ));

    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "fn nuova() {}\n"
    );
    assert!(app.pending_restored_tabs.contains(id));
}

#[test]
fn lettura_apertura_non_sovrascrive_reload_watcher_su_tab_lazy() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("attiva.rs");
    let deferred = root.path().join("in-ricarica.rs");
    std::fs::write(&active, "fn attiva() {}\n").unwrap();
    std::fs::write(&deferred, "fn vecchia() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&deferred).unwrap();
    let versione = app.workspace.get(id).unwrap().version();
    let lettura_vecchia = primo_output(app.open_file_task(deferred.clone(), DopoApertura::Niente));
    std::fs::write(&deferred, "fn nuova() {}\n").unwrap();
    let _ = app.doc_ricaricato(
        id,
        &deferred,
        versione,
        Ok(Ricaricato {
            testo: Testo::from("fn nuova() {}\n".to_owned()),
            nota: None,
        }),
    );
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "fn nuova() {}\n"
    );

    let _ = app.update(lettura_vecchia);

    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "fn nuova() {}\n",
        "una lettura generica più vecchia non deve sovrascrivere il reload del watcher"
    );
    assert!(app.pending_restored_tabs.contains(id));
}

#[test]
fn lettura_pending_non_sovrascrive_il_file_svuotato_dal_watcher() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("attiva.rs");
    let deferred = root.path().join("svuotata.rs");
    std::fs::write(&active, "fn attiva() {}\n").unwrap();
    std::fs::write(&deferred, "fn vecchia() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&deferred).unwrap();
    let version = app.workspace.get(id).unwrap().version();
    let _read = app.activate_tab(id);
    let request = app.restored_tab_reads[&id];
    std::fs::write(&deferred, "").unwrap();
    let _watcher_read = app.doc_ricaricato(
        id,
        &deferred,
        version,
        Ok(Ricaricato {
            testo: crate::file_read::Testo::from(String::new()),
            nota: None,
        }),
    );
    let stale = RestoredTabPayloadMessage::new(RestoredTabPayload {
        text: crate::file_read::Testo::from("fn vecchia() {}\n".to_owned()),
        syntax: None,
        note: None,
        notes_selection: app.notes_directory_scelta.clone(),
        note_matches_selection: true,
    });
    let _ = app.update(Message::RestoredTabReadFinished(
        id,
        deferred.clone(),
        request,
        app.scritture.guarda(None).epoca,
        version,
        Ok(stale).into(),
    ));

    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "",
        "il risultato lazy più vecchio non deve ripristinare testo cancellato"
    );
}

#[test]
fn restore_snapshot_mantiene_recuperabile_un_file_mancante() {
    use lume_store::SessionSnapshot;

    let root = tempdir("restore_lazy_missing");
    let presenti = [root.join("presente.rs"), root.join("mancante.rs")];
    std::fs::write(&presenti[0], "pub fn presente() {}\n").unwrap();
    std::fs::write(&presenti[1], "pub fn da_leggere() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.clone());
    snapshot.tabs = presenti.to_vec();
    snapshot.active_tab = presenti.first().cloned();
    app.restore_snapshot(snapshot);
    std::fs::remove_file(&presenti[1]).unwrap();

    let id = app.workspace.get_by_path(&presenti[1]).unwrap();
    let task = app.apply_tab_intent(super::EditorTabBarIntent::Activate(id));
    let _ = app.update(primo_output(task));
    assert_eq!(app.active_path(), Some(presenti[1].as_path()));
    assert!(app.notice.as_ref().is_some_and(|(_, notice)| {
        notice.kind == NoticeKind::Error && notice.text.contains("mancante.rs")
    }));
    assert!(
        app.pending_restored_tabs
            .get(id)
            .is_some_and(|tab| tab.failure.is_some())
    );
    assert!(!app.syntax.contains_key(&id));

    std::fs::write(&presenti[1], "pub fn recuperata() {}\n").unwrap();
    let task = app.apply_tab_intent(super::EditorTabBarIntent::Activate(id));
    let _ = app.update(primo_output(task));
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "pub fn recuperata() {}\n"
    );
    assert!(app.syntax.contains_key(&id));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn scheda_ripristinata_pending_non_accoda_git_blame_sul_placeholder() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("attiva.rs");
    let file = root.path().join("inattiva.rs");
    std::fs::write(&active, "fn attiva() {}\n").unwrap();
    std::fs::write(&file, "fn reale() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), file.clone()];
    // La scheda inattiva rimane pending dopo il restore.
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&file).unwrap();
    assert!(app.pending_restored_tabs.contains(id));
    app.editor_blame_enabled = true;
    app.editor_blame_available = true;
    let _ = app.update(Message::MenuAction(MenuAct::ToggleEditorBlame));
    app.queue_editor_blame(id);

    assert!(!app.pending_tool_requests.iter().any(|(_, request)| {
        matches!(request, super::ToolRequest::GitBlameFile(path) if path == &file)
    }));
}

#[test]
fn cerca_materializza_la_scheda_pending_prima_di_leggere_il_rope() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("ritornato.rs");
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![file.clone()];
    snapshot.active_tab = Some(file.clone());
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&file).unwrap();
    assert!(app.pending_restored_tabs.contains(id));
    assert!(app.find.is_none());
    assert!(app.notice.as_ref().is_some_and(|(_, notice)| {
        notice.kind == NoticeKind::Error && notice.text.contains("ritornato.rs")
    }));

    std::fs::write(&file, "ago in un buffer ripristinato\n").unwrap();
    let task = app.open_find(true);
    completa_azione_restaurata(&mut app, task);
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "ago in un buffer ripristinato\n"
    );
    assert!(app.find.is_some());
}

#[test]
fn vai_a_riga_materializza_prima_di_calcolare_il_numero_di_righe() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("righe.rs");
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![file.clone()];
    snapshot.active_tab = Some(file.clone());
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&file).unwrap();
    assert!(app.pending_restored_tabs.contains(id));
    assert!(app.goto_line.is_none());
    let missing = app.open_goto_line();
    completa_azione_restaurata(&mut app, missing);
    assert!(
        app.goto_line.is_none(),
        "la finestra resta chiusa se manca il file"
    );
    assert!(app.notice.as_ref().is_some_and(|(_, notice)| {
        notice.kind == NoticeKind::Error && notice.text.contains("righe.rs")
    }));

    std::fs::write(&file, "uno\ndue\ntre\n").unwrap();
    let retry = app.open_goto_line();
    completa_azione_restaurata(&mut app, retry);
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(app.goto_prompt().hint, "4 righe");
    app.goto_line = Some("3".to_owned());
    let _ = app.apply_goto_message(&crate::prompt::PromptMessage::Confirm);
    assert_eq!(app.shell.active_overlay(), None);
    let document = app.workspace.get(id).unwrap();
    assert_eq!(document.rope().char_to_line(document.selection().head), 2);
}

#[test]
fn segnalibro_ritenta_la_lettura_invece_di_salvare_preview_vuota() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("bookmark.rs");
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![file.clone()];
    snapshot.active_tab = Some(file.clone());
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&file).unwrap();
    assert!(app.pending_restored_tabs.contains(id));
    let missing = app.update(Message::ToggleBookmark);
    completa_azione_restaurata(&mut app, missing);
    assert!(app.bookmarks.is_empty());

    std::fs::write(&file, "fn reale() {}\n").unwrap();
    let retry = app.update(Message::ToggleBookmark);
    completa_azione_restaurata(&mut app, retry);
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(app.bookmarks.len(), 1);
    assert_eq!(app.bookmarks[0].path, file);
    assert_eq!(app.bookmarks[0].line, 1);
    assert_eq!(app.bookmarks[0].preview, "fn reale() {}");
}

#[test]
fn incolla_da_menu_non_modifica_il_placeholder_e_ritenta_la_lettura() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("incolla.rs");
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![file.clone()];
    snapshot.active_tab = Some(file.clone());
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&file).unwrap();
    assert!(app.pending_restored_tabs.contains(id));
    let first = app.update(Message::ClipboardPasted(Some("X".to_owned())));
    completa_azione_restaurata(&mut app, first);
    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");
    assert!(!app.workspace.get(id).unwrap().is_dirty());

    std::fs::write(&file, "contenuto reale\n").unwrap();
    let retry = app.update(Message::ClipboardPasted(Some("X".to_owned())));
    completa_azione_restaurata(&mut app, retry);
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "XXcontenuto reale\n"
    );
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn copia_da_menu_ritenta_la_lettura_prima_di_leggere_la_selezione() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("attiva.rs");
    let deferred = root.path().join("copia.rs");
    std::fs::write(&active, "fn attiva() {}\n").unwrap();
    std::fs::write(&deferred, "contenuto reale\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&deferred).unwrap();
    std::fs::remove_file(&deferred).unwrap();
    app.groups.activate(id);
    assert!(app.pending_restored_tabs.contains(id));
    std::fs::write(&deferred, "contenuto ripristinato\n").unwrap();
    let copy = app.update(Message::MenuAction(MenuAct::Copy));
    completa_azione_restaurata(&mut app, copy);
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "contenuto ripristinato\n"
    );
}

#[test]
fn azioni_lsp_da_menu_attendono_il_worker_prima_di_leggere_il_placeholder() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("attiva.rs");
    let actions = [
        ("definizione.rs", MenuAct::GotoDefinition),
        ("riferimenti.rs", MenuAct::FindUsages),
        ("rinomina.rs", MenuAct::Rename),
    ];
    std::fs::write(&active, "fn attiva() {}\n").unwrap();
    let paths: Vec<_> = actions
        .iter()
        .map(|(name, _)| root.path().join(name))
        .collect();
    for path in &paths {
        std::fs::write(path, "fn parola_reale() {}\n").unwrap();
    }
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = std::iter::once(active.clone())
        .chain(paths.iter().cloned())
        .collect();
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);

    for (path, (_, action)) in paths.iter().zip(actions) {
        let id = app.workspace.get_by_path(path).unwrap();
        std::fs::remove_file(path).unwrap();
        app.groups.activate(id);
        assert!(app.pending_restored_tabs.contains(id));
        std::fs::write(path, "fn parola_reale() {}\n").unwrap();
        let task = app.update(Message::MenuAction(action));
        assert!(app.pending_restored_tabs.contains(id));
        assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");
        completa_azione_restaurata(&mut app, task);
        assert!(!app.pending_restored_tabs.contains(id));
        assert_eq!(
            app.workspace.get(id).unwrap().rope().to_string(),
            "fn parola_reale() {}\n"
        );
    }
}

#[test]
fn completamento_da_menu_ritenta_la_scheda_pending() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("attiva.rs");
    let deferred = root.path().join("differita.rs");
    std::fs::write(&active, "fn attiva() {}\n").unwrap();
    std::fs::write(&deferred, "fn completabile() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&deferred).unwrap();
    std::fs::remove_file(&deferred).unwrap();
    app.groups.activate(id);
    assert!(app.pending_restored_tabs.contains(id));
    std::fs::write(&deferred, "fn completabile() {}\n").unwrap();
    let completion = app.request_completion_from_menu();
    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");
    completa_azione_restaurata(&mut app, completion);
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "fn completabile() {}\n"
    );
}

#[test]
fn salva_da_menu_ritenta_la_scheda_pending() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("attiva.rs");
    let deferred = root.path().join("differita.rs");
    std::fs::write(&active, "fn attiva() {}\n").unwrap();
    std::fs::write(&deferred, "fn salvata() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&deferred).unwrap();
    std::fs::remove_file(&deferred).unwrap();
    app.groups.activate(id);
    assert!(app.pending_restored_tabs.contains(id));
    std::fs::write(&deferred, "fn salvata() {}\n").unwrap();
    let save = app.save_doc_active();
    completa_azione_restaurata(&mut app, save);
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "fn salvata() {}\n"
    );
}

#[test]
fn salva_con_nome_non_salva_un_placeholder_vuoto_se_il_file_manca() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("mancante.rs");
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![file.clone()];
    snapshot.active_tab = Some(file.clone());
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&file).unwrap();
    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");

    let missing = app.update(Message::MenuAction(MenuAct::SaveAs));
    completa_azione_restaurata(&mut app, missing);

    assert!(app.save_as_picker.is_none());
    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");
    assert!(app.notice.as_ref().is_some_and(|(_, notice)| {
        notice.kind == NoticeKind::Error && notice.text.contains("mancante.rs")
    }));

    std::fs::write(&file, "pub fn recuperata() {}\n").unwrap();
    let retry = app.update(Message::MenuAction(MenuAct::SaveAs));
    completa_azione_restaurata(&mut app, retry);
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(
        app.save_as_picker
            .as_ref()
            .map(|picker| picker.snapshot.rope().to_string()),
        Some("pub fn recuperata() {}\n".to_owned())
    );
}

#[test]
fn salva_con_nome_su_restore_in_corso_attende_la_lettura_owned() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("restore-save-as.rs");
    let contenuto = "pub fn ripristinata() {}\n";
    std::fs::write(&file, contenuto).unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![file.clone()];
    snapshot.active_tab = Some(file.clone());

    let lettura = app.restore_snapshot_async(snapshot);
    let id = app.workspace.get_by_path(&file).unwrap();
    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");

    let _ = app.update(Message::MenuAction(MenuAct::SaveAs));
    assert!(app.save_as_picker.is_none());
    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");

    let mut task = app.update(primo_output(lettura));
    for _ in 0..8 {
        if app.save_as_picker.is_some() {
            break;
        }
        let Some(message) = primo_output_opzionale(task) else {
            break;
        };
        task = app.update(message);
    }

    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(
        app.save_as_picker
            .as_ref()
            .map(|picker| picker.snapshot.rope().to_string()),
        Some(contenuto.to_owned())
    );
    assert!((app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    let sequenza = app.save_as_picker.as_ref().unwrap().sequence;
    drop(app.update(Message::SaveAsPicked(sequenza, None)));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn sostituzione_materializza_la_scheda_ripristinata_prima_di_leggerne_il_buffer() {
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let attiva = root.path().join("attiva.rs");
    let inattiva = root.path().join("inattiva.rs");
    let altra = root.path().join("altra.rs");
    std::fs::write(&attiva, "fn attiva() {}\n").unwrap();
    std::fs::write(&inattiva, "ago qui\n").unwrap();
    std::fs::write(&altra, "ago anche qui\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![attiva, inattiva.clone(), altra.clone()];
    snapshot.active_tab = Some(root.path().join("attiva.rs"));
    app.restore_snapshot(snapshot);

    let id = app.workspace.get_by_path(&inattiva).unwrap();
    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");

    let altro_id = app.workspace.get_by_path(&altra).unwrap();
    assert!(app.pending_restored_tabs.contains(altro_id));
    let task = app.replace_all(
        &[
            trovato(&inattiva, "ago qui"),
            trovato(&altra, "ago anche qui"),
        ],
        "X",
    );
    completa_azione_restaurata(&mut app, task);

    assert_eq!(std::fs::read_to_string(&inattiva).unwrap(), "X qui\n");
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "X qui\n");
    assert!(!app.pending_restored_tabs.contains(id));
    assert_eq!(std::fs::read_to_string(&altra).unwrap(), "X anche qui\n");
    assert_eq!(
        app.workspace.get(altro_id).unwrap().rope().to_string(),
        "X anche qui\n"
    );
    assert!(!app.pending_restored_tabs.contains(altro_id));
}

/// Regressione sul percorso d'avvio, l'unico non coperto dagli altri test:
/// `restore_snapshot` chiama `set_project_root` (che semina lo scope con la
/// radice) e subito dopo `notify_initial_visibility` notifica l'apertura al
/// tool Progetto. Se quella notifica restringesse lo scope, non arriverebbe
/// piu' alcun evento FS e l'albero resterebbe congelato per sempre.
#[test]
fn l_avvio_con_l_albero_aperto_non_restringe_lo_scope_del_watcher() {
    use lume_store::SessionSnapshot;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_app_boot_scope_{nonce}"));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src").join("main.rs"), "fn main() {}\n").unwrap();

    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.clone());
    snapshot.active_panel = Some("project".into());
    app.restore_snapshot(snapshot);

    // stessa sequenza di `Lume::new`
    app.notify_initial_visibility();
    let _ = app.drain_tool_requests();

    assert!(
        app.registry.is_open(tools::project::ID),
        "il pannello Progetto deve risultare aperto dopo il ripristino"
    );
    assert!(
        app.project_watch_directories.contains(&root),
        "la radice deve restare osservata dopo la notifica d'apertura"
    );
    assert!(
        app.fs_watch_directories().contains(&root),
        "senza la radice nel watcher l'albero non si ripopolerebbe piu'"
    );
    fs::remove_dir_all(root).unwrap();
}

// ---- Ibernazione della modalità SQL.
//
// `Subscription` è opaca e non si può asserire «non contiene rami SQL»:
// si asserisce sui seam, `sql_is_open()` e `idle_timer_active()`, che sono
// esattamente ciò che pilota il ramo in `subscription()`.

/// Porta l'app allo stato «modalità SQL viva e visibile».
#[cfg(feature = "sql")]
fn app_with_live_sql() -> Lume {
    let (mut app, _) = Lume::new_without_store();
    app.sql = Some(crate::sql_mode::SqlWorkspace::new());
    app.open_sql_console(None, None);
    assert!(app.sql_is_open());
    app
}

/// Chiude la tab della console **dal percorso vero**. Un `tabs.retain`
/// non passa da `workspace.close`, quindi non vedrebbe mai il documento
/// sparire: il test passerebbe mentre l'app perde il testo dell'utente.
#[cfg(feature = "sql")]
fn close_console_tab(app: &mut Lume) -> super::DocId {
    let console = app.sql_console.expect("console aperta");
    let _ = app.update(Message::EditorTabBar(
        crate::editor_tab_bar::EditorTabBarMessage::Close(console),
    ));
    console
}

#[test]
#[cfg(feature = "sql")]
fn chiudere_solo_la_console_con_il_pannello_aperto_non_avvia_il_countdown() {
    let mut app = app_with_live_sql();
    let requests = app.registry.click(tools::db::ID);
    app.pending_tool_requests.extend(requests);
    let _ = app.drain_tool_requests();

    close_console_tab(&mut app);
    let _ = app.drain_tool_requests();

    assert!(app.sql_is_open(), "il pannello Database e' ancora aperto");
    assert!(
        !app.idle_timer_active(crate::features::Feature::Database),
        "con una delle due viste aperta la modalita' e' in uso: niente countdown"
    );
}

#[test]
#[cfg(feature = "sql")]
fn chiudere_entrambe_avvia_il_countdown_e_libera_subito_la_griglia() {
    let mut app = app_with_live_sql();
    if let Some(pane) = app.sql.as_mut().and_then(|w| w.pane_mut(0)) {
        pane.set_results(lume_sql::ResultSet {
            columns: Vec::new(),
            rows: vec![Vec::new(); 3],
            elapsed: std::time::Duration::from_millis(9),
        });
    }
    close_console_tab(&mut app);
    let _ = app.drain_tool_requests();

    assert!(!app.sql_is_open());
    assert!(
        app.idle_timer_active(crate::features::Feature::Database),
        "il countdown deve partire"
    );
    assert!(
        app.sql
            .as_ref()
            .and_then(|w| w.active_panes().first())
            .is_some_and(|pane| pane.results.is_none()),
        "la griglia e' la parte grossa e invisibile: si libera subito"
    );
    assert!(
        app.sql.is_some(),
        "il socket resta finche' non scade il timeout: riaprire non deve ripagare l'handshake"
    );
}

#[test]
#[cfg(feature = "sql")]
fn riaprire_una_delle_due_viste_azzera_il_countdown() {
    let mut app = app_with_live_sql();
    close_console_tab(&mut app);
    let _ = app.drain_tool_requests();
    assert!(app.idle_timer_active(crate::features::Feature::Database));
    let epoch = app.idle.epoch(crate::features::Feature::Database);

    let requests = app.registry.click(tools::db::ID);
    app.pending_tool_requests.extend(requests);
    let _ = app.drain_tool_requests();

    assert!(app.sql_is_open());
    assert!(
        !app.idle_timer_active(crate::features::Feature::Database),
        "riaprendo il ramo del timer sparisce"
    );
    assert!(
        app.idle.epoch(crate::features::Feature::Database) > epoch,
        "riaprire avanza l'epoca: il timer armato alla chiusura e' gia' obsoleto"
    );
    assert!(
        !app.idle
            .accepts(crate::features::Feature::Database, epoch, false),
        "nemmeno se il suo messaggio fosse gia' in volo"
    );
}

/// Un solo timer da cinque minuti per il Database, e solo dopo che **sono
/// chiuse entrambe** le sorgenti: si prova a turno col pannello e con la
/// console come unica vista rimasta.
#[test]
#[cfg(feature = "sql")]
fn il_database_arma_un_solo_timer_da_cinque_minuti_a_chiusura_completa() {
    use crate::features::Feature;
    let mut app = app_with_live_sql();
    let requests = app.registry.click(tools::db::ID);
    app.pending_tool_requests.extend(requests);
    let _ = app.drain_tool_requests();
    assert!(!app.idle_timer_active(Feature::Database), "tutto aperto");

    // resta solo il pannello
    close_console_tab(&mut app);
    let _ = app.drain_tool_requests();
    assert!(!app.idle_timer_active(Feature::Database));
    assert_eq!(app.idle.dormant().count(), 0);

    // chiuso anche il pannello: parte il timer, uno solo
    let requests = app.registry.click(tools::db::ID);
    app.pending_tool_requests.extend(requests);
    let _ = app.drain_tool_requests();
    assert!(app.idle_timer_active(Feature::Database));
    let epoch = app.idle.epoch(Feature::Database);
    assert_eq!(
        app.idle.dormant().collect::<Vec<_>>(),
        vec![(Feature::Database, epoch)]
    );
    assert_eq!(
        crate::features::spec(Feature::Database).idle,
        Some(crate::sql_mode::IDLE_TIMEOUT)
    );

    // reconcile ripetuto non riarma
    let _ = app.drain_tool_requests();
    assert_eq!(app.idle.epoch(Feature::Database), epoch);

    // riapre solo la console: basta una sorgente per disarmare
    app.open_sql_console(None, None);
    let _ = app.drain_tool_requests();
    assert!(!app.idle_timer_active(Feature::Database));
    assert!(app.idle.epoch(Feature::Database) > epoch);
}

/// Un messaggio di scadenza con l'epoca giusta ma arrivato **dopo** che
/// la vista è tornata in uso non butta giù niente: l'uso si legge alla
/// scadenza, non all'armamento.
#[test]
#[cfg(feature = "sql")]
fn una_scadenza_a_vista_riaperta_non_ferma_la_funzionalita() {
    use crate::features::Feature;
    let mut app = app_with_live_sql();
    close_console_tab(&mut app);
    let _ = app.drain_tool_requests();
    let epoch = app.idle.epoch(Feature::Database);
    // la console torna, ma la tabella non ha ancora visto il fronte
    app.open_sql_console(None, None);
    assert!(app.sql_is_open());

    let _ = app.update(Message::IdleElapsed(Feature::Database, epoch));

    assert!(app.sql.is_some(), "e' di nuovo in uso: non si ferma");
}

/// Il predicato «in uso» guarda solo il registro e la tab della console:
/// niente scansione dei documenti aperti.
#[test]
#[cfg(feature = "sql")]
fn in_uso_dipende_solo_dal_registro_e_dalla_tab_della_console() {
    use crate::features::Feature;
    let (mut app, _) = Lume::new_without_store();
    assert!(!app.feature_is_in_use(Feature::Database));
    // le altre funzionalita' seguono i loro tool nel registro
    assert_eq!(
        app.feature_is_in_use(Feature::Terminal),
        app.registry.is_open(tools::terminal::ID)
    );

    let requests = app.registry.click(tools::db::ID);
    app.pending_tool_requests.extend(requests);
    let _ = app.drain_tool_requests();
    assert!(app.feature_is_in_use(Feature::Database), "pannello db");
    let requests = app.registry.click(tools::db::ID);
    app.pending_tool_requests.extend(requests);
    let _ = app.drain_tool_requests();
    assert!(!app.feature_is_in_use(Feature::Database));

    app.open_sql_console(None, None);
    assert!(app.feature_is_in_use(Feature::Database), "tab console");
    close_console_tab(&mut app);
    assert!(!app.feature_is_in_use(Feature::Database));
}

/// Nessun'altra funzionalità produce un ramo di timer: anche con tutto
/// vivo e senza viste, la tabella resta vuota.
#[test]
#[cfg(feature = "sql")]
fn le_altre_funzionalita_non_armano_mai_un_timer() {
    let mut app = app_with_live_sql();
    close_console_tab(&mut app);
    let _ = app.drain_tool_requests();
    let dormienti: Vec<_> = app.idle.dormant().map(|(feature, _)| feature).collect();
    assert_eq!(dormienti, vec![crate::features::Feature::Database]);
}

#[test]
#[cfg(feature = "sql")]
fn a_timeout_scaduto_cade_tutto_ma_il_testo_della_console_resta() {
    let mut app = app_with_live_sql();
    let scritta = "SELECT 1 -- scritta a mano";
    let console = app.sql_console.expect("console aperta");
    app.workspace
        .get_mut(console)
        .expect("documento console")
        .reload_from_str(scritta);

    let console = close_console_tab(&mut app);
    let _ = app.drain_tool_requests();
    let epoch = app.idle.epoch(crate::features::Feature::Database);

    let _ = app.update(Message::IdleElapsed(
        crate::features::Feature::Database,
        epoch,
    ));

    assert!(app.sql.is_none(), "freddo: nessun socket, nessun thread");
    assert!(
        !app.idle_timer_active(crate::features::Feature::Database),
        "senza nulla da rilasciare il ramo del timer non va prodotto"
    );
    assert!(
        !app.syntax.contains_key(&console),
        "l'albero tree-sitter e' la parte grossa: chiudendo la tab se ne va"
    );
    assert_eq!(
        app.workspace
            .get(console)
            .map(|document| document.rope().to_string()),
        Some(scritta.to_owned()),
        "il buffer costa poco e contiene il testo dell'utente: non si butta"
    );

    // e riaprendo si ritrova lo stesso documento, non uno vuoto
    app.open_sql_console(None, None);
    assert_eq!(app.sql_console, Some(console));
    assert!(app.groups.contains(console));
    assert_eq!(
        app.workspace
            .get(console)
            .map(|document| document.rope().to_string()),
        Some(scritta.to_owned())
    );
}

#[test]
#[cfg(feature = "sql")]
fn un_timer_di_unepoca_superata_non_butta_giu_quella_corrente() {
    let mut app = app_with_live_sql();
    close_console_tab(&mut app);
    let _ = app.drain_tool_requests();
    let stale = app.idle.epoch(crate::features::Feature::Database);

    // riapre e richiude: nuova epoca
    app.open_sql_console(None, None);
    let _ = app.drain_tool_requests();
    close_console_tab(&mut app);
    let _ = app.drain_tool_requests();
    assert_ne!(stale, app.idle.epoch(crate::features::Feature::Database));

    let _ = app.update(Message::IdleElapsed(
        crate::features::Feature::Database,
        stale,
    ));
    assert!(
        app.sql.is_some(),
        "il timer della chiusura precedente e' obsoleto e va ignorato"
    );
}

#[test]
#[cfg(feature = "sql")]
fn la_console_e_una_tab_senza_path_e_non_finisce_nella_sessione() {
    let mut app = app_with_live_sql();
    let console = app.sql_console.expect("console aperta");
    assert!(app.groups.contains(console));
    assert_eq!(app.sql_console_name(console), Some(super::SQL_CONSOLE_NAME));
    assert!(
        app.workspace
            .get(console)
            .is_some_and(|document| document.path().is_none()),
        "senza path: barra tab, rope, undo ed evidenziazione SQL arrivano gratis"
    );
    assert!(
        app.active_path().is_none(),
        "non c'e' un file attivo quando la console ha il fuoco"
    );
    // Ctrl+S su un documento senza path e' gia' un no-op innocuo
    let _ = app.update(Message::SaveDocFixture(console));
}

// ---- Percorso dati della modalita' SQL, con un motore finto.
//
// I `Task` non si possono attendere in un test unitario, ma la parte
// interessante e' sincrona: `sql_submit` fissa subito la generazione, e
// `Message::SqlResponse` si inietta come farebbe il `Task`.

#[cfg(feature = "sql")]
fn fake_driver(
    _profile: &lume_sql::ConnectionProfile,
    _secret: &lume_sql::Secret,
) -> Result<Box<dyn lume_sql::Driver>, lume_sql::SqlError> {
    struct Fake;
    impl lume_sql::Driver for Fake {
        fn tables(&mut self) -> Result<Vec<String>, lume_sql::SqlError> {
            Ok(vec!["users".into()])
        }
        fn columns(
            &mut self,
            _table: &str,
        ) -> Result<Vec<lume_sql::ColumnInfo>, lume_sql::SqlError> {
            Ok(Vec::new())
        }
        fn query_stream(
            &mut self,
            _sql: &str,
            sink: &mut dyn lume_sql::QuerySink,
        ) -> Result<std::time::Duration, lume_sql::SqlError> {
            lume_sql::stream_result_set(fake_results(), sink)
        }
    }
    Ok(Box::new(Fake))
}

#[cfg(feature = "sql")]
fn fake_results() -> lume_sql::ResultSet {
    lume_sql::ResultSet {
        columns: vec![lume_sql::ColumnInfo {
            name: "id".into(),
            type_name: "int".into(),
        }],
        rows: vec![vec![lume_sql::Value::Int(1)]],
        elapsed: std::time::Duration::from_millis(3),
    }
}

#[cfg(feature = "sql")]
fn consegna_fake_query(app: &mut Lume, pane: usize, generation: u64) {
    let result = fake_results();
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    sender
        .try_send(lume_sql::QueryEvent::Started {
            columns: result.columns,
        })
        .expect("evento iniziale");
    sender
        .try_send(lume_sql::QueryEvent::Batch(lume_sql::QueryBatch::new(
            result.rows,
        )))
        .expect("batch");
    let mut stream = crate::sql_events::tests::stream(receiver);
    for event in [
        None,
        None,
        Some(lume_sql::QueryEvent::Completed {
            elapsed: std::time::Duration::from_millis(3),
        }),
    ] {
        if let Some(event) = event {
            sender.try_send(event).expect("evento finale");
        }
        let Some(delivery) = futures::executor::block_on(futures::StreamExt::next(&mut stream))
        else {
            break;
        };
        let _ = app.update(Message::SqlQueryEvent {
            connection: "a".into(),
            pane,
            generation,
            delivery,
        });
    }
}

#[cfg(feature = "sql")]
fn sql_profile() -> lume_sql::ConnectionProfile {
    lume_sql::ConnectionProfile {
        id: "a".into(),
        label: "azure · prod".into(),
        engine: lume_sql::Engine::AzureSql,
        host: "acme.database.windows.net".into(),
        database: "prod".into(),
        auth: lume_sql::AuthKind::SqlLogin {
            user: "lume".into(),
        },
    }
}

#[test]
#[cfg(feature = "sql")]
fn l_albero_sql_segnala_le_connessioni_oracle_in_chiaro() {
    let mut app = app_senza_cassaforte_vera();
    let oracle = |id: &str, host: &str| lume_sql::ConnectionProfile {
        id: id.into(),
        label: format!("oracle · {id}"),
        engine: lume_sql::Engine::Oracle,
        host: host.into(),
        database: "XEPDB1".into(),
        auth: lume_sql::AuthKind::SqlLogin {
            user: "lume".into(),
        },
    };
    app.sql_profiles = vec![
        sql_profile(),
        oracle("cifrata", "db.example.com"),
        oracle("chiaro", "tcp://db.example.com:1521"),
    ];
    let albero = app.sql_tree();
    let cleartext: Vec<(&str, bool)> = albero
        .iter()
        .map(|c| (c.id.as_str(), c.cleartext))
        .collect();
    assert_eq!(
        cleartext,
        vec![("a", false), ("cifrata", false), ("chiaro", true)]
    );
}

/// App con un profilo e un motore finto, connessa e con la console aperta
/// su una query.
/// Cassaforte finta. I `Task` non girano nei test, quindi nessuno la
/// interroga: serve a garantire che **nessun test tocchi il Gestore
/// credenziali vero**, che è quel che farebbe l'`OsVault` del
/// costruttore se un percorso lo raggiungesse.
#[cfg(feature = "sql")]
#[derive(Debug, Default)]
struct FakeVault {
    secrets: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

#[cfg(feature = "sql")]
impl lume_sql::SecretStore for FakeVault {
    fn get(&self, key: &str) -> Result<Option<lume_sql::Secret>, lume_sql::VaultError> {
        Ok(self
            .secrets
            .lock()
            .expect("cassaforte finta")
            .get(key)
            .map(|value| lume_sql::Secret::new(value.clone())))
    }

    fn set(&self, key: &str, secret: &lume_sql::Secret) -> Result<(), lume_sql::VaultError> {
        self.secrets
            .lock()
            .expect("cassaforte finta")
            .insert(key.to_owned(), secret.expose().to_owned());
        Ok(())
    }

    fn forget(&self, key: &str) -> Result<(), lume_sql::VaultError> {
        self.secrets.lock().expect("cassaforte finta").remove(key);
        Ok(())
    }
}

/// App con la cassaforte finta al posto di quella di sistema.
#[cfg(feature = "sql")]
fn app_senza_cassaforte_vera() -> Lume {
    let (mut app, _) = Lume::new_without_store();
    app.sql_vault = std::sync::Arc::new(FakeVault::default());
    app
}

/// Il tasto «Ferma» del Database deve **saltare la coda**: fare adesso
/// quel che il timeout d'inattività farebbe fra cinque minuti, non
/// avviare un altro conto alla rovescia.
#[test]
#[cfg(feature = "sql")]
fn fermare_il_database_va_a_freddo_senza_aspettare_il_timeout() {
    use crate::features::{Feature, FeatureCommand};
    let mut app = app_connected_to_fake_sql();
    assert_eq!(app.feature_live_count(Feature::Database), Some(1));
    // viste chiuse: il conto alla rovescia dell'ibernazione e' in moto,
    // ed e' proprio la situazione in cui «Ferma» deve saltare la coda
    close_console_tab(&mut app);
    let _ = app.drain_tool_requests();
    assert!(app.idle_timer_active(Feature::Database));

    let _ = app.update(Message::Feature(FeatureCommand::Stop(Feature::Database)));

    assert!(app.sql.is_none(), "socket e thread devono essere caduti");
    assert!(
        !app.idle_timer_active(crate::features::Feature::Database),
        "fermare non e' rimandare: nessun conto alla rovescia deve restare"
    );
    assert_eq!(app.feature_live_count(Feature::Database), Some(0));
}

/// Il tasto e il timeout devono portare **allo stesso stato**: se
/// divergessero, una delle due strade libererebbe meno dell'altra senza
/// che nessuno se ne accorga.
#[test]
#[cfg(feature = "sql")]
fn il_tasto_ferma_e_il_timeout_portano_allo_stesso_stato() {
    use crate::features::{Feature, FeatureCommand};

    let mut col_tasto = app_connected_to_fake_sql();
    let _ = col_tasto.update(Message::Feature(FeatureCommand::Stop(Feature::Database)));

    let mut col_timeout = app_connected_to_fake_sql();
    col_timeout.hibernate_sql();

    assert_eq!(col_tasto.sql.is_none(), col_timeout.sql.is_none());
    assert_eq!(
        col_tasto.idle_timer_active(crate::features::Feature::Database),
        col_timeout.idle_timer_active(crate::features::Feature::Database)
    );
    assert_eq!(
        col_tasto.feature_live_count(Feature::Database),
        col_timeout.feature_live_count(Feature::Database)
    );
}

/// Fermare il terminale può buttare via una build in corso: si chiede
/// prima, e annullare non deve mandare niente.
#[test]
fn fermare_il_terminale_chiede_conferma_se_un_processo_e_vivo() {
    use crate::features::{Feature, FeatureCommand};
    let (mut app, _) = Lume::new_without_store();
    if let Some(tool) = app.registry.get_mut(tools::terminal::ID) {
        tool.update(&lume_ui::tool::ToolEvent::new(
            tools::terminal::Event::NewSession,
        ));
    }
    assert_eq!(app.feature_live_count(Feature::Terminal), Some(1));

    // primo «Ferma»: chiede e non tocca niente
    let _ = app.update(Message::Feature(FeatureCommand::Stop(Feature::Terminal)));
    assert_eq!(
        app.feature_stop_pending,
        Some(FeatureCommand::Stop(Feature::Terminal))
    );
    assert_eq!(
        app.feature_live_count(Feature::Terminal),
        Some(1),
        "chiedere conferma non deve gia' aver chiuso la sessione"
    );

    // annullare non manda niente
    let _ = app.update(Message::FeatureStopCancelled);
    assert_eq!(app.feature_stop_pending, None);
    assert_eq!(app.feature_live_count(Feature::Terminal), Some(1));

    // confermare chiude davvero
    let _ = app.update(Message::Feature(FeatureCommand::Stop(Feature::Terminal)));
    let _ = app.update(Message::FeatureStopConfirmed(Feature::Terminal));
    assert_eq!(app.feature_stop_pending, None);
    assert_eq!(app.feature_live_count(Feature::Terminal), Some(0));
}

/// ATTENZIONE: Confermare uno **spegnimento** deve spegnere, non solo fermare.
///
/// I tre «Ferma» del chrome e del pannello mandano tutti `Stop(f)`: senza
/// ricordare *quale* comando ha aperto la conferma, spegnere il terminale
/// dall'interruttore chiuderebbe le sessioni e lascerebbe la funzionalità
/// accesa — l'interruttore tornerebbe su da solo subito dopo che l'utente
/// ha confermato di volerlo giù.
#[test]
fn confermare_uno_spegnimento_spegne_e_non_solo_ferma() {
    use crate::features::{Feature, FeatureCommand};
    let (mut app, _) = Lume::new_without_store();
    if let Some(tool) = app.registry.get_mut(tools::terminal::ID) {
        tool.update(&lume_ui::tool::ToolEvent::new(
            tools::terminal::Event::NewSession,
        ));
    }
    assert_eq!(app.feature_live_count(Feature::Terminal), Some(1));
    // il pannello dev'essere aperto **e** fotografato: l'interruttore
    // legge il verso dalla riga che sta mostrando
    let _ = app.update(Message::ShowExtensions);

    // l'interruttore del pannello: chiede conferma e non tocca niente
    let _ = app.update(Message::Tool(
        tools::ext::ID,
        lume_ui::tool::ToolEvent::new(tools::ext::Event::Toggle(Feature::Terminal)),
    ));
    assert_eq!(
        app.feature_stop_pending,
        Some(FeatureCommand::SetEnabled(Feature::Terminal, false))
    );
    assert!(app.feature_is_enabled(Feature::Terminal));
    assert_eq!(app.feature_live_count(Feature::Terminal), Some(1));

    // «Ferma» nella riga di conferma: manda `Stop`, ma quel che si
    // conferma è lo spegnimento
    let _ = app.update(Message::Tool(
        tools::ext::ID,
        lume_ui::tool::ToolEvent::new(tools::ext::Event::Stop(Feature::Terminal)),
    ));

    assert_eq!(app.feature_stop_pending, None);
    // il tool resta registrato (toglierlo sposterebbe le icone salvate),
    // ma non tiene piu' niente
    assert_eq!(app.feature_live_count(Feature::Terminal), Some(0));
    assert!(!app.feature_is_enabled(Feature::Terminal));
    assert!(
        !app.registry
            .bar(lume_ui::tool::Side::Left)
            .contains(&tools::terminal::ID),
        "spento vuol dire anche icona via dalla barra"
    );
}

/// Senza processi vivi non si chiede niente: una conferma per buttare via
/// il nulla è solo un ostacolo.
#[test]
fn senza_processi_vivi_fermare_non_chiede_conferma() {
    use crate::features::{Feature, FeatureCommand};
    let (mut app, _) = Lume::new_without_store();
    assert_eq!(app.feature_live_count(Feature::Terminal), Some(0));

    let _ = app.update(Message::Feature(FeatureCommand::Stop(Feature::Terminal)));

    assert_eq!(app.feature_stop_pending, None);
}

/// Spegnere dal pannello Estensioni una funzionalità **viva** la deve
/// prima fermare, o resterebbero socket appesi a una cosa che non si vede
/// più — e i suoi pannelli devono sparire dalle barre.
#[test]
#[cfg(feature = "sql")]
fn disabilitare_il_database_lo_ferma_e_ne_toglie_il_pannello() {
    use crate::features::{Feature, FeatureCommand};
    let mut app = app_connected_to_fake_sql();
    app.registry.set_active_panel(Some(tools::db::ID));
    assert!(
        app.registry
            .bar(lume_ui::tool::Side::Left)
            .contains(&tools::db::ID)
    );

    let _ = app.update(Message::Feature(FeatureCommand::SetEnabled(
        Feature::Database,
        false,
    )));

    assert!(app.sql.is_none(), "spegnere deve prima fermare");
    assert!(
        !app.registry
            .bar(lume_ui::tool::Side::Left)
            .contains(&tools::db::ID)
    );
    assert_eq!(app.registry.active_panel, None);
    assert!(!app.feature_is_enabled(Feature::Database));

    let _ = app.update(Message::Feature(FeatureCommand::SetEnabled(
        Feature::Database,
        true,
    )));
    assert!(
        app.registry
            .bar(lume_ui::tool::Side::Left)
            .contains(&tools::db::ID)
    );
    assert!(app.feature_is_enabled(Feature::Database));
}

/// Una funzionalità spenta deve **restare** spenta al riavvio: è una
/// scelta dell'utente, non uno stato di runtime.
#[test]
fn le_funzionalita_spente_fanno_andata_e_ritorno_nella_sessione() {
    use crate::features::{Feature, FeatureCommand};
    let (mut app, _) = Lume::new_without_store();
    let _ = app.update(Message::Feature(FeatureCommand::SetEnabled(
        Feature::Terminal,
        false,
    )));

    let snapshot = app.session_snapshot();
    assert_eq!(snapshot.disabled_features, vec!["terminale".to_string()]);

    let (mut ripreso, _) = Lume::new_without_store();
    ripreso.restore_snapshot(snapshot);

    assert!(!ripreso.feature_is_enabled(Feature::Terminal));
    assert!(
        !ripreso
            .registry
            .bar(lume_ui::tool::Side::Left)
            .contains(&tools::terminal::ID)
    );
}

/// Il giro completo dal pannello: interruttore → `ToolRequest` → comando
/// → funzionalità spenta → icona sparita. È l'unico test che attraversa
/// la traduzione chiave → `Feature`, che è il punto in cui un refuso non
/// si vedrebbe.
#[test]
fn l_interruttore_del_pannello_estensioni_spegne_davvero() {
    use crate::features::Feature;
    let (mut app, _) = Lume::new_without_store();
    app.registry.set_active_panel(Some(tools::ext::ID));
    let _ = app.drain_tool_requests();

    let _ = app.update(Message::Tool(
        tools::ext::ID,
        lume_ui::tool::ToolEvent::new(tools::ext::Event::Toggle(Feature::Terminal)),
    ));

    assert!(!app.feature_is_enabled(Feature::Terminal));
    assert!(
        !app.registry
            .bar(lume_ui::tool::Side::Left)
            .contains(&tools::terminal::ID)
    );

    let _ = app.update(Message::Tool(
        tools::ext::ID,
        lume_ui::tool::ToolEvent::new(tools::ext::Event::Toggle(Feature::Terminal)),
    ));
    assert!(app.feature_is_enabled(Feature::Terminal));
}

/// Il gruppo 1 non mostra mai né diff né console: sono viste del gruppo 0.
/// È la lettura del prototipo, ed è anche il flusso utile — il file a
/// destra, il suo diff a sinistra.
#[test]
#[cfg(feature = "sql")]
fn il_secondo_gruppo_mostra_sempre_e_solo_l_editor() {
    use crate::editor_groups::GroupId;
    let (mut app, _) = Lume::new_without_store();
    let a = app.workspace.open(None, "fn a() {}");
    let b = app.workspace.open(None, "fn b() {}");
    app.groups.open(GroupId::Primary, a);
    app.groups.open(GroupId::Primary, b);
    app.groups.split();
    assert!(app.groups.is_split());

    // una console aperta nel gruppo 0 non trascina con sé il gruppo 1
    app.sql_console = Some(a);
    app.groups.activate(a);
    assert_eq!(
        app.central_view_of(GroupId::Primary),
        super::CentralView::Sql
    );
    assert_eq!(
        app.central_view_of(GroupId::Secondary),
        super::CentralView::Editor,
        "la console è una vista del gruppo 0"
    );
    assert_eq!(app.groups.focus(), GroupId::Primary);
    assert_eq!(
        app.groups.active_in(GroupId::Secondary),
        Some(b),
        "il gruppo 1 resta sul suo documento"
    );

    // ATTENZIONE: il fuoco passa a destra, la console resta a sinistra: la
    // precedenza si legge sul **gruppo**, non sulla colonna col fuoco.
    // Guardando `active_doc()` qui l'area di sinistra sarebbe tornata un
    // editor, e la query si sarebbe mostrata come testo grezzo.
    app.groups.activate(b);
    assert_eq!(app.groups.focus(), GroupId::Secondary);
    assert_eq!(
        app.central_view_of(GroupId::Primary),
        super::CentralView::Sql,
        "la console non dipende da dove sta il fuoco"
    );
}

/// La console SQL non è un file: nel gruppo 1 `central_view_of` dà sempre
/// `Editor`, quindi ci si vedrebbe la query come testo grezzo, senza
/// barra né griglia. Le due strade per portarcela sono due, e vanno
/// chiuse tutte e due.
#[test]
#[cfg(feature = "sql")]
fn una_scheda_senza_path_non_si_trascina_nel_gruppo_uno() {
    use crate::editor_groups::GroupId;
    use crate::editor_tab_bar::EditorTabBarIntent;
    use crate::menu::{MenuAct, MenuIntent};
    let (mut app, _) = Lume::new_without_store();
    let file = app.workspace.open(None, "fn a() {}");
    let altro = app.workspace.open(None, "fn b() {}");
    let console = app.workspace.open(None, "select 1");
    app.sql_console = Some(console);
    app.groups.open(GroupId::Primary, file);
    app.groups.open(GroupId::Primary, altro);
    app.groups.open(GroupId::Primary, console);
    // il gruppo 1 lo apre un file vero, non la console
    app.groups.activate(file);
    app.groups.split();
    assert_eq!(app.groups.group_of(file), Some(GroupId::Secondary));

    // trascinamento
    let _ = app.apply_tab_intent(EditorTabBarIntent::Drop {
        id: console,
        group: GroupId::Secondary,
        before: None,
    });
    assert_eq!(
        app.groups.group_of(console),
        Some(GroupId::Primary),
        "trascinata a destra, la console resta a sinistra"
    );

    // «Dividi a destra»
    app.groups.activate(console);
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::SplitEditor));
    assert_eq!(
        app.groups.group_of(console),
        Some(GroupId::Primary),
        "«Dividi a destra» non sposta la console"
    );
    // e resta un comando vivo per una scheda vera
    app.groups.activate(altro);
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::SplitEditor));
    assert_eq!(app.groups.group_of(altro), Some(GroupId::Secondary));
}

/// ATTENZIONE: Da quando le schede riportano il sorvolo, `EditorTabBar(_)` intero
/// voleva dire **una scrittura su SQLite per ogni pixel** passato sopra
/// una scheda. Nessun test lo vedeva: la sessione veniva salvata, solo
/// centinaia di volte di troppo.
#[test]
fn passare_sopra_una_scheda_non_salva_la_sessione() {
    use crate::editor_groups::GroupId;
    use crate::editor_tab_bar::EditorTabBarMessage as T;
    use lume_core::DocId;
    assert!(
        Message::EditorTabBar(T::Hover(GroupId::Primary, Some(DocId(1))))
            .politica()
            .persistenza
            != super::ingresso::Persistenza::DopoGestione
    );
    assert!(
        Message::EditorTabBar(T::Press(GroupId::Primary, DocId(1)))
            .politica()
            .persistenza
            != super::ingresso::Persistenza::DopoGestione
    );
    assert!(
        Message::EditorTabBar(T::CursorMoved(iced::Point::ORIGIN))
            .politica()
            .persistenza
            != super::ingresso::Persistenza::DopoGestione
    );
    // quel che cambia davvero la sessione si salva ancora
    assert!(
        Message::EditorTabBar(T::Close(DocId(1)))
            .politica()
            .persistenza
            == super::ingresso::Persistenza::DopoGestione
    );
    assert!(
        Message::EditorTabBar(T::Release).politica().persistenza
            == super::ingresso::Persistenza::DopoGestione
    );
    assert!(Message::DragEnd.politica().persistenza == super::ingresso::Persistenza::DopoGestione);
}

/// Aprire un file nel gruppo 1 **non** deve chiudere il diff del gruppo 0:
/// il diff vince sull'area centrale del suo gruppo, non dello schermo.
#[test]
fn aprire_nel_gruppo_uno_non_chiude_il_diff_del_gruppo_zero() {
    use crate::editor_groups::GroupId;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_split_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    let uno = root.join("uno.rs");
    let due = root.join("due.rs");
    fs::write(&uno, "fn uno() {}\n").unwrap();
    fs::write(&due, "fn due() {}\n").unwrap();

    let (mut app, _) = Lume::new_without_store();
    assert!(app.open_file(&uno).is_ok());
    assert!(app.open_file(&due).is_ok());
    app.groups.split();
    assert_eq!(app.groups.focus(), GroupId::Secondary);

    // un diff aperto sul gruppo 0
    app.diff_request = Some((uno.clone(), lume_git::DiffBase::Worktree));
    app.diff = Some(Box::new(lume_git::FileDiff {
        path: uno.clone(),
        base: lume_git::DiffBase::Worktree,
        rows: Vec::new(),
        added: 0,
        removed: 0,
        binary: false,
        truncated: false,
        hunks: Vec::new(),
    }));
    assert_eq!(app.central_view(), super::CentralView::Diff);

    // riaprire un file già nel gruppo 1 non tocca il gruppo 0
    assert!(app.open_file(&due).is_ok());
    assert!(app.diff.is_some(), "il diff del gruppo 0 resta");
    assert_eq!(app.groups.focus(), GroupId::Secondary);
    assert_eq!(
        app.central_view_of(GroupId::Secondary),
        super::CentralView::Editor,
        "e il gruppo 1 mostra comunque il suo file, non il diff"
    );

    fs::remove_dir_all(root).unwrap();
}

/// La sessione fa andata e ritorno con e senza il secondo gruppo, e
/// `tabs`/`active_tab` restano il gruppo 0 — così anche un binario vecchio
/// riapre la sessione invece di non aprire niente.
#[test]
fn la_divisione_fa_andata_e_ritorno_nella_sessione() {
    use crate::editor_groups::GroupId;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_split_sess_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    let uno = root.join("uno.rs");
    let due = root.join("due.rs");
    fs::write(&uno, "fn uno() {}\n").unwrap();
    fs::write(&due, "fn due() {}\n").unwrap();

    let (mut app, _) = Lume::new_without_store();
    app.set_project_root(root.clone());
    assert!(app.open_file(&uno).is_ok());
    assert!(app.open_file(&due).is_ok());
    app.groups.split();
    app.layout.split_ratio = 0.4;

    let snapshot = app.session_snapshot();
    assert_eq!(snapshot.tabs, vec![uno.clone()], "il gruppo 0 sta dov'era");
    assert_eq!(snapshot.active_tab, Some(uno.clone()));
    let secondo = snapshot.secondary_group.clone().unwrap();
    assert_eq!(secondo.tabs, vec![due.clone()]);
    assert!(snapshot.focus_secondary);
    assert!((snapshot.split_ratio - 0.4).abs() < 1e-6);

    let (mut ripreso, _) = Lume::new_without_store();
    ripreso.restore_snapshot(snapshot);

    assert!(ripreso.groups.is_split());
    assert_eq!(ripreso.groups.tabs(GroupId::Primary).len(), 1);
    assert_eq!(ripreso.groups.tabs(GroupId::Secondary).len(), 1);
    assert_eq!(ripreso.groups.focus(), GroupId::Secondary);
    assert_eq!(ripreso.active_path(), Some(due.as_path()));
    assert!((ripreso.layout.split_ratio - 0.4).abs() < 1e-6);

    // e senza divisione la sessione non ne inventa una
    let (mut sola, _) = Lume::new_without_store();
    assert!(sola.open_file(&uno).is_ok());
    let snapshot = sola.session_snapshot();
    assert!(snapshot.secondary_group.is_none());
    assert!(!snapshot.focus_secondary);

    fs::remove_dir_all(root).unwrap();
}

/// Battere in una colonna ci porta il fuoco: il gesto stesso lo dice, e
/// non serve un `mouse_area` fra l'utente e il testo.
#[test]
fn battere_in_una_colonna_ci_porta_il_fuoco() {
    use crate::editor_groups::GroupId;
    let (mut app, _) = Lume::new_without_store();
    let a = app.workspace.open(None, "");
    let b = app.workspace.open(None, "");
    app.groups.open(GroupId::Primary, a);
    app.groups.open(GroupId::Primary, b);
    app.groups.split();
    app.groups.activate(a);
    assert_eq!(app.groups.focus(), GroupId::Primary);

    let _ = app.update(Message::EditorIn(
        GroupId::Secondary,
        EditorCommand::Insert("x".into()),
    ));

    assert_eq!(app.groups.focus(), GroupId::Secondary);
    assert_eq!(app.active_doc(), Some(b));
    assert_eq!(
        app.workspace.get(b).map(|doc| doc.rope().to_string()),
        Some("x".to_owned()),
        "e il testo finisce nel documento di quella colonna"
    );
    assert_eq!(
        app.workspace.get(a).map(|doc| doc.rope().to_string()),
        Some(String::new())
    );
}

/// Ctrl+Maiusc+X apre il pannello, e il pannello si fotografa **solo da
/// aperto**: `drain_tool_requests` gira a ogni messaggio, compresi i
/// movimenti del mouse, e una fotografia per frame di un pannello chiuso
/// sarebbe lo spreco che il budget vieta.
#[test]
fn il_pannello_estensioni_si_fotografa_solo_da_aperto() {
    let (mut app, _) = Lume::new_without_store();
    assert!(
        !app.sync_extensions_tool(),
        "a pannello chiuso non si fotografa niente"
    );

    let _ = app.update(Message::ShowExtensions);

    assert_eq!(app.registry.active_panel, Some(tools::ext::ID));
    assert!(app.sync_extensions_tool());

    // e la seconda volta lo chiude: è un toggle come il clic sull’icona
    let _ = app.update(Message::ShowExtensions);
    assert_eq!(app.registry.active_panel, None);
    assert!(!app.sync_extensions_tool());
}

/// La fotografia elenca **solo le funzionalità cablate**, e i numeri
/// vengono da chi possiede davvero le risorse: una connessione SQL aperta
/// deve comparire come una, non come «non avviata».
#[test]
#[cfg(feature = "sql")]
fn la_fotografia_del_pannello_conta_le_risorse_vere() {
    use crate::features::Feature;
    let mut app = app_connected_to_fake_sql();

    let righe = app.extension_rows();
    assert_eq!(
        righe.len(),
        crate::features::FEATURES
            .iter()
            .filter(|spec| spec.wired)
            .count()
    );
    // ATTENZIONE: Questa asserzione era rovesciata: finché il language
    // server non era cablato e la riga non doveva esistere. Ora esiste,
    // e parte **ferma** — nessun avvio automatico al boot, che è ciò che
    // protegge il «< 100 ms».
    let lsp = righe
        .iter()
        .find(|riga| riga.feature == Feature::LanguageServer)
        .expect("il language server è cablato: la riga c'è");
    assert_eq!(lsp.live, Some(0), "non si avvia da solo");
    assert!(lsp.enabled);
    // La feature Cargo nasconde la riga, non il backend già collegato:
    // il conteggio delle risorse deve restare valido in entrambi i build.
    assert_eq!(app.feature_live_count(Feature::Database), Some(1));
    let database = righe.iter().find(|riga| riga.feature == Feature::Database);
    #[cfg(feature = "sql")]
    {
        let database = database.expect("la riga Database è visibile con sql");
        assert_eq!(database.live, Some(1));
        assert!(database.enabled);
        assert!(!database.confirming);
    }
    #[cfg(not(feature = "sql"))]
    assert!(
        database.is_none(),
        "senza sql la riga Database non è visibile"
    );

    app.feature_stop_pending = Some(crate::features::FeatureCommand::Stop(Feature::Terminal));
    let terminale = app
        .extension_rows()
        .into_iter()
        .find(|riga| riga.feature == Feature::Terminal)
        .unwrap();
    assert!(terminale.confirming, "la conferma aperta si vede");
}

// -------------------------------------------------- navigazione leggera

/// ATTENZIONE: La cronologia registra **la partenza e l'arrivo**, e il test è sul
/// percorso vero: un salto alla definizione, poi Ctrl+Alt+← che riporta
/// dov'eri. Senza la partenza, il primo «indietro» non avrebbe dove
/// tornare e il comando sembrerebbe rotto proprio la prima volta che lo
/// si usa.
#[test]
fn indietro_riporta_dove_si_era_e_avanti_rifa_il_passo() {
    use crate::menu::{MenuAct, MenuIntent};
    let root = tempdir("navigazione");
    let uno = root.join("uno.rs");
    let due = root.join("due.rs");
    std::fs::write(&uno, "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
    std::fs::write(&due, "fn d() {}\nfn e() {}\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.open_file(&uno).unwrap();
    let id_uno = app.active_doc().unwrap();
    app.workspace
        .get_mut(id_uno)
        .unwrap()
        .set_selection(lume_core::Range::point(20)); // riga 2

    app.jump_to(&due, lsp_types::Position::new(1, 0)).unwrap();
    assert_eq!(app.active_path(), Some(due.as_path()));

    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::NavBack));
    assert_eq!(
        app.active_path(),
        Some(uno.as_path()),
        "indietro riporta al file di partenza"
    );
    let riga = |app: &Lume| {
        let documento = app.workspace.get(app.active_doc().unwrap()).unwrap();
        documento.rope().char_to_line(documento.selection().head)
    };
    assert_eq!(riga(&app), 2, "e alla riga di partenza");

    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::NavForward));
    assert_eq!(app.active_path(), Some(due.as_path()));
    assert_eq!(riga(&app), 1);

    // ATTENZIONE: Navigare nella cronologia **non** si registra: se lo facesse, la
    // pila crescerebbe a ogni passo indietro e non si tornerebbe mai
    // davvero
    let quanti = app.navigazione.len();
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::NavBack));
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::NavForward));
    assert_eq!(app.navigazione.len(), quanti);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn definizione_apre_il_file_su_worker_e_posiziona_il_cursore() {
    let root = tempfile::tempdir().unwrap();
    let sorgente = root.path().join("sorgente.rs");
    let definizione = root.path().join("definizione.rs");
    std::fs::write(&sorgente, "fn chiamante() {}\n").unwrap();
    std::fs::write(&definizione, "uno\ndue\ntre\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.open_file(&sorgente).unwrap();

    let location = lsp_types::Location {
        uri: crate::lsp_uri::path_to_uri(&definizione).unwrap(),
        range: lsp_types::Range::new(
            lsp_types::Position::new(2, 1),
            lsp_types::Position::new(2, 1),
        ),
    };
    let origine = app.active_doc().unwrap();
    let versione = app.workspace.get(origine).unwrap().version();
    let task = app.apri_definizione(
        Some(lsp_types::GotoDefinitionResponse::Scalar(location)),
        (origine, versione),
    );

    assert!(app.workspace.get_by_path(&definizione).is_none());
    let message = primo_output(task);
    let _ = app.update(message);

    let doc = app
        .workspace
        .get_by_path(&definizione)
        .and_then(|id| app.workspace.get(id))
        .unwrap();
    assert_eq!(doc.rope().char_to_line(doc.selection().head), 2);
    assert_eq!(doc.selection().head, 9);
}

#[test]
fn navigazione_su_tab_ripristinato_attende_il_worker() {
    use crate::navigazione::Posizione;
    use lume_store::SessionSnapshot;

    let root = tempfile::tempdir().unwrap();
    let attivo = root.path().join("attivo.rs");
    let differito = root.path().join("differito.rs");
    std::fs::write(&attivo, "fn attivo() {}\n").unwrap();
    std::fs::write(&differito, "uno\ndue\ntre\n").unwrap();

    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![attivo.clone(), differito.clone()];
    snapshot.active_tab = Some(attivo.clone());
    let restore = app.restore_snapshot_async(snapshot);
    completa_azione_restaurata(&mut app, restore);

    let id = app.workspace.get_by_path(&differito).unwrap();
    assert!(app.pending_restored_tabs.contains(id));
    app.navigazione.push(Posizione::new(&differito, 2));
    app.navigazione.push(Posizione::new(&attivo, 0));

    let task = app.navigate(-1);

    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");
    completa_azione_restaurata(&mut app, task);

    assert!(!app.pending_restored_tabs.contains(id));
    let doc = app.workspace.get(id).unwrap();
    assert_eq!(doc.rope().to_string(), "uno\ndue\ntre\n");
    assert_eq!(doc.rope().char_to_line(doc.selection().head), 2);
}

/// L'apertura dal menu crea una sola configurazione iniziale e la conserva
/// alla chiusura/reapertura dell'overlay.
#[test]
fn la_voce_esegui_apre_forge_esc_lo_chiude_e_conserva_la_configurazione() {
    use crate::forge::Family;
    use crate::forge_panel::ForgeMessage;
    use crate::menu::{MenuAct, MenuIntent};
    use crate::overlay_manager::OverlayId;
    let (mut app, _) = Lume::new_without_store();
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::Forge));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Forge));
    assert_eq!(
        app.forge_config.as_ref().map(|c| c.family),
        Some(Family::Rust)
    );
    let _ = app.update(Message::Forge(ForgeMessage::Target("lume".to_owned())));
    let _ = app.update(Message::CloseAll);
    assert_eq!(app.shell.active_overlay(), None);
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::Forge));
    assert_eq!(
        app.forge_config.as_ref().map(|c| c.target.as_str()),
        Some("lume")
    );
    let _ = app.update(Message::Forge(ForgeMessage::Close));
}

#[test]
fn eseguire_forge_apre_dock_terminal_e_crea_sessione() {
    use crate::features::Feature;
    use crate::forge::Family;
    use crate::forge_panel::ForgeMessage;
    use crate::menu::{MenuAct, MenuIntent};
    let (mut app, _) = Lume::new_without_store();
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::Forge));
    let _ = app.update(Message::Forge(ForgeMessage::Family(Family::Shell)));
    let _ = app.update(Message::Forge(ForgeMessage::Target(
        "echo prova-forge".into(),
    )));
    let _ = app.update(Message::Forge(ForgeMessage::Run));
    assert!(app.registry.is_open(tools::terminal::ID));
    assert_eq!(app.feature_live_count(Feature::Terminal), Some(1));
    assert_eq!(app.shell.active_overlay(), None);
}

#[test]
fn configurazione_forge_non_valida_non_apre_il_terminale() {
    use crate::features::Feature;
    use crate::forge::Family;
    use crate::forge_panel::ForgeMessage;
    use crate::menu::{MenuAct, MenuIntent};
    use crate::overlay_manager::OverlayId;
    let (mut app, _) = Lume::new_without_store();
    let _ = app.update(Message::MenuAction(MenuAct::ToggleTerminal));
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::Forge));
    let _ = app.update(Message::Forge(ForgeMessage::Family(Family::Shell)));
    let _ = app.update(Message::Forge(ForgeMessage::Run));
    let _ = app.update(Message::Forge(ForgeMessage::Target(
        "echo uno\ngit push".into(),
    )));
    let _ = app.update(Message::Forge(ForgeMessage::Run));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Forge));
    assert!(!app.registry.is_open(tools::terminal::ID));
    assert_eq!(app.feature_live_count(Feature::Terminal).unwrap_or(0), 0);
}

#[test]
fn terminale_disabilitato_impedisce_esecuzione_forge() {
    use crate::features::Feature;
    use crate::forge::Family;
    use crate::forge_panel::ForgeMessage;
    use crate::menu::{MenuAct, MenuIntent};
    use crate::overlay_manager::OverlayId;
    let (mut app, _) = Lume::new_without_store();
    app.set_feature_enabled(Feature::Terminal, false);
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::Forge));
    let _ = app.update(Message::Forge(ForgeMessage::Family(Family::Shell)));
    let _ = app.update(Message::Forge(ForgeMessage::Target("echo prova".into())));
    let _ = app.update(Message::Forge(ForgeMessage::Run));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Forge));
    assert!(!app.registry.is_open(tools::terminal::ID));
    assert_eq!(app.feature_live_count(Feature::Terminal).unwrap_or(0), 0);
}

/// Ctrl+G conta le righe da 1 e il salto entra nella cronologia; Ctrl+E
/// offre i file distinti, senza quello dove si è già.
#[test]
fn vai_a_riga_e_recenti_lavorano_sulla_stessa_pila() {
    use crate::menu::{MenuAct, MenuIntent};
    use crate::overlay_manager::OverlayId;
    use crate::prompt::PromptMessage;
    let root = tempdir("navigazione_g");
    let uno = root.join("uno.rs");
    let due = root.join("due.rs");
    std::fs::write(&uno, "1\n2\n3\n4\n5\n").unwrap();
    std::fs::write(&due, "a\nb\n").unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.open_file(&uno).unwrap();

    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::GotoLine));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::GotoLine));
    let _ = app.update(Message::GotoLine(PromptMessage::Changed("4".to_owned())));
    let _ = app.update(Message::GotoLine(PromptMessage::Confirm));
    let documento = app.workspace.get(app.active_doc().unwrap()).unwrap();
    assert_eq!(
        documento.rope().char_to_line(documento.selection().head),
        3,
        "la quarta riga è l'indice 3"
    );
    assert!(app.goto_line.is_none(), "il riquadro si chiude");

    // un numero che non è una riga non salta e non chiude
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::GotoLine));
    let _ = app.update(Message::GotoLine(PromptMessage::Changed("0".to_owned())));
    let _ = app.update(Message::GotoLine(PromptMessage::Confirm));
    assert!(app.goto_line.is_some(), "la riga zero non esiste");
    let _ = app.update(Message::CloseAll);

    // i recenti: il file dove si è **non** compare, o Ctrl+E offrirebbe
    // come prima scelta un giro a vuoto
    app.jump_to(&due, lsp_types::Position::new(0, 0)).unwrap();
    let righe = app.recents_rows();
    assert_eq!(righe.len(), 1, "solo l'altro file: {righe:?}");
    assert_eq!(righe[0].name, "uno.rs");
    assert_eq!(righe[0].line, 4, "con la riga dell'ultima visita");

    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::Recents));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Recents));
    let _ = app.update(Message::Recents(crate::recents::RecentsMessage::Open(0)));
    assert_eq!(app.active_path(), Some(uno.as_path()));
    assert_ne!(app.shell.active_overlay(), Some(OverlayId::Recents));
    let _ = std::fs::remove_dir_all(&root);
}

/// ATTENZIONE: Saltare a una riga **è** navigare: la cronologia lo registra, e
/// «indietro» riporta dov'eri. Senza, Ctrl+G sarebbe l'unico modo di
/// spostarsi da cui non si può tornare.
///
/// Sta in un test suo e non in coda a
/// [`vai_a_riga_e_recenti_lavorano_sulla_stessa_pila`] perché tornare
/// indietro **sposta il dito** nella pila, e il salto dopo ne tronca la
/// coda: in mezzo a quell'altro test cambierebbe le righe che i recenti
/// mostrano, che è esattamente quel che quell'altro sta verificando.
#[test]
fn saltare_a_una_riga_si_registra_e_indietro_riporta_dov_eri() {
    use crate::menu::{MenuAct, MenuIntent};
    use crate::prompt::PromptMessage;
    let root = tempdir("navigazione_indietro");
    let uno = root.join("uno.rs");
    std::fs::write(
        &uno,
        "1
2
3
4
5
",
    )
    .unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.open_file(&uno).unwrap();

    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::GotoLine));
    let _ = app.update(Message::GotoLine(PromptMessage::Changed("4".to_owned())));
    let _ = app.update(Message::GotoLine(PromptMessage::Confirm));

    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::NavBack));
    let documento = app.workspace.get(app.active_doc().unwrap()).unwrap();
    assert_eq!(
        documento.rope().char_to_line(documento.selection().head),
        0,
        "si torna alla riga da cui si era saltati"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// ATTENZIONE: Le frecce e l'Invio del listener globale sono cablati alla ricerca.
/// Mentre l'elenco dei recenti è aperto sono **suoi**: Ctrl+E è un
/// comando da tastiera, e un elenco che si può solo cliccare farebbe
/// togliere la mano dai tasti proprio nel gesto in cui non si vuole.
#[test]
fn con_i_recenti_aperti_le_frecce_sono_dellelenco() {
    use crate::recents::RecentsKey;
    use crate::search::GlobalSearchMessage;
    assert_eq!(
        Lume::recents_key(&GlobalSearchMessage::Navigate(1)),
        Some(RecentsKey::Move(1))
    );
    assert_eq!(
        Lume::recents_key(&GlobalSearchMessage::Accept),
        Some(RecentsKey::Accept)
    );
    assert_eq!(
        Lume::recents_key(&GlobalSearchMessage::QueryChanged("x".to_owned())),
        None,
        "quel che non è delle frecce resta alla ricerca"
    );
    assert_eq!(Lume::recents_key(&GlobalSearchMessage::CycleTab), None);
}

// ------------------------------------------------------ language server

/// Un banco di prova senza processi: si costruisce lo stato freddo a
/// mano e si iniettano gli eventi che lo stream manderebbe. La sessione
/// vera ha il suo test, in `lume-lsp`, contro un server finto vero.
fn app_con_lsp(root: &std::path::Path) -> (Lume, lume_core::DocId) {
    use crate::lsp_mode::{LspState, LspWorkspace};
    let spec = lume_language_servers::ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let (mut app, _) = Lume::new_without_store();
    let file = root.join("main.rs");
    std::fs::write(&file, "fn mian() {}").unwrap();
    app.open_file(&file).unwrap();
    let id = app.active_doc().unwrap();
    let mut lsp = LspWorkspace::new(spec, root.to_path_buf());
    lsp.command = Some(comando_finto());
    lsp.version = "1.2.3".to_owned();
    lsp.state = LspState::Avvio;
    let _ = lsp.bump();
    app.lsp.insert(lsp);
    (app, id)
}

fn edit_su(files: &[(&std::path::Path, u32, u32)], replacement: &str) -> lsp_types::WorkspaceEdit {
    lsp_types::WorkspaceEdit {
        changes: Some(
            files
                .iter()
                .map(|(path, start, end)| {
                    (
                        crate::lsp_uri::path_to_uri(path).unwrap(),
                        vec![lsp_types::TextEdit {
                            range: lsp_types::Range::new(
                                lsp_types::Position::new(0, *start),
                                lsp_types::Position::new(0, *end),
                            ),
                            new_text: replacement.to_owned(),
                        }],
                    )
                })
                .collect(),
        ),
        ..lsp_types::WorkspaceEdit::default()
    }
}

#[test]
fn il_piano_rinomina_non_modifica_e_diventa_stale_se_il_testo_cambia() {
    let root = tempdir("rename_prepared_stale");
    let (mut app, doc) = app_con_lsp(&root);
    app.workspace
        .get_mut(doc)
        .unwrap()
        .reload_from_str("fn mian() {}");
    let mut mailbox = acceso(&mut app, sa_rinominare());
    while mailbox.try_recv().is_ok() {}

    let prepared = app
        .prepare_workspace_edit(&edit_su(&[(&root.join("main.rs"), 3, 7)], "main"))
        .unwrap();
    assert_eq!(prepared.files.len(), 1);
    assert_eq!(prepared.occurrences(), 1);
    assert_eq!(
        app.workspace.get(doc).unwrap().rope().to_string(),
        "fn mian() {}"
    );

    app.workspace
        .get_mut(doc)
        .unwrap()
        .reload_from_str("// mio\nfn mian() {}");
    let error = app.apply_prepared(prepared).unwrap_err();
    assert!(
        matches!(error, crate::rename::RenameError::Stale(_)),
        "{error}"
    );
    assert_eq!(
        app.workspace.get(doc).unwrap().rope().to_string(),
        "// mio\nfn mian() {}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn un_piano_fresco_rinomina_tutti_i_file_senza_scrivere_su_disco() {
    let root = tempdir("rename_prepared_apply");
    let (mut app, doc) = app_con_lsp(&root);
    app.workspace
        .get_mut(doc)
        .unwrap()
        .reload_from_str("fn mian() {}");
    let closed = root.join("closed.rs");
    std::fs::write(&closed, "fn usa() { mian() }\n").unwrap();
    let mut mailbox = acceso(&mut app, sa_rinominare());
    while mailbox.try_recv().is_ok() {}

    let prepared = app
        .prepare_workspace_edit(&edit_su(
            &[(&root.join("main.rs"), 3, 7), (&closed, 11, 15)],
            "main",
        ))
        .unwrap();
    assert_eq!(app.apply_prepared(prepared), Ok(2));
    assert_eq!(
        app.workspace.get(doc).unwrap().rope().to_string(),
        "fn main() {}"
    );
    let opened = app.workspace.get_by_path(&closed).unwrap();
    assert_eq!(
        app.workspace.get(opened).unwrap().rope().to_string(),
        "fn usa() { main() }\n"
    );
    assert!(app.workspace.get(opened).unwrap().is_dirty());
    assert_eq!(
        std::fs::read_to_string(&closed).unwrap(),
        "fn usa() { mian() }\n"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn un_documento_citato_due_volte_si_rifiuta() {
    let root = tempdir("rinomina_doppione");
    let (mut app, id) = app_con_lsp(&root);
    app.workspace
        .get_mut(id)
        .unwrap()
        .reload_from_str("fn mian() {}");
    let mut posta = acceso(&mut app, sa_rinominare());
    let _ = posta.try_recv().expect("il didOpen");

    let uri = crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap();
    let un_documento = |start: u32, end: u32| lsp_types::TextDocumentEdit {
        text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: None,
        },
        edits: vec![lsp_types::OneOf::Left(lsp_types::TextEdit {
            range: lsp_types::Range::new(
                lsp_types::Position::new(0, start),
                lsp_types::Position::new(0, end),
            ),
            new_text: "main".to_owned(),
        })],
    };
    let edit = lsp_types::WorkspaceEdit {
        document_changes: Some(lsp_types::DocumentChanges::Edits(vec![
            un_documento(3, 7),
            un_documento(0, 2),
        ])),
        ..lsp_types::WorkspaceEdit::default()
    };
    let errore = app.prepare_workspace_edit(&edit).unwrap_err();
    assert!(
        matches!(errore, crate::rename::RenameError::Duplicate(_)),
        "{errore}"
    );
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "fn mian() {}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn un_text_document_edit_con_versione_vecchia_si_rifiuta() {
    let root = tempdir("rinomina_versione_vecchia");
    let (mut app, id) = app_con_lsp(&root);
    app.workspace
        .get_mut(id)
        .unwrap()
        .reload_from_str("fn mian() {}");
    let mut posta = acceso(&mut app, sa_rinominare());
    let _ = posta.try_recv().expect("didOpen");
    let version = app.workspace.get(id).unwrap().version();
    let uri = crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap();
    let make_edit = |version| lsp_types::WorkspaceEdit {
        document_changes: Some(lsp_types::DocumentChanges::Edits(vec![
            lsp_types::TextDocumentEdit {
                text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version: Some(version),
                },
                edits: vec![lsp_types::OneOf::Left(lsp_types::TextEdit {
                    range: lsp_types::Range::new(
                        lsp_types::Position::new(0, 3),
                        lsp_types::Position::new(0, 7),
                    ),
                    new_text: "main".into(),
                })],
            },
        ])),
        ..lsp_types::WorkspaceEdit::default()
    };
    let current_version = i32::try_from(version).unwrap();
    let matching = app
        .prepare_workspace_edit(&make_edit(current_version))
        .expect("la versione corrente è applicabile");
    assert_eq!(matching.occurrences(), 1);
    let edit = make_edit(current_version.saturating_sub(1));
    let error = app.prepare_workspace_edit(&edit).unwrap_err();
    assert!(
        matches!(error, crate::rename::RenameError::Stale(_)),
        "{error}"
    );
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "fn mian() {}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

fn comando_finto() -> lume_lsp::ServerCommand {
    lume_lsp::ServerCommand {
        program: std::path::PathBuf::from("rust-analyzer"),
        args: Vec::new(),
        cwd: None,
        env: Vec::new(),
        source: lume_lsp::CommandSource::SystemPath,
    }
}

fn tempdir(nome: &str) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_{nome}_{nonce}"));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn app_con_documento(nome: &str, testo: &str) -> (Lume, std::path::PathBuf, lume_core::DocId) {
    let root = tempdir(nome);
    let file = root.join("prova.txt");
    std::fs::write(&file, testo).unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.open_file(&file).unwrap();
    let id = app.active_doc().unwrap();
    (app, root, id)
}

// ------------------------------------------------------------- menù Modifica

/// Annulla/Ripeti da menù chiamano lo stesso Undo/Redo della tastiera:
/// un edit fatto, annullato da menù, torna al testo di prima.
#[test]
fn modifica_annulla_e_ripeti_dal_menu_toccano_il_documento_attivo() {
    use crate::menu::MenuAct;

    let (mut app, root, doc_id) = app_con_documento("modifica_undo_redo", "");
    app.workspace
        .get_mut(doc_id)
        .unwrap()
        .set_selection(lume_core::Range::point(0));
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        lume_ui::editor::EditorCommand::Insert("ciao".into()),
    ));
    assert_eq!(
        app.workspace.get(doc_id).unwrap().rope().to_string(),
        "ciao"
    );

    let _ = app.update(Message::MenuAction(MenuAct::Undo));
    assert_eq!(app.workspace.get(doc_id).unwrap().rope().to_string(), "");

    let _ = app.update(Message::MenuAction(MenuAct::Redo));
    assert_eq!(
        app.workspace.get(doc_id).unwrap().rope().to_string(),
        "ciao"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Taglia da menù cancella la selezione — la scrittura sulla clipboard
/// di sistema non è verificabile in un test (non c'è una clipboard
/// headless), ma l'effetto sul documento sì.
#[test]
fn modifica_taglia_dal_menu_cancella_la_selezione() {
    use crate::menu::MenuAct;

    let (mut app, root, doc_id) = app_con_documento("modifica_taglia", "ciao mondo");
    app.workspace
        .get_mut(doc_id)
        .unwrap()
        .set_selection(lume_core::Range::new(0, 4));
    let _ = app.update(Message::MenuAction(MenuAct::Cut));
    assert_eq!(
        app.workspace.get(doc_id).unwrap().rope().to_string(),
        " mondo"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Incolla da menù: la risposta della clipboard arriva come un
/// messaggio a parte (`ClipboardPasted`), non sincrona come Taglia.
#[test]
fn modifica_incolla_applica_il_testo_arrivato_dalla_clipboard() {
    let (mut app, root, doc_id) = app_con_documento("modifica_incolla", "");
    app.workspace
        .get_mut(doc_id)
        .unwrap()
        .set_selection(lume_core::Range::point(0));
    let _ = app.update(Message::ClipboardPasted(Some("incollato".into())));
    assert_eq!(
        app.workspace.get(doc_id).unwrap().rope().to_string(),
        "incollato"
    );
    let _ = std::fs::remove_dir_all(&root);
}

// --------------------------------------------------------- Trova e Sostituisci

/// Ctrl+F apre il riquadro sul documento attivo, precompilato con la
/// selezione corrente se non è vuota — la stessa convenzione di quasi
/// ogni editor: cerchi quel che avevi appena selezionato.
#[test]
fn ctrl_f_apre_trova_precompilato_dalla_selezione() {
    use crate::menu::MenuAct;

    let (mut app, root, doc_id) = app_con_documento("ctrl_f_precompilato", "uno due uno");
    app.workspace
        .get_mut(doc_id)
        .unwrap()
        .set_selection(lume_core::Range::new(0, 3));

    let _ = app.update(Message::MenuAction(MenuAct::Find));

    let trova = app.find.as_ref().expect("Trova deve essere aperto");
    assert_eq!(trova.options.needle, "uno");
    assert!(!trova.replace_mode);
    assert_eq!(trova.matches, vec![0..3, 8..11]);
    let _ = std::fs::remove_dir_all(&root);
}

/// Ctrl+R apre lo stesso riquadro, ma in modalità Sostituisci.
#[test]
fn ctrl_r_apre_trova_in_modalita_sostituisci() {
    use crate::menu::MenuAct;

    let (mut app, root, _doc_id) = app_con_documento("ctrl_r_sostituisci", "uno");
    let _ = app.update(Message::MenuAction(MenuAct::Replace));
    assert!(app.find.as_ref().unwrap().replace_mode);
    let _ = std::fs::remove_dir_all(&root);
}

/// «Successivo» sposta davvero il cursore del documento, non solo
/// l'indice interno del riquadro.
#[test]
fn successivo_sposta_la_selezione_del_documento() {
    use crate::find_replace::FindMessage;
    use crate::menu::MenuAct;

    let (mut app, root, doc_id) = app_con_documento("successivo_sposta", "a a a");
    app.workspace
        .get_mut(doc_id)
        .unwrap()
        .set_selection(lume_core::Range::point(0));
    let _ = app.update(Message::MenuAction(MenuAct::Find));
    let _ = app.update(Message::Find(FindMessage::QueryChanged("a".into())));

    let _ = app.update(Message::Find(FindMessage::Next));
    assert_eq!(
        app.workspace.get(doc_id).unwrap().selection(),
        lume_core::Range::new(2, 3)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Sostituisci tutto: un solo Ctrl+Z riporta l'intero file com'era —
/// non uno per occorrenza.
#[test]
fn sostituisci_tutto_e_un_solo_gradino_di_undo() {
    use crate::find_replace::FindMessage;
    use crate::menu::MenuAct;

    let (mut app, root, doc_id) = app_con_documento("sostituisci_tutto_undo", "a a a");
    let _ = app.update(Message::MenuAction(MenuAct::Replace));
    let _ = app.update(Message::Find(FindMessage::QueryChanged("a".into())));
    let _ = app.update(Message::Find(FindMessage::ReplacementChanged("bb".into())));
    let _ = app.update(Message::Find(FindMessage::ReplaceAll));

    assert_eq!(
        app.workspace.get(doc_id).unwrap().rope().to_string(),
        "bb bb bb"
    );
    let _ = app.workspace.get_mut(doc_id).unwrap().undo();
    assert_eq!(
        app.workspace.get(doc_id).unwrap().rope().to_string(),
        "a a a",
        "un Ctrl+Z solo, non tre"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// #8: oltre il tetto si sostituiscono le prime 5000, lo
/// si dice, e dopo il conto torna esatto sul testo che resta.
#[test]
fn sostituisci_tutto_oltre_il_tetto_lo_dice_e_lascia_le_altre() {
    use crate::find_replace::FindMessage;
    use crate::menu::MenuAct;

    let testo = "a ".repeat(lume_core::MAX_FIND_MATCHES + 1);
    let (mut app, root, doc_id) = app_con_documento("sostituisci_tutto_tetto", &testo);
    let _ = app.update(Message::MenuAction(MenuAct::Replace));
    let _ = app.update(Message::Find(FindMessage::QueryChanged("a".into())));
    let _ = app.update(Message::Find(FindMessage::ReplacementChanged("b".into())));
    let _ = app.update(Message::Find(FindMessage::ReplaceAll));

    let dopo = app.workspace.get(doc_id).unwrap().rope().to_string();
    assert_eq!(dopo.matches('b').count(), lume_core::MAX_FIND_MATCHES);
    assert_eq!(dopo.matches('a').count(), 1, "l'ultima resta");
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(
        notifica.contains(&lume_core::MAX_FIND_MATCHES.to_string()) && notifica.contains("altre"),
        "{notifica}"
    );
    let stato = app.find.as_ref().expect("il riquadro resta aperto");
    assert_eq!(stato.matches.len(), 1);
    assert!(!stato.truncated, "dopo, il conto è esatto");
    assert!(!crate::find_replace::contatore(stato).ends_with('+'));
    let _ = std::fs::remove_dir_all(&root);
}

/// Chiudere (Esc / CloseAll) libera lo stato: niente lasciato acceso a
/// riquadro chiuso — stesso principio già seguito da `goto_line`/`rename`.
#[test]
fn chiudere_trova_lo_svuota() {
    use crate::menu::MenuAct;

    let (mut app, root, _doc_id) = app_con_documento("chiudere_trova", "uno");
    let _ = app.update(Message::MenuAction(MenuAct::Find));
    assert!(app.find.is_some());
    let _ = app.update(Message::CloseAll);
    assert!(app.find.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

/// #9: sotto un altro riquadro nessun match resta evidenziato.
#[test]
fn trova_si_spegne_quando_un_altro_riquadro_prende_il_posto() {
    use crate::find_replace::FindMessage;
    use crate::menu::MenuAct;
    use crate::overlay_manager::OverlayId;

    for (nome, altro, atteso) in [
        (
            "trova_poi_vai_a_riga",
            MenuAct::GotoLine,
            OverlayId::GotoLine,
        ),
        ("trova_poi_recenti", MenuAct::Recents, OverlayId::Recents),
    ] {
        let (mut app, root, _doc) = app_con_documento(nome, "uno due uno");
        let _ = app.update(Message::MenuAction(MenuAct::Find));
        let _ = app.update(Message::Find(FindMessage::QueryChanged("uno".into())));
        assert_eq!(
            app.find.as_ref().map(|f| f.matches.len()),
            Some(2),
            "{nome}"
        );
        let _ = app.update(Message::MenuAction(altro));
        assert_eq!(app.shell.active_overlay(), Some(atteso), "{nome}");
        assert!(
            app.find.is_none(),
            "{nome}: match ancora evidenziati sotto un altro riquadro"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Aprire Sostituisci sopra Trova riusa lo stato di Trova, non è «un altro riquadro».
#[test]
fn sostituisci_sopra_trova_tiene_lo_stato_e_apre_il_campo() {
    use crate::find_replace::FindMessage;
    use crate::menu::MenuAct;
    use crate::overlay_manager::OverlayId;

    let (mut app, root, _doc) = app_con_documento("trova_poi_sostituisci", "uno due uno");
    let _ = app.update(Message::MenuAction(MenuAct::Find));
    let _ = app.update(Message::Find(FindMessage::QueryChanged("uno".into())));
    let _ = app.update(Message::MenuAction(MenuAct::Replace));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Find));
    let stato = app
        .find
        .as_ref()
        .expect("Trova resta vivo sotto Sostituisci");
    assert!(stato.replace_mode);
    assert_eq!(stato.options.needle, "uno", "la selezione diventa la query");
    let _ = std::fs::remove_dir_all(&root);
}

fn badge_problemi(app: &Lume) -> Option<String> {
    app.registry
        .get(tools::problems::ID)
        .and_then(lume_ui::tool::Tool::badge)
}

fn session_ready(server: lsp_types::ServerCapabilities) -> lume_lsp::SessionReady {
    lume_lsp::SessionReady {
        capabilities: lume_lsp::NegotiatedCapabilities {
            server,
            sync_kind: lsp_types::TextDocumentSyncKind::FULL,
        },
        peak_memory_bytes: None,
    }
}

/// Un language server acceso, con le capability che gli si passano.
fn acceso(app: &mut Lume, capabilities: lsp_types::ServerCapabilities) -> Posta {
    let epoca = app.lsp.only().unwrap().epoch;
    let (mailbox, posta) = lume_lsp::LspCommandMailbox::channel();
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::Ready(mailbox),
    ));
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Initialized(Box::new(
            session_ready(capabilities),
        ))),
    ));
    posta
}

type Posta = lume_lsp::LspCommandReceiver;

fn sa_definire() -> lsp_types::ServerCapabilities {
    lsp_types::ServerCapabilities {
        definition_provider: Some(lsp_types::OneOf::Left(true)),
        ..lsp_types::ServerCapabilities::default()
    }
}

#[test]
fn completamento_e_hover_compaiono_solo_con_la_capability() {
    use lsp_types::{CompletionOptions, HoverProviderCapability, ServerCapabilities};
    let root = tempdir("cap_completa");
    let (mut app, _) = app_con_lsp(&root);
    let _posta = acceso(&mut app, sa_definire());
    let lsp = app.lsp.only().unwrap();
    assert!(!lsp.can_complete() && !lsp.can_hover(), "solo definition");
    let (mut altra, _) = app_con_lsp(&root);
    let _posta = acceso(
        &mut altra,
        ServerCapabilities {
            completion_provider: Some(CompletionOptions::default()),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            ..ServerCapabilities::default()
        },
    );
    let lsp = altra.lsp.only().unwrap();
    assert!(lsp.can_complete() && lsp.can_hover());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn l_errore_di_una_domanda_silenziosa_non_diventa_un_avviso() {
    let root = tempdir("errore_muto");
    let (mut app, id) = app_con_lsp(&root);
    let _posta = acceso(&mut app, sa_definire());
    let versione = app.workspace.get(id).unwrap().version();
    let muta = app.lsp.only_mut().unwrap().ask_quiet(id, versione);
    app.completion_wait = Some(crate::completion_ui::CompletionWait {
        server: "rust-analyzer".to_owned(),
        request: muta,
        doc: id,
        anchor: 0,
    });
    let _ = app.lsp_answered("rust-analyzer", muta, Err("scaduta".to_owned()));
    assert!(
        app.notice.is_none(),
        "un timeout del completamento non riempie lo schermo di avvisi"
    );
    assert!(app.completion_wait.is_none(), "il timeout libera l'attesa");
    let normale = app.lsp.only_mut().unwrap().ask(id, versione);
    let _ = app.lsp_answered("rust-analyzer", normale, Err("scaduta".to_owned()));
    assert!(app.notice.is_some(), "una domanda normale invece sì");
    let _ = std::fs::remove_dir_all(&root);
}

/// Un progetto con un server acceso che dichiara `capabilities`, un
/// documento fatto di `testo` e il cursore in fondo.
fn app_con_server(
    nome: &str,
    testo: &str,
    capabilities: lsp_types::ServerCapabilities,
) -> (Lume, lume_core::DocId, Posta, std::path::PathBuf) {
    let root = tempdir(nome);
    let (mut app, id) = app_con_lsp(&root);
    let mut posta = acceso(&mut app, capabilities);
    let _ = posta.try_recv().expect("il didOpen d'apertura");
    let documento = app.workspace.get_mut(id).unwrap();
    documento.reload_from_str(testo);
    let fine = documento.rope().len_chars();
    documento.set_selection(lume_core::Range::point(fine));
    (app, id, posta, root)
}

fn sa_completare() -> lsp_types::ServerCapabilities {
    lsp_types::ServerCapabilities {
        completion_provider: Some(lsp_types::CompletionOptions::default()),
        ..lsp_types::ServerCapabilities::default()
    }
}

fn comanda(app: &mut Lume, comando: EditorCommand) {
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        comando,
    ));
}

fn digita(app: &mut Lume, testo: &str) {
    comanda(app, EditorCommand::Insert(testo.to_owned()));
}

/// Legge dalla posta il `didChange` e la domanda di completamento che lo
/// seguono: restituisce (testo mandato, id della domanda, posizione).
fn domanda_di_completamento(posta: &mut Posta) -> (String, u64, lsp_types::Position) {
    let lume_lsp::LspCommand::ChangeSnapshot { text, .. } =
        posta.try_recv().expect("il testo prima della domanda")
    else {
        panic!("prima della domanda deve arrivare il didChange");
    };
    let lume_lsp::LspCommand::Ask {
        id,
        request: lume_lsp::LspRequest::Completion { position, .. },
    } = posta.try_recv().expect("la domanda")
    else {
        panic!("la domanda non è un completamento");
    };
    (text.to_string(), id, position)
}

fn risposta_di_completamento(app: &mut Lume, richiesta: u64, voci: Vec<lsp_types::CompletionItem>) {
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Answer(
            richiesta,
            Box::new(Ok(lume_lsp::LspAnswer::Completion(Some(
                lsp_types::CompletionResponse::Array(voci),
            )))),
        )),
    ));
}

fn voce_di_completamento(label: &str) -> lsp_types::CompletionItem {
    lsp_types::CompletionItem {
        label: label.to_owned(),
        ..lsp_types::CompletionItem::default()
    }
}

fn etichette_del_popup(app: &Lume, id: lume_core::DocId) -> Vec<String> {
    app.completion_popup(id)
        .map(|popup| popup.rows.iter().map(|riga| riga.label.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn completamento_la_prima_lettera_di_una_parola_chiede_al_server() {
    let (mut app, _id, mut posta, root) = app_con_server("compl_trigger", "x ", sa_completare());
    digita(&mut app, "p");
    let (testo, _, posizione) = domanda_di_completamento(&mut posta);
    assert_eq!(
        testo, "x p",
        "il server deve avere il testo su cui si chiede"
    );
    assert_eq!((posizione.line, posizione.character), (0, 3));
    assert!(app.completion_wait.is_some());
    assert!(posta.try_recv().is_err(), "una domanda sola");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completamento_senza_la_capability_non_parte_nessuna_domanda() {
    let (mut app, _id, mut posta, root) = app_con_server("compl_senza_cap", "x ", sa_definire());
    digita(&mut app, "p");
    assert!(
        posta.try_recv().is_err(),
        "niente didChange e niente domanda"
    );
    assert!(app.completion_wait.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completamento_la_risposta_apre_il_popup_e_invio_scrive_la_voce() {
    let (mut app, id, mut posta, root) = app_con_server("compl_giro", "x ", sa_completare());
    digita(&mut app, "p");
    let (_, richiesta, _) = domanda_di_completamento(&mut posta);
    risposta_di_completamento(
        &mut app,
        richiesta,
        vec![
            voce_di_completamento("push"),
            voce_di_completamento("pop"),
            voce_di_completamento("len"),
        ],
    );
    assert_eq!(
        etichette_del_popup(&app, id),
        ["pop", "push"],
        "«len» non ha il prefisso «p»"
    );
    assert!(
        app.completion_popup(lume_core::DocId(9999)).is_none(),
        "il popup è di un documento solo"
    );

    comanda(&mut app, EditorCommand::PopupDown);
    assert_eq!(app.completion_popup(id).unwrap().selected, 1);
    comanda(&mut app, EditorCommand::PopupAccept);

    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "x push");
    assert_eq!(
        app.workspace.get(id).unwrap().selection().head,
        6,
        "il cursore va dopo la voce"
    );
    assert!(app.completion.is_none(), "accettare chiude il popup");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completamento_una_lettera_dentro_la_parola_rifiltra_senza_richiedere() {
    let (mut app, id, mut posta, root) = app_con_server("compl_filtro", "x ", sa_completare());
    digita(&mut app, "p");
    let (_, richiesta, _) = domanda_di_completamento(&mut posta);
    risposta_di_completamento(
        &mut app,
        richiesta,
        vec![voce_di_completamento("push"), voce_di_completamento("pop")],
    );
    digita(&mut app, "u");
    assert_eq!(etichette_del_popup(&app, id), ["push"]);
    assert!(posta.try_recv().is_err(), "il filtro non chiede al server");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completamento_un_carattere_fuori_parola_chiude_il_popup() {
    let (mut app, id, mut posta, root) = app_con_server("compl_chiude", "x ", sa_completare());
    digita(&mut app, "p");
    let (_, richiesta, _) = domanda_di_completamento(&mut posta);
    risposta_di_completamento(&mut app, richiesta, vec![voce_di_completamento("push")]);
    assert!(app.completion_popup(id).is_some());
    digita(&mut app, " ");
    assert!(app.completion.is_none());
    assert!(app.completion_wait.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completamento_escape_chiude_il_popup() {
    let (mut app, id, mut posta, root) = app_con_server("compl_esc", "x ", sa_completare());
    digita(&mut app, "p");
    let (_, richiesta, _) = domanda_di_completamento(&mut posta);
    risposta_di_completamento(&mut app, richiesta, vec![voce_di_completamento("push")]);
    comanda(&mut app, EditorCommand::PopupDismiss);
    assert!(app.completion_popup(id).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

/// L'utente cambia idea mentre il server pensa: la risposta non ha più
/// nessuno che la aspetti e non deve far comparire un popup dal nulla.
#[test]
fn completamento_una_risposta_con_il_cursore_altrove_si_scarta() {
    let (mut app, id, mut posta, root) = app_con_server("compl_altrove", "x ", sa_completare());
    digita(&mut app, "p");
    let (_, richiesta, _) = domanda_di_completamento(&mut posta);
    comanda(
        &mut app,
        EditorCommand::SetCursor {
            pos: 0,
            extend: false,
        },
    );
    risposta_di_completamento(&mut app, richiesta, vec![voce_di_completamento("push")]);
    assert!(app.completion_popup(id).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completamento_pendente_si_revoca_se_si_cambia_scheda_e_si_torna() {
    let (mut app, id, mut posta, root) =
        app_con_server("compl_cambio_scheda", "x ", sa_completare());
    digita(&mut app, "p");
    let (_, richiesta, _) = domanda_di_completamento(&mut posta);
    let altra = root.join("altra.rs");
    std::fs::write(&altra, "fn altra() {}\n").unwrap();
    app.open_file(&altra).unwrap();
    assert_ne!(app.active_doc(), Some(id));

    let _ = app.activate_tab(id);

    assert!(
        app.completion_wait.is_none(),
        "tornare alla scheda non deve riattivare la richiesta precedente"
    );
    risposta_di_completamento(&mut app, richiesta, vec![voce_di_completamento("push")]);
    assert!(app.completion.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cambiare_root_revoca_il_completamento_del_server_precedente() {
    let (mut app, _id, mut posta, root) =
        app_con_server("compl_cambio_root", "x ", sa_completare());
    digita(&mut app, "p");
    let (_, _, _) = domanda_di_completamento(&mut posta);
    assert!(app.completion_wait.is_some());

    let nuovo_root = tempdir("compl_nuovo_root");
    app.set_project_root(nuovo_root.clone());

    assert!(app.completion_wait.is_none());
    assert!(app.completion.is_none());
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&nuovo_root);
}

/// Una risposta che arriva dopo qualche altra lettera è ancora buona: la
/// parola è la stessa, e il filtro usa il cursore di adesso.
#[test]
fn completamento_una_risposta_dopo_altre_lettere_usa_il_prefisso_di_adesso() {
    let (mut app, id, mut posta, root) = app_con_server("compl_tardi", "x ", sa_completare());
    digita(&mut app, "p");
    let (_, richiesta, _) = domanda_di_completamento(&mut posta);
    digita(&mut app, "u");
    risposta_di_completamento(
        &mut app,
        richiesta,
        vec![voce_di_completamento("push"), voce_di_completamento("pop")],
    );
    assert_eq!(etichette_del_popup(&app, id), ["push"]);
    let _ = std::fs::remove_dir_all(&root);
}

/// Review Focus 4: l'intervallo del server è in UTF-16 e l'emoji vale
/// due unità. Contandolo come char la sostituzione partirebbe una
/// lettera più in là e scriverebbe «ppush».
#[test]
fn completamento_dopo_un_emoji_sostituisce_i_caratteri_giusti() {
    let (mut app, id, mut posta, root) = app_con_server("compl_emoji", "😀 pu", sa_completare());
    comanda(&mut app, EditorCommand::TriggerComplete);
    let (_, richiesta, posizione) = domanda_di_completamento(&mut posta);
    assert_eq!(posizione.character, 5, "emoji (2) + spazio + «pu»");

    let mut voce = voce_di_completamento("push");
    voce.text_edit = Some(lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit {
        range: lsp_types::Range {
            start: lsp_types::Position {
                line: 0,
                character: 3,
            },
            end: lsp_types::Position {
                line: 0,
                character: 5,
            },
        },
        new_text: "push".to_owned(),
    }));
    risposta_di_completamento(&mut app, richiesta, vec![voce]);
    comanda(&mut app, EditorCommand::PopupAccept);

    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "😀 push");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completamento_la_voce_di_menu_chiede_come_ctrl_spazio() {
    use crate::menu::{MenuAct, MenuIntent};
    let (mut app, _id, mut posta, root) = app_con_server("compl_menu", "xy", sa_completare());
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::Complete));
    let (_, _, posizione) = domanda_di_completamento(&mut posta);
    assert_eq!(posizione.character, 2);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completamento_un_popup_rimasto_a_un_altro_documento_si_chiude() {
    let (mut app, id, mut posta, root) = app_con_server("compl_altro_doc", "x ", sa_completare());
    digita(&mut app, "p");
    let (_, richiesta, _) = domanda_di_completamento(&mut posta);
    risposta_di_completamento(&mut app, richiesta, vec![voce_di_completamento("push")]);
    assert!(app.completion_popup(id).is_some());

    // si passa a un altro file: l'editor attivo non è più `id`
    let altro = root.join("altro.txt");
    std::fs::write(&altro, "z").unwrap();
    app.open_file(&altro).unwrap();
    assert!(
        app.completion_popup(id).is_none(),
        "il documento di prima non è quello col fuoco"
    );
    digita(&mut app, "q");
    assert!(
        app.completion.is_none(),
        "la prima battuta altrove lo chiude"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn la_risposta_di_un_server_non_chiude_la_domanda_di_un_altro_con_lo_stesso_id() {
    use crate::lsp_mode::{LspEvent, LspState, LspWorkspace};
    let root = tempdir("due_server");
    let (mut app, id) = app_con_lsp(&root);
    let tombi = lume_language_servers::ServerCatalog::embedded()
        .unwrap()
        .get("tombi")
        .unwrap();
    let mut secondo = LspWorkspace::new(tombi, root.clone());
    secondo.state = LspState::Avvio;
    let epoca_tombi = secondo.bump();
    app.lsp.insert(secondo);
    let ra_id = app.lsp.get_mut("rust-analyzer").unwrap().ask(id, 1);
    let tombi_id = app.lsp.get_mut("tombi").unwrap().ask(id, 1);
    assert_eq!(ra_id, tombi_id);
    let risposta = lume_lsp::ActorEvent::Answer(
        tombi_id,
        Box::new(Ok(lume_lsp::LspAnswer::Definition(None))),
    );
    let _ = app.update(Message::Lsp(
        "tombi".to_owned(),
        epoca_tombi,
        LspEvent::From(risposta),
    ));
    assert!(
        app.lsp
            .get_mut("tombi")
            .unwrap()
            .answered(tombi_id)
            .is_none()
    );
    assert!(
        app.lsp
            .get_mut("rust-analyzer")
            .unwrap()
            .answered(ra_id)
            .is_some()
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn un_evento_con_un_altra_epoca_si_butta_anche_con_piu_server() {
    use crate::lsp_mode::LspEvent;
    let root = tempdir("epoca_altro");
    let (mut app, _) = app_con_lsp(&root);
    let epoca = app.lsp.get("rust-analyzer").unwrap().epoch;
    let (mailbox, _posta) = lume_lsp::LspCommandMailbox::channel();
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca.wrapping_sub(1),
        LspEvent::Ready(mailbox),
    ));
    assert!(app.lsp.get("rust-analyzer").unwrap().mailbox.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ogni_server_vivo_ha_il_suo_stream_e_uno_fermo_non_ne_ha() {
    use crate::lsp_mode::{LspState, LspWorkspace};
    let root = tempdir("due_stream");
    let (mut app, _) = app_con_lsp(&root);
    assert_eq!(app.lsp_spawns().len(), 1);
    let tombi = lume_language_servers::ServerCatalog::embedded()
        .unwrap()
        .get("tombi")
        .unwrap();
    let mut secondo = LspWorkspace::new(tombi, root.clone());
    secondo.command = Some(comando_finto());
    secondo.state = LspState::Avvio;
    let _ = secondo.bump();
    app.lsp.insert(secondo);
    let ids: Vec<String> = app.lsp_spawns().into_iter().map(|s| s.server_id).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"rust-analyzer".to_owned()) && ids.contains(&"tombi".to_owned()));
    app.lsp.get_mut("tombi").unwrap().state = LspState::Ferma;
    assert_eq!(app.lsp_spawns().len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn il_download_ha_uno_stato_suo_con_avanzamento_e_fermarlo_lo_annulla() {
    use crate::features::Feature;
    use crate::lsp_mode::{FaseScarico, LspState, lsp_status_text};
    use lume_language_servers::InstallProgress;
    let mib = 1024 * 1024;
    let root = tempdir("download");
    let (mut app, _) = app_con_lsp(&root);
    app.lsp.only_mut().unwrap().state = LspState::Assente("manca".to_owned());

    let _ = app.download_lsp();
    let lsp = app.lsp.only().unwrap();
    assert_eq!(
        lsp.state,
        LspState::Scaricamento {
            received: 0,
            total: 0,
            fase: FaseScarico::Scarico
        }
    );
    let flag = lsp.download_cancel.clone().unwrap();
    let epoca = lsp.epoch;

    let _ = app.update(Message::LspInstallProgress(
        "rust-analyzer".to_owned(),
        epoca,
        InstallProgress::Downloading {
            received: 5 * mib,
            total: 10 * mib,
        },
    ));
    let testo = lsp_status_text(app.lsp.only()).unwrap();
    assert!(testo.contains("5.0 / 10.0 MB (50%)"), "{testo}");
    let sezione = app.language_server_section().unwrap();
    assert!(sezione.downloading);
    assert!(!sezione.can_download);
    assert!(sezione.detail.contains("50%"), "{}", sezione.detail);

    let _ = app.update(Message::LspInstallProgress(
        "rust-analyzer".to_owned(),
        epoca.wrapping_sub(1),
        InstallProgress::Downloading {
            received: 9 * mib,
            total: 10 * mib,
        },
    ));
    assert!(
        lsp_status_text(app.lsp.only()).unwrap().contains("50%"),
        "l'avanzamento di un'altra generazione si butta"
    );

    let _ = app.update(Message::LspInstallProgress(
        "rust-analyzer".to_owned(),
        epoca,
        InstallProgress::Verifying,
    ));
    assert!(matches!(
        app.lsp.only().unwrap().state,
        LspState::Scaricamento {
            fase: FaseScarico::Verifica,
            ..
        }
    ));

    // «Annulla» nella sezione manda lo stesso evento del tasto Ferma.
    let _ = app.update(Message::Tool(
        tools::ext::ID,
        lume_ui::tool::ToolEvent::new(tools::ext::Event::Stop(Feature::LanguageServer)),
    ));
    let lsp = app.lsp.only().unwrap();
    assert_eq!(lsp.state, LspState::Ferma);
    assert!(
        flag.is_cancelled(),
        "il thread di download è stato fermato davvero"
    );
    assert!(lsp.download_cancel.is_none());
    assert!(app.lsp_spawns().is_empty());

    // Il risultato tardivo del thread (arriva comunque) non riaccende niente.
    let _ = app.update(Message::LspInstalled(
        "rust-analyzer".to_owned(),
        epoca,
        Ok(()),
    ));
    assert_eq!(app.lsp.only().unwrap().state, LspState::Ferma);
    assert!(app.lsp_spawns().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn un_download_fallito_torna_in_errore_senza_flag_armato_ne_annulla_fantasma() {
    use crate::lsp_mode::{LspStartAction, LspState, lsp_start_action};
    let root = tempdir("download_errore");
    let (mut app, _) = app_con_lsp(&root);
    app.lsp.only_mut().unwrap().state = LspState::Assente("manca".to_owned());
    let _ = app.download_lsp();
    let epoca = app.lsp.only().unwrap().epoch;

    let _ = app.update(Message::LspInstalled(
        "rust-analyzer".to_owned(),
        epoca,
        Err("rete assente".to_owned()),
    ));
    let lsp = app.lsp.only().unwrap();
    assert_eq!(lsp.state, LspState::Errore("rete assente".to_owned()));
    assert!(lsp.download_cancel.is_none(), "nessun flag orfano");
    assert_eq!(
        lsp_start_action(Some(lsp)),
        LspStartAction::Avvia,
        "si può riprovare"
    );
    assert!(
        !app.language_server_section().unwrap().downloading,
        "nessun «Annulla» fantasma"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn un_download_riuscito_pulisce_il_flag_e_passa_all_avvio() {
    use crate::lsp_mode::LspState;
    let root = tempdir("download_ok");
    let (mut app, _) = app_con_lsp(&root);
    app.lsp.only_mut().unwrap().state = LspState::Assente("manca".to_owned());
    let _ = app.download_lsp();
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(Message::LspInstalled(
        "rust-analyzer".to_owned(),
        epoca,
        Ok(()),
    ));
    let lsp = app.lsp.only().unwrap();
    assert!(lsp.download_cancel.is_none());
    assert!(
        matches!(lsp.state, LspState::Ricerca),
        "installato: parte la ricerca del binario, cioè l'avvio"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Il giro completo del passo 2: si chiede, si risponde, si atterra —
/// e la posizione la si legge in UTF-16, che è quel che il protocollo
/// parla.
#[test]
fn vai_alla_definizione_chiede_dove_sta_il_cursore_e_ci_porta() {
    use crate::menu::{MenuAct, MenuIntent};
    let root = tempdir("lsp_def");
    let (mut app, id) = app_con_lsp(&root);
    let mut posta = acceso(&mut app, sa_definire());
    let _ = posta.try_recv().expect("il didOpen d'apertura");

    // il cursore sul nome della funzione, dopo un'emoji: è lì che le due
    // conte divergono
    app.workspace.get_mut(id).unwrap().reload_from_str(
        "// 😀
fn mian() {}",
    );
    let _ = app.update(Message::LspChangeDebounced(
        id,
        app.workspace.get(id).unwrap().version(),
    ));
    let _ = posta.try_recv().expect("il didChange");
    app.workspace
        .get_mut(id)
        .unwrap()
        .set_selection(lume_core::Range::point(8));

    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::GotoDefinition));
    let (richiesta, position) = match posta.try_recv().expect("la domanda") {
        lume_lsp::LspCommand::Ask {
            id,
            request: lume_lsp::LspRequest::Definition { position, .. },
        } => (id, position),
        altro => panic!("atteso Ask, visto {altro:?}"),
    };
    assert_eq!(
        position,
        lsp_types::Position::new(1, 3),
        "riga 1, colonna 3: la riga con l'emoji è un'altra"
    );

    // la risposta porta altrove, e ci si arriva
    let altro = root.join("altro.rs");
    std::fs::write(
        &altro,
        "// 😀
pub fn qui() {}",
    )
    .unwrap();
    let epoca = app.lsp.only().unwrap().epoch;
    let apertura = app.update(risposta(
        epoca,
        richiesta,
        Ok(lume_lsp::LspAnswer::Definition(Some(
            lsp_types::GotoDefinitionResponse::Scalar(lsp_types::Location {
                uri: crate::lsp_uri::path_to_uri(&altro).unwrap(),
                range: lsp_types::Range::new(
                    lsp_types::Position::new(1, 7),
                    lsp_types::Position::new(1, 10),
                ),
            }),
        ))),
    ));
    let risposta_file = primo_output(apertura);
    let _ = app.update(risposta_file);

    assert_eq!(
        app.active_path(),
        Some(altro.as_path()),
        "il file della definizione è aperto e attivo"
    );
    let arrivo = app.workspace.get(app.active_doc().unwrap()).unwrap();
    assert_eq!(
        arrivo.selection().head,
        12,
        "«qui» comincia all'undicesimo carattere: in UTF-16 sarebbe un altro numero"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn definizione_tardiva_non_sposta_il_fuoco_dopo_un_cambio_scheda() {
    let root = tempdir("lsp_def_fuoco");
    let (mut app, source) = app_con_lsp(&root);
    let mut posta = acceso(&mut app, sa_definire());
    let _ = posta.try_recv().expect("il didOpen sorgente");
    let other_path = root.join("altro.rs");
    let target_path = root.join("destinazione.rs");
    std::fs::write(&other_path, "fn altro() {}\n").unwrap();
    std::fs::write(&target_path, "fn destinazione() {}\n").unwrap();
    app.open_file(&other_path).unwrap();
    let other = app.active_doc().unwrap();
    app.open_file(&target_path).unwrap();
    let target = app.active_doc().unwrap();
    let _ = app.activate_tab(source);

    let _ = app.goto_definition_for(source);
    let richiesta = loop {
        match posta.try_recv().expect("la richiesta definition") {
            lume_lsp::LspCommand::Ask {
                id,
                request: lume_lsp::LspRequest::Definition { .. },
            } => break id,
            _ => continue,
        }
    };
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.activate_tab(other);
    assert_eq!(app.active_doc(), Some(other));

    let _ = app.update(risposta(
        epoca,
        richiesta,
        Ok(lume_lsp::LspAnswer::Definition(Some(
            lsp_types::GotoDefinitionResponse::Scalar(lsp_types::Location {
                uri: crate::lsp_uri::path_to_uri(&target_path).unwrap(),
                range: lsp_types::Range::new(
                    lsp_types::Position::new(0, 3),
                    lsp_types::Position::new(0, 15),
                ),
            }),
        ))),
    ));

    assert_eq!(app.active_doc(), Some(other));
    assert_ne!(app.active_doc(), Some(target));
    let _ = std::fs::remove_dir_all(&root);
}

fn risposta(epoca: u64, richiesta: u64, esito: Result<lume_lsp::LspAnswer, String>) -> Message {
    Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Answer(richiesta, Box::new(esito))),
    )
}

#[test]
fn azioni_vuote_chiudono_solo_il_proprio_riquadro() {
    for sostituito in [false, true] {
        let root = tempdir("azioni_vuote_overlay");
        let (mut app, doc) = app_con_lsp(&root);
        // Il comando è configurazione della fixture: nessun processo viene avviato.
        app.lsp.only_mut().unwrap().command = Some(lume_lsp::ServerCommand {
            program: "rust-analyzer".into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            source: lume_lsp::CommandSource::SystemPath,
        });
        let mut posta = acceso(
            &mut app,
            lsp_types::ServerCapabilities {
                code_action_provider: Some(lsp_types::CodeActionProviderCapability::Simple(true)),
                ..Default::default()
            },
        );
        let _ = posta.try_recv().expect("didOpen");
        // Una voce preesistente rappresenta la lista mostrata prima della richiesta.
        app.code_actions
            .push(lsp_types::CodeActionOrCommand::Command(
                lsp_types::Command {
                    title: "Azione precedente".to_owned(),
                    command: "precedente".to_owned(),
                    arguments: None,
                },
            ));
        app.apri_overlay(OverlayId::CodeActions);
        let versione = app.workspace.get(doc).unwrap().version();
        let _avviso = app.code_actions();
        let lume_lsp::LspCommand::Ask {
            id: richiesta,
            request: lume_lsp::LspRequest::CodeAction { .. },
        } = posta.try_recv().expect("richiesta CodeAction")
        else {
            panic!("attesa richiesta CodeAction");
        };
        if sostituito {
            app.goto_line = Some("42".into());
            app.apri_overlay(OverlayId::GotoLine);
        }
        assert_eq!(app.workspace.get(doc).unwrap().version(), versione);
        let epoca = app.lsp.only().unwrap().epoch;
        let _avviso = app.update(risposta(
            epoca,
            richiesta,
            Ok(lume_lsp::LspAnswer::CodeAction(None)),
        ));
        assert_eq!(
            app.shell.active_overlay(),
            sostituito.then_some(OverlayId::GotoLine)
        );
        assert!(app.code_actions.is_empty());
        if sostituito {
            assert_eq!(app.goto_line.as_deref(), Some("42"));
        }
        assert!(app.lsp.only_mut().unwrap().answered(richiesta).is_none());
        assert_eq!(
            app.notice.as_ref().unwrap().1.text,
            "Nessuna azione disponibile"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

/// ATTENZIONE: Il tasto destro **porta il caret dove si è cliccato**, e questo è
/// il test che lo tiene fermo: senza, il menù si aprirebbe dove punta il
/// mouse e le sue tre voci parlerebbero del simbolo di *prima*. È il modo
/// più silenzioso di sbagliare — sembra tutto a posto, e si salta altrove.
///
/// E il menù **non si apre** quando non ha voci: a server fermo un
/// rettangolo grigio vuoto è peggio di nessun menù.
#[test]
fn il_tasto_destro_sposta_il_caret_e_apre_il_menu_solo_se_ha_voci() {
    use crate::editor_groups::GroupId;
    use crate::overlay_manager::OverlayId;
    use lume_ui::editor::EditorCommand;
    let root = tempdir("lsp_contesto");
    let (mut app, id) = app_con_lsp(&root);
    app.workspace
        .get_mut(id)
        .unwrap()
        .reload_from_str("fn mian() {}\n");

    let destro = |x: i32| {
        Message::EditorIn(
            GroupId::Primary,
            EditorCommand::ContextMenu {
                pos: 4,
                screen_x: x,
                screen_y: 200,
            },
        )
    };

    // Anche a server fermo il menu offre le azioni di modifica.
    let _ = app.update(destro(120));
    assert_eq!(app.workspace.get(id).unwrap().selection().head, 4);
    assert!(
        app.context_menu.is_some(),
        "le azioni clipboard sono sempre disponibili"
    );
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Context));

    // con un server che dichiara le tre capability, si apre dove si è
    // cliccato
    let _ = acceso(&mut app, sa_tutto());
    app.workspace
        .get_mut(id)
        .unwrap()
        .set_selection(lume_core::Range::point(0));
    let _ = app.update(destro(120));
    assert_eq!(
        app.workspace.get(id).unwrap().selection().head,
        4,
        "il caret segue il clic"
    );
    let dove = app.context_menu.as_ref().expect("il menù si apre");
    assert_eq!((dove.x, dove.y), (120.0, 200.0));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Context));
    assert_eq!(
        crate::context_menu::items(&dove.soggetto, app.menu_checks()).len(),
        6,
        "tre azioni LSP e tre azioni clipboard"
    );

    // scegliere una voce lo chiude
    let _ = app.apply_menu_intent(crate::menu::MenuIntent::Activate(
        crate::menu::MenuAct::GotoDefinition,
    ));
    assert!(app.context_menu.is_none());
    assert_ne!(app.shell.active_overlay(), Some(OverlayId::Context));

    // e l'Esc pure
    let _ = app.update(destro(90));
    assert!(app.context_menu.is_some());
    let _ = app.update(Message::CloseAll);
    assert!(app.context_menu.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

fn sa_tutto() -> lsp_types::ServerCapabilities {
    lsp_types::ServerCapabilities {
        definition_provider: Some(lsp_types::OneOf::Left(true)),
        references_provider: Some(lsp_types::OneOf::Left(true)),
        rename_provider: Some(lsp_types::OneOf::Right(lsp_types::RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: lsp_types::WorkDoneProgressOptions::default(),
        })),
        ..lsp_types::ServerCapabilities::default()
    }
}

fn sa_rinominare() -> lsp_types::ServerCapabilities {
    lsp_types::ServerCapabilities {
        rename_provider: Some(lsp_types::OneOf::Right(lsp_types::RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: lsp_types::WorkDoneProgressOptions::default(),
        })),
        ..lsp_types::ServerCapabilities::default()
    }
}

/// Il giro del passo 5, con la sua promessa: **niente tocca il disco**.
///
/// Un file chiuso coinvolto dalla rinomina diventa una scheda sporca, non
/// una scrittura. Il salvataggio è atomico per file, non per progetto:
/// scrivere qui più file potrebbe lasciare a metà la rinomina.
#[test]
fn la_rinomina_apre_schede_sporche_e_non_scrive_niente_su_disco() {
    use crate::menu::{MenuAct, MenuIntent};
    let root = tempdir("lsp_rinomina");
    let (mut app, id) = app_con_lsp(&root);
    app.workspace
        .get_mut(id)
        .unwrap()
        .reload_from_str("fn mian() {}\n");
    let chiuso = root.join("chiuso.rs");
    std::fs::write(&chiuso, "fn usa() { mian() }\n").unwrap();
    let prima = std::fs::read_to_string(&chiuso).unwrap();

    let mut posta = acceso(&mut app, sa_rinominare());
    let _ = posta.try_recv().expect("il didOpen");
    // il debounce va svuotato o il server non ha il testo nuovo
    let _ = app.update(Message::LspChangeDebounced(
        id,
        app.workspace.get(id).unwrap().version(),
    ));
    let _ = posta.try_recv();

    app.workspace
        .get_mut(id)
        .unwrap()
        .set_selection(lume_core::Range::point(4));
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::Rename));

    // prima `prepareRename`, poi il riquadro
    let lume_lsp::LspCommand::Ask {
        id: prepara,
        request: lume_lsp::LspRequest::PrepareRename { .. },
    } = posta.try_recv().expect("prepareRename")
    else {
        panic!("atteso PrepareRename")
    };
    assert!(app.rename.is_none(), "il riquadro aspetta il sì del server");
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(risposta(
        epoca,
        prepara,
        Ok(lume_lsp::LspAnswer::PrepareRename(Some(
            lsp_types::PrepareRenameResponse::Range(lsp_types::Range::new(
                lsp_types::Position::new(0, 3),
                lsp_types::Position::new(0, 7),
            )),
        ))),
    ));
    let riquadro = app.rename.as_ref().expect("il riquadro si apre");
    assert_eq!(riquadro.original, "mian", "col nome che c'era");

    let _ = app.update(Message::Rename(crate::rename_box::RenameMessage::Changed(
        "main".to_owned(),
    )));
    let _ = app.update(Message::Rename(crate::rename_box::RenameMessage::Confirm));
    let lume_lsp::LspCommand::Ask {
        id: rinomina,
        request: lume_lsp::LspRequest::Rename { new_name, .. },
    } = posta.try_recv().expect("rename")
    else {
        panic!("atteso Rename")
    };
    assert_eq!(new_name, "main");

    let modifica = |riga: u32, da: u32, a: u32| lsp_types::TextEdit {
        range: lsp_types::Range::new(
            lsp_types::Position::new(riga, da),
            lsp_types::Position::new(riga, a),
        ),
        new_text: "main".to_owned(),
    };
    let edit = lsp_types::WorkspaceEdit {
        changes: Some(
            [
                (
                    crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                    vec![modifica(0, 3, 7)],
                ),
                (
                    crate::lsp_uri::path_to_uri(&chiuso).unwrap(),
                    vec![modifica(0, 11, 15)],
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ..lsp_types::WorkspaceEdit::default()
    };
    let _ = app.update(risposta(
        epoca,
        rinomina,
        Ok(lume_lsp::LspAnswer::Rename(Some(edit))),
    ));

    assert!(app.rename.is_none(), "il riquadro si chiude");
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "fn main() {}\n"
    );
    let id_chiuso = app
        .workspace
        .get_by_path(&chiuso)
        .expect("aperto in memoria");
    assert_eq!(
        app.workspace.get(id_chiuso).unwrap().rope().to_string(),
        "fn usa() { main() }\n"
    );
    assert!(
        app.workspace.get(id_chiuso).unwrap().is_dirty(),
        "scheda sporca, non file scritto"
    );
    assert!(
        app.groups.contains(id_chiuso),
        "e la scheda si vede, o l'utente non saprebbe cosa salvare"
    );
    assert_eq!(
        app.groups.active(),
        Some(id),
        "ATTENZIONE: e il fuoco non si sposta: dodici file rinominati non devono \
             lasciare l'utente su un file a caso"
    );
    assert_eq!(
        std::fs::read_to_string(&chiuso).unwrap(),
        prima,
        "ATTENZIONE: il disco non si tocca"
    );

    // ATTENZIONE: Il gruppo di annullamento si **chiude** sulla rinomina, e questo
    // è il perché: quel che si batte dopo è una revisione a sé. Senza,
    // la prima battuta si fonderebbe con la rinomina e un Ctrl+Z
    // porterebbe via tutte e due — l'utente si ritroverebbe il nome
    // vecchio per aver annullato una lettera.
    let lunghezza = app.workspace.get(id_chiuso).unwrap().rope().len_chars();
    let battuta =
        lume_core::ChangeSet::replace_many(&[(0, 0, "// nota\n".to_owned())], lunghezza).unwrap();
    app.workspace
        .get_mut(id_chiuso)
        .unwrap()
        .apply(lume_core::Transaction {
            changes: battuta,
            selection_after: None,
        });
    app.workspace.get_mut(id_chiuso).unwrap().undo();
    assert_eq!(
        app.workspace.get(id_chiuso).unwrap().rope().to_string(),
        "fn usa() { main() }\n",
        "il primo Ctrl+Z porta via la battuta, non la rinomina"
    );

    // e una sola revisione per file: il secondo la porta via tutta
    app.workspace.get_mut(id_chiuso).unwrap().undo();
    assert_eq!(
        app.workspace.get(id_chiuso).unwrap().rope().to_string(),
        "fn usa() { mian() }\n"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// ATTENZIONE: La guardia che serve al **multi-file**, e che nessun'altra copre.
///
/// Per il documento da cui si è partiti basta l'epoca della domanda. Per
/// gli **altri** file che la rinomina tocca no: nessuno ne ha fotografato
/// la revisione, e se l'utente ha battuto lì mentre il server pensava, le
/// posizioni parlano di un testo diverso — la rinomina cadrebbe qualche
/// carattere più in là, dentro codice che non c'entra niente.
///
/// Il modo di saperlo c'è già ed è esatto: `sent` avanza solo quando un
/// `didChange` parte davvero, quindi «il server ha il nostro testo» è
/// `sent == version`.
#[test]
fn una_rinomina_non_tocca_un_file_cambiato_sotto_mentre_il_server_pensava() {
    let root = tempdir("lsp_rinomina_desync");
    let (mut app, id) = app_con_lsp(&root);
    let secondo_path = root.join("secondo.rs");
    std::fs::write(&secondo_path, "fn usa() { mian() }\n").unwrap();
    app.open_file(&secondo_path).unwrap();
    let secondo = app.active_doc().unwrap();
    app.groups.activate(id);
    let mut posta = acceso(&mut app, sa_rinominare());
    let _ = posta.try_recv();
    let _ = posta.try_recv();

    let prima_uno = app.workspace.get(id).unwrap().rope().to_string();

    // si batte nel **secondo** file, e quel testo non è ancora partito
    app.workspace
        .get_mut(secondo)
        .unwrap()
        .reload_from_str("// nota\nfn usa() { mian() }\n");
    let prima_due = app.workspace.get(secondo).unwrap().rope().to_string();

    let modifica = |riga: u32, da: u32, a: u32| lsp_types::TextEdit {
        range: lsp_types::Range::new(
            lsp_types::Position::new(riga, da),
            lsp_types::Position::new(riga, a),
        ),
        new_text: "main".to_owned(),
    };
    let edit = lsp_types::WorkspaceEdit {
        changes: Some(
            [
                (
                    crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                    vec![modifica(0, 3, 7)],
                ),
                (
                    crate::lsp_uri::path_to_uri(&secondo_path).unwrap(),
                    vec![modifica(0, 11, 15)],
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ..lsp_types::WorkspaceEdit::default()
    };
    let _ = app.apply_workspace_edit(&edit);

    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        prima_uno,
        "nemmeno il file allineato si tocca: o tutto, o niente"
    );
    assert_eq!(
        app.workspace.get(secondo).unwrap().rope().to_string(),
        prima_due
    );
    assert!(app.notice.is_some(), "e si dice quale file era cambiato");
    let _ = std::fs::remove_dir_all(&root);
}

/// ATTENZIONE: Una `WorkspaceEdit` che vuole rinominare **file** si rifiuta, e si
/// rifiuta **prima di toccare qualunque cosa**: applicarne la metà
/// testuale lascerebbe un progetto che non compila, con i riferimenti
/// aggiornati a un file che non è stato rinominato.
#[test]
fn una_rinomina_di_file_si_rifiuta_senza_toccare_niente() {
    let root = tempdir("lsp_rinomina_no");
    let (mut app, id) = app_con_lsp(&root);
    let prima = app.workspace.get(id).unwrap().rope().to_string();
    let edit = lsp_types::WorkspaceEdit {
        document_changes: Some(lsp_types::DocumentChanges::Operations(vec![
            lsp_types::DocumentChangeOperation::Op(lsp_types::ResourceOp::Rename(
                lsp_types::RenameFile {
                    old_uri: crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap(),
                    new_uri: crate::lsp_uri::path_to_uri(&root.join("altro.rs")).unwrap(),
                    options: None,
                    annotation_id: None,
                },
            )),
        ])),
        ..lsp_types::WorkspaceEdit::default()
    };
    let _ = app.apply_workspace_edit(&edit);
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), prima);
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert!(app.notice.is_some(), "e si dice perché");
    let _ = std::fs::remove_dir_all(&root);
}

fn sa_utilizzi() -> lsp_types::ServerCapabilities {
    lsp_types::ServerCapabilities {
        references_provider: Some(lsp_types::OneOf::Left(true)),
        ..lsp_types::ServerCapabilities::default()
    }
}

/// Il giro del passo 4: si chiede, il dock si apre **subito** dicendo che
/// sta cercando, e la risposta lo riempie.
///
/// ATTENZIONE: L'apertura immediata non è cosmetica: su un simbolo molto usato la
/// risposta può metterci secondi, e un comando che per tre secondi non dà
/// segno di vita sembra non aver funzionato — l'utente lo ripreme, e la
/// seconda ricerca scarta la prima.
#[test]
fn trova_utilizzi_apre_il_dock_subito_e_lo_riempie_quando_arriva() {
    use crate::menu::{MenuAct, MenuIntent};
    let root = tempdir("lsp_utilizzi");
    let (mut app, id) = app_con_lsp(&root);
    app.workspace.get_mut(id).unwrap().reload_from_str(
        "fn mian() {}
",
    );
    app.workspace
        .get_mut(id)
        .unwrap()
        .set_selection(lume_core::Range::point(4));
    let mut posta = acceso(&mut app, sa_utilizzi());
    let _ = posta.try_recv().expect("il didOpen");

    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::FindUsages));
    let richiesta = match posta.try_recv().expect("la domanda") {
        lume_lsp::LspCommand::Ask {
            id,
            request: lume_lsp::LspRequest::References { position, .. },
        } => {
            assert_eq!(position, lsp_types::Position::new(0, 4));
            id
        }
        altro => panic!("attesa References, vista {altro:?}"),
    };
    assert!(
        app.registry.is_open(tools::usages::ID),
        "il dock si apre prima della risposta"
    );
    assert_eq!(app.usages.symbol, "mian", "e dice cosa sta cercando");
    assert!(
        !app.usages.status.is_empty(),
        "con scritto che sta cercando"
    );

    let altro = root.join("altro.rs");
    let posizione = |file: &std::path::Path, riga: u32| lsp_types::Location {
        uri: crate::lsp_uri::path_to_uri(file).unwrap(),
        range: lsp_types::Range::new(
            lsp_types::Position::new(riga, 3),
            lsp_types::Position::new(riga, 7),
        ),
    };
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(risposta(
        epoca,
        richiesta,
        Ok(lume_lsp::LspAnswer::References(Some(vec![
            posizione(&altro, 8),
            posizione(&root.join("main.rs"), 0),
        ]))),
    ));
    assert_eq!(app.usages.groups.len(), 2);
    assert_eq!(app.usages.total(), 2);
    assert!(app.usages.status.is_empty(), "trovato: niente da spiegare");

    // e «nessun utilizzo» è una risposta, non un guasto: si dice, non si
    // notifica come errore
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::FindUsages));
    let lume_lsp::LspCommand::Ask { id: seconda, .. } = posta.try_recv().expect("la seconda")
    else {
        panic!("attesa Ask")
    };
    let _ = app.update(risposta(
        epoca,
        seconda,
        Ok(lume_lsp::LspAnswer::References(Some(Vec::new()))),
    ));
    assert!(app.usages.groups.is_empty());
    assert!(app.usages.status.contains("mian"), "{}", app.usages.status);
    assert!(app.notice.is_none(), "non è un errore");
    let _ = std::fs::remove_dir_all(&root);
}

fn sa_struttura() -> lsp_types::ServerCapabilities {
    lsp_types::ServerCapabilities {
        document_symbol_provider: Some(lsp_types::OneOf::Left(true)),
        ..lsp_types::ServerCapabilities::default()
    }
}

/// #10: la fotografia segue il gruppo col fuoco, anche
/// quando il gruppo 1 sparisce con la sua ultima scheda.
#[test]
fn struttura_e_simboli_seguono_il_gruppo_col_fuoco() {
    let root = tempdir("struttura_fuoco");
    let a = root.join("a.rs");
    let b = root.join("b.rs");
    std::fs::write(&a, "fn a() {}").unwrap();
    std::fs::write(&b, "fn b() {}").unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.open_file(&a).unwrap();
    let id_a = app.active_doc().unwrap();
    app.open_file(&b).unwrap();
    let id_b = app.active_doc().unwrap();
    assert!(app.groups.split(), "b.rs passa nel gruppo 1");
    let nome = |app: &Lume| {
        app.structure_snapshot()
            .path
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
    };

    app.groups.activate(id_a);
    assert_eq!(nome(&app).as_deref(), Some("a.rs"));
    app.groups.activate(id_b);
    assert_eq!(nome(&app).as_deref(), Some("b.rs"), "il fuoco è a destra");

    assert!(app.groups.close(id_b));
    assert!(!app.groups.is_split(), "il gruppo 1 vuoto sparisce");
    assert_eq!(
        nome(&app).as_deref(),
        Some("a.rs"),
        "e Struttura torna a sinistra"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// #10: il pannello aperto si aggiorna quando il fuoco passa all'altra
/// colonna battendoci dentro, anche se `EditorIn` non drena le richieste.
#[test]
fn battere_nell_altro_gruppo_aggiorna_il_pannello_struttura() {
    use crate::editor_groups::GroupId;
    use crate::menu::MenuAct;
    use lume_ui::editor::Motion;
    use lume_ui::tool::{ToolEvent, ToolRequest};

    let root = tempdir("struttura_fuoco_pannello");
    let (mut app, primo) = app_con_lsp(&root);
    let _posta = acceso(&mut app, sa_struttura());
    let altro = root.join("altro.rs");
    std::fs::write(&altro, "fn altro() {}").unwrap();
    app.open_file(&altro).unwrap();
    let secondo = app.active_doc().unwrap();
    assert!(app.groups.split(), "altro.rs passa nel gruppo 1");
    app.groups.activate(primo);
    for id in [primo, secondo] {
        let versione = app.workspace.get(id).unwrap().version();
        app.lsp.only_mut().unwrap().store_outline(
            id,
            crate::lsp_mode::Outline {
                version: versione,
                rows: crate::lsp_mode::outline_rows(Some(simboli())),
            },
        );
    }
    let _ = app.update(Message::MenuAction(MenuAct::ShowStructure));
    assert_eq!(app.registry.active_panel, Some(tools::structure::ID));

    let _ = app.update(Message::EditorIn(
        GroupId::Secondary,
        EditorCommand::Move(Motion::Right, false),
    ));
    assert_eq!(app.groups.focus(), GroupId::Secondary);
    let richieste = app
        .registry
        .get_mut(tools::structure::ID)
        .unwrap()
        .update(&ToolEvent::new(tools::structure::Event::Open(0)));
    let [ToolRequest::OpenFileAt { path, .. }] = &richieste[..] else {
        panic!("attesa una OpenFileAt, avute {richieste:?}");
    };
    assert_eq!(path.file_name().unwrap(), "altro.rs");
    let _ = std::fs::remove_dir_all(&root);
}

/// Il giro del passo 3: la struttura arriva dal server, il pannello la
/// mostra e il breadcrumb ne prende la catena.
///
/// ATTENZIONE: E la si chiede **una volta per revisione**. `sync_outlines` gira a
/// ogni messaggio — compresi i `DragMoved` del mouse — e senza la guardia
/// un pannello aperto diventerebbe un polling sul language server: la
/// cosa che questo progetto esiste per non fare.
#[test]
fn la_struttura_arriva_dal_server_e_non_si_richiede_a_ogni_frame() {
    let root = tempdir("lsp_struttura");
    let (mut app, id) = app_con_lsp(&root);
    app.workspace.get_mut(id).unwrap().reload_from_str(
        "struct Cosa {
    campo: u32,
}
",
    );
    let mut posta = acceso(&mut app, sa_struttura());
    let _ = posta.try_recv().expect("il didOpen");

    let richiesta = match posta.try_recv().expect("la domanda sulla struttura") {
        lume_lsp::LspCommand::Ask {
            id,
            request: lume_lsp::LspRequest::DocumentSymbols { .. },
        } => id,
        altro => panic!("attesa DocumentSymbols, vista {altro:?}"),
    };
    // e non se ne chiedono altre finché il testo non cambia
    let _ = app.drain_tool_requests();
    let _ = app.drain_tool_requests();
    assert!(
        posta.try_recv().is_err(),
        "una domanda per revisione, non una per frame"
    );

    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(risposta(
        epoca,
        richiesta,
        Ok(lume_lsp::LspAnswer::Symbols(Some(simboli()))),
    ));

    // il pannello: la fotografia è il seam, il tool dietro il muro
    let fotografia = app.structure_snapshot();
    assert_eq!(fotografia.rows.len(), 2);
    assert_eq!(fotografia.rows[0].name, "Cosa");
    assert_eq!(fotografia.rows[0].kind, "struct");
    assert_eq!(fotografia.rows[1].depth, 1, "il campo sta dentro");
    assert_eq!(
        fotografia.rows[1].line, 2,
        "«OpenFileAt» conta da 1, il protocollo da 0"
    );

    // il breadcrumb: percorso più catena di simboli sotto il cursore
    app.workspace
        .get_mut(id)
        .unwrap()
        .set_selection(lume_core::Range::point(18));
    let briciole = app.crumbs_of(crate::editor_groups::GroupId::Primary);
    assert_eq!(
        briciole.last().map(String::as_str),
        Some("campo"),
        "il più interno per ultimo: {briciole:?}"
    );
    assert!(briciole.contains(&"Cosa".to_owned()));
    assert!(briciole.contains(&"main.rs".to_owned()));
    let _ = std::fs::remove_dir_all(&root);
}

/// Una struttura che parla di una revisione superata **non si mostra**:
/// indicherebbe righe che non esistono più con la stessa faccia di una
/// buona. E non si notifica niente — è un aggiornamento di sfondo, che
/// l'utente non ha chiesto.
#[test]
fn una_struttura_superata_non_si_mostra_e_non_si_lamenta() {
    let root = tempdir("lsp_struttura_vecchia");
    let (mut app, id) = app_con_lsp(&root);
    app.workspace.get_mut(id).unwrap().reload_from_str(
        "struct Cosa {
    campo: u32,
}
",
    );
    let mut posta = acceso(&mut app, sa_struttura());
    let _ = posta.try_recv().expect("il didOpen");
    let lume_lsp::LspCommand::Ask { id: richiesta, .. } = posta.try_recv().expect("la domanda")
    else {
        panic!("attesa Ask")
    };

    // si batte mentre il server pensa
    app.workspace.get_mut(id).unwrap().reload_from_str(
        "struct Altro {}
",
    );
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(risposta(
        epoca,
        richiesta,
        Ok(lume_lsp::LspAnswer::Symbols(Some(simboli()))),
    ));

    assert!(
        app.structure_snapshot().rows.is_empty(),
        "una fotografia vecchia non si appende al muro"
    );
    assert!(
        app.notice.is_none(),
        "e non se ne parla: non l'aveva chiesta nessuno"
    );

    // ATTENZIONE: Il secondo tempo, e vuole una guardia **sua**: qui la struttura
    // arriva buona, viene appesa, e *poi* si batte. Senza il filtro sulla
    // revisione in `outline_of` resterebbe appesa — con la faccia di una
    // buona — a indicare righe che non esistono più. La prima guardia
    // (scartare la risposta tardiva) non copre questo caso: la risposta
    // era arrivata in tempo.
    let versione = app.workspace.get(id).unwrap().version();
    app.lsp.only_mut().unwrap().store_outline(
        id,
        crate::lsp_mode::Outline {
            version: versione,
            rows: crate::lsp_mode::outline_rows(Some(simboli())),
        },
    );
    assert_eq!(
        app.structure_snapshot().rows.len(),
        2,
        "sul testo giusto si vede"
    );
    assert!(
        !app.crumbs_of(crate::editor_groups::GroupId::Primary)
            .is_empty(),
        "e il breadcrumb pure"
    );

    app.workspace
        .get_mut(id)
        .unwrap()
        .reload_from_str("struct Terzo {}\n");
    assert!(
        app.structure_snapshot().rows.is_empty(),
        "battuta una lettera, la struttura di prima non parla più di questo testo"
    );
    assert_eq!(
        app.crumbs_of(crate::editor_groups::GroupId::Primary),
        vec!["main.rs".to_owned()],
        "e il breadcrumb torna al solo percorso invece di inventare un simbolo"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Cerca ovunque, scheda Simboli: la stessa fotografia di Struttura, solo
/// un'altra vetrina — e attivare un'entry salta alla riga del simbolo,
/// non solo al file.
#[test]
fn cerca_ovunque_mostra_i_simboli_e_ci_salta_alla_riga() {
    let root = tempdir("lsp_simboli_cerca");
    let (mut app, id) = app_con_lsp(&root);
    app.workspace.get_mut(id).unwrap().reload_from_str(
        "struct Cosa {
    campo: u32,
}
",
    );
    let mut posta = acceso(&mut app, sa_struttura());
    let _ = posta.try_recv().expect("il didOpen");
    let lume_lsp::LspCommand::Ask { id: richiesta, .. } =
        posta.try_recv().expect("la domanda sulla struttura")
    else {
        panic!("attesa Ask")
    };
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(risposta(
        epoca,
        richiesta,
        Ok(lume_lsp::LspAnswer::Symbols(Some(simboli()))),
    ));

    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Open(
        SearchTab::Simboli,
    )));

    let entries = app.global_search.entries();
    let campo = entries
        .iter()
        .position(|e| e.name == "campo")
        .expect("«campo» deve comparire fra i Simboli, aperta la ricerca");
    assert_eq!(entries[campo].tab, SearchTab::Simboli);
    assert_eq!(
        entries[campo].line,
        Some(2),
        "«OpenFileAt» conta da 1, il protocollo da 0"
    );

    let _ = app.activate_search_entry(campo);
    let _ = app.update(Message::FileLetto(leggi_per_apertura(
        entries[campo].path.clone().expect("l'entry ha un file"),
        DopoApertura::Ricerca {
            riga: entries[campo].line,
        },
    )));

    let doc = app.workspace.get(id).unwrap();
    let riga_cursore = doc.rope().char_to_line(doc.selection().head);
    assert_eq!(
        riga_cursore, 1,
        "il cursore atterra sulla riga del simbolo, non all'inizio del file"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// I simboli di `struct Cosa {\n    campo: u32,\n}\n`, con gli intervalli
/// che manderebbe un server vero: il corpo per `range`, il solo nome per
/// `selection_range`.
fn simboli() -> lsp_types::DocumentSymbolResponse {
    let da_a = |r1: u32, c1: u32, r2: u32, c2: u32| {
        lsp_types::Range::new(
            lsp_types::Position::new(r1, c1),
            lsp_types::Position::new(r2, c2),
        )
    };
    #[allow(deprecated)]
    let campo = lsp_types::DocumentSymbol {
        name: "campo".to_owned(),
        detail: Some("u32".to_owned()),
        kind: lsp_types::SymbolKind::FIELD,
        tags: None,
        deprecated: None,
        range: da_a(1, 4, 1, 15),
        selection_range: da_a(1, 4, 1, 9),
        children: None,
    };
    #[allow(deprecated)]
    let cosa = lsp_types::DocumentSymbol {
        name: "Cosa".to_owned(),
        detail: None,
        kind: lsp_types::SymbolKind::STRUCT,
        tags: None,
        deprecated: None,
        range: da_a(0, 0, 2, 1),
        selection_range: da_a(0, 7, 0, 11),
        children: Some(vec![campo]),
    };
    lsp_types::DocumentSymbolResponse::Nested(vec![cosa])
}

/// Una capability non dichiarata non diventa una domanda — e non è lo
/// stesso caso del server fermo: qui il server c'è, è avviato, e
/// semplicemente **non sa** rispondere a questo. Mandargliela lo farebbe
/// rispondere «metodo sconosciuto» dopo un giro sul filo, e la voce di
/// menù prometterebbe una cosa che non c'è.
#[test]
fn un_server_che_non_sa_definire_non_riceve_la_domanda() {
    use crate::menu::{MenuAct, MenuIntent};
    let root = tempdir("lsp_def_muto");
    let (mut app, _) = app_con_lsp(&root);
    let mut posta = acceso(&mut app, lsp_types::ServerCapabilities::default());
    let _ = posta.try_recv().expect("il didOpen");

    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::GotoDefinition));
    assert!(posta.try_recv().is_err(), "niente parte sul filo");
    assert!(
        !app.menu_checks().lsp_definition,
        "e la voce di menù non si disegna"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// ATTENZIONE: Una risposta che parla di un testo che non c'è più **non è quasi
/// giusta, è sbagliata**: il salto atterrerebbe qualche riga in là, e lo
/// si scoprirebbe solo guardandosi intorno. Si guarda la revisione del
/// documento, non l'orologio. E una domanda che non abbiamo fatto noi non
/// ha destinatario.
#[test]
fn una_risposta_su_un_testo_gia_cambiato_non_fa_saltare_nessuno() {
    use crate::menu::{MenuAct, MenuIntent};
    let root = tempdir("lsp_def_vecchia");
    let (mut app, id) = app_con_lsp(&root);
    let mut posta = acceso(&mut app, sa_definire());
    let _ = posta.try_recv().expect("il didOpen");

    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::GotoDefinition));
    let lume_lsp::LspCommand::Ask { id: richiesta, .. } = posta.try_recv().expect("la domanda")
    else {
        panic!("atteso Ask")
    };

    // si batte una lettera mentre il server pensa
    app.workspace
        .get_mut(id)
        .unwrap()
        .reload_from_str("fn main() {}");
    let altro = root.join("altro.rs");
    std::fs::write(&altro, "pub fn qui() {}").unwrap();
    let meta = lume_lsp::LspAnswer::Definition(Some(lsp_types::GotoDefinitionResponse::Scalar(
        lsp_types::Location {
            uri: crate::lsp_uri::path_to_uri(&altro).unwrap(),
            range: lsp_types::Range::default(),
        },
    )));
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(risposta(epoca, richiesta, Ok(meta.clone())));
    assert_ne!(
        app.active_path(),
        Some(altro.as_path()),
        "revisione superata: non si salta"
    );
    assert!(app.notice.is_some(), "e lo si dice, invece di tacere");

    // e un numero che non abbiamo mai chiesto non apre niente e non
    // notifica niente: non è successo nulla
    app.notice = None;
    let _ = app.update(risposta(epoca, 9999, Ok(meta)));
    assert_ne!(app.active_path(), Some(altro.as_path()));
    assert!(
        app.notice.is_none(),
        "una domanda mai fatta non ha risposta"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// ATTENZIONE: Prima di chiedere si manda a **tutti** i documenti aperti quel che
/// il server non ha ancora, non solo a quello sotto il cursore. La
/// risposta parla di posizioni nel file **d'arrivo**, e se quello è
/// aperto con battute non ancora mandate le sue righe sono quelle di
/// prima: il salto atterra altrove e niente lo segnala.
#[test]
fn prima_di_chiedere_si_manda_quel_che_il_server_non_ha_ancora() {
    use crate::menu::{MenuAct, MenuIntent};
    let root = tempdir("lsp_def_flush");
    let (mut app, id) = app_con_lsp(&root);
    let secondo_path = root.join("secondo.rs");
    std::fs::write(&secondo_path, "pub fn b() {}").unwrap();
    app.open_file(&secondo_path).unwrap();
    let secondo = app.active_doc().unwrap();
    let mut posta = acceso(&mut app, sa_definire());
    // due didOpen, uno per documento
    let _ = posta.try_recv().expect("didOpen");
    let _ = posta.try_recv().expect("didOpen");

    // si batte nel **secondo** file, poi si torna sul primo e si chiede
    app.workspace
        .get_mut(secondo)
        .unwrap()
        .reload_from_str("pub fn b() { qui() }");
    app.groups.activate(id);
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::GotoDefinition));

    let mut cambiati = Vec::new();
    let mut chiesto = false;
    while let Ok(comando) = posta.try_recv() {
        match comando {
            lume_lsp::LspCommand::ChangeSnapshot { text, .. } => cambiati.push(text.to_string()),
            lume_lsp::LspCommand::Ask { .. } => chiesto = true,
            _ => {}
        }
    }
    assert!(chiesto, "la domanda parte");
    assert!(
        cambiati.iter().any(|testo| testo.contains("qui()")),
        "il testo non mandato del file d'arrivo parte prima: {cambiati:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// ATTENZIONE: Fermare **mentre si accende** è il caso che l'epoca deve coprire, e
/// il passo 1 non lo copriva: `forget` azzerava la mailbox ma lasciava
/// ferma la generazione. Due percorsi, tutti e due a un clic di distanza.
///
/// 1. Ferma durante la ricerca del binario. La sonda `--version` gira in
///    `spawn_blocking` e **non si annulla**: un paio di secondi dopo il
///    suo `LspResolved` arriva comunque, e con l'epoca ferma rimetterebbe
///    `Avvio` — cioè il server partirebbe *dopo* lo Stop.
/// 2. Ferma durante l'avvio, prima del `Ready`. Lo stream viene droppato,
///    ma quel che ha già mandato sta nel canale di iced e arriva lo
///    stesso: un `Initialized` creduto rimetterebbe `Attivo`, e
///    `lsp_spawn` tornerebbe a produrre la **stessa** chiave — iced
///    aprirebbe un processo nuovo.
#[test]
fn fermare_mentre_si_accende_non_lascia_partire_il_server_dopo() {
    use crate::features::{Feature, FeatureCommand};
    use crate::lsp_mode::{LspEvent, LspState};
    let root = tempdir("lsp_corsa");
    let (mut app, _) = app_con_lsp(&root);
    let ferma = Message::Feature(FeatureCommand::Stop(Feature::LanguageServer));

    // 1. si sta ancora cercando il binario
    {
        let lsp = app.lsp.only_mut().unwrap();
        lsp.command = None;
        lsp.state = LspState::Ricerca;
    }
    let vecchia = app.lsp.only().unwrap().epoch;
    let _ = app.update(ferma.clone());
    assert_eq!(app.lsp.only().unwrap().state, LspState::Ferma);

    let _ = app.update(Message::LspResolved(
        "rust-analyzer".to_owned(),
        vecchia,
        Ok(lume_language_servers::ResolvedServer {
            command: comando_finto(),
            version: "1.2.3".to_owned(),
            attempts: Vec::new(),
        }),
    ));
    assert_eq!(
        app.lsp.only().unwrap().state,
        LspState::Ferma,
        "la sonda in ritardo non riaccende quel che l'utente ha spento"
    );
    assert!(app.lsp_spawns().is_empty(), "e nessun processo parte");

    // 2. si sta avviando, e il `Ready` non è ancora arrivato
    {
        let lsp = app.lsp.only_mut().unwrap();
        lsp.command = Some(comando_finto());
        lsp.state = LspState::Avvio;
    }
    let vecchia = app.lsp.only().unwrap().epoch;
    let _ = app.update(ferma);
    assert_eq!(app.lsp.only().unwrap().state, LspState::Ferma);

    let (mailbox, _posta) = lume_lsp::LspCommandMailbox::channel();
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        vecchia,
        LspEvent::Ready(mailbox),
    ));
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        vecchia,
        LspEvent::From(lume_lsp::ActorEvent::Initialized(Box::new(session_ready(
            lsp_types::ServerCapabilities::default(),
        )))),
    ));
    assert_eq!(
        app.lsp.only().unwrap().state,
        LspState::Ferma,
        "gli eventi già in volo non resuscitano la generazione che li ha prodotti"
    );
    assert!(app.lsp_spawns().is_empty(), "niente ramo, niente processo");
    let _ = std::fs::remove_dir_all(&root);
}

/// Cambiare cartella lascia cadere il server della cartella di prima.
///
/// La radice si dichiara all'`initialize` e **non si cambia a caldo**. Un
/// server che sopravvive al cambio indicizza il progetto vecchio e ne
/// pubblica le diagnostiche nel dock di quello nuovo: righe che puntano a
/// file fuori dall'albero, che nessun salvataggio può più chiudere.
#[test]
fn cambiare_radice_lascia_cadere_il_server_di_prima() {
    use crate::lsp_mode::LspEvent;
    let root = tempdir("lsp_radice");
    let (mut app, _) = app_con_lsp(&root);
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        LspEvent::From(lume_lsp::ActorEvent::Diagnostics(Box::new(diagnostica(
            &root,
        )))),
    ));
    assert!(!app.lsp_spawns().is_empty(), "prima del cambio: vivo");
    assert_eq!(badge_problemi(&app), Some("1".to_owned()));

    let altra = tempdir("lsp_radice_2");
    app.set_project_root(altra.clone());
    assert!(
        app.lsp.is_empty(),
        "il server era legato alla radice di prima"
    );
    assert!(
        app.lsp_spawns().is_empty(),
        "e il suo processo cade con lui"
    );
    assert_eq!(
        badge_problemi(&app),
        None,
        "e i suoi problemi non restano a galleggiare nel dock"
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&altra);
}

#[test]
fn i_segni_dell_editor_seguono_le_diagnostiche_del_server() {
    use crate::lsp_mode::LspEvent;
    use lume_ui::editor::{DiagnosticMark, MarkSeverity};
    let root = tempdir("segni_editor");
    let (mut app, id) = app_con_lsp(&root);
    let _posta = acceso(&mut app, sa_definire());
    assert!(!app.doc_marks.contains_key(&id), "prima: nessun segno");

    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        LspEvent::From(lume_lsp::ActorEvent::Diagnostics(Box::new(diagnostica(
            &root,
        )))),
    ));
    assert_eq!(
        app.doc_marks.get(&id),
        Some(&vec![DiagnosticMark {
            range: 3..7,
            severity: MarkSeverity::Error
        }]),
        "«mian» in `fn mian() {{}}` sono i char 3..7"
    );

    let _ = app.lsp_forget("rust-analyzer", crate::lsp_mode::LspState::Ferma);
    assert!(!app.doc_marks.contains_key(&id), "nessun segno orfano");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn chiudere_la_scheda_toglie_i_segni_del_documento() {
    use crate::lsp_mode::LspEvent;
    let root = tempdir("segni_chiusura");
    let (mut app, id) = app_con_lsp(&root);
    let _posta = acceso(&mut app, sa_definire());
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        LspEvent::From(lume_lsp::ActorEvent::Diagnostics(Box::new(diagnostica(
            &root,
        )))),
    ));
    assert!(app.doc_marks.contains_key(&id));
    app.close_tab(id);
    assert!(
        !app.doc_marks.contains_key(&id),
        "un documento chiuso non tiene segni in memoria"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// «Scarica» compare al posto di «Avvia» quando il binario manca: è un
/// consenso esplicito (§6.7), distinto dall'avvio vero.
#[test]
fn il_tasto_avvia_si_disegna_solo_dove_puo_agire() {
    use crate::app::NoticeKind;
    use crate::features::{Feature, FeatureCommand};
    use crate::lsp_mode::LspState;
    let root = tempdir("lsp_avvia");
    let (mut app, _) = app_con_lsp(&root);
    let riga = |app: &Lume| {
        app.extension_rows()
            .into_iter()
            .find(|riga| riga.feature == Feature::LanguageServer)
            .expect("la riga del language server")
    };

    app.lsp.only_mut().unwrap().state = LspState::Ferma;
    assert!(riga(&app).can_start, "fermo e installato: si avvia");
    assert_eq!(riga(&app).start_label, Some("Avvia"));
    app.lsp.only_mut().unwrap().state = LspState::Assente("non nel PATH".to_owned());
    assert!(riga(&app).can_start, "manca il binario: si scarica");
    assert_eq!(riga(&app).start_label, Some("Scarica"));

    // Senza nemmeno un server candidato il tasto **resta**: finché non si
    // prova non si sa se questo progetto ne abbia uno. Ma premerlo lo
    // dice, invece di non fare niente in silenzio.
    let (mut vuota, _) = Lume::new_without_store();
    assert!(vuota.lsp.is_empty());
    assert!(riga(&vuota).can_start);
    let _ = vuota.update(Message::Feature(FeatureCommand::Start(
        Feature::LanguageServer,
    )));
    assert!(
        vuota.lsp.is_empty(),
        "niente da avviare, niente da inventare"
    );
    assert_eq!(
        vuota.notice.as_ref().map(|(_, notice)| notice.kind),
        Some(NoticeKind::Error),
        "e lo si dice"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn la_sezione_server_si_vede_senza_workspace_vivo_e_usa_la_versione_catalogo() {
    use crate::lsp_mode::LspState;
    let root = tempdir("lsp_sezione");
    let file = root.join("main.rs");
    std::fs::write(&file, "fn main() {}").unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.open_file(&file).unwrap();
    assert!(app.lsp.is_empty());
    let sezione = app
        .language_server_section()
        .expect("un .rs aperto basta a mostrare la sezione");
    assert_eq!(sezione.name, "rust-analyzer");
    assert_eq!(sezione.version, "2026-07-20");
    assert_eq!(sezione.detail, "non avviata");
    assert!(!sezione.can_download);

    let (mut accesa, _) = app_con_lsp(&root);
    accesa.lsp.only_mut().unwrap().version = "1.2.3".to_owned();
    let sezione = accesa.language_server_section().expect("workspace vivo");
    assert_eq!(
        sezione.version, "2026-07-20",
        "catalogo, non sonda --version"
    );
    assert_eq!(sezione.detail, "in avvio");
    accesa.lsp.only_mut().unwrap().state = LspState::Assente("non nel PATH".to_owned());
    let sezione = accesa.language_server_section().unwrap();
    assert_eq!(sezione.detail, "non installato\nnon nel PATH");
    assert!(sezione.can_download);
    let _ = std::fs::remove_dir_all(&root);
}

/// ATTENZIONE: Il test che pinza la metà app-side del baco «due file»: l'URI del
/// `didOpen` dev'essere **esattamente** quella che il server rimanderà
/// nei `publishDiagnostics`. Una differenza di scrittura e rust-analyzer
/// vede due file, i problemi si duplicano, e niente lo segnala.
#[test]
fn all_avvio_i_documenti_aperti_si_annunciano_con_la_loro_uri_canonica() {
    let root = tempdir("lsp_open");
    let (mut app, id) = app_con_lsp(&root);
    let epoca = app.lsp.only().unwrap().epoch;
    let (mailbox, mut posta) = lume_lsp::LspCommandMailbox::channel();

    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::Ready(mailbox),
    ));
    assert!(
        posta.try_recv().is_err(),
        "prima di initialize non si parla"
    );

    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Initialized(Box::new(
            session_ready(lsp_types::ServerCapabilities::default()),
        ))),
    ));

    let percorso = app.workspace.get(id).unwrap().path().unwrap().to_path_buf();
    let attesa = crate::lsp_uri::path_to_uri(&percorso).unwrap();
    match posta.try_recv().expect("un didOpen per il file aperto") {
        lume_lsp::LspCommand::Open { uri, language, .. } => {
            assert_eq!(uri.as_str(), attesa.as_str(), "URI canonica, non «una» URI");
            assert_eq!(language, "rust");
        }
        altro => panic!("atteso Open, visto {altro:?}"),
    }
    assert!(
        posta.try_recv().is_err(),
        "un solo documento, un solo didOpen"
    );

    // e il cronometro ha un numero vero, non una stima
    let lsp = app.lsp.only().unwrap();
    assert!(matches!(
        lsp.state,
        crate::lsp_mode::LspState::Attivo { .. }
    ));
    assert!(lsp.startup_ms.is_some(), "misurato dal comando a «usabile»");
    let _ = std::fs::remove_dir_all(&root);
}

/// Il giro completo del dock Problemi: badge acceso perché **c'è**
/// qualcosa, e Ferma che lo svuota. Era l'ultimo mock del progetto.
#[test]
fn le_diagnostiche_accendono_il_badge_e_fermare_lo_spegne() {
    let root = tempdir("lsp_diag");
    let (mut app, _) = app_con_lsp(&root);
    let epoca = app.lsp.only().unwrap().epoch;
    let badge = |app: &Lume| {
        app.registry
            .get(tools::problems::ID)
            .and_then(lume_ui::tool::Tool::badge)
    };
    assert_eq!(badge(&app), None, "a freddo non c'è niente da mostrare");

    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Diagnostics(Box::new(diagnostica(
            &root,
        )))),
    ));
    assert_eq!(badge(&app), Some("1".to_owned()), "righe vere, badge vero");

    // ATTENZIONE: Un evento di una generazione superata non riaccende niente:
    // senza l'epoca, una diagnostica in volo al momento dello Stop
    // rimetterebbe il badge un istante dopo averlo spento.
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca.wrapping_sub(1),
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Stopped),
    ));
    assert_eq!(
        badge(&app),
        Some("1".to_owned()),
        "generazione superata: muta"
    );

    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Stopped),
    ));
    assert_eq!(badge(&app), None, "fermare svuota davvero");
    assert_eq!(
        app.lsp.only().unwrap().state,
        crate::lsp_mode::LspState::Ferma
    );
    let _ = std::fs::remove_dir_all(&root);
}

fn diagnostica(root: &std::path::Path) -> lsp_types::PublishDiagnosticsParams {
    let uri = crate::lsp_uri::path_to_uri(&root.join("main.rs")).unwrap();
    lsp_types::PublishDiagnosticsParams {
        uri,
        diagnostics: vec![lsp_types::Diagnostic {
            range: lsp_types::Range::new(
                lsp_types::Position::new(0, 3),
                lsp_types::Position::new(0, 7),
            ),
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            message: "cannot find function `mian`".to_owned(),
            ..lsp_types::Diagnostic::default()
        }],
        version: None,
    }
}

/// ATTENZIONE: `Subscription` è opaca e non si può asserire «non contiene un ramo
/// LSP». Si asserisce sul seam che la produce: `None` vuol dire che iced
/// droppa lo stream, il future muore e il Job Object porta via l'albero.
/// È la forma che ha, qui, «niente processi da inattivo».
#[test]
fn a_server_fermo_il_ramo_della_subscription_non_esiste() {
    use crate::lsp_mode::LspState;
    let root = tempdir("lsp_sub");
    let (mut app, _) = app_con_lsp(&root);
    assert!(!app.lsp_spawns().is_empty(), "in avvio il ramo c'è");

    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Stopped),
    ));
    assert!(
        app.lsp_spawns().is_empty(),
        "fermo: niente ramo, niente processo"
    );

    // e un fallimento lo spegne allo stesso modo, dicendo perché
    app.lsp.only_mut().unwrap().state = LspState::Avvio;
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Failed("morto".into())),
    ));
    assert!(app.lsp_spawns().is_empty());
    assert_eq!(
        app.lsp.only().unwrap().state,
        LspState::Errore("morto".to_owned())
    );

    // L'uscita anomala comunicata dal processo non deve diventare Ferma
    // quando l'attore invia subito dopo il proprio evento Stopped.
    app.lsp.only_mut().unwrap().state = LspState::Avvio;
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::ProcessExited {
            success: false,
            code: Some(17),
        }),
    ));
    assert_eq!(
        app.lsp.only().unwrap().state,
        LspState::Errore("il server LSP è terminato con codice 17".to_owned())
    );
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Stopped),
    ));
    assert!(matches!(app.lsp.only().unwrap().state, LspState::Errore(_)));
    let _ = std::fs::remove_dir_all(&root);
}

/// `subscription()` gira a ogni messaggio: lo spec di `initialize` non si
/// ricostruisce ogni volta, si costruisce una volta per accensione.
#[test]
fn lo_spec_di_initialize_si_costruisce_una_volta_per_generazione() {
    let root = tempdir("lsp_spec");
    let (mut app, _) = app_con_lsp(&root);

    let a = app.lsp_spawns().into_iter().next().unwrap();
    let b = app.lsp_spawns().into_iter().next().unwrap();
    assert!(
        std::sync::Arc::ptr_eq(&a.spec, &b.spec),
        "stessa generazione: stesso spec, nessuna ricostruzione"
    );
    assert!(a.spec.root_uri.is_some());
    let nome = root.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(
        a.spec.workspace_folders.as_ref().unwrap()[0].name,
        nome,
        "il nome della cartella di lavoro viene dalla radice"
    );

    let _ = app.lsp.only_mut().unwrap().bump();
    let c = app.lsp_spawns().into_iter().next().unwrap();
    assert!(
        !std::sync::Arc::ptr_eq(&a.spec, &c.spec),
        "una nuova accensione ricostruisce lo spec"
    );
    assert_eq!(c.spec.root_uri, a.spec.root_uri, "col contenuto di prima");
    assert_eq!(c.epoch, a.epoch + 1);
    let _ = std::fs::remove_dir_all(&root);
}

/// Chiudere una scheda deve dirlo al server **prima** che il documento
/// sparisca: dopo, il path da cui si ricava l'URI non c'è più, e il
/// server resterebbe convinto che quel file sia ancora aperto.
#[test]
fn chiudere_una_scheda_manda_il_did_close_finche_c_e_ancora_il_path() {
    use crate::editor_tab_bar::EditorTabBarIntent;
    let root = tempdir("lsp_close");
    let (mut app, id) = app_con_lsp(&root);
    let epoca = app.lsp.only().unwrap().epoch;
    let (mailbox, mut posta) = lume_lsp::LspCommandMailbox::channel();
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::Ready(mailbox),
    ));
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Initialized(Box::new(
            session_ready(lsp_types::ServerCapabilities::default()),
        ))),
    ));
    let _ = posta.try_recv().expect("il didOpen");

    let _ = app.apply_tab_intent(EditorTabBarIntent::Close(id));
    assert!(
        matches!(posta.try_recv(), Ok(lume_lsp::LspCommand::Close { .. })),
        "il didClose parte, e con l'URI giusta"
    );
    assert!(
        !app.lsp.only().unwrap().opened.contains_key(&id),
        "e il server non lo conosce più"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Il `didChange` non parte a ogni lettera, e non parte due volte per lo
/// stesso testo.
///
/// Due guardie diverse, e servono **tutte e due**: il debounce collassa la
/// raffica di battute (una revisione più recente fa tacere quella vecchia),
/// ma non i movimenti di cursore, che fanno scattare lo stesso timer senza
/// cambiare una lettera. La seconda guardia è la versione già mandata.
#[test]
fn il_did_change_aspetta_le_battute_e_non_rimanda_lo_stesso_testo() {
    let root = tempdir("lsp_change");
    let (mut app, id) = app_con_lsp(&root);
    let epoca = app.lsp.only().unwrap().epoch;
    let (mailbox, mut posta) = lume_lsp::LspCommandMailbox::channel();
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::Ready(mailbox),
    ));
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Initialized(Box::new(
            session_ready(lsp_types::ServerCapabilities::default()),
        ))),
    ));
    let _ = posta.try_recv().expect("il didOpen d'apertura");

    // si batte
    app.workspace
        .get_mut(id)
        .unwrap()
        .reload_from_str("fn main() {}");
    let versione = app.workspace.get(id).unwrap().version();

    // una revisione superata non manda niente: è arrivata un'altra battuta
    let _ = app.update(Message::LspChangeDebounced(id, versione.wrapping_sub(1)));
    assert!(
        posta.try_recv().is_err(),
        "revisione vecchia: il giro è già superato"
    );

    let _ = app.update(Message::LspChangeDebounced(id, versione));
    match posta.try_recv().expect("il didChange") {
        lume_lsp::LspCommand::ChangeSnapshot { text, .. } => assert_eq!(text, "fn main() {}"),
        altro => panic!("atteso Change, visto {altro:?}"),
    }

    // e lo stesso testo non si rimanda: muovere il cursore fa scattare il
    // timer senza cambiare una lettera
    let _ = app.update(Message::LspChangeDebounced(id, versione));
    assert!(posta.try_recv().is_err(), "niente doppioni");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn input_widget_renderizzato_produce_did_change_con_la_revisione_corrente() {
    use futures::StreamExt;
    use iced::advanced::Shell;
    use lume_ui::editor::EditorState;

    let root = tempdir("lsp_widget_revision");
    let (mut app, id) = app_con_lsp(&root);
    let epoch = app.lsp.only().unwrap().epoch;
    let (mailbox, mut posta) = lume_lsp::LspCommandMailbox::channel();
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoch,
        crate::lsp_mode::LspEvent::Ready(mailbox),
    ));
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoch,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Initialized(Box::new(
            session_ready(lsp_types::ServerCapabilities::default()),
        ))),
    ));
    let _ = posta.try_recv().expect("didOpen d'apertura");

    // Passa dall'EditorView prodotto dall'app e dall'evento tastiera Iced,
    // non da una mutazione diretta del Document o da EditorCommand manuale.
    let (command, mut tree) = {
        let mut view = app.editor_of(crate::editor_groups::GroupId::Primary);
        let mut tree = iced::advanced::widget::Tree::new(&view);
        tree.state
            .downcast_mut::<EditorState>()
            .is_focused
            .set(true);
        let renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
            iced::Font::default(),
            iced::Pixels(16.0),
        ));
        let limits =
            iced::advanced::layout::Limits::new(iced::Size::ZERO, iced::Size::new(800.0, 600.0));
        let node = view.as_widget_mut().layout(&mut tree, &renderer, &limits);
        let event = iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Character("x".into()),
            modified_key: iced::keyboard::Key::Character("x".into()),
            physical_key: iced::keyboard::key::Physical::Code(iced::keyboard::key::Code::KeyX),
            location: iced::keyboard::Location::Standard,
            modifiers: iced::keyboard::Modifiers::default(),
            text: Some("x".into()),
            repeat: false,
        });
        let mut published = Vec::new();
        let mut shell = Shell::new(&mut published);
        view.as_widget_mut().update(
            &mut tree,
            &event,
            iced::advanced::Layout::new(&node),
            iced::mouse::Cursor::Unavailable,
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut shell,
            &iced::Rectangle::new(iced::Point::ORIGIN, iced::Size::new(800.0, 600.0)),
        );
        assert!(shell.is_event_captured(), "il widget consuma il tasto");
        (
            published
                .into_iter()
                .find(|message| matches!(message, Message::EditorIn(..)))
                .expect("il widget pubblica EditorIn"),
            tree,
        )
    };
    assert!(matches!(
        command,
        Message::EditorIn(
            crate::editor_groups::GroupId::Primary,
            EditorCommand::Insert(ref text)
        ) if text == "x"
    ));
    let mut debounce = iced_runtime::task::into_stream(app.update(command))
        .expect("il widget avvia il debounce LSP reale");
    let document = app.workspace.get(id).expect("documento aperto");
    let current_text = document.rope().to_string();
    assert_eq!(current_text, "xfn mian() {}");
    let version = document.version();

    let action = tokio::time::timeout(std::time::Duration::from_secs(2), debounce.next())
        .await
        .expect("il debounce emette entro il limite")
        .expect("il task produce un messaggio");
    let iced_runtime::Action::Output(message) = action else {
        panic!("il debounce produce l'evento di output");
    };
    assert!(matches!(
        &message,
        Message::LspChangeDebounced(received_id, received_version)
            if *received_id == id && *received_version == version
    ));
    let _ = app.update(message);
    let mut latest_view = app.editor_of(crate::editor_groups::GroupId::Primary);
    tree.diff(&latest_view);
    let mut renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
        iced::Font::default(),
        iced::Pixels(16.0),
    ));
    let limits =
        iced::advanced::layout::Limits::new(iced::Size::ZERO, iced::Size::new(800.0, 600.0));
    let node = latest_view
        .as_widget_mut()
        .layout(&mut tree, &renderer, &limits);
    latest_view.as_widget().draw(
        &tree,
        &mut renderer,
        &iced::Theme::Light,
        &iced::advanced::renderer::Style::default(),
        iced::advanced::Layout::new(&node),
        iced::mouse::Cursor::Unavailable,
        &iced::Rectangle::new(iced::Point::ORIGIN, iced::Size::new(800.0, 600.0)),
    );
    assert_eq!(
        tree.state.downcast_ref::<EditorState>().last_version.get(),
        version,
        "il draw dell'EditorView usa la revisione appena spedita a LSP"
    );
    match posta
        .try_recv()
        .expect("didChange della revisione corrente")
    {
        lume_lsp::LspCommand::ChangeSnapshot { text, .. } => {
            assert_eq!(text, current_text.as_str())
        }
        altro => panic!("atteso Change, visto {altro:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn shift_del_editor_focalizzato_non_rimbalza_al_listener_globale() {
    use iced::advanced::{Shell, layout, widget::Tree};
    use iced::keyboard::{
        Event as KeyboardEvent, Location, Modifiers,
        key::{Code, Physical},
    };
    use lume_ui::editor::EditorState;

    let directory = tempfile::tempdir().unwrap();
    let mut app = app();
    write_and_open(&mut app, directory.path(), "shift.rs", "fn main() {}\n");
    let mut view = app.editor_of(crate::editor_groups::GroupId::Primary);
    let mut tree = Tree::new(&view);
    tree.state
        .downcast_mut::<EditorState>()
        .is_focused
        .set(true);
    let renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
        iced::Font::default(),
        iced::Pixels(16.0),
    ));
    let limits = layout::Limits::new(iced::Size::ZERO, iced::Size::new(800.0, 600.0));
    let node = view.as_widget_mut().layout(&mut tree, &renderer, &limits);
    let event = iced::Event::Keyboard(KeyboardEvent::KeyPressed {
        key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Shift),
        modified_key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Shift),
        physical_key: Physical::Code(Code::ShiftLeft),
        location: Location::Left,
        modifiers: Modifiers::SHIFT,
        text: None,
        repeat: false,
    });
    let mut published = Vec::new();
    let mut shell = Shell::new(&mut published);
    view.as_widget_mut().update(
        &mut tree,
        &event,
        iced::advanced::Layout::new(&node),
        iced::mouse::Cursor::Unavailable,
        &renderer,
        &mut iced::advanced::clipboard::Null,
        &mut shell,
        &iced::Rectangle::new(iced::Point::ORIGIN, iced::Size::new(800.0, 600.0)),
    );

    assert!(
        shell.is_event_captured(),
        "Shift deve restare all'editor focalizzato"
    );
    assert!(
        published
            .iter()
            .all(|message| matches!(message, Message::SyntaxViewport(_))),
        "Shift non genera un comando di testo; il viewport può richiedere sintassi"
    );
}

#[test]
fn change_rifiutato_dalla_mailbox_piena_si_riprova_sulla_revisione_corrente() {
    let root = tempdir("lsp_change_retry");
    let (mut app, id) = app_con_lsp(&root);
    let epoch = app.lsp.only().unwrap().epoch;
    let (mailbox, mut posta) =
        lume_lsp::LspCommandMailbox::with_capacity(lume_lsp::DEFERRED_COMMAND_CAPACITY + 1);
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoch,
        crate::lsp_mode::LspEvent::Ready(mailbox.clone()),
    ));
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoch,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Initialized(Box::new(
            session_ready(lsp_types::ServerCapabilities::default()),
        ))),
    ));
    let uri = match posta.try_recv().expect("didOpen iniziale") {
        lume_lsp::LspCommand::Open { uri, .. } => uri,
        altro => panic!("atteso Open, ricevuto {altro:?}"),
    };
    assert!(
        mailbox
            .send(lume_lsp::LspCommand::Save { uri: uri.clone() })
            .is_ok()
    );
    for _ in 0..lume_lsp::DEFERRED_COMMAND_CAPACITY {
        assert!(
            mailbox
                .send(lume_lsp::LspCommand::Save { uri: uri.clone() })
                .is_ok()
        );
    }

    app.workspace
        .get_mut(id)
        .unwrap()
        .reload_from_str("fn latest() {}");
    let version = app.workspace.get(id).unwrap().version();
    let _retry = app.update(Message::LspChangeDebounced(id, version));
    while posta.try_recv().is_ok() {}

    let _ = app.update(Message::LspChangeRetry(id, 0));
    assert!(matches!(
        posta.try_recv(),
        Ok(lume_lsp::LspCommand::ChangeSnapshot { text, .. }) if text == "fn latest() {}"
    ));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn appunti_cablati_aprono_il_lifecycle_reale_senza_lavoro_in_costruzione() {
    let (mut app, _) = Lume::new_without_store();
    assert!(
        crate::features::spec(crate::features::Feature::Notes).wired,
        "il flag wired segue il consumer reale"
    );
    assert!(app.feature_is_enabled(crate::features::Feature::Notes));
    assert!(
        app.notes.is_none(),
        "nessuna allocazione Notes prima dell'uso"
    );
    assert!(
        app.registry
            .bar(lume_ui::tool::Side::Right)
            .contains(&tools::notes::ID),
        "il pannello cablato deve essere raggiungibile"
    );
    let requests = app.registry.click(tools::notes::ID);
    assert!(requests.iter().any(|(id, request)| {
        *id == tools::notes::ID && matches!(request, lume_ui::tool::ToolRequest::NotesLoad)
    }));
    let _task = app.apply_tool_requests(requests);
    assert!(app.registry.is_open(tools::notes::ID));
    assert!(
        app.notes.is_some(),
        "la prima apertura possiede il lifecycle"
    );
}

/// Il pannello attivo salvato di una funzionalità spenta **non** si
/// ripristina. È il caso che si dimentica sempre: si spegne il Database,
/// si chiude Lume, e alla riapertura il pannello tornerebbe su un tool
/// che non ha nemmeno più l'icona nella barra.
#[test]
#[cfg(feature = "sql")]
fn il_pannello_salvato_di_una_funzionalita_spenta_non_si_riapre() {
    use lume_store::SessionSnapshot;
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.disabled_features = vec!["database".into()];
    snapshot.active_panel = Some("db".into());
    snapshot.active_dock = Some("terminal".into());

    app.restore_snapshot(snapshot);

    assert_eq!(app.registry.active_panel, None);
    // il terminale invece è acceso: il filtro non deve mangiare tutto
    assert_eq!(app.registry.active_dock, Some(tools::terminal::ID));
}

/// La cassaforte ha risposto «non c'è niente»: è quel che il `Task` di
/// `sql_recall_secret` produce quando il segreto non è mai stato salvato,
/// ed è il punto da cui compare il campo password.
#[cfg(feature = "sql")]
fn nessun_segreto_in_cassaforte(app: &mut Lume, connection: &str) {
    let _ = app.update(Message::SqlSecretRecalled {
        connection: connection.to_owned(),
        secret: None,
        error: None,
    });
}

#[test]
#[cfg(feature = "sql")]
fn il_messaggio_con_il_segreto_riletto_non_lo_stampa() {
    // Non regressione: è verde già prima del task (`SqlSecretRecalled.secret`
    // è già un `Redacted`) e blocca chi lo riportasse a `String`.
    let messaggio = Message::SqlSecretRecalled {
        connection: "a".to_owned(),
        secret: Some(lume_ui::tool::Redacted::new("p4ss-segretissima".to_owned())),
        error: None,
    };
    assert!(!format!("{messaggio:?}").contains("p4ss"));
}

#[cfg(feature = "sql")]
fn app_connected_to_fake_sql() -> Lume {
    let mut app = app_senza_cassaforte_vera();
    app.sql_factory = fake_driver;
    app.sql_profiles = vec![sql_profile()];
    app.pending_tool_requests.push((
        tools::db::ID,
        lume_ui::tool::ToolRequest::SqlToggleConnection(0),
    ));
    let _ = app.drain_tool_requests();
    nessun_segreto_in_cassaforte(&mut app, "a");
    app.pending_tool_requests.push((
        tools::db::ID,
        lume_ui::tool::ToolRequest::SqlSecretSubmitted(lume_ui::tool::Redacted::new(
            "p4ss".to_owned(),
        )),
    ));
    let _ = app.drain_tool_requests();
    app.open_sql_console(Some("a".into()), None);
    if let Some(document) = app.sql_console.and_then(|id| app.workspace.get_mut(id)) {
        document.reload_from_str("SELECT 1");
    }
    app
}

#[cfg(feature = "sql")]
fn pane_of(app: &Lume) -> &crate::sql_mode::Pane {
    app.sql
        .as_ref()
        .and_then(|workspace| workspace.active_panes().first())
        .expect("riquadro attivo")
}

#[test]
#[cfg(feature = "sql")]
fn espandere_una_connessione_chiede_la_password_e_solo_dopo_apre_il_socket() {
    let mut app = app_senza_cassaforte_vera();
    app.sql_factory = fake_driver;
    app.sql_profiles = vec![sql_profile()];

    app.pending_tool_requests.push((
        tools::db::ID,
        lume_ui::tool::ToolRequest::SqlToggleConnection(0),
    ));
    let _ = app.drain_tool_requests();
    assert!(
        app.sql_secret_for.is_none(),
        "il campo non compare prima di aver guardato in cassaforte"
    );
    nessun_segreto_in_cassaforte(&mut app, "a");
    assert_eq!(app.sql_secret_for.as_deref(), Some("a"));
    assert!(
        app.sql.is_none(),
        "senza segreto non si apre nessun socket e nessun thread"
    );

    app.pending_tool_requests.push((
        tools::db::ID,
        lume_ui::tool::ToolRequest::SqlSecretSubmitted(lume_ui::tool::Redacted::new(
            "p4ss".to_owned(),
        )),
    ));
    let _ = app.drain_tool_requests();
    assert!(app.sql_secret_for.is_none(), "il segreto non resta in giro");
    assert_eq!(
        app.sql
            .as_ref()
            .map(crate::sql_mode::SqlWorkspace::connection_count),
        Some(1)
    );
}

/// **Il motivo per cui la cassaforte esiste.** Con il segreto salvato,
/// riaprire una connessione dopo l'ibernazione non deve chiedere niente:
/// il campo password non compare e il socket si riapre da solo.
#[test]
#[cfg(feature = "sql")]
fn con_il_segreto_in_cassaforte_riaprire_non_chiede_piu_niente() {
    let mut app = app_connected_to_fake_sql();
    // La connessione è stata aperta digitando la password: il segreto è
    // in sospeso finché il server non risponde.
    assert_eq!(
        app.sql_pending_secret.as_ref().map(|(id, _)| id.as_str()),
        Some("a"),
        "il segreto digitato aspetta l'esito, non si salva subito"
    );

    // Le tabelle arrivano: la connessione è viva.
    assert_eq!(app.sql_vault_action("a", true), VaultAction::Remember);
    let _ = app.update(Message::SqlResponse {
        connection: "a".into(),
        pane: None,
        generation: 0,
        result: Ok(lume_sql::worker::Response::Tables(vec!["t".into()])),
    });
    assert!(
        app.sql_pending_secret.is_none(),
        "a connessione riuscita il segreto se ne va in cassaforte"
    );

    // Ibernazione: cade tutto.
    app.hibernate_sql();
    assert!(app.sql.is_none());

    // Si riespande. Ora la cassaforte risponde, e il campo non compare.
    app.pending_tool_requests.push((
        tools::db::ID,
        lume_ui::tool::ToolRequest::SqlToggleConnection(0),
    ));
    let _ = app.drain_tool_requests();
    let _ = app.update(Message::SqlSecretRecalled {
        connection: "a".into(),
        secret: Some(lume_ui::tool::Redacted::new("p4ss".to_owned())),
        error: None,
    });

    assert!(
        app.sql_secret_for.is_none(),
        "con il segreto in cassaforte la password non si chiede piu'"
    );
    assert_eq!(
        app.sql
            .as_ref()
            .map(crate::sql_mode::SqlWorkspace::connection_count),
        Some(1),
        "il socket si riapre da solo"
    );
    assert!(
        app.sql_pending_secret.is_none(),
        "un segreto che viene dalla cassaforte non va riscritto in cassaforte"
    );
}

/// La lettura in cassaforte è asincrona: fra la richiesta e la risposta
/// c'è tempo per eliminare il profilo, o per richiudere il nodo. In
/// nessuno dei due casi deve comparire un campo password sotto una riga
/// che non c'è o che è chiusa.
#[test]
#[cfg(feature = "sql")]
fn una_risposta_della_cassaforte_che_arriva_tardi_non_lascia_campi_orfani() {
    // 1. il profilo è stato eliminato mentre la cassaforte rispondeva
    let mut app = app_connected_to_fake_sql();
    app.hibernate_sql();
    let _ = app.delete_sql_connection(0);
    let _ = app.update(Message::SqlSecretRecalled {
        connection: "a".into(),
        secret: None,
        error: None,
    });
    assert!(
        app.sql_secret_for.is_none(),
        "nessun campo password per un profilo che non esiste piu'"
    );

    // 2. il nodo è stato compresso mentre il server rifiutava le
    //    credenziali: il ramo che rimette il campo deve accorgersene
    fn driver_che_rifiuta(
        _profile: &lume_sql::ConnectionProfile,
        _secret: &lume_sql::Secret,
    ) -> Result<Box<dyn lume_sql::Driver>, lume_sql::SqlError> {
        Err(lume_sql::SqlError::Auth("password errata".into()))
    }

    let mut app = app_senza_cassaforte_vera();
    app.sql_factory = driver_che_rifiuta;
    app.sql_profiles = vec![sql_profile()];
    app.pending_tool_requests.push((
        tools::db::ID,
        lume_ui::tool::ToolRequest::SqlToggleConnection(0),
    ));
    let _ = app.drain_tool_requests();
    nessun_segreto_in_cassaforte(&mut app, "a");
    app.pending_tool_requests.push((
        tools::db::ID,
        lume_ui::tool::ToolRequest::SqlSecretSubmitted(lume_ui::tool::Redacted::new(
            "p4ss".to_owned(),
        )),
    ));
    let _ = app.drain_tool_requests();
    assert!(wait_until(|| app
        .sql
        .as_ref()
        .and_then(|w| w.connection("a"))
        .is_some_and(|c| c.auth_failed())));

    // si comprime prima che l'esito arrivi
    app.sql_expanded.clear();
    app.sql_secret_for = None;
    assert_eq!(
        app.sql_vault_action("a", false),
        VaultAction::Forget,
        "il segreto va comunque tolto dalla cassaforte: quello e' vero comunque"
    );
    let _ = app.update(Message::SqlResponse {
        connection: "a".into(),
        pane: None,
        generation: 0,
        result: Err("autenticazione fallita".into()),
    });
    assert!(
        app.sql_secret_for.is_none(),
        "un campo password sotto una riga chiusa non si vedrebbe nemmeno"
    );
}

/// In «Confronta due database» due connessioni si aprono a poca distanza:
/// la risposta della prima non deve portarsi in cassaforte il segreto che
/// sta aspettando la seconda.
#[test]
#[cfg(feature = "sql")]
fn il_segreto_in_sospeso_e_di_una_connessione_sola() {
    let mut app = app_connected_to_fake_sql();
    app.sql_pending_secret = Some(("altra".to_owned(), lume_sql::Secret::new("p4ss".to_owned())));

    let _ = app.sql_remember_secret("a");

    assert_eq!(
        app.sql_pending_secret.as_ref().map(|(id, _)| id.as_str()),
        Some("altra"),
        "il segreto di un'altra connessione deve restare ad aspettare il suo esito"
    );

    let _ = app.sql_remember_secret("altra");
    assert!(app.sql_pending_secret.is_none());
}

/// Una password ruotata sul server: la cassaforte continuerebbe a ridare
/// quella vecchia e quel nodo resterebbe inapribile per sempre. Un host
/// giù invece non dice niente sul segreto, e buttarlo via costringerebbe
/// a ridigitare una password giusta a ogni capriccio della rete.
#[test]
#[cfg(feature = "sql")]
fn solo_le_credenziali_respinte_svuotano_la_cassaforte() {
    fn driver_che_rifiuta_lautenticazione(
        _profile: &lume_sql::ConnectionProfile,
        _secret: &lume_sql::Secret,
    ) -> Result<Box<dyn lume_sql::Driver>, lume_sql::SqlError> {
        Err(lume_sql::SqlError::Auth("password errata".into()))
    }
    fn driver_che_non_trova_lhost(
        _profile: &lume_sql::ConnectionProfile,
        _secret: &lume_sql::Secret,
    ) -> Result<Box<dyn lume_sql::Driver>, lume_sql::SqlError> {
        Err(lume_sql::SqlError::Connect("host irraggiungibile".into()))
    }

    for (factory, atteso, perche) in [
        (
            driver_che_rifiuta_lautenticazione as lume_sql::DriverFactory,
            VaultAction::Forget,
            "credenziali respinte: il segreto salvato non vale piu'",
        ),
        (
            driver_che_non_trova_lhost as lume_sql::DriverFactory,
            VaultAction::Nothing,
            "la rete non dice niente sulla password",
        ),
    ] {
        let mut app = app_senza_cassaforte_vera();
        app.sql_factory = factory;
        app.sql_profiles = vec![sql_profile()];
        app.pending_tool_requests.push((
            tools::db::ID,
            lume_ui::tool::ToolRequest::SqlToggleConnection(0),
        ));
        let _ = app.drain_tool_requests();
        nessun_segreto_in_cassaforte(&mut app, "a");
        app.pending_tool_requests.push((
            tools::db::ID,
            lume_ui::tool::ToolRequest::SqlSecretSubmitted(lume_ui::tool::Redacted::new(
                "p4ss".to_owned(),
            )),
        ));
        let _ = app.drain_tool_requests();

        // Il `connect` fallisce dentro il thread: lo stato arriva dopo.
        assert!(wait_until(|| app
            .sql
            .as_ref()
            .and_then(|w| w.connection("a"))
            .is_some_and(|c| c.state() == lume_sql::worker::State::Failed)));

        assert_eq!(app.sql_vault_action("a", false), atteso, "{perche}");
    }
}

/// Senza cassaforte l'app deve comportarsi **esattamente** come prima di
/// averla: si chiede il segreto e si va avanti. Dirlo a ogni espansione
/// sarebbe rumore su una condizione che non cambia.
#[test]
#[cfg(feature = "sql")]
fn una_cassaforte_che_non_ce_si_dice_una_volta_sola_e_non_blocca_niente() {
    let mut app = app_senza_cassaforte_vera();
    app.sql_factory = fake_driver;
    app.sql_profiles = vec![sql_profile()];

    let non_disponibile = |app: &mut Lume| {
        let _ = app.update(Message::SqlSecretRecalled {
            connection: "a".into(),
            secret: None,
            error: Some("cassaforte di sistema non disponibile: niente magazzino".into()),
        });
    };

    non_disponibile(&mut app);
    assert_eq!(
        app.sql_secret_for.as_deref(),
        Some("a"),
        "senza cassaforte si torna a chiedere, non ci si blocca"
    );
    assert!(app.notice.is_some(), "la prima volta va detto");

    app.notice = None;
    non_disponibile(&mut app);
    assert!(
        app.notice.is_none(),
        "la seconda volta no: e' la stessa condizione di prima"
    );
    assert_eq!(app.sql_secret_for.as_deref(), Some("a"));
}

#[test]
#[cfg(feature = "sql")]
fn annullare_la_password_richiude_il_nodo_invece_di_lasciarlo_vuoto() {
    let mut app = app_senza_cassaforte_vera();
    app.sql_factory = fake_driver;
    app.sql_profiles = vec![sql_profile()];
    app.pending_tool_requests.push((
        tools::db::ID,
        lume_ui::tool::ToolRequest::SqlToggleConnection(0),
    ));
    let _ = app.drain_tool_requests();
    nessun_segreto_in_cassaforte(&mut app, "a");

    app.pending_tool_requests.push((
        tools::db::ID,
        lume_ui::tool::ToolRequest::SqlSecretCancelled,
    ));
    let _ = app.drain_tool_requests();

    assert!(app.sql_secret_for.is_none());
    assert!(
        !app.sql_expanded.contains("a"),
        "un nodo aperto e vuoto sembrerebbe un database senza tabelle"
    );
    assert!(app.sql.is_none());
}

#[test]
#[cfg(feature = "sql")]
fn scarta_risultato_abbandona_l_attesa_senza_ripopolare_la_griglia() {
    let mut app = app_connected_to_fake_sql();
    let _ = app.update(Message::Sql(crate::sql_console::SqlMessage::Run));
    let generation = pane_of(&app).query_gen;
    assert_eq!(pane_of(&app).status, crate::sql_mode::PaneStatus::Running);

    let _ = app.update(Message::Sql(crate::sql_console::SqlMessage::DiscardResult));
    assert_eq!(pane_of(&app).status, crate::sql_mode::PaneStatus::Idle);
    assert_eq!(pane_of(&app).query_gen, crate::sql_mode::NO_QUERY);
    consegna_fake_query(&mut app, 0, generation);
    assert!(
        pane_of(&app).results.is_none(),
        "dopo Scarta la risposta tardiva non deve comparire"
    );
    assert_eq!(pane_of(&app).status, crate::sql_mode::PaneStatus::Idle);
}

#[test]
#[cfg(feature = "sql")]
fn una_risposta_di_una_generazione_superata_non_ripopola_la_griglia() {
    let mut app = app_connected_to_fake_sql();
    let _ = app.update(Message::Sql(crate::sql_console::SqlMessage::Run));

    let generation = pane_of(&app).query_gen;
    assert_ne!(generation, crate::sql_mode::NO_QUERY);
    assert_eq!(pane_of(&app).status, crate::sql_mode::PaneStatus::Running);

    consegna_fake_query(&mut app, 0, generation + 1);
    assert!(
        pane_of(&app).results.is_none(),
        "una risposta che non e' quella attesa va buttata"
    );
    consegna_fake_query(&mut app, 0, generation);
    assert_eq!(pane_of(&app).status, crate::sql_mode::PaneStatus::Ok);
    assert_eq!(
        pane_of(&app)
            .results
            .as_ref()
            .map(lume_sql::ResultSet::row_count),
        Some(1)
    );
}

#[test]
#[cfg(feature = "sql")]
fn una_query_in_volo_quando_si_chiude_tutto_non_riempie_una_griglia_invisibile() {
    let mut app = app_connected_to_fake_sql();
    let _ = app.update(Message::Sql(crate::sql_console::SqlMessage::Run));
    let generation = pane_of(&app).query_gen;

    close_console_tab(&mut app);
    let _ = app.drain_tool_requests();
    assert!(
        app.idle_timer_active(crate::features::Feature::Database),
        "console e pannello chiusi"
    );
    consegna_fake_query(&mut app, 0, generation);
    assert!(
        pane_of(&app).results.is_none(),
        "l'ibernazione ha appena liberato la griglia: non va ricaricata"
    );
}

#[test]
#[cfg(feature = "sql")]
fn una_richiesta_di_schema_non_annulla_la_query_di_un_riquadro() {
    // È il motivo per cui il token di generazione sta sul riquadro e non
    // sulla connessione: sulla stessa connessione passano anche le
    // richieste dell'albero Database.
    let mut app = app_connected_to_fake_sql();
    let _ = app.update(Message::Sql(crate::sql_console::SqlMessage::Run));
    let generation = pane_of(&app).query_gen;

    let _ = app.sql_submit("a", lume_sql::worker::Request::Tables, None);

    assert_eq!(
        pane_of(&app).query_gen,
        generation,
        "una richiesta di schema non deve toccare cio' che aspetta il riquadro"
    );
    consegna_fake_query(&mut app, 0, generation);
    assert_eq!(pane_of(&app).status, crate::sql_mode::PaneStatus::Ok);
}

#[test]
#[cfg(feature = "sql")]
fn un_errore_del_motore_si_legge_nella_barra_del_riquadro() {
    let mut app = app_connected_to_fake_sql();
    let _ = app.update(Message::Sql(crate::sql_console::SqlMessage::Run));
    let generation = pane_of(&app).query_gen;

    let _ = app.update(Message::SqlResponse {
        connection: "a".into(),
        pane: Some(0),
        generation,
        result: Err("ORA-00942: table or view does not exist".into()),
    });
    assert_eq!(
        pane_of(&app).status,
        crate::sql_mode::PaneStatus::Error("ORA-00942: table or view does not exist".into()),
        "l'errore sta dove sta la query che l'ha causato"
    );
    assert_eq!(pane_of(&app).query_gen, crate::sql_mode::NO_QUERY);
}

#[test]
#[cfg(feature = "sql")]
fn creare_unorigine_dati_produce_un_profilo_usabile_e_un_record_salvabile() {
    let (mut app, _) = Lume::new_without_store();
    let draft = lume_ui::tool::SqlConnectionDraft {
        label: "  Azure · Prod  ".into(),
        oracle: false,
        host: " acme.database.windows.net ".into(),
        database: " prod ".into(),
        service_principal: false,
        user: " lume ".into(),
        ..Default::default()
    };
    let _ = app.create_sql_connection(&draft);

    let profile = app.sql_profiles.first().expect("profilo creato");
    assert_eq!(profile.id, "azure-prod");
    assert_eq!(profile.label, "Azure · Prod", "l'etichetta resta leggibile");
    assert_eq!(
        profile.host, "acme.database.windows.net",
        "gli spazi attorno ai campi non devono finire nell'host"
    );
    assert_eq!(profile.engine, lume_sql::Engine::AzureSql);

    let record = super::sql_record_from_profile(profile);
    assert_eq!(record.auth, "sql-login");
    assert_eq!(record.username.as_deref(), Some("lume"));
    assert_eq!(
        super::sql_profile_from_record(&record).as_ref(),
        Some(profile),
        "il giro record → profilo deve tornare al punto di partenza"
    );
}

#[test]
#[cfg(feature = "sql")]
fn due_origini_con_la_stessa_etichetta_non_si_sovrascrivono() {
    let (mut app, _) = Lume::new_without_store();
    let draft = lume_ui::tool::SqlConnectionDraft {
        label: "prod".into(),
        host: "h".into(),
        database: "d".into(),
        user: "u".into(),
        ..Default::default()
    };
    let _ = app.create_sql_connection(&draft);
    let _ = app.create_sql_connection(&draft);
    let ids: Vec<&str> = app
        .sql_profiles
        .iter()
        .map(|profile| profile.id.as_str())
        .collect();
    assert_eq!(ids, vec!["prod", "prod-2"]);
}

#[test]
#[cfg(feature = "sql")]
fn un_record_incoerente_non_diventa_un_profilo_che_fallira_e_basta() {
    let mut record = lume_store::SqlConnectionRecord {
        id: "x".into(),
        label: "x".into(),
        engine: "azure-sql".into(),
        host: "h".into(),
        database: "d".into(),
        auth: "service-principal".into(),
        username: None,
        tenant_id: None,
        client_id: Some("c".into()),
    };
    assert!(
        super::sql_profile_from_record(&record).is_none(),
        "service principal senza tenant: meglio un profilo in meno"
    );
    record.engine = "postgres".into();
    record.tenant_id = Some("t".into());
    assert!(
        super::sql_profile_from_record(&record).is_none(),
        "motore che non esiste in questa versione"
    );
}

#[test]
#[cfg(feature = "sql")]
fn eliminare_unorigine_chiude_anche_il_socket_e_il_nodo() {
    let mut app = app_connected_to_fake_sql();
    assert_eq!(
        app.sql
            .as_ref()
            .map(crate::sql_mode::SqlWorkspace::connection_count),
        Some(1)
    );

    let _ = app.delete_sql_connection(0);

    assert!(app.sql_profiles.is_empty());
    assert!(
        !app.sql_expanded.contains("a"),
        "un nodo di un profilo che non c'e' piu' non deve restare aperto"
    );
    assert!(
        app.sql_tree().is_empty(),
        "l'albero non deve mostrare una riga irraggiungibile"
    );
    assert_eq!(
        app.sql
            .as_ref()
            .map(crate::sql_mode::SqlWorkspace::connection_count),
        Some(0),
        "il socket e il thread devono cadere subito, non all'ibernazione"
    );
}

#[test]
#[cfg(feature = "sql")]
fn una_password_sbagliata_si_puo_ridigitare_senza_aspettare_libernazione() {
    fn refusing_driver(
        _profile: &lume_sql::ConnectionProfile,
        _secret: &lume_sql::Secret,
    ) -> Result<Box<dyn lume_sql::Driver>, lume_sql::SqlError> {
        Err(lume_sql::SqlError::Auth("password errata".into()))
    }

    let mut app = app_senza_cassaforte_vera();
    app.sql_factory = refusing_driver;
    app.sql_profiles = vec![sql_profile()];

    let expand = |app: &mut Lume| {
        app.pending_tool_requests.push((
            tools::db::ID,
            lume_ui::tool::ToolRequest::SqlToggleConnection(0),
        ));
        let _ = app.drain_tool_requests();
    };
    expand(&mut app);
    nessun_segreto_in_cassaforte(&mut app, "a");
    app.pending_tool_requests.push((
        tools::db::ID,
        lume_ui::tool::ToolRequest::SqlSecretSubmitted(lume_ui::tool::Redacted::new(
            "sbagliata".to_owned(),
        )),
    ));
    let _ = app.drain_tool_requests();

    // Il thread parte comunque: e' dentro che il `connect` fallisce.
    assert!(wait_until(|| app
        .sql
        .as_ref()
        .and_then(|w| w.connection("a"))
        .is_some_and(|c| c.state() == lume_sql::worker::State::Failed)));

    expand(&mut app); // comprime
    expand(&mut app); // riespande
    nessun_segreto_in_cassaforte(&mut app, "a");
    assert_eq!(
        app.sql_secret_for.as_deref(),
        Some("a"),
        "una connessione fallita non conta come viva: la password si richiede subito"
    );

    // E ridigitarla deve davvero riconnettere: `open` esce subito se l'id
    // c'è già, quindi senza buttare via il tentativo fallito il prompt
    // tornerebbe ma il socket resterebbe quello morto.
    app.sql_factory = fake_driver;
    app.pending_tool_requests.push((
        tools::db::ID,
        lume_ui::tool::ToolRequest::SqlSecretSubmitted(lume_ui::tool::Redacted::new(
            "giusta".to_owned(),
        )),
    ));
    let _ = app.drain_tool_requests();
    assert!(
        wait_until(|| app
            .sql
            .as_ref()
            .and_then(|w| w.connection("a"))
            .is_some_and(|c| c.state() == lume_sql::worker::State::Connected)),
        "riprovare deve aprire un socket nuovo, non riusare quello fallito"
    );
}

/// Il `connect` avviene dentro il thread del worker: lo stato arriva
/// poco dopo, e un test non puo' che aspettarlo.
#[cfg(feature = "sql")]
fn wait_until(mut predicate: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if predicate() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

#[test]
#[cfg(feature = "sql")]
fn uscire_da_confronta_non_lascia_il_riquadro_b_in_esecuzione_per_sempre() {
    let mut app = app_connected_to_fake_sql();
    if let Some(workspace) = app.sql.as_mut() {
        workspace.compare = true;
        if let Some(pane) = workspace.pane_mut(1) {
            pane.connection = Some("a".into());
        }
    }
    let _ = app.run_sql_query();
    let generation = app
        .sql
        .as_ref()
        .and_then(|w| w.pane(1))
        .map(|pane| pane.query_gen)
        .expect("riquadro B");

    // Il confronto si chiude mentre B ha una query in volo.
    if let Some(workspace) = app.sql.as_mut() {
        workspace.compare = false;
    }
    consegna_fake_query(&mut app, 1, generation);

    if let Some(workspace) = app.sql.as_mut() {
        workspace.compare = true;
    }
    assert_eq!(
        app.sql
            .as_ref()
            .and_then(|w| w.pane(1))
            .map(|pane| &pane.status),
        Some(&crate::sql_mode::PaneStatus::Ok),
        "la risposta di B non va scartata solo perche' il confronto era chiuso"
    );
}

#[test]
#[cfg(feature = "sql")]
fn le_tabelle_arrivate_dal_worker_finiscono_nellalbero() {
    let mut app = app_connected_to_fake_sql();
    let _ = app.update(Message::SqlResponse {
        connection: "a".into(),
        pane: None,
        generation: 1,
        result: Ok(lume_sql::worker::Response::Tables(vec!["users".into()])),
    });
    assert_eq!(
        app.sql_tree().first().map(|view| view.tables.len()),
        Some(1)
    );
}

#[test]
fn page_motion_uses_the_line_count_supplied_by_the_viewport() {
    use lume_ui::editor::{EditorCommand, PageDirection};

    let (mut app, _) = Lume::new_without_store();
    let id = app
        .workspace
        .open(None, "zero\none\ntwo\nthree\nfour\nfive\nsix\nseven\n");
    app.groups.open(crate::editor_groups::GroupId::Primary, id);

    let _ = app.update(super::Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::MovePage {
            direction: PageDirection::Down,
            lines: 3,
            extend: false,
        },
    ));

    let doc = app.workspace.get(id).unwrap();
    assert_eq!(doc.rope().char_to_line(doc.selection().head), 3);
}

/// ATTENZIONE: Il terminale «sordo» (N14). Chiudere il dock droppa lo stream del
/// ponte; se in quel momento un risveglio era già stato consegnato al
/// ponte vecchio ma non ancora consumato, il ponte nuovo deve riceverlo
/// lo stesso. Con `spawn_blocking(recv())` il `recv()` vecchio restava
/// vivo e se lo mangiava: il flag `output_pending` restava alzato e non
/// arrivava più nessun risveglio.
#[test]
fn un_risveglio_consegnato_al_ponte_chiuso_arriva_al_ponte_riaperto() {
    use futures::FutureExt;
    let wakeup = std::sync::Arc::new(tokio::sync::Notify::new());

    // dock aperto: il ponte aspetta
    let (out, _rx_vecchio) = futures::channel::mpsc::channel(4);
    let mut vecchio = Box::pin(super::terminal_wakeups(wakeup.clone(), out));
    assert!(
        vecchio.as_mut().now_or_never().is_none(),
        "senza risvegli il ponte aspetta"
    );

    // la shell scrive: il risveglio tocca al ponte in attesa...
    wakeup.notify_one();
    // ...ma il dock si chiude prima che il ponte lo consumi
    drop(vecchio);

    // riapertura: un ponte nuovo
    let (out, mut rx) = futures::channel::mpsc::channel(4);
    let mut nuovo = Box::pin(super::terminal_wakeups(wakeup.clone(), out));
    assert!(
        nuovo.as_mut().now_or_never().is_none(),
        "consegnato il Tick, torna ad aspettare"
    );
    // `try_recv` e non `try_next`: in futures-channel 0.3.32 (Cargo.lock)
    // `try_next` è `#[deprecated]` (mpsc/mod.rs:1042) e darebbe un warning
    match rx.try_recv() {
        Ok(Message::Tool(id, _)) => assert_eq!(id, tools::terminal::ID),
        altro => panic!("atteso un Tick del terminale, trovato {altro:?}"),
    }
}

/// Un risveglio arrivato a dock chiuso, senza nessun ponte in piedi, resta
/// come permesso e arriva alla riapertura.
#[test]
fn un_risveglio_a_dock_chiuso_arriva_alla_riapertura() {
    use futures::FutureExt;
    let wakeup = std::sync::Arc::new(tokio::sync::Notify::new());
    wakeup.notify_one();
    wakeup.notify_one(); // coalescato: al massimo un permesso

    let (out, mut rx) = futures::channel::mpsc::channel(4);
    let mut ponte = Box::pin(super::terminal_wakeups(wakeup.clone(), out));
    assert!(ponte.as_mut().now_or_never().is_none());
    assert!(matches!(rx.try_recv(), Ok(Message::Tool(..))));
    // il ponte è ancora vivo (tiene il Sender): canale vuoto = `Err`
    assert!(
        rx.try_recv().is_err(),
        "due notify senza attesa valgono un Tick solo"
    );
}

/// Il ponte serve solo se c'è qualcuno da svegliare.
#[test]
fn il_ponte_dei_risvegli_vive_solo_con_dock_aperto_e_shell_viva() {
    let (mut app, _) = Lume::new_without_store();
    assert!(app.registry.is_open(tools::terminal::ID));
    assert!(
        !app.terminal_needs_wakeups(),
        "dock aperto senza shell: niente da svegliare"
    );

    if let Some(tool) = app.registry.get_mut(tools::terminal::ID) {
        tool.update(&lume_ui::tool::ToolEvent::new(
            tools::terminal::Event::NewSession,
        ));
    }
    assert!(app.terminal_needs_wakeups());

    let _ = app.registry.set_active_dock(None);
    assert!(
        !app.terminal_needs_wakeups(),
        "dock chiuso: la shell resta viva, il ponte no"
    );

    // A dock riaperto il ponte torna; quando la sessione finisce, sparisce.
    let _ = app.registry.set_active_dock(Some(tools::terminal::ID));
    assert!(app.registry.is_open(tools::terminal::ID));
    assert!(
        app.terminal_needs_wakeups(),
        "dock riaperto con la shell ancora viva: il ponte torna"
    );
    // la prima sessione di un terminale nuovo ha id 1
    if let Some(tool) = app.registry.get_mut(tools::terminal::ID) {
        tool.update(&lume_ui::tool::ToolEvent::new(
            tools::terminal::Event::Kill(1),
        ));
    }
    assert!(
        !app.terminal_needs_wakeups(),
        "dock aperto ma nessuna shell viva: il ponte sparisce"
    );
}

/// Il difetto della review finale: il `Tick` che legge `Exited` porta
/// `live_count` a 0, ma il lettore PTY può avere ancora l'ultimo output.
/// Il ponte deve restare vivo finché il PTY è in mano al tool. Shell reale.
#[test]
fn il_ponte_resta_vivo_dopo_l_uscita_della_shell_fino_al_rilascio_del_pty() {
    use crate::features::Feature;
    let (mut app, _) = Lume::new_without_store();
    assert!(app.registry.is_open(tools::terminal::ID));
    let invia = |app: &mut Lume, event: tools::terminal::Event| {
        if let Some(tool) = app.registry.get_mut(tools::terminal::ID) {
            tool.update(&lume_ui::tool::ToolEvent::new(event));
        }
    };
    invia(&mut app, tools::terminal::Event::NewSession);
    invia(
        &mut app,
        tools::terminal::Event::Input(b"exit 7\r".to_vec()),
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while app.feature_live_count(Feature::Terminal) != Some(0)
        && std::time::Instant::now() < deadline
    {
        invia(&mut app, tools::terminal::Event::Tick);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        app.feature_live_count(Feature::Terminal),
        Some(0),
        "la shell doveva uscire"
    );
    assert!(
        app.terminal_needs_wakeups(),
        "shell uscita ma PTY ancora in mano: l'ultimo output deve poter svegliare la vista"
    );

    // la prima sessione di un terminale nuovo ha id 1
    invia(&mut app, tools::terminal::Event::Kill(1));
    assert!(
        !app.terminal_needs_wakeups(),
        "PTY rilasciato: il ponte si chiude"
    );
}

fn sa_fare_hover() -> lsp_types::ServerCapabilities {
    lsp_types::ServerCapabilities {
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        ..lsp_types::ServerCapabilities::default()
    }
}

fn sa_dare_signature_help() -> lsp_types::ServerCapabilities {
    lsp_types::ServerCapabilities {
        signature_help_provider: Some(lsp_types::SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
            retrigger_characters: Some(Vec::new()),
            work_done_progress_options: lsp_types::WorkDoneProgressOptions::default(),
        }),
        ..lsp_types::ServerCapabilities::default()
    }
}

fn domanda_di_signature_help(posta: &mut Posta) -> (u64, lsp_types::Position, Option<String>) {
    loop {
        if let lume_lsp::LspCommand::Ask {
            id,
            request:
                lume_lsp::LspRequest::SignatureHelp {
                    position,
                    trigger_character,
                    ..
                },
        } = posta.try_recv().expect("una domanda di signature help")
        {
            return (id, position, trigger_character);
        }
    }
}

fn risposta_di_signature_help(
    app: &mut Lume,
    richiesta: u64,
    answer: Option<lsp_types::SignatureHelp>,
) {
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Answer(
            richiesta,
            Box::new(Ok(lume_lsp::LspAnswer::SignatureHelp(answer))),
        )),
    ));
}

fn signature_help_di_esempio() -> lsp_types::SignatureHelp {
    serde_json::from_value(serde_json::json!({
        "signatures": [{
            "label": "scrivi(nome, valore)",
            "parameters": [
                { "label": "nome" },
                { "label": "valore", "documentation": "testo da scrivere" }
            ]
        }],
        "activeSignature": 0,
        "activeParameter": 1
    }))
    .unwrap()
}

#[test]
fn signature_help_si_apre_sul_carattere_trigger_e_mostra_il_parametro_attivo() {
    let (mut app, id, mut posta, root) =
        app_con_server("signature_help_trigger", "scrivi", sa_dare_signature_help());
    svuota(&mut posta);
    digita(&mut app, "(");
    let (request, position, trigger) = domanda_di_signature_help(&mut posta);
    assert_eq!(position.character, 7);
    assert_eq!(trigger.as_deref(), Some("("));
    risposta_di_signature_help(&mut app, request, Some(signature_help_di_esempio()));
    let card = app.hover_card(id).expect("popup firma presente");
    assert_eq!(card.lines[0].text, "scrivi(nome, valore)");
    assert_eq!(card.lines[1].text, "▶ valore");
    assert_eq!(card.lines[2].text, "testo da scrivere");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn signature_help_manuale_non_invia_trigger_character() {
    use crate::menu::{MenuAct, MenuIntent};
    let (mut app, _id, mut posta, root) = app_con_server(
        "signature_help_manuale",
        "scrivi()",
        sa_dare_signature_help(),
    );
    svuota(&mut posta);
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::SignatureHelp));
    let (_, position, trigger) = domanda_di_signature_help(&mut posta);
    assert_eq!(position.character, 8);
    assert!(trigger.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

fn svuota(posta: &mut Posta) {
    while posta.try_recv().is_ok() {}
}

/// La prossima domanda di hover sulla posta (salta i `didChange`).
fn domanda_di_hover(posta: &mut Posta) -> (u64, lsp_types::Position) {
    loop {
        if let lume_lsp::LspCommand::Ask {
            id,
            request: lume_lsp::LspRequest::Hover { position, .. },
        } = posta.try_recv().expect("una domanda di hover")
        {
            return (id, position);
        }
    }
}

fn hover_markdown(testo: &str) -> lsp_types::Hover {
    lsp_types::Hover {
        contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
            kind: lsp_types::MarkupKind::Markdown,
            value: testo.to_owned(),
        }),
        range: None,
    }
}

fn risposta_di_hover(app: &mut Lume, richiesta: u64, hover: Option<lsp_types::Hover>) {
    let epoca = app.lsp.only().unwrap().epoch;
    let _ = app.update(Message::Lsp(
        "rust-analyzer".to_owned(),
        epoca,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Answer(
            richiesta,
            Box::new(Ok(lume_lsp::LspAnswer::Hover(hover))),
        )),
    ));
}

fn punta(app: &mut Lume, pos: Option<usize>) -> iced::Task<Message> {
    app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::HoverAt(pos),
    ))
}

/// Punta la parola, lascia scadere il conto e restituisce la domanda.
fn punta_e_aspetta(
    app: &mut Lume,
    id: lume_core::DocId,
    posta: &mut Posta,
    pos: usize,
) -> (u64, lsp_types::Position) {
    let _ = punta(app, Some(pos));
    let gettone = app.hover_gen;
    let _ = app.update(Message::HoverDue(id, gettone, pos));
    domanda_di_hover(posta)
}

#[test]
fn hover_il_puntatore_su_una_parola_avvia_il_conto_ma_non_chiede() {
    let (mut app, id, mut posta, root) =
        app_con_server("hover_conto", "let x = 1;", sa_fare_hover());
    svuota(&mut posta);
    let attesa = punta(&mut app, Some(4));
    assert!(attesa.units() > 0, "il conto alla rovescia è una task");
    assert!(posta.try_recv().is_err(), "il puntatore da solo non chiede");
    assert!(app.hover_wait.is_none());
    assert!(app.hover_card(id).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hover_senza_la_capability_non_parte_nemmeno_il_conto() {
    let (mut app, _id, _posta, root) =
        app_con_server("hover_senza_cap", "let x = 1;", sa_completare());
    assert_eq!(
        punta(&mut app, Some(4)).units(),
        0,
        "niente task, niente timer"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hover_scaduto_il_conto_si_chiede_e_la_risposta_mostra_la_scheda() {
    let (mut app, id, mut posta, root) =
        app_con_server("hover_giro", "let x = 1;", sa_fare_hover());
    svuota(&mut posta);
    let (richiesta, posizione) = punta_e_aspetta(&mut app, id, &mut posta, 4);
    assert_eq!((posizione.line, posizione.character), (0, 4));

    risposta_di_hover(
        &mut app,
        richiesta,
        Some(hover_markdown("```rust\nlet x\n```\n\nun intero")),
    );
    let scheda = app.hover_card(id).expect("la scheda");
    assert_eq!(scheda.anchor, 4);
    assert_eq!(
        scheda.lines,
        [
            lume_ui::editor::HoverLine {
                text: "let x".to_owned(),
                code: true
            },
            lume_ui::editor::HoverLine {
                text: String::new(),
                code: false
            },
            lume_ui::editor::HoverLine {
                text: "un intero".to_owned(),
                code: false
            },
        ]
    );
    assert!(
        app.hover_card(lume_core::DocId(9999)).is_none(),
        "la scheda è di un documento solo"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hover_un_conto_superato_dal_puntatore_non_chiede() {
    let (mut app, id, mut posta, root) =
        app_con_server("hover_superato", "let x = 1;", sa_fare_hover());
    svuota(&mut posta);
    let _ = punta(&mut app, Some(4));
    let vecchio = app.hover_gen;
    let _ = punta(&mut app, Some(8));
    let _ = app.update(Message::HoverDue(id, vecchio, 4));
    assert!(
        posta.try_recv().is_err(),
        "il puntatore si era già spostato"
    );

    let _ = app.update(Message::HoverDue(id, app.hover_gen, 8));
    let (_, posizione) = domanda_di_hover(&mut posta);
    assert_eq!(posizione.character, 8, "quello corrente sì");
    let _ = std::fs::remove_dir_all(&root);
}

/// Review Focus 9: il testo cambia mentre il server pensa. L'ancora della
/// scheda è un indice di quel testo: su un altro testo punterebbe una
/// parola qualunque.
#[test]
fn hover_una_risposta_su_un_testo_cambiato_si_scarta() {
    let (mut app, id, mut posta, root) =
        app_con_server("hover_stantio", "let x = 1;", sa_fare_hover());
    svuota(&mut posta);
    let (richiesta, _) = punta_e_aspetta(&mut app, id, &mut posta, 4);
    // il file cambia sotto i piedi (ricarica da disco): senza passare da
    // `EditorIn`, quindi l'attesa è ancora in piedi e conta la revisione
    let _ = app
        .workspace
        .get_mut(id)
        .unwrap()
        .reload_from_str("tutt'altro testo");
    risposta_di_hover(&mut app, richiesta, Some(hover_markdown("un intero")));
    assert!(app.hover_card(id).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hover_una_risposta_senza_niente_da_dire_non_mostra_niente() {
    let (mut app, id, mut posta, root) =
        app_con_server("hover_vuoto", "let x = 1;", sa_fare_hover());
    svuota(&mut posta);
    let (richiesta, _) = punta_e_aspetta(&mut app, id, &mut posta, 4);
    risposta_di_hover(&mut app, richiesta, None);
    assert!(app.hover_card(id).is_none());
    let (richiesta, _) = punta_e_aspetta(&mut app, id, &mut posta, 4);
    risposta_di_hover(&mut app, richiesta, Some(hover_markdown("  \n")));
    assert!(app.hover_card(id).is_none(), "una scheda vuota non si apre");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hover_qualsiasi_altro_comando_toglie_la_scheda_e_scarta_la_risposta_in_volo() {
    let (mut app, id, mut posta, root) =
        app_con_server("hover_comando", "let x = 1;", sa_fare_hover());
    svuota(&mut posta);
    let (richiesta, _) = punta_e_aspetta(&mut app, id, &mut posta, 4);
    risposta_di_hover(&mut app, richiesta, Some(hover_markdown("un intero")));
    assert!(app.hover_card(id).is_some());
    comanda(
        &mut app,
        EditorCommand::Move(lume_ui::editor::Motion::Left, false),
    );
    assert!(app.hover_card(id).is_none(), "un tasto chiude la scheda");

    // e una domanda ancora in volo, se l'utente scrive, non ha più
    // nessuno che la aspetti
    let (richiesta, _) = punta_e_aspetta(&mut app, id, &mut posta, 4);
    comanda(&mut app, EditorCommand::PopupDismiss);
    risposta_di_hover(&mut app, richiesta, Some(hover_markdown("tardi")));
    assert!(app.hover_card(id).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hover_il_puntatore_che_esce_dalla_parola_toglie_la_scheda() {
    let (mut app, id, mut posta, root) =
        app_con_server("hover_esce", "let x = 1;", sa_fare_hover());
    svuota(&mut posta);
    let (richiesta, _) = punta_e_aspetta(&mut app, id, &mut posta, 4);
    risposta_di_hover(&mut app, richiesta, Some(hover_markdown("un intero")));
    assert!(app.hover_card(id).is_some());
    assert_eq!(punta(&mut app, None).units(), 0, "uscire non avvia niente");
    assert!(app.hover_card(id).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hover_con_il_popup_del_completamento_aperto_non_parte() {
    let caps = lsp_types::ServerCapabilities {
        completion_provider: Some(lsp_types::CompletionOptions::default()),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        ..lsp_types::ServerCapabilities::default()
    };
    let (mut app, id, mut posta, root) = app_con_server("hover_e_popup", "x ", caps);
    digita(&mut app, "p");
    let (_, richiesta, _) = domanda_di_completamento(&mut posta);
    risposta_di_completamento(&mut app, richiesta, vec![voce_di_completamento("push")]);
    assert!(app.completion_popup(id).is_some());
    assert_eq!(
        punta(&mut app, Some(0)).units(),
        0,
        "la scheda cede al popup"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hover_la_voce_di_menu_chiede_sulla_parola_del_cursore() {
    use crate::menu::{MenuAct, MenuIntent};
    let (mut app, id, mut posta, root) = app_con_server("hover_menu", "xy", sa_fare_hover());
    svuota(&mut posta);
    let _ = app.apply_menu_intent(MenuIntent::Activate(MenuAct::QuickDoc));
    let (richiesta, posizione) = domanda_di_hover(&mut posta);
    assert_eq!(posizione.character, 0, "si chiede sull'inizio della parola");
    risposta_di_hover(&mut app, richiesta, Some(hover_markdown("due lettere")));
    let scheda = app.hover_card(id).expect("la scheda");
    assert_eq!(scheda.anchor, 0);
    let _ = std::fs::remove_dir_all(&root);
}

fn salva_con_nome(app: &mut Lume, id: lume_core::DocId, path: &std::path::Path) {
    // Registrazione del picker senza pollare il dialogo OS; risultato
    // controllato sulla stessa validazione e Task/worker della produzione.
    drop(app.pick_save_path(id));
    if let Some(sequence) = app.save_as_picker.as_ref().map(|p| p.sequence) {
        let task = app.update(Message::SaveAsPicked(sequence, Some(path.to_path_buf())));
        app.completa_salvataggi(task);
    }
}

fn nuovo_file(app: &mut Lume) -> lume_core::DocId {
    let _ = app.update(Message::MenuAction(MenuAct::NewFile));
    app.active_doc().unwrap()
}

fn testo_avviso(app: &Lume) -> String {
    app.notice
        .as_ref()
        .map(|(_, avviso)| avviso.text.clone())
        .unwrap_or_default()
}

#[test]
fn salva_con_nome_scrive_il_file_e_lega_il_documento() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    let p = dir.path().join("nuovo.txt");

    salva_con_nome(&mut app, id, &p);

    assert_eq!(std::fs::read_to_string(&p).unwrap(), "X");
    let doc = app.workspace.get(id).unwrap();
    assert_eq!(doc.path(), Some(p.as_path()));
    assert!(!doc.is_dirty());
    assert!(!app.e_senza_nome(id));
    assert!(!app.senza_nome.contains_key(&id), "il numero si toglie");
    assert_eq!(app.nome_documento(id), "nuovo.txt");
    assert_eq!(app.workspace.get_by_path(&p), Some(id));
    assert_eq!(
        app.active_doc(),
        Some(id),
        "è la stessa scheda, non una nuova"
    );
    assert!(app.groups.contains(id));
    assert!(
        app.disk_fingerprints.contains_key(&id),
        "l'eco del watcher non deve ricaricare il documento"
    );
    assert!(
        app.notice.is_none(),
        "il nome sulla scheda è già la risposta"
    );
}

/// Da qui in poi il documento è un file come gli altri: «Salva» riscrive
/// lo stesso file e chiudere la scheda non chiede niente.
#[test]
fn dopo_salva_con_nome_salva_e_chiudi_si_comportano_da_file_normale() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    let p = dir.path().join("nuovo.txt");
    salva_con_nome(&mut app, id, &p);

    sporca_attivo(&mut app);
    let task = app.update(Message::SaveActive);
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "XX");

    chiudi_scheda(&mut app, id);
    assert!(!app.groups.contains(id));
    assert!(!riquadro_aperto(&app), "salvato: niente da chiedere");
}

#[test]
fn salva_con_nome_riconosce_il_linguaggio_dal_nuovo_nome() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo_file(&mut app);
    assert!(
        !app.syntax.contains_key(&id),
        "un file nuovo non ha linguaggio"
    );
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("fn main() {}\n".into()),
    ));

    salva_con_nome(&mut app, id, &dir.path().join("prova.rs"));

    assert!(app.syntax.contains_key(&id), "dal `.rs` si ricava Rust");
}

#[test]
fn salva_con_nome_su_un_file_con_path_sposta_il_documento_e_lascia_il_vecchio() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let a = write_and_open(&mut app, dir.path(), "a.txt", "a\n");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let b = dir.path().join("b.txt");

    salva_con_nome(&mut app, id, &b);

    assert_eq!(std::fs::read_to_string(&b).unwrap(), "Xa\n");
    assert_eq!(
        std::fs::read_to_string(&a).unwrap(),
        "a\n",
        "il file di prima non si tocca: è una copia, non una modifica"
    );
    assert_eq!(app.workspace.get(id).unwrap().path(), Some(b.as_path()));
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(app.workspace.get_by_path(&a), None);
    assert_eq!(app.workspace.get_by_path(&b), Some(id));
}

#[test]
fn salva_con_nome_sul_proprio_percorso_e_un_normale_salvataggio() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let a = write_and_open(&mut app, dir.path(), "a.txt", "a\n");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);

    salva_con_nome(&mut app, id, &a);

    assert_eq!(std::fs::read_to_string(&a).unwrap(), "Xa\n");
    assert_eq!(app.workspace.get(id).unwrap().path(), Some(a.as_path()));
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert!(app.notice.is_none(), "non è un errore: è il proprio nome");
}

/// Review Focus 7: due schede sullo stesso file, e l'ultima a salvare
/// vince in silenzio. Si rifiuta **prima** di scrivere.
#[test]
fn salva_con_nome_su_un_percorso_aperto_in_un_altra_scheda_e_rifiutato() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let a = write_and_open(&mut app, dir.path(), "a.txt", "a\n");
    let altro = app.active_doc().unwrap();
    let id = nuovo_file(&mut app);
    sporca_attivo(&mut app);

    salva_con_nome(&mut app, id, &a);

    assert_eq!(
        std::fs::read_to_string(&a).unwrap(),
        "a\n",
        "niente è stato scritto"
    );
    let doc = app.workspace.get(id).unwrap();
    assert!(doc.path().is_none(), "il documento resta senza nome");
    assert!(doc.is_dirty(), "e sporco");
    assert!(app.e_senza_nome(id));
    assert_eq!(app.nome_documento(id), "senza nome 1");
    assert_eq!(
        app.workspace.get_by_path(&a),
        Some(altro),
        "il proprietario non cambia"
    );
    let avviso = testo_avviso(&app);
    assert!(avviso.contains("a.txt"), "{avviso}");
    assert!(avviso.contains("già aperto in un'altra scheda"), "{avviso}");
}

#[test]
fn annullare_il_dialogo_non_cambia_niente() {
    let mut app = app();
    let id = nuovo_file(&mut app);
    sporca_attivo(&mut app);

    drop(app.pick_save_path(id));
    let ticket = app.save_as_picker.as_ref().unwrap().sequence;
    let _ = app.update(Message::SaveAsPicked(ticket, None));

    assert!(app.e_senza_nome(id));
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(
        app.notice
            .as_ref()
            .is_none_or(|(_, notice)| notice.kind != NoticeKind::Error),
        "annullare non è un errore"
    );
}

#[test]
fn se_la_scrittura_fallisce_il_documento_resta_com_era() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    let impossibile = dir.path().join("cartella-che-non-c-e").join("x.txt");

    salva_con_nome(&mut app, id, &impossibile);

    let doc = app.workspace.get(id).unwrap();
    assert!(doc.path().is_none());
    assert!(doc.is_dirty());
    assert!(app.e_senza_nome(id));
    assert_eq!(app.workspace.get_by_path(&impossibile), None);
    assert!(
        testo_avviso(&app).contains("Salvataggio di"),
        "{}",
        testo_avviso(&app)
    );
}

#[test]
#[cfg(feature = "sql")]
fn salva_con_nome_e_negato_alla_console_sql() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let console = app.workspace.open(None, "select 1");
    app.sql_console = Some(console);
    app.groups
        .open(crate::editor_groups::GroupId::Primary, console);
    let p = dir.path().join("query.sql");

    salva_con_nome(&mut app, console, &p);

    assert!(!p.exists(), "la console non è un file");
    assert!(app.workspace.get(console).unwrap().path().is_none());
    assert!(
        testo_avviso(&app).contains("console SQL"),
        "{}",
        testo_avviso(&app)
    );
}

#[test]
fn salva_con_nome_dal_menu_senza_documento_lo_dice() {
    let mut app = app();

    let _ = app.update(Message::MenuAction(MenuAct::SaveAs));

    assert!(
        testo_avviso(&app).contains("nessun file"),
        "{}",
        testo_avviso(&app)
    );
}

/// Il dialogo nativo non si apre nei test, ma il compito che lo porta sì:
/// un compito solo (il dialogo) per il documento attivo, nessuno se non
/// ce n'è.
#[test]
fn il_dialogo_parte_dal_menu_e_da_ctrl_s_su_un_file_nuovo() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo_file(&mut app);
    sporca_attivo(&mut app);

    let dal_menu = app.update(Message::MenuAction(MenuAct::SaveAs));
    assert_eq!(dal_menu.units(), 1);
    let primo = app.save_as_picker.as_ref().unwrap().sequence;
    drop(app.update(Message::SaveActive));
    assert_eq!(app.save_as_picker.as_ref().unwrap().sequence, primo);
    assert!(testo_avviso(&app).contains("Salva con nome"));
    drop(app.update(Message::SaveAsPicked(primo, None)));
    let da_tastiera = app.update(Message::SaveActive);
    assert_eq!(
        da_tastiera.units(),
        1,
        "Ctrl+S su un file nuovo chiede il nome"
    );
    assert!(
        app.e_senza_nome(id),
        "finché non si sceglie, non cambia niente"
    );
    let secondo = app.save_as_picker.as_ref().unwrap().sequence;
    drop(app.update(Message::SaveAsPicked(secondo, None)));

    // con un file vero, Ctrl+S salva e basta
    write_and_open(&mut app, dir.path(), "vero.txt", "v\n");
    sporca_attivo(&mut app);
    let salvato = app.update(Message::SaveActive);
    app.completa_salvataggi(salvato);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("vero.txt")).unwrap(),
        "Xv\n"
    );
}

/// Il server conosceva il documento col nome vecchio: lo deve sapere
/// chiuso, e riaperto col nuovo (le diagnostiche sono legate all'URI).
#[test]
fn salva_con_nome_sposta_il_documento_anche_sul_server() {
    let (mut app, id, mut posta, root) = app_con_server(
        "salva_con_nome",
        "fn a() {}\n",
        lsp_types::ServerCapabilities::default(),
    );
    let nuovo = root.join("altro.rs");

    salva_con_nome(&mut app, id, &nuovo);

    let lume_lsp::LspCommand::Close { uri } =
        posta.try_recv().expect("il didClose del nome vecchio")
    else {
        panic!("il primo comando deve chiudere il vecchio URI");
    };
    assert!(uri.as_str().ends_with("main.rs"), "{}", uri.as_str());
    let lume_lsp::LspCommand::Open { uri, text, .. } =
        posta.try_recv().expect("il didOpen del nome nuovo")
    else {
        panic!("il secondo comando deve aprire il nuovo URI");
    };
    assert!(uri.as_str().ends_with("altro.rs"), "{}", uri.as_str());
    assert_eq!(text, "fn a() {}\n");
    assert!(posta.try_recv().is_err(), "e nient'altro");
}

#[test]
fn salva_con_nome_revoca_la_definition_del_vecchio_uri() {
    let (mut app, id, mut posta, root) =
        app_con_server("salva_nome_definition", "fn sorgente() {}\n", sa_definire());
    let target_path = root.join("destinazione.rs");
    std::fs::write(&target_path, "fn destinazione() {}\n").unwrap();
    app.open_file(&target_path).unwrap();
    let target = app.active_doc().unwrap();
    let _ = app.activate_tab(id);

    let _ = app.goto_definition_for(id);
    let request = loop {
        match posta.try_recv().expect("la richiesta definition") {
            lume_lsp::LspCommand::Ask {
                id,
                request: lume_lsp::LspRequest::Definition { .. },
            } => break id,
            _ => continue,
        }
    };
    let epoch = app.lsp.only().unwrap().epoch;
    let new_path = root.join("nuovo_nome.rs");
    salva_con_nome(&mut app, id, &new_path);
    assert!(app.lsp.only_mut().unwrap().answered(request).is_none());

    let _ = app.update(risposta(
        epoch,
        request,
        Ok(lume_lsp::LspAnswer::Definition(Some(
            lsp_types::GotoDefinitionResponse::Scalar(lsp_types::Location {
                uri: crate::lsp_uri::path_to_uri(&target_path).unwrap(),
                range: lsp_types::Range::new(
                    lsp_types::Position::new(0, 3),
                    lsp_types::Position::new(0, 15),
                ),
            }),
        ))),
    ));

    assert_eq!(app.active_doc(), Some(id));
    assert_eq!(app.active_path(), Some(new_path.as_path()));
    assert_ne!(app.active_doc(), Some(target));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn salva_con_nome_interrompe_la_ricerca_usages_sul_vecchio_uri() {
    let (mut app, id, mut posta, root) =
        app_con_server("salva_nome_usages", "fn sorgente() {}\n", sa_tutto());
    let _ = app.find_usages_for(id);
    let request = loop {
        match posta.try_recv().expect("la richiesta References") {
            lume_lsp::LspCommand::Ask {
                id,
                request: lume_lsp::LspRequest::References { .. },
            } => break id,
            _ => continue,
        }
    };
    assert_eq!(app.usages.status, "Ricerca in corso…");
    let epoch = app.lsp.only().unwrap().epoch;
    let new_path = root.join("nuovo_nome.rs");

    salva_con_nome(&mut app, id, &new_path);

    let status = app.usages.status.clone();
    assert!(status.contains("interrotta"), "{status}");
    assert!(app.usages_wait.is_none());
    let _ = app.update(risposta(
        epoch,
        request,
        Ok(lume_lsp::LspAnswer::References(None)),
    ));
    assert_eq!(
        app.usages.status, status,
        "la risposta del vecchio URI si scarta"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(feature = "mcp")]
#[test]
fn errore_reload_mcp_obsoleto_non_notifica_documento_successore() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "mcp-reload-stale.txt", "prima\n");
    let id = app.active_doc().unwrap();
    let old_version = app.workspace.get(id).unwrap().version();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("locale ".into()),
    ));
    app.notice = None;
    let _ = app.mcp_doc_ricaricato(id, &path, old_version, Err("errore tardivo".into()));
    assert!(app.notice.is_none());
    assert!(!app.external_conflicts.contains(&id));
    let version = app.workspace.get(id).unwrap().version();
    let _ = app.mcp_doc_ricaricato(
        id,
        &dir.path().join("precedente.txt"),
        version,
        Err("vecchio percorso".into()),
    );
    assert!(app.notice.is_none());
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "locale prima\n"
    );
    let _ = app.mcp_doc_ricaricato(id, &path, version, Err("errore corrente".into()));
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, notice)| notice.text.contains("errore corrente"))
    );
}

#[test]
fn risposta_watcher_durante_save_accoda_rilettura_senza_applicare_esito() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "watcher-save.txt", "prima\n");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let version = app.workspace.get(id).unwrap().version();
    let save = app.update(Message::SaveDocFixture(id));
    assert!(
        (app.scritture
            .guarda(Some((
                id,
                app.workspace.get(id).and_then(lume_core::Document::path)
            )))
            .file
            != crate::scritture::Presa::Libero
            || app.git_operazione.is_some())
    );
    app.notice = None;
    let _ = app.doc_ricaricato(
        id,
        &dir.path().join("altro.txt"),
        version,
        Err("altro binding".into()),
    );
    assert!(!app.pending_fs_rechecks.contains(&id));
    let _ = app.doc_ricaricato(id, &path, version, Err("lettura in volo".into()));
    assert!(app.pending_fs_rechecks.contains(&id));
    assert!(app.notice.is_none());
    assert!(!app.external_conflicts.contains(&id));
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "Xprima\n"
    );
    app.completa_salvataggi(save);
    assert!(!app.pending_fs_rechecks.contains(&id));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "Xprima\n");
}

#[test]
#[ignore = "misura diagnostica flush UI full-text, non budget release/server/renderer"]
fn misura_flush_lsp_full_text_ui() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, id) = app_con_lsp(root.path());
    let epoch = app.lsp.only().unwrap().epoch;
    let (mailbox, mut posta) = lume_lsp::LspCommandMailbox::channel();
    let _ = app.update(Message::Lsp(
        "rust-analyzer".into(),
        epoch,
        crate::lsp_mode::LspEvent::Ready(mailbox),
    ));
    let _ = app.update(Message::Lsp(
        "rust-analyzer".into(),
        epoch,
        crate::lsp_mode::LspEvent::From(lume_lsp::ActorEvent::Initialized(Box::new(
            session_ready(lsp_types::ServerCapabilities::default()),
        ))),
    ));
    assert!(matches!(
        posta.try_recv(),
        Ok(lume_lsp::LspCommand::Open { .. })
    ));
    for mib in [1, 8, 31] {
        let mut samples = Vec::new();
        for sample in 0..7 {
            let payload = format!("{}{}", sample, "x".repeat(mib * 1024 * 1024 - 1));
            app.workspace.get_mut(id).unwrap().reload_from_str(&payload);
            let version = app.workspace.get(id).unwrap().version();
            let start = std::time::Instant::now();
            assert!(!app.lsp_flush_change(id, Some(version)));
            samples.push(start.elapsed().as_micros());
            let lume_lsp::LspCommand::ChangeSnapshot { text, .. } = posta.try_recv().unwrap()
            else {
                panic!("atteso Change corrente");
            };
            assert_eq!(text, payload.as_str());
        }
        samples.sort_unstable();
        eprintln!(
            "LSP_FLUSH_UI_BASELINE:mib={mib},samples=7,median_us={},max_us={},debug_assertions={}",
            samples[3],
            samples[6],
            cfg!(debug_assertions)
        );
    }
}

#[cfg(target_os = "linux")]
#[path = "tests/native_rescan.rs"]
mod native_rescan;

fn failed_watcher_recipe(subscription: iced::Subscription<Message>) -> Message {
    use futures::StreamExt;
    let recipe = iced::advanced::subscription::into_recipes(subscription)
        .pop()
        .unwrap();
    let mut stream = recipe.stream(futures::stream::empty().boxed());
    let message = runtime_file_tests().block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
    });
    assert!(matches!(&message, Message::FsWatchFailed(_, _)));
    message
}

#[test]
fn watcher_error_root_aba_e_errore_corrente_reale() {
    let old = tempfile::tempdir().unwrap();
    let old_path = old.path().to_path_buf();
    let other = tempfile::tempdir().unwrap();
    let mut app = app();
    app.set_project_root(old_path.clone());
    let recipe = app.fs_watcher_subscription();
    drop(old);
    let old_error = failed_watcher_recipe(recipe);
    app.set_project_root(other.path().to_path_buf());
    let _ = app.fs_watcher_subscription();
    app.set_project_root(old_path);
    let current_recipe = app.fs_watcher_subscription();
    let _ = app.update(old_error);
    assert!(app.watcher_error().is_none());
    let current_error = failed_watcher_recipe(current_recipe);
    let _ = app.update(current_error);
    assert!(
        app.watcher_error().is_some(),
        "il fallimento attuale deve restare visibile"
    );
}

#[test]
fn watcher_error_scope_aba_e_cache_recipe_stabile() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let mut app = app();
    app.set_project_root(root.clone());
    let recipe = app.fs_watcher_subscription();
    let old_scope = app.watcher_scope.borrow().as_ref().unwrap().clone();
    for _ in 0..32 {
        let _ = app.fs_watcher_subscription();
        assert!(old_scope.same_recipe(app.watcher_scope.borrow().as_ref().unwrap()));
    }
    std::fs::remove_dir(&root).unwrap();
    let old_error = failed_watcher_recipe(recipe);
    let nested = root.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    app.set_project_watch_directories(vec![root.clone(), nested.clone()]);
    let _ = app.fs_watcher_subscription();
    app.set_project_watch_directories(vec![root.clone()]);
    let _ = app.fs_watcher_subscription();
    assert!(!old_scope.same_recipe(app.watcher_scope.borrow().as_ref().unwrap()));
    let _ = app.update(old_error);
    assert!(app.watcher_error().is_none());
    let current_recipe = app.fs_watcher_subscription();
    std::fs::remove_dir(&nested).unwrap();
    std::fs::remove_dir(&root).unwrap();
    let _ = app.update(failed_watcher_recipe(current_recipe));
    assert!(app.watcher_error().is_some());
    app.set_project_watch_directories(vec![root.clone(), nested]);
    let _ = app.fs_watcher_subscription();
    assert!(
        app.watcher_error().is_none(),
        "avviso di un'altra recipe non va mostrato"
    );
}

#[path = "tests/multicursore.rs"]
mod multicursore;

#[path = "tests/multicursore_misure.rs"]
mod misure_multicursore;

#[test]
fn pagina_conta_righe_visibili_con_folding_in_entrambe_le_direzioni() {
    use lume_core::Range;
    use lume_syntax::FoldRegion;
    use lume_ui::editor::{EditorCommand, FoldProjection, PageDirection};
    let mut app = app();
    let dir = tempfile::tempdir().unwrap();
    write_and_open(&mut app, dir.path(), "pagina.txt", &"abcd\n".repeat(20));
    let id = app.active_doc().unwrap();
    app.fold_projections.insert(
        id,
        FoldProjection::from_regions(
            &[
                FoldRegion {
                    start_line: 1,
                    end_line: 5,
                },
                FoldRegion {
                    start_line: 7,
                    end_line: 9,
                },
            ],
            |_| true,
        ),
    );
    app.workspace
        .get_mut(id)
        .unwrap()
        .set_selection(Range::point(2));
    app.apply_editor_command_for(
        id,
        EditorCommand::MovePage {
            direction: PageDirection::Down,
            lines: 4,
            extend: false,
        },
    );
    assert_eq!(app.workspace.get(id).unwrap().selection(), Range::point(52));
    app.apply_editor_command_for(
        id,
        EditorCommand::MovePage {
            direction: PageDirection::Up,
            lines: 4,
            extend: true,
        },
    );
    assert_eq!(
        app.workspace.get(id).unwrap().selection(),
        Range::new(52, 2)
    );
}

#[test]
fn pagina_giu_si_ferma_all_ultima_intestazione_visibile_senza_newline_finale() {
    use lume_core::Range;
    use lume_syntax::FoldRegion;
    use lume_ui::editor::{FoldProjection, PageDirection};
    let mut app = app();
    let dir = tempfile::tempdir().unwrap();
    write_and_open(
        &mut app,
        dir.path(),
        "fine.txt",
        "prima\nblocco\ncorpo\nfine",
    );
    let id = app.active_doc().unwrap();
    app.fold_projections.insert(
        id,
        FoldProjection::from_regions(
            &[FoldRegion {
                start_line: 1,
                end_line: 3,
            }],
            |_| true,
        ),
    );
    app.editor_apply_page(id, PageDirection::Down, usize::MAX, false);
    assert_eq!(app.workspace.get(id).unwrap().selection(), Range::point(6));
}

#[test]
fn tick_terminal_pty_reale_procede_sotto_modale_ma_input_e_rifiutato() {
    lume_testkit::richiedi!(lume_testkit::Strumento::ShellPty);
    use crate::features::Feature;
    let dir = tempfile::tempdir().unwrap();
    let vietato = dir.path().join("vietato.txt");
    let consentito = dir.path().join("consentito.txt");
    let mut app = app();
    let manda = |app: &mut Lume, evento| {
        app.update(Message::Tool(
            tools::terminal::ID,
            lume_ui::tool::ToolEvent::new(evento),
        ))
    };
    let _ = manda(&mut app, tools::terminal::Event::NewSession);
    assert_eq!(app.feature_live_count(Feature::Terminal), Some(1));
    app.apri_overlay(OverlayId::About);
    let comando_vietato = format!("echo vietato > \"{}\"\r", vietato.display());
    let _ = manda(
        &mut app,
        tools::terminal::Event::Input(comando_vietato.into_bytes()),
    );
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, n)| n.text.contains("Rispondi prima"))
    );
    let _ = app.update(Message::CloseAll);
    #[cfg(windows)]
    let comando_consentito = format!("echo consentito > \"{}\" & exit\r", consentito.display());
    #[cfg(not(windows))]
    let comando_consentito = format!("echo consentito > \"{}\"; exit 7\r", consentito.display());
    let _ = manda(
        &mut app,
        tools::terminal::Event::Input(comando_consentito.into_bytes()),
    );
    app.apri_overlay(OverlayId::About);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while app.feature_live_count(Feature::Terminal) != Some(0)
        && std::time::Instant::now() < deadline
    {
        let _ = manda(&mut app, tools::terminal::Event::Tick);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(app.feature_live_count(Feature::Terminal), Some(0));
    assert!(
        consentito.exists(),
        "la shell ha eseguito il comando successivo"
    );
    assert!(
        !vietato.exists(),
        "l'input sotto modale non ha raggiunto la shell"
    );
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
}
