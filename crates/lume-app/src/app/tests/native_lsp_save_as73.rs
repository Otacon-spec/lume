//! SaveAs worker/app reali e controprova server old overlay liberato/new open.
//! Chooser message controllato: NON dialogo nativo; Ready passa da update.
use super::root67::{Collection67, accensione, raccolta67};
use super::*;

fn accetta73(app: &mut Lume, message: Message, doc: lume_core::DocId, expected: &str) {
    let _ = app.update(message);
    let shown = app
        .hover
        .as_ref()
        .expect("hover realmente accettato dal consumer UI");
    assert_eq!(shown.doc, doc);
    let text = shown
        .card
        .lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("valore") && text.contains(expected),
        "DTO UI conserva il tipo nativo"
    );
    for other in ["i32", "u64", "i16"] {
        if other != expected {
            assert!(!text.contains(other), "DTO UI non mantiene un tipo vecchio");
        }
    }
}

// Domande readonly vere. I probe oldURI non sono richieste dell'editor corrente
// e non vengono consumati dalla sua tabella; ID fixture distinti/monotoni.
async fn hover73(
    app: &mut Lume,
    stream: &mut BoxStream<'static, Message>,
    expected: &str,
    probe: Option<lsp_types::Uri>,
    next_probe: &mut u64,
) -> Esito<Message> {
    tokio::time::timeout(ATTESA, async {
        loop {
            let request = if let Some(uri) = &probe {
                let request = *next_probe;
                assert!(request >= 1_000_000);
                *next_probe = next_probe
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("ID probe esauriti"))?;
                let line = TESTO.lines().nth(1).unwrap();
                let column = line[..line.rfind("valore;").unwrap() + 1]
                    .encode_utf16()
                    .count();
                assert!(app.lsp.only().unwrap().send(lume_lsp::LspCommand::Ask {
                    id: request,
                    request: lume_lsp::LspRequest::Hover {
                        uri: uri.clone(),
                        position: lsp_types::Position::new(1, u32::try_from(column)?)
                    }
                }));
                request
            } else {
                let _ = app.update(Message::MenuAction(crate::menu::MenuAct::QuickDoc));
                let request = app
                    .hover_wait
                    .as_ref()
                    .ok_or_else(|| io::Error::other("attesa hover UI assente"))?
                    .request;
                assert!(
                    request < 1_000_000,
                    "ID UI osservati disgiunti dai probe fixture"
                );
                request
            };
            loop {
                let message = evento(stream).await?;
                if let Message::Lsp(
                    _,
                    _,
                    crate::lsp_mode::LspEvent::From(ActorEvent::Answer(id, answer)),
                ) = &message
                    && *id == request
                {
                    match &**answer {
                        Ok(LspAnswer::Hover(Some(h))) => {
                            let text = serde_json::to_string(&h.contents)?;
                            assert!(
                                text.contains("valore") && text.contains(expected),
                                "tipo atteso nel vero hover"
                            );
                            for other in ["i32", "u64", "i16"] {
                                if other != expected {
                                    assert!(
                                        !text.contains(other),
                                        "hover non contiene anche un tipo contrastante"
                                    );
                                }
                            }
                            return Ok(message);
                        }
                        Ok(LspAnswer::Hover(None)) => {}
                        Err(e) if e.starts_with("server error -32801:") => {}
                        _ => {
                            return Err(io::Error::other(
                                "probe hover reale inatteso; errore grezzo non pubblicato",
                            )
                            .into());
                        }
                    }
                    if probe.is_none() {
                        consegna(app, message)?;
                    }
                    break;
                }
                consegna(app, message)?;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "SaveAs LSP reale opt-in: test-app-lsp-save-as-real"]
async fn save_as_worker_native_old_close_new_open_overlay_reali() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("binario reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("binario reale non assoluto/file").into());
    }
    eprintln!(
        "APP_REALE73:binario_sha256={}",
        crate::aggiornamento::esadecimale(&<sha2::Sha256 as sha2::Digest>::digest(std::fs::read(
            &program
        )?))
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-saveas73-città spazio-")
        .tempdir()?;
    let root = fixture.path();
    let src = root.join("src");
    std::fs::create_dir(&src)?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"native_saveas73\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    )?;
    std::fs::write(src.join("lib.rs"), "pub mod città;\npub mod nuova;\n")?;
    let mut app = app();
    let old = write_and_open(&mut app, &src, "città.rs", TESTO);
    let new = src.join("nuova.rs");
    let doc = app.active_doc().unwrap();
    let anchor = TESTO[..TESTO.rfind("valore;").unwrap()].chars().count() + 1;
    app.workspace
        .get_mut(doc)
        .unwrap()
        .set_selection(lume_core::Range::point(anchor));
    app.project_root = Some(root.to_path_buf());
    let _ = accensione(&mut app, root, root, &program);
    let mut stream = recipe(&app);
    let mut collection = Collection67::default();
    let mut job: Option<crate::save::Job> = None;
    let mut save_maybe_started = false;
    let mut save_collected = false;
    let mut next_probe = 1_000_000u64;
    let changed = TESTO.replace("i32", "u64");
    let scenario = limitato(async {
        while !app.lsp.only().unwrap().can_hover() { consegna(&mut app, evento(&mut stream).await?)?; }
        let answer = hover73(&mut app, &mut stream, "i32", None, &mut next_probe).await?;
        accetta73(&mut app, answer, doc, "i32");
        let old_uri = app.lsp.only().unwrap().opened.get(&doc).unwrap().clone();
        let group = app.groups.focus();
        let _ = app.update(Message::EditorIn(group, lume_ui::editor::EditorCommand::SelectAll));
        let _ = app.update(Message::EditorIn(group, lume_ui::editor::EditorCommand::Insert(changed.clone())));
        app.workspace.get_mut(doc).unwrap().set_selection(lume_core::Range::point(anchor));
        let answer = hover73(&mut app, &mut stream, "u64", None, &mut next_probe).await?;
        accetta73(&mut app, answer, doc, "u64");
        // Backend witness: oldURI overlay è davvero diverso dal disco PRIMA
        // della scrittura; la sola assenza dalla mappa UI non basta per Close.
        let _ = hover73(&mut app, &mut stream, "u64", Some(old_uri.clone()), &mut next_probe).await?;
        assert!(app.workspace.get(doc).unwrap().is_dirty());
        assert_eq!(std::fs::read_to_string(&old)?, TESTO);
        drop(app.update(Message::MenuAction(crate::menu::MenuAct::SaveAs)));
        let nonce = app.save_as_picker.as_ref().unwrap().sequence;
        let task = app.update(Message::SaveAsPicked(nonce, Some(new.clone())));
        job = app.save_job.clone();
        assert!(job.is_some() && app.save_as_flight.is_some());
        assert!(!new.exists(), "chooser non scrive sul consumer");
        let mut writes = iced_runtime::task::into_stream(task).ok_or_else(|| io::Error::other("Task SaveAs assente"))?;
        save_maybe_started = true;
        let prepared = writes.next().await.ok_or_else(|| io::Error::other("EOF prima del Ready SaveAs"))?;
        let iced_runtime::Action::Output(ready @ Message::SaveReady(_)) = prepared else {
            return Err(io::Error::other("Ready SaveAs autentico assente").into());
        };
        assert!(!new.exists(), "prepare non pubblica prima dell'autorità UI");
        let _ = app.update(ready);
        let action = writes.next().await.ok_or_else(|| io::Error::other("EOF prima del receipt SaveAs"))?;
        let iced_runtime::Action::Output(message @ Message::SaveCollected(_)) = action else {
            return Err(io::Error::other("output SaveAs autentico assente").into());
        };
        assert!(writes.next().await.is_none(), "un solo receipt ed EOF");
        save_collected = true;
        assert_eq!(std::fs::read_to_string(&new)?, changed);
        let _ = app.update(message); // autenticazione Job Arc e ackold→rebind production.
        assert!(app.save_job.is_none() && app.save_as_flight.is_none());
        assert_eq!(app.workspace.get(doc).unwrap().path(), Some(new.as_path()));
        assert_eq!(app.workspace.get_by_path(&old), None);
        assert_eq!(app.workspace.get_by_path(&new), Some(doc));
        assert!(!app.workspace.get(doc).unwrap().is_dirty());
        assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), changed);
        let new_uri = crate::lsp_uri::path_to_uri(&new).unwrap();
        assert_ne!(old_uri, new_uri);
        assert_eq!(app.lsp.only().unwrap().opened.get(&doc), Some(&new_uri));
        let answer = hover73(&mut app, &mut stream, "u64", None, &mut next_probe).await?;
        accetta73(&mut app, answer, doc, "u64");
        // Un hover nuovo uguale al disco non dimostra Open. Nuovo edit non
        // salvato: Change su newURI richiede il wire DocumentStore aperto e
        // il server deve mostrare i16 mentre il file nuovo rimane u64.
        let again = changed.replace("u64", "i16");
        let _ = app.update(Message::EditorIn(group, lume_ui::editor::EditorCommand::SelectAll));
        let _ = app.update(Message::EditorIn(group, lume_ui::editor::EditorCommand::Insert(again)));
        app.workspace.get_mut(doc).unwrap().set_selection(lume_core::Range::point(anchor));
        let answer = hover73(&mut app, &mut stream, "i16", None, &mut next_probe).await?;
        accetta73(&mut app, answer, doc, "i16");
        assert!(app.workspace.get(doc).unwrap().is_dirty());
        assert_eq!(std::fs::read_to_string(&new)?, changed);
        // Native oldURI torna al disco i32: Close ha tolto l'overlay u64.
        // Non serve eseguire comandi, cambiare il file originale o fakeAnswers.
        let _ = hover73(&mut app, &mut stream, "i32", Some(old_uri), &mut next_probe).await?;
        assert_eq!(std::fs::read_to_string(&old)?, TESTO);
        assert_eq!(std::fs::read_to_string(&new)?, changed);
        let _ = app.update(Message::EditorIn(group, lume_ui::editor::EditorCommand::Undo));
        assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), changed);
        assert!(!app.workspace.get(doc).unwrap().is_dirty());
        app.workspace.get_mut(doc).unwrap().set_selection(lume_core::Range::point(anchor));
        let answer = hover73(&mut app, &mut stream, "u64", None, &mut next_probe).await?;
        accetta73(&mut app, answer, doc, "u64");
        eprintln!("APP_REALE73:old_overlay_before=u64;old_disk_after=i32;new_buffer_hover=i16_disk_u64;undo_clean=true");
        Ok(())
    }, "SaveAs73 scenario: collection separata sarà tentata").await;
    let saves = limitato(
        async {
            if save_maybe_started && !save_collected {
                let owner = job
                    .as_ref()
                    .ok_or_else(|| io::Error::other("owner SaveAs mancante"))?;
                owner.discard_pending(Some(doc)); // non rollback di un writer admitted.
                let receipt = owner.collect().await;
                save_collected = true;
                let _ = app.update(Message::SaveCollected(receipt));
            }
            Ok(())
        },
        "SaveAs73 save collector incerto",
    )
    .await;
    let stop = limitato(
        async {
            let _ = app.stop_lsp();
            raccolta67(&mut app, &mut stream, &mut collection).await
        },
        "SaveAs73 actor collector incerto",
    )
    .await;
    drop(stream);
    drop(job);
    if let Some(error) = saves.err().or(stop.err()) {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE73:fixture_preservata={}; writer/actor incerti",
            residue.display()
        );
        return Err(error);
    }
    if save_maybe_started && !save_collected {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE73:fixture_preservata={}; receipt non raccolto",
            residue.display()
        );
        scenario?;
        return Err(io::Error::other("writer forse avviato non raccolto").into());
    }
    scenario?;
    fixture.close()?;
    eprintln!(
        "APP_REALE73:sequenza_completa=native_old_overlay_saveas_actual_worker_receipt_rebind_new_hover_old_disk_exitstatus_drop"
    );
    Ok(())
}
