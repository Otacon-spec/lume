//! Menu contestuali delle superfici diverse dall'editor: la scheda e l'albero
//! di progetto. Il menu dell'editor ha le sue prove in `diff_tests`.
use super::*;
use crate::context_menu::Soggetto;
use crate::editor_tab_bar::EditorTabBarMessage;
use crate::menu::MenuAct;
use lume_ui::tool::ToolEvent;

#[test]
fn il_tasto_destro_sulla_scheda_apre_il_menu_alla_posizione_del_clic() {
    let mut app = app();
    let root = tempdir("menu_scheda");
    let percorso = write_and_open(&mut app, &root, "prova.txt", "uno\n");
    let id = app.active_doc().unwrap();

    let punto = iced::Point::new(120.0, 30.0);
    let _ = app.update(Message::EditorTabBar(EditorTabBarMessage::RightPress(
        id, punto,
    )));

    let menu = app.context_menu.as_ref().expect("il menù si apre");
    match &menu.soggetto {
        Soggetto::Scheda { id: scheda, path } => {
            assert_eq!(*scheda, id, "la scheda del clic");
            assert_eq!(path.as_deref(), Some(percorso.as_path()));
        }
        altro => panic!("soggetto atteso la scheda, trovato {altro:?}"),
    }
    assert_eq!(
        (menu.x, menu.y),
        (120.0, 30.0),
        "il menù si apre dove si è cliccato"
    );
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Context));
}

#[test]
fn chiudi_scheda_chiude_proprio_quella_scheda() {
    let mut app = app();
    let root = tempdir("menu_chiudi");
    write_and_open(&mut app, &root, "primo.txt", "uno\n");
    let primo = app.active_doc().unwrap();
    write_and_open(&mut app, &root, "secondo.txt", "due\n");
    let secondo = app.active_doc().unwrap();

    // il clic è sulla prima scheda, che non è quella attiva
    let _ = app.update(Message::EditorTabBar(EditorTabBarMessage::RightPress(
        primo,
        iced::Point::new(10.0, 20.0),
    )));
    let _ = app.update(Message::MenuAction(MenuAct::CloseTab));

    assert!(!app.groups.contains(primo), "la scheda cliccata è chiusa");
    assert!(app.groups.contains(secondo), "l'altra scheda resta aperta");
    assert!(
        app.context_menu.is_none(),
        "scegliere una voce chiude il menù"
    );
}

#[test]
fn il_tasto_destro_sull_albero_apre_il_menu_con_il_nodo() {
    let mut app = app();
    let root = tempdir("menu_albero");
    let percorso = root.join("prova.txt");
    std::fs::write(&percorso, "uno\n").unwrap();

    let _ = app.update(Message::Tool(
        crate::tools::project::ID,
        ToolEvent::new(crate::tools::project::Event::RowRightClicked(
            percorso.clone(),
            false,
            10.0,
            20.0,
        )),
    ));

    let menu = app.context_menu.as_ref().expect("il menù si apre");
    match &menu.soggetto {
        Soggetto::Albero { path, is_dir } => {
            assert_eq!(path, &percorso);
            assert!(!is_dir);
        }
        altro => panic!("soggetto atteso l'albero, trovato {altro:?}"),
    }
    assert_eq!((menu.x, menu.y), (10.0, 20.0));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Context));
}

#[test]
fn sull_albero_il_file_da_aprire_e_il_nodo_cliccato() {
    let percorso = std::path::PathBuf::from("/tmp/prova.txt");
    let file = Soggetto::Albero {
        path: percorso.clone(),
        is_dir: false,
    };
    assert_eq!(file.file_da_aprire(), Some(percorso.as_path()));

    // su una cartella non c'è nulla da aprire: la voce non c'è.
    let cartella = Soggetto::Albero {
        path: std::path::PathBuf::from("/tmp/cartella"),
        is_dir: true,
    };
    assert_eq!(cartella.file_da_aprire(), None);
    // e le schede non sono file dell'albero.
    let scheda = Soggetto::Scheda {
        id: lume_core::DocId(1),
        path: Some(percorso.clone()),
    };
    assert_eq!(scheda.file_da_aprire(), None);
}

#[test]
fn un_percorso_che_non_si_risolve_avvisa_invece_di_fare_silenzio() {
    let mut app = app();
    let root = tempdir("menu_non_risolto");
    app.set_project_root(root.clone());

    // un percorso **relativo** che nel progetto non c'è: `resolve_path`
    // non lo risolve, e la voce «Apri» non può aprirlo.
    let _ = app.update(Message::Tool(
        crate::tools::project::ID,
        ToolEvent::new(crate::tools::project::Event::RowRightClicked(
            std::path::PathBuf::from("sparito.txt"),
            false,
            10.0,
            20.0,
        )),
    ));
    let _ = app.update(Message::MenuAction(MenuAct::OpenPath));

    let avviso = app
        .notice
        .as_ref()
        .expect("un avviso, non un silenzio")
        .1
        .text
        .clone();
    assert!(
        avviso.contains("Non si apre"),
        "l'avviso deve dire che il file non si apre: {avviso}"
    );
}

#[test]
fn il_percorso_del_soggetto_e_solo_quello() {
    let percorso = std::path::PathBuf::from("/tmp/prova.txt");
    let scheda = Soggetto::Scheda {
        id: lume_core::DocId(1),
        path: Some(percorso.clone()),
    };
    assert_eq!(scheda.path(), Some(percorso.as_path()));
    let albero = Soggetto::Albero {
        path: percorso.clone(),
        is_dir: true,
    };
    assert_eq!(albero.path(), Some(percorso.as_path()));
    let testo = Soggetto::Editor {
        pos: 3,
        selezione: None,
    };
    assert_eq!(testo.path(), None);
    assert_eq!(testo.pos(), Some(3));
}

#[test]
fn il_menu_della_scheda_offre_di_chiudere_le_altre() {
    let app = app();
    let voci = crate::context_menu::items(
        &Soggetto::Scheda {
            id: lume_core::DocId(1),
            path: Some(std::path::PathBuf::from("/tmp/prova.txt")),
        },
        app.menu_checks(),
    );
    let etichette: Vec<&str> = voci.iter().map(|(etichetta, _)| *etichetta).collect();
    assert_eq!(
        etichette,
        ["Chiudi scheda", "Chiudi le altre", "Copia percorso"]
    );
}

#[test]
fn chiudi_le_altre_chiude_le_altre_del_gruppo_e_tenere_questa() {
    let mut app = app();
    let root = tempdir("menu_chiudi_altre");
    write_and_open(&mut app, &root, "uno.txt", "uno\n");
    write_and_open(&mut app, &root, "due.txt", "due\n");
    write_and_open(&mut app, &root, "tre.txt", "tre\n");
    let tenere = app.active_doc().expect("una scheda attiva");
    assert_eq!(
        app.groups
            .tabs(crate::editor_groups::GroupId::Primary)
            .len(),
        3
    );

    // il clic è sulla terza scheda: quella viene tenuta
    let _ = app.update(Message::EditorTabBar(EditorTabBarMessage::RightPress(
        tenere,
        iced::Point::new(10.0, 20.0),
    )));
    let _ = app.update(Message::MenuAction(MenuAct::CloseOtherTabs));

    let rimaste = app.groups.tabs(crate::editor_groups::GroupId::Primary);
    assert_eq!(rimaste, &[tenere], "resta solo la scheda cliccata");
}

#[test]
fn chiudi_le_altre_rifiuta_se_una_delle_altre_ha_modifiche_non_salvate() {
    let mut app = app();
    let root = tempdir("menu_chiudi_altre_sporca");
    write_and_open(&mut app, &root, "uno.txt", "uno\n");
    write_and_open(&mut app, &root, "due.txt", "due\n");
    let tenere = app.active_doc().expect("una scheda attiva");

    // la scheda da tenere è pulita, l'altra ha modifiche non salvate
    let sporca = app
        .groups
        .tabs(crate::editor_groups::GroupId::Primary)
        .iter()
        .copied()
        .find(|id| *id != tenere)
        .expect("l'altra scheda");
    let lunghezza = app
        .workspace
        .get(sporca)
        .expect("il documento")
        .rope()
        .len_chars();
    let cambi = lume_core::ChangeSet::replace_range(0, 0, "x", lunghezza);
    app.workspace
        .get_mut(sporca)
        .expect("il documento")
        .apply(lume_core::Transaction {
            changes: cambi,
            selection_after: None,
        });

    let _ = app.update(Message::EditorTabBar(EditorTabBarMessage::RightPress(
        tenere,
        iced::Point::new(10.0, 20.0),
    )));
    let _ = app.update(Message::MenuAction(MenuAct::CloseOtherTabs));

    assert_eq!(
        app.groups
            .tabs(crate::editor_groups::GroupId::Primary)
            .len(),
        2,
        "nessuna scheda è stata chiusa"
    );
    assert!(app.groups.contains(sporca), "la scheda sporca resta");
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(notifica.contains("modifiche non salvate"), "{notifica}");
}

#[test]
fn chiudi_le_altre_chiude_solo_il_gruppo_della_scheda() {
    let mut app = app();
    let root = tempdir("menu_chiudi_altre_gruppi");
    write_and_open(&mut app, &root, "uno.txt", "uno\n");
    write_and_open(&mut app, &root, "due.txt", "due\n");
    write_and_open(&mut app, &root, "tre.txt", "tre\n");
    // `split` sposta la scheda attiva nell'altro gruppo: nel primario
    // restano «uno» e «due», nel secondario «tre».
    app.groups.split();
    let nel_primario = app
        .groups
        .tabs(crate::editor_groups::GroupId::Primary)
        .iter()
        .copied()
        .find(|id| {
            app.workspace
                .get(*id)
                .and_then(lume_core::Document::path)
                .is_some_and(|percorso| percorso.ends_with("due.txt"))
        })
        .expect("la scheda «due» nel gruppo primario");
    let nell_altro = app
        .groups
        .tabs(crate::editor_groups::GroupId::Secondary)
        .iter()
        .copied()
        .next()
        .expect("la scheda nell'altro gruppo");

    let _ = app.update(Message::EditorTabBar(EditorTabBarMessage::RightPress(
        nel_primario,
        iced::Point::new(10.0, 20.0),
    )));
    let _ = app.update(Message::MenuAction(MenuAct::CloseOtherTabs));

    assert_eq!(
        app.groups.tabs(crate::editor_groups::GroupId::Primary),
        &[nel_primario],
        "il gruppo primario tiene la sua scheda"
    );
    assert!(
        app.groups.contains(nell_altro),
        "la scheda dell'altro gruppo non è stata toccata"
    );
}

#[test]
fn chiudi_le_altre_senza_altre_schede_avvisa_e_non_fa_nulla() {
    let mut app = app();
    let root = tempdir("menu_chiudi_altre_nessuna");
    write_and_open(&mut app, &root, "uno.txt", "uno\n");
    let tenere = app.active_doc().expect("una scheda attiva");

    let _ = app.update(Message::EditorTabBar(EditorTabBarMessage::RightPress(
        tenere,
        iced::Point::new(10.0, 20.0),
    )));
    let _ = app.update(Message::MenuAction(MenuAct::CloseOtherTabs));

    assert!(app.groups.contains(tenere), "la scheda resta aperta");
    let notifica = app.notice.as_ref().expect("una notifica").1.text.clone();
    assert!(
        notifica.to_lowercase().contains("non ci sono altre schede"),
        "{notifica}"
    );
}

#[test]
fn chiudi_le_altre_persiste_la_sessione() {
    let mut app = app();
    let root = tempdir("menu_chiudi_altre_sessione");
    write_and_open(&mut app, &root, "uno.txt", "uno\n");
    write_and_open(&mut app, &root, "due.txt", "due\n");
    let tenere = app.active_doc().expect("una scheda attiva");

    let _ = app.update(Message::EditorTabBar(EditorTabBarMessage::RightPress(
        tenere,
        iced::Point::new(10.0, 20.0),
    )));
    let _ = app.update(Message::MenuAction(MenuAct::CloseOtherTabs));

    // la sessione salvata deve avere una scheda sola: la tenuta.
    let snapshot = app.session_snapshot();
    assert_eq!(
        snapshot.tabs.len(),
        1,
        "la sessione tiene una scheda sola, non {}",
        snapshot.tabs.len()
    );
}

#[test]
fn tool_apri_file_non_risolto_avvisa_anche_con_riga() {
    use lume_ui::tool::ToolRequest;
    for richiesta in [
        ToolRequest::OpenFile("sparito.txt".into()),
        ToolRequest::OpenFileAt {
            path: "sparito.txt".into(),
            line: 3,
        },
    ] {
        let mut app = app();
        let root = tempdir("tool_non_risolto");
        app.set_project_root(root);
        let _ = app.apply_tool_requests(vec![(crate::tools::project::ID, richiesta)]);
        let avviso = &app
            .notice
            .as_ref()
            .expect("azione rifiutata visibile")
            .1
            .text;
        assert!(
            avviso.contains("Non si apre") && avviso.contains("sparito.txt"),
            "{avviso}"
        );
    }
}

#[test]
fn menu_chiudi_scheda_sporca_chiede_sulla_scheda_cliccata() {
    let mut app = app();
    let root = tempdir("menu_chiudi_sporca");
    write_and_open(&mut app, &root, "sporca.txt", "uno\n");
    let sporca = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    write_and_open(&mut app, &root, "attiva.txt", "due\n");
    let attiva = app.active_doc().unwrap();
    let _ = app.update(Message::EditorTabBar(EditorTabBarMessage::RightPress(
        sporca,
        iced::Point::new(10.0, 20.0),
    )));
    let _ = app.update(Message::MenuAction(MenuAct::CloseTab));
    assert!(app.groups.contains(sporca) && app.groups.contains(attiva));
    assert!(app.workspace.get(sporca).unwrap().is_dirty());
    assert_eq!(
        app.shell.active_overlay(),
        Some(OverlayId::ConfermaChiusura)
    );
    assert_eq!(
        app.riquadro_chiusura().unwrap().titolo,
        "Salvare le modifiche a sporca.txt?"
    );
}

#[test]
fn menu_apri_dispatch_apre_i_byte_del_nodo_cliccato() {
    let mut app = app();
    let root = tempdir("menu_dispatch_apri");
    let path = root.join("nodo.txt");
    std::fs::write(&path, "contenuto nodo\n").unwrap();
    let _ = app.update(Message::Tool(
        crate::tools::project::ID,
        ToolEvent::new(crate::tools::project::Event::RowRightClicked(
            path.clone(),
            false,
            10.0,
            20.0,
        )),
    ));
    let task = app.update(Message::MenuAction(MenuAct::OpenPath));
    let message = primo_output(task);
    let _ = app.update(message);
    let doc = app
        .workspace
        .get(app.active_doc().expect("scheda aperta"))
        .unwrap();
    assert_eq!(doc.path(), Some(path.as_path()));
    assert_eq!(doc.rope().to_string(), "contenuto nodo\n");
    assert!(app.context_menu.is_none());
}

#[test]
fn menu_copia_percorso_dispatch_scrive_il_soggetto_nella_clipboard() {
    use futures::StreamExt;
    let mut app = app();
    let root = tempdir("menu_dispatch_copia");
    let path = write_and_open(&mut app, &root, "cliccata.txt", "uno\n");
    let cliccata = app.active_doc().unwrap();
    write_and_open(&mut app, &root, "attiva.txt", "due\n");
    let _ = app.update(Message::EditorTabBar(EditorTabBarMessage::RightPress(
        cliccata,
        iced::Point::new(10.0, 20.0),
    )));
    let task = app.update(Message::MenuAction(MenuAct::CopyPath));
    let mut stream = iced_runtime::task::into_stream(task).unwrap();
    let copied = futures::executor::block_on(async {
        while let Some(azione) = stream.next().await {
            if let iced_runtime::Action::Clipboard(iced_runtime::clipboard::Action::Write {
                contents,
                ..
            }) = azione
            {
                return contents;
            }
        }
        panic!("manca scrittura clipboard");
    });
    assert_eq!(copied, path.to_string_lossy());
    assert!(app.context_menu.is_none());
}
