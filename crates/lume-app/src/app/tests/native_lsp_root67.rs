//! Due radici/attori reali: Answer e Stopped vecchi non adottano il successore.
//! Il driver trattiene il vecchio stream per COLLEZIONE: non prova che Iced
//! produzione lo conservi dopo il cambio root né std-thread/reaper drain.
use super::*;
use std::path::Path;

pub(super) fn accensione(app: &mut Lume, root: &Path, base: &Path, program: &Path) -> u64 {
    let spec = lume_language_servers::ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    // Percorso ensure di produzione: eredita la generazione conservata da
    // clear, non ricrea un workspace epoch0 con insert(test).
    app.lsp.ensure(spec, root.to_path_buf());
    let mut env = vec![
        ("CARGO_NET_OFFLINE".into(), "true".into()),
        ("HOME".into(), base.join("home").into_os_string()),
        ("CARGO_HOME".into(), base.join("cargo").into_os_string()),
        (
            "XDG_CONFIG_HOME".into(),
            base.join("config").into_os_string(),
        ),
    ];
    if let Some(home) = std::env::var_os("RUSTUP_HOME").or_else(|| {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".rustup").into_os_string())
    }) {
        env.push(("RUSTUP_HOME".into(), home));
    }
    let lsp = app.lsp.get_mut("rust-analyzer").unwrap();
    lsp.command = Some(ServerCommand {
        program: program.to_path_buf(),
        args: vec![],
        cwd: Some(root.to_path_buf()),
        env,
        source: CommandSource::SystemPath,
    });
    lsp.state = crate::lsp_mode::LspState::Avvio;
    lsp.bump()
}

async fn risposta67(
    app: &mut Lume,
    stream: &mut BoxStream<'static, Message>,
    request: u64,
) -> Esito<Option<Message>> {
    loop {
        let message = evento(stream).await?;
        if let Message::Lsp(_, _, crate::lsp_mode::LspEvent::From(ActorEvent::Answer(id, answer))) =
            &message
            && *id == request
        {
            match &**answer {
                Ok(LspAnswer::Hover(Some(h))) => {
                    let text = serde_json::to_string(&h.contents)?;
                    if !text.contains("valore") || !text.contains("u64") || text.contains("i32") {
                        return Err(io::Error::other("hover del nuovo attore non è u64").into());
                    }
                    return Ok(Some(message));
                }
                Ok(LspAnswer::Hover(None)) => {}
                Err(e) if e.starts_with("server error -32801:") => {}
                _ => return Err(io::Error::other("risposta del successore inattesa").into()),
            }
            consegna(app, message)?;
            return Ok(None);
        }
        consegna(app, message)?;
    }
}

async fn hover_successore(
    app: &mut Lume,
    stream: &mut BoxStream<'static, Message>,
) -> Esito<Message> {
    tokio::time::timeout(ATTESA, async {
        loop {
            let _ = app.update(Message::MenuAction(crate::menu::MenuAct::QuickDoc));
            let request = app.hover_wait.as_ref().unwrap().request;
            if let Some(m) = risposta67(app, stream, request).await? {
                return Ok(m);
            }
            // Fixture attiva readonly, nuova richiesta/ID; no policy prodotto.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?
}

#[derive(Default)]
pub(super) struct Collection67 {
    exited: bool,
    stopped: bool,
    pending_checked: bool,
    failure: Option<Box<dyn Error + Send + Sync>>,
}

pub(super) async fn raccolta67(
    app: &mut Lume,
    stream: &mut BoxStream<'static, Message>,
    state: &mut Collection67,
) -> Esito {
    // Lo stato vive FUORI dalle future cancellabili: marcatori già consumati
    // non devono sparire su timeout/panic o prima del risultato finale.
    while !state.stopped {
        let message = evento(stream).await?;
        if matches!(
            &message,
            Message::Lsp(_, _, crate::lsp_mode::LspEvent::From(ActorEvent::Stopped))
        ) {
            state.stopped = true;
        }
        if let Message::Lsp(
            _,
            _,
            crate::lsp_mode::LspEvent::From(ActorEvent::ProcessExited { success, code }),
        ) = &message
        {
            state.exited = true;
            if !success || *code != Some(0) {
                state
                    .failure
                    .get_or_insert_with(|| io::Error::other("ExitStatus reale non zero").into());
            }
        }
        if let Err(e) = consegna(app, message) {
            state.failure.get_or_insert(e);
        }
    }
    if state.failure.is_some() {
        return Err(io::Error::other(
            "raccolta con errore conservato; stato terminale non riatteso",
        )
        .into());
    }
    assert!(
        state.exited,
        "Stopped non sostituisce osservazione del ProcessExited"
    );
    // Distinto dai marcatori terminali e ripetibile senza consumarli di nuovo.
    if !state.pending_checked {
        state.pending_checked = tokio::time::timeout(Duration::from_millis(30), stream.next())
            .await
            .is_err();
        assert!(state.pending_checked);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "root/epoch consumer reale opt-in: test-app-lsp-root-real"]
async fn recipe_due_radici_answer_stopped_vecchi_successore_reali() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("binario reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("binario reale non assoluto/file").into());
    }
    eprintln!(
        "APP_REALE67:binario_sha256={:x}",
        <sha2::Sha256 as sha2::Digest>::digest(std::fs::read(&program)?)
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-root67-città spazio-")
        .tempdir()?;
    let first = fixture.path().join("prima");
    let second = fixture.path().join("seconda");
    for (root, name) in [(&first, "prima67"), (&second, "seconda67")] {
        std::fs::create_dir(root)?;
        std::fs::create_dir(root.join("src"))?;
        std::fs::write(
            root.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
        )?;
        std::fs::write(root.join("src/lib.rs"), "pub mod città;\n")?;
    }
    let mut app = app();
    let first_path = write_and_open(&mut app, &first.join("src"), "città.rs", TESTO);
    let first_doc = app.active_doc().unwrap();
    let anchor = TESTO[..TESTO.rfind("valore;").unwrap()].chars().count();
    app.workspace
        .get_mut(first_doc)
        .unwrap()
        .set_selection(lume_core::Range::point(anchor + 1));
    app.project_root = Some(first.clone());
    let old_epoch = accensione(&mut app, &first, fixture.path(), &program);
    let mut old_stream = recipe(&app);
    let mut new_stream = None;
    let mut old_collection = Collection67::default();
    let mut new_collection = Collection67::default();
    let scenario = limitato(
        async {
            while !app.lsp.only().unwrap().can_hover() {
                consegna(&mut app, evento(&mut old_stream).await?)?;
            }
            let old_answer = hover_reale(&mut app, &mut old_stream).await?;
            // Cambia root attraverso update. Non pollare Task Git/Store/PTY né
            // recipe watcher: la fixture verifica SOLO il consumer LSP.
            let _ = app.update(Message::FolderPicked(Some(second.clone())));
            assert!(app.lsp.is_empty());
            assert!(app.lsp_spawns().is_empty());
            assert!(app.completion.is_none());
            let next_text = TESTO.replace("i32", "u64");
            let path = write_and_open(&mut app, &second.join("src"), "città.rs", &next_text);
            let doc = app.active_doc().unwrap();
            let next_anchor = next_text[..next_text.rfind("valore;").unwrap()]
                .chars()
                .count();
            app.workspace
                .get_mut(doc)
                .unwrap()
                .set_selection(lume_core::Range::point(next_anchor + 1));
            let new_epoch = accensione(&mut app, &second, fixture.path(), &program);
            assert!(
                new_epoch > old_epoch,
                "clear/ensure non ricicla generazione del vecchio server"
            );
            new_stream = Some(recipe(&app));
            let current_stream = new_stream.as_mut().unwrap();
            while !app.lsp.only().unwrap().can_hover() {
                consegna(&mut app, evento(current_stream).await?)?;
            }
            let _ = app.update(Message::MenuAction(crate::menu::MenuAct::QuickDoc));
            let current_wait = app.hover_wait.as_ref().unwrap().clone();
            consegna(&mut app, old_answer)?;
            assert_eq!(
                app.hover_wait.as_ref(),
                Some(&current_wait),
                "vecchio Answer non sottrae il successore"
            );
            assert!(app.hover.is_none());
            let answer = match risposta67(&mut app, current_stream, current_wait.request).await? {
                Some(m) => m,
                None => hover_successore(&mut app, current_stream).await?,
            };
            consegna(&mut app, answer)?;
            let shown = app.hover.as_ref().unwrap();
            assert_eq!(shown.doc, doc);
            assert!(shown.card.lines.iter().any(|l| l.text.contains("u64")));
            // Il vecchio stream è mantenuto dal TEST per raccogliere l'attore
            // dopo last-mailbox Drop; non dedurre che la UI produzione lo faccia.
            raccolta67(&mut app, &mut old_stream, &mut old_collection).await?;
            assert_eq!(app.lsp.only().unwrap().epoch, new_epoch);
            assert!(
                app.lsp.only().unwrap().state.is_live(),
                "Stopped vecchio non ferma il nuovo workspace"
            );
            assert_eq!(
                app.lsp.only().unwrap().opened.get(&doc),
                crate::lsp_uri::path_to_uri(&path).as_ref()
            );
            assert_eq!(std::fs::read_to_string(&first_path)?, TESTO);
            assert_eq!(std::fs::read_to_string(&path)?, next_text);
            eprintln!(
                "APP_REALE67:old_epoch={old_epoch},new_epoch={new_epoch},exit_status_vecchio=0"
            );
            Ok(())
        },
        "root67 scenario: raccolta vecchio/nuovo sarà tentata",
    )
    .await;
    let shutdown = limitato(
        async {
            let _ = app.stop_lsp();
            Ok(())
        },
        "root67 invio stop incerto",
    )
    .await;
    // Budget/catch distinti: il vecchio non può cancellare persino il
    // tentativo sul nuovo. Nessun retry RPC/kill, solo raccolta degli owner.
    let old_stop = limitato(
        raccolta67(&mut app, &mut old_stream, &mut old_collection),
        "root67 raccolta vecchio incerta",
    )
    .await;
    let new_stop = if let Some(stream) = new_stream.as_mut() {
        limitato(
            raccolta67(&mut app, stream, &mut new_collection),
            "root67 raccolta nuovo incerta",
        )
        .await
    } else {
        Ok(())
    };
    let first_error = shutdown.err().or(old_stop.err()).or(new_stop.err());
    drop(old_stream);
    drop(new_stream);
    if let Some(e) = first_error {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE67:fixture_preservata={}; raccolta incerta",
            residue.display()
        );
        return Err(e);
    }
    assert!(app.lsp_spawns().is_empty());
    scenario?;
    fixture.close()?;
    eprintln!(
        "APP_REALE67:sequenza_completa=root_change_old_answer_old_stopped_new_hover_two_exitstatus_drop"
    );
    Ok(())
}
