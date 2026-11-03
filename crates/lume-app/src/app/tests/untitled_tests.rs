//! Task14: documenti veri, Task di salvataggio e filesystem posseduto.
use super::*;

#[test]
fn nuovo_file_non_lascia_struttura_del_documento_precedente() {
    use crate::tools::structure::{self, Event, Row, Snapshot};
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "precedente.txt", "prima");
    app.registry.active_panel = Some(structure::ID);
    // Snapshot controllato nel tool reale, non un server LSP simulato.
    let tool = app.registry.get_mut(structure::ID).unwrap();
    tool.update(&lume_ui::tool::ToolEvent::new(Event::Replace(Snapshot {
        path: Some(path.clone()),
        rows: vec![Row {
            name: "precedente".into(),
            detail: None,
            kind: "fn",
            depth: 0,
            line: 1,
        }],
        empty_reason: "",
    })));
    assert!(
        matches!(&tool.update(&lume_ui::tool::ToolEvent::new(Event::Open(0)))[..],
        [lume_ui::tool::ToolRequest::OpenFileAt { path: old, .. }] if old == &path)
    );
    let nuovo = nuovo_file(&mut app);
    assert!(app.e_senza_nome(nuovo));
    assert!(
        app.registry
            .get_mut(structure::ID)
            .unwrap()
            .update(&lume_ui::tool::ToolEvent::new(Event::Open(0)))
            .is_empty(),
        "Struttura non deve inviare navigazione al file precedente"
    );
}

fn nuovo_file(app: &mut Lume) -> lume_core::DocId {
    let _ = app.update(Message::MenuAction(MenuAct::NewFile));
    app.active_doc().unwrap()
}

#[test]
fn nuovo_file_apre_una_scheda_vuota_senza_nome_e_la_attiva() {
    let mut app = app();
    let id = nuovo_file(&mut app);
    assert!(app.groups.contains(id));
    assert!(app.e_senza_nome(id));
    assert_eq!(app.nome_documento(id), "senza nome 1");
    let doc = app.workspace.get(id).unwrap();
    assert!(doc.path().is_none());
    assert_eq!(doc.rope().to_string(), "");
    assert!(!doc.is_dirty());
    assert_eq!(app.editor_tab_name(id).as_deref(), Some("senza nome 1"));
    assert_eq!(app.editor_kind(id), crate::editor_groups::EditorKind::Code);
}

#[test]
fn i_numeri_dei_file_nuovi_non_si_riusano_e_non_scorrono() {
    let mut app = app();
    let primo = nuovo_file(&mut app);
    let secondo = nuovo_file(&mut app);
    assert_ne!(primo, secondo);
    assert_eq!(app.nome_documento(primo), "senza nome 1");
    assert_eq!(app.nome_documento(secondo), "senza nome 2");
    chiudi_scheda(&mut app, primo);
    assert_eq!(app.nome_documento(secondo), "senza nome 2");
    let terzo = nuovo_file(&mut app);
    assert_eq!(app.nome_documento(terzo), "senza nome 3");
    assert!(!app.senza_nome.contains_key(&primo));
}

#[test]
#[cfg(feature = "sql")]
fn la_console_sql_non_e_un_file_nuovo() {
    let mut app = app();
    let console = app.workspace.open(None, "select 1");
    app.sql_console = Some(console);
    assert!(!app.e_senza_nome(console));
    assert!(!app.e_senza_nome(lume_core::DocId(9_999)));
    let dir = tempfile::tempdir().unwrap();
    write_and_open(&mut app, dir.path(), "file.txt", "x\n");
    assert!(!app.e_senza_nome(app.active_doc().unwrap()));
}

#[test]
fn un_file_nuovo_pulito_si_chiude_subito() {
    let mut app = app();
    let id = nuovo_file(&mut app);
    chiudi_scheda(&mut app, id);
    assert!(!app.groups.contains(id));
    assert!(app.workspace.get(id).is_none());
    assert!(!riquadro_aperto(&app));
    assert!(!app.senza_nome.contains_key(&id));
}

#[test]
fn un_file_nuovo_sporco_chiede_prima_di_chiudersi() {
    let mut app = app();
    let id = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    chiudi_scheda(&mut app, id);
    assert!(app.groups.contains(id));
    assert!(riquadro_aperto(&app));
    assert_eq!(
        app.riquadro_chiusura().unwrap().titolo,
        "Salvare le modifiche a senza nome 1?"
    );
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "X");
}

#[test]
fn salva_su_un_file_nuovo_non_lo_chiude_e_lo_dice() {
    let mut app = app();
    let id = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    chiudi_scheda(&mut app, id);
    assert!(riquadro_aperto(&app));
    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Salva);
    assert!(app.groups.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "X");
    assert!(riquadro_aperto(&app));
    let errore = app.riquadro_chiusura().unwrap().errore.unwrap();
    assert!(errore.contains("senza nome 1"), "{errore}");
    assert!(errore.contains("non ha ancora un file"), "{errore}");
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    rispondi(
        &mut app,
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    );
    assert!(!app.groups.contains(id));
    assert!(app.workspace.get(id).is_none());
    assert!(!app.senza_nome.contains_key(&id));
}

#[test]
#[cfg(feature = "sql")]
fn write_doc_su_un_file_nuovo_pulito_e_sulla_console_non_e_un_errore() {
    let mut app = app();
    let nuovo = nuovo_file(&mut app);
    assert!(app.write_doc(nuovo).is_ok());
    let console = app.workspace.open(None, "select 1");
    app.sql_console = Some(console);
    app.groups
        .open(crate::editor_groups::GroupId::Primary, console);
    app.groups.activate(console);
    sporca_attivo(&mut app);
    assert!(app.workspace.get(console).unwrap().is_dirty());
    assert!(!app.chiede_salvataggio(console));
    assert!(app.write_doc(console).is_ok());
}

#[test]
fn la_chiusura_della_finestra_conta_i_file_nuovi_sporchi() {
    let mut app = app();
    nuovo_file(&mut app);
    assert_eq!(app.richiesta_uscita(None), super::super::Uscita::Esci);
    sporca_attivo(&mut app);
    let _ = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
    assert!(riquadro_aperto(&app));
    let riquadro = app.riquadro_chiusura().unwrap();
    assert_eq!(riquadro.titolo, "1 file con modifiche non salvate");
    assert_eq!(riquadro.nomi, vec!["senza nome 1".to_owned()]);
}

#[test]
fn esci_dal_menu_chiede_se_c_e_qualcosa_da_perdere() {
    let mut app = app();
    nuovo_file(&mut app);
    sporca_attivo(&mut app);
    let _ = app.update(Message::MenuAction(MenuAct::Exit));
    assert!(riquadro_aperto(&app));
    assert_eq!(
        app.riquadro_chiusura().unwrap().titolo,
        "1 file con modifiche non salvate"
    );
    assert!(matches!(
        app.chiusura.as_ref().unwrap().oggetto,
        super::super::DaChiudere::Finestra(None)
    ));
}

#[test]
fn salva_tutto_salta_i_file_nuovi_e_lo_dice() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "a.txt", "a\n");
    sporca_attivo(&mut app);
    let nuovo = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    let task = app.update(Message::MenuAction(MenuAct::SaveAll));
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "a\n",
        "nessun I/O prima del poll"
    );
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "Xa\n");
    assert!(app.workspace.get(nuovo).unwrap().is_dirty());
    let (_, avviso) = app.notice.as_ref().unwrap();
    assert_eq!(avviso.kind, NoticeKind::Info);
    assert!(avviso.text.contains("1 file nuovo"), "{}", avviso.text);
}

#[test]
fn salva_tutto_senza_file_nuovi_non_avvisa_di_niente() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "a.txt", "a\n");
    sporca_attivo(&mut app);
    let task = app.update(Message::MenuAction(MenuAct::SaveAll));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(p).unwrap(), "Xa\n");
    assert!(app.notice.is_none());
}

#[test]
fn nuovo_file_nel_gruppo_col_focus_non_sostituisce_il_primario() {
    use crate::editor_groups::GroupId;
    let mut app = app();
    let primo = nuovo_file(&mut app);
    let secondo = nuovo_file(&mut app);
    let _ = app.update(Message::MenuAction(MenuAct::SplitEditor));
    assert_eq!(app.groups.active_in(GroupId::Secondary), Some(secondo));
    let terzo = nuovo_file(&mut app);
    assert_eq!(app.groups.focus(), GroupId::Secondary);
    assert_eq!(app.groups.active_in(GroupId::Primary), Some(primo));
    assert_eq!(app.groups.active_in(GroupId::Secondary), Some(terzo));
    let _ = app.update(Message::EditorIn(
        GroupId::Secondary,
        EditorCommand::Insert("destra".into()),
    ));
    assert_eq!(
        app.workspace.get(terzo).unwrap().rope().to_string(),
        "destra"
    );
    assert_eq!(app.workspace.get(primo).unwrap().rope().to_string(), "");
    chiudi_scheda(&mut app, terzo);
    assert!(riquadro_aperto(&app));
}

#[test]
fn nuovo_file_rifiuta_overflow_senza_creare_documenti_o_cambiare_focus() {
    let mut app = app();
    let id = nuovo_file(&mut app);
    app.senza_nome_seq = u32::MAX - 1;
    let ultimo = nuovo_file(&mut app);
    assert_eq!(
        app.nome_documento(ultimo),
        format!("senza nome {}", u32::MAX)
    );
    let ids = app.groups.all().collect::<Vec<_>>();
    let count = app.workspace.ids().count();
    let _ = app.update(Message::MenuAction(MenuAct::NewFile));
    assert_eq!(app.groups.all().collect::<Vec<_>>(), ids);
    assert_eq!(app.workspace.ids().count(), count);
    assert_eq!(app.active_doc(), Some(ultimo));
    assert_eq!(app.senza_nome_seq, u32::MAX);
    assert_eq!(app.nome_documento(id), "senza nome 1");
    assert_eq!(app.notice.as_ref().unwrap().1.kind, NoticeKind::Error);
}

#[test]
fn salva_attivo_pathless_sporco_occupa_il_coordinatore_senza_ticket() {
    let mut app = app();
    let id = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    let epoch = app.scritture.guarda(None).epoca;
    let version = app.workspace.get(id).unwrap().version();
    let task = app.update(Message::MenuAction(MenuAct::Save));
    assert_eq!(task.units(), 1, "il comando apre il dialogo nativo");
    assert!((app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert_eq!(
        app.scritture.guarda(None).in_volo,
        Some(crate::scritture::Tipo::SalvaConNome)
    );
    assert_eq!(app.scritture.guarda(None).epoca, epoch + 1);
    assert_eq!(app.saves.active_document(), None);
    assert_eq!(app.workspace.get(id).unwrap().version(), version);
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(app.notice.is_none(), "il dialogo non è un errore");
    let sequenza = app.save_as_picker.as_ref().unwrap().sequence;
    drop(app.update(Message::SaveAsPicked(sequenza, None)));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn prepare_save_pathless_non_registrato_non_e_console_e_non_perde_testo() {
    let mut app = app();
    let id = app.workspace.open(None, "");
    app.groups.open(crate::editor_groups::GroupId::Primary, id);
    sporca_attivo(&mut app);
    assert!(app.e_senza_nome(id));
    let version = app.workspace.get(id).unwrap().version();
    assert!(
        app.prepare_save(id, version)
            .err()
            .unwrap()
            .contains("non ha ancora un file")
    );
    assert!(
        app.prepare_save(id, version - 1)
            .err()
            .unwrap()
            .contains("non ha ancora un file")
    );
    assert!(app.chiede_salvataggio(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "X");
    #[cfg(feature = "sql")]
    {
        app.sql_console = Some(id);
        assert!(app.prepare_save(id, version).unwrap().is_none());
        assert!(!app.e_senza_nome(id));
        app.sql_console = None;
    }
    assert!(app.chiede_salvataggio(id));
}

#[test]
fn nuovo_file_annulla_e_risposta_scaduta_non_scarta_modifiche() {
    let mut app = app();
    let id = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    chiudi_scheda(&mut app, id);
    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Annulla);
    assert!(!riquadro_aperto(&app));
    chiudi_scheda(&mut app, id);
    app.apri_overlay(crate::overlay_manager::OverlayId::GotoLine);
    assert!(riquadro_aperto(&app));
    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Annulla);
    rispondi(
        &mut app,
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    );
    assert!(app.groups.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "X");
    assert!(app.senza_nome.contains_key(&id));
}

#[test]
fn exit_salva_tiene_domanda_e_errore_pathless_anche_alla_seconda_richiesta() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "ok.txt", "ok");
    sporca_attivo(&mut app);
    let id = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    let _ = app.update(Message::MenuAction(MenuAct::Exit));
    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Salva);
    assert_eq!(std::fs::read_to_string(p).unwrap(), "Xok");
    assert!(riquadro_aperto(&app));
    let riquadro = app.riquadro_chiusura().unwrap();
    assert_eq!(riquadro.nomi, vec!["senza nome 1".to_owned()]);
    let error = riquadro.errore.unwrap();
    assert!(error.contains("non ha ancora un file"));
    let _ = app.update(Message::MenuAction(MenuAct::Exit));
    assert_eq!(
        app.riquadro_chiusura().unwrap().errore.as_deref(),
        Some(error.as_str())
    );
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "X");
    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Annulla);
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn salva_tutto_conta_solo_nuovi_sporchi_non_console_o_puliti() {
    let mut app = app();
    let a = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    let b = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    nuovo_file(&mut app);
    #[cfg(feature = "sql")]
    app.open_sql_console(None, None);
    sporca_attivo(&mut app);
    let task = app.update(Message::MenuAction(MenuAct::SaveAll));
    app.completa_salvataggi(task);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    #[cfg(feature = "sql")]
    assert!(app.notice.as_ref().unwrap().1.text.contains("2 file nuovi"));
    #[cfg(not(feature = "sql"))]
    assert!(app.notice.as_ref().unwrap().1.text.contains("3 file nuovi"));
    assert!(app.workspace.get(a).unwrap().is_dirty());
    assert!(app.workspace.get(b).unwrap().is_dirty());
}

#[test]
fn pathless_non_persistiti_nei_due_gruppi_della_sessione() {
    use crate::editor_groups::GroupId;
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let p = write_and_open(&mut app, dir.path(), "persistito.txt", "ok");
    nuovo_file(&mut app);
    sporca_attivo(&mut app);
    let destro = nuovo_file(&mut app);
    let _ = app.update(Message::MenuAction(MenuAct::SplitEditor));
    assert_eq!(app.groups.active_in(GroupId::Secondary), Some(destro));
    let snap = app.session_snapshot();
    assert_eq!(snap.tabs, vec![p]);
    assert!(snap.active_tab.is_none());
    let secondary = snap.secondary_group.unwrap();
    assert!(secondary.tabs.is_empty());
    assert!(secondary.active.is_none());
    assert!(snap.sql_console.is_none());
}

#[test]
fn undo_del_file_nuovo_lo_rende_pulito_e_chiudibile_senza_domanda() {
    let mut app = app();
    let id = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    let _ = app.update(Message::MenuAction(MenuAct::Undo));
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    chiudi_scheda(&mut app, id);
    assert!(!riquadro_aperto(&app));
    assert!(app.workspace.get(id).is_none());
}

#[test]
fn salva_tutto_errore_reale_prevale_sull_avviso_dei_file_nuovi() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "scomparso.txt", "prima");
    let file = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let nuovo = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    let task = app.update(Message::MenuAction(MenuAct::SaveAll));
    assert_eq!(app.notice.as_ref().unwrap().1.kind, NoticeKind::Info);
    app.completa_salvataggi(task);
    let notice = &app.notice.as_ref().unwrap().1;
    assert_eq!(notice.kind, NoticeKind::Error);
    assert!(notice.text.contains("scomparso.txt"));
    assert!(app.workspace.get(file).unwrap().is_dirty());
    assert!(app.workspace.get(nuovo).unwrap().is_dirty());
    assert!(path.is_dir());
}

#[test]
fn salva_tutto_con_soli_nuovi_puliti_non_avvisa() {
    let mut app = app();
    nuovo_file(&mut app);
    nuovo_file(&mut app);
    let task = app.update(Message::MenuAction(MenuAct::SaveAll));
    app.completa_salvataggi(task);
    assert!(app.notice.is_none());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn esci_con_nuovo_sporco_revoca_save_involo_prima_del_commit() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "started.txt", "prima");
    let file = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let task = app.update(Message::SaveActive);
    let mut stream = iced_runtime::task::into_stream(task).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(ready)) = runtime
        .block_on(futures::StreamExt::next(&mut stream))
        .unwrap()
    else {
        panic!("il worker deve attendere l'autorizzazione prima del commit");
    };
    let pause = app.save_job.as_ref().unwrap().pause().unwrap();
    let _ = app.update(Message::SaveReady(ready));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "prima");
    let nuovo = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    let _ = app.update(Message::MenuAction(MenuAct::Exit));
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::Salva,
    ));
    assert!(
        (app.scritture.contiene_salvataggio(nuovo) || app.saves.active_document() == Some(nuovo)),
        "Salva mette in coda anche il pathless"
    );
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    ));
    assert!(
        !(app.scritture.contiene_salvataggio(nuovo) || app.saves.active_document() == Some(nuovo)),
        "pending revocato prima di prepararlo"
    );
    assert!(
        (app.scritture.contiene_salvataggio(file) || app.saves.active_document() == Some(file)),
        "il save in corso va raccolto"
    );
    assert!(riquadro_aperto(&app));
    drop(pause);
    let iced_runtime::Action::Output(completion) = runtime
        .block_on(futures::StreamExt::next(&mut stream))
        .unwrap()
    else {
        panic!("la completion del worker deve essere consegnata");
    };
    let _ = app.update(completion);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(!riquadro_aperto(&app));
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "prima",
        "il gate nega il commit prima di CommitStarted"
    );
    assert_eq!(app.workspace.get(nuovo).unwrap().rope().to_string(), "X");
}

#[test]
fn pathless_sporco_senza_scheda_viene_contato_da_exit() {
    let mut app = app();
    let id = nuovo_file(&mut app);
    sporca_attivo(&mut app);
    app.groups.close(id);
    let _ = app.update(Message::MenuAction(MenuAct::Exit));
    assert!(riquadro_aperto(&app));
    assert_eq!(
        app.riquadro_chiusura().unwrap().nomi,
        vec!["senza nome 1".to_owned()]
    );
}

#[test]
#[cfg(feature = "sql")]
fn la_console_e_esclusa_per_identita_anche_se_ha_un_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "non-console.txt", "prima");
    let id = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    app.sql_console = Some(id);
    assert!(!app.chiede_salvataggio(id));
    assert!(!app.e_senza_nome(id));
    let task = app.update(Message::SaveActive);
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "prima");
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(app.notice.is_none());
}

#[test]
fn tavolozza_nuovo_file_ed_esci_usano_le_azioni_reali() {
    let mut app = app();
    let _ = app.open_search(SearchTab::Azioni);
    let index = app
        .global_search
        .entries()
        .iter()
        .position(|e| e.act == Some(MenuAct::NewFile))
        .unwrap();
    let _ = app.activate_search_entry(index);
    let id = app.active_doc().unwrap();
    assert!(app.e_senza_nome(id));
    assert_eq!(app.nome_documento(id), "senza nome 1");
    sporca_attivo(&mut app);
    let _ = app.open_search(SearchTab::Azioni);
    let index = app
        .global_search
        .entries()
        .iter()
        .position(|e| e.act == Some(MenuAct::Exit))
        .unwrap();
    let _ = app.activate_search_entry(index);
    assert!(riquadro_aperto(&app));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "X");
}
