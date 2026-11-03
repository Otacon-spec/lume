//! Cerca reale: engine, file, Task e modello; nessun risultato simulato.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use futures::{Stream, StreamExt};
use std::pin::Pin;
type Actions = Pin<Box<dyn Stream<Item = iced_runtime::Action<Message>> + Send>>;
fn fixture() -> (tempfile::TempDir, Lume, Task<Message>) {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..100 {
        std::fs::write(
            dir.path().join(format!("query-{index:03}.txt")),
            "ago reale\n",
        )
        .unwrap();
    }
    let (mut app, _) = Lume::new_without_store();
    app.set_project_root(dir.path().to_path_buf());
    let task = app.run_search(
        lume_core::SearchOptions::literal("ago"),
        lume_core::SearchArea::Progetto,
    );
    (dir, app, task)
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
fn actions(task: Task<Message>) -> Actions {
    Box::pin(iced_runtime::task::into_stream(task).unwrap())
}
async fn first(stream: &mut Actions) -> Message {
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .unwrap()
        {
            Some(iced_runtime::Action::Output(message @ Message::SearchUpdate(..))) => {
                return message;
            }
            Some(iced_runtime::Action::Output(Message::SearchCollected(..))) => {}
            other => panic!("primo evento Cerca richiesto: {other:?}"),
        }
    }
}
async fn cleanup(app: &mut Lume) {
    for job in app.search_jobs.snapshot().0 {
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), job.collect())
            .await
            .unwrap();
        assert!(result.is_ok());
        let _ = app.update(Message::SearchCollected(job.id(), result));
    }
    assert!(!app.search_jobs.busy());
}

#[test]
fn terminale_chiude_il_modello_e_rilascia_il_worker_con_search_ancora_nella_shell() {
    use std::sync::atomic::Ordering;

    runtime().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("selettivo.txt");
        std::fs::write(&file, "file reale\n").unwrap();
        let (mut app, _) = Lume::new_without_store();
        app.set_project_root(dir.path().to_path_buf());
        let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Open(
            SearchTab::File,
        )));
        let task = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
            "selettivo".into(),
        )));
        assert_eq!(app.shell.active_overlay(), Some(OverlayId::Search));
        assert!(app.global_search.is_open());
        assert!(app.global_search.is_file_searching());
        assert!(app.global_file_search_worker.is_some());
        let generation = app.global_search_gen.load(Ordering::SeqCst);
        let mut stream = actions(task);
        let action = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap();
        let iced_runtime::Action::Output(message @ Message::GlobalFileSearch(_, Ok(_))) = action
        else {
            panic!("risultato filesystem atteso: {action:?}");
        };
        assert!(matches!(&message, Message::GlobalFileSearch(_, Ok(entries)) if entries.iter().any(|entry| entry.path.as_ref() == Some(&file))));

        let _ = app.update(Message::MenuAction(MenuAct::ToggleTerminal));
        assert!(!app.registry.is_open(tools::terminal::ID));
        assert_eq!(app.shell.active_overlay(), Some(OverlayId::Search));
        assert!(!app.global_search.is_open());
        assert!(!app.global_search.is_file_searching());
        assert!(app.global_search_gen.load(Ordering::SeqCst) > generation);
        assert!(app.global_file_search_worker.is_none(), "worker raccolto dopo ToggleTerminal");
        assert!(app.global_file_search_worker_error.is_none());
        let _ = app.update(message);
        assert!(app.global_search.entries().is_empty(), "risposta tardiva scartata");
    });
}

#[test]
fn terminale_rilascia_anche_errore_avvio_ricerca_con_search_nella_shell() {
    let (mut app, _) = Lume::new_without_store();
    let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Open(
        SearchTab::File,
    )));
    // Stato memorizzato da un avvio fallito, senza simulare il backend.
    app.global_file_search_worker_error = Some("limite thread".into());
    assert!(app.global_file_search_worker.is_none());
    let _ = app.update(Message::MenuAction(MenuAct::ToggleTerminal));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::Search));
    assert!(!app.global_search.is_open());
    assert!(app.global_file_search_worker_error.is_none());
}

/// I dati già ottenuti restano nel modello quando una Card ne nasconde la vista.
#[cfg(feature = "mcp")]
#[test]
fn card_mcp_conserva_risultati_selezione_e_simboli_della_ricerca_sospesa() {
    use crate::mcp_card::CardMessage;
    use lume_mcp::{AppQuery, McpCommand, McpEvent};
    use std::sync::atomic::Ordering;

    runtime().block_on(async {
        for esc in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let primo = dir.path().join("selettivo_a.rs");
            let secondo = dir.path().join("selettivo_b.rs");
            std::fs::write(&primo, "fn selettivo_funzione() {}\n").unwrap();
            std::fs::write(&secondo, "fn altro() {}\n").unwrap();
            let (mut app, _) = Lume::new_without_store();
            app.set_project_root(dir.path().to_path_buf());
            let _ = app.registry.set_feature_enabled("mcp", true);
            let _ = app.mcp_accendi(lume_mcp::confine::canonical(dir.path()).unwrap());
            let epoch = app.mcp.as_ref().unwrap().epoch;
            let (tx, mut posta) = tokio::sync::mpsc::unbounded_channel();
            let _ = app.update(Message::Mcp(epoch, McpEvent::Ready(tx)));
            let _ = app.update(Message::Mcp(
                epoch,
                McpEvent::ClientConnected {
                    conn: 1,
                    name: "cliente".into(),
                },
            ));
            while posta.try_recv().is_ok() {}

            let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Open(
                SearchTab::Tutto,
            )));
            let task = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
                "selettivo".into(),
            )));
            let mut stream = actions(task);
            let action = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                .await
                .unwrap()
                .unwrap();
            let iced_runtime::Action::Output(message @ Message::GlobalFileSearch(_, Ok(_))) =
                action
            else {
                panic!("risultato filesystem atteso: {action:?}");
            };
            let tardivo = message.clone();
            let _ = app.update(message);
            assert!(app.global_file_search_worker.is_some());
            // Fotografia di dominio, senza avviare o simulare un server LSP.
            app.global_search
                .set_symbol_results(&tools::structure::Snapshot {
                    path: Some(primo.clone()),
                    rows: vec![tools::structure::Row {
                        name: "selettivo_funzione".into(),
                        detail: None,
                        kind: "fn",
                        depth: 0,
                        line: 1,
                    }],
                    empty_reason: "",
                });
            let dati = |app: &Lume| {
                app.global_search
                    .entries()
                    .into_iter()
                    .map(|entry| (entry.name, entry.tab, entry.path, entry.line))
                    .collect::<Vec<_>>()
            };
            let prima = dati(&app);
            assert!(
                prima
                    .iter()
                    .any(|(_, tab, path, _)| *tab == SearchTab::File
                        && path.as_ref() == Some(&primo))
            );
            assert!(prima.iter().any(
                |(_, tab, path, _)| *tab == SearchTab::File && path.as_ref() == Some(&secondo)
            ));
            assert!(
                prima
                    .iter()
                    .any(|(name, tab, path, line)| name == "selettivo_funzione"
                        && *tab == SearchTab::Simboli
                        && path.as_ref() == Some(&primo)
                        && *line == Some(1))
            );
            let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Navigate(1)));
            assert_eq!(
                app.global_search.update(GlobalSearchMessage::Accept),
                GlobalSearchIntent::Activate(1)
            );
            let generation = app.global_search_gen.load(Ordering::SeqCst);

            let _ = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 78,
                    conn: 1,
                    query: AppQuery::WriteFile {
                        relative: "vietato.txt".into(),
                        content: "no".into(),
                    },
                },
            ));
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
            assert!(app.global_search_gen.load(Ordering::SeqCst) > generation);
            assert!(!app.global_search.is_file_searching());
            assert!(app.global_file_search_worker.is_none());
            assert_eq!(app.global_search.file_query().as_deref(), Some("selettivo"));
            assert_eq!(dati(&app), prima, "dati economici già presenti conservati");
            assert_eq!(
                app.global_search.update(GlobalSearchMessage::Accept),
                GlobalSearchIntent::Activate(1)
            );

            let _ = app.update(tardivo);
            assert_eq!(
                dati(&app),
                prima,
                "la risposta tardiva non sostituisce i dati"
            );
            assert_eq!(
                app.global_search.update(GlobalSearchMessage::Accept),
                GlobalSearchIntent::Activate(1)
            );
            let _ = if esc {
                app.update(Message::CloseAll)
            } else {
                app.update(Message::McpCard(CardMessage::Deny))
            };
            assert_eq!(app.shell.active_overlay(), None);
            assert_eq!(
                dati(&app),
                prima,
                "la chiusura della Card conserva il modello nascosto"
            );
            assert_eq!(
                app.global_search.update(GlobalSearchMessage::Accept),
                GlobalSearchIntent::Activate(1)
            );
            let risposte: Vec<_> = std::iter::from_fn(|| posta.try_recv().ok())
                .filter_map(|cmd| match cmd {
                    McpCommand::Answer { id, answer } => Some((id, answer)),
                    _ => None,
                })
                .collect();
            assert!(matches!(risposte.as_slice(), [(78, Err(_))]));
            assert!(!dir.path().join("vietato.txt").exists());
            assert!(app.global_file_search_worker.is_none());
        }
    });
}

/// La Card sostituisce una ricerca reale: il suo risultato non deve più
/// arrivare al modello nascosto, né il thread sopravvivere alla sostituzione.
#[cfg(feature = "mcp")]
#[test]
fn card_mcp_sospende_ricerca_file_e_riapertura_esplicita_la_riavvia() {
    use crate::mcp_card::CardMessage;
    use lume_mcp::{AppQuery, McpCommand, McpEvent};
    use std::sync::atomic::Ordering;

    runtime().block_on(async {
        for (risultato_pronto, esc) in [(true, false), (false, true)] {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("selettivo.txt");
            std::fs::write(&file, "contenuto reale").unwrap();
            std::fs::write(dir.path().join("altro.txt"), "altro").unwrap();
            let (mut app, _) = Lume::new_without_store();
            app.set_project_root(dir.path().to_path_buf());
            let _ = app.registry.set_feature_enabled("mcp", true);
            let _ = app.mcp_accendi(lume_mcp::confine::canonical(dir.path()).unwrap());
            let epoch = app.mcp.as_ref().unwrap().epoch;
            let (tx, mut posta) = tokio::sync::mpsc::unbounded_channel();
            let _ = app.update(Message::Mcp(epoch, McpEvent::Ready(tx)));
            let _ = app.update(Message::Mcp(
                epoch,
                McpEvent::ClientConnected {
                    conn: 1,
                    name: "cliente".into(),
                },
            ));
            while posta.try_recv().is_ok() {}

            let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Open(
                SearchTab::File,
            )));
            let task = app.update(Message::GlobalSearch(GlobalSearchMessage::QueryChanged(
                "selettivo".into(),
            )));
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::Search));
            assert!(app.global_search.is_file_searching());
            assert!(app.global_file_search_worker.is_some());
            let generation = app.global_search_gen.load(Ordering::SeqCst);
            let mut stream = actions(task);
            let precedente = if risultato_pronto {
                let action = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                    .await
                    .unwrap()
                    .unwrap();
                let iced_runtime::Action::Output(message @ Message::GlobalFileSearch(_, Ok(_))) =
                    action
                else {
                    panic!("risultato filesystem atteso: {action:?}");
                };
                Some(message)
            } else {
                assert!(
                    futures::poll!(stream.next()).is_pending(),
                    "Task della scansione in attesa"
                );
                None
            };

            let _ = app.update(Message::Mcp(
                epoch,
                McpEvent::Ask {
                    id: 77,
                    conn: 1,
                    query: AppQuery::WriteFile {
                        relative: "vietato.txt".into(),
                        content: "no".into(),
                    },
                },
            ));
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
            assert!(app.global_search.is_open());
            assert_eq!(app.global_search.file_query().as_deref(), Some("selettivo"));
            assert!(
                app.global_search_gen.load(Ordering::SeqCst) > generation,
                "scansione revocata sotto la Card"
            );
            assert!(!app.global_search.is_file_searching());
            assert!(
                app.global_file_search_worker.is_none(),
                "worker rilasciato già sotto la Card"
            );

            let precedente = match precedente {
                Some(message) => message,
                None => {
                    let action =
                        tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                            .await
                            .unwrap()
                            .unwrap();
                    let iced_runtime::Action::Output(message @ Message::GlobalFileSearch(_, _)) =
                        action
                    else {
                        panic!("risposta del worker attesa: {action:?}");
                    };
                    message
                }
            };
            let _ = app.update(precedente);
            assert!(
                app.global_search.entries().is_empty(),
                "risposta tardiva scartata"
            );
            assert!(app.global_search.file_search_error().is_none());
            let _ = if esc {
                app.update(Message::CloseAll)
            } else {
                app.update(Message::McpCard(CardMessage::Deny))
            };
            assert_eq!(app.shell.active_overlay(), None);
            assert!(app.global_file_search_worker.is_none());
            assert_eq!(app.global_search.file_query().as_deref(), Some("selettivo"));
            let risposte: Vec<_> = std::iter::from_fn(|| posta.try_recv().ok())
                .filter_map(|cmd| match cmd {
                    McpCommand::Answer { id, answer } => Some((id, answer)),
                    _ => None,
                })
                .collect();
            assert!(matches!(risposte.as_slice(), [(77, Err(_))]));
            assert!(!dir.path().join("vietato.txt").exists());

            let riapertura = if esc {
                let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::ShiftPressed));
                app.update(Message::GlobalSearch(GlobalSearchMessage::ShiftPressed))
            } else {
                app.update(Message::Titlebar(crate::titlebar::TitlebarMessage::Search))
            };
            assert_eq!(app.shell.active_overlay(), Some(OverlayId::Search));
            assert_eq!(app.global_search.file_query().as_deref(), Some("selettivo"));
            assert!(app.global_file_search_worker.is_some());
            let mut riaperto = actions(riapertura);
            loop {
                let action =
                    tokio::time::timeout(std::time::Duration::from_secs(5), riaperto.next())
                        .await
                        .unwrap()
                        .unwrap();
                if let iced_runtime::Action::Output(message @ Message::GlobalFileSearch(_, Ok(_))) =
                    action
                {
                    let _ = app.update(message);
                    break;
                }
            }
            assert!(
                app.global_search
                    .entries()
                    .iter()
                    .any(|entry| entry.path.as_ref() == Some(&file))
            );
            assert!(!app.global_search.is_file_searching());
            let _ = app.update(Message::GlobalSearch(GlobalSearchMessage::Escape));
            assert!(app.global_file_search_worker.is_none());
            assert!(posta.try_recv().is_err(), "nessuna risposta MCP duplicata");
        }
    });
}

#[test]
fn app_cancel_riavvio_con_ack_vecchio_vivo_raccoglie_e_arriva_a_done_e_eof() {
    let (_dir, mut app, task) = fixture();
    runtime().block_on(async {
        let mut old_stream = actions(task);
        let old_packet = first(&mut old_stream).await;
        let old_job = app.search_jobs.current().unwrap();
        app.cancel_search();
        let next_task = app.run_search(
            lume_core::SearchOptions::literal("ago"),
            lume_core::SearchArea::Progetto,
        );
        let new_job = app.search_jobs.current().unwrap();
        assert_ne!(old_job.id(), new_job.id());
        let mut next_stream = actions(next_task);
        let first_new = first(&mut next_stream).await;
        assert!(
            old_job.collected(),
            "il nuovo producer attende il join precedente"
        );
        assert!(old_job.succeeded());
        assert!(matches!(&old_packet, Message::SearchUpdate(_, d) if !d.is_current()));
        let mut files = 0;
        let mut terminals = 0;
        let mut message = Some(first_new);
        loop {
            if let Some(packet) = message.take() {
                let Message::SearchUpdate(_, delivery) = &packet else {
                    unreachable!()
                };
                match delivery.observe().unwrap() {
                    lume_core::SearchUpdate::Batch(batch) => files += batch.len(),
                    lume_core::SearchUpdate::Done { .. } => terminals += 1,
                    other => panic!("esito inatteso: {other:?}"),
                }
                let _ = app.update(packet);
            }
            match tokio::time::timeout(std::time::Duration::from_secs(2), next_stream.next())
                .await
                .unwrap()
            {
                Some(iced_runtime::Action::Output(packet @ Message::SearchUpdate(..))) => {
                    message = Some(packet)
                }
                Some(iced_runtime::Action::Output(packet @ Message::SearchCollected(..))) => {
                    let _ = app.update(packet);
                }
                None => break,
                other => panic!("azione inattesa: {other:?}"),
            }
        }
        assert_eq!(files, 100);
        assert_eq!(terminals, 1);
        assert!(new_job.collected());
        assert!(new_job.succeeded());
        // Il vecchio ack resta vivo per l'intero ciclo della nuova ricerca.
        let _ = app.update(old_packet);
        drop(old_stream);
        cleanup(&mut app).await;
    });
}

#[test]
fn app_non_consegna_secondo_batch_prima_di_applicare_primo() {
    let (_dir, mut app, task) = fixture();
    runtime().block_on(async {
        let mut stream = actions(task);
        let packet = first(&mut stream).await;
        // Nessuna app.update(packet): il messaggio non è stato applicato.
        let second =
            tokio::time::timeout(std::time::Duration::from_millis(200), stream.next()).await;
        app.cancel_search();
        drop(packet);
        drop(stream);
        cleanup(&mut app).await; // anche se la assert del bound fallisse
        assert!(
            second.is_err(),
            "una Task non applicata non deve alimentare la coda UI col secondo evento"
        );
    });
}
#[test]
fn app_message_clonato_non_riapplica_batch_ne_rifiuta_ack_gia_accettato() {
    let (_dir, mut app, task) = fixture();
    runtime().block_on(async {
        let mut stream=actions(task);
        let packet=first(&mut stream).await;
        let copy=packet.clone();
        let Message::SearchUpdate(_,delivery)=&copy else { unreachable!() };
        assert!(matches!(delivery.observe(),Some(lume_core::SearchUpdate::Batch(files)) if files.len()==20));
        let _=app.update(packet);
        assert!(delivery.observe().is_none(),"payload consumato nell'handler UI");
        let _=app.update(copy);
        let next=first(&mut stream).await;
        assert!(matches!(&next,Message::SearchUpdate(_,d) if matches!(d.observe(),Some(lume_core::SearchUpdate::Batch(files)) if files.len()==20)));
        app.cancel_search();drop(next);drop(stream);cleanup(&mut app).await;
    });
}
#[test]
fn app_drop_data_e_collector_task_mantiene_owner_raccoglibile() {
    let (_dir, mut app, task) = fixture();
    runtime().block_on(async {
        let mut stream = actions(task);
        let packet = first(&mut stream).await;
        // Polla il collector fino a Pending, poi abbandona l'intera Task.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), stream.next())
                .await
                .is_err()
        );
        drop(packet);
        drop(stream);
        assert!(!app.search_jobs.current().unwrap().is_current());
        // Guida il vero stream della subscription fallback, non un join UI.
        let mut collectors = Box::pin(crate::search_events::completions(
            &app.search_jobs.snapshot(),
        ));
        while let Some((id, result)) =
            tokio::time::timeout(std::time::Duration::from_secs(2), collectors.next())
                .await
                .unwrap()
        {
            assert!(result.is_ok());
            let _ = app.update(Message::SearchCollected(id, result));
        }
        assert!(!app.search_jobs.busy());
    });
}
#[test]
fn app_ticket_corretto_ma_cleanup_non_completato_non_libera_owner() {
    let (_dir, mut app, task) = fixture();
    runtime().block_on(async {
        let mut stream = actions(task);
        let packet = first(&mut stream).await;
        let job = app.search_jobs.current().unwrap();
        let _ = app.update(Message::SearchCollected(job.id(), Ok(())));
        let retained = app
            .search_jobs
            .snapshot()
            .0
            .iter()
            .any(|j| j.id() == job.id());
        app.cancel_search();
        drop(stream);
        drop(packet);
        assert!(job.collect().await.is_ok());
        cleanup(&mut app).await;
        assert!(
            retained,
            "ticket non è prova del join: receipt precoce non deve rimuovere l'owner"
        );
    });
}
#[test]
fn app_receipt_exit_precoce_non_consuma_uscita_pendente() {
    let (_dir, mut app, task) = fixture();
    runtime().block_on(async {
        let mut stream = actions(task);
        let packet = first(&mut stream).await;
        let close = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
        let epoch = app.search_exit.as_ref().unwrap().epoch;
        let early = app.update(Message::SearchExitReady(epoch, true));
        let retained = app.search_exit.is_some();
        drop(early); // nessuna finestra/renderer guidata nel test
        let mut close = actions(close);
        let receipt = close.next().await.unwrap();
        assert!(matches!(
            receipt,
            iced_runtime::Action::Output(Message::SearchExitReady(_, true))
        ));
        drop(packet);
        drop(stream);
        cleanup(&mut app).await;
        assert!(
            retained,
            "non trattare receipt precoce come autorizzazione a chiudere"
        );
    });
}

#[test]
fn app_task_abbandonata_prima_del_primo_poll_non_avvia_engine() {
    let (_dir, mut app, task) = fixture();
    let job = app.search_jobs.current().unwrap();
    drop(task);
    assert!(!job.is_current());
    runtime().block_on(cleanup(&mut app));
    assert!(!app.search_jobs.busy());
}
#[test]
fn app_cancel_sveglia_ack_silente_anche_se_packet_rimane_vivo() {
    let (_dir, mut app, task) = fixture();
    runtime().block_on(async {
        let mut stream = actions(task);
        let packet = first(&mut stream).await;
        app.cancel_search();
        while let Some(action) =
            tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .unwrap()
        {
            match action {
                iced_runtime::Action::Output(Message::SearchCollected(id, result)) => {
                    assert!(result.is_ok());
                    let _ = app.update(Message::SearchCollected(id, result));
                }
                other => panic!("nessun evento dati superato: {other:?}"),
            }
        }
        // Non è il drop dell'ack a causare il wake: packet vive fino a qui.
        drop(packet);
        assert!(!app.search_jobs.busy());
    });
}
#[test]
fn app_scope_stale_e_ticket_cleanup_errato_non_toccano_job_corrente() {
    let (_dir, mut app, task) = fixture();
    let newdir = tempfile::tempdir().unwrap();
    std::fs::write(newdir.path().join("nuovo.txt"), "ago nuovo\n").unwrap();
    runtime().block_on(async {
        let mut oldstream=actions(task);let oldpacket=first(&mut oldstream).await;
        app.set_project_root(newdir.path().to_path_buf());
        let newtask=app.run_search(lume_core::SearchOptions::literal("ago"),lume_core::SearchArea::Progetto);
        let newjob=app.search_jobs.current().unwrap();
        let count=app.search_jobs.snapshot().0.len();
        let _=app.update(Message::SearchCollected(newjob.id()+100,Ok(())));
        assert_eq!(app.search_jobs.snapshot().0.len(),count);
        assert!(matches!(&oldpacket,Message::SearchUpdate(_,d) if !d.is_current()));
        let _=app.update(oldpacket);
        drop(oldstream);
        let mut newstream=actions(newtask);let newpacket=first(&mut newstream).await;
        assert!(matches!(&newpacket,Message::SearchUpdate(_,d) if matches!(d.observe(),Some(lume_core::SearchUpdate::Batch(files)) if files.iter().all(|f|f.path.starts_with(newdir.path())))));
        let _=app.update(newpacket);
        app.cancel_search();drop(newstream);cleanup(&mut app).await;
    });
}

#[test]
fn app_root_aba_non_consuma_packet_vecchio_e_ricerca_byte_correnti() {
    let (dir, mut app, task) = fixture();
    let other = tempfile::tempdir().unwrap();
    runtime().block_on(async {
        let mut old_stream = actions(task);
        let old_packet = first(&mut old_stream).await;
        let old_job = app.search_jobs.current().unwrap();
        let old_generation = old_job.generation();
        app.set_project_root(other.path().to_path_buf());
        for path in std::fs::read_dir(dir.path()).unwrap() {
            std::fs::write(path.unwrap().path(), "ago dopo ritorno ABA\n").unwrap();
        }
        app.set_project_root(dir.path().to_path_buf());
        let new_task = app.run_search(
            lume_core::SearchOptions::literal("ago"),
            lume_core::SearchArea::Progetto,
        );
        let new_job = app.search_jobs.current().unwrap();
        assert_ne!(old_generation, new_job.generation());
        let Message::SearchUpdate(_, old_delivery) = &old_packet else {
            unreachable!()
        };
        let old_delivery = old_delivery.clone();
        assert!(!old_delivery.is_current());
        assert!(old_delivery.observe().is_some());
        let _ = app.update(old_packet);
        assert!(
            old_delivery.observe().is_some(),
            "handler non consuma payload stale"
        );
        let mut new_stream = actions(new_task);
        let new_packet = first(&mut new_stream).await;
        assert!(old_job.collected());
        assert!(old_job.succeeded());
        let Message::SearchUpdate(_, delivery) = &new_packet else {
            unreachable!()
        };
        assert!(delivery.is_current());
        assert!(
            matches!(delivery.observe(), Some(lume_core::SearchUpdate::Batch(files))
            if files.len() == 20 && files.iter().all(|file| file.path.starts_with(dir.path())
                && file.hits.iter().all(|hit| hit.line_text.contains("dopo ritorno ABA"))))
        );
        let _ = app.update(new_packet);
        app.cancel_search();
        drop(new_stream);
        drop(old_stream);
        cleanup(&mut app).await;
    });
}

#[test]
fn app_nuove_config_non_pollate_non_accumulano_workers_o_owner_infinito() {
    let (_dir, mut app, task) = fixture();
    runtime().block_on(async {
        let mut stream = actions(task);
        let packet = first(&mut stream).await;
        let mut pending = Vec::new();
        for _ in 0..100 {
            pending.push(app.run_search(
                lume_core::SearchOptions::literal("ago"),
                lume_core::SearchArea::Progetto,
            ));
            assert!(
                app.search_jobs.snapshot().0.len() <= 2,
                "un vecchio producer e una config corrente al massimo"
            );
        }
        drop(pending);
        drop(packet);
        drop(stream);
        cleanup(&mut app).await;
    });
}

#[test]
fn app_drop_notifica_raccolta_osservata_fuori_dalla_ui() {
    let (_dir, app, task) = fixture();
    runtime().block_on(async {
        let mut stream = actions(task);
        let packet = first(&mut stream).await;
        let job = app.search_jobs.current().unwrap();
        drop(app);
        drop(stream);
        drop(packet);
        assert!(!job.is_current());
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), job.collect())
                .await
                .unwrap()
                .is_ok()
        );
        // L'osservatore mantiene l'owner. Non è una prova di shutdown GUI.
    });
}
#[test]
fn app_finestra_attende_raccolta_e_receipt_duplicata_non_richiude() {
    let (_dir, mut app, task) = fixture();
    runtime().block_on(async {
        let mut stream=actions(task);let packet=first(&mut stream).await;
        // Le attese vengono invalidate prima del ticket, senza promozione.
        app.apri_overlay(OverlayId::About);
        app.apri_overlay(OverlayId::Scorciatoie);
        let window=iced::window::Id::unique();
        let close=app.update(Message::WindowCloseRequested(window));
        assert!(app.search_exit.is_some(),"non emettere Window::Close prima di raccogliere Cerca");
        let mut close=actions(close);
        let receipt=match tokio::time::timeout(std::time::Duration::from_secs(2),close.next()).await.unwrap() {
            Some(iced_runtime::Action::Output(message @ Message::SearchExitReady(_,true)))=>message,
            other=>panic!("receipt raccolta reale attesa: {other:?}"),
        };
        let copy=receipt.clone();
        let mut effect=actions(app.update(receipt));
        assert!(matches!(effect.next().await,Some(iced_runtime::Action::Window(iced_runtime::window::Action::Close(id))) if id==window));
        assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
        let _ = app.update(Message::CloseAll);
        assert_eq!(app.shell.active_overlay(), None);
        if let Some(mut effects) = iced_runtime::task::into_stream(app.update(copy)) {
            let duplicate = effects.next().await;
            assert!(duplicate.is_none(), "Effetto dopo la ricevuta duplicata: {duplicate:?}");
        }
        drop(packet);drop(stream);cleanup(&mut app).await;
    });
}
#[test]
fn informazioni_rifiutate_durante_uscita_mostrano_avviso() {
    let (_dir, mut app, task) = fixture();
    runtime().block_on(async {
        let mut stream = actions(task);
        let packet = first(&mut stream).await;
        let window = iced::window::Id::unique();
        let close = app.update(Message::WindowCloseRequested(window));
        assert!(app.search_exit.is_some());
        let _ = app.update(Message::MenuAction(crate::menu::MenuAct::About));
        assert_eq!(app.shell.active_overlay(), None);
        assert!(app.notice.as_ref().is_some_and(|(_, n)| n.kind == NoticeKind::Info && n.text.contains("Chiusura")));
        let mut close = actions(close);
        let receipt = match tokio::time::timeout(std::time::Duration::from_secs(2), close.next()).await.unwrap() {
            Some(iced_runtime::Action::Output(message @ Message::SearchExitReady(_, true))) => message,
            other => panic!("receipt: {other:?}"),
        };
        let mut effect = actions(app.update(receipt));
        assert!(matches!(effect.next().await, Some(iced_runtime::Action::Window(iced_runtime::window::Action::Close(id))) if id == window));
        drop(packet);
        drop(stream);
        cleanup(&mut app).await;
    });
}

#[cfg(feature = "mcp")]
#[test]
fn uscita_con_card_in_attesa_raccoglie_cerca_senza_promozioni() {
    let (dir, mut app, task) = fixture();
    let _ = app.mcp_accendi(lume_mcp::confine::canonical(dir.path()).unwrap());
    let epoch = app.mcp.as_ref().unwrap().epoch;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = app.update(Message::Mcp(epoch, lume_mcp::McpEvent::Ready(tx)));
    app.apri_overlay(OverlayId::About);
    let query = || lume_mcp::AppQuery::WriteFile {
        relative: "vietato.txt".into(),
        content: "no".into(),
    };
    let _ = app.update(Message::Mcp(
        epoch,
        lume_mcp::McpEvent::Ask {
            id: 41,
            conn: 1,
            query: query(),
        },
    ));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
    assert_eq!(app.mcp_pending(), 1);
    runtime().block_on(async {
        let mut stream = actions(task);
        let packet = first(&mut stream).await;
        let window = iced::window::Id::unique();
        let close = app.update(Message::WindowCloseRequested(window));
        assert!(app.search_exit.is_some());
        let _ = app.update(Message::Mcp(epoch, lume_mcp::McpEvent::AskGone(41)));
        let _ = app.update(Message::Mcp(epoch, lume_mcp::McpEvent::Ask { id: 42, conn: 1, query: query() }));
        assert_eq!(app.shell.active_overlay(), Some(OverlayId::About));
        let mut close = actions(close);
        let receipt = match tokio::time::timeout(std::time::Duration::from_secs(2), close.next()).await.unwrap() {
            Some(iced_runtime::Action::Output(message @ Message::SearchExitReady(_, true))) => message,
            other => panic!("receipt: {other:?}"),
        };
        let mut effect = actions(app.update(receipt));
        assert!(matches!(effect.next().await, Some(iced_runtime::Action::Window(iced_runtime::window::Action::Close(id))) if id == window));
        let _ = app.update(Message::CloseAll);
        assert_eq!(app.shell.active_overlay(), None);
        assert!(rx.try_recv().is_err());
        assert!(!dir.path().join("vietato.txt").exists());
        drop(packet);
        drop(stream);
        cleanup(&mut app).await;
    });
}

#[cfg(feature = "mcp")]
#[test]
fn stop_mcp_durante_uscita_non_invalida_la_ricevuta_cerca() {
    let (dir, mut app, task) = fixture();
    let _ = app.mcp_accendi(lume_mcp::confine::canonical(dir.path()).unwrap());
    let epoch = app.mcp.as_ref().unwrap().epoch;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = app.update(Message::Mcp(epoch, lume_mcp::McpEvent::Ready(tx)));
    let _ = app.update(Message::Mcp(
        epoch,
        lume_mcp::McpEvent::Ask {
            id: 55,
            conn: 1,
            query: lume_mcp::AppQuery::WriteFile {
                relative: "vietato.txt".into(),
                content: "no".into(),
            },
        },
    ));
    assert_eq!(app.shell.active_overlay(), Some(OverlayId::CardMcp));
    runtime().block_on(async {
        let mut stream = actions(task);
        let packet = first(&mut stream).await;
        let window = iced::window::Id::unique();
        let close = app.update(Message::WindowCloseRequested(window));
        assert!(app.search_exit.is_some());
        assert_eq!(app.shell.active_overlay(), None);
        assert_eq!(app.mcp_pending(), 1);
        let _ = app.update(Message::Mcp(epoch, lume_mcp::McpEvent::Failed("fermo".into())));
        assert_eq!(app.shell.active_overlay(), None);
        let mut close = actions(close);
        let receipt = match tokio::time::timeout(std::time::Duration::from_secs(2), close.next()).await.unwrap() {
            Some(iced_runtime::Action::Output(message @ Message::SearchExitReady(_, true))) => message,
            other => panic!("receipt: {other:?}"),
        };
        let mut effect = actions(app.update(receipt));
        assert!(matches!(effect.next().await, Some(iced_runtime::Action::Window(iced_runtime::window::Action::Close(id))) if id == window));
        assert!(matches!(rx.try_recv(), Ok(lume_mcp::McpCommand::Answer { id: 55, answer: Err(_) })));
        assert!(rx.try_recv().is_err());
        assert!(!dir.path().join("vietato.txt").exists());
        drop(packet);
        drop(stream);
        cleanup(&mut app).await;
    });
}

#[test]
fn app_overlay_nuovo_durante_exit_await_invalida_chiusura() {
    let (_dir, mut app, task) = fixture();
    runtime().block_on(async {
        let mut stream = actions(task);
        let packet = first(&mut stream).await;
        let close = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
        app.apri_overlay(OverlayId::Find);
        let mut close = actions(close);
        let receipt = match close.next().await {
            Some(iced_runtime::Action::Output(message @ Message::SearchExitReady(_, true))) => {
                message
            }
            other => panic!("receipt: {other:?}"),
        };
        if let Some(mut effects) = iced_runtime::task::into_stream(app.update(receipt)) {
            while let Some(action) = effects.next().await {
                assert!(!matches!(
                    action,
                    iced_runtime::Action::Window(iced_runtime::window::Action::Close(_))
                ));
            }
        }
        assert_eq!(app.shell.active_overlay(), Some(OverlayId::Find));
        drop(packet);
        drop(stream);
        cleanup(&mut app).await;
    });
}

#[test]
fn app_edit_durante_exit_await_raccoglie_ma_non_perde_nuovo_testo() {
    let (dir, mut app, task) = fixture();
    app.open_file(&dir.path().join("query-000.txt")).unwrap();
    runtime().block_on(async {
        let mut stream = actions(task);
        let packet = first(&mut stream).await;
        let close = app.update(Message::WindowCloseRequested(iced::window::Id::unique()));
        let id = app.active_doc().unwrap();
        let doc = app.workspace.get_mut(id).unwrap();
        doc.apply(Transaction::new(ChangeSet::insert_at(
            0,
            "modifica tardiva",
            doc.rope().len_chars(),
        )));
        let mut close = actions(close);
        let receipt = match close.next().await {
            Some(iced_runtime::Action::Output(message @ Message::SearchExitReady(_, true))) => {
                message
            }
            other => panic!("receipt: {other:?}"),
        };
        let task = app.update(receipt);
        if let Some(mut effects) = iced_runtime::task::into_stream(task) {
            while let Some(action) = effects.next().await {
                assert!(!matches!(
                    action,
                    iced_runtime::Action::Window(iced_runtime::window::Action::Close(_))
                ));
            }
        }
        assert!(app.workspace.get(id).unwrap().is_dirty());
        assert_eq!(
            app.workspace.get(id).unwrap().rope().to_string(),
            "modifica tardivaago reale\n"
        );
        drop(packet);
        drop(stream);
        cleanup(&mut app).await;
    });
}
