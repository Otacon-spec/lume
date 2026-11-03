//! Owner app e receipt autentiche; I/O reale in TempDir. Non GUI/native exit.
use super::*;
use futures::StreamExt;

#[path = "save_owned_stale_tests.rs"]
mod stale_tests;

fn fallback(app: &Lume, job: &crate::save::Job) -> futures::stream::BoxStream<'static, Message> {
    use iced_runtime::futures::subscription::{Hasher, Recipe, into_recipes};
    use std::hash::Hasher as _;
    let hash = |recipe: &dyn Recipe<Output = Message>| {
        let mut h = Hasher::default();
        recipe.hash(&mut h);
        h.finish()
    };
    // Seleziona la recipe realmente prodotta dall'app senza pollare listener
    // OS estranei. L'hash è identità strutturale Iced, NON autorità/prova GUI.
    let expected =
        iced::Subscription::run_with(job.clone(), crate::save::completed).map(Lume::save_event);
    let key = hash(&*into_recipes(expected).pop().unwrap());
    let mut recipes: Vec<_> = into_recipes(app.subscription())
        .into_iter()
        .filter(|r| hash(&**r) == key)
        .collect();
    assert_eq!(recipes.len(), 1);
    recipes
        .pop()
        .unwrap()
        .stream(futures::stream::empty().boxed())
}

#[test]
fn recipe_app_recupera_ready_e_done_del_save_as_dopo_drop_del_producer() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, old) = prepara(&mut app, dir.path());
    let path = dir.path().join("dopo.txt");
    let task = app.save_as(id, path.clone());
    let job = app.save_job.clone().unwrap();
    let mut collector = fallback(&app, &job);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), collector.next())
                .await
                .is_err()
        );
        assert!(!job.running(), "recipe Pending non ammette lavoro");
        assert!(!path.exists());
        let mut producer = iced_runtime::task::into_stream(task).unwrap();
        let iced_runtime::Action::Output(Message::SaveReady(first)) =
            tokio::time::timeout(std::time::Duration::from_secs(3), producer.next())
                .await
                .unwrap()
                .unwrap()
        else {
            panic!("Ready effettivo della Task assente");
        };
        assert!(job.matches_ready(&first));
        assert!(!path.exists());
        drop(producer);
        let Message::SaveReady(recovered) =
            tokio::time::timeout(std::time::Duration::from_secs(3), collector.next())
                .await
                .unwrap()
                .unwrap()
        else {
            panic!("Ready autentico della recipe app assente");
        };
        assert!(job.matches_ready(&recovered));
        drop(first);
        drop(app.update(Message::SaveReady(recovered)));
        let Message::SaveCollected(receipt) =
            tokio::time::timeout(std::time::Duration::from_secs(3), collector.next())
                .await
                .unwrap()
                .unwrap()
        else {
            panic!("Done autentico della recipe app assente");
        };
        assert!(job.matches(&receipt));
        let replay = receipt.clone();
        drop(app.update(Message::SaveCollected(receipt)));
        assert!(collector.next().await.is_none());
        assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "prima");
        assert_eq!(app.workspace.get(id).unwrap().path(), Some(path.as_path()));
        assert!(!app.workspace.get(id).unwrap().is_dirty());
        drop(app.update(Message::SaveCollected(replay)));
        assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");
    });
}

fn prepara(app: &mut Lume, dir: &std::path::Path) -> (lume_core::DocId, std::path::PathBuf) {
    let path = write_and_open(app, dir, "a.txt", "prima");
    let id = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));
    (id, path)
}

fn rinomina_durante_save_reale(stesso_file: bool) {
    use crate::prompt::PromptMessage;
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.set_project_root(dir.path().to_path_buf());
    let (id, salvato) = prepara(&mut app, dir.path());
    let target = if stesso_file {
        salvato.clone()
    } else {
        let altro = dir.path().join("altro.txt");
        std::fs::write(&altro, "altro").unwrap();
        altro
    };
    let nuovo = dir.path().join("dopo.txt");
    let task = app.update(Message::SaveActive);
    let job = app.save_job.clone().expect("Job reale ammesso");
    let lease = app.save_lease.expect("lease del salvataggio");
    assert_eq!(app.saves.active_document(), Some(id));
    let mut stream = iced_runtime::task::into_stream(task).unwrap();
    let runtime = runtime_file_tests();
    let ready = runtime.block_on(async {
        let action = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
            .await
            .unwrap()
            .unwrap();
        let iced_runtime::Action::Output(Message::SaveReady(ready)) = action else {
            panic!("Ready autentico assente");
        };
        ready
    });
    assert!(job.matches_ready(&ready));
    assert_eq!(std::fs::read_to_string(&salvato).unwrap(), "prima");
    drop(app.update(Message::Tool(
        crate::tools::project::ID,
        lume_ui::tool::ToolEvent::new(crate::tools::project::Event::RowRightClicked(
            target.clone(),
            false,
            10.0,
            20.0,
        )),
    )));
    drop(app.update(Message::MenuAction(MenuAct::RenameTreeFile)));
    drop(app.update(Message::AlberoRinomina(PromptMessage::Changed(
        "dopo.txt".into(),
    ))));
    let vista = app.scritture.guarda(Some((id, Some(&salvato))));
    assert_eq!(vista.in_volo, Some(crate::scritture::Tipo::Salvataggio));
    assert_eq!(vista.file, crate::scritture::Presa::InScrittura);
    drop(app.update(Message::AlberoRinomina(PromptMessage::Confirm)));
    assert_eq!(app.scritture.guarda(Some((id, Some(&salvato)))), vista);
    assert_eq!(app.save_lease, Some(lease));
    assert_eq!(app.save_job.as_ref(), Some(&job));
    assert_eq!(app.saves.active_document(), Some(id));
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        if stesso_file { "prima" } else { "altro" }
    );
    assert!(!nuovo.exists());
    let richiesta = app.albero_rinomina.as_ref().unwrap();
    assert_eq!(richiesta.percorso, target);
    assert_eq!(richiesta.valore, "dopo.txt");
    assert!(richiesta.errore.is_none());
    assert!(app.albero_rinomina_prompt().unwrap().ready);
    assert!(testo_avviso(&app).contains("salvataggio"));

    drop(app.update(Message::SaveReady(ready)));
    let seguito = runtime.block_on(async {
        let action = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
            .await
            .unwrap()
            .unwrap();
        let iced_runtime::Action::Output(Message::SaveCollected(receipt)) = action else {
            panic!("ricevuta autentica assente");
        };
        assert!(job.matches(&receipt));
        app.update(Message::SaveCollected(receipt))
    });
    app.completa_salvataggi(seguito);
    assert!(app.save_lease.is_none());
    assert!(app.save_job.is_none());
    assert_eq!(std::fs::read_to_string(&salvato).unwrap(), "Xprima");
    let prima_retry = app.scritture.guarda(None).epoca;
    assert!(app.albero_rinomina_prompt().unwrap().ready);
    drop(app.update(Message::AlberoRinomina(PromptMessage::Confirm)));
    assert!(!target.exists());
    assert_eq!(
        std::fs::read_to_string(&nuovo).unwrap(),
        if stesso_file { "Xprima" } else { "altro" }
    );
    assert_eq!(app.scritture.guarda(None).epoca, prima_retry + 1);
    assert!(app.scritture.guarda(None).in_volo.is_none());
    assert!(app.albero_rinomina.is_none());
    assert_eq!(
        app.workspace.get(id).unwrap().path(),
        Some(if stesso_file {
            nuovo.as_path()
        } else {
            salvato.as_path()
        })
    );
}

#[test]
fn rinomina_attende_save_reale_sullo_stesso_file() {
    rinomina_durante_save_reale(true);
}

#[test]
fn rinomina_attende_save_reale_su_file_estraneo() {
    rinomina_durante_save_reale(false);
}

#[test]
fn chooser_occupa_il_coordinatore_e_un_secondo_chooser_non_lo_sostituisce() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, _) = prepara(&mut app, dir.path());
    let dialogo = app.pick_save_path(id);
    let sequenza = app.save_as_picker.as_ref().unwrap().sequence;
    assert_eq!(
        app.scritture.guarda(Some((id, None))).file,
        crate::scritture::Presa::InScrittura
    );
    assert!((app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
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
    drop(app.pick_save_path(id));
    assert_eq!(app.save_as_picker.as_ref().unwrap().sequence, sequenza);
    assert!(testo_avviso(&app).contains("corso"));
    drop(app.update(Message::SaveAsPicked(sequenza, None)));
    assert!(app.save_as_picker.is_none());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.workspace.get(id).unwrap().is_dirty());
    drop(dialogo);
}

#[test]
fn chooser_trasferisce_la_stessa_presa_al_worker_senza_quiete() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, old) = prepara(&mut app, dir.path());
    let dialogo = app.pick_save_path(id);
    let sequenza = app.save_as_picker.as_ref().unwrap().sequence;
    assert_eq!(
        app.scritture.guarda(None).in_volo,
        Some(crate::scritture::Tipo::SalvaConNome)
    );
    let epoca = app.scritture.guarda(None).epoca;
    let path = dir.path().join("nuovo.txt");
    let worker = app.update(Message::SaveAsPicked(sequenza, Some(path.clone())));
    assert!(app.save_as_picker.is_none());
    assert!(app.save_as_flight.is_some());
    assert_eq!(app.scritture.guarda(None).epoca, epoca);
    assert!((app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
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
    app.completa_salvataggi(worker);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "Xprima");
    assert_eq!(std::fs::read_to_string(old).unwrap(), "prima");
    drop(dialogo);
}

#[test]
fn recipe_app_raccoglie_chooser_abbandonato_prima_del_primo_poll() {
    use iced_runtime::futures::subscription::{Hasher, Recipe, into_recipes};
    use std::hash::Hasher as _;
    let hash = |recipe: &dyn Recipe<Output = Message>| {
        let mut h = Hasher::default();
        recipe.hash(&mut h);
        h.finish()
    };
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, _) = prepara(&mut app, dir.path());
    let precedenti: std::collections::HashSet<_> = into_recipes(app.subscription())
        .into_iter()
        .map(|recipe| hash(&*recipe))
        .collect();
    let dialogo = app.pick_save_path(id);
    let mut nuove: Vec<_> = into_recipes(app.subscription())
        .into_iter()
        .filter(|recipe| !precedenti.contains(&hash(&**recipe)))
        .collect();
    assert_eq!(
        nuove.len(),
        1,
        "il chooser ha una recipe posseduta dall'app"
    );
    let mut collector = nuove
        .pop()
        .unwrap()
        .stream(futures::stream::empty().boxed());
    drop(dialogo);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let messaggio = runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(3), collector.next())
            .await
            .unwrap()
            .unwrap()
    });
    drop(app.update(messaggio));
    assert!(app.save_as_picker.is_none());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(testo_avviso(&app).contains("annull"));
}

#[test]
fn risultato_di_un_altro_chooser_con_stessa_sequenza_non_libera_il_proprietario() {
    use iced_runtime::futures::subscription::{Hasher, Recipe, into_recipes};
    use std::hash::Hasher as _;
    fn abbandona(app: &mut Lume, id: lume_core::DocId) -> Message {
        let hash = |recipe: &dyn Recipe<Output = Message>| {
            let mut h = Hasher::default();
            recipe.hash(&mut h);
            h.finish()
        };
        let precedenti: std::collections::HashSet<_> = into_recipes(app.subscription())
            .into_iter()
            .map(|recipe| hash(&*recipe))
            .collect();
        let task = app.pick_save_path(id);
        let mut nuove: Vec<_> = into_recipes(app.subscription())
            .into_iter()
            .filter(|recipe| !precedenti.contains(&hash(&**recipe)))
            .collect();
        assert_eq!(nuove.len(), 1);
        let mut collector = nuove
            .pop()
            .unwrap()
            .stream(futures::stream::empty().boxed());
        drop(task);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(3), collector.next())
                .await
                .unwrap()
                .unwrap()
        })
    }
    let dir = tempfile::tempdir().unwrap();
    let mut prima = app();
    let (primo, _) = prepara(&mut prima, dir.path());
    let proprio = abbandona(&mut prima, primo);
    let mut seconda = app();
    let (secondo, _) = prepara(&mut seconda, dir.path());
    let estraneo = abbandona(&mut seconda, secondo);
    assert_eq!(
        prima.save_as_picker.as_ref().unwrap().sequence,
        seconda.save_as_picker.as_ref().unwrap().sequence
    );
    drop(prima.update(estraneo));
    assert!(prima.save_as_picker.is_some());
    assert!((prima.scritture.guarda(None).in_volo.is_some() || prima.git_operazione.is_some()));
    drop(prima.update(proprio));
    assert!(prima.save_as_picker.is_none());
    assert!(!(prima.scritture.guarda(None).in_volo.is_some() || prima.git_operazione.is_some()));
}

#[test]
fn chooser_rifiuta_tutti_i_salvataggi_utente_senza_accodarli() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, old) = prepara(&mut app, dir.path());
    let altro = write_and_open(&mut app, dir.path(), "altro.txt", "altro");
    let altro_id = app.active_doc().unwrap();
    drop(app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("Y".into()),
    )));
    let dialogo = app.pick_save_path(id);
    let sequenza = app.save_as_picker.as_ref().unwrap().sequence;
    for messaggio in [
        Message::SaveActive,
        Message::MenuAction(MenuAct::Save),
        Message::MenuAction(MenuAct::SaveAll),
    ] {
        app.notice = None;
        drop(app.update(messaggio));
        assert!(testo_avviso(&app).contains("Salva con nome"));
        assert!(
            !(app.scritture.contiene_salvataggio(altro_id)
                || app.saves.active_document() == Some(altro_id))
        );
    }
    drop(app.update(Message::MenuAction(MenuAct::Exit)));
    app.notice = None;
    drop(app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Save,
    )));
    assert!(testo_avviso(&app).contains("Rispondi prima"));
    assert!(
        !(app.scritture.contiene_salvataggio(altro_id)
            || app.saves.active_document() == Some(altro_id))
    );
    app.notice = None;
    drop(app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::Salva,
    )));
    assert!(testo_avviso(&app).contains("Salva con nome"));
    assert!(app.close_after_save.is_none());
    assert!(!(app.scritture.contiene_salvataggio(id) || app.saves.active_document() == Some(id)));
    assert!(
        !(app.scritture.contiene_salvataggio(altro_id)
            || app.saves.active_document() == Some(altro_id))
    );
    drop(app.update(Message::SaveAsPicked(sequenza, None)));
    assert!(app.chiusura_attiva().is_some());
    assert_eq!(std::fs::read_to_string(old).unwrap(), "prima");
    assert_eq!(std::fs::read_to_string(altro).unwrap(), "altro");
    drop(dialogo);
}

#[test]
fn lettura_di_avvio_tardiva_non_cambia_progetto_durante_il_chooser() {
    let dir = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, old) = prepara(&mut app, dir.path());
    app.project_root = Some(root.path().to_path_buf());
    let read = leggi_per_apertura(
        old,
        DopoApertura::Avvio {
            fuori_progetto: true,
        },
    );
    let dialogo = app.pick_save_path(id);
    let sequenza = app.save_as_picker.as_ref().unwrap().sequence;
    drop(app.update(Message::FileLetto(read)));
    assert_eq!(app.project_root.as_deref(), Some(root.path()));
    assert!(testo_avviso(&app).contains("Salva con nome"));
    drop(app.update(Message::SaveAsPicked(sequenza, None)));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    drop(dialogo);
}

#[test]
fn chiusura_di_scheda_pulita_attende_annullamento_del_chooser() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "pulito.txt", "prima");
    let id = app.active_doc().unwrap();
    let dialogo = app.pick_save_path(id);
    let sequenza = app.save_as_picker.as_ref().unwrap().sequence;
    drop(app.update(Message::EditorTabBar(
        crate::editor_tab_bar::EditorTabBarMessage::Close(id),
    )));
    assert!(app.chiusura_attiva().is_some());
    assert!(app.workspace.get(id).is_some());
    assert!((app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    drop(app.update(Message::SaveAsPicked(sequenza, None)));
    assert!(app.workspace.get(id).is_none());
    assert!(!app.groups.contains(id));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "prima");
    drop(dialogo);
}

#[test]
fn flight_diretto_rifiuta_salvataggi_e_la_chiusura_non_li_accoda() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, old) = prepara(&mut app, dir.path());
    let altro = write_and_open(&mut app, dir.path(), "altro.txt", "altro");
    let altro_id = app.active_doc().unwrap();
    drop(app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("Y".into()),
    )));
    let path = dir.path().join("nuovo.txt");
    let worker = app.save_as(id, path.clone());
    assert_eq!(
        app.scritture.guarda(None).in_volo,
        Some(crate::scritture::Tipo::SalvaConNome)
    );
    for messaggio in [
        Message::SaveActive,
        Message::MenuAction(MenuAct::Save),
        Message::MenuAction(MenuAct::SaveAll),
    ] {
        app.notice = None;
        drop(app.update(messaggio));
        assert!(testo_avviso(&app).contains("Salva con nome"));
        assert!(
            !(app.scritture.contiene_salvataggio(altro_id)
                || app.saves.active_document() == Some(altro_id))
        );
    }
    drop(app.update(Message::MenuAction(MenuAct::Exit)));
    drop(app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::Salva,
    )));
    assert!(testo_avviso(&app).contains("Salva con nome"));
    assert!(app.close_after_save.is_none());
    assert!(
        !(app.scritture.contiene_salvataggio(altro_id)
            || app.saves.active_document() == Some(altro_id))
    );
    app.completa_salvataggi(worker);
    assert!(!path.exists());
    assert_eq!(std::fs::read_to_string(old).unwrap(), "prima");
    assert_eq!(std::fs::read_to_string(altro).unwrap(), "altro");
    assert!(app.chiusura_attiva().is_some());
}

#[test]
fn autosave_reale_rimandato_dal_chooser_riprende_dopo_il_flight() {
    let dir = tempfile::tempdir().unwrap();
    let note_dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, _) = prepara(&mut app, dir.path());
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(note_dir.path()).unwrap();
    app.finish_notes_load(generation, note_dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let note_path = loaded.path().to_path_buf();
    let note_doc = app.open_loaded_note(loaded);
    let prima = std::fs::read(&note_path).unwrap();
    drop(app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("modifica automatica".into()),
    )));
    let versione = app.workspace.get(note_doc).unwrap().version();
    let dialogo = app.pick_save_path(id);
    let sequenza = app.save_as_picker.as_ref().unwrap().sequence;
    app.notice = None;
    drop(app.update(Message::NotesAutosaveDue(note_doc, versione)));
    assert!(
        (app.scritture.contiene_salvataggio(note_doc)
            || app.saves.active_document() == Some(note_doc))
    );
    assert_eq!(app.saves.active_document(), None);
    assert_eq!(std::fs::read(&note_path).unwrap(), prima);
    assert!(testo_avviso(&app).contains("rimandato"));
    let destination = dir.path().join("nuovo.txt");
    let worker = app.update(Message::SaveAsPicked(sequenza, Some(destination.clone())));
    assert!(app.save_as_flight.is_some());
    assert_eq!(app.saves.active_document(), Some(id));
    app.completa_salvataggi(worker);
    assert_eq!(std::fs::read_to_string(destination).unwrap(), "Xprima");
    assert_eq!(
        store.load(&note_path).unwrap().note.markdown,
        "modifica automatica"
    );
    assert!(!app.workspace.get(note_doc).unwrap().is_dirty());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    drop(dialogo);
}

#[test]
fn chooser_con_due_note_rimandate_conserva_lease_e_ricontrolla_alla_quiete() {
    let dir = tempfile::tempdir().unwrap();
    let note_dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (code, _) = prepara(&mut app, dir.path());
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(note_dir.path()).unwrap();
    app.finish_notes_load(generation, note_dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let mut note = Vec::new();
    for text in ["prima", "seconda"] {
        let loaded = store.create(text).unwrap();
        let path = loaded.path().to_path_buf();
        let id = app.open_loaded_note(loaded);
        drop(app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert(text.into()),
        )));
        note.push((id, path, app.workspace.get(id).unwrap().version(), text));
    }
    let (id, path, _, _) = note.first().unwrap();
    let read = primo_output(app.reload_doc(*id, path.clone()));
    let dialogo = app.pick_save_path(code);
    let picker = app.save_as_picker.as_ref().unwrap();
    let lease = picker.lease;
    let sequence = picker.sequence;
    let epoch = app.scritture.guarda(None).epoca;
    for (id, _, version, _) in &note {
        drop(app.update(Message::NotesAutosaveDue(*id, *version)));
        assert_eq!(
            app.scritture
                .guarda(Some((*id, app.workspace.get(*id).unwrap().path())))
                .file,
            crate::scritture::Presa::InScrittura
        );
    }
    assert!(app.scritture.guarda(None).epoca > epoch);
    assert!(
        app.scritture
            .possiede(lease, crate::scritture::Tipo::SalvaConNome)
    );
    let prima = app.workspace.get(*id).unwrap().rope().to_string();
    drop(app.update(read));
    assert_eq!(app.workspace.get(*id).unwrap().rope().to_string(), prima);
    assert!(app.pending_fs_rechecks.contains(id));
    let destination = dir.path().join("nuovo.txt");
    let task = app.update(Message::SaveAsPicked(sequence, Some(destination.clone())));
    assert_eq!(app.save_as_flight.as_ref().unwrap().lease, lease);
    let runtime = runtime_file_tests();
    runtime.block_on(async {
        let mut streams = futures::stream::SelectAll::new();
        streams.push(iced_runtime::task::into_stream(task).unwrap());
        while let Some(action) = streams.next().await {
            if let iced_runtime::Action::Output(message) = action
                && let Some(next) = iced_runtime::task::into_stream(app.update(message))
            {
                streams.push(next);
            }
        }
    });
    assert_eq!(std::fs::read_to_string(destination).unwrap(), "Xprima");
    assert!(!app.pending_fs_rechecks.contains(id));
    for (id, path, _, text) in note {
        assert_eq!(store.load(path).unwrap().note.markdown, text);
        assert!(!app.workspace.get(id).unwrap().is_dirty());
    }
    drop(dialogo);
}

#[test]
fn salva_con_nome_rifiuta_nota_reale_sia_dal_chooser_sia_dal_percorso_diretto() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let original = std::fs::read(loaded.path()).unwrap();
    let note = app.open_loaded_note(loaded.clone());
    let destination = dir.path().join("copia.md");
    drop(app.update(Message::MenuAction(crate::menu::MenuAct::SaveAs)));
    assert!(app.save_as_picker.is_none());
    assert!(testo_avviso(&app).contains("nessun file"));
    drop(app.pick_save_path(note));
    assert!(app.save_as_picker.is_none());
    assert!(testo_avviso(&app).contains("Appunti"));
    drop(app.save_as(note, destination.clone()));
    assert!(testo_avviso(&app).contains("Appunti"));
    assert!(!destination.exists());
    assert_eq!(std::fs::read(loaded.path()).unwrap(), original);
    assert!(app.save_job.is_none());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn salva_con_nome_codice_riprende_due_note_estranee_senza_timer_appeso() {
    let dir = tempfile::tempdir().unwrap();
    let note_dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (code, original) = prepara(&mut app, dir.path());
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(note_dir.path()).unwrap();
    app.finish_notes_load(generation, note_dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let mut note = Vec::new();
    for text in ["prima nota", "seconda nota"] {
        let loaded = store.create(text).unwrap();
        let path = loaded.path().to_path_buf();
        let id = app.open_loaded_note(loaded);
        drop(app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert(text.into()),
        )));
        note.push((id, path, app.workspace.get(id).unwrap().version(), text));
    }
    let destination = dir.path().join("nuovo.txt");
    let save_as = app.save_as(code, destination.clone());
    for (id, _, version, _) in &note {
        drop(app.update(Message::NotesAutosaveDue(*id, *version)));
        assert!(testo_avviso(&app).contains("rimandato"));
    }
    app.completa_salvataggi(save_as);
    assert_eq!(std::fs::read_to_string(original).unwrap(), "prima");
    assert_eq!(std::fs::read_to_string(destination).unwrap(), "Xprima");
    for (id, path, _, text) in note {
        assert_eq!(store.load(path).unwrap().note.markdown, text);
        assert!(!app.workspace.get(id).unwrap().is_dirty());
        assert_eq!(
            app.notes.as_ref().unwrap().salvataggi.get(&id),
            Some(&crate::autosave::StatoSalvataggio::Pulito)
        );
    }
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn rebind_revoca_solo_il_pendente_vecchio_senza_toccare_il_successore() {
    let dir = tempfile::tempdir().unwrap();
    let note_dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (code, original) = prepara(&mut app, dir.path());
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(note_dir.path()).unwrap();
    app.finish_notes_load(generation, note_dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let mut note = Vec::new();
    for text in ["prima nota", "seconda nota"] {
        let loaded = store.create(text).unwrap();
        let path = loaded.path().to_path_buf();
        let id = app.open_loaded_note(loaded);
        drop(app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert(text.into()),
        )));
        note.push((id, path, app.workspace.get(id).unwrap().version(), text));
    }
    let destination = dir.path().join("nuovo.txt");
    let save_as = app.save_as(code, destination.clone());
    // Setup difensivo: un automatico su una scheda Code non arriva dal timer Appunti.
    let (first, _, version, _) = &note[0];
    drop(app.update(Message::NotesAutosaveDue(*first, *version)));
    app.queue_save_per(code, crate::scritture::Causa::Automatico);
    let code_version = app.workspace.get(code).unwrap().version();
    app.notes.as_mut().unwrap().salvataggi.insert(
        code,
        crate::autosave::StatoSalvataggio::InCorso {
            versione: code_version,
        },
    );
    let (second, _, version, _) = &note[1];
    drop(app.update(Message::NotesAutosaveDue(*second, *version)));
    assert!(app.save_origins.contains_key(&code));
    let runtime = runtime_file_tests();
    let next = runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(save_as).unwrap();
        let iced_runtime::Action::Output(Message::SaveReady(ready)) = stream.next().await.unwrap()
        else {
            panic!("Ready SaveAs assente")
        };
        drop(app.update(Message::SaveReady(ready)));
        let iced_runtime::Action::Output(done @ Message::SaveCollected(_)) =
            stream.next().await.unwrap()
        else {
            panic!("Done SaveAs assente")
        };
        app.update(done)
    });
    assert_eq!(app.saves.active_document(), Some(*first));
    assert!(app.save_origins.contains_key(second));
    assert!(
        !app.save_origins.contains_key(&code),
        "origine della richiesta revocata rimasta orfana"
    );
    assert!(!matches!(
        app.notes.as_ref().unwrap().salvataggi.get(&code),
        Some(crate::autosave::StatoSalvataggio::InCorso { .. })
    ));
    assert!(!app.scritture.contiene_salvataggio(code));
    app.completa_salvataggi(next);
    assert_eq!(std::fs::read_to_string(original).unwrap(), "prima");
    assert_eq!(std::fs::read_to_string(destination).unwrap(), "Xprima");
    for (id, path, _, text) in note {
        assert_eq!(store.load(path).unwrap().note.markdown, text);
        assert_eq!(
            app.notes.as_ref().unwrap().salvataggi.get(&id),
            Some(&crate::autosave::StatoSalvataggio::Pulito)
        );
    }
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn chiusura_attende_tutti_i_save_e_l_ultimo_errore_resta_visibile() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (_, primo) = prepara(&mut app, dir.path());
    let secondo = write_and_open(&mut app, dir.path(), "secondo.txt", "prima");
    let id = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("X".into()),
    ));
    let _ = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
    let task = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::Salva,
    ));
    assert!(app.chiusura_attiva().is_some());
    let doc = app.workspace.get_mut(id).unwrap();
    doc.apply(lume_core::Transaction::new(
        lume_core::ChangeSet::insert_at(0, "Y", doc.rope().len_chars()),
    ));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(primo).unwrap(), "Xprima");
    assert_eq!(std::fs::read_to_string(secondo).unwrap(), "prima");
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(
        app.riquadro_chiusura()
            .unwrap()
            .errore
            .as_deref()
            .unwrap()
            .contains("cambiato")
    );
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn revisione_scaduta_non_avviata_libera_il_successore_senza_salvare_la_nuova() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (primo, first_path) = prepara(&mut app, dir.path());
    let second_path = write_and_open(&mut app, dir.path(), "b.txt", "secondo");
    let secondo = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("B".into()),
    ));
    let third_path = write_and_open(&mut app, dir.path(), "c.txt", "terzo");
    let terzo = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("C".into()),
    ));
    let task = app.update(Message::SaveDocFixture(primo));
    let _ = app.update(Message::SaveDocFixture(secondo));
    let _ = app.update(Message::SaveDocFixture(terzo));
    let doc = app.workspace.get_mut(secondo).unwrap();
    doc.apply(lume_core::Transaction::new(
        lume_core::ChangeSet::insert_at(0, "X", doc.rope().len_chars()),
    ));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(first_path).unwrap(), "Xprima");
    assert_eq!(std::fs::read_to_string(second_path).unwrap(), "secondo");
    assert_eq!(std::fs::read_to_string(third_path).unwrap(), "Cterzo");
    assert!(app.workspace.get(secondo).unwrap().is_dirty());
    assert!(!app.workspace.get(terzo).unwrap().is_dirty());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(testo_avviso(&app).contains("cambiato"));
}

#[test]
fn due_autosave_rimandati_annunciano_ogni_ripartenza_con_save_utente_interposto() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let (code, code_path) = prepara(&mut app, dir.path());
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let mut note = Vec::new();
    for title in ["Prima", "Seconda"] {
        let loaded = store.create(title).unwrap();
        let id = app.open_loaded_note(loaded);
        let _ = app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert(title.into()),
        ));
        note.push((id, app.workspace.get(id).unwrap().version(), title));
    }
    let chooser = app.pick_save_path(code);
    let sequence = app.save_as_picker.as_ref().unwrap().sequence;
    for (id, version, _) in &note {
        let _ = app.update(Message::NotesAutosaveDue(*id, *version));
        assert!(testo_avviso(&app).contains("rimandato"));
    }
    let primo = app.update(Message::SaveAsPicked(sequence, None));
    assert_eq!(app.saves.active_document(), Some(note[0].0));
    assert!(testo_avviso(&app).contains("ripartito"));
    let first_notice = app.notice_seq;
    let manuale = app.update(Message::SaveDocFixture(code));
    assert!(testo_avviso(&app).contains("accodato"));
    let runtime = runtime_file_tests();
    let secondo = runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(primo).unwrap();
        let iced_runtime::Action::Output(Message::SaveReady(ready)) = stream.next().await.unwrap()
        else {
            panic!("Ready primo autosave")
        };
        drop(app.update(Message::SaveReady(ready)));
        let iced_runtime::Action::Output(done @ Message::SaveCollected(_)) =
            stream.next().await.unwrap()
        else {
            panic!("Done primo autosave")
        };
        app.update(done)
    });
    assert_eq!(app.saves.active_document(), Some(code));
    let terzo = runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(secondo).unwrap();
        let iced_runtime::Action::Output(Message::SaveReady(ready)) = stream.next().await.unwrap()
        else {
            panic!("Ready save utente")
        };
        drop(app.update(Message::SaveReady(ready)));
        let iced_runtime::Action::Output(done @ Message::SaveCollected(_)) =
            stream.next().await.unwrap()
        else {
            panic!("Done save utente")
        };
        app.update(done)
    });
    assert_eq!(app.saves.active_document(), Some(note[1].0));
    assert!(app.notice_seq > first_notice);
    assert!(testo_avviso(&app).contains("ripartito"));
    app.completa_salvataggi(iced::Task::batch([terzo, manuale]));
    for (id, _, text) in note {
        let path = app.workspace.get(id).unwrap().path().unwrap();
        assert_eq!(store.load(path).unwrap().note.markdown, text);
    }
    assert_eq!(std::fs::read_to_string(code_path).unwrap(), "Xprima");
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    drop(chooser);
}

#[test]
fn salva_utente_durante_autosave_si_accoda_con_la_revisione_richiesta() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let id = app.open_loaded_note(loaded);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("prima".into()),
    ));
    let versione = app.workspace.get(id).unwrap().version();
    let automatico = app.update(Message::NotesAutosaveDue(id, versione));
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert(" dopo".into()),
    ));
    let richiesto = app.workspace.get(id).unwrap().rope().to_string();
    let manuale = app.update(Message::SaveActive);
    assert!(testo_avviso(&app).contains("accodato"));
    app.completa_salvataggi(iced::Task::batch([automatico, manuale]));
    let path = app.workspace.get(id).unwrap().path().unwrap().to_path_buf();
    assert_eq!(store.load(path).unwrap().note.markdown, richiesto);
    assert!(!app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn salvataggio_ordinario_occupa_il_coordinatore_fino_alla_ricevuta() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let task = app.update(Message::SaveDocFixture(id));
    assert_eq!(
        app.scritture.guarda(None).in_volo,
        Some(crate::scritture::Tipo::Salvataggio)
    );
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "Xprima");
    assert_eq!(app.scritture.guarda(None).in_volo, None);
}

#[test]
fn app_autorizza_il_save_ordinario_prima_del_commit_e_ack() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let mut stream =
        iced_runtime::task::into_stream(app.update(Message::SaveDocFixture(id))).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ready = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(ready)) = ready else {
        panic!("Ready del save ordinario assente");
    };
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "prima");
    let _ = app.update(Message::SaveReady(ready));
    let done = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(done @ Message::SaveCollected(_)) = done else {
        panic!("Done del save ordinario assente");
    };
    let _ = app.update(done);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");
    assert!(!app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn app_autorizza_il_save_note_prima_della_sostituzione() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let path = loaded.path().to_path_buf();
    let original = std::fs::read_to_string(&path).unwrap();
    let id = app.open_loaded_note(loaded);
    app.workspace
        .get_mut(id)
        .unwrap()
        .apply(lume_core::Transaction::new(
            lume_core::ChangeSet::insert_at(0, "Testo locale\n", 0),
        ));

    let mut stream =
        iced_runtime::task::into_stream(app.update(Message::SaveDocFixture(id))).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ready = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(ready)) = ready else {
        panic!("Ready del save NoteStore assente");
    };
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert!(app.save_ready_current(&ready));
    let _ = app.update(Message::SaveReady(ready));
    let done = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(done @ Message::SaveCollected(_)) = done else {
        panic!("Done del save NoteStore assente");
    };
    let _ = app.update(done);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("Testo locale")
    );
    assert!(!app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn autosave_estraneo_rimandato_dalla_sostituzione_riprende_con_edit_recenti() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let nota = loaded.path().to_path_buf();
    let id = app.open_loaded_note(loaded);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("X".into()),
    ));
    let versione = app.workspace.get(id).unwrap().version();
    let path = write_and_open(&mut app, dir.path(), "file.txt", "ago\n");
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let _ = app.update(Message::NotesAutosaveDue(id, versione));
    assert!((app.scritture.contiene_salvataggio(id) || app.saves.active_document() == Some(id)));
    assert!(testo_avviso(&app).contains("rimandato"));
    app.groups.activate(id);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("Y".into()),
    ));
    let testo = app.workspace.get(id).unwrap().rope().to_string();
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(&path).unwrap(), b"AGO\n");
    assert!(std::fs::read_to_string(nota).unwrap().contains(&testo));
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn nota_coinvolta_timer_scaduto_e_continuazione_fredda_revocata_non_resta_in_corso() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let path = loaded.path().to_path_buf();
    let id = app.open_loaded_note(loaded);
    // Il file della nota è reale: la sostituzione trova il titolo nel body.
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("ago".into()),
    ));
    let initial = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(initial);
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let ready = primo_output(task);
    let receipt = primo_output(app.update(ready));
    let save_task = app.update(receipt);
    let job = app.save_job.clone().unwrap();
    let timer = app.notes_autosave_schedule();
    let scadenza = primo_output(timer);
    assert!(matches!(scadenza, Message::NotesAutosaveDue(doc, _) if doc == id));
    let _ = app.update(scadenza);
    drop(save_task);
    let receipt = futures::executor::block_on(job.collect());
    let next = app.update(Message::SaveCollected(receipt));
    app.completa_salvataggi(next);
    assert!(!matches!(
        app.notes.as_ref().unwrap().salvataggi.get(&id),
        Some(crate::autosave::StatoSalvataggio::InCorso { .. })
    ));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn nota_coinvolta_con_timer_scaduto_e_continuazione_revocata_non_riparte() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let file = write_and_open(&mut app, dir.path(), "primo.txt", "ago\n");
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let id = app.open_loaded_note(loaded);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("ago".into()),
    ));
    let initial = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(initial);
    let path = app.workspace.get(id).unwrap().path().unwrap().to_path_buf();
    let original = std::fs::read(&path).unwrap();
    let replace = app.replace_all(&[trovato(&file, "ago"), trovato(&path, "ago")], "AGO");
    let ready = primo_output(replace);
    let receipt = primo_output(app.update(ready));
    let primo_save = app.update(receipt);
    assert_ne!(app.saves.active_document(), Some(id));
    let timer = app.notes_autosave_schedule();
    let scadenza = primo_output(timer);
    assert!(matches!(scadenza, Message::NotesAutosaveDue(doc, _) if doc == id));
    let _ = app.update(scadenza);
    assert!(matches!(
        app.notes.as_ref().unwrap().salvataggi.get(&id),
        Some(crate::autosave::StatoSalvataggio::InCorso { .. })
    ));
    app.replacement.as_ref().unwrap().cancel();
    app.completa_salvataggi(primo_save);
    assert_eq!(std::fs::read(path).unwrap(), original);
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(!matches!(
        app.notes.as_ref().unwrap().salvataggi.get(&id),
        Some(crate::autosave::StatoSalvataggio::InCorso { .. })
    ));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn save_interno_freddo_perso_raccoglie_abbandono_e_libera_la_sostituzione() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let path = write_and_open(&mut app, dir.path(), "file.txt", "ago\n");
    let id = app.active_doc().unwrap();
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let ready = primo_output(task);
    let task = app.update(ready);
    let receipt = primo_output(task);
    let save_task = app.update(receipt);
    let job = app.save_job.clone().unwrap();
    let mut collector = fallback(&app, &job);
    assert_eq!(
        app.scritture.guarda(None).in_volo,
        Some(crate::scritture::Tipo::Sostituisci)
    );
    drop(save_task);
    runtime_file_tests().block_on(async {
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), collector.next())
            .await
            .unwrap()
            .unwrap();
        let Message::SaveCollected(receipt) = &message else {
            panic!("ricevuta assente")
        };
        assert!(job.matches(receipt));
        drop(app.update(message));
        assert!(collector.next().await.is_none());
    });
    assert_eq!(std::fs::read(path).unwrap(), b"ago\n");
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "AGO\n");
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(app.notice.as_ref().unwrap().1.text.contains("file.txt"));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn autosave_rimandato_con_progetto_cambiato_non_resta_in_corso() {
    let dir = tempfile::tempdir().unwrap();
    let altro = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    app.set_project_root(dir.path().to_path_buf());
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let nota = loaded.path().to_path_buf();
    let original = std::fs::read(&nota).unwrap();
    let id = app.open_loaded_note(loaded);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("X".into()),
    ));
    let versione = app.workspace.get(id).unwrap().version();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "ago\n").unwrap();
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let _ = app.update(Message::NotesAutosaveDue(id, versione));
    app.set_project_root(altro.path().to_path_buf());
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(nota).unwrap(), original);
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(matches!(
        app.notes.as_ref().unwrap().salvataggi.get(&id),
        Some(crate::autosave::StatoSalvataggio::Errore(_))
    ));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn autosave_con_ready_revocato_mostra_errore_e_non_pubblica() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let path = loaded.path().to_path_buf();
    let originale = std::fs::read(&path).unwrap();
    let id = app.open_loaded_note(loaded);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("X".into()),
    ));
    let version = app.workspace.get(id).unwrap().version();
    let task = app.update(Message::NotesAutosaveDue(id, version));
    let runtime = runtime_file_tests();
    runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let iced_runtime::Action::Output(Message::SaveReady(ready)) = stream.next().await.unwrap()
        else {
            panic!("Ready assente")
        };
        app.notes.as_mut().unwrap().scan_generation += 1;
        drop(app.update(Message::SaveReady(ready)));
        let iced_runtime::Action::Output(done @ Message::SaveCollected(_)) =
            stream.next().await.unwrap()
        else {
            panic!("Done assente")
        };
        drop(app.update(done));
    });
    assert_eq!(std::fs::read(path).unwrap(), originale);
    assert!(matches!(
        app.notes.as_ref().unwrap().salvataggi.get(&id),
        Some(crate::autosave::StatoSalvataggio::Errore(_))
    ));
    assert!(matches!(
        app.notice.as_ref().map(|(_, n)| n.kind),
        Some(NoticeKind::Error)
    ));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn autosave_rimandato_con_conflitto_non_avvia_job_e_mostra_il_conflitto() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let path = loaded.path().to_path_buf();
    let original = std::fs::read(&path).unwrap();
    let id = app.open_loaded_note(loaded);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("X".into()),
    ));
    let version = app.workspace.get(id).unwrap().version();
    let file = write_and_open(&mut app, dir.path(), "file.txt", "ago\n");
    let replace = app.replace_all(&[trovato(&file, "ago")], "AGO");
    let _ = app.update(Message::NotesAutosaveDue(id, version));
    app.external_conflicts.insert(id);
    app.completa_salvataggi(replace);
    assert_eq!(std::fs::read(path).unwrap(), original);
    assert!(matches!(
        app.notes.as_ref().unwrap().salvataggi.get(&id),
        Some(crate::autosave::StatoSalvataggio::Conflitto)
    ));
    assert!(testo_avviso(&app).contains("cambiato"));
    assert!(app.save_job.is_none());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn autosave_rimandato_senza_store_segnala_errore_senza_restare_in_corso() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let nota = loaded.path().to_path_buf();
    let original = std::fs::read(&nota).unwrap();
    let id = app.open_loaded_note(loaded);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("X".into()),
    ));
    let versione = app.workspace.get(id).unwrap().version();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "ago\n").unwrap();
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let _ = app.update(Message::NotesAutosaveDue(id, versione));
    app.notes.as_mut().unwrap().store = None;
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(nota).unwrap(), original);
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(matches!(
        app.notes.as_ref().unwrap().salvataggi.get(&id),
        Some(crate::autosave::StatoSalvataggio::Errore(_))
    ));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn autosave_rimandato_tornato_pulito_non_resta_in_corso() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let nota = loaded.path().to_path_buf();
    let original = std::fs::read(&nota).unwrap();
    let id = app.open_loaded_note(loaded);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("X".into()),
    ));
    let versione = app.workspace.get(id).unwrap().version();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "ago\n").unwrap();
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let _ = app.update(Message::NotesAutosaveDue(id, versione));
    let _ = app.update(Message::EditorIn(app.groups.focus(), EditorCommand::Undo));
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(nota).unwrap(), original);
    assert!(matches!(
        app.notes.as_ref().unwrap().salvataggi.get(&id),
        Some(crate::autosave::StatoSalvataggio::Pulito)
    ));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn nota_ready_di_una_generazione_vecchia_viene_rifiutata() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let path = loaded.path().to_path_buf();
    let original = std::fs::read_to_string(&path).unwrap();
    let id = app.open_loaded_note(loaded);
    app.workspace
        .get_mut(id)
        .unwrap()
        .apply(lume_core::Transaction::new(
            lume_core::ChangeSet::insert_at(0, "Testo locale\n", 0),
        ));

    let mut stream =
        iced_runtime::task::into_stream(app.update(Message::SaveDocFixture(id))).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ready = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(ready)) = ready else {
        panic!("Ready del save NoteStore assente");
    };
    assert!(app.save_ready_current(&ready));
    app.notes.as_mut().unwrap().scan_generation += 1;
    assert!(!app.save_ready_current(&ready));
    let _ = app.update(Message::SaveReady(ready));
    let done = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(done @ Message::SaveCollected(_)) = done else {
        panic!("Done del save NoteStore revocato assente");
    };
    let _ = app.update(done);
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn non_salvare_revoca_save_note_pronto_prima_della_sostituzione() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let path = loaded.path().to_path_buf();
    let original = std::fs::read_to_string(&path).unwrap();
    let id = app.open_loaded_note(loaded);
    app.workspace
        .get_mut(id)
        .unwrap()
        .apply(lume_core::Transaction::new(
            lume_core::ChangeSet::insert_at(0, "Testo locale\n", 0),
        ));

    let mut stream =
        iced_runtime::task::into_stream(app.update(Message::SaveDocFixture(id))).unwrap();
    let job = app.save_job.clone().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ready = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(ready)) = ready else {
        panic!("Ready del save NoteStore assente");
    };
    let pause = job.pause().unwrap();
    let _ = app.update(Message::SaveReady(ready));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::Exit));
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    ));
    pause.finish(false);
    let done = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(done) = done else {
        panic!("completion del save NoteStore revocato assente");
    };
    let _ = app.update(done);

    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.chiusura.is_none());
}

#[test]
fn save_all_salva_la_nota_solo_dopo_ready() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let path = loaded.path().to_path_buf();
    let original = std::fs::read_to_string(&path).unwrap();
    let id = app.open_loaded_note(loaded);
    app.workspace
        .get_mut(id)
        .unwrap()
        .apply(lume_core::Transaction::new(
            lume_core::ChangeSet::insert_at(0, "Save All\n", 0),
        ));

    let mut stream = iced_runtime::task::into_stream(
        app.update(Message::MenuAction(crate::menu::MenuAct::SaveAll)),
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ready = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(Message::SaveReady(ready)) = ready else {
        panic!("Ready del Save All NoteStore assente");
    };
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    let _ = app.update(Message::SaveReady(ready));
    let done = runtime.block_on(stream.next()).unwrap();
    let iced_runtime::Action::Output(done @ Message::SaveCollected(_)) = done else {
        panic!("Done del Save All NoteStore assente");
    };
    let _ = app.update(done);

    let saved = store.load(path).unwrap();
    assert!(saved.note.markdown.contains("Save All"));
    assert!(!app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn chiudi_scheda_salva_la_nota_prima_di_rimuoverla() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = Lume::new_without_store();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(dir.path()).unwrap();
    app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let path = loaded.path().to_path_buf();
    let id = app.open_loaded_note(loaded.clone());
    app.workspace
        .get_mut(id)
        .unwrap()
        .apply(lume_core::Transaction::new(
            lume_core::ChangeSet::insert_at(0, "Chiudi e salva\n", 0),
        ));

    chiudi_scheda(&mut app, id);
    rispondi(&mut app, crate::conferma_chiusura::ConfermaMessage::Salva);

    assert!(!app.groups.contains(id));
    assert!(
        store
            .load(path)
            .unwrap()
            .note
            .markdown
            .contains("Chiudi e salva")
    );
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn evento_watcher_durante_save_rilegge_dopo_la_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let save = app.update(Message::SaveDocFixture(id));
    let mut worker = iced_runtime::task::into_stream(save).unwrap();
    let _ = app.on_fs_changed(vec![path.clone()]);
    assert!(app.pending_fs_rechecks.contains(&id));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ready = runtime.block_on(worker.next()).unwrap();
    let iced_runtime::Action::Output(ready @ Message::SaveReady(_)) = ready else {
        panic!("Ready autentico assente");
    };
    drop(app.update(ready));
    let receipt = runtime.block_on(worker.next()).unwrap();
    let iced_runtime::Action::Output(receipt @ Message::SaveCollected(_)) = receipt else {
        panic!("ricevuta autentica assente");
    };
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert(" edit recente".into()),
    ));
    std::fs::write(&path, "modifica esterna dopo il commit").unwrap();
    let task = app.update(receipt);
    let mut streams = futures::stream::SelectAll::new();
    streams.push(iced_runtime::task::into_stream(task).unwrap());
    runtime.block_on(async {
        while let Some(action) = streams.next().await {
            if let iced_runtime::Action::Output(message) = action
                && let Some(next) = iced_runtime::task::into_stream(app.update(message))
            {
                streams.push(next);
            }
        }
    });

    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(app.external_conflicts.contains(&id));
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "modifica esterna dopo il commit"
    );
}

#[test]
fn save_task_droppata_prepoll_viene_raccolta_senza_scrittura() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let task = app.update(Message::SaveDocFixture(id));
    let job = app.save_job.clone().unwrap();
    assert!((app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    drop(task);
    let result = futures::executor::block_on(job.collect());
    let _ = app.update(Message::SaveCollected(result));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.save_job.is_none());
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "prima");
    assert!(app.notice.as_ref().unwrap().1.text.contains("non avviato"));
}

#[test]
fn save_task_droppata_dopo_avvio_non_perde_worker_ack_o_modifiche_recenti() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let task = app.update(Message::SaveDocFixture(id));
    let job = app.save_job.clone().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (entered, admitted) = std::sync::mpsc::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            entered.send(()).unwrap();
            gate.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        });
        admitted
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        // Poll Pending del batch Iced non prova che il figlio sia già stato
        // pollato. Attendere l'ammissione reale nel Job, senza timer periodici.
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            futures::future::poll_fn(|cx| {
                assert!(stream.as_mut().poll_next(cx).is_pending());
                if job.running() {
                    std::task::Poll::Ready(())
                } else {
                    std::task::Poll::Pending
                }
            }),
        )
        .await
        .unwrap();
        drop(stream);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "prima");
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
        let _ = app.update(Message::EditorIn(
            crate::editor_groups::GroupId::Primary,
            EditorCommand::Insert("Y".into()),
        ));
        release.send(()).unwrap();
        blocker.await.unwrap();
        let mut fallback = fallback(&app, &job);
        let ready = tokio::time::timeout(std::time::Duration::from_secs(3), fallback.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(ready, Message::SaveReady(_)));
        let _ = app.update(ready);
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), fallback.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, Message::SaveCollected(_)));
        let duplicate = result.clone();
        let _ = app.update(result);
        assert!(fallback.next().await.is_none());
        assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
        assert!(app.save_job.is_none());
        assert!(
            app.notice.is_none(),
            "esito worker: {:?}",
            app.notice.as_ref().map(|(_, n)| n.text.as_str())
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");
        let doc = app.workspace.get(id).unwrap();
        assert_eq!(doc.rope().to_string(), "XYprima");
        assert!(doc.is_dirty());
        let epoch = app.scritture.guarda(None).epoca;
        let _ = app.update(duplicate);
        assert_eq!(app.scritture.guarda(None).epoca, epoch);
        assert!(app.workspace.get(id).unwrap().is_dirty());
        assert!(app.workspace.get_mut(id).unwrap().undo().is_some());
        assert!(!app.workspace.get(id).unwrap().is_dirty());
    });
}

#[test]
fn ricevuta_estranea_a_stesso_ticket_non_sblocca_job_corrente() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let task = app.update(Message::SaveDocFixture(id));
    let current = app.save_job.clone().unwrap();
    let other_path = dir.path().join("altro.txt");
    std::fs::write(&other_path, "prima").unwrap();
    let other = crate::save::Job::new(crate::save::Request {
        ticket: crate::save::Ticket {
            document: id,
            sequence: 1,
        },
        path: other_path.clone(),
        snapshot: app.workspace.get_mut(id).unwrap().snapshot_for_save(),
        note: None,
        aliases: None,
        expected_fingerprint: None,
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut foreign_delivery = other.delivery();
    let foreign = runtime.block_on(async {
        let Some(crate::save::Event::Ready(ready)) = foreign_delivery.next().await else {
            panic!("Ready del save estraneo assente");
        };
        assert!(ready.grant());
        let Some(crate::save::Event::Done(receipt)) = foreign_delivery.next().await else {
            panic!("Done del save estraneo assente");
        };
        receipt
    });
    let duplicate = foreign.clone();
    let _ = app.update(Message::SaveCollected(foreign));
    assert_eq!(app.save_job.as_ref(), Some(&current));
    assert!((app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    let foreign_result = duplicate.take().unwrap().result;
    assert!(
        foreign_result.is_ok(),
        "l'app non consuma ricevute estranee"
    );
    assert_eq!(std::fs::read_to_string(other_path).unwrap(), "Xprima");
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "prima");
    drop(task);
    let receipt = futures::executor::block_on(current.collect());
    let _ = app.update(Message::SaveCollected(receipt));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn fixture_completion_diretta_non_scavalca_owner_started_o_pending() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let task = app.update(Message::SaveDocFixture(id));
    let job = app.save_job.clone().unwrap();
    let fake = crate::save::Completion {
        ticket: crate::save::Ticket {
            document: id,
            sequence: 1,
        },
        path: path.clone(),
        snapshot: app.workspace.get_mut(id).unwrap().snapshot_for_save(),
        result: Ok(None),
        fingerprint: None,
        resolved: None,
        warning: None,
    };
    let _ = app.update(Message::SaveFinished(Box::new(fake)));
    assert!((app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "prima");
    drop(task);
    let receipt = futures::executor::block_on(job.collect());
    let _ = app.update(Message::SaveCollected(receipt));
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn avviso_sincronizzazione_cartella_arriva_in_status_bar_senza_fallire_il_save() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let version = app.workspace.get(id).unwrap().version();
    let request = app.prepare_save(id, version).unwrap().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut completion = runtime.block_on(request.run());
    // Salvataggio reale su TempDir: la cartella si sincronizza senza avvisi.
    assert!(completion.result.is_ok());
    assert_eq!(completion.warning, None);
    // Un fsync della cartella non si fa fallire su richiesta: si inietta solo
    // il testo, per provare che l'app lo mostra senza trattarlo da errore.
    completion.warning = Some("cartella non sincronizzata: EIO".into());

    let _ = app.update(Message::SaveFinished(Box::new(completion)));

    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|(_, notice)| notice.text.contains("cartella non sincronizzata"))
    );
    assert!(app.disk_fingerprints.contains_key(&id));
    assert!(std::fs::metadata(path).is_ok());
}

#[test]
fn avviso_sincronizzazione_cartella_non_copre_un_errore_successivo() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, _path) = prepara(&mut app, dir.path());
    let version = app.workspace.get(id).unwrap().version();
    let request = app.prepare_save(id, version).unwrap().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut completion = runtime.block_on(request.run());
    assert!(completion.result.is_ok());
    completion.warning = Some("cartella non sincronizzata: EIO".into());
    // Il documento cambia percorso durante l'I/O: la scrittura non vale più
    // per lui e l'app lo segnala come errore.
    app.workspace
        .rebind_path(id, dir.path().join("altrove.txt"))
        .unwrap();

    let _ = app.update(Message::SaveFinished(Box::new(completion)));

    let text = &app.notice.as_ref().unwrap().1.text;
    assert!(
        text.contains("documento o il percorso è cambiato"),
        "{text}"
    );
}

#[test]
fn chiusura_finestra_durante_un_salvataggio_esce_quando_il_salvataggio_finisce() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let task = app.update(Message::SaveDocFixture(id));
    assert!((app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    // La X della finestra arriva mentre il salvataggio è in volo: il file
    // risulta ancora sporco finché la scrittura non si conclude.
    let _ = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
    let riquadro = app.riquadro_chiusura().unwrap();
    assert!(riquadro.salvataggio_in_corso);

    app.completa_salvataggi(task);

    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "Xprima");
    // Il riquadro non resta aperto con «0 file»: l'app esce da sola.
    assert!(
        app.riquadro_chiusura().is_none(),
        "il riquadro resta aperto"
    );
    assert!(app.chiusura.is_none());
}

#[test]
fn chiusura_scheda_durante_un_salvataggio_la_chiude_quando_il_salvataggio_finisce() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let task = app.update(Message::SaveDocFixture(id));
    assert!((app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    // La × della scheda arriva mentre il salvataggio è in volo: il documento
    // risulta ancora sporco e il riquadro chiede.
    let _ = app.update(Message::EditorTabBar(
        crate::editor_tab_bar::EditorTabBarMessage::Close(id),
    ));
    let riquadro = app.riquadro_chiusura().unwrap();
    assert!(riquadro.salvataggio_in_corso);

    app.completa_salvataggi(task);

    assert_eq!(std::fs::read_to_string(path).unwrap(), "Xprima");
    // Finito il salvataggio non resta nulla da perdere: la scheda si chiude.
    assert!(
        app.riquadro_chiusura().is_none(),
        "il riquadro resta aperto"
    );
    assert!(app.chiusura.is_none());
    assert!(!app.groups.contains(id), "la scheda resta aperta");
}

#[test]
fn chiusura_durante_un_salvataggio_resta_aperta_se_compare_un_errore() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, _path) = prepara(&mut app, dir.path());
    let version = app.workspace.get(id).unwrap().version();
    let request = app.prepare_save(id, version).unwrap().unwrap();
    let _ = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
    assert!(app.riquadro_chiusura().is_some());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut completion = runtime.block_on(request.run());
    // Testo iniettato in una completion reale: un fsync di cartella non si
    // fa fallire su richiesta.
    completion.warning = Some("cartella non sincronizzata: EIO".into());

    let _ = app.update(Message::SaveFinished(Box::new(completion)));

    let riquadro = app.riquadro_chiusura().expect("il riquadro resta aperto");
    assert!(
        riquadro
            .errore
            .as_deref()
            .is_some_and(|errore| errore.contains("cartella non sincronizzata"))
    );
}

#[test]
fn chiusura_durante_un_salvataggio_resta_aperta_se_un_altro_file_e_sporco() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    // b.txt resta sporco e non viene salvato; a.txt è il salvataggio in volo.
    write_and_open(&mut app, dir.path(), "b.txt", "altro");
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("Y".into()),
    ));
    let (id, _path) = prepara(&mut app, dir.path());
    let task = app.update(Message::SaveDocFixture(id));
    let _ = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));

    app.completa_salvataggi(task);

    let riquadro = app.riquadro_chiusura().expect("il riquadro resta aperto");
    assert_eq!(riquadro.nomi, vec!["b.txt".to_owned()]);
}

#[test]
fn save_utente_assorbe_autosave_rimandato_della_stessa_nota_e_riarma() {
    for edit_dopo_snapshot in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = Lume::new_without_store();
        let (code, _) = prepara(&mut app, dir.path());
        let generation = app.begin_notes_load().unwrap();
        let store = lume_notes::NoteStore::open(dir.path()).unwrap();
        app.finish_notes_load(generation, dir.path().to_path_buf(), Vec::new())
            .unwrap();
        let mut note = Vec::new();
        let mut timers = Vec::new();
        for title in ["Prima", "Seconda"] {
            let loaded = store.create(title).unwrap();
            let path = loaded.path().to_path_buf();
            let id = app.open_loaded_note(loaded);
            drop(app.update(Message::EditorIn(
                app.groups.focus(),
                EditorCommand::Insert(title.into()),
            )));
            timers.push(app.notes_autosave_schedule());
            note.push((id, path, app.workspace.get(id).unwrap().version()));
        }
        let chooser = app.pick_save_path(code);
        let sequence = app.save_as_picker.as_ref().unwrap().sequence;
        for ((id, _, version), timer) in note.iter().zip(timers) {
            let due = primo_output(timer);
            assert!(
                matches!(due, Message::NotesAutosaveDue(doc, v) if doc == *id && v == *version)
            );
            drop(app.update(due));
        }
        let primo = app.update(Message::SaveAsPicked(sequence, None));
        assert_eq!(app.saves.active_document(), Some(note[0].0));
        let (target, path, _) = &note[1];
        drop(app.activate_tab(*target));
        drop(app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert(" nuovo".into()),
        )));
        let manuale = app.update(Message::SaveActive);
        assert!(testo_avviso(&app).contains("accodato"));
        let runtime = runtime_file_tests();
        let secondo = runtime.block_on(async {
            let mut stream = iced_runtime::task::into_stream(primo).unwrap();
            let iced_runtime::Action::Output(Message::SaveReady(ready)) =
                stream.next().await.unwrap()
            else {
                panic!("Ready primo")
            };
            drop(app.update(Message::SaveReady(ready)));
            let iced_runtime::Action::Output(done @ Message::SaveCollected(_)) =
                stream.next().await.unwrap()
            else {
                panic!("Done primo")
            };
            app.update(done)
        });
        assert_eq!(app.saves.active_document(), Some(*target));
        let seguito = runtime.block_on(async {
            let mut stream = iced_runtime::task::into_stream(secondo).unwrap();
            let iced_runtime::Action::Output(Message::SaveReady(ready)) =
                stream.next().await.unwrap()
            else {
                panic!("Ready secondo")
            };
            drop(app.update(Message::SaveReady(ready)));
            if edit_dopo_snapshot {
                drop(app.update(Message::EditorIn(
                    app.groups.focus(),
                    EditorCommand::Insert(" tardivo".into()),
                )));
            }
            let iced_runtime::Action::Output(done @ Message::SaveCollected(_)) =
                stream.next().await.unwrap()
            else {
                panic!("Done secondo")
            };
            app.update(done)
        });
        app.completa_salvataggi(iced::Task::batch([seguito, manuale]));
        let salvati: Vec<_> = app
            .history
            .iter()
            .filter(|r| r.action == lume_store::HistoryAction::Saved)
            .collect();
        assert_eq!(
            salvati.len(),
            1,
            "Ctrl+S assorbito deve registrare un solo salvataggio; il primo automatico nessuno"
        );
        assert_eq!(salvati[0].path.as_deref(), Some(path.as_path()));
        let archivio = futures::executor::block_on(lume_store::StoreHandle::open_at(
            &dir.path().join("cronologia.sqlite"),
        ))
        .unwrap();
        futures::executor::block_on(archivio.save_session(app.session_snapshot())).unwrap();
        let sessione = futures::executor::block_on(archivio.load_session())
            .unwrap()
            .0;
        assert_eq!(
            sessione
                .history
                .iter()
                .filter(|r| r.action == lume_store::HistoryAction::Saved
                    && r.path.as_deref() == Some(path.as_path()))
                .count(),
            1
        );
        assert!(
            !matches!(
                app.notes.as_ref().unwrap().salvataggi.get(target),
                Some(crate::autosave::StatoSalvataggio::InCorso { .. })
            ),
            "autosave assorbito resta InCorso"
        );
        let saved = store.load(path).unwrap().note.markdown;
        assert!(saved.contains("nuovo"));
        assert!(!saved.contains("tardivo"));
        if !edit_dopo_snapshot {
            drop(app.update(Message::EditorIn(
                app.groups.focus(),
                EditorCommand::Insert(" finale".into()),
            )));
        }
        let timer = app.notes_autosave_schedule();
        let due = primo_output(timer);
        assert!(matches!(due, Message::NotesAutosaveDue(id, _) if id == *target));
        let task = app.update(due);
        app.completa_salvataggi(task);
        assert_eq!(
            app.history
                .iter()
                .filter(|r| r.action == lume_store::HistoryAction::Saved)
                .count(),
            1,
            "autosave puro non aggiunge cronologia"
        );
        assert_eq!(
            store.load(path).unwrap().note.markdown,
            app.workspace.get(*target).unwrap().rope().to_string()
        );
        assert!(!app.workspace.get(*target).unwrap().is_dirty());
        drop(chooser);
    }
}

#[test]
fn salvataggio_avviato_prima_del_modale_consegna_ready_e_ricevuta() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let (id, path) = prepara(&mut app, dir.path());
    let task = app.update(Message::SaveDocFixture(id));
    let job = app.save_job.clone().unwrap();
    app.apri_overlay(OverlayId::About);
    runtime_file_tests().block_on(async {
        let mut producer = iced_runtime::task::into_stream(task).unwrap();
        let iced_runtime::Action::Output(Message::SaveReady(ready)) =
            tokio::time::timeout(std::time::Duration::from_secs(5), producer.next())
                .await
                .unwrap()
                .unwrap()
        else {
            panic!("Ready assente")
        };
        assert!(job.matches_ready(&ready));
        assert!(app.scritture.guarda(None).in_volo.is_some());
        let _ = app.update(Message::SaveReady(ready));
        let iced_runtime::Action::Output(Message::SaveCollected(receipt)) =
            tokio::time::timeout(std::time::Duration::from_secs(5), producer.next())
                .await
                .unwrap()
                .unwrap()
        else {
            panic!("ricevuta assente")
        };
        assert!(job.matches(&receipt));
        let _ = app.update(Message::SaveCollected(receipt));
        assert!(app.scritture.guarda(None).in_volo.is_none());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "Xprima");
        assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
    });
}

#[test]
fn tre_chiusure_sotto_modale_raccolgono_il_writer_avviato() {
    for sorgente in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        let (id, path) = prepara(&mut app, dir.path());
        let task = app.update(Message::SaveDocFixture(id));
        let job = app.save_job.clone().expect("job autentico");
        let lease = app.save_lease.expect("lease autentico");
        app.apri_overlay(OverlayId::About);
        let chiusura = match sorgente {
            0 => Message::WindowCloseRequested(iced::window::Id::unique()),
            1 => Message::MenuAction(MenuAct::Exit),
            _ => Message::Titlebar(TitlebarMessage::Close),
        };
        let _ = app.update(chiusura);
        assert_eq!(
            app.shell.active_overlay(),
            Some(OverlayId::ConfermaChiusura)
        );
        assert!(
            app.scritture
                .possiede(lease, crate::scritture::Tipo::Salvataggio)
        );
        runtime_file_tests().block_on(async {
            let mut stream = iced_runtime::task::into_stream(task).unwrap();
            let iced_runtime::Action::Output(Message::SaveReady(ready)) =
                tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                    .await
                    .unwrap()
                    .unwrap()
            else {
                panic!("Ready assente")
            };
            assert!(job.matches_ready(&ready));
            let _ = app.update(Message::SaveReady(ready));
            let iced_runtime::Action::Output(Message::SaveCollected(ricevuta)) =
                tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                    .await
                    .unwrap()
                    .unwrap()
            else {
                panic!("ricevuta assente")
            };
            assert!(job.matches(&ricevuta));
            let _ = app.update(Message::SaveCollected(ricevuta));
        });
        assert!(
            !app.scritture
                .possiede(lease, crate::scritture::Tipo::Salvataggio)
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "Xprima");
    }
}

#[test]
fn uscita_ripetuta_conserva_domanda_e_seguito_durante_commit_reale() {
    for sorgente in 0..4 {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        let altro = write_and_open(&mut app, dir.path(), "altro.txt", "altro");
        let altro_id = app.active_doc().unwrap();
        let _ = app.update(Message::EditorIn(
            crate::editor_groups::GroupId::Primary,
            EditorCommand::Insert("Y".into()),
        ));
        let (id, path) = prepara(&mut app, dir.path());
        let task = app.update(Message::SaveDocFixture(id));
        let job = app.save_job.clone().unwrap();
        let lease = app.save_lease.unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        job.set_commit_started_hook(move || {
            started_tx.send(()).unwrap();
            release_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
        });
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let runtime = runtime_file_tests();
        let ready = runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                .await
                .unwrap()
                .unwrap()
        });
        let iced_runtime::Action::Output(Message::SaveReady(ready)) = ready else {
            panic!("Ready autentico assente")
        };
        assert!(job.matches_ready(&ready));
        let _ = app.update(Message::SaveReady(ready));
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let window = iced::window::Id::unique();
        let _ = app.update(Message::WindowCloseRequested(window));
        let _ = app.update(Message::ConfermaChiusura(
            crate::conferma_chiusura::ConfermaMessage::NonSalvare,
        ));
        app.chiusura.as_mut().unwrap().errore = Some("errore distintivo".into());
        let epoch = app.close_epoch;
        let seguito = app.close_after_save;
        let oggetto = app.chiusura_attiva().unwrap().oggetto;
        let uscita = match sorgente {
            0 => Message::MenuBar(crate::menu::MenuBarMessage::Activate(MenuAct::Exit)),
            1 => Message::MenuAction(MenuAct::Exit),
            2 => Message::WindowCloseRequested(window),
            _ => Message::Titlebar(TitlebarMessage::Close),
        };
        let _ = app.update(uscita);
        assert_eq!(app.close_epoch, epoch, "sorgente {sorgente}");
        assert_eq!(app.close_after_save, seguito, "sorgente {sorgente}");
        assert_eq!(app.chiusura_attiva().unwrap().oggetto, oggetto);
        assert_eq!(
            app.chiusura_attiva().unwrap().errore.as_deref(),
            Some("errore distintivo")
        );
        assert!(
            app.scritture
                .possiede(lease, crate::scritture::Tipo::Salvataggio)
        );
        release_tx.send(()).unwrap();
        let done = runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                .await
                .unwrap()
                .unwrap()
        });
        let iced_runtime::Action::Output(Message::SaveCollected(receipt)) = done else {
            panic!("ricevuta autentica assente")
        };
        assert!(job.matches(&receipt));
        let close = app.update(Message::SaveCollected(receipt));
        assert!(app.chiusura.is_none());
        assert!(app.close_after_save.is_none());
        assert!(
            !app.scritture
                .possiede(lease, crate::scritture::Tipo::Salvataggio)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");
        assert_eq!(std::fs::read_to_string(&altro).unwrap(), "altro");
        assert!(app.workspace.get(altro_id).unwrap().is_dirty());
        let mut actions = iced_runtime::task::into_stream(close).unwrap();
        let action = runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(5), actions.next())
                .await
                .unwrap()
                .unwrap()
        });
        assert!(
            matches!(action, iced_runtime::Action::Window(iced_runtime::window::Action::Close(id)) if id == window)
        );
    }
}
