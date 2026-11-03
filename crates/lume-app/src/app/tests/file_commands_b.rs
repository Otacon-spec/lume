//! Comandi reali, Task/recipe dell'app e file posseduti; non renderer/uscita OS.
use super::*;
use futures::StreamExt;
use std::time::Duration;

fn recipe(
    app: &Lume,
    job: &crate::replace_batch::Job,
) -> futures::stream::BoxStream<'static, Message> {
    use iced_runtime::futures::subscription::{Hasher, Recipe, into_recipes};
    use std::hash::Hasher as _;
    let hash = |recipe: &dyn Recipe<Output = Message>| {
        let mut state = Hasher::default();
        recipe.hash(&mut state);
        state.finish()
    };
    let expected = iced::Subscription::run_with(job.clone(), crate::replace_batch::completed)
        .map(Lume::replace_event);
    let key = hash(&*into_recipes(expected).pop().unwrap());
    let mut matches: Vec<_> = into_recipes(app.subscription())
        .into_iter()
        .filter(|item| hash(&**item) == key)
        .collect();
    assert_eq!(matches.len(), 1);
    matches
        .pop()
        .unwrap()
        .stream(futures::stream::empty().boxed())
}

async fn next(stream: &mut futures::stream::BoxStream<'static, Message>) -> Message {
    tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
}

#[test]
fn non_salvare_b_non_revoca_il_save_preparato_di_a() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let a = write_and_open(&mut app, dir.path(), "a.txt", "prima");
    let id_a = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let b = write_and_open(&mut app, dir.path(), "b.txt", "seconda");
    let id_b = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let task = app.update(Message::SaveDocFixture(id_a));
    runtime_file_tests().block_on(async {
        let mut producer = iced_runtime::task::into_stream(task).unwrap();
        let iced_runtime::Action::Output(Message::SaveReady(ready)) =
            producer.next().await.unwrap()
        else {
            panic!("Ready reale assente");
        };
        chiudi_scheda(&mut app, id_b);
        drop(app.update(Message::ConfermaChiusura(
            crate::conferma_chiusura::ConfermaMessage::NonSalvare,
        )));
        assert!(app.groups.contains(id_b), "attendere il writer autentico");
        assert!(app.save_job_model_current(), "NonSalvare B non revoca A");
        drop(app.update(Message::SaveReady(ready)));
        let iced_runtime::Action::Output(Message::SaveCollected(receipt)) =
            producer.next().await.unwrap()
        else {
            panic!("ricevuta reale assente");
        };
        drop(app.update(Message::SaveCollected(receipt)));
        assert!(producer.next().await.is_none());
        assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
        assert!(!app.groups.contains(id_b));
        assert!(!app.workspace.get(id_a).unwrap().is_dirty());
        assert_eq!(std::fs::read(a).unwrap(), b"Xprima");
        assert_eq!(std::fs::read(b).unwrap(), b"seconda");
    });
}

#[test]
fn replace_freddo_perso_recipe_non_ammette_e_raccoglie_abbandono() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "ago\n").unwrap();
    let mut app = app();
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    assert_eq!(
        app.scritture.guarda(None).in_volo,
        Some(crate::scritture::Tipo::Sostituisci)
    );
    let job = app.replace_job.clone().unwrap();
    let mut fallback = recipe(&app, &job);
    runtime_file_tests().block_on(async {
        assert!(
            tokio::time::timeout(Duration::from_millis(20), fallback.next())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        drop(task); // mai interrogato: niente ammissione dal collector.
        let message = next(&mut fallback).await;
        let Message::ReplaceCollected(receipt) = &message else {
            panic!("Done assente");
        };
        assert!(job.matches(receipt));
        drop(app.update(message));
        assert!(fallback.next().await.is_none());
        assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
        assert_eq!(app.scritture.guarda(None).in_volo, None);
        assert_eq!(std::fs::read(&path).unwrap(), b"ago\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(app.notice.as_ref().unwrap().1.text.contains("abbandon"));
    });
}

#[test]
fn replace_recipe_recupera_ready_e_ricevuta_persa_senza_rieseguire() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "ago\n").unwrap();
    let mut app = app();
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let job = app.replace_job.clone().unwrap();
    let mut fallback = recipe(&app, &job);
    runtime_file_tests().block_on(async {
        let mut producer = iced_runtime::task::into_stream(task).unwrap();
        let iced_runtime::Action::Output(Message::ReplaceReady(first)) =
            producer.next().await.unwrap()
        else {
            panic!("Ready reale assente");
        };
        assert_eq!(std::fs::read(&path).unwrap(), b"ago\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        drop(producer);
        let Message::ReplaceReady(recovered) = next(&mut fallback).await else {
            panic!("Ready recuperato assente");
        };
        assert!(job.matches_ready(&recovered));
        drop(first);
        let task = app.update(Message::ReplaceReady(recovered));
        let mut done = iced_runtime::task::into_stream(task).unwrap();
        let iced_runtime::Action::Output(Message::ReplaceCollected(receipt)) =
            done.next().await.unwrap()
        else {
            panic!("Done autentico assente");
        };
        assert!(done.next().await.is_none());
        assert!(job.matches(&receipt));
        assert_eq!(std::fs::read(&path).unwrap(), b"AGO\n");
        drop(receipt); // output autentico prodotto ma mai consegnato alla UI.
        std::fs::write(&path, b"attore esterno\n").unwrap();
        let Message::ReplaceCollected(receipt) = next(&mut fallback).await else {
            panic!("Done recuperato assente");
        };
        assert!(job.matches(&receipt));
        let replay = receipt.clone();
        drop(app.update(Message::ReplaceCollected(receipt)));
        assert!(fallback.next().await.is_none());
        assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
        let successivo = dir.path().join("successivo.txt");
        std::fs::write(&successivo, "ago\n").unwrap();
        let task_successivo = app.replace_all(&[trovato(&successivo, "ago")], "AGO");
        let lease_successivo = app.replacement.as_ref().unwrap().lease;
        drop(app.update(Message::ReplaceCollected(replay)));
        assert!(
            app.scritture
                .possiede(lease_successivo, crate::scritture::Tipo::Sostituisci)
        );
        assert_eq!(app.replacement.as_ref().unwrap().lease, lease_successivo);
        // La recipe e il Task autentico del successore restano vivi dopo il replay.
        let mut stream = iced_runtime::task::into_stream(task_successivo).unwrap();
        let iced_runtime::Action::Output(ready) = stream.next().await.unwrap() else {
            panic!("Ready assente")
        };
        let task = app.update(ready);
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let iced_runtime::Action::Output(done) = stream.next().await.unwrap() else {
            panic!("Done assente")
        };
        drop(app.update(done));
        assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
        assert_eq!(std::fs::read(&successivo).unwrap(), b"AGO\n");
        assert_eq!(std::fs::read(&path).unwrap(), b"attore esterno\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    });
}

#[test]
fn replace_root_aba_revoca_il_preparato_reale() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "ago\n").unwrap();
    let mut app = app();
    app.set_project_root(dir.path().to_path_buf());
    let task = app.replace_all(&[trovato(&path, "ago")], "AGO");
    let prepared = primo_output(task);
    let epoch = app.git_root_epoch;
    app.set_project_root(other.path().to_path_buf());
    app.set_project_root(dir.path().to_path_buf());
    assert_ne!(app.git_root_epoch, epoch);
    assert!(!app.replacement_valid());
    let task = app.update(prepared);
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(&path).unwrap(), b"ago\n");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn save_preparato_root_aba_e_coda_stantia_non_pubblicano() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let mut app = app();
    app.set_project_root(dir.path().to_path_buf());
    let a = write_and_open(&mut app, dir.path(), "a.txt", "prima");
    let id_a = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let b = write_and_open(&mut app, dir.path(), "b.txt", "seconda");
    let id_b = app.active_doc().unwrap();
    sporca_attivo(&mut app);
    let task = app.update(Message::SaveAllFixture);
    runtime_file_tests().block_on(async {
        let mut producer = iced_runtime::task::into_stream(task).unwrap();
        let iced_runtime::Action::Output(Message::SaveReady(ready)) =
            producer.next().await.unwrap()
        else {
            panic!("Ready reale assente");
        };
        app.set_project_root(other.path().to_path_buf());
        app.set_project_root(dir.path().to_path_buf());
        drop(app.update(Message::SaveReady(ready)));
        let iced_runtime::Action::Output(Message::SaveCollected(receipt)) =
            producer.next().await.unwrap()
        else {
            panic!("Done reale assente");
        };
        drop(app.update(Message::SaveCollected(receipt)));
        assert!(producer.next().await.is_none());
        assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
        assert!(app.workspace.get(id_a).unwrap().is_dirty());
        assert!(app.workspace.get(id_b).unwrap().is_dirty());
        assert_eq!(std::fs::read(a).unwrap(), b"prima");
        assert_eq!(std::fs::read(b).unwrap(), b"seconda");
        assert!(
            app.notice
                .as_ref()
                .unwrap()
                .1
                .text
                .contains("accodato scaduto")
        );
    });
}
