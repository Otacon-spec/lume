//! Due selezioni e risposte di rust-analyzer reale; cleanup del recipe condiviso.
use super::root67::{Collection67, accensione, raccolta67};
use super::*;
use lume_core::{Range, SelectionSet};
const SORGENTE: &str = "pub struct Dato { pub valore: i32 }\npub fn somma(a: i32, b: i32) -> i32 { a + b }\npub fn usa() { let dato = Dato { valore: 42 }; let _emoji = \"😀\"; dato.val; somma(1, 2); }\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "rust-analyzer reale: test-app-lsp-multi-real"]
async fn multicursore_lsp_reale_principale_completamento_e_stale() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("serve LUME_RUST_ANALYZER reale"))?;
    assert!(program.is_absolute() && program.is_file());
    eprintln!(
        "MULTICURSORE_LSP_REALE:binario_sha256={}",
        crate::aggiornamento::esadecimale(&<sha2::Sha256 as sha2::Digest>::digest(std::fs::read(
            &program
        )?))
    );
    let base = tempfile::tempdir()?;
    let root = base.path().join("progetto città spazio");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"prova_multicursore\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    )?;
    let mut app = app();
    let path = write_and_open(&mut app, &root.join("src"), "lib.rs", SORGENTE);
    let doc = app.active_doc().unwrap();
    let head = SORGENTE[..SORGENTE.rfind("val;").unwrap()].chars().count() + 3;
    let set = SelectionSet::try_new(
        vec![Range::point(0), Range::point(head)],
        1,
        SORGENTE.chars().count(),
    )
    .unwrap();
    app.workspace
        .get_mut(doc)
        .unwrap()
        .set_selections(set.clone());
    app.project_root = Some(root.clone());
    accensione(&mut app, &root, base.path(), &program);
    let mut stream = recipe(&app);
    let mut collection = Collection67::default();
    let scenario = limitato(
        async {
            while !app.lsp.only().unwrap().can_complete()
                || !app.lsp.only().unwrap().can_signature_help()
            {
                consegna(&mut app, evento(&mut stream).await?)?;
            }
            let answer = super::completion65::completamento_reale(&mut app, &mut stream).await?;
            consegna(&mut app, answer)?;
            let ui = app.completion.as_ref().unwrap();
            assert_eq!(ui.popup.anchor, head - 3);
            let index = ui
                .popup
                .rows
                .iter()
                .position(|row| row.label == "valore")
                .unwrap();
            let group = app.groups.focus();
            for _ in 0..index {
                let _ = app.update(Message::EditorIn(group, EditorCommand::PopupDown));
            }
            let _ = app.update(Message::EditorIn(group, EditorCommand::PopupAccept));
            let document = app.workspace.get(doc).unwrap();
            assert_eq!(
                document.rope().to_string(),
                SORGENTE.replace("dato.val;", "dato.valore;")
            );
            assert_eq!(
                document.selections().ranges(),
                &[Range::point(0), Range::point(head + 3)]
            );
            let _ = app.update(Message::EditorIn(group, EditorCommand::Undo));
            assert_eq!(app.workspace.get(doc).unwrap().selections(), &set);
            assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), SORGENTE);
            let firma = SORGENTE[..SORGENTE.rfind("somma(").unwrap() + 6]
                .chars()
                .count();
            crate::app::set_primary(app.workspace.get_mut(doc).unwrap(), Range::point(firma));
            let answer = super::signature::firma_menu(&mut app, &mut stream).await?;
            consegna(&mut app, answer)?;
            assert_eq!(app.hover_card(doc).unwrap().anchor, firma);
            assert_eq!(
                app.workspace.get(doc).unwrap().selections().ranges().len(),
                2
            );
            let stale = super::signature::firma_menu(&mut app, &mut stream).await?;
            let _ = app.update(Message::EditorIn(group, EditorCommand::Insert(" ".into())));
            consegna(&mut app, stale)?;
            assert!(app.hover_card(doc).is_none());
            let _ = app.update(Message::EditorIn(group, EditorCommand::Undo));
            assert_eq!(std::fs::read_to_string(&path)?, SORGENTE);
            Ok(())
        },
        "multicursore LSP",
    )
    .await;
    let _ = app.apply_feature_command(crate::features::FeatureCommand::Stop(
        crate::features::Feature::LanguageServer,
    ));
    let cleanup = limitato(
        raccolta67(&mut app, &mut stream, &mut collection),
        "commiato multicursore",
    )
    .await;
    scenario?;
    cleanup?;
    eprintln!("MULTICURSORE_LSP_REALE:completo");
    Ok(())
}
