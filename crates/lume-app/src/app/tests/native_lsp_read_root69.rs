//! Definition consumata/reader realmente raccolto: root ABA prima di update.
//! Non reader cancel/drop/full-lifecycle o confinamento filesystem.
use super::definition68::{SOURCE, TARGET, definizione_reale};
use super::root67::{Collection67, accensione, raccolta67};
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lettura LSP root reale opt-in: test-app-lsp-read-root-real"]
async fn recipe_definizione_lettura_root_aba_rifiuta_risultato_reale() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("binario reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("binario reale non assoluto/file").into());
    }
    eprintln!(
        "APP_REALE69:binario_sha256={:x}",
        <sha2::Sha256 as sha2::Digest>::digest(std::fs::read(&program)?)
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-read69-città spazio-")
        .tempdir()?;
    let root = fixture.path().join("prima");
    let other = fixture.path().join("seconda");
    let src = root.join("src");
    std::fs::create_dir(&root)?;
    std::fs::create_dir(&src)?;
    std::fs::create_dir(&other)?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"native_read69\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    )?;
    let target = src.join("città.rs");
    std::fs::write(&target, TARGET)?;
    let mut app = app();
    let source = write_and_open(&mut app, &src, "lib.rs", SOURCE);
    let doc = app.active_doc().unwrap();
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
    let scenario = limitato(
        async {
            while !app.lsp.only().unwrap().can_define() {
                consegna(&mut app, evento(&mut stream).await?)?;
            }
            let answer = definizione_reale(&mut app, &mut stream, &target).await?;
            let task = app.update(answer);
            assert!(app.definition_wait.is_none());
            assert!(app.workspace.get_by_path(&target).is_none());
            let epoch = app.git_root_epoch;
            let file_epoch = app.scritture.guarda(None).epoca;
            let mut reading = iced_runtime::task::into_stream(task).unwrap();
            read_maybe_started = true;
            let action = tokio::time::timeout(ATTESA, reading.next()).await?.unwrap();
            let iced_runtime::Action::Output(message) = action else {
                return Err(io::Error::other("non output reader reale").into());
            };
            let Message::FileReadFinished(delivery_epoch, _, result) = &message else {
                return Err(io::Error::other("non FileReadFinished reale").into());
            };
            read_finished = true;
            assert_eq!(*delivery_epoch, file_epoch);
            assert_eq!(result.path, target);
            assert_eq!(&**result.esito.as_ref().unwrap(), TARGET);
            assert!(
                tokio::time::timeout(ATTESA, reading.next())
                    .await?
                    .is_none()
            );
            // Risultato autentico held, NON envelope/lettura costruiti. ABA via
            // due update reali, stesso path finale non equivale a stesso scope.
            let _ = app.update(Message::FolderPicked(Some(other.clone())));
            let _ = app.update(Message::FolderPicked(Some(root.clone())));
            assert_eq!(app.project_root, Some(root.clone()));
            assert!(app.git_root_epoch > epoch);
            assert_eq!(
                app.scritture.guarda(None).epoca,
                file_epoch,
                "file epoch non spiega il rifiuto root"
            );
            assert!(app.lsp_spawns().is_empty());
            let _ = app.update(message);
            assert!(
                app.workspace.get_by_path(&target).is_none(),
                "lettura LSP vecchia non deve aprire il target dopo root ABA"
            );
            assert_eq!(app.active_doc(), Some(doc));
            assert_eq!(app.workspace.get(doc).unwrap().selection().head, anchor + 1);
            assert_eq!(
                app.notice.as_ref().unwrap().1.text,
                "Lettura LSP superata da un cambio progetto: ripeti il salto"
            );
            assert!(matches!(
                app.notice.as_ref().unwrap().1.kind,
                NoticeKind::Info
            ));
            // Apertura manuale invariata: nuovo Task reale senza origine LSP.
            let manual = app.open_file_task(target.clone(), crate::file_read::DopoApertura::Niente);
            let mut manual = iced_runtime::task::into_stream(manual).unwrap();
            read_finished = false;
            let action = tokio::time::timeout(ATTESA, manual.next()).await?.unwrap();
            let iced_runtime::Action::Output(message) = action else {
                return Err(io::Error::other("non output reader manuale reale").into());
            };
            let Message::FileReadFinished(_, _, result) = &message else {
                return Err(io::Error::other("non FileReadFinished manuale reale").into());
            };
            read_finished = true;
            assert_eq!(result.path, target);
            assert!(matches!(
                result.dopo,
                crate::file_read::DopoApertura::Niente
            ));
            let _ = app.update(message);
            assert!(tokio::time::timeout(ATTESA, manual.next()).await?.is_none());
            let opened = app.workspace.get_by_path(&target).unwrap();
            assert_eq!(app.active_doc(), Some(opened));
            assert_eq!(
                app.workspace.get(opened).unwrap().rope().to_string(),
                TARGET
            );
            assert!(!app.workspace.get(opened).unwrap().is_dirty());
            assert_eq!(std::fs::read_to_string(&source)?, SOURCE);
            assert_eq!(std::fs::read_to_string(&target)?, TARGET);
            eprintln!(
                "APP_REALE69:root_epoch={epoch}->{}; reader già raccolto prima ABA",
                app.git_root_epoch
            );
            Ok(())
        },
        "read69 scenario: actor sarà raccolto",
    )
    .await;
    let shutdown = limitato(
        async {
            let _ = app.stop_lsp();
            Ok(())
        },
        "read69 invio stop incerto",
    )
    .await;
    let stop = limitato(
        raccolta67(&mut app, &mut stream, &mut collected),
        "read69 raccolta incerta",
    )
    .await;
    drop(stream);
    if let Some(e) = shutdown.err().or(stop.err()) {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE69:fixture_preservata={}; actor incerto",
            residue.display()
        );
        return Err(e);
    }
    if read_maybe_started && !read_finished {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE69:fixture_preservata={}; reader incerto",
            residue.display()
        );
        scenario?;
        return Err(io::Error::other("reader forse avviato non raccolto").into());
    }
    scenario?;
    fixture.close()?;
    eprintln!(
        "APP_REALE69:sequenza_completa=native_definition_real_read_held_root_aba_no_open_exitstatus_drop"
    );
    Ok(())
}
