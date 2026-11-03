//! Firma da menu e digitazione, recipe e rust-analyzer reali; nessun server finto.
use super::root67::{Collection67, accensione, raccolta67};
use super::*;

const SORGENTE: &str = "pub fn somma(a: i32, b: i32) -> i32 { a + b }\npub fn usa() { let _emoji = \"😀\"; somma(1, 2); }\n";

async fn risposta_firma(
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
                Ok(LspAnswer::SignatureHelp(Some(help))) => {
                    assert!(help.signatures.iter().any(|s| s.label.contains("somma")));
                    return Ok(Some(message));
                }
                Ok(LspAnswer::SignatureHelp(None)) => {}
                Err(error) if error.starts_with("server error -32801:") => {}
                other => {
                    return Err(
                        io::Error::other(format!("risposta firma inattesa: {other:?}")).into(),
                    );
                }
            }
            consegna(app, message)?;
            return Ok(None);
        }
        consegna(app, message)?;
    }
}

pub(super) async fn firma_menu(
    app: &mut Lume,
    stream: &mut BoxStream<'static, Message>,
) -> Esito<Message> {
    tokio::time::timeout(ATTESA, async {
        let mut tentativi = 0;
        loop {
            let _ = app.update(Message::MenuAction(crate::menu::MenuAct::SignatureHelp));
            let request = app
                .signature_help_wait
                .as_ref()
                .expect("menu cablato")
                .request;
            tentativi += 1;
            if let Some(message) = risposta_firma(app, stream, request).await? {
                eprintln!("SIGNATURE_REALE:modello_semantico_pronto,tentativi={tentativi}");
                return Ok(message);
            }
            // Solo fixture attiva: initialize negozia le capability prima che
            // rust-analyzer abbia caricato il modello semantico del progetto.
            // None e ContentModified sono risposte transitorie ammesse dal protocollo.
            // Non introduce retry o polling nell'applicazione.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?
}

async fn firma_digitata(
    app: &mut Lume,
    stream: &mut BoxStream<'static, Message>,
    doc: lume_core::DocId,
    anchor: usize,
    trigger: &str,
) -> Esito<Message> {
    tokio::time::timeout(ATTESA, async {
        loop {
            app.workspace
                .get_mut(doc)
                .unwrap()
                .set_selection(lume_core::Range::point(anchor));
            let group = app.groups.focus();
            let _ = app.update(Message::EditorIn(group, EditorCommand::Backspace));
            let _ = app.update(Message::EditorIn(
                group,
                EditorCommand::Insert(trigger.into()),
            ));
            assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), SORGENTE);
            let wait = app.signature_help_wait.as_ref().expect("trigger cablato");
            assert_eq!(wait.anchor, anchor);
            if let Some(message) = risposta_firma(app, stream, wait.request).await? {
                return Ok(message);
            }
            // Solo fixture: ripete l'azione reale dopo ContentModified/None.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "rust-analyzer reale opt-in: test-app-lsp-signature-real"]
async fn firma_reale_menu_e_digitazione() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("serve LUME_RUST_ANALYZER reale"))?;
    assert!(program.is_absolute() && program.is_file());
    eprintln!(
        "SIGNATURE_REALE:binario_sha256={:x}",
        <sha2::Sha256 as sha2::Digest>::digest(std::fs::read(&program)?)
    );
    let base = tempfile::tempdir()?;
    let root = base.path().join("progetto città spazio");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"prova_firma\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    )?;
    let mut app = app();
    let path = write_and_open(&mut app, &root.join("src"), "lib.rs", SORGENTE);
    let doc = app.active_doc().unwrap();
    let anchor = SORGENTE[..SORGENTE.rfind("somma(").unwrap() + "somma(".len()]
        .chars()
        .count();
    app.workspace
        .get_mut(doc)
        .unwrap()
        .set_selection(lume_core::Range::point(anchor));
    app.project_root = Some(root.clone());
    accensione(&mut app, &root, base.path(), &program);
    let mut stream = recipe(&app);
    let mut collection = Collection67::default();
    let scenario = limitato(
        async {
            while !app.lsp.only().unwrap().can_signature_help() {
                consegna(&mut app, evento(&mut stream).await?)?;
            }
            assert!(app.menu_checks().lsp_signature_help);
            assert_eq!(
                app.lsp.only().unwrap().opened.get(&doc),
                crate::lsp_uri::path_to_uri(&path).as_ref()
            );
            let risposta = firma_menu(&mut app, &mut stream).await?;
            consegna(&mut app, risposta)?;
            let card = app.hover_card(doc).expect("firma pubblicata dall'app");
            assert_eq!(card.anchor, anchor);
            assert!(card.lines.iter().any(|line| line.text.contains("somma")));
            assert!(card.lines.iter().any(|line| line.text == "▶ a: i32"));
            eprintln!("SIGNATURE_REALE:menu_popup_primo_parametro=ok");

            let stantia = firma_menu(&mut app, &mut stream).await?;
            let group = app.groups.focus();
            let _ = app.update(Message::EditorIn(group, EditorCommand::Insert(" ".into())));
            consegna(&mut app, stantia)?;
            assert!(
                app.hover_card(doc).is_none(),
                "la vecchia risposta non riapre il popup"
            );
            let _ = app.update(Message::EditorIn(group, EditorCommand::Undo));
            assert_eq!(app.workspace.get(doc).unwrap().rope().to_string(), SORGENTE);
            eprintln!("SIGNATURE_REALE:risposta_stantia_scartata=ok");

            let risposta = firma_digitata(&mut app, &mut stream, doc, anchor, "(").await?;
            consegna(&mut app, risposta)?;
            assert!(
                app.hover_card(doc)
                    .unwrap()
                    .lines
                    .iter()
                    .any(|line| line.text == "▶ a: i32")
            );
            let comma = SORGENTE[..SORGENTE.rfind("somma(1,").unwrap() + "somma(1,".len()]
                .chars()
                .count();
            let risposta = firma_digitata(&mut app, &mut stream, doc, comma, ",").await?;
            consegna(&mut app, risposta)?;
            assert!(
                app.hover_card(doc)
                    .unwrap()
                    .lines
                    .iter()
                    .any(|line| line.text == "▶ b: i32")
            );
            assert_eq!(
                std::fs::read_to_string(&path)?,
                SORGENTE,
                "nessuna scrittura implicita"
            );
            eprintln!("SIGNATURE_REALE:trigger_parentesi_virgola_utf16_disco=ok");
            Ok(())
        },
        "firma reale nell'app",
    )
    .await;
    let _ = app.apply_feature_command(crate::features::FeatureCommand::Stop(
        crate::features::Feature::LanguageServer,
    ));
    let cleanup = limitato(
        raccolta67(&mut app, &mut stream, &mut collection),
        "commiato firma",
    )
    .await;
    scenario?;
    cleanup?;
    eprintln!("SIGNATURE_REALE:sequenza_completa=menu_trigger_parametri_stale_disco_commiato");
    Ok(())
}
