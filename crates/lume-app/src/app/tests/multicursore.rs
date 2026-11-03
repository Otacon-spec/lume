use super::*;
use lume_core::{Range, SelectionSet};
use lume_ui::editor::{Motion, PageDirection};

#[test]
fn multi_app_edit_movimenti_clipboard_e_due_viste() {
    let mut app = app();
    let dir = tempfile::tempdir().unwrap();
    write_and_open(&mut app, dir.path(), "multi.txt", "abc def");
    let id = app.active_doc().unwrap();
    app.workspace.get_mut(id).unwrap().set_selections(
        SelectionSet::try_new(vec![Range::point(1), Range::point(5)], 1, 7).unwrap(),
    );
    assert!(app.groups.split());
    app.apply_editor_command_for(id, EditorCommand::Insert("X".into()));
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "aXbc dXef"
    );
    assert_eq!(
        app.workspace.get(id).unwrap().selections().ranges(),
        &[Range::point(2), Range::point(7)]
    );
    app.editor_apply_motion(id, Motion::Left, true);
    assert_eq!(
        app.workspace.get(id).unwrap().selections().ranges(),
        &[Range::new(2, 1), Range::new(7, 6)]
    );
    assert_eq!(
        lume_core::multi_edit::selected_text(app.workspace.get(id).unwrap()).as_deref(),
        Some("X\nX")
    );
    app.apply_editor_command_for(id, EditorCommand::Undo);
    assert_eq!(
        app.workspace.get(id).unwrap().selections().ranges(),
        &[Range::point(1), Range::point(5)]
    );
    app.apply_editor_command_for(id, EditorCommand::Redo);
    app.editor_apply_page(id, PageDirection::Up, 2, false);
    assert_eq!(
        app.workspace.get(id).unwrap().selections().ranges().len(),
        2
    );
}

#[test]
fn multi_worker_reale_rifiuta_selezione_binding_e_scope_stantii() {
    for invalidation in 0..5 {
        let mut app = app();
        let root = tempfile::tempdir().unwrap();
        write_and_open(&mut app, root.path(), "worker.txt", "foo foo foo");
        let id = app.active_doc().unwrap();
        app.workspace
            .get_mut(id)
            .unwrap()
            .set_selection(Range::new(0, 3));
        let response = primo_output_opzionale(app.next_occurrence_for(id)).expect("worker reale");
        let ticket = app.multi_occurrence.as_ref().unwrap().sequence;
        let duplicate = app.next_occurrence_for(id);
        assert!(iced_runtime::task::into_stream(duplicate).is_none());
        assert_eq!(app.multi_occurrence.as_ref().unwrap().sequence, ticket);
        assert!(app.notice.as_ref().unwrap().1.text.contains("corso"));
        match invalidation {
            0 => app
                .workspace
                .get_mut(id)
                .unwrap()
                .set_selection(Range::point(2)),
            1 => app
                .workspace
                .rebind_path(id, root.path().join("new.txt"))
                .unwrap(),
            2 => {
                app.git_root_epoch += 1;
            }
            3 => {
                app.close_tab(id);
            }
            _ => {
                app.editor_replace_selection(id, "bar");
            }
        }
        let before = app.workspace.get(id).map(|d| d.selections().clone());
        let replay = response.clone();
        let _ = app.update(response);
        assert!(app.multi_occurrence.is_none());
        assert_eq!(
            app.workspace.get(id).map(|d| d.selections().clone()),
            before
        );
        if app.workspace.get(id).is_some() {
            let next = app.next_occurrence_for(id);
            let sequence = app.multi_occurrence.as_ref().unwrap().sequence;
            let _ = app.update(replay);
            assert_eq!(app.multi_occurrence.as_ref().unwrap().sequence, sequence);
            let _ = app.update(primo_output_opzionale(next).unwrap());
        }
    }
}

#[test]
fn multi_reader_ripristinato_conserva_ordine_e_protezioni() {
    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("active.txt");
    let deferred = root.path().join("deferred.txt");
    std::fs::write(&active, "attiva").unwrap();
    std::fs::write(&deferred, "foo foo").unwrap();
    let mut app = app();
    let mut snapshot = lume_store::SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);
    let id = app.workspace.get_by_path(&deferred).unwrap();
    let read = app.activate_tab(id);
    let group = app.groups.focus();
    let _ = app.update(Message::EditorIn(
        group,
        EditorCommand::Insert("foo ".into()),
    ));
    let _ = app.update(Message::EditorIn(group, EditorCommand::AddNextOccurrence));
    let _ = app.update(Message::EditorIn(group, EditorCommand::Insert("X".into())));
    assert!(
        app.restored_tab_commands
            .get(&id)
            .is_some_and(|q| q.len() == 3)
    );
    let task = app.update(primo_output_opzionale(read).unwrap());
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "foo foo foo"
    );
    let task = app.update(primo_output_opzionale(task).expect("occorrenza dopo reader"));
    // Il primo Ctrl+D da caret seleziona la parola: X la sostituisce, in ordine.
    let _ = task;
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "foo X foo"
    );
    assert!(!app.restored_tab_commands.contains_key(&id));
}

#[test]
fn multi_input_differito_impedisce_chiusura_prematura() {
    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("active.txt");
    let deferred = root.path().join("deferred.txt");
    std::fs::write(&active, "attiva").unwrap();
    std::fs::write(&deferred, "foo foo").unwrap();
    let mut app = app();
    let mut snapshot = lume_store::SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);
    let id = app.workspace.get_by_path(&deferred).unwrap();
    let read = app.activate_tab(id);
    let group = app.groups.focus();
    let _ = app.update(Message::EditorIn(group, EditorCommand::Insert("x".into())));
    chiudi_scheda(&mut app, id);
    assert!(
        app.groups.contains(id),
        "input trattenuto dal reader ancora da salvare"
    );
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, notice)| notice.text.contains("attesa"))
    );
    assert!(matches!(
        app.richiesta_uscita(None),
        crate::app::Uscita::Chiedi
    ));
    let _ = app.update(primo_output_opzionale(read).unwrap());
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "xfoo foo"
    );
}

#[test]
fn multi_principale_navigazione_e_comandi_sincroni() {
    let mut app = app();
    let root = tempfile::tempdir().unwrap();
    write_and_open(&mut app, root.path(), "comandi.txt", "abc\ndef\nghi");
    let id = app.active_doc().unwrap();
    app.apply_editor_command_for(id, EditorCommand::ToggleCursor { pos: 5 });
    assert_eq!(
        app.workspace.get(id).unwrap().selections().ranges(),
        &[Range::point(0), Range::point(5)]
    );
    app.apply_editor_command_for(id, EditorCommand::AddCursorLine { above: false });
    assert_eq!(
        app.workspace.get(id).unwrap().selections().ranges(),
        &[Range::point(0), Range::point(5), Range::point(9)]
    );
    app.goto_line_index_for(id, 1);
    assert_eq!(
        app.workspace.get(id).unwrap().selections().ranges(),
        &[Range::point(0), Range::point(4), Range::point(5)]
    );
    app.apply_editor_command_for(id, EditorCommand::KeepPrimary);
    assert_eq!(app.workspace.get(id).unwrap().selection(), Range::point(4));
    assert_eq!(app.workspace.get(id).unwrap().version(), 0);
    assert!(!app.workspace.get(id).unwrap().is_dirty());
}

fn fixture_differita() -> (tempfile::TempDir, Lume, lume_core::DocId, lume_core::DocId) {
    let root = tempfile::tempdir().unwrap();
    let active = root.path().join("active.txt");
    let deferred = root.path().join("deferred.txt");
    std::fs::write(&active, "foo foo").unwrap();
    std::fs::write(&deferred, "foo foo").unwrap();
    let mut app = app();
    let mut snapshot = lume_store::SessionSnapshot::empty();
    snapshot.workspace_root = Some(root.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), deferred.clone()];
    snapshot.active_tab = Some(active.clone());
    app.restore_snapshot(snapshot);
    let a = app.workspace.get_by_path(&active).unwrap();
    let b = app.workspace.get_by_path(&deferred).unwrap();
    (root, app, a, b)
}

#[test]
fn multi_replay_b_riprende_quando_a_occupa_la_ricerca() {
    let (_root, mut app, a, b) = fixture_differita();
    let held = primo_output_opzionale(app.next_occurrence_for(a)).unwrap();
    let read = app.activate_tab(b);
    let group = app.groups.focus();
    let _ = app.update(Message::EditorIn(group, EditorCommand::AddNextOccurrence));
    let _ = app.update(Message::EditorIn(group, EditorCommand::Insert("X".into())));
    let _ = app.update(primo_output_opzionale(read).unwrap());
    let _ = app.update(held);
    assert_eq!(app.workspace.get(b).unwrap().rope().to_string(), "Xfoo foo");
    assert!(!app.restored_tab_commands.contains_key(&b));
}

#[test]
fn multi_reader_fallito_consente_scarto_esplicito() {
    let (root, mut app, _a, b) = fixture_differita();
    std::fs::remove_file(root.path().join("deferred.txt")).unwrap();
    let read = app.activate_tab(b);
    let group = app.groups.focus();
    let _ = app.update(Message::EditorIn(group, EditorCommand::Insert("X".into())));
    let _ = app.update(primo_output_opzionale(read).unwrap());
    chiudi_scheda(&mut app, b);
    assert!(
        riquadro_aperto(&app),
        "il reader fallito deve permettere scarto esplicito"
    );
    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Annulla);
    assert!(app.groups.contains(b));
    assert!(app.pending_editor_input(b));
    chiudi_scheda(&mut app, b);
    rispondi(
        &mut app,
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    );
    assert!(!app.groups.contains(b));
    assert!(!root.path().join("deferred.txt").exists());
}

fn emetti_widget<M>(
    view: &mut iced::Element<'_, M>,
    tree: &mut iced::advanced::widget::Tree,
    event: iced::Event,
    cursor: iced::mouse::Cursor,
) -> Vec<M> {
    let renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
        iced::Font::default(),
        iced::Pixels(16.0),
    ));
    let size = iced::Size::new(800.0, 600.0);
    let node = view.as_widget_mut().layout(
        tree,
        &renderer,
        &iced::advanced::layout::Limits::new(iced::Size::ZERO, size),
    );
    let mut published = Vec::new();
    let mut shell = iced::advanced::Shell::new(&mut published);
    view.as_widget_mut().update(
        tree,
        &event,
        iced::advanced::Layout::new(&node),
        cursor,
        &renderer,
        &mut iced::advanced::clipboard::Null,
        &mut shell,
        &iced::Rectangle::new(iced::Point::ORIGIN, size),
    );
    published
}

#[test]
fn multi_widget_modificatori_esc_e_precomposizione() {
    use iced::{
        Event,
        keyboard::{self, Modifiers},
        mouse,
    };
    use lume_ui::editor::{EditorOverlay, EditorState, editor_view};
    let doc = lume_core::Document::from_str("rosso rosso\nrosso\n", None, lume_core::DocId(1));
    let theme = lume_theme::LumeTheme::default();
    let mut view = editor_view(
        &doc,
        None,
        &theme,
        &[],
        EditorOverlay::default(),
        |command| command,
    );
    let mut tree = iced::advanced::widget::Tree::new(&view);
    let cursor = mouse::Cursor::Available(iced::Point::new(90.0, 20.0));
    emetti_widget(
        &mut view,
        &mut tree,
        Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::ALT)),
        cursor,
    );
    let commands = emetti_widget(
        &mut view,
        &mut tree,
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        cursor,
    );
    assert!(matches!(
        commands.as_slice(),
        [EditorCommand::ToggleCursor { .. }]
    ));
    assert!(!tree.state.downcast_ref::<EditorState>().is_dragging.get());
    let commands = emetti_widget(
        &mut view,
        &mut tree,
        Event::Mouse(mouse::Event::CursorMoved {
            position: iced::Point::new(120.0, 20.0),
        }),
        cursor,
    );
    assert!(commands.is_empty());
    let unavailable = mouse::Cursor::Unavailable;
    emetti_widget(
        &mut view,
        &mut tree,
        Event::InputMethod(iced::advanced::input_method::Event::Opened),
        unavailable,
    );
    let commands = emetti_widget(
        &mut view,
        &mut tree,
        Event::InputMethod(iced::advanced::input_method::Event::Preedit(
            "界".into(),
            None,
        )),
        unavailable,
    );
    assert!(commands.is_empty());
    let enter = iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
        modified_key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
        physical_key: iced::keyboard::key::Physical::Code(iced::keyboard::key::Code::Enter),
        location: iced::keyboard::Location::Standard,
        modifiers: iced::keyboard::Modifiers::default(),
        text: None,
        repeat: false,
    });
    assert!(emetti_widget(&mut view, &mut tree, enter, unavailable).is_empty());
    let commands = emetti_widget(
        &mut view,
        &mut tree,
        Event::InputMethod(iced::advanced::input_method::Event::Commit("界".into())),
        unavailable,
    );
    assert_eq!(commands, [EditorCommand::Insert("界".into())]);
    assert_eq!(doc.version(), 0);
}

#[test]
fn multi_tasto_destro_nella_secondaria_conserva_insieme() {
    let mut app = app();
    let root = tempfile::tempdir().unwrap();
    write_and_open(&mut app, root.path(), "context.txt", "abc def");
    let id = app.active_doc().unwrap();
    let set = SelectionSet::try_new(vec![Range::new(0, 3), Range::new(4, 7)], 0, 7).unwrap();
    app.workspace.get_mut(id).unwrap().set_selections(set);
    app.apply_editor_command_for(
        id,
        EditorCommand::ContextMenu {
            pos: 5,
            screen_x: 10,
            screen_y: 20,
        },
    );
    assert_eq!(
        app.workspace.get(id).unwrap().selections().ranges(),
        &[Range::new(0, 3), Range::new(4, 7)]
    );
    assert_eq!(app.workspace.get(id).unwrap().selection(), Range::new(4, 7));
    app.apply_editor_command_for(
        id,
        EditorCommand::ContextMenu {
            pos: 3,
            screen_x: 10,
            screen_y: 20,
        },
    );
    assert_eq!(
        app.workspace.get(id).unwrap().selections().ranges().len(),
        2
    );
}

#[test]
fn multi_riattivazione_scheda_riprende_input_senza_clic_nel_testo() {
    use iced::keyboard::{
        Event as K, Key, Location, Modifiers,
        key::{Code, Physical},
    };
    let mut app = app();
    let root = tempfile::tempdir().unwrap();
    let first = write_and_open(&mut app, root.path(), "primo.txt", "rosso rosso\nrosso\n");
    let id = app.active_doc().unwrap();
    app.workspace.get_mut(id).unwrap().set_selections(
        SelectionSet::try_new(vec![Range::point(0), Range::point(12)], 1, 18).unwrap(),
    );
    write_and_open(&mut app, root.path(), "altro.txt", "altro");
    let _ = app.update(Message::EditorTabBar(
        crate::editor_tab_bar::EditorTabBarMessage::Press(app.groups.focus(), id),
    ));
    let _ = app.update(Message::EditorTabBar(
        crate::editor_tab_bar::EditorTabBarMessage::Release,
    ));
    assert_eq!(app.workspace.get_by_path(&first), Some(id));
    let mut view = app.editor_of(app.groups.focus());
    let mut tree = iced::advanced::widget::Tree::new(&view);
    let event = iced::Event::Keyboard(K::KeyPressed {
        key: Key::Character("x".into()),
        modified_key: Key::Character("x".into()),
        physical_key: Physical::Code(Code::KeyX),
        location: Location::Standard,
        modifiers: Modifiers::default(),
        text: Some("x".into()),
        repeat: false,
    });
    let commands = emetti_widget(
        &mut view,
        &mut tree,
        event,
        iced::mouse::Cursor::Unavailable,
    );
    assert!(commands.iter().any(|message|matches!(message,Message::EditorIn(_,EditorCommand::Insert(text)) if text=="x")),"scheda riattivata senza focus");
    drop(view);
    for command in commands {
        let _ = app.update(command);
    }
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "xrosso rosso\nxrosso\n"
    );
}

#[test]
fn multi_menu_worker_e_esc_prima_chiude_overlay() {
    let mut app = app();
    let root = tempfile::tempdir().unwrap();
    write_and_open(&mut app, root.path(), "menu.txt", "rosso rosso\nrosso\n");
    let id = app.active_doc().unwrap();
    app.workspace.get_mut(id).unwrap().set_selections(
        SelectionSet::try_new(vec![Range::point(0), Range::point(12)], 1, 18).unwrap(),
    );
    let task = app.update(Message::MenuAction(MenuAct::AddNextOccurrence));
    let _ = app.update(primo_output_opzionale(task).expect("menu restituisce worker"));
    assert_eq!(
        app.workspace.get(id).unwrap().selections().ranges(),
        &[Range::point(0), Range::new(12, 17)]
    );
    app.apri_overlay(OverlayId::About);
    let group = app.groups.focus();
    let _ = app.update(Message::EditorIn(group, EditorCommand::KeepPrimary));
    assert_eq!(
        app.workspace.get(id).unwrap().selections().ranges().len(),
        2
    );
    let _ = app.update(Message::CloseAll);
    assert!(app.shell.active_overlay().is_none());
    let _ = app.update(Message::EditorIn(group, EditorCommand::KeepPrimary));
    assert_eq!(
        app.workspace.get(id).unwrap().selections().ranges(),
        &[Range::new(12, 17)]
    );
}
