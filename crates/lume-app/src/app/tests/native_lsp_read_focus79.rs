//! Reader Definition autentico completato → cambio scheda e focus A→B→A.
//! Non OS input, renderer, server diversi o cancellazione dell'IO.
use super::definition68::{SOURCE, TARGET, definizione_reale};
use super::read_close74::{lettura74, risultato74};
use super::root67::{Collection67, accensione, raccolta67};
use super::*;

fn scheda79(app: &mut Lume, doc: lume_core::DocId) {
    let group = app.groups.group_of(doc).unwrap();
    // Consumer reale della barra: press senza drag e release producono Activate.
    let _ = app.update(Message::EditorTabBar(
        crate::editor_tab_bar::EditorTabBarMessage::Press(group, doc),
    ));
    let _ = app.update(Message::EditorTabBar(
        crate::editor_tab_bar::EditorTabBarMessage::Release,
    ));
    assert_eq!(app.active_doc(), Some(doc));
}
fn rifiuto79(
    app: &mut Lume,
    message: Message,
    target: &std::path::Path,
    active: lume_core::DocId,
    caret: usize,
) {
    app.notice = None;
    let _ = app.update(message);
    assert!(
        app.workspace.get_by_path(target).is_none(),
        "reader Definition completato non deve rubare il focus dopo un cambio scheda"
    );
    assert_eq!(app.active_doc(), Some(active));
    assert_eq!(app.workspace.get(active).unwrap().selection().head, caret);
    assert_eq!(
        app.notice.as_ref().unwrap().1.text,
        "Lettura LSP superata da una nuova richiesta o cambio di scheda: ripeti il salto"
    );
    assert!(matches!(
        app.notice.as_ref().unwrap().1.kind,
        NoticeKind::Info
    ));
}

#[test]
fn epoca_definition_esaurita_resta_terminale_senza_wrap() {
    let mut app = app();
    app.definition_epoch = Some(u64::MAX - 1);
    app.definition_cancel();
    assert_eq!(app.definition_epoch, Some(u64::MAX));
    app.definition_cancel();
    assert_eq!(app.definition_epoch, None);
    app.definition_cancel();
    assert_eq!(app.definition_epoch, None);
}

#[test]
fn scheda_gia_attiva_non_revoca_epoca_definition() {
    let mut app = app();
    let doc = app.new_untitled().unwrap();
    assert_eq!(app.active_doc(), Some(doc));
    let before = app.definition_epoch;
    assert!(before.is_some());
    scheda79(&mut app, doc);
    assert_eq!(app.definition_epoch, before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "focus reader LSP reale opt-in: test-app-lsp-read-focus-real"]
async fn recipe_definizione_reader_cambio_scheda_focus_aba_rifiuta_reale() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("binario reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("binario reale non assoluto/file").into());
    }
    eprintln!(
        "APP_REALE79:binario_sha256={:x}",
        <sha2::Sha256 as sha2::Digest>::digest(std::fs::read(&program)?)
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-read79-città spazio-")
        .tempdir()?;
    let root = fixture.path().join("progetto");
    let src = root.join("src");
    std::fs::create_dir_all(&src)?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"native_read79\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    )?;
    let target = src.join("città.rs");
    std::fs::write(&target, TARGET)?;
    const OTHER: &str = "pub fn resta() { let _ = \"😀\"; }\n";
    let mut app = app();
    let other_path = write_and_open(&mut app, &src, "resta.rs", OTHER);
    let other = app.active_doc().unwrap();
    let other_caret = OTHER[..OTHER.find("resta").unwrap()].chars().count() + 2;
    app.workspace
        .get_mut(other)
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
        let root_epoch = app.git_root_epoch;
        let file_epoch = app.scritture.guarda(None).epoca;
        // Una domanda reale → Answer consumato → vero Task reader già finito.
        let answer = definizione_reale(&mut app, &mut stream, &target).await?;
        let task = app.update(answer);
        assert!(app.definition_wait.is_none());
        let first = lettura74(task, &mut read_maybe_started, &mut read_finished).await?;
        risultato74(&first, &target, TARGET, file_epoch, Some((root_epoch, doc, version)));
        assert!(app.workspace.get_by_path(&target).is_none());
        scheda79(&mut app, other);
        assert_eq!(app.workspace.get(doc).unwrap().version(), version);
        assert_eq!(app.git_root_epoch, root_epoch);
        assert_eq!(app.scritture.guarda(None).epoca, file_epoch);
        rifiuto79(&mut app, first, &target, other, other_caret);
        // Secondo reader indipendente; nessun clone del primo risultato.
        scheda79(&mut app, doc);
        let answer = definizione_reale(&mut app, &mut stream, &target).await?;
        let task = app.update(answer);
        let second = lettura74(task, &mut read_maybe_started, &mut read_finished).await?;
        risultato74(&second, &target, TARGET, file_epoch, Some((root_epoch, doc, version)));
        scheda79(&mut app, other);
        scheda79(&mut app, doc);
        assert_eq!(app.workspace.get(doc).unwrap().version(), version, "focus ABA non cambia revisione");
        assert_eq!(app.workspace.get(doc).unwrap().selection().head, anchor);
        assert_eq!(app.git_root_epoch, root_epoch);
        assert_eq!(app.scritture.guarda(None).epoca, file_epoch);
        rifiuto79(&mut app, second, &target, doc, anchor);
        // Il nuovo salto resta funzionante: origine/revisione uguali, ma
        // richiesta realmente successiva al cambio di focus A→B→A.
        let answer = definizione_reale(&mut app, &mut stream, &target).await?;
        let task = app.update(answer);
        let fresh = lettura74(task, &mut read_maybe_started, &mut read_finished).await?;
        risultato74(&fresh, &target, TARGET, file_epoch, Some((root_epoch, doc, version)));
        let _ = app.update(fresh);
        let opened = app.workspace.get_by_path(&target).unwrap();
        let expected = TARGET[..TARGET.find("dato").unwrap()].chars().count();
        assert_eq!(app.active_doc(), Some(opened));
        assert_eq!(app.workspace.get(opened).unwrap().selection().head, expected);
        assert_eq!(app.workspace.get(opened).unwrap().rope().to_string(), TARGET);
        assert!(!app.workspace.get(opened).unwrap().is_dirty());
        assert_eq!(std::fs::read_to_string(&source)?, SOURCE);
        assert_eq!(std::fs::read_to_string(&target)?, TARGET);
        assert_eq!(std::fs::read_to_string(&other_path)?, OTHER);
        eprintln!("APP_REALE79:completed_reader_focus_changed_rejected=true;focus_aba_same_doc_version_rejected=true;fresh_native_read_open=true");
        Ok(())
    }, "read79 scenario: actor sarà raccolto indipendentemente").await;
    let shutdown = limitato(
        async {
            let _ = app.stop_lsp();
            Ok(())
        },
        "read79 invio stop incerto",
    )
    .await;
    let actor = limitato(
        raccolta67(&mut app, &mut stream, &mut collected),
        "read79 raccolta actor incerta",
    )
    .await;
    drop(stream);
    if shutdown.is_err() || actor.is_err() || (read_maybe_started && !read_finished) {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE79:fixture_preservata={}; owner reader/actor incerto",
            residue.display()
        );
        return Err(io::Error::other("raccolta reader/actor non confermata").into());
    }
    if let Err(error) = scenario {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE79:fixture_preservata={}; scenario fallito, owner raccolti nei limiti dichiarati",
            residue.display()
        );
        return Err(error);
    }
    fixture.close()?;
    eprintln!(
        "APP_REALE79:sequenza_completa=native_definition_completed_read_tab_change_focus_aba_reject_fresh_open_exitstatus_drop"
    );
    Ok(())
}
