//! Diagnostica push reale → store/badge/segni, Change/clear e stale publish.
//! Task8 mantiene segni approssimati fra pubblicazioni; non inventare
//! uguaglianza tra versioni del core e contatori wire del DocumentStore.
use super::*;

const ERRATO: &str = "pub fn usa() { let _emoji = \"😀\"; let valore = ; let _ = valore; }\n";
const CORRETTO: &str = "pub fn usa() { let _emoji = \"😀\"; let valore = 1; let _ = valore; }\n";

async fn diagnostica_reale(
    app: &mut Lume,
    stream: &mut BoxStream<'static, Message>,
    uri: &lsp_types::Uri,
    vuota: bool,
    dopo: Option<i32>,
) -> Esito<Message> {
    tokio::time::timeout(ATTESA, async {
        loop {
            let message = evento(stream).await?;
            if let Message::Lsp(
                _,
                _,
                crate::lsp_mode::LspEvent::From(ActorEvent::Diagnostics(params)),
            ) = &message
                && params.uri == *uri
                && params.diagnostics.is_empty() == vuota
                && params
                    .version
                    .is_some_and(|v| dopo.is_none_or(|old| v > old))
            {
                return Ok(message);
            }
            consegna(app, message)?;
        }
    })
    .await?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "diagnostiche consumer reale opt-in: test-app-lsp-diagnostics-real"]
async fn recipe_diagnostica_segni_change_clear_stale_reali() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("binario reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("binario reale non assoluto/file").into());
    }
    eprintln!(
        "APP_REALE66:binario_sha256={:x}",
        <sha2::Sha256 as sha2::Digest>::digest(std::fs::read(&program)?)
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-diag66-città spazio-")
        .tempdir()?;
    let root = fixture.path();
    std::fs::create_dir(root.join("src"))?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"diagnostics66\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )?;
    std::fs::write(root.join("src/lib.rs"), "pub mod città;\n")?;
    let mut app = app();
    let path = write_and_open(&mut app, &root.join("src"), "città.rs", ERRATO);
    let doc = app.active_doc().unwrap();
    let uri = crate::lsp_uri::path_to_uri(&path).unwrap();
    let initial_core = app.workspace.get(doc).unwrap().version();
    app.project_root = Some(root.to_path_buf());
    let mut env = vec![
        ("CARGO_NET_OFFLINE".into(), "true".into()),
        ("HOME".into(), root.join("home").into_os_string()),
        ("CARGO_HOME".into(), root.join("cargo").into_os_string()),
        (
            "XDG_CONFIG_HOME".into(),
            root.join("config").into_os_string(),
        ),
    ];
    if let Some(home) = std::env::var_os("RUSTUP_HOME").or_else(|| {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".rustup").into_os_string())
    }) {
        env.push(("RUSTUP_HOME".into(), home));
    }
    let spec = lume_language_servers::ServerCatalog::embedded()
        .unwrap()
        .get("rust-analyzer")
        .unwrap();
    let mut lsp = crate::lsp_mode::LspWorkspace::new(spec, root.to_path_buf());
    lsp.command = Some(ServerCommand {
        program,
        args: vec![],
        cwd: Some(root.to_path_buf()),
        env,
        source: CommandSource::SystemPath,
    });
    lsp.state = crate::lsp_mode::LspState::Avvio;
    let _ = lsp.bump();
    app.lsp.insert(lsp);
    let mut stream = recipe(&app);
    let scenario = limitato(async {
        let bad = diagnostica_reale(&mut app, &mut stream, &uri, false, None).await?;
        let (wire_old, expected, diagnostic) = match &bad {
            Message::Lsp(_, _, crate::lsp_mode::LspEvent::From(ActorEvent::Diagnostics(p))) => {
                let after_emoji = ERRATO[..ERRATO.find('😀').unwrap()].encode_utf16().count() + 2;
                let d = p.diagnostics.iter().find(|d|
                    d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)
                    && d.range.start.line == 0 && d.range.end.line == 0
                    && usize::try_from(d.range.start.character).unwrap() > after_emoji)
                    .ok_or_else(|| io::Error::other("nessun errore reale dopo emoji"))?;
                // Fixture singola riga: l'unico scalar a due unità UTF16 è
                // l'emoji. Oracle indipendente da diag_marks/lsp_pos.
                let start = usize::try_from(d.range.start.character)? - 1;
                let end = usize::try_from(d.range.end.character)? - 1;
                (p.version.unwrap(), start..end, d.clone())
            }
            _ => return Err(io::Error::other("evento diverso da diagnostica").into()),
        };
        let stale = bad.clone(); // Payload veramente ricevuto, non sintetico.
        consegna(&mut app, bad)?;
        let problems = app.lsp.only().unwrap().problems.for_uri(&uri);
        let related = diagnostic.related_information.clone().unwrap_or_default();
        assert!(problems.iter().any(|p| p.uri == uri
            && p.range == diagnostic.range && p.severity == diagnostic.severity
            && p.code == diagnostic.code && p.source == diagnostic.source
            && p.message == diagnostic.message && p.related == related),
            "campi conservati del payload reale fedeli nello store");
        assert!(app.doc_marks.get(&doc).unwrap().iter().any(|m|
            m.range == expected && m.severity == lume_ui::editor::MarkSeverity::Error));
        assert!(badge_problemi(&app).is_some());
        let _ = app.update(Message::EditorIn(crate::editor_groups::GroupId::Primary,
            EditorCommand::SelectAll));
        let _ = app.update(Message::EditorIn(crate::editor_groups::GroupId::Primary,
            EditorCommand::Insert(CORRETTO.into())));
        assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), CORRETTO);
        assert!(app.workspace.get(doc).unwrap().version() > initial_core);
        // Prima di consegnare nuove pubblicazioni, Task8 conserva i segni
        // approssimati; non richiediamo identiche coordinate o core==wire.
        assert!(!app.lsp.only().unwrap().problems.for_uri(&uri).is_empty());
        assert!(app.doc_marks.contains_key(&doc));
        assert!(badge_problemi(&app).is_some());
        // F7 percorre lsp_flush_change PRIMA dell'Ask reale; niente evento
        // Debounce/Diagnostics finto o invio Change direttamente dal test.
        let anchor = CORRETTO[..CORRETTO.rfind("valore;").unwrap()].chars().count();
        app.workspace.get_mut(doc).unwrap().set_selection(lume_core::Range::point(anchor + 1));
        let _ = app.update(Message::MenuAction(crate::menu::MenuAct::QuickDoc));
        assert!(app.hover_wait.is_some(), "F7 genera una domanda dopo la correzione");
        let clear = diagnostica_reale(&mut app, &mut stream, &uri, true, Some(wire_old)).await?;
        let wire_new = match &clear {
            Message::Lsp(_, _, crate::lsp_mode::LspEvent::From(ActorEvent::Diagnostics(p))) => p.version.unwrap(),
            _ => return Err(io::Error::other("clear diverso da diagnostica").into()),
        };
        consegna(&mut app, clear)?;
        assert!(app.lsp.only().unwrap().problems.for_uri(&uri).is_empty());
        assert!(!app.doc_marks.contains_key(&doc));
        assert!(badge_problemi(&app).is_none());
        assert_eq!(app.lsp.only().unwrap().problems.generation_count(), 1,
            "URI aperto conserva generazione vuota per la guardia di versione (N30)");
        consegna(&mut app, stale)?;
        assert!(app.lsp.only().unwrap().problems.for_uri(&uri).is_empty());
        assert!(!app.doc_marks.contains_key(&doc));
        assert!(badge_problemi(&app).is_none(), "vecchia pubblicazione non resuscita errori");
        assert_eq!(std::fs::read_to_string(&path)?, ERRATO, "correzione resta nel buffer, non salva");
        eprintln!("APP_REALE66:core_old={initial_core},core_new={},wire_old={wire_old},wire_new={wire_new}",
            app.workspace.get(doc).unwrap().version());
        Ok(())
    }, "diagnostics66 scenario: stop/raccolta saranno tentati").await;
    let stop = limitato(
        async {
            let _ = app.stop_lsp();
            let mut failure = None;
            loop {
                let message = evento(&mut stream).await?;
                let stopped = matches!(
                    &message,
                    Message::Lsp(_, _, crate::lsp_mode::LspEvent::From(ActorEvent::Stopped))
                );
                if let Err(e) = consegna(&mut app, message) {
                    failure.get_or_insert(e);
                }
                if stopped {
                    break;
                }
            }
            assert!(app.lsp_spawns().is_empty());
            if let Some(e) = failure {
                return Err(e);
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(30), stream.next())
                    .await
                    .is_err()
            );
            Ok(())
        },
        "diagnostics66 stop: processo/reaper incerti",
    )
    .await;
    drop(stream);
    if let Err(e) = stop {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE66:fixture_preservata={}; raccolta incerta",
            residue.display()
        );
        return Err(e);
    }
    scenario?;
    fixture.close()?;
    eprintln!(
        "APP_REALE66:sequenza_completa=native_diagnostics_marks_edit_clear_stale_stopped_drop"
    );
    Ok(())
}
