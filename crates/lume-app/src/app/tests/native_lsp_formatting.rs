//! Formattazione attraverso menu, recipe e rust-analyzer reali; nessun server finto.
use super::root67::{Collection67, accensione, raccolta67};
use super::*;
use lume_ui::editor::EditorCommand;

const GREZZO: &str = "pub fn numero()->u32{42}\n";
const FORMATTATO: &str = "pub fn numero() -> u32 {\n    42\n}\n";

async fn domanda(app: &mut Lume, stream: &mut BoxStream<'static, Message>) -> Esito<Message> {
    let _ = app.update(Message::MenuAction(crate::menu::MenuAct::FormatDocument));
    loop {
        let message = evento(stream).await?;
        if let Message::Lsp(_, _, crate::lsp_mode::LspEvent::From(ActorEvent::Answer(_, answer))) =
            &message
        {
            match &**answer {
                Ok(LspAnswer::Formatting {
                    edits: Some(edits), ..
                }) => {
                    assert!(!edits.is_empty(), "rustfmt reale deve cambiare la fixture");
                    return Ok(message);
                }
                Err(error) => return Err(io::Error::other(error.clone()).into()),
                _ => {}
            }
        }
        consegna(app, message)?;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "rust-analyzer reale opt-in: test-app-lsp-formatting-real"]
async fn formattazione_reale_menu_versione_stantia_e_altra_scheda() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("serve LUME_RUST_ANALYZER reale"))?;
    assert!(program.is_absolute() && program.is_file());
    let base = tempfile::tempdir()?;
    let root = base.path().join("progetto");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"prova_formatta\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    )?;
    let mut app = app();
    let path = write_and_open(&mut app, &root.join("src"), "lib.rs", GREZZO);
    let doc = app.active_doc().unwrap();
    app.project_root = Some(root.clone());
    accensione(&mut app, &root, base.path(), &program);
    let mut stream = recipe(&app);
    let mut collection = Collection67::default();
    let scenario = limitato(
        async {
            while !app.lsp.only().unwrap().can_format() {
                consegna(&mut app, evento(&mut stream).await?)?;
            }
            assert!(app.menu_checks().lsp_formatting);
            let risposta = domanda(&mut app, &mut stream).await?;
            let group = app.groups.focus();
            let _ = app.update(Message::EditorIn(
                group,
                EditorCommand::Insert("// cambiato\n".into()),
            ));
            let cambiato = app.workspace.get(doc).unwrap().rope().to_string();
            assert_ne!(cambiato, GREZZO);
            tokio::task::block_in_place(|| consegna(&mut app, risposta))?;
            assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), cambiato);
            assert!(app.notice.as_ref().unwrap().1.text.contains("scartata"));
            let _ = app.update(Message::EditorIn(group, EditorCommand::Undo));
            assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), GREZZO);
            let risposta = domanda(&mut app, &mut stream).await?;
            let other_path =
                write_and_open(&mut app, &root.join("src"), "altro.rs", "// altra scheda\n");
            let other = app.active_doc().unwrap();
            assert_ne!(other, doc);
            tokio::task::block_in_place(|| consegna(&mut app, risposta))?;
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                FORMATTATO
            );
            assert!(app.workspace.get(doc).unwrap().is_dirty());
            assert_eq!(
                std::fs::read_to_string(&path)?,
                GREZZO,
                "si salva solo con Ctrl+S"
            );
            assert_eq!(
                app.workspace.get(other).unwrap().rope().to_string(),
                "// altra scheda\n"
            );
            assert_eq!(std::fs::read_to_string(other_path)?, "// altra scheda\n");
            assert!(
                app.notice
                    .as_ref()
                    .unwrap()
                    .1
                    .text
                    .contains("Documento formattato")
            );
            Ok(())
        },
        "formattazione reale",
    )
    .await;
    let _ = app.apply_feature_command(crate::features::FeatureCommand::Stop(
        crate::features::Feature::LanguageServer,
    ));
    let cleanup = limitato(
        raccolta67(&mut app, &mut stream, &mut collection),
        "commiato formattazione",
    )
    .await;
    scenario?;
    cleanup?;
    eprintln!("FORMATTA_REALE:menu_stale_isolamento_dirty_disco_commiato=ok");
    Ok(())
}
