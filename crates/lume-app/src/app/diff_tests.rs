#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use super::*;

fn fake_diff(path: &str, commit: Option<&str>) -> Box<lume_git::FileDiff> {
    Box::new(lume_git::FileDiff {
        path: PathBuf::from(path),
        base: match commit {
            Some(id) => lume_git::DiffBase::Commit(id.to_owned()),
            None => lume_git::DiffBase::Worktree,
        },
        rows: vec![lume_git::DiffRow::Line {
            old_line: Some(1),
            new_line: Some(1),
            sign: lume_git::DiffSign::Context,
            text: "riga".into(),
            hunk_idx: None,
        }],
        hunks: Vec::new(),
        added: 0,
        removed: 0,
        binary: false,
        truncated: false,
    })
}

/// Il diff vince sull'area centrale, come `showDiff` nel prototipo: anche
/// con la console SQL attiva.
#[test]
#[cfg(feature = "sql")]
fn il_diff_ha_la_precedenza_sulla_console_e_sulleditor() {
    let (mut app, _) = Lume::new_without_store();
    assert_eq!(app.central_view(), CentralView::Editor);

    app.open_sql_console(None, None);
    assert_eq!(app.central_view(), CentralView::Sql);

    app.diff_request = Some((PathBuf::from("a.rs"), lume_git::DiffBase::Worktree));
    let _ = app.update(Message::DiffLoaded {
        request: (PathBuf::from("a.rs"), lume_git::DiffBase::Worktree),
        result: Ok(fake_diff("a.rs", None)),
        refresh: false,
    });
    assert_eq!(app.central_view(), CentralView::Diff);

    let _ = app.update(Message::CloseDiff);
    assert_eq!(
        app.central_view(),
        CentralView::Sql,
        "chiudendo il diff l'area centrale torna a chi c'era prima"
    );
}

/// Due clic rapidi su file diversi: la prima risposta non deve scalzare
/// la seconda.
#[test]
fn un_diff_superato_non_scalza_quello_chiesto_dopo() {
    let (mut app, _) = Lume::new_without_store();
    app.diff_request = Some((PathBuf::from("secondo.rs"), lume_git::DiffBase::Worktree));

    let _ = app.update(Message::DiffLoaded {
        request: (PathBuf::from("primo.rs"), lume_git::DiffBase::Worktree),
        result: Ok(fake_diff("primo.rs", None)),
        refresh: false,
    });
    assert!(
        app.diff.is_none(),
        "la risposta del file abbandonato va scartata"
    );

    let _ = app.update(Message::DiffLoaded {
        request: (PathBuf::from("secondo.rs"), lume_git::DiffBase::Worktree),
        result: Ok(fake_diff("secondo.rs", None)),
        refresh: false,
    });
    assert_eq!(
        app.diff.as_ref().map(|diff| diff.path.clone()),
        Some(PathBuf::from("secondo.rs"))
    );
}

/// L'evidenziazione è per `(path, base)`: lo stesso file aperto dal
/// pannello Commit e da un commit del Log Git sono due righe diverse.
#[test]
fn levidenziazione_distingue_la_base_del_diff() {
    let (mut app, _) = Lume::new_without_store();
    assert!(app.open_diff_ref().is_none());

    app.diff_request = Some((
        PathBuf::from("a.rs"),
        lume_git::DiffBase::Commit("abc123".into()),
    ));
    assert_eq!(
        app.open_diff_ref(),
        Some((Path::new("a.rs"), Some("abc123")))
    );

    app.diff_request = Some((PathBuf::from("a.rs"), lume_git::DiffBase::Worktree));
    assert_eq!(app.open_diff_ref(), Some((Path::new("a.rs"), None)));
}

/// Il diff vince su tutto: senza questo, aprire un file dall'albero o
/// cambiare scheda non cambierebbe nulla a schermo e si resterebbe
/// bloccati finché non si trova la «×».
#[test]
#[cfg(feature = "sql")]
fn attivare_un_documento_esce_dal_diff() {
    let (mut app, _) = Lume::new_without_store();
    let file = std::env::temp_dir().join(format!(
        "lume_diff_esce_{}.txt",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &file, b"ciao
",
    )
    .unwrap();

    let apri_diff = |app: &mut Lume| {
        app.diff_request = Some((PathBuf::from("a.rs"), lume_git::DiffBase::Worktree));
        let _ = app.update(Message::DiffLoaded {
            request: (PathBuf::from("a.rs"), lume_git::DiffBase::Worktree),
            result: Ok(fake_diff("a.rs", None)),
            refresh: false,
        });
        assert_eq!(app.central_view(), CentralView::Diff);
    };

    // 1. aprire un file (albero progetto, ricerca, «vai a definizione»)
    apri_diff(&mut app);
    app.open_file(&file).unwrap();
    assert_eq!(app.central_view(), CentralView::Editor);

    // 2. clic su una scheda già aperta: premere e rilasciare senza
    //    muoversi, che e' la strada vera da quando le schede si trascinano
    let scheda = app.active_doc().unwrap();
    apri_diff(&mut app);
    let _ = app.update(Message::EditorTabBar(
        crate::editor_tab_bar::EditorTabBarMessage::Press(
            crate::editor_groups::GroupId::Primary,
            scheda,
        ),
    ));
    let _ = app.update(Message::EditorTabBar(
        crate::editor_tab_bar::EditorTabBarMessage::Release,
    ));
    assert_eq!(app.central_view(), CentralView::Editor);

    // 3. doppio clic su una tabella nel pannello Database
    apri_diff(&mut app);
    app.open_sql_console(None, None);
    assert_eq!(app.central_view(), CentralView::Sql);

    std::fs::remove_file(file).unwrap();
}

fn modificato(path: &str) -> lume_git::FileStatus {
    lume_git::FileStatus {
        path: PathBuf::from(path),
        index: None,
        worktree: Some(lume_git::ChangeKind::Modified),
    }
}

/// Il diff sul worktree è la vista di una riga del pannello Commit: vive
/// finché vive quella riga. Le basi su commit non invecchiano.
#[test]
fn il_diff_sul_worktree_vive_quanto_la_sua_riga_nel_pannello() {
    let (mut app, _) = Lume::new_without_store();
    let uno = [modificato("a.rs")];
    assert_eq!(
        app.diff_to_refresh(Some(&uno)),
        DiffRefresh::Keep,
        "nessun diff aperto"
    );

    app.diff_request = Some((PathBuf::from("a.rs"), lume_git::DiffBase::Worktree));
    assert_eq!(
        app.diff_to_refresh(Some(&uno)),
        DiffRefresh::Refresh(PathBuf::from("a.rs"), lume_git::DiffBase::Worktree)
    );
    assert_eq!(
        app.diff_to_refresh(Some(&[])),
        DiffRefresh::Close,
        "committato o ripristinato: la riga non c'è più"
    );
    assert_eq!(
        app.diff_to_refresh(None),
        DiffRefresh::Keep,
        "uno status fallito non dice niente sul worktree"
    );

    app.diff_request = Some((
        PathBuf::from("a.rs"),
        lume_git::DiffBase::Commit("abc123".into()),
    ));
    assert_eq!(
        app.diff_to_refresh(Some(&[])),
        DiffRefresh::Keep,
        "un diff contro un commit è immutabile: non si ricalcola e non si chiude"
    );
}

/// Un diff senza righe non vuol dire «non c'è più niente»: un binario
/// modificato ne ha zero per costruzione. La regola sulle righe chiudeva
/// la vista di `logo.png` al primo refresh, col file ancora nel pannello.
#[test]
fn un_binario_modificato_resta_a_schermo_anche_dopo_un_ricalcolo() {
    let (mut app, _) = Lume::new_without_store();
    let mut binario = fake_diff("logo.png", None);
    binario.rows.clear();
    binario.binary = true;

    app.diff_request = Some((PathBuf::from("logo.png"), lume_git::DiffBase::Worktree));
    let _ = app.update(Message::DiffLoaded {
        request: (PathBuf::from("logo.png"), lume_git::DiffBase::Worktree),
        result: Ok(binario),
        refresh: true,
    });

    assert_eq!(app.central_view(), CentralView::Diff);
    assert!(app.diff.as_ref().is_some_and(|diff| diff.binary));
}

/// Dopo un «Ripristina» riuscito su un file non versionato il file non è
/// più né in HEAD né su disco: il ricalcolo fallirebbe. `diff_to_refresh`
/// chiude prima di arrivarci, e se qualcosa sfugge il fallimento resta
/// muto — notificarlo sembrerebbe un fallimento del ripristino, che
/// invece è andato bene.
#[test]
fn un_ricalcolo_fallito_chiude_senza_notificare() {
    let (mut app, _) = Lume::new_without_store();
    app.diff_request = Some((PathBuf::from("b.rs"), lume_git::DiffBase::Worktree));
    let _ = app.update(Message::DiffLoaded {
        request: (PathBuf::from("b.rs"), lume_git::DiffBase::Worktree),
        result: Err("Tried to diff resources that are both considered removed".into()),
        refresh: true,
    });

    assert!(app.diff.is_none() && app.diff_request.is_none());
    assert!(
        app.notice.is_none(),
        "un ricalcolo non deve notificare niente: {:?}",
        app.notice.as_ref().map(|(_, notice)| notice.text.clone())
    );
}

/// Un errore di calcolo non lascia l'area centrale su un diff a metà.
#[test]
fn un_diff_fallito_chiude_la_vista_invece_di_lasciarla_vuota() {
    let (mut app, _) = Lume::new_without_store();
    app.diff_request = Some((PathBuf::from("a.rs"), lume_git::DiffBase::Worktree));
    let _ = app.update(Message::DiffLoaded {
        request: (PathBuf::from("a.rs"), lume_git::DiffBase::Worktree),
        result: Err("blob illeggibile".into()),
        refresh: false,
    });

    assert!(app.diff.is_none());
    assert!(app.diff_request.is_none());
    assert_eq!(app.central_view(), CentralView::Editor);
}

#[test]
fn notes_lazy_lifecycle_opens_updates_and_closes_note_tabs() {
    let (mut app, _) = Lume::new_without_store();
    assert!(
        app.notes.is_none(),
        "construction must allocate no Notes state"
    );

    let generation = app.begin_notes_load().expect("first open starts loading");
    assert!(
        app.begin_notes_load().is_none(),
        "same lifecycle loads only once"
    );
    let directory = tempfile::tempdir().unwrap();
    let store = lume_notes::NoteStore::open(directory.path()).unwrap();
    let loaded = store.create("Prima").unwrap();
    app.finish_notes_load(
        generation,
        directory.path().to_path_buf(),
        vec![loaded.clone()],
    )
    .unwrap();

    let doc = app.open_loaded_note(loaded.clone());
    assert!(matches!(
        app.editor_kind(doc),
        crate::editor_groups::EditorKind::Note(id) if id == loaded.note.header.id
    ));
    assert_eq!(app.editor_tab_name(doc).as_deref(), Some("Prima"));

    let mut rinominata = loaded.clone();
    rinominata.note.header.title = "Dopo".into();
    app.nota_salvata(doc, generation, &loaded.note.header.id, rinominata)
        .unwrap();
    assert_eq!(app.editor_tab_name(doc).as_deref(), Some("Dopo"));

    app.finish_note_delete(&loaded.note.header.id);
    assert!(!app.groups.contains(doc));
    assert!(app.workspace.get(doc).is_none());

    app.set_feature_enabled(crate::features::Feature::Notes, false);
    assert!(
        app.notes.is_none(),
        "disabled Notes retains no lifecycle state"
    );
}

#[test]
fn notes_real_filesystem_chain_creates_opens_and_indexes() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let (store, summaries) = notes_scan_blocking(directory.path().to_path_buf())
            .await
            .unwrap();
        assert!(
            summaries.is_empty(),
            "production library has no seeded data"
        );

        let (created, avviso) = notes_create_blocking(store.clone()).await.unwrap();
        assert!(avviso.is_none(), "{avviso:?}");
        assert!(created.path().is_file());
        let opened = notes_open_blocking(store.clone(), created.path().to_path_buf())
            .await
            .unwrap();
        assert_eq!(opened.note.header.id, created.note.header.id);
        assert_eq!(store.scan().unwrap().len(), 1);

        // `NoteStore::delete` è testato solo con un Cestino temporaneo in
        // lume-notes: nessun test d'integrazione deve sporcare il Cestino
        // dell'utente che lancia la suite.
        assert!(created.path().is_file());
        assert_eq!(store.scan().unwrap().len(), 1);
    });
}

fn con_una_nota() -> (
    Lume,
    tempfile::TempDir,
    lume_notes::NoteStore,
    lume_notes::LoadedNote,
    lume_core::DocId,
) {
    let dir = tempfile::tempdir().unwrap();
    let mut app = Lume::new_without_store().0;
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    let loaded = store.create("Prima").unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), vec![loaded.clone()])
        .unwrap();
    let doc = app.open_loaded_note(loaded.clone());
    (app, dir, store, loaded, doc)
}

fn con_codice_e_nota() -> (
    Lume,
    tempfile::TempDir,
    tempfile::TempDir,
    lume_notes::NoteStore,
    lume_core::DocId,
    lume_core::DocId,
) {
    use crate::editor_groups::GroupId;
    let (mut app, note_dir, store, _loaded, note_doc) = con_una_nota();
    let project_dir = tempfile::tempdir().unwrap();
    let file = project_dir.path().join("src").join("main.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "uno\ndue\ntre\n").unwrap();
    app.project_root = Some(project_dir.path().to_path_buf());
    app.open_file(&file).unwrap();
    let code_doc = app.active_doc().unwrap();
    assert!(app.groups.split());
    assert_eq!(app.groups.active_in(GroupId::Primary), Some(note_doc));
    assert_eq!(app.groups.group_of(code_doc), Some(GroupId::Secondary));
    app.groups.activate(code_doc);
    (app, project_dir, note_dir, store, note_doc, code_doc)
}

#[test]
fn il_link_usa_la_selezione_o_la_riga_del_cursore() {
    let (mut app, project, note_dir, _store, _note, code) = con_codice_e_nota();
    let link_for = |app: &Lume, selection| {
        let link = app.code_link_for(code, selection).unwrap();
        (link.from, link.to, link.workspace, link.path, link.symbol)
    };
    let (from, to, _, _, _) = link_for(&app, Some(Range::new(4, 7)));
    assert_eq!((from, to), (2, 2));
    assert_eq!(
        (
            link_for(&app, Some(Range::new(4, 8))).0,
            link_for(&app, Some(Range::new(4, 8))).1
        ),
        (2, 2)
    );
    assert_eq!(
        (
            link_for(&app, Some(Range::new(0, 11))).0,
            link_for(&app, Some(Range::new(0, 11))).1
        ),
        (1, 3)
    );
    app.workspace
        .get_mut(code)
        .unwrap()
        .set_selection(Range::point(9));
    let (from, to, workspace, path, symbol) = link_for(&app, None);
    assert_eq!((from, to), (3, 3));
    assert_eq!(workspace, project.path());
    assert_eq!(path, Path::new("src").join("main.rs"));
    assert!(symbol.is_none());
    app.project_root = Some(note_dir.path().to_path_buf());
    assert!(app.code_link_for(code, None).is_none());
}

#[test]
fn collegare_all_appunto_aperto_salva_il_link_dal_writer_comune() {
    let (mut app, _project, _notes_dir, store, note_doc, code_doc) = con_codice_e_nota();
    let id = app.notes.as_ref().unwrap().tabs[&note_doc].clone();
    let link = app.code_link_for(code_doc, Some(Range::new(4, 7))).unwrap();
    let task = app.modifica_collegamenti(note_doc, move |note| {
        crate::note_links::aggiungi_collegamento(note, link)
    });
    let task = iced::Task::batch([task, app.applica_seguito()]);
    assert!(matches!(
        app.notes.as_ref().unwrap().salvataggi[&note_doc],
        crate::autosave::StatoSalvataggio::InCorso { .. }
    ));
    assert_eq!(
        app.saves.active_document(),
        Some(note_doc),
        "notice: {:?}; busy={} git={} delete={} replacement={}",
        app.notice.as_ref().map(|(_, notice)| &notice.text),
        (app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some()),
        app.git_write.is_some(),
        app.note_delete.is_some(),
        app.replacement.is_some()
    );
    app.completa_salvataggi(task);
    assert_eq!(
        app.notes.as_ref().unwrap().marcatori[&code_doc],
        vec![1..2],
        "il nuovo indice raggiunge i marcatori dopo il save"
    );
    let notes = app.notes.as_ref().unwrap();
    assert_eq!(
        notes.salvataggi[&note_doc],
        crate::autosave::StatoSalvataggio::Pulito
    );
    assert_eq!(notes.loaded[&id].note.header.links.len(), 1);
    assert_eq!(
        notes
            .summaries
            .iter()
            .find(|summary| summary.id == id)
            .unwrap()
            .links
            .len(),
        1,
        "summary={:?}; notice={:?}",
        notes.summaries,
        app.notice.as_ref().map(|(_, notice)| &notice.text)
    );
    assert_eq!(
        notes.salvataggi[&note_doc],
        crate::autosave::StatoSalvataggio::Pulito
    );
    assert_eq!(
        store
            .load(notes.loaded[&id].path())
            .unwrap()
            .note
            .header
            .links
            .len(),
        1
    );
}

#[test]
fn link_doppio_o_salvataggio_gia_in_volo_non_parte_due_volte() {
    let (mut app, _project, _notes_dir, _store, note_doc, code_doc) = con_codice_e_nota();
    let link = app.code_link_for(code_doc, Some(Range::new(4, 7))).unwrap();
    let first = link.clone();
    let task = app.modifica_collegamenti(note_doc, move |note| {
        crate::note_links::aggiungi_collegamento(note, first)
    });
    let task = iced::Task::batch([task, app.applica_seguito()]);
    let busy = app
        .prepara_collegamento(note_doc, |_| Ok("secondo".into()))
        .unwrap_err();
    assert!(busy.contains("si sta salvando"));
    app.completa_salvataggi(task);
    let duplicate = app
        .prepara_collegamento(note_doc, move |note| {
            crate::note_links::aggiungi_collegamento(note, link)
        })
        .unwrap_err();
    assert!(duplicate.contains("già"));
}

#[test]
fn il_tasto_destro_ricorda_la_selezione_solo_se_il_clic_e_dentro() {
    use crate::editor_groups::GroupId;
    let (mut app, _project, _notes_dir, _store, _note, code) = con_codice_e_nota();
    let right_click = |pos| {
        Message::EditorIn(
            GroupId::Secondary,
            EditorCommand::ContextMenu {
                pos,
                screen_x: 50,
                screen_y: 60,
            },
        )
    };
    app.workspace
        .get_mut(code)
        .unwrap()
        .set_selection(Range::new(4, 7));
    let _ = app.update(right_click(5));
    assert_eq!(
        app.context_menu.as_ref().unwrap().soggetto.selezione(),
        Some(Range::new(4, 7))
    );
    assert_eq!(
        app.workspace.get(code).unwrap().selection(),
        Range::new(4, 7)
    );
    assert!(
        crate::context_menu::items(
            &app.context_menu.as_ref().unwrap().soggetto,
            app.menu_checks()
        )
        .iter()
        .any(|(_, action)| *action == MenuAct::LinkToOpenNote)
    );
    app.workspace
        .get_mut(code)
        .unwrap()
        .set_selection(Range::new(4, 7));
    let _ = app.update(right_click(9));
    assert_eq!(
        app.context_menu.as_ref().unwrap().soggetto.selezione(),
        None
    );
}

#[test]
fn scegliere_collega_all_appunto_avvia_il_salvataggio_e_chiude_il_menu() {
    use crate::editor_groups::GroupId;
    let (mut app, _project, _notes_dir, _store, note, code) = con_codice_e_nota();
    app.workspace
        .get_mut(code)
        .unwrap()
        .set_selection(Range::new(4, 7));
    let _ = app.update(Message::EditorIn(
        GroupId::Secondary,
        EditorCommand::ContextMenu {
            pos: 5,
            screen_x: 50,
            screen_y: 60,
        },
    ));
    let _ = app.update(Message::MenuAction(MenuAct::LinkToOpenNote));
    assert!(app.context_menu.is_none());
    assert!(matches!(
        app.notes.as_ref().unwrap().salvataggi[&note],
        crate::autosave::StatoSalvataggio::InCorso { .. }
    ));
    assert_eq!(app.saves.active_document(), Some(note));
}

#[test]
fn menu_link_appare_solo_con_un_soggetto_valido() {
    let (app, _) = Lume::new_without_store();
    assert_eq!(app.note_link_menu(), (false, false));
    let (mut app, project, notes_dir, _store, note, code) = con_codice_e_nota();
    assert_eq!(app.note_link_menu(), (true, true));
    app.groups.activate(note);
    assert_eq!(app.note_link_menu(), (false, false));
    app.groups.activate(code);
    app.project_root = Some(notes_dir.path().to_path_buf());
    assert_eq!(app.note_link_menu(), (false, false));
    app.project_root = Some(project.path().to_path_buf());
    app.close_tab(note);
    assert_eq!(app.note_link_menu(), (false, true));
}

fn collega_nota_al_codice(app: &mut Lume, doc: DocId, da: usize, a: usize) {
    let root = app.project_root.clone().unwrap();
    let link = lume_notes::CodeLink::try_new(root, "src/main.rs", da, a, None).unwrap();
    let id = app.notes.as_ref().unwrap().tabs[&doc].clone();
    let notes = app.notes.as_mut().unwrap();
    let loaded = notes.loaded.get_mut(&id).unwrap();
    loaded.note.add_link(link);
    let summary = loaded.summary();
    *notes
        .summaries
        .iter_mut()
        .find(|item| item.id == id)
        .unwrap() = summary;
    app.sync_notes_tool();
}

#[test]
fn indice_e_striscia_collegamenti_si_aggiornano_con_i_riepiloghi() {
    use crate::editor_groups::GroupId;
    let (mut app, _project, _notes_dir, _store, note, code) = con_codice_e_nota();
    assert!(app.notes.as_ref().unwrap().link_index.is_empty());
    assert!(app.striscia_collegamenti(GroupId::Primary).is_none());
    assert!(app.striscia_collegamenti(GroupId::Secondary).is_none());
    collega_nota_al_codice(&mut app, note, 2, 3);
    assert!(!app.notes.as_ref().unwrap().link_index.is_empty());
    assert!(app.striscia_collegamenti(GroupId::Primary).is_some());
    assert!(app.striscia_collegamenti(GroupId::Secondary).is_some());
    assert_ne!(note, code);
}

#[test]
fn chip_della_nota_apre_il_file_e_seleziona_le_righe() {
    let (mut app, _project, _notes_dir, _store, note, code) = con_codice_e_nota();
    collega_nota_al_codice(&mut app, note, 2, 2);
    app.groups.activate(note);
    let _ = app.update(Message::NotesOpenCode {
        doc: note,
        index: 0,
    });
    assert_eq!(app.active_doc(), Some(code));
    assert_eq!(
        app.workspace.get(code).unwrap().selection(),
        Range::new(4, 8)
    );
}

#[test]
fn rimuovere_un_link_avvia_il_salvataggio_della_nota() {
    let (mut app, _project, _notes_dir, _store, note, _code) = con_codice_e_nota();
    collega_nota_al_codice(&mut app, note, 2, 3);
    let _ = app.update(Message::NotesRemoveLink {
        doc: note,
        index: 0,
    });
    assert!(matches!(
        app.notes.as_ref().unwrap().salvataggi[&note],
        crate::autosave::StatoSalvataggio::InCorso { .. }
    ));
}

#[test]
fn le_righe_collegate_diventano_marcatori_del_file_di_codice() {
    let (mut app, _project, _notes_dir, _store, note, code) = con_codice_e_nota();
    assert!(app.notes.as_ref().unwrap().marcatori.is_empty());
    collega_nota_al_codice(&mut app, note, 2, 3);
    let notes = app.notes.as_ref().unwrap();
    assert_eq!(notes.marcatori[&code], vec![1..3]);
    assert!(!notes.marcatori.contains_key(&note));
    app.close_tab(code);
    assert!(!app.notes.as_ref().unwrap().marcatori.contains_key(&code));
}

#[test]
fn il_click_sul_marcatore_apre_l_appunto_sulla_riga_collegata() {
    use crate::editor_groups::GroupId;
    let (mut app, _project, _notes_dir, _store, note, code) = con_codice_e_nota();
    collega_nota_al_codice(&mut app, note, 2, 3);
    let click = |line| {
        Message::EditorIn(
            GroupId::Secondary,
            EditorCommand::NoteMarkerClicked { line },
        )
    };
    let _ = app.update(click(0));
    assert_eq!(app.active_doc(), Some(code));
    let _ = app.update(click(1));
    assert_eq!(app.active_doc(), Some(note));
}

#[test]
fn piu_appunti_sulla_stessa_riga_non_ne_sceglie_uno_a_caso() {
    use crate::editor_groups::GroupId;
    let (mut app, _project, _notes_dir, store, note, code) = con_codice_e_nota();
    let second = app.open_loaded_note(store.create("Seconda").unwrap());
    collega_nota_al_codice(&mut app, note, 2, 3);
    collega_nota_al_codice(&mut app, second, 2, 2);
    app.groups.activate(code);
    let _ = app.update(Message::EditorIn(
        GroupId::Secondary,
        EditorCommand::NoteMarkerClicked { line: 1 },
    ));
    assert_eq!(app.active_doc(), Some(code));
    assert!(app.notice.as_ref().unwrap().1.text.contains("Più appunti"));
}

#[test]
fn chiudere_tutti_gli_appunti_pulisce_indice_e_marcatori() {
    let (mut app, _project, _notes_dir, _store, note, _code) = con_codice_e_nota();
    collega_nota_al_codice(&mut app, note, 2, 3);
    assert!(!app.notes.as_ref().unwrap().marcatori.is_empty());
    app.close_all_note_tabs();
    let notes = app.notes.as_ref().unwrap();
    assert!(notes.marcatori.is_empty());
    assert!(notes.link_index.is_empty());
}

#[test]
fn nuovo_appunto_collegato_si_apre_dopo_la_creazione() {
    use futures::StreamExt;
    let (mut app, _project, _notes_dir, store, _note, code) = con_codice_e_nota();
    app.workspace
        .get_mut(code)
        .unwrap()
        .set_selection(Range::new(4, 7));
    let task = app.collega_a_nota_nuova(None);
    let mut stream = iced_runtime::task::into_stream(task).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let action = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(message) = action else {
        panic!("la creazione non ha prodotto un messaggio");
    };
    let _ = app.update(message);

    let notes = app.notes.as_ref().unwrap();
    let linked = notes
        .tabs
        .iter()
        .find(|(_, note_id)| {
            notes
                .loaded
                .get(*note_id)
                .is_some_and(|note| !note.note.header.links.is_empty())
        })
        .map(|(doc, note_id)| (*doc, note_id.clone()))
        .unwrap();
    assert_eq!(notes.loaded[&linked.1].note.header.links.len(), 1);
    assert_eq!(store.scan().unwrap().len(), 2);
    assert_eq!(
        app.workspace.get(linked.0).unwrap().path(),
        Some(notes.loaded[&linked.1].path())
    );
}

fn batti(app: &mut Lume, text: &str) {
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert(text.into()),
    ));
}

fn in_conflitto(
    app: &mut Lume,
    store: &lume_notes::NoteStore,
    loaded: &lume_notes::LoadedNote,
    doc: lume_core::DocId,
) -> String {
    batti(app, "mio testo non salvato\n");
    let local = app.workspace.get(doc).unwrap().rope().to_string();
    std::fs::write(loaded.path(), "# Altrove\n\nscritto da un altro editor\n").unwrap();
    let version = app.workspace.get(doc).unwrap().version();
    let task = app.update(Message::NotesAutosaveDue(doc, version));
    app.completa_salvataggi(task);
    assert!(app.conflitto_della_scheda(doc));
    assert!(
        store
            .load(loaded.path())
            .unwrap()
            .note
            .markdown
            .contains("scritto da un altro editor")
    );
    local
}

#[test]
fn conflitto_e_barra_esistono_solo_dopo_il_rifiuto_su_disco() {
    let (mut app, _dir, store, loaded, doc) = con_una_nota();
    assert!(!app.conflitto_della_scheda(doc));
    batti(&mut app, "x\n");
    assert!(!app.conflitto_della_scheda(doc));
    in_conflitto(&mut app, &store, &loaded, doc);
    assert!(app.conflitto_della_scheda(doc));
}

#[test]
fn ctrl_s_rileva_il_conflitto_e_mostra_la_barra() {
    let (mut app, _dir, _store, loaded, doc) = con_una_nota();
    batti(&mut app, "testo locale\n");
    std::fs::write(loaded.path(), "# Altrove\n").unwrap();

    let error = app.write_doc(doc).unwrap_err();

    assert!(error.contains("fallito"));
    assert!(app.conflitto_della_scheda(doc));
}

#[test]
fn ricarica_mostra_il_disco_e_undo_ripristina_il_testo_locale() {
    let (mut app, _dir, store, loaded, doc) = con_una_nota();
    let local = in_conflitto(&mut app, &store, &loaded, doc);

    let _ = app.update(Message::NotesResolveConflict { doc, copia: false });

    let document = app.workspace.get(doc).unwrap();
    assert!(
        document
            .rope()
            .to_string()
            .contains("scritto da un altro editor")
    );
    assert!(!document.is_dirty());
    assert!(!app.conflitto_della_scheda(doc));
    assert_eq!(
        app.notes.as_ref().unwrap().tabs.get(&doc),
        Some(&loaded.note.header.id)
    );
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Undo,
    ));
    assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), local);
}

#[test]
fn salva_copia_conserva_il_testo_locale_e_ricarica_la_scheda_originale() {
    let (mut app, _dir, store, loaded, doc) = con_una_nota();
    let local = in_conflitto(&mut app, &store, &loaded, doc);

    let _ = app.update(Message::NotesResolveConflict { doc, copia: true });

    assert!(
        app.workspace
            .get(doc)
            .unwrap()
            .rope()
            .to_string()
            .contains("scritto da un altro editor")
    );
    assert!(!app.conflitto_della_scheda(doc));
    let copies: Vec<_> = store
        .scan()
        .unwrap()
        .into_iter()
        .filter(|summary| summary.title.ends_with("(copia)"))
        .collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].title, "Prima (copia)");
    let copy_doc = app
        .notes
        .as_ref()
        .unwrap()
        .tabs
        .iter()
        .find(|(_, note_id)| **note_id == copies[0].id)
        .map(|(doc, _)| *doc)
        .expect("la copia è aperta in una scheda");
    assert_eq!(
        app.workspace.get(copy_doc).unwrap().rope().to_string(),
        local
    );
}

#[test]
fn una_modifica_alla_nota_arma_il_timer_e_il_salvataggio_la_pulisce() {
    let (mut app, _dir, store, loaded, doc) = con_una_nota();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("nuovo testo".into()),
    ));
    let version = app.workspace.get(doc).unwrap().version();
    assert_eq!(
        app.notes.as_ref().unwrap().salvataggi.get(&doc),
        Some(&crate::autosave::StatoSalvataggio::InAttesa { versione: version })
    );

    let task = app.update(Message::NotesAutosaveDue(doc, version));
    assert_eq!(
        app.notes.as_ref().unwrap().salvataggi.get(&doc),
        Some(&crate::autosave::StatoSalvataggio::InCorso { versione: version })
    );
    app.completa_salvataggi(task);

    assert!(!app.workspace.get(doc).unwrap().is_dirty());
    assert_eq!(
        app.notes.as_ref().unwrap().salvataggi.get(&doc),
        Some(&crate::autosave::StatoSalvataggio::Pulito)
    );
    assert_eq!(
        store.load(loaded.path()).unwrap().note.markdown,
        "nuovo testo"
    );
}

#[test]
fn modifica_durante_autosalvataggio_riarma_senza_falso_conflitto() {
    let (mut app, _dir, store, loaded, doc) = con_una_nota();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("prima".into()),
    ));
    let first_version = app.workspace.get(doc).unwrap().version();
    let task = app.update(Message::NotesAutosaveDue(doc, first_version));
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert(" dopo".into()),
    ));
    app.completa_salvataggi(task);

    let latest_version = app.workspace.get(doc).unwrap().version();
    assert!(app.workspace.get(doc).unwrap().is_dirty());
    assert_eq!(
        app.notes.as_ref().unwrap().salvataggi.get(&doc),
        Some(&crate::autosave::StatoSalvataggio::InAttesa {
            versione: latest_version
        })
    );
    assert_eq!(store.load(loaded.path()).unwrap().note.markdown, "prima");

    let task = app.update(Message::NotesAutosaveDue(doc, latest_version));
    app.completa_salvataggi(task);
    assert!(!app.workspace.get(doc).unwrap().is_dirty());
    assert_eq!(
        store.load(loaded.path()).unwrap().note.markdown,
        "prima dopo"
    );
}

#[test]
fn un_timer_superato_non_scrive_la_nota() {
    let (mut app, _dir, store, loaded, doc) = con_una_nota();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("prima".into()),
    ));
    let old_version = app.workspace.get(doc).unwrap().version();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("poi".into()),
    ));
    let disk_before = std::fs::read(loaded.path()).unwrap();

    let _ = app.update(Message::NotesAutosaveDue(doc, old_version));

    assert!(app.workspace.get(doc).unwrap().is_dirty());
    assert_eq!(std::fs::read(loaded.path()).unwrap(), disk_before);
    assert_eq!(
        app.notes.as_ref().unwrap().salvataggi.get(&doc),
        Some(&crate::autosave::StatoSalvataggio::InAttesa {
            versione: app.workspace.get(doc).unwrap().version()
        })
    );
    assert_eq!(store.scan().unwrap().len(), 1);
}

#[test]
fn conflitto_di_autosalvataggio_non_sovrascrive_il_file_esterno() {
    let (mut app, _dir, _store, loaded, doc) = con_una_nota();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("locale".into()),
    ));
    let version = app.workspace.get(doc).unwrap().version();
    let external = "+++:\\nforeign = true\\n+++\\n# esterno\\n";
    std::fs::write(loaded.path(), external).unwrap();

    let task = app.update(Message::NotesAutosaveDue(doc, version));
    app.completa_salvataggi(task);

    assert!(app.workspace.get(doc).unwrap().is_dirty());
    assert_eq!(
        app.notes.as_ref().unwrap().salvataggi.get(&doc),
        Some(&crate::autosave::StatoSalvataggio::Conflitto)
    );
    assert_eq!(std::fs::read_to_string(loaded.path()).unwrap(), external);
}

#[test]
fn titolo_nel_testo_rinomina_file_riepilogo_e_scheda() {
    let (mut app, _dir, store, loaded, doc) = con_una_nota();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("# Idea nuova\n".into()),
    ));
    let version = app.workspace.get(doc).unwrap().version();
    let task = app.update(Message::NotesAutosaveDue(doc, version));
    app.completa_salvataggi(task);

    let nuovo = app
        .workspace
        .get(doc)
        .unwrap()
        .path()
        .unwrap()
        .to_path_buf();
    assert_ne!(nuovo, loaded.path());
    assert!(!loaded.path().exists());
    assert!(nuovo.is_file());
    assert_eq!(app.editor_tab_name(doc).as_deref(), Some("Idea nuova"));
    let notes = app.notes.as_ref().unwrap();
    assert_eq!(notes.summaries.len(), 1);
    assert_eq!(notes.summaries[0].title, "Idea nuova");
    assert_eq!(notes.summaries[0].path, nuovo);
    assert_eq!(store.scan().unwrap().len(), 1);
    assert!(!app.workspace.get(doc).unwrap().is_dirty());
}

#[test]
fn titolo_con_lo_stesso_nome_base_non_duplica_il_file() {
    let (mut app, dir, store, loaded, doc) = con_una_nota();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("# PRIMA!\n".into()),
    ));
    let version = app.workspace.get(doc).unwrap().version();
    let task = app.update(Message::NotesAutosaveDue(doc, version));
    app.completa_salvataggi(task);

    assert_eq!(app.workspace.get(doc).unwrap().path(), Some(loaded.path()));
    assert_eq!(app.editor_tab_name(doc).as_deref(), Some("PRIMA!"));
    assert_eq!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
            .count(),
        1
    );
    assert_eq!(store.scan().unwrap()[0].title, "PRIMA!");
}

#[test]
fn testo_senza_titolo_non_cambia_nome_o_titolo_nota() {
    let (mut app, _dir, _store, loaded, doc) = con_una_nota();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("solo corpo, nessun titolo\n".into()),
    ));
    let version = app.workspace.get(doc).unwrap().version();
    let task = app.update(Message::NotesAutosaveDue(doc, version));
    app.completa_salvataggi(task);

    assert_eq!(app.workspace.get(doc).unwrap().path(), Some(loaded.path()));
    assert_eq!(app.editor_tab_name(doc).as_deref(), Some("Prima"));
}

#[test]
fn ctrl_s_rinomina_la_nota_come_l_autosalvataggio() {
    let (mut app, _dir, _store, loaded, doc) = con_una_nota();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("# Titolo da Ctrl+S\n".into()),
    ));

    assert_eq!(app.write_doc(doc), Ok(()));
    let nuovo = app
        .workspace
        .get(doc)
        .unwrap()
        .path()
        .unwrap()
        .to_path_buf();
    assert_ne!(nuovo, loaded.path());
    assert!(nuovo.is_file());
    assert_eq!(
        app.editor_tab_name(doc).as_deref(),
        Some("Titolo da Ctrl+S")
    );
}

#[test]
fn ctrl_s_durante_autosalvataggio_accoda_senza_sovrapporre_i_worker() {
    let (mut app, _dir, store, loaded, doc) = con_una_nota();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("testo".into()),
    ));
    let version = app.workspace.get(doc).unwrap().version();
    let autosave = app.update(Message::NotesAutosaveDue(doc, version));
    let job = app.save_job.clone().unwrap();
    let manuale = app.update(Message::SaveActive);
    assert_eq!(app.save_job.as_ref(), Some(&job));
    assert!(app.notice.as_ref().unwrap().1.text.contains("accodato"));
    app.completa_salvataggi(iced::Task::batch([autosave, manuale]));
    assert_eq!(store.load(loaded.path()).unwrap().note.markdown, "testo");
    assert!(!app.workspace.get(doc).unwrap().is_dirty());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn senza_cestino_la_nota_resta_e_l_errore_e_in_italiano() {
    let (mut app, _dir, _store, loaded, _doc) = con_una_nota();
    let id = loaded.note.header.id.clone();
    let task = app.delete_note(id.clone());
    let preparato = super::appunti_scarico_tests::risultato_task(task);
    let _worker_non_eseguito = app.update(preparato);
    let deletion = app.note_delete.as_ref().unwrap();
    let (request, scope, lease) = (deletion.request, deletion.scope, deletion.lease);
    // Questo è solo un esito sintetico per il gestore autenticato; il backend
    // del Cestino vero è verificato separatamente nei test Appunti.
    let _ = app.update(Message::NotesDeleted(
        request,
        id.clone(),
        scope,
        lease,
        Ok(DeleteDisposition::NeedsPermanentConfirmation),
    ));
    assert_eq!(
        app.persistence_error.as_deref(),
        Some("Il Cestino non è disponibile: la nota non è stata eliminata")
    );
    assert!(
        app.notes
            .as_ref()
            .unwrap()
            .summaries
            .iter()
            .any(|summary| summary.id == id)
    );
}
