//! Operazioni sull'albero: creare un file o una cartella dal menu contestuale.
use super::*;
#[cfg(target_os = "linux")]
use crate::app::DeferredRestoredAction;
use crate::context_menu::Soggetto;
use crate::menu::MenuAct;
#[cfg(target_os = "linux")]
use iced::Task;
use lume_ui::tool::ToolEvent;

/// Isola il Cestino nativo in un processo con XDG_DATA_HOME privato.
#[cfg(target_os = "linux")]
fn cestino_isolato(nome_test: &str) -> Option<std::path::PathBuf> {
    let variabile = "LUME_TEST_PASSO4_CESTINO";
    if let Some(base) = std::env::var_os(variabile) {
        return Some(base.into());
    }
    let base = tempfile::tempdir().unwrap();
    let esito = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", nome_test, "--test-threads=1", "--nocapture"])
        .env(variabile, base.path())
        .env("XDG_DATA_HOME", base.path().join("xdg"))
        .status()
        .unwrap();
    assert!(esito.success(), "test isolato: {esito}");
    None
}

#[cfg(target_os = "linux")]
fn prepara_cestino(
    base: &std::path::Path,
) -> (
    Lume,
    std::path::PathBuf,
    lume_core::DocId,
    lume_core::DocId,
    Task<Message>,
) {
    let root = base.join("progetto");
    let folder = root.join("moduli");
    std::fs::create_dir_all(&folder).unwrap();
    let toccato = folder.join("aperto.txt");
    let estraneo = root.join("altro.txt");
    std::fs::write(&toccato, "prima").unwrap();
    std::fs::write(&estraneo, "altro").unwrap();
    let mut app = app();
    app.set_project_root(root);
    let doc = app.workspace.open(Some(toccato), "prima");
    let altro = app.workspace.open(Some(estraneo), "altro");
    app.groups.open(crate::editor_groups::GroupId::Primary, doc);
    app.groups
        .open(crate::editor_groups::GroupId::Primary, altro);
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let task = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    assert!(app.albero_elimina.as_ref().unwrap().in_corso);
    (app, folder, doc, altro, task)
}

// Per i casi con una sola ricevuta attesa, restituisce il Task successivo
// al chiamante; consuma_task_cestino invece esaurisce tutta la catena.
#[cfg(target_os = "linux")]
fn consegna_cestino_reale(app: &mut Lume, task: Task<Message>) -> Task<Message> {
    let messaggio = primo_output(task);
    assert!(matches!(messaggio, Message::AlberoEliminato(..)));
    app.update(messaggio)
}

/// Consuma tutti i Task e consegna ogni output, incluse eventuali ricevute I/O.
#[cfg(target_os = "linux")]
fn consuma_task_cestino(app: &mut Lume, task: Task<Message>) -> usize {
    use futures::StreamExt;
    let mut task_in_attesa = std::collections::VecDeque::from([task]);
    let mut ricevute = 0;
    while let Some(task) = task_in_attesa.pop_front() {
        let Some(stream) = iced_runtime::task::into_stream(task) else {
            continue;
        };
        let messaggi = runtime_file_tests().block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(15), async {
                stream
                    .filter_map(|azione| async move {
                        match azione {
                            iced_runtime::Action::Output(message) => Some(message),
                            _ => None,
                        }
                    })
                    .collect::<Vec<_>>()
                    .await
            })
            .await
            .expect("il Task deve terminare")
        });
        for message in messaggi {
            if matches!(message, Message::AlberoEliminato(..)) {
                ricevute += 1;
            }
            task_in_attesa.push_back(app.update(message));
        }
    }
    ricevute
}

#[cfg(target_os = "linux")]
fn conferma_file_sostituita(
    base: &std::path::Path,
) -> (Lume, std::path::PathBuf, lume_core::DocId) {
    let root = base.join("progetto");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("file.txt");
    std::fs::write(&file, "contenuto\n").unwrap();
    let mut app = app();
    app.set_project_root(root);
    let doc = app.workspace.open(Some(file.clone()), "contenuto\n");
    app.groups.open(crate::editor_groups::GroupId::Primary, doc);
    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConfermaElimina));
    // La vista può scomparire mentre il modello è ancora posseduto dal dominio.
    assert!(app.chiudi_overlay_se(OverlayId::ConfermaElimina));
    assert_eq!(app.shell.active_overlay(), None);
    (app, file, doc)
}

#[cfg(target_os = "linux")]
#[test]
fn elimina_tardiva_dopo_sostituzione_non_avvia_il_cestino() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::elimina_tardiva_dopo_sostituzione_non_avvia_il_cestino",
    ) else {
        return;
    };
    let (mut app, file, doc) = conferma_file_sostituita(&base);
    let richiesta = app.albero_elimina.clone().unwrap();
    let task = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    let avviso = app
        .notice
        .as_ref()
        .map(|(_, notice)| (notice.kind, notice.text.clone()));
    let stato_dopo_risposta = app.albero_elimina.clone();
    let scrittura_dopo_risposta = app.scritture.guarda(None).in_volo;
    let ricevute = consuma_task_cestino(&mut app, task);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "contenuto\n");
    assert!(app.groups.contains(doc));
    assert_eq!(
        app.workspace.get(doc).unwrap().rope().to_string(),
        "contenuto\n"
    );
    assert_eq!(ricevute, 0, "nessun worker Cestino deve partire");
    assert_eq!(stato_dopo_risposta.as_ref(), Some(&richiesta));
    assert_eq!(app.albero_elimina.as_ref(), Some(&richiesta));
    assert_eq!(scrittura_dopo_risposta, None);
    assert_eq!(app.scritture.guarda(None).in_volo, None);
    assert!(
        matches!(avviso, Some((NoticeKind::Info, testo)) if testo.contains("non è più visibile"))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn annulla_tardiva_dopo_sostituzione_conserva_la_domanda() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::annulla_tardiva_dopo_sostituzione_conserva_la_domanda",
    ) else {
        return;
    };
    let (mut app, file, doc) = conferma_file_sostituita(&base);
    let richiesta = app.albero_elimina.clone().unwrap();
    let task = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Annulla,
    ));
    let avviso = app
        .notice
        .as_ref()
        .map(|(_, notice)| (notice.kind, notice.text.clone()));
    let ricevute = consuma_task_cestino(&mut app, task);
    assert_eq!(app.albero_elimina.as_ref(), Some(&richiesta));
    assert_eq!(ricevute, 0);
    assert_eq!(app.scritture.guarda(None).in_volo, None);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "contenuto\n");
    assert!(app.groups.contains(doc));
    assert_eq!(
        app.workspace.get(doc).unwrap().rope().to_string(),
        "contenuto\n"
    );
    assert!(
        matches!(avviso, Some((NoticeKind::Info, testo)) if testo.contains("non è più visibile"))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn risposte_tardive_durante_cestino_conservano_lease_e_ricevuta_reale() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::risposte_tardive_durante_cestino_conservano_lease_e_ricevuta_reale",
    ) else {
        return;
    };
    let (mut app, folder, doc, _, task_originale) = prepara_cestino(&base);
    let richiesta = app.albero_elimina.clone().unwrap();
    let lease = richiesta.lease.unwrap();
    let _ = app.update(Message::MenuAction(MenuAct::GotoLine));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConfermaElimina));
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("Rispondi prima")
    );
    // Esc nasconde la vista ma non revoca il lease del worker reale.
    let _ = app.update(Message::CloseAll);
    assert_eq!(app.shell.active_overlay(), None);
    for risposta in [
        crate::conferma_elimina::ConfermaMessage::Elimina,
        crate::conferma_elimina::ConfermaMessage::Annulla,
    ] {
        let task = app.update(Message::AlberoElimina(risposta));
        assert!(
            matches!(app.notice.as_ref(), Some((_, avviso)) if avviso.kind == NoticeKind::Info && avviso.text.contains("già avviato"))
        );
        assert_eq!(consuma_task_cestino(&mut app, task), 0);
        assert_eq!(app.albero_elimina.as_ref(), Some(&richiesta));
        assert!(
            app.scritture
                .possiede(lease, crate::scritture::Tipo::Cestino)
        );
        assert!(folder.exists());
        assert!(app.groups.contains(doc));
    }
    assert_eq!(consuma_task_cestino(&mut app, task_originale), 1);
    assert!(!folder.exists());
    assert!(!app.groups.contains(doc));
    assert!(
        !app.scritture
            .possiede(lease, crate::scritture::Tipo::Cestino)
    );
    assert!(app.albero_elimina.is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn cestino_blocca_solo_input_del_discendente_e_l_altro_editor_resta_usabile() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::cestino_blocca_solo_input_del_discendente_e_l_altro_editor_resta_usabile",
    ) else {
        return;
    };
    let (mut app, folder, doc, altro, task) = prepara_cestino(&base);
    assert!(app.albero_elimina.as_ref().unwrap().in_corso);
    let richiesta = app.albero_elimina.clone().unwrap();
    let lease = richiesta.lease.unwrap();
    assert!(
        app.scritture
            .possiede(lease, crate::scritture::Tipo::Cestino)
    );
    let _ = app.update(Message::CloseAll);
    assert!(
        app.albero_elimina.as_ref().unwrap().in_corso,
        "Esc nasconde il riquadro senza cancellare il worker"
    );
    assert_eq!(app.albero_elimina.as_ref(), Some(&richiesta));
    assert!(
        app.scritture
            .possiede(lease, crate::scritture::Tipo::Cestino)
    );
    assert_eq!(app.shell.active_overlay(), None);
    let root = app.project_root.clone();
    let _ = app.update(Message::FolderPicked(Some(base.join("altra-radice"))));
    assert_eq!(app.project_root, root);
    assert!(app.notice.as_ref().unwrap().1.text.contains("Cestino"));
    let _ = app.activate_tab(doc);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("vietato".into()),
    ));
    assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), "prima");
    assert!(app.notice.is_some(), "l'input rifiutato è avvisato");
    let _ = app.activate_tab(altro);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("x".into()),
    ));
    assert_eq!(
        app.workspace.get(altro).unwrap().rope().to_string(),
        "xaltro"
    );
    let _ = consegna_cestino_reale(&mut app, task);
    assert!(!folder.exists());
    assert!(!app.groups.contains(doc));
    assert_eq!(
        app.workspace.get(altro).unwrap().rope().to_string(),
        "xaltro"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn cestino_lascia_passare_clipboard_sull_editor_estraneo() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::cestino_lascia_passare_clipboard_sull_editor_estraneo",
    ) else {
        return;
    };
    let (mut app, folder, doc, altro, task) = prepara_cestino(&base);
    let _ = app.update(Message::CloseAll);
    let _ = app.activate_tab(doc);
    let _ = app.update(Message::ClipboardPasted(Some("vietato".into())));
    assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), "prima");
    let _ = app.activate_tab(altro);
    let _ = app.update(Message::ClipboardPasted(Some("incolla".into())));
    assert_eq!(
        app.workspace.get(altro).unwrap().rope().to_string(),
        "incollaaltro"
    );
    let _ = consegna_cestino_reale(&mut app, task);
    assert!(!folder.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn cestino_rifiuta_salvataggi_senza_coda_ne_resurrezione() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::cestino_rifiuta_salvataggi_senza_coda_ne_resurrezione",
    ) else {
        return;
    };
    let (mut app, folder, doc, altro, task) = prepara_cestino(&base);
    let _ = app.update(Message::CloseAll);
    let _ = app.activate_tab(altro);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("nuovo".into()),
    ));
    assert!(app.workspace.get(altro).unwrap().is_dirty());
    let _ = app.update(Message::SaveActive);
    assert!(app.notice.as_ref().unwrap().1.text.contains("Cestino"));
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
    let _ = app.activate_tab(doc);
    let _ = app.update(Message::EditorIn(app.groups.focus(), EditorCommand::Save));
    assert!(app.notice.as_ref().unwrap().1.text.contains("Cestino"));
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
    let _ = consegna_cestino_reale(&mut app, task);
    assert!(!folder.exists());
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
    assert!(!app.groups.contains(doc));
    assert!(app.groups.contains(altro));
    assert!(app.workspace.get(altro).unwrap().is_dirty());
    assert_eq!(
        std::fs::read_to_string(base.join("progetto/altro.txt")).unwrap(),
        "altro"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn salvataggio_editor_durante_la_conferma_del_cestino_mostra_un_avviso() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::salvataggio_editor_durante_la_conferma_del_cestino_mostra_un_avviso",
    ) else {
        return;
    };
    let (mut app, folder, _, _, task) = prepara_cestino(&base);
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConfermaElimina));
    let _ = app.update(Message::EditorIn(app.groups.focus(), EditorCommand::Save));
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, avviso)| avviso.text.contains("Rispondi prima"))
    );
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
    let _ = consegna_cestino_reale(&mut app, task);
    assert!(!folder.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn cestino_non_parte_durante_un_salvataggio_reale() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::cestino_non_parte_durante_un_salvataggio_reale",
    ) else {
        return;
    };
    let root = base.join("progetto");
    let folder = root.join("moduli");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), "prima").unwrap();
    let altro = root.join("altro.txt");
    std::fs::write(&altro, "altro").unwrap();
    let mut app = app();
    app.set_project_root(root);
    app.open_file(&altro).unwrap();
    let doc = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("x".into()),
    ));
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    // L'ingresso UI è protetto dal modale; la scrittura interna esercita
    // comunque l'arbitraggio del Cestino contro un lease già posseduto.
    let richiesta_save = app.save_doc(doc);
    let save = Task::batch([richiesta_save, app.applica_seguito()]);
    assert_eq!(app.saves.active_document(), Some(doc));
    let _ = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    assert!(!app.albero_elimina.as_ref().unwrap().in_corso);
    assert!(app.notice.as_ref().unwrap().1.text.contains("scrittura"));
    assert!(folder.exists());
    app.completa_salvataggi(save);
    assert_eq!(std::fs::read_to_string(altro).unwrap(), "xaltro");
    let cestino = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    let _ = consegna_cestino_reale(&mut app, cestino);
    assert!(!folder.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn conferma_differita_non_crea_file_mentre_il_cestino_e_in_corso() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::conferma_differita_non_crea_file_mentre_il_cestino_e_in_corso",
    ) else {
        return;
    };
    let root = base.join("progetto");
    let folder = root.join("moduli");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), "prima").unwrap();
    let mut app = app();
    app.set_project_root(root.clone());
    menu_su_cartella(&mut app, &root);
    let _ = app.update(Message::MenuAction(MenuAct::NewTreeFile));
    let _ = app.update(Message::Albero(crate::prompt::PromptMessage::Changed(
        "nuovo.txt".into(),
    )));
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let task = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    let _ = app.update(Message::Albero(crate::prompt::PromptMessage::Confirm));
    assert!(!root.join("nuovo.txt").exists());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("Rispondi prima")
    );
    let _ = consegna_cestino_reale(&mut app, task);
    assert!(!folder.exists());
}

#[test]
fn ricevuta_con_lease_non_piu_posseduto_non_tocca_lo_stato_successore() {
    use crate::scritture::{Esito, Op, Permesso, Tipo};
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file.txt");
    std::fs::write(&file, "prima").unwrap();
    let mut app = app();
    app.set_project_root(root.path().to_owned());
    let doc = app.workspace.open(Some(file.clone()), "prima");
    app.groups.open(crate::editor_groups::GroupId::Primary, doc);
    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let _task = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    let vecchia = app.albero_elimina.clone().unwrap();
    let lease = vecchia.lease.unwrap();
    app.scritture.rilascia(lease, Esito::NonAvviato);
    let Permesso::Avvia(successore) = app
        .scritture
        .ammetti(Op::Cestino { docs: vec![doc] })
        .unwrap()
    else {
        panic!("atteso lease successore");
    };
    let epoca = app.scritture.guarda(None).epoca;
    // Solo identità: nessun risultato sintetico è prova del Cestino.
    let _ = app.update(Message::AlberoEliminato(
        vecchia.clone(),
        Ok(lume_core::TrashOutcome::Trashed),
    ));
    assert_eq!(app.albero_elimina.as_ref(), Some(&vecchia));
    assert!(app.scritture.possiede(successore, Tipo::Cestino));
    assert_eq!(app.scritture.guarda(None).epoca, epoca);
    assert!(app.groups.contains(doc));
    assert!(file.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn chiusura_durante_cestino_prosegue_alla_quiete() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::chiusura_durante_cestino_prosegue_alla_quiete",
    ) else {
        return;
    };
    let (mut app, folder, doc, _, task) = prepara_cestino(&base);
    let _ = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
    let riquadro = app.riquadro_chiusura().expect("chiusura in sospeso");
    assert!(riquadro.salvataggio_in_corso);
    assert!(app.chiusura.is_some());
    let chiusura = consegna_cestino_reale(&mut app, task);
    assert!(!folder.exists());
    assert!(!app.groups.contains(doc));
    assert!(app.chiusura.is_none(), "la quiete prosegue la chiusura");
    assert!(iced_runtime::task::into_stream(chiusura).is_some());
}

#[cfg(all(target_os = "linux", feature = "mcp"))]
#[test]
fn errore_cestino_e_card_reali_attendono_e_esc_promuove_in_ordine() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::errore_cestino_e_card_reali_attendono_e_esc_promuove_in_ordine",
    ) else {
        return;
    };
    std::fs::write(base.join("xdg"), "non è una cartella").unwrap();
    let (mut app, folder, doc, _, worker) = prepara_cestino(&base);
    let root = app.project_root.clone().unwrap();
    let _ = app.mcp_accendi(lume_mcp::confine::canonical(&root).unwrap());
    let epoch = app.mcp.as_ref().unwrap().epoch;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = app.update(Message::Mcp(epoch, lume_mcp::McpEvent::Ready(tx)));
    let _ = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
    let _ = app.update(Message::Mcp(
        epoch,
        lume_mcp::McpEvent::Ask {
            id: 901,
            conn: 1,
            query: lume_mcp::AppQuery::WriteFile {
                relative: "vietato.txt".into(),
                content: "no".into(),
            },
        },
    ));
    assert_eq!(
        app.shell.active_overlay(),
        Some(OverlayId::ConfermaChiusura)
    );
    let result = primo_output(worker);
    assert!(matches!(result, Message::AlberoEliminato(_, Err(_))));
    let _ = app.update(result);
    assert!(app.riquadro_chiusura().unwrap().errore.is_some());
    assert!(app.riquadro_elimina().unwrap().errore.is_some());
    let _ = app.update(Message::CloseAll);
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
    assert!(app.riquadro_elimina().unwrap().errore.is_some());
    assert!(app.scritture.guarda(None).in_volo.is_none());
    let _ = app.update(Message::CloseAll);
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConfermaElimina));
    assert!(app.riquadro_elimina().unwrap().errore.is_some());
    assert!(matches!(
        rx.try_recv(),
        Ok(lume_mcp::McpCommand::Answer {
            id: 901,
            answer: Err(_)
        })
    ));
    assert!(rx.try_recv().is_err());
    assert!(folder.exists());
    assert!(app.groups.contains(doc));
    assert!(!root.join("vietato.txt").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn scorciatoia_non_sostituisce_la_chiusura_in_sospeso_durante_cestino() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::scorciatoia_non_sostituisce_la_chiusura_in_sospeso_durante_cestino",
    ) else {
        return;
    };
    let (mut app, folder, _, _, task) = prepara_cestino(&base);
    let _ = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
    let _ = app.update(Message::MenuAction(MenuAct::Find));
    assert_eq!(
        app.shell.active_overlay(),
        Some(OverlayId::ConfermaChiusura)
    );
    assert!(app.riquadro_chiusura().unwrap().salvataggio_in_corso);
    let _ = consegna_cestino_reale(&mut app, task);
    assert!(!folder.exists());
    assert!(app.chiusura.is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn fallimento_reale_del_cestino_conserva_la_chiusura_e_l_errore() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::fallimento_reale_del_cestino_conserva_la_chiusura_e_l_errore",
    ) else {
        return;
    };
    std::fs::write(base.join("xdg"), "non è una cartella").unwrap();
    let (mut app, folder, doc, _, task) = prepara_cestino(&base);
    let _ = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
    assert!(app.riquadro_chiusura().unwrap().salvataggio_in_corso);
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    ));
    let risultato = primo_output(task);
    assert!(matches!(risultato, Message::AlberoEliminato(_, Err(_))));
    let _ = app.update(risultato);
    assert!(folder.exists());
    assert!(app.groups.contains(doc));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.riquadro_chiusura().unwrap().errore.is_some());
    assert!(app.riquadro_elimina().unwrap().errore.is_some());
    assert_eq!(
        app.shell.active_overlay(),
        Some(OverlayId::ConfermaChiusura)
    );
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::Annulla,
    ));
    assert!(app.chiusura.is_none());
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConfermaElimina));
    assert!(app.riquadro_elimina().unwrap().errore.is_some());
    assert!(app.scritture.guarda(None).in_volo.is_none());
    assert!(folder.exists());
    // La promozione mostra solo l'errore; un nuovo tentativo richiede un gesto.
    let _ = app.update(Message::CloseAll);
    std::fs::rename(base.join("xdg"), base.join("xdg-bloccato")).unwrap();
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let nuovo = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    let _ = consegna_cestino_reale(&mut app, nuovo);
    assert!(!folder.exists());
    assert!(!app.groups.contains(doc));
}

#[cfg(target_os = "linux")]
#[test]
fn cestino_rifiuta_anche_una_copia_appunti_chiesta_durante_il_worker() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::cestino_rifiuta_anche_una_copia_appunti_chiesta_durante_il_worker",
    ) else {
        return;
    };
    let root = base.join("progetto");
    let folder = root.join("moduli");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), "prima").unwrap();
    let mut app = app();
    app.set_project_root(root);
    let directory = base.join("appunti");
    let store = lume_notes::NoteStore::open(&directory).unwrap();
    let generation = app.begin_notes_load().unwrap();
    app.finish_notes_load(generation, directory, Vec::new())
        .unwrap();
    let nota = store.create("Nota").unwrap();
    let path = nota.path().to_owned();
    let doc = app.open_loaded_note(nota);
    let timer = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("modifica locale".into()),
    ));
    std::fs::write(&path, "# Altrove\n\nscritto da un altro editor\n").unwrap();
    let scadenza = primo_output(timer);
    assert!(matches!(scadenza, Message::NotesAutosaveDue(id, _) if id == doc));
    let salvataggio = app.update(scadenza);
    app.completa_salvataggi(salvataggio);
    assert!(app.conflitto_della_scheda(doc));
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let task = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    let _ = app.update(Message::NotesResolveConflict { doc, copia: true });
    assert_eq!(
        store.scan().unwrap().len(),
        1,
        "nessuna copia scritta durante Cestino"
    );
    assert!(app.workspace.get(doc).unwrap().is_dirty());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("Rispondi prima")
    );
    let _ = consegna_cestino_reale(&mut app, task);
    assert!(!folder.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn scheda_ripristinata_toccata_non_accoda_input_o_salvataggi_durante_cestino() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::scheda_ripristinata_toccata_non_accoda_input_o_salvataggi_durante_cestino",
    ) else {
        return;
    };
    let root = base.join("progetto");
    let folder = root.join("moduli");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("ripristinato.txt");
    let altro = root.join("attivo.txt");
    std::fs::write(&file, "prima").unwrap();
    std::fs::write(&altro, "altro").unwrap();
    let mut app = app();
    let mut snapshot = lume_store::SessionSnapshot::empty();
    snapshot.workspace_root = Some(root);
    snapshot.tabs = vec![altro.clone(), file.clone()];
    snapshot.active_tab = Some(altro);
    app.restore_snapshot(snapshot);
    let doc = app.workspace.get_by_path(&file).unwrap();
    assert!(app.pending_restored_tabs.contains(doc));
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let task = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    let _ = app.update(Message::CloseAll);
    let _read = app.activate_tab(doc);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("vietato".into()),
    ));
    let _ = app.update(Message::ClipboardPasted(Some("vietato".into())));
    let _ = app.update(Message::RestoredTabAction(
        doc,
        DeferredRestoredAction::SaveDocument,
    ));
    assert!(!app.restored_tab_commands.contains_key(&doc));
    assert!(!app.restored_tab_actions.contains_key(&doc));
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
    assert!(app.notice.as_ref().unwrap().1.text.contains("Cestino"));
    let _ = consegna_cestino_reale(&mut app, task);
    assert!(!folder.exists());
    assert!(!app.groups.contains(doc));
    let _ = app.update(Message::RestoredTabAction(
        doc,
        DeferredRestoredAction::SaveDocument,
    ));
    assert!(!file.exists());
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
}

#[cfg(target_os = "linux")]
#[test]
fn lettura_di_avvio_non_cambia_progetto_durante_il_cestino() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::lettura_di_avvio_non_cambia_progetto_durante_il_cestino",
    ) else {
        return;
    };
    let (mut app, folder, _, _, task) = prepara_cestino(&base);
    let root = app.project_root.clone();
    let esterno = base.join("esterno.txt");
    std::fs::write(&esterno, "esterno").unwrap();
    let id = app.workspace.open(Some(esterno.clone()), "esterno");
    app.groups.open(crate::editor_groups::GroupId::Primary, id);
    let read = app.open_file_task(
        esterno,
        crate::file_read::DopoApertura::Avvio {
            fuori_progetto: true,
        },
    );
    let esito = primo_output(read);
    let _ = app.update(esito);
    assert_eq!(app.project_root, root);
    assert!(app.notice.as_ref().unwrap().1.text.contains("Cestino"));
    let _ = consegna_cestino_reale(&mut app, task);
    assert!(!folder.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn autosalvataggio_estraneo_rimandato_riparte_con_avviso_alla_quiete() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::autosalvataggio_estraneo_rimandato_riparte_con_avviso_alla_quiete",
    ) else {
        return;
    };
    let root = base.join("progetto");
    let folder = root.join("moduli");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), "prima").unwrap();
    let mut app = app();
    app.set_project_root(root);
    let directory = base.join("appunti");
    let store = lume_notes::NoteStore::open(&directory).unwrap();
    let generation = app.begin_notes_load().unwrap();
    app.finish_notes_load(generation, directory, Vec::new())
        .unwrap();
    let nota = store.create("Nota").unwrap();
    let path = nota.path().to_owned();
    let originale = std::fs::read(&path).unwrap();
    let doc = app.open_loaded_note(nota);
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let cestino = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    let _ = app.update(Message::CloseAll);
    let timer = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("testo rimandato".into()),
    ));
    let scadenza = primo_output(timer);
    assert!(matches!(scadenza, Message::NotesAutosaveDue(id, _) if id == doc));
    let _ = app.update(scadenza);
    assert!((app.scritture.contiene_salvataggio(doc) || app.saves.active_document() == Some(doc)));
    assert!(app.notice.as_ref().unwrap().1.text.contains("rimandato"));
    assert_eq!(std::fs::read(&path).unwrap(), originale);
    let ripresa = consegna_cestino_reale(&mut app, cestino);
    assert!(!folder.exists());
    assert_eq!(app.saves.active_document(), Some(doc));
    assert!(app.notice.as_ref().unwrap().1.text.contains("ripart"));
    app.completa_salvataggi(ripresa);
    assert!(
        std::fs::read_to_string(path)
            .unwrap()
            .contains("testo rimandato")
    );
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
}

#[cfg(target_os = "linux")]
#[test]
fn scheda_ripristinata_estranea_elabora_input_differito_durante_cestino() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::scheda_ripristinata_estranea_elabora_input_differito_durante_cestino",
    ) else {
        return;
    };
    let root = base.join("progetto");
    let folder = root.join("moduli");
    std::fs::create_dir_all(&folder).unwrap();
    let colpito = folder.join("a.txt");
    let altro = root.join("altro.txt");
    let attivo = root.join("attivo.txt");
    std::fs::write(&colpito, "prima").unwrap();
    std::fs::write(&altro, "altro").unwrap();
    std::fs::write(&attivo, "attivo").unwrap();
    let mut app = app();
    let mut snapshot = lume_store::SessionSnapshot::empty();
    snapshot.workspace_root = Some(root);
    snapshot.tabs = vec![attivo.clone(), colpito, altro.clone()];
    snapshot.active_tab = Some(attivo);
    app.restore_snapshot(snapshot);
    let doc = app.workspace.get_by_path(&altro).unwrap();
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let task = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    let _ = app.update(Message::CloseAll);
    let read = app.activate_tab(doc);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("x".into()),
    ));
    let esito = primo_output(read);
    let _ = app.update(esito);
    assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), "xaltro");
    let _ = consegna_cestino_reale(&mut app, task);
    assert!(!folder.exists());
    assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), "xaltro");
}

#[cfg(target_os = "linux")]
#[test]
fn esito_duplicato_sullo_stesso_percorso_non_interferisce_col_tentativo_successivo() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::esito_duplicato_sullo_stesso_percorso_non_interferisce_col_tentativo_successivo",
    ) else {
        return;
    };
    std::fs::write(base.join("xdg"), "non è una cartella").unwrap();
    let (mut app, folder, doc, _, primo) = prepara_cestino(&base);
    let vecchia = app.albero_elimina.clone().unwrap();
    let esito = primo_output(primo);
    assert!(matches!(esito, Message::AlberoEliminato(_, Err(_))));
    let _ = app.update(esito);
    std::fs::rename(base.join("xdg"), base.join("xdg-bloccato")).unwrap();
    let secondo = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    assert!(app.albero_elimina.as_ref().unwrap().in_corso);
    let epoca = app.scritture.guarda(None).epoca;
    // Replica sintetica solo per autenticare la ricevuta, non per provare il backend.
    let _ = app.update(Message::AlberoEliminato(
        vecchia,
        Ok(lume_core::TrashOutcome::NeedsPermanentConfirmation),
    ));
    assert!(app.albero_elimina.as_ref().unwrap().in_corso);
    assert_eq!(app.scritture.guarda(None).epoca, epoca);
    assert!(app.groups.contains(doc));
    let _ = consegna_cestino_reale(&mut app, secondo);
    assert!(!folder.exists());
    assert!(!app.groups.contains(doc));
}

#[cfg(target_os = "linux")]
#[test]
fn cestino_consente_find_estraneo_e_rifiuta_sostituzioni_del_discendente() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::cestino_consente_find_estraneo_e_rifiuta_sostituzioni_del_discendente",
    ) else {
        return;
    };
    use crate::find_replace::FindMessage;
    let (mut app, folder, doc, altro, task) = prepara_cestino(&base);
    let _ = app.update(Message::CloseAll);
    let _ = app.activate_tab(doc);
    let _ = app.update(Message::MenuAction(MenuAct::Replace));
    let _ = app.update(Message::Find(FindMessage::QueryChanged("prima".into())));
    let _ = app.update(Message::Find(FindMessage::ReplacementChanged(
        "vietato".into(),
    )));
    let _ = app.update(Message::Find(FindMessage::ReplaceAll));
    assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), "prima");
    let _ = app.update(Message::CloseAll);
    let _ = app.activate_tab(altro);
    let _ = app.update(Message::MenuAction(MenuAct::Replace));
    let _ = app.update(Message::Find(FindMessage::QueryChanged("altro".into())));
    let _ = app.update(Message::Find(FindMessage::ReplacementChanged(
        "cambiato".into(),
    )));
    let _ = app.update(Message::Find(FindMessage::ReplaceAll));
    assert_eq!(
        app.workspace.get(altro).unwrap().rope().to_string(),
        "cambiato"
    );
    let _ = consegna_cestino_reale(&mut app, task);
    assert!(!folder.exists());
}

/// Apre il menu su una cartella dell'albero, come fa il tasto destro.
fn menu_su_cartella(app: &mut Lume, cartella: &std::path::Path) {
    let _ = app.update(Message::Tool(
        crate::tools::project::ID,
        ToolEvent::new(crate::tools::project::Event::RowRightClicked(
            cartella.to_path_buf(),
            true,
            10.0,
            20.0,
        )),
    ));
}

#[test]
fn su_una_cartella_il_menu_offre_di_creare() {
    let app = app();
    let voci = crate::context_menu::items(
        &Soggetto::Albero {
            path: std::path::PathBuf::from("/tmp/cartella"),
            is_dir: true,
        },
        app.menu_checks(),
    );
    let etichette: Vec<&str> = voci.iter().map(|(etichetta, _)| *etichetta).collect();
    let mut attese = vec!["Nuovo file…", "Nuova cartella…"];
    if !cfg!(windows) {
        attese.push("Elimina (Cestino)…");
    }
    attese.push("Copia percorso");
    assert_eq!(etichette, attese);
}

#[test]
fn nuovo_file_crea_il_file_su_disco_e_chiude_il_riquadro() {
    let mut app = app();
    let root = tempdir("albero_nuovo_file");
    let cartella = root.join("src");
    std::fs::create_dir(&cartella).unwrap();
    app.set_project_root(root.clone());

    menu_su_cartella(&mut app, &cartella);
    let _ = app.update(Message::MenuAction(MenuAct::NewTreeFile));
    assert!(app.albero.is_some(), "il riquadro è aperto");
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Albero));

    let _ = app.update(Message::Albero(crate::prompt::PromptMessage::Changed(
        "nuovo.txt".into(),
    )));
    let _ = app.update(Message::Albero(crate::prompt::PromptMessage::Confirm));

    assert!(cartella.join("nuovo.txt").is_file(), "il file è sul disco");
    assert!(app.albero.is_none(), "il riquadro è chiuso");
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(notifica.contains("Creato"), "{notifica}");
}

#[test]
fn nuova_cartella_crea_la_cartella_su_disco() {
    let mut app = app();
    let root = tempdir("albero_nuova_cartella");
    app.set_project_root(root.clone());

    menu_su_cartella(&mut app, &root);
    let _ = app.update(Message::MenuAction(MenuAct::NewTreeFolder));
    let _ = app.update(Message::Albero(crate::prompt::PromptMessage::Changed(
        "moduli".into(),
    )));
    let _ = app.update(Message::Albero(crate::prompt::PromptMessage::Confirm));

    assert!(root.join("moduli").is_dir(), "la cartella è su disco");
}

#[test]
fn un_nome_gia_presente_non_e_sovrapposto_e_il_riquadro_resta() {
    let mut app = app();
    let root = tempdir("albero_gia_presente");
    let cartella = root.join("src");
    std::fs::create_dir(&cartella).unwrap();
    std::fs::write(cartella.join("gia.txt"), b"contenuto").unwrap();
    app.set_project_root(root.clone());

    menu_su_cartella(&mut app, &cartella);
    let _ = app.update(Message::MenuAction(MenuAct::NewTreeFile));
    let _ = app.update(Message::Albero(crate::prompt::PromptMessage::Changed(
        "gia.txt".into(),
    )));
    let _ = app.update(Message::Albero(crate::prompt::PromptMessage::Confirm));

    assert_eq!(
        std::fs::read(cartella.join("gia.txt")).unwrap(),
        b"contenuto",
        "il file esistente non è stato toccato"
    );
    let richiesta = app.albero.as_ref().expect("il riquadro resta aperto");
    let errore = richiesta.errore.clone().expect("l'errore nel riquadro");
    assert!(errore.contains("esiste già"), "{errore}");
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(notifica.contains("esiste già"), "{notifica}");
}

#[test]
fn fuori_progetto_la_richiesta_e_rifiutata_con_avviso() {
    let mut app = app();
    let root = tempdir("albero_fuori");
    app.set_project_root(root.clone());
    let fuori = tempfile::tempdir().unwrap();

    // il menu su una cartella fuori dal progetto: la voce non apre niente
    menu_su_cartella(&mut app, fuori.path());
    let _ = app.update(Message::MenuAction(MenuAct::NewTreeFile));

    assert!(app.albero.is_none(), "il riquadro non si apre");
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(notifica.contains("non è dentro il progetto"), "{notifica}");
}

#[test]
fn la_esc_chiude_il_riquadro_e_non_lascia_nome_scritto() {
    let mut app = app();
    let root = tempdir("albero_esc");
    app.set_project_root(root.clone());

    menu_su_cartella(&mut app, &root);
    let _ = app.update(Message::MenuAction(MenuAct::NewTreeFile));
    let _ = app.update(Message::Albero(crate::prompt::PromptMessage::Changed(
        "nuovo".into(),
    )));
    assert!(app.albero.is_some());

    // l'Esc è l'«Annulla»: il riquadro chiude e non resta nome scritto
    let _ = app.update(Message::CloseAll);

    assert!(
        app.albero.is_none(),
        "lo stato non mente su quello che è a schermo"
    );
    assert_eq!(app.shell.active_overlay(), None);
    assert!(!root.join("nuovo").exists(), "niente è stato creato");
}

#[test]
fn un_nome_invalido_non_crea_nulla() {
    let mut app = app();
    let root = tempdir("albero_nome_invalido");
    app.set_project_root(root.clone());

    menu_su_cartella(&mut app, &root);
    let _ = app.update(Message::MenuAction(MenuAct::NewTreeFile));
    let _ = app.update(Message::Albero(crate::prompt::PromptMessage::Changed(
        "../fuori.txt".into(),
    )));
    let _ = app.update(Message::Albero(crate::prompt::PromptMessage::Confirm));

    assert!(!root.parent().unwrap().join("fuori.txt").exists());
    let richiesta = app.albero.as_ref().expect("il riquadro resta aperto");
    assert!(richiesta.errore.is_some(), "l'errore è nel riquadro");
}

/// Apre il menu su un file dell'albero, come fa il tasto destro.
fn menu_su_file(app: &mut Lume, file: &std::path::Path) {
    let _ = app.update(Message::Tool(
        crate::tools::project::ID,
        ToolEvent::new(crate::tools::project::Event::RowRightClicked(
            file.to_path_buf(),
            false,
            10.0,
            20.0,
        )),
    ));
}

#[test]
fn su_un_file_il_menu_offre_di_rinominarlo_e_di_eliminarlo_nel_cestino() {
    let app = app();
    let voci = crate::context_menu::items(
        &Soggetto::Albero {
            path: std::path::PathBuf::from("/tmp/file.txt"),
            is_dir: false,
        },
        app.menu_checks(),
    );
    let etichette: Vec<&str> = voci.iter().map(|(etichetta, _)| *etichetta).collect();
    assert_eq!(
        etichette,
        ["Apri", "Rinomina…", "Elimina (Cestino)…", "Copia percorso"]
    );
}

#[test]
fn su_una_cartella_il_menu_offre_il_cestino() {
    let app = app();
    let voci = crate::context_menu::items(
        &Soggetto::Albero {
            path: std::path::PathBuf::from("/tmp/cartella"),
            is_dir: true,
        },
        app.menu_checks(),
    );
    let etichette: Vec<&str> = voci.iter().map(|(etichetta, _)| *etichetta).collect();
    assert_eq!(
        etichette.contains(&"Elimina (Cestino)…"),
        !cfg!(windows),
        "{etichette:?}"
    );
}

#[test]
fn la_conferma_di_eliminazione_si_apre_sul_file_e_il_file_resta_sul_disco() {
    let mut app = app();
    let root = tempdir("albero_elimina_conferma");
    let file = root.join("da-eliminare.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    app.set_project_root(root.clone());

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));

    assert!(app.albero_elimina.is_some(), "la conferma è aperta");
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConfermaElimina));
    assert!(
        file.is_file(),
        "il file non è stato toccato prima della conferma"
    );
    let riquadro = app.riquadro_elimina().expect("il riquadro");
    assert_eq!(riquadro.nome, "da-eliminare.txt");
}

#[test]
fn annullare_la_conferma_non_tocca_il_file() {
    let mut app = app();
    let root = tempdir("albero_elimina_annulla");
    let file = root.join("da-eliminare.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    app.set_project_root(root.clone());

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let _ = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Annulla,
    ));

    assert!(app.albero_elimina.is_none(), "la conferma è chiusa");
    assert!(file.is_file(), "il file è ancora sul disco");
    assert_eq!(std::fs::read(&file).unwrap(), b"contenuto");
}

#[test]
fn esc_su_altro_overlay_conserva_la_domanda_cestino_sostituita() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("da-eliminare.txt");
    std::fs::write(&file, "contenuto").unwrap();
    let mut app = app();
    app.set_project_root(root.path().to_path_buf());
    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    app.goto_line = Some("7".into());
    app.apri_overlay(OverlayId::GotoLine);
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConfermaElimina));
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, n)| n.kind == NoticeKind::Info)
    );
    let _ = app.update(Message::CloseAll);
    assert!(app.albero_elimina.is_none());
    assert_eq!(app.goto_line.as_deref(), Some("7"));
    assert_eq!(app.shell.active_overlay(), None);
    assert!(file.exists());
}

#[test]
fn fuori_progetto_l_eliminazione_e_rifiutata_con_avviso() {
    let mut app = app();
    let root = tempdir("albero_elimina_fuori");
    app.set_project_root(root.clone());
    let fuori = tempfile::tempdir().unwrap();
    let file = fuori.path().join("fuori.txt");
    std::fs::write(&file, b"contenuto").unwrap();

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));

    assert!(app.albero_elimina.is_none(), "la conferma non si apre");
    assert!(file.is_file(), "il file fuori progetto è intatto");
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(notifica.contains("non è dentro il progetto"), "{notifica}");
}

#[test]
fn un_file_aperto_con_modifiche_non_si_elimina() {
    let mut app = app();
    let root = tempdir("albero_elimina_sporco");
    let file = root.join("aperto.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    app.set_project_root(root.clone());

    // il file è aperto in una scheda, e il buffer è modificato
    let doc = app.workspace.open(Some(file.clone()), "contenuto");
    app.groups.open(crate::editor_groups::GroupId::Primary, doc);
    let lunghezza = app
        .workspace
        .get(doc)
        .expect("il documento")
        .rope()
        .len_chars();
    let cambi = lume_core::ChangeSet::replace_range(0, 0, "altro", lunghezza);
    app.workspace
        .get_mut(doc)
        .expect("il documento")
        .apply(lume_core::Transaction {
            changes: cambi,
            selection_after: None,
        });

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));

    assert!(app.albero_elimina.is_none(), "la conferma non si apre");
    assert!(file.is_file(), "il file aperto e sporco è intatto");
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(notifica.contains("modifiche non salvate"), "{notifica}");
}

#[test]
fn la_conferma_di_eliminazione_registra_le_schede_aperte_sul_file() {
    let mut app = app();
    let root = tempdir("albero_elimina_schede");
    let file = root.join("aperto.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    app.set_project_root(root.clone());

    // il file è aperto in una scheda, ma il buffer è pulito
    let doc = app.workspace.open(Some(file.clone()), "contenuto");
    app.groups.open(crate::editor_groups::GroupId::Primary, doc);

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));

    let richiesta = app.albero_elimina.as_ref().expect("la conferma è aperta");
    assert_eq!(
        richiesta.schede,
        vec![doc],
        "la scheda del file è registrata"
    );
    assert!(!richiesta.in_corso, "prima di confermare non è in corso");
}

#[test]
fn una_ricevuta_senza_worker_non_autorizza_un_errore() {
    let mut app = app();
    let root = tempdir("albero_elimina_errore");
    let file = root.join("da-eliminare.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    app.set_project_root(root.clone());

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let richiesta = app.albero_elimina.clone().expect("la conferma è aperta");

    // Un messaggio senza worker ammesso non appartiene alla conferma.
    let _ = app.update(Message::AlberoEliminato(
        richiesta,
        Ok(lume_core::TrashOutcome::NeedsPermanentConfirmation),
    ));

    let riquadro = app.riquadro_elimina().expect("il riquadro resta");
    assert!(riquadro.errore.is_none());
    assert!(!riquadro.in_corso, "lo spostamento è finito");
    assert!(file.is_file(), "il file non è stato toccato");
}

#[cfg(target_os = "linux")]
#[test]
fn l_esito_non_si_perde_se_il_utente_preme_esc_durante_lo_spostamento() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::l_esito_non_si_perde_se_il_utente_preme_esc_durante_lo_spostamento",
    ) else {
        return;
    };
    let mut app = app();
    let root = base.join("progetto");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("da-eliminare.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    app.set_project_root(root.clone());

    // il file è aperto in una scheda, pulita
    let doc = app.workspace.open(Some(file.clone()), "contenuto");
    app.groups.open(crate::editor_groups::GroupId::Primary, doc);

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let task = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));

    // l'utente preme Esc mentre lo spostamento è in corso
    let _ = app.update(Message::CloseAll);
    assert!(
        app.albero_elimina
            .as_ref()
            .is_some_and(|richiesta| richiesta.in_corso),
        "lo stato dello spostamento resta: l'esito deve poter tornare"
    );

    // l'esito arriva: le schede chiudono davvero e l'utente viene avvisato
    let _ = consegna_cestino_reale(&mut app, task);
    assert!(!file.exists());
    assert!(
        !app.groups.contains(doc),
        "la scheda del file eliminato è chiusa"
    );
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(notifica.contains("Cestino"), "{notifica}");
}

#[test]
fn un_esito_ripetuto_non_tocca_la_conferma_di_un_altro_file() {
    let mut app = app();
    let root = tempdir("albero_elimina_altra_richiesta");
    let primo = root.join("primo.txt");
    let secondo = root.join("secondo.txt");
    std::fs::write(&primo, b"uno").unwrap();
    std::fs::write(&secondo, b"due").unwrap();
    app.set_project_root(root.clone());

    // parte lo spostamento del primo file
    menu_su_file(&mut app, &primo);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let _ = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    let richiesta_primo = app.albero_elimina.clone().expect("la conferma è aperta");

    // La prima operazione termina; poi l'utente apre una nuova conferma.
    let _ = app.update(Message::AlberoEliminato(
        richiesta_primo.clone(),
        Ok(lume_core::TrashOutcome::Trashed),
    ));
    menu_su_file(&mut app, &secondo);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let richiesta_secondo = app.albero_elimina.clone().expect("la seconda conferma");
    assert_eq!(richiesta_secondo.percorso, secondo);

    // Un replay del primo esito non chiude la conferma del secondo.
    let _ = app.update(Message::AlberoEliminato(
        richiesta_primo,
        Ok(lume_core::TrashOutcome::Trashed),
    ));
    let richiesta = app
        .albero_elimina
        .as_ref()
        .expect("la conferma del secondo file resta");
    assert_eq!(
        richiesta.percorso, secondo,
        "l'esito del primo file non ha toccato la conferma del secondo"
    );
}

#[test]
fn annullare_durante_lo_spostamento_non_fa_sparire_la_conferma() {
    let mut app = app();
    let root = tempdir("albero_elimina_in_corso");
    let file = root.join("da-eliminare.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    app.set_project_root(root.clone());

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    // lo spostamento parte
    let _ = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    assert!(
        app.albero_elimina.as_ref().is_some_and(|r| r.in_corso),
        "lo spostamento è in corso"
    );

    // annullare ora non azzera niente: l'esito deve poter tornare
    let _ = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Annulla,
    ));
    assert!(
        app.albero_elimina.as_ref().is_some_and(|r| r.in_corso),
        "annullare durante lo spostamento non azzera la conferma"
    );
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, avviso)| avviso.text.contains("Cestino")),
        "l'annullamento rifiutato deve essere spiegato"
    );
}

#[test]
fn il_menu_del_file_offre_anche_di_rinominarlo() {
    let app = app();
    let voci = crate::context_menu::items(
        &Soggetto::Albero {
            path: std::path::PathBuf::from("/tmp/file.txt"),
            is_dir: false,
        },
        app.menu_checks(),
    );
    let etichette: Vec<&str> = voci.iter().map(|(etichetta, _)| *etichetta).collect();
    assert_eq!(
        etichette,
        ["Apri", "Rinomina…", "Elimina (Cestino)…", "Copia percorso"]
    );
}

#[test]
fn rinominare_su_disco_il_file_del_progetto() {
    let mut app = app();
    let root = tempdir("albero_rinomina");
    let file = root.join("prima.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    app.set_project_root(root.clone());

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::RenameTreeFile));
    let richiesta = app.albero_rinomina.as_ref().expect("il riquadro è aperto");
    assert_eq!(richiesta.nome_attuale, "prima.txt");
    assert_eq!(
        richiesta.valore, "prima.txt",
        "il campo parte dal nome attuale"
    );

    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Changed("dopo.txt".into()),
    ));
    let prima = app.scritture.guarda(None);
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Confirm,
    ));

    assert!(!file.exists(), "il nome vecchio non esiste più");
    let nuovo = root.join("dopo.txt");
    assert!(nuovo.is_file(), "il file è sul disco con il nome nuovo");
    assert_eq!(std::fs::read(&nuovo).unwrap(), b"contenuto");
    assert!(app.albero_rinomina.is_none(), "il riquadro è chiuso");
    let dopo = app.scritture.guarda(None);
    assert_eq!(dopo.epoca, prima.epoca + 1);
    assert_eq!(dopo.in_volo, None);
    assert_eq!(dopo.file, crate::scritture::Presa::Libero);
    assert_eq!(
        app.scritture.seguito(),
        crate::scritture::Seguito::default()
    );
}

#[test]
fn rinomina_rifiutata_durante_mutazione_si_riprova_senza_cambiare_nome() {
    use crate::scritture::{Esito, Op, Permesso};
    let mut app = app();
    let root = tempdir("rinomina_rifiuto");
    let file = root.join("prima.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    app.set_project_root(root.clone());
    let lease = match app.scritture.ammetti(Op::Cestino { docs: vec![] }).unwrap() {
        Permesso::Avvia(lease) => lease,
        altro => panic!("permesso inatteso: {altro:?}"),
    };
    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::RenameTreeFile));
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Changed("dopo.txt".into()),
    ));
    let epoca = app.scritture.guarda(None).epoca;
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Confirm,
    ));
    assert!(file.exists());
    assert_eq!(app.scritture.guarda(None).epoca, epoca);
    assert!(app.albero_rinomina_prompt().unwrap().ready);
    assert!(app.notice.is_some());
    app.scritture.rilascia(lease, Esito::Riuscito);
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Confirm,
    ));
    assert!(!file.exists());
    assert!(root.join("dopo.txt").exists());
    assert!(app.scritture.guarda(None).in_volo.is_none());
}

#[test]
fn un_nome_invalido_non_rinomina_e_l_errore_resta_nel_riquadro() {
    let mut app = app();
    let root = tempdir("albero_rinomina_invalida");
    let file = root.join("prima.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    app.set_project_root(root.clone());

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::RenameTreeFile));
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Changed("../fuga.txt".into()),
    ));
    let prima = app.scritture.guarda(None);
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Confirm,
    ));

    assert!(file.is_file(), "il file non è stato toccato");
    assert_eq!(std::fs::read(&file).unwrap(), b"contenuto");
    assert_eq!(app.scritture.guarda(None), prima);
    let richiesta = app
        .albero_rinomina
        .as_ref()
        .expect("il riquadro resta aperto");
    let errore = richiesta.errore.clone().expect("l'errore nel riquadro");
    assert!(errore.contains("separatori"), "{errore}");
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(notifica.contains("separatori"), "{notifica}");
}

#[test]
fn rinominare_riallinea_il_documento_aperto_sul_nuovo_percorso() {
    let mut app = app();
    let root = tempdir("albero_rinomina_aperto");
    let file = root.join("prima.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    app.set_project_root(root.clone());

    // il file è aperto in una scheda
    let doc = app.workspace.open(Some(file.clone()), "contenuto");
    app.groups.open(crate::editor_groups::GroupId::Primary, doc);

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::RenameTreeFile));
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Changed("dopo.txt".into()),
    ));
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Confirm,
    ));

    let percorso = app
        .workspace
        .get(doc)
        .and_then(lume_core::Document::path)
        .map(|p| p.to_path_buf());
    assert_eq!(
        percorso,
        Some(root.join("dopo.txt")),
        "il documento aperto segue il nome nuovo"
    );
}

#[test]
fn la_rinomina_non_lega_a_se_stessi_un_documento_di_un_altro_file() {
    let mut app = app();
    let root = tempdir("albero_rinomina_estraneo");
    let file = root.join("prima.txt");
    let altro = root.join("altro.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    std::fs::write(&altro, b"altro contenuto").unwrap();
    app.set_project_root(root.clone());

    // il file da rinominare e' aperto, e con lui un altro file
    let giusto = app.workspace.open(Some(file.clone()), "contenuto");
    app.groups
        .open(crate::editor_groups::GroupId::Primary, giusto);
    let estraneo = app.workspace.open(Some(altro.clone()), "altro contenuto");
    let percorso_altro = altro.clone();
    // Il file dell'altro documento viene cancellato dal disco: e' il caso in cui
    // il vecchio confronto canonico (None == None) lo abbinerebbe per errore.
    std::fs::remove_file(&altro).unwrap();
    app.groups
        .open(crate::editor_groups::GroupId::Primary, estraneo);

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::RenameTreeFile));
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Changed("dopo.txt".into()),
    ));
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Confirm,
    ));

    // il documento giusto segue il nome nuovo
    let percorso_giusto = app
        .workspace
        .get(giusto)
        .and_then(lume_core::Document::path)
        .map(|p| p.to_path_buf());
    assert_eq!(percorso_giusto, Some(root.join("dopo.txt")));
    // e quello estraneo resta dov'era
    let percorso_estraneo = app
        .workspace
        .get(estraneo)
        .and_then(lume_core::Document::path)
        .map(|p| p.to_path_buf());
    assert_eq!(
        percorso_estraneo,
        Some(percorso_altro.clone()),
        "il documento dell'altro file non è stato toccato"
    );
    assert!(
        !percorso_altro.exists(),
        "il file dell'altro documento non è stato toccato"
    );
}

#[test]
fn la_rinomina_di_un_file_aperto_conserva_il_contenuto_sul_nuovo_percorso() {
    let mut app = app();
    let root = tempdir("albero_rinomina_contenuto");
    let file = root.join("prima.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    app.set_project_root(root.clone());

    let doc = app.workspace.open(Some(file.clone()), "contenuto");
    app.groups.open(crate::editor_groups::GroupId::Primary, doc);

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::RenameTreeFile));
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Changed("dopo.txt".into()),
    ));
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Confirm,
    ));

    // il file rinominato conserva il contenuto: la rinomina non tocca i byte
    assert_eq!(
        std::fs::read(root.join("dopo.txt")).unwrap(),
        b"contenuto",
        "il contenuto del file rinominato e' intatto"
    );
    assert!(!file.exists(), "il nome vecchio non esiste piu'");
}

#[test]
fn il_salvataggio_dopo_la_rinomina_finisce_sul_nuovo_percorso() {
    let mut app = app();
    let root = tempdir("albero_rinomina_salvataggio");
    let file = root.join("prima.txt");
    std::fs::write(&file, b"iniziale\n").unwrap();
    app.set_project_root(root.clone());

    // il file e' aperto e il buffer e' modificato
    let doc = app.workspace.open(Some(file.clone()), "iniziale\n");
    app.groups.open(crate::editor_groups::GroupId::Primary, doc);
    let lunghezza = app
        .workspace
        .get(doc)
        .expect("il documento")
        .rope()
        .len_chars();
    let cambi = lume_core::ChangeSet::replace_range(0, lunghezza, "modificato\n", lunghezza);
    app.workspace
        .get_mut(doc)
        .expect("il documento")
        .apply(lume_core::Transaction {
            changes: cambi,
            selection_after: None,
        });

    // rinomina
    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::RenameTreeFile));
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Changed("dopo.txt".into()),
    ));
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Confirm,
    ));

    // e il salvataggio va sul nome nuovo
    app.write_doc(doc).expect("il salvataggio riesce");

    assert_eq!(
        std::fs::read(root.join("dopo.txt")).unwrap(),
        b"modificato\n",
        "il testo modificato e' sul nome nuovo"
    );
    assert!(
        !file.exists(),
        "il nome vecchio non e' stato ricreato dal salvataggio"
    );
}

#[test]
fn se_il_rebind_fallisce_il_ciclo_continua_e_l_avviso_nomina_il_documento() {
    let mut app = app();
    let root = tempdir("albero_rinomina_rebind");
    let file = root.join("prima.txt");
    let occupato = root.join("occupato.txt");
    std::fs::write(&file, b"contenuto").unwrap();
    // il file che occupa la destinazione NON e' su disco: e' un documento
    // aperto su un percorso sparito. Cosi' la rinomina va avanti (il nome
    // libero sul disco) e il rebind fallisce con PathTaken, perche' l'indice
    // del workspace ha gia' quel percorso.
    app.set_project_root(root.clone());

    // il file da rinominare e' aperto, e un altro documento ha gia' preso il
    // percorso di destinazione: il rebind del primo fallira' con PathTaken.
    let giusto = app.workspace.open(Some(file.clone()), "contenuto");
    app.groups
        .open(crate::editor_groups::GroupId::Primary, giusto);
    let altro = app.workspace.open(Some(occupato.clone()), "occupato");
    app.groups
        .open(crate::editor_groups::GroupId::Primary, altro);

    menu_su_file(&mut app, &file);
    let _ = app.update(Message::MenuAction(MenuAct::RenameTreeFile));
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Changed("occupato.txt".into()),
    ));
    let _ = app.update(Message::AlberoRinomina(
        crate::prompt::PromptMessage::Confirm,
    ));

    // il riquadro e' chiuso e l'errore e' in barra di stato
    assert!(app.albero_rinomina.is_none(), "il riquadro è chiuso");
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(notifica.contains("riallineati"), "{notifica}");
    // il file non e' stato perso: il nome nuovo non esiste, quello vecchio c'e'
    // La rinomina **su disco** e' riuscita: il file ha il nome nuovo. Quello
    // che non e' riuscito e' il **legame** del documento, che resta sul
    // percorso vecchio: e' esattamente cio' che l'avviso deve dire.
    assert!(
        root.join("occupato.txt").is_file(),
        "il file e' stato rinominato su disco"
    );
    let percorso = app
        .workspace
        .get(giusto)
        .and_then(lume_core::Document::path)
        .map(|p| p.to_path_buf());
    assert_eq!(
        percorso,
        Some(file.clone()),
        "il documento resta sul percorso vecchio: il rebind non e' riuscito"
    );
}

#[test]
fn cartella_con_discendente_sporco_o_scrittura_in_corso_resta_intatta() {
    let mut app = app();
    let root = tempdir("cartella_protetta");
    let folder = root.join("moduli");
    std::fs::create_dir(&folder).unwrap();
    let file = folder.join("aperto.txt");
    std::fs::write(&file, "prima").unwrap();
    app.set_project_root(root);
    let doc = app.workspace.open(Some(file.clone()), "prima");
    app.groups.open(crate::editor_groups::GroupId::Primary, doc);
    // La coda reale comprende anche scritture non ancora partite.
    let Ok(crate::scritture::Permesso::Avvia(lease)) =
        app.scritture.ammetti(crate::scritture::Op::Salva {
            doc,
            versione: 0,
            causa: crate::scritture::Causa::Utente,
        })
    else {
        panic!("save non ammesso")
    };
    let _ = app.scritture.seguito();
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    assert!(app.albero_elimina.is_none());
    assert!(app.notice.as_ref().unwrap().1.text.contains("scrittura"));
    app.scritture
        .rilascia(lease, crate::scritture::Esito::NonAvviato);
    let _ = app.scritture.seguito();
    app.workspace
        .get_mut(doc)
        .unwrap()
        .apply(lume_core::Transaction {
            changes: lume_core::ChangeSet::replace_range(0, 0, "altro", 5),
            selection_after: None,
        });
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    assert!(app.albero_elimina.is_none());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("modifiche non salvate")
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), "prima");
}

#[test]
fn cartella_conferma_contenuto_e_ricontrolla_i_discendenti_prima_del_worker() {
    let mut app = app();
    let root = tempdir("cartella_conferma");
    let folder = root.join("moduli");
    std::fs::create_dir(&folder).unwrap();
    let file = folder.join("aperto.txt");
    std::fs::write(&file, "prima").unwrap();
    app.set_project_root(root.clone());
    let doc = app.workspace.open(Some(file.clone()), "prima");
    app.groups.open(crate::editor_groups::GroupId::Primary, doc);
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    assert_eq!(
        app.albero_elimina
            .as_ref()
            .expect("conferma cartella")
            .schede,
        vec![doc]
    );
    assert!(folder.is_dir());
    let Ok(crate::scritture::Permesso::Avvia(lease)) =
        app.scritture.ammetti(crate::scritture::Op::Salva {
            doc,
            versione: 0,
            causa: crate::scritture::Causa::Utente,
        })
    else {
        panic!("save non ammesso")
    };
    let _ = app.scritture.seguito();
    let _ = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    assert!(!app.albero_elimina.as_ref().unwrap().in_corso);
    assert!(app.notice.as_ref().unwrap().1.text.contains("scrittura"));
    app.scritture
        .rilascia(lease, crate::scritture::Esito::NonAvviato);
    let _ = app.scritture.seguito();
    let _ = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Annulla,
    ));
    assert!(folder.is_dir());
    menu_su_cartella(&mut app, &root);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    assert!(app.albero_elimina.is_none());
    assert!(root.is_dir());
}

#[cfg(target_os = "linux")]
#[test]
fn cartella_worker_reale_chiude_solo_le_schede_interessate_dopo_il_cestino() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    use futures::StreamExt;
    let isolated = std::env::var_os("LUME_TEST_CARTELLA_CESTINO");
    let Some(isolated) = isolated else {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "app::tests::albero_operations::cartella_worker_reale_chiude_solo_le_schede_interessate_dopo_il_cestino", "--test-threads=1", "--nocapture"])
            .env("LUME_TEST_CARTELLA_CESTINO", dir.path())
            .env("XDG_DATA_HOME", dir.path().join("xdg"))
            .status().unwrap();
        assert!(status.success());
        return;
    };
    let base = std::path::PathBuf::from(isolated);
    let root = base.join("progetto");
    let folder = root.join("moduli");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(root.join("moduli2")).unwrap();
    let file = folder.join("aperto.txt");
    let sibling = root.join("moduli2/aperto.txt");
    let external = base.join("esterno.txt");
    std::fs::write(&file, "prima").unwrap();
    std::fs::write(&sibling, "fratello").unwrap();
    std::fs::write(&external, "esterno").unwrap();
    std::os::unix::fs::symlink(&external, folder.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(&file, root.join("alias.txt")).unwrap();
    let mut app = app();
    app.set_project_root(root.clone());
    let doc = app.workspace.open(Some(file.clone()), "prima");
    let alias = app.workspace.open(Some(root.join("alias.txt")), "prima");
    let linked = app.workspace.open(Some(folder.join("link.txt")), "esterno");
    let tardiva = folder.join("tardiva.txt");
    std::fs::write(&tardiva, "ritardata").unwrap();
    let before_start =
        primo_output(app.open_file_task(tardiva.clone(), crate::file_read::DopoApertura::Niente));
    let during =
        primo_output(app.open_file_task(tardiva.clone(), crate::file_read::DopoApertura::Niente));
    assert!(matches!(&before_start, Message::FileReadFinished(_, None, r) if r.esito.is_ok()));
    assert!(matches!(&during, Message::FileReadFinished(_, None, r) if r.esito.is_ok()));
    let other = app.workspace.open(Some(sibling.clone()), "fratello");
    for id in [doc, alias, linked, other] {
        app.groups.open(crate::editor_groups::GroupId::Primary, id);
    }
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    assert!(app.riquadro_elimina().unwrap().cartella);
    let task = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    let _ = app.update(before_start);
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("Lettura superata")
    );
    assert!(
        app.workspace.get_by_path(&tardiva).is_none(),
        "lettura precedente all'avvio invalidata"
    );
    assert!(app.groups.contains(doc));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let message = runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        match tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
        {
            iced_runtime::Action::Output(
                message @ Message::AlberoEliminato(_, Ok(lume_core::TrashOutcome::Trashed)),
            ) => message,
            other => panic!("esito inatteso: {other:?}"),
        }
    });
    assert!(!folder.exists());
    assert!(
        app.groups.contains(doc),
        "la chiusura aspetta la ricevuta del worker"
    );
    let _ = app.update(message);
    let _ = app.update(during);
    assert!(
        app.workspace.get_by_path(&tardiva).is_none(),
        "lettura durante il worker invalidata alla conclusione"
    );
    for id in [doc, alias, linked] {
        assert!(!app.groups.contains(id));
    }
    assert!(app.groups.contains(other));
    assert_eq!(std::fs::read_to_string(&sibling).unwrap(), "fratello");
    assert_eq!(std::fs::read_to_string(&external).unwrap(), "esterno");
    assert_eq!(
        std::fs::read_to_string(base.join("xdg/Trash/files/moduli/aperto.txt")).unwrap(),
        "prima"
    );
    assert!(base.join("xdg/Trash/info/moduli.trashinfo").is_file());
    // Cestino irraggiungibile: nessuna cancellazione, scheda e conferma restano.
    // Il secondo processo ha un ambiente isolato e immutabile: il backend
    // non può creare Trash se il suo percorso padre è stato reso un file.
    std::fs::rename(base.join("xdg"), base.join("xdg-preservato")).unwrap();
    std::fs::write(base.join("xdg"), "bloccato").unwrap();
    menu_su_cartella(&mut app, &root.join("moduli2"));
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let task = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    let message = runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        match tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
        {
            iced_runtime::Action::Output(message @ Message::AlberoEliminato(_, Err(_))) => message,
            other => panic!("esito inatteso: {other:?}"),
        }
    });
    let _ = app.update(message);
    assert!(app.riquadro_elimina().unwrap().errore.is_some());
    assert!(app.groups.contains(other));
    assert_eq!(std::fs::read_to_string(sibling).unwrap(), "fratello");
}

#[test]
fn cartella_rifiuta_input_differito_sul_discendente_ripristinato() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("moduli");
    std::fs::create_dir(&folder).unwrap();
    let active = root.path().join("attiva.txt");
    let deferred = folder.join("differita.txt");
    std::fs::write(&active, "attiva").unwrap();
    std::fs::write(&deferred, "prima").unwrap();
    let mut app = app();
    let mut snapshot = lume_store::SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_owned());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);
    let id = app.workspace.get_by_path(&deferred).unwrap();
    let read = app.activate_tab(id);
    let output = primo_output(read); // reader reale pronto, consegna trattenuta
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("x".into()),
    ));
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    assert!(
        app.albero_elimina.is_none(),
        "input in coda equivale a modifiche da non perdere"
    );
    let _ = app.update(output);
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "xprima");
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[cfg(unix)]
#[test]
fn cartella_rifiuta_alias_sporco_anche_se_il_file_e_scomparso() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("moduli");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), "prima").unwrap();
    std::os::unix::fs::symlink(&folder, root.path().join("alias")).unwrap();
    let mut app = app();
    app.set_project_root(root.path().to_owned());
    let id = app
        .workspace
        .open(Some(root.path().join("alias/a.txt")), "prima");
    app.groups.open(crate::editor_groups::GroupId::Primary, id);
    app.workspace
        .get_mut(id)
        .unwrap()
        .apply(lume_core::Transaction {
            changes: lume_core::ChangeSet::replace_range(0, 0, "x", 5),
            selection_after: None,
        });
    std::fs::remove_file(folder.join("a.txt")).unwrap();
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    assert!(app.albero_elimina.is_none());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("modifiche non salvate")
    );
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "xprima");
}

#[test]
fn la_conferma_di_un_file_non_autorizza_una_cartella_sostituita_dall_esterno() {
    let mut app = app();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("elemento");
    std::fs::write(&path, "file").unwrap();
    app.set_project_root(root.path().to_owned());
    menu_su_file(&mut app, &path);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("nuovo.txt"), "preserva").unwrap();
    let _ = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    assert!(!app.albero_elimina.as_ref().unwrap().in_corso);
    assert!(app.albero_elimina.as_ref().unwrap().errore.is_some());
    assert_eq!(
        std::fs::read_to_string(path.join("nuovo.txt")).unwrap(),
        "preserva"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn uscite_menu_raggiungono_richiesta_durante_cestino_e_chiusura() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::uscite_menu_raggiungono_richiesta_durante_cestino_e_chiusura",
    ) else {
        return;
    };
    let (mut app, folder, _, altro, task) = prepara_cestino(&base);
    let richiesta = app.albero_elimina.clone().unwrap();
    let lease = richiesta.lease.unwrap();
    let window = iced::window::Id::unique();
    let _ = app.update(Message::WindowCloseRequested(window));
    assert_eq!(
        app.shell.active_overlay(),
        Some(OverlayId::ConfermaChiusura)
    );
    let epoch = app.close_epoch;
    // Input già accodato da un caricamento: richiesta_uscita segnala questo
    // stato, distinguendo il suo percorso dalla guardia Cestino precedente.
    app.restored_tab_commands
        .entry(altro)
        .or_default()
        .push_back(EditorCommand::Insert("attesa".into()));
    for msg in [
        Message::MenuAction(MenuAct::Exit),
        Message::MenuBar(crate::menu::MenuBarMessage::Activate(MenuAct::Exit)),
    ] {
        app.notice = None;
        let _ = app.update(msg);
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("Input in attesa"))
        );
        assert_eq!(app.close_epoch, epoch);
        assert_eq!(
            app.shell.active_overlay(),
            Some(OverlayId::ConfermaChiusura)
        );
        assert_eq!(app.albero_elimina.as_ref(), Some(&richiesta));
        assert!(
            app.scritture
                .possiede(lease, crate::scritture::Tipo::Cestino)
        );
    }
    app.restored_tab_commands.remove(&altro);
    let _ = app.update(Message::MenuAction(MenuAct::GotoLine));
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, n)| n.text.contains("Rispondi prima"))
    );
    assert_eq!(consuma_task_cestino(&mut app, task), 1);
    assert!(!folder.exists());
    assert!(
        !app.scritture
            .possiede(lease, crate::scritture::Tipo::Cestino)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn ponte_pty_reale_drena_durante_cestino_e_chiusura() {
    lume_testkit::richiedi!(lume_testkit::Strumento::EseguibileCorrente);
    lume_testkit::richiedi!(lume_testkit::Strumento::ShellPty);
    let Some(base) = cestino_isolato(
        "app::tests::albero_operations::ponte_pty_reale_drena_durante_cestino_e_chiusura",
    ) else {
        return;
    };
    use crate::features::Feature;
    use futures::StreamExt;
    let root = base.join("progetto");
    let folder = root.join("moduli");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("aperto.txt");
    let altro_path = root.join("altro.txt");
    std::fs::write(&file, "prima").unwrap();
    std::fs::write(&altro_path, "altro").unwrap();
    let mut app = app();
    app.set_project_root(root.clone());
    let doc = app.workspace.open(Some(file), "prima");
    let altro = app.workspace.open(Some(altro_path), "altro");
    app.groups.open(crate::editor_groups::GroupId::Primary, doc);
    app.groups
        .open(crate::editor_groups::GroupId::Primary, altro);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("X".into()),
    ));
    let invia = |app: &mut Lume, evento| {
        app.update(Message::Tool(tools::terminal::ID, ToolEvent::new(evento)))
    };
    let _ = invia(&mut app, tools::terminal::Event::NewSession);
    assert_eq!(app.feature_live_count(Feature::Terminal), Some(1));
    let prima = base.join("pty-prima.txt");
    let riprendi = base.join("pty-riprendi.txt");
    let output = base.join("pty-output.txt");
    // La shell vera aspetta un segnale del test: le due emissioni PTY sono
    // separate dalla raccolta della ricevuta Cestino, con attesa limitata.
    let comando = format!(
        "sleep 0.2; printf 'prima' > \"{}\"; printf 'prima\\n'; i=0; while [ ! -f \"{}\" ] && [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done; printf 'dopo\\n'; printf 'ponte-reale' > \"{}\"; exit 7\r",
        prima.display(),
        riprendi.display(),
        output.display()
    );
    let _ = invia(
        &mut app,
        tools::terminal::Event::Input(comando.into_bytes()),
    );
    assert!(app.terminal_needs_wakeups());
    menu_su_cartella(&mut app, &folder);
    let _ = app.update(Message::MenuAction(MenuAct::DeleteTreeFile));
    let task = app.update(Message::AlberoElimina(
        crate::conferma_elimina::ConfermaMessage::Elimina,
    ));
    let richiesta = app.albero_elimina.clone().unwrap();
    let lease = richiesta.lease.unwrap();
    let window = iced::window::Id::unique();
    let _ = app.update(Message::WindowCloseRequested(window));
    assert_eq!(
        app.shell.active_overlay(),
        Some(OverlayId::ConfermaChiusura)
    );
    app.notice = None;
    runtime_file_tests().block_on(async {
        let mut trash = iced_runtime::task::into_stream(task).unwrap();
        let raccolta = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(10), async move {
                while let Some(azione) = trash.next().await {
                    if let iced_runtime::Action::Output(m @ Message::AlberoEliminato(..)) = azione { return m; }
                }
                panic!("ricevuta Cestino assente")
            }).await.unwrap()
        });
        let (tx, mut rx) = futures::channel::mpsc::channel(8);
        let ponte = tokio::spawn(super::super::terminal_wakeups(app.terminal_wakeup.clone(), tx));
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut tick = 0;
        while tick == 0 || !prima.exists() {
            let msg = tokio::time::timeout_at(deadline, rx.next()).await.unwrap().unwrap();
            assert!(matches!(&msg, Message::Tool(id, event) if *id == tools::terminal::ID && matches!(event.downcast::<tools::terminal::Event>(), Some(tools::terminal::Event::Tick))));
            let _ = app.update(msg);
            tick += 1;
            assert!(app.notice.is_none(), "un tick non deve produrre avvisi");
            assert_eq!(app.albero_elimina.as_ref(), Some(&richiesta));
            assert!(app.scritture.possiede(lease, crate::scritture::Tipo::Cestino));
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConfermaChiusura));
        }
        assert_eq!(std::fs::read_to_string(&prima).unwrap(), "prima");
        let ricevuta = raccolta.await.unwrap();
        let _ = app.update(ricevuta);
        assert!(!app.scritture.possiede(lease, crate::scritture::Tipo::Cestino));
        assert!(!folder.exists());
        assert!(app.workspace.get(altro).unwrap().is_dirty());
        assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConfermaChiusura));
        assert_eq!(app.feature_live_count(Feature::Terminal), Some(1));
        let avviso_dopo_ricevuta = app.notice.as_ref().map(|(id, _)| *id);
        let tick_prima = tick;
        std::fs::write(&riprendi, "riprendi").unwrap();
        while app.feature_live_count(Feature::Terminal) != Some(0) {
            let msg = tokio::time::timeout_at(deadline, rx.next()).await.unwrap().unwrap();
            assert!(matches!(&msg, Message::Tool(id, event) if *id == tools::terminal::ID && matches!(event.downcast::<tools::terminal::Event>(), Some(tools::terminal::Event::Tick))));
            let _ = app.update(msg);
            tick += 1;
            assert_eq!(app.notice.as_ref().map(|(id, _)| *id), avviso_dopo_ricevuta);
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::ConfermaChiusura));
        }
        assert!(tick > tick_prima, "il ponte riceve risvegli dopo la raccolta");
        assert_eq!(std::fs::read_to_string(&output).unwrap(), "ponte-reale");
        assert!(app.terminal_needs_wakeups(), "il ponte resta disponibile finché il PTY è posseduto");
        ponte.abort();
    });
}
