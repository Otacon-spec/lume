//! Answer consumato/reader autentico: edit e Undo ABA prima di update.
//! Non binding/focus/server/confinamento o lifecycle completo del reader.
use super::definition68::{SOURCE, TARGET, definizione_reale};
use super::root67::{Collection67, accensione, raccolta67};
use super::*;
use lume_ui::editor::EditorCommand;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "reader LSP revisione reale opt-in: test-app-lsp-read-revision-real"]
async fn recipe_definizione_reader_edit_undo_aba_rifiuta_risultato_reale() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("binario reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("binario reale non assoluto/file").into());
    }
    eprintln!(
        "APP_REALE71:binario_sha256={}",
        crate::aggiornamento::esadecimale(&<sha2::Sha256 as sha2::Digest>::digest(std::fs::read(
            &program
        )?))
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-read71-città spazio-")
        .tempdir()?;
    let root = fixture.path().join("progetto");
    let src = root.join("src");
    std::fs::create_dir_all(&src)?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"native_read71\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    )?;
    let target = src.join("città.rs");
    std::fs::write(&target, TARGET)?;
    let mut app = app();
    let source = write_and_open(&mut app, &src, "lib.rs", SOURCE);
    let doc = app.active_doc().unwrap();
    let group = app.groups.group_of(doc).unwrap();
    let anchor = SOURCE[..SOURCE.find("dato()").unwrap()].chars().count();
    app.workspace
        .get_mut(doc)
        .unwrap()
        .set_selection(lume_core::Range::point(anchor + 1));
    app.project_root = Some(root.clone());
    let _ = accensione(&mut app, &root, fixture.path(), &program);
    let mut stream = recipe(&app);
    let mut collected = Collection67::default();
    let mut read_maybe_started = false;
    let mut read_finished = false;
    let scenario = limitato(async {
        while !app.lsp.only().unwrap().can_define() {
            consegna(&mut app, evento(&mut stream).await?)?;
        }
        for undo_before_delivery in [false, true] {
            assert!(app.workspace.get_by_path(&target).is_none());
            let answer = definizione_reale(&mut app, &mut stream, &target).await?;
            let version = app.workspace.get(doc).unwrap().version();
            let task = app.update(answer);
            assert!(app.definition_wait.is_none(), "Answer già consumato");
            let epoch = app.git_root_epoch;
            let file_epoch = app.scritture.guarda(None).epoca;
            let mut reading = iced_runtime::task::into_stream(task).unwrap();
            read_maybe_started = true;
            read_finished = false;
            let action = tokio::time::timeout(ATTESA, reading.next()).await?.unwrap();
            let iced_runtime::Action::Output(message) = action else { return Err(io::Error::other("non output reader reale").into()); };
            let Message::FileReadFinished(delivery_epoch, _, result) = &message else { return Err(io::Error::other("non FileReadFinished reale").into()); };
            read_finished = true;
            assert_eq!(*delivery_epoch, file_epoch);
            assert!(matches!(result.dopo, crate::file_read::DopoApertura::PosizioneLsp { progetto, origine, .. } if progetto == epoch && origine == (doc, version)));
            assert_eq!(result.path, target);
            assert_eq!(&**result.esito.as_ref().unwrap(), TARGET);
            assert!(tokio::time::timeout(ATTESA, reading.next()).await?.is_none());
            // Il worker ha già finito; modifica reale, non envelope falsificato.
            let _ = app.update(Message::EditorIn(group, EditorCommand::Insert(" ".into())));
            assert!(app.workspace.get(doc).unwrap().version() > version);
            if undo_before_delivery {
                let _ = app.update(Message::EditorIn(group, EditorCommand::Undo));
                assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), SOURCE);
                assert!(!app.workspace.get(doc).unwrap().is_dirty());
                assert!(app.workspace.get(doc).unwrap().version() > version, "Undo non ripristina la revisione vecchia");
            }
            let caret = app.workspace.get(doc).unwrap().selection().head;
            assert_eq!(app.git_root_epoch, epoch, "guard root non spiega il rifiuto");
            assert_eq!(app.scritture.guarda(None).epoca, file_epoch, "guard file non spiega il rifiuto");
            assert!(app.lsp.only().unwrap().state.is_live());
            let _ = app.update(message);
            assert!(app.workspace.get_by_path(&target).is_none(), "reader LSP vecchio non deve aprire il target dopo edit sorgente");
            assert_eq!(app.active_doc(), Some(doc));
            assert_eq!(app.workspace.get(doc).unwrap().selection().head, caret);
            assert_eq!(app.notice.as_ref().unwrap().1.text, "Lettura LSP superata da una modifica o chiusura del documento sorgente: ripeti il salto");
            assert!(matches!(app.notice.as_ref().unwrap().1.kind, NoticeKind::Info));
            if !undo_before_delivery {
                let _ = app.update(Message::EditorIn(group, EditorCommand::Undo));
                assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), SOURCE);
            }
        }
        // Risposta nuova sulla revisione nuova: non bloccare i salti validi.
        let answer = definizione_reale(&mut app, &mut stream, &target).await?;
        let version = app.workspace.get(doc).unwrap().version();
        let epoch = app.git_root_epoch;
        let task = app.update(answer);
        let mut reading = iced_runtime::task::into_stream(task).unwrap();
        read_finished = false;
        let action = tokio::time::timeout(ATTESA, reading.next()).await?.unwrap();
        let iced_runtime::Action::Output(message) = action else { return Err(io::Error::other("non output reader fresco reale").into()); };
        let Message::FileReadFinished(_, _, result) = &message else { return Err(io::Error::other("non reader fresco reale").into()); };
        read_finished = true;
        assert!(matches!(result.dopo, crate::file_read::DopoApertura::PosizioneLsp { progetto, origine, .. } if progetto == epoch && origine == (doc, version)));
        assert_eq!(result.path, target);
        assert_eq!(&**result.esito.as_ref().unwrap(), TARGET);
        assert!(tokio::time::timeout(ATTESA, reading.next()).await?.is_none());
        let _ = app.update(message);
        let opened = app.workspace.get_by_path(&target).unwrap();
        assert_eq!(app.active_doc(), Some(opened));
        assert_eq!(app.workspace.get(opened).unwrap().rope().to_string(), TARGET);
        assert!(!app.workspace.get(opened).unwrap().is_dirty());
        assert_eq!(app.workspace.get(opened).unwrap().selection().head, TARGET[..TARGET.find("dato").unwrap()].chars().count());
        assert_eq!(std::fs::read_to_string(&source)?, SOURCE);
        assert_eq!(std::fs::read_to_string(&target)?, TARGET);
        Ok(())
    }, "read71 scenario: actor sarà raccolto").await;
    let shutdown = limitato(
        async {
            let _ = app.stop_lsp();
            Ok(())
        },
        "read71 invio stop incerto",
    )
    .await;
    let stop = limitato(
        raccolta67(&mut app, &mut stream, &mut collected),
        "read71 raccolta incerta",
    )
    .await;
    drop(stream);
    if let Some(e) = shutdown.err().or(stop.err()) {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE71:fixture_preservata={}; actor incerto",
            residue.display()
        );
        return Err(e);
    }
    if read_maybe_started && !read_finished {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE71:fixture_preservata={}; reader incerto",
            residue.display()
        );
        scenario?;
        return Err(io::Error::other("reader forse avviato non raccolto").into());
    }
    scenario?;
    fixture.close()?;
    eprintln!(
        "APP_REALE71:sequenza_completa=native_definition_real_read_edit_undo_aba_revoked_fresh_open_exitstatus_drop"
    );
    Ok(())
}
