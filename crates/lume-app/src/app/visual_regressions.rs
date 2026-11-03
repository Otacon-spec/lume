//! Prove del widget e dei comandi UI, senza finestra o database reali.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use super::*;

#[test]
fn informazioni_apre_un_riquadro_visibile() {
    let (mut app, _) = Lume::new_without_store();
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::About));
    assert!(
        app.shell.active_overlay().is_some(),
        "Informazioni non deve essere soltanto una notifica in status bar"
    );
}

#[test]
fn informazioni_blocca_i_comandi_del_progetto_sottostante() {
    let (mut app, _) = Lume::new_without_store();
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::About));
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::NewFile));
    assert!(app.active_doc().is_none());
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, n)| n.kind == NoticeKind::Info && n.text.contains("Rispondi prima"))
    );
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
    let _ = app.update(Message::CloseAll);
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::NewFile));
    assert!(app.active_doc().is_some());
}

#[test]
fn scorciatoie_blocca_la_modifica_dell_editor_con_avviso() {
    let (mut app, _) = Lume::new_without_store();
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::NewFile));
    let id = app.active_doc().unwrap();
    let prima = app.workspace.get(id).unwrap().rope().to_string();
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::Shortcuts));
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("modifica vietata".into()),
    ));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), prima);
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Scorciatoie));
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, n)| n.text.contains("Rispondi prima"))
    );
}

#[test]
fn informazioni_blocca_anche_doppio_shift() {
    let (mut app, _) = Lume::new_without_store();
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::About));
    for _ in 0..2 {
        let _ = app.update(Message::GlobalSearch(
            crate::search::GlobalSearchMessage::ShiftPressed,
        ));
    }
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
    assert!(!app.global_search.is_open());
}

#[test]
fn disposizione_legacy_non_nasconde_appunti_e_altri_pannelli() {
    let (mut app, _) = Lume::new_without_store();
    let snapshot = lume_store::SessionSnapshot {
        left_tools: vec!["project".into()],
        right_tools: vec!["history".into()],
        ..lume_store::SessionSnapshot::empty()
    };
    let _ = app.restore_snapshot_async(snapshot);
    assert!(
        app.registry
            .bar(lume_ui::tool::Side::Right)
            .contains(&tools::notes::ID)
    );
    assert!(
        app.registry
            .bar(lume_ui::tool::Side::Right)
            .contains(&tools::structure::ID)
    );
    assert!(
        app.registry
            .bar(lume_ui::tool::Side::Left)
            .contains(&tools::usages::ID)
    );
}

#[test]
fn menu_destro_disponibile_senza_language_server() {
    let (app, _) = Lume::new_without_store();
    let items = crate::context_menu::items(
        &crate::context_menu::Soggetto::Editor {
            pos: 0,
            selezione: None,
        },
        app.menu_checks(),
    );
    for action in [
        crate::menu::MenuAct::Cut,
        crate::menu::MenuAct::Copy,
        crate::menu::MenuAct::Paste,
    ] {
        assert!(items.iter().any(|(_, a)| *a == action));
    }
}

#[test]
fn clic_destro_dentro_selezione_non_la_cancella() {
    let (mut app, _) = Lume::new_without_store();
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::NewFile));
    let id = app.active_doc().expect("documento nuovo");
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("abcdef".into()),
    ));
    app.workspace
        .get_mut(id)
        .expect("documento")
        .set_selection(lume_core::Range::new(1, 4));
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::ContextMenu {
            pos: 2,
            screen_x: 100,
            screen_y: 100,
        },
    ));
    assert_eq!(
        app.workspace.get(id).expect("documento").selection(),
        lume_core::Range::new(1, 4)
    );
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Context));
}

#[test]
fn scroll_orizzontale_non_cambia_i_pixel_del_gutter() {
    use iced::advanced::{Widget, widget::Tree};
    use lume_ui::editor::{EditorState, EditorView};
    let doc = lume_core::Document::from_str(
        &format!("{}\n{}\n", "M".repeat(180), "W".repeat(180)),
        None,
        lume_core::DocId(912),
    );
    let theme = lume_theme::LumeTheme::builtin().remove(0);
    let mut view = EditorView::new(&doc, None, &theme, &[], |_| ());
    let mut renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
        iced::Font::default(),
        iced::Pixels(16.0),
    ));
    let mut tree = Tree::new(&view as &dyn Widget<(), iced::Theme, iced::Renderer>);
    let size = iced::Size::new(800.0, 160.0);
    let bounds = iced::Rectangle::with_size(size);
    let node = view.layout(
        &mut tree,
        &renderer,
        &iced::advanced::layout::Limits::new(iced::Size::ZERO, size),
    );
    let render = |view: &EditorView<'_, ()>, tree: &Tree, renderer: &mut iced::Renderer| {
        use iced::advanced::Renderer as _;
        renderer.reset(bounds);
        view.draw(
            tree,
            renderer,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style::default(),
            iced::advanced::Layout::new(&node),
            iced::mouse::Cursor::Unavailable,
            &bounds,
        );
        let iced::Renderer::Secondary(software) = renderer else {
            unreachable!()
        };
        iced_tiny_skia::window::compositor::screenshot(
            software,
            &iced_tiny_skia::graphics::Viewport::with_physical_size(iced::Size::new(800, 160), 1.0),
            theme.bg,
        )
    };
    let baseline = render(&view, &tree, &mut renderer);
    tree.state.downcast_ref::<EditorState>().scroll_x.set(123.0);
    let shifted = render(&view, &tree, &mut renderer);
    for y in 0..160 {
        let start = y * 800 * 4;
        assert_eq!(
            &baseline[start..start + 58 * 4],
            &shifted[start..start + 58 * 4],
            "testo passato nel gutter alla riga pixel {y}"
        );
    }
}

// Caratterizzazione della coda con uno store reale: i risultati SQL della
// console non sono prodotti da questa fixture, quindi SessionSaved è snapshot.
#[tokio::test]
async fn persistenza_baseline_task_e_sqlite_reali() {
    use futures::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let store = lume_store::StoreHandle::open_at(dir.path().join("session.sqlite3"))
        .await
        .unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.store = Some(store.clone());
    for (messaggio, attesi) in [
        (
            Message::EditorTabBar(EditorTabBarMessage::Hover(
                crate::editor_groups::GroupId::Primary,
                None,
            )),
            0,
        ),
        (
            Message::EditorTabBar(EditorTabBarMessage::Press(
                crate::editor_groups::GroupId::Primary,
                DocId(123),
            )),
            0,
        ),
        (
            Message::EditorTabBar(EditorTabBarMessage::CursorMoved(Point::ORIGIN)),
            0,
        ),
        (
            Message::EditorTabBar(EditorTabBarMessage::Close(DocId(123))),
            1,
        ),
        (Message::EditorTabBar(EditorTabBarMessage::Release), 1),
        (Message::DragEnd, 1),
        (Message::Dock(DockMessage::FeatureStopCancelled), 1),
        (Message::SessionSaved(Ok(())), 0),
    ] {
        let task = app.update_inner(messaggio);
        let mut conteggio = 0;
        if let Some(mut stream) = iced_runtime::task::into_stream(task) {
            while let Some(azione) =
                tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                    .await
                    .unwrap()
            {
                if let iced_runtime::Action::Output(Message::SessionSaved(esito)) = azione {
                    esito.unwrap();
                    conteggio += 1;
                }
            }
        }
        assert_eq!(conteggio, attesi);
    }
    let (salvata, _) = store.load_session().await.unwrap();
    assert_eq!(salvata, app.session_snapshot());
}

#[tokio::test]
async fn modali_scartano_gesti_persistenti_e_sfondo_senza_scrivere() {
    use futures::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let store = lume_store::StoreHandle::open_at(dir.path().join("session.sqlite3"))
        .await
        .unwrap();
    for overlay in [OverlayId::About, OverlayId::Scorciatoie] {
        let (mut app, _) = Lume::new_without_store();
        app.store = Some(store.clone());
        let file = dir.path().join("buffer.txt");
        std::fs::write(&file, "testo invariato\n").unwrap();
        app.open_file(&file).unwrap();
        let doc = app.active_doc().unwrap();
        let buffer = app.workspace.get(doc).unwrap().rope().to_string();
        let prima = app.session_snapshot();
        store.save_session(prima.clone()).await.unwrap();
        app.apri_overlay(overlay);
        let task = app.update(Message::ShowExtensions);
        assert!(
            app.notice.as_ref().is_some_and(
                |(_, n)| n.kind == NoticeKind::Info && n.text.contains("Rispondi prima")
            )
        );
        let avviso = app.notice.as_ref().unwrap().0;
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let mut scritture = 0;
        while let Some(azione) =
            tokio::time::timeout(std::time::Duration::from_secs(7), stream.next())
                .await
                .unwrap()
        {
            if let iced_runtime::Action::Output(Message::SessionSaved(_)) = azione {
                scritture += 1;
            }
        }
        assert_eq!(scritture, 0);
        for msg in [
            Message::GlobalSearch(GlobalSearchMessage::ShiftPressed),
            Message::GlobalSearch(GlobalSearchMessage::ShiftPressed),
            Message::DragEnd,
            Message::EditorTabBar(EditorTabBarMessage::CursorMoved(Point::ORIGIN)),
            Message::EditorTabBar(EditorTabBarMessage::Hover(
                crate::editor_groups::GroupId::Primary,
                Some(doc),
            )),
        ] {
            if let Some(mut stream) = iced_runtime::task::into_stream(app.update(msg)) {
                while let Some(azione) =
                    tokio::time::timeout(std::time::Duration::from_secs(7), stream.next())
                        .await
                        .unwrap()
                {
                    assert!(!matches!(
                        azione,
                        iced_runtime::Action::Output(Message::SessionSaved(_))
                    ));
                }
            }
        }
        assert_eq!(app.notice.as_ref().unwrap().0, avviso);
        assert!(!app.global_search.is_open());
        assert_eq!(app.shell.active_overlay(), Some(overlay));
        assert_eq!(app.session_snapshot(), prima);
        assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), buffer);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "testo invariato\n");
        assert_eq!(store.load_session().await.unwrap().0, prima);
    }
}

#[tokio::test]
async fn folder_picked_persistenza_anticipata_una_sola_volta() {
    use futures::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("progetto");
    std::fs::create_dir(&root).unwrap();
    let store = lume_store::StoreHandle::open_at(dir.path().join("session.sqlite3"))
        .await
        .unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.store = Some(store.clone());
    let mut stream =
        iced_runtime::task::into_stream(app.update(Message::FolderPicked(Some(root.clone()))))
            .unwrap();
    let mut scritture = 0;
    while let Some(azione) = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next())
        .await
        .unwrap()
    {
        if let iced_runtime::Action::Output(Message::SessionSaved(result)) = azione {
            result.unwrap();
            scritture += 1;
        }
    }
    assert_eq!(scritture, 1);
    assert_eq!(
        store.load_session().await.unwrap().0.workspace_root,
        Some(root)
    );
}

#[test]
fn comandi_locali_e_risposte_estranee_non_chiudono_un_modale() {
    let (mut app, _) = Lume::new_without_store();
    app.apri_overlay(OverlayId::About);
    for msg in [
        Message::MenuBar(MenuBarMessage::Escape),
        Message::GlobalSearch(GlobalSearchMessage::Escape),
        Message::EditorIn(
            crate::editor_groups::GroupId::Primary,
            EditorCommand::PopupDismiss,
        ),
        Message::AlberoElimina(crate::conferma_elimina::ConfermaMessage::Annulla),
        Message::ConfermaChiusura(crate::conferma_chiusura::ConfermaMessage::Annulla),
        Message::MenuAction(crate::menu::MenuAct::Save),
    ] {
        let _ = app.update(msg);
        assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(_, n)| n.text.contains("Rispondi prima"))
        );
    }
}

#[tokio::test]
async fn chiusure_finestra_persistono_prima_dell_azione_close() {
    use futures::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let store = lume_store::StoreHandle::open_at(dir.path().join("session.sqlite3"))
        .await
        .unwrap();
    for tipo in 0..3 {
        let (mut app, _) = Lume::new_without_store();
        app.store = Some(store.clone());
        app.apri_overlay(OverlayId::About);
        let id = iced::window::Id::unique();
        let messaggio = match tipo {
            0 => Message::WindowCloseRequested(id),
            1 => Message::MenuAction(crate::menu::MenuAct::Exit),
            _ => Message::Titlebar(TitlebarMessage::Close),
        };
        let mut stream = iced_runtime::task::into_stream(app.update(messaggio)).unwrap();
        let primo = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            primo,
            iced_runtime::Action::Output(Message::SessionSaved(Ok(())))
        ));
        let secondo = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap();
        match secondo {
            iced_runtime::Action::Window(iced_runtime::window::Action::Close(destinazione)) => {
                assert_eq!(destinazione, id)
            }
            iced_runtime::Action::Window(iced_runtime::window::Action::GetLatest(risposta)) => {
                risposta.send(Some(id)).unwrap();
                let terzo = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                    .await
                    .unwrap()
                    .unwrap();
                assert!(
                    matches!(terzo, iced_runtime::Action::Window(iced_runtime::window::Action::Close(destinazione)) if destinazione == id)
                );
            }
            _ => panic!("azione finestra inattesa"),
        }
    }
}

#[test]
fn ctrl_g_non_sostituisce_la_domanda_di_chiusura_sporca() {
    let (mut app, _) = Lume::new_without_store();
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::NewFile));
    let id = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("testo".into()),
    ));
    let _ = app.update(Message::EditorTabBar(EditorTabBarMessage::Close(id)));
    assert_eq!(
        app.shell.active_overlay(),
        Some(OverlayId::ConfermaChiusura)
    );
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::GotoLine));
    assert_eq!(
        app.shell.active_overlay(),
        Some(OverlayId::ConfermaChiusura)
    );
    assert!(app.chiusura_attiva().is_some());
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, n)| n.text.contains("Rispondi prima"))
    );
}

#[cfg(feature = "mcp")]
#[test]
fn avvio_mcp_sotto_altro_modale_non_crea_consenso() {
    let (mut app, _) = Lume::new_without_store();
    app.apri_overlay(OverlayId::About);
    let _ = app.update(Message::Feature(crate::features::FeatureCommand::Start(
        crate::features::Feature::Mcp,
    )));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
    assert!(app.riquadro_consenso_mcp().is_none());
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, n)| n.text.contains("Rispondi prima"))
    );
}

#[test]
fn uscita_da_menu_non_cancella_la_domanda_di_chiusura_in_corso() {
    let (mut app, _) = Lume::new_without_store();
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::NewFile));
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("modifica".into()),
    ));
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::Exit));
    assert_eq!(
        app.shell.active_overlay(),
        Some(OverlayId::ConfermaChiusura)
    );
    let _ = app.update(Message::MenuBar(MenuBarMessage::Activate(
        crate::menu::MenuAct::Exit,
    )));
    assert_eq!(
        app.shell.active_overlay(),
        Some(OverlayId::ConfermaChiusura)
    );
    assert!(app.chiusura_attiva().is_some());
}

#[cfg(feature = "sql")]
#[tokio::test]
async fn ricevuta_sql_non_riscrive_lo_snapshot_di_sessione() {
    use futures::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let store = lume_store::StoreHandle::open_at(dir.path().join("session.sqlite3"))
        .await
        .unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.store = Some(store.clone());
    let mut snapshot = app.session_snapshot();
    snapshot.workspace_root = Some(dir.path().join("sentinella"));
    store.save_session(snapshot.clone()).await.unwrap();
    let record = lume_store::SqlConnectionRecord {
        id: "prova".into(),
        label: "Prova".into(),
        engine: "azure-sql".into(),
        host: "localhost".into(),
        database: "db".into(),
        auth: "sql-login".into(),
        username: Some("utente".into()),
        tenant_id: None,
        client_id: None,
    };
    let mut stream =
        iced_runtime::task::into_stream(app.persist_sql_connection(record.clone())).unwrap();
    let iced_runtime::Action::Output(ricevuta @ Message::SessionSaved(Ok(()))) =
        tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
    else {
        panic!("ricevuta SQL assente")
    };
    assert_eq!(store.load_sql_connections().await.unwrap(), vec![record]);
    let task = app.update(ricevuta);
    if let Some(mut stream) = iced_runtime::task::into_stream(task) {
        while let Some(azione) =
            tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                .await
                .unwrap()
        {
            assert!(!matches!(
                azione,
                iced_runtime::Action::Output(Message::SessionSaved(_))
            ));
        }
    }
    assert_eq!(store.load_session().await.unwrap().0, snapshot);
}

#[test]
fn doppio_shift_e_sfondo_non_cambiano_nessun_modale() {
    for overlay in [
        OverlayId::About,
        OverlayId::Scorciatoie,
        OverlayId::ConfermaChiusura,
        OverlayId::ConfermaElimina,
        #[cfg(feature = "mcp")]
        OverlayId::ConsensoMcp,
        #[cfg(feature = "mcp")]
        OverlayId::CardMcp,
    ] {
        let (mut app, _) = Lume::new_without_store();
        app.apri_overlay(overlay);
        app.notice = None;
        for _ in 0..2 {
            let _ = app.update_inner(Message::GlobalSearch(GlobalSearchMessage::ShiftPressed));
        }
        assert_eq!(app.shell.active_overlay(), Some(overlay));
        assert!(!app.global_search.is_open());
        assert!(app.notice.is_none());
    }
}

#[tokio::test]
async fn coda_comune_scrive_una_volta_lo_snapshot_aggiornato() {
    use futures::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let store = lume_store::StoreHandle::open_at(dir.path().join("session.sqlite3"))
        .await
        .unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.store = Some(store.clone());
    let prima = app.session_snapshot();
    store.save_session(prima.clone()).await.unwrap();
    let mut stream = iced_runtime::task::into_stream(app.update(Message::ShowExtensions)).unwrap();
    let attesa = app.session_snapshot();
    assert_ne!(attesa, prima);
    let mut scritture = 0;
    while let Some(azione) = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .unwrap()
    {
        if let iced_runtime::Action::Output(Message::SessionSaved(esito)) = azione {
            esito.unwrap();
            scritture += 1;
        }
    }
    assert_eq!(scritture, 1);
    assert_eq!(store.load_session().await.unwrap().0, attesa);
}

#[test]
fn apertura_modale_annulla_drag_senza_click_o_drop() {
    let (mut app, _) = Lume::new_without_store();
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::NewFile));
    let id = app.active_doc().unwrap();
    let _ = app.update(Message::DragStart(crate::layout::HandleKind::Panel));
    let _ = app.update(Message::EditorTabBar(EditorTabBarMessage::Press(
        crate::editor_groups::GroupId::Primary,
        id,
    )));
    let _ = app.update(Message::ActivityBar(ActivityBarMessage::Press(
        tools::notes::ID,
    )));
    assert!(app.layout.drag.is_some());
    assert!(app.editor_tab_bar.drag().is_some());
    assert!(app.activity_bar.drag().is_some());
    let prima = app.session_snapshot();
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::About));
    assert!(app.layout.drag.is_none());
    assert!(app.editor_tab_bar.drag().is_none());
    assert!(app.activity_bar.drag().is_none());
    let _ = app.update(Message::DragEnd);
    let _ = app.update(Message::CloseAll);
    assert_eq!(app.session_snapshot(), prima);
}

#[cfg(feature = "sql")]
#[tokio::test]
async fn folder_e_avvio_catturano_snapshot_prima_del_drenaggio_reale() {
    use futures::StreamExt;
    for avvio in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("progetto");
        std::fs::create_dir(&root).unwrap();
        let store = lume_store::StoreHandle::open_at(dir.path().join("session.sqlite3"))
            .await
            .unwrap();
        let (mut app, _) = Lume::new_without_store();
        app.store = Some(store.clone());
        let profile = lume_sql::ConnectionProfile {
            id: "ordine".into(),
            label: "Ordine".into(),
            engine: lume_sql::Engine::AzureSql,
            host: "127.0.0.1:1".into(),
            database: "prova".into(),
            auth: lume_sql::AuthKind::SqlLogin {
                user: "prova".into(),
            },
        };
        let mut workspace = crate::sql_mode::SqlWorkspace::new();
        workspace
            .open(
                profile,
                lume_sql::Secret::new(String::new()),
                lume_sql::open_driver,
            )
            .unwrap();
        app.sql = Some(workspace);
        assert!(app.session_snapshot().sql_console.is_some());
        app.pending_tool_requests
            .push((tools::db::ID, ToolRequest::StopFeature("database")));
        let messaggio = if avvio {
            let file = root.join("aperto.txt");
            std::fs::write(&file, "contenuto\n").unwrap();
            Message::FileLetto(crate::file_read::leggi_per_apertura(
                file,
                crate::file_read::DopoApertura::Avvio {
                    fuori_progetto: true,
                },
            ))
        } else {
            Message::FolderPicked(Some(root.clone()))
        };
        let task = app.update(messaggio);
        assert!(
            app.sql.is_none(),
            "il drenaggio ha fermato il workspace SQL"
        );
        assert!(app.session_snapshot().sql_console.is_none());
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let mut scritture = 0;
        while let Some(azione) =
            tokio::time::timeout(std::time::Duration::from_secs(10), stream.next())
                .await
                .unwrap()
        {
            if let iced_runtime::Action::Output(Message::SessionSaved(esito)) = azione {
                esito.unwrap();
                scritture += 1;
            }
        }
        assert_eq!(
            scritture, 1,
            "il drenaggio non salva record SQL né snapshot"
        );
        let (salvata, _) = store.load_session().await.unwrap();
        assert_eq!(salvata.workspace_root, Some(root));
        assert!(
            salvata.sql_console.is_some(),
            "la cattura precede il drenaggio"
        );
    }
}
