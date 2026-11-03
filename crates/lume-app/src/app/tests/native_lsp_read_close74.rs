//! Definition nativa/reader app già completato → Close sorgente e reopen ABA.
//! No nuovo guard/API: qualifica nominale del ramo source missing introdotto71.
use super::definition68::{SOURCE, TARGET, definizione_reale};
use super::root67::{Collection67, accensione, raccolta67};
use super::*;

pub(super) async fn lettura74(
    task: iced::Task<Message>,
    maybe_started: &mut bool,
    finished: &mut bool,
) -> Esito<Message> {
    *maybe_started = true; // conservativo, prima del primo poll di ogni Task.
    *finished = false;
    let mut reading = iced_runtime::task::into_stream(task)
        .ok_or_else(|| io::Error::other("Task reader app assente"))?;
    let action = tokio::time::timeout(ATTESA, reading.next())
        .await?
        .ok_or_else(|| io::Error::other("EOF prima dell'output reader"))?;
    let iced_runtime::Action::Output(message @ Message::FileReadFinished(_, _, _)) = action else {
        return Err(io::Error::other("non FileReadFinished prodotto dal reader reale").into());
    };
    if tokio::time::timeout(ATTESA, reading.next())
        .await?
        .is_some()
    {
        return Err(io::Error::other("output ulteriore dopo il reader reale").into());
    }
    *finished = true; // output+EOF prima di assertion/update; flag esterno.
    Ok(message)
}

pub(super) fn risultato74(
    message: &Message,
    path: &std::path::Path,
    text: &str,
    file_epoch: u64,
    origin: Option<(u64, lume_core::DocId, u64)>,
) {
    let Message::FileReadFinished(epoch, _, result) = message else {
        panic!("reader autentico richiesto")
    };
    assert_eq!(*epoch, file_epoch);
    assert_eq!(result.path, path);
    assert_eq!(&**result.esito.as_ref().unwrap(), text);
    match origin {
        Some((root, doc, version)) => assert!(matches!(result.dopo,
            crate::file_read::DopoApertura::PosizioneLsp { progetto, origine, .. }
            if progetto == root && origine == (doc, version))),
        None => assert!(matches!(
            result.dopo,
            crate::file_read::DopoApertura::Niente
        )),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "reader LSP source close reale opt-in: test-app-lsp-read-close-real"]
async fn recipe_definizione_reader_source_close_reopen_aba_rifiuta_reale() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("binario reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("binario reale non assoluto/file").into());
    }
    eprintln!(
        "APP_REALE74:binario_sha256={:x}",
        <sha2::Sha256 as sha2::Digest>::digest(std::fs::read(&program)?)
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-read74-città spazio-")
        .tempdir()?;
    let root = fixture.path().join("progetto");
    let src = root.join("src");
    std::fs::create_dir_all(&src)?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"native_read74\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    )?;
    let target = src.join("città.rs");
    std::fs::write(&target, TARGET)?;
    let mut app = app();
    const OTHER: &str = "pub fn resta() { let _ = \"😀\"; }\n";
    let other_path = write_and_open(&mut app, &src, "resta.rs", OTHER);
    let other_doc = app.active_doc().unwrap();
    let other_caret = OTHER[..OTHER.find("resta").unwrap()].chars().count() + 2;
    app.workspace
        .get_mut(other_doc)
        .unwrap()
        .set_selection(lume_core::Range::point(other_caret));
    let source = write_and_open(&mut app, &src, "lib.rs", SOURCE);
    let doc = app.active_doc().unwrap();
    let anchor = SOURCE[..SOURCE.find("dato()").unwrap()].chars().count() + 1;
    app.workspace
        .get_mut(doc)
        .unwrap()
        .set_selection(lume_core::Range::point(anchor));
    app.project_root = Some(root.clone());
    let _ = accensione(&mut app, &root, fixture.path(), &program);
    let mut stream = recipe(&app);
    let mut collected = Collection67::default();
    let mut read_maybe_started = false;
    let mut read_finished = false;
    let scenario = limitato(async {
        while !app.lsp.only().unwrap().can_define() { consegna(&mut app, evento(&mut stream).await?)?; }
        let version = app.workspace.get(doc).unwrap().version();
        let epoch = app.git_root_epoch;
        let file_epoch = app.scritture.guarda(None).epoca;
        let mut old = Vec::new();
        // Due domande native/Task app diversi, non clone di un reader DTO.
        for _ in 0..2 {
            let answer = definizione_reale(&mut app, &mut stream, &target).await?;
            let task = app.update(answer);
            assert!(app.definition_wait.is_none(), "Answer consumato prima del reader");
            let message = lettura74(task, &mut read_maybe_started, &mut read_finished).await?;
            risultato74(&message, &target, TARGET, file_epoch, Some((epoch, doc, version)));
            old.push(message);
            assert!(app.workspace.get_by_path(&target).is_none());
        }
        assert!(!app.workspace.get(doc).unwrap().is_dirty(), "chiusura pulita non richiede discard");
        let _ = app.update(Message::EditorTabBar(crate::editor_tab_bar::EditorTabBarMessage::Close(doc)));
        assert!(app.workspace.get(doc).is_none(), "Close consumer ha rimosso davvero la sorgente");
        assert!(app.workspace.get_by_path(&source).is_none());
        assert!(!app.groups.all().any(|id| id == doc));
        assert!(!app.lsp.only().unwrap().opened.contains_key(&doc));
        assert_eq!(app.active_doc(), Some(other_doc));
        assert_eq!(app.workspace.get(other_doc).unwrap().version(), version, "revisione uguale del doc attivo non autorizza l'origine assente");
        let old_first = old.remove(0);
        app.notice = None; // la notice deve provenire da questa consegna.
        let _ = app.update(old_first);
        assert!(app.workspace.get_by_path(&target).is_none());
        assert_eq!(app.active_doc(), Some(other_doc));
        assert_eq!(app.workspace.get(other_doc).unwrap().selection().head, other_caret);
        assert_eq!(app.notice.as_ref().unwrap().1.text, "Lettura LSP superata da una modifica o chiusura del documento sorgente: ripeti il salto");
        assert!(matches!(app.notice.as_ref().unwrap().1.kind, NoticeKind::Info));
        assert_eq!(app.git_root_epoch, epoch, "root guard non spiega il rifiuto");
        assert_eq!(app.scritture.guarda(None).epoca, file_epoch, "file guard non spiega il rifiuto");
        // Riapertura manuale vera dello STESSO nome/contenuto/versione; nuovo
        // DocId. L'origine del secondo reader rimane il DocId chiuso.
        let manual = app.open_file_task(source.clone(), crate::file_read::DopoApertura::Niente);
        let message = lettura74(manual, &mut read_maybe_started, &mut read_finished).await?;
        risultato74(&message, &source, SOURCE, file_epoch, None);
        let _ = app.update(message);
        let reopened = app.workspace.get_by_path(&source).unwrap();
        assert_ne!(reopened, doc);
        assert_eq!(app.active_doc(), Some(reopened));
        assert_eq!(app.workspace.get(reopened).unwrap().version(), version);
        assert!(!app.workspace.get(reopened).unwrap().is_dirty());
        app.workspace.get_mut(reopened).unwrap().set_selection(lume_core::Range::point(anchor));
        assert!(app.workspace.get(doc).is_none());
        assert_eq!(app.git_root_epoch, epoch);
        assert_eq!(app.scritture.guarda(None).epoca, file_epoch);
        let old_second = old.remove(0);
        risultato74(&old_second, &target, TARGET, file_epoch, Some((epoch, doc, version)));
        app.notice = None; // non riutilizzare l'Info del primo reader rifiutato.
        let _ = app.update(old_second);
        assert_eq!(app.git_root_epoch, epoch);
        assert_eq!(app.scritture.guarda(None).epoca, file_epoch);
        assert!(app.workspace.get_by_path(&target).is_none(), "stesso path/revisione non rianima il DocId chiuso");
        assert_eq!(app.active_doc(), Some(reopened));
        assert_eq!(app.workspace.get(reopened).unwrap().selection().head, anchor);
        assert_eq!(app.notice.as_ref().unwrap().1.text, "Lettura LSP superata da una modifica o chiusura del documento sorgente: ripeti il salto");
        assert!(matches!(app.notice.as_ref().unwrap().1.kind, NoticeKind::Info));
        // Una nuova domanda della sorgente nuova resta funzionante.
        let answer = definizione_reale(&mut app, &mut stream, &target).await?;
        let task = app.update(answer);
        let message = lettura74(task, &mut read_maybe_started, &mut read_finished).await?;
        risultato74(&message, &target, TARGET, file_epoch, Some((epoch, reopened, version)));
        let _ = app.update(message);
        let opened = app.workspace.get_by_path(&target).unwrap();
        assert_eq!(app.active_doc(), Some(opened));
        assert_eq!(app.workspace.get(opened).unwrap().rope().to_string(), TARGET);
        assert!(!app.workspace.get(opened).unwrap().is_dirty());
        assert_eq!(app.workspace.get(opened).unwrap().selection().head,
            TARGET[..TARGET.find("dato").unwrap()].chars().count());
        assert_eq!(std::fs::read_to_string(&source)?, SOURCE);
        assert_eq!(std::fs::read_to_string(&target)?, TARGET);
        assert_eq!(std::fs::read_to_string(&other_path)?, OTHER);
        eprintln!("APP_REALE74:source_closed=true;same_path_version_reopened_new_doc=true;two_native_reader_old_rejected=true;fresh_origin_open=true");
        Ok(())
    }, "read74 scenario: actor sarà raccolto indipendentemente").await;
    let shutdown = limitato(
        async {
            let _ = app.stop_lsp();
            Ok(())
        },
        "read74 invio stop incerto",
    )
    .await;
    let stop = limitato(
        raccolta67(&mut app, &mut stream, &mut collected),
        "read74 raccolta actor incerta",
    )
    .await;
    drop(stream);
    if let Some(e) = shutdown.err().or(stop.err()) {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE74:fixture_preservata={}; actor incerto",
            residue.display()
        );
        return Err(e);
    }
    if read_maybe_started && !read_finished {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE74:fixture_preservata={}; reader incerto",
            residue.display()
        );
        scenario?;
        return Err(io::Error::other("reader forse avviato non raccolto").into());
    }
    scenario?;
    fixture.close()?;
    eprintln!(
        "APP_REALE74:sequenza_completa=native_definition_two_real_readers_source_close_reopen_path_version_aba_old_reject_fresh_open_exitstatus_drop"
    );
    Ok(())
}
