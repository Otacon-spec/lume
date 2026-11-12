//! Progress reali → Indexing/status DTO; End vecchio contro Begin successore.
//! Nessun download/server finto/API nuova; richieste server auto-reply non tap.
use super::root67::{Collection67, accensione, raccolta67};
use super::*;
use lsp_types::{ProgressParamsValue, WorkDoneProgress};
use std::collections::BTreeMap;

#[derive(Default)]
struct Oracle75 {
    active: BTreeMap<String, (String, Option<String>, Option<u32>)>,
    begins: usize,
    reports: usize,
    ends: usize,
}
impl Oracle75 {
    fn update(&mut self, params: &lsp_types::ProgressParams) {
        let key = match &params.token {
            lsp_types::NumberOrString::String(s) => s.clone(),
            lsp_types::NumberOrString::Number(n) => n.to_string(),
        };
        let ProgressParamsValue::WorkDone(value) = &params.value;
        match value {
            WorkDoneProgress::Begin(b) => {
                assert!(!b.title.is_empty(), "title dal Begin nativo");
                self.active
                    .insert(key, (b.title.clone(), b.message.clone(), b.percentage));
                self.begins += 1;
            }
            WorkDoneProgress::Report(r) => {
                self.reports += 1;
                let entry = self
                    .active
                    .get_mut(&key)
                    .expect("Report nativo segue Begin osservato");
                if let Some(message) = &r.message {
                    entry.1 = Some(message.clone());
                }
                if let Some(percent) = r.percentage {
                    entry.2 = Some(percent);
                }
            }
            WorkDoneProgress::End(_) => {
                assert!(
                    self.active.remove(&key).is_some(),
                    "End nativo del token aperto osservato"
                );
                self.ends += 1;
            }
        }
    }
    fn label(&self) -> Option<String> {
        let (title, message, percent) = self.active.values().next()?;
        let mut pieces = vec![title.clone()];
        if let Some(p) = percent {
            pieces[0].push_str(&format!(" {p}%"));
        }
        if let Some(m) = message.as_ref().filter(|s| !s.is_empty()) {
            pieces.push(m.clone());
        }
        Some(pieces.join(" · "))
    }
}
fn params75(message: &Message) -> Option<&lsp_types::ProgressParams> {
    match message {
        Message::Lsp(_, _, crate::lsp_mode::LspEvent::From(ActorEvent::Progress(p))) => Some(p),
        _ => None,
    }
}
fn apply75(app: &mut Lume, message: Message, oracle: &mut Oracle75) -> Esito {
    if let Some(params) = params75(&message) {
        oracle.update(params);
        let Message::Lsp(server, epoch, _) = &message else {
            unreachable!()
        };
        assert_eq!(app.lsp.only().unwrap().epoch, *epoch);
        assert_eq!(&app.lsp.only().unwrap().spec.id, server);
        let _ = app.update(message);
        let lsp = app.lsp.only().unwrap();
        assert_eq!(lsp.indexing.is_empty(), oracle.active.is_empty());
        assert_eq!(
            lsp.indexing.label(),
            oracle.label(),
            "campi nativi fedeli nel DTO Indexing"
        );
        if let Some(label) = oracle.label() {
            assert_eq!(
                crate::lsp_mode::lsp_status_text(Some(lsp)),
                Some(format!("{} · {label}", lsp.spec.name)),
                "status consumer mostra il progresso nativo"
            );
        } else {
            let crate::lsp_mode::LspState::Attivo { version, memory_mb } = &lsp.state else {
                panic!("progress nativo dopo Initialized")
            };
            let mut normal = lsp.spec.name.clone();
            if !version.is_empty() {
                normal.push_str(&format!(" {version}"));
            }
            if let Some(mb) = memory_mb {
                normal.push_str(&format!(" · {mb} MB"));
            }
            assert_eq!(
                crate::lsp_mode::lsp_status_text(Some(lsp)),
                Some(normal),
                "End nativo ripristina lo status senza progresso residuo"
            );
        }
        Ok(())
    } else {
        consegna(app, message)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "progress consumer reale opt-in: test-app-lsp-progress-real"]
async fn recipe_progress_native_status_end_old_epoch_successore_reali() -> Esito {
    let program = std::env::var_os("LUME_RUST_ANALYZER")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("binario reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("binario reale non assoluto/file").into());
    }
    eprintln!(
        "APP_REALE75:binario_sha256={}",
        crate::aggiornamento::esadecimale(&<sha2::Sha256 as sha2::Digest>::digest(std::fs::read(
            &program
        )?))
    );
    let fixture = tempfile::Builder::new()
        .prefix("lume-progress75-città spazio-")
        .tempdir()?;
    let roots = [fixture.path().join("A"), fixture.path().join("B")];
    for (n, root) in roots.iter().enumerate() {
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname=\"native_progress75_{n}\"\nversion=\"0.1.0\"\nedition=\"2024\"\n"
            ),
        )?;
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn città() { let _emoji = \"😀\"; }\n",
        )?;
    }
    let mut app = app();
    let _source_a = write_and_open(
        &mut app,
        &roots[0].join("src"),
        "lib.rs",
        "pub fn città() { let _emoji = \"😀\"; }\n",
    );
    app.project_root = Some(roots[0].clone());
    let epoch_a = accensione(&mut app, &roots[0], fixture.path(), &program);
    let mut a = recipe(&app);
    let mut b: Option<BoxStream<'static, Message>> = None;
    let mut collected_a = Collection67::default();
    let mut collected_b = Collection67::default();
    let mut oracle_a = Oracle75::default();
    let mut oracle_b = Oracle75::default();
    let scenario = limitato(async {
        let mut fetching_a = None;
        let mut fetching_reports_a = 0;
        let old_end = loop {
            let message = evento(&mut a).await?;
            let mut end = false;
            if let Some(p) = params75(&message) {
                let ProgressParamsValue::WorkDone(value) = &p.value;
                if matches!(value, WorkDoneProgress::Begin(begin) if begin.title == "Fetching") { fetching_a = Some(p.token.clone()); }
                if fetching_a.as_ref() == Some(&p.token) && matches!(value, WorkDoneProgress::Report(_)) { fetching_reports_a += 1; }
                end = fetching_a.as_ref() == Some(&p.token) && matches!(value, WorkDoneProgress::End(_));
            }
            let held = end.then(|| message.clone()); // autentico replay, non Progress DTO inventato.
            apply75(&mut app, message, &mut oracle_a)?;
            if let Some(held) = held { break held; }
        };
        assert!(fetching_a.is_some() && oracle_a.begins > 0 && oracle_a.ends > 0);
        // Root transition reale. Task Git/Store restano cold, niente download.
        let _ = app.update(Message::FolderPicked(Some(roots[1].clone())));
        let _ = write_and_open(&mut app, &roots[1].join("src"), "lib.rs", "pub fn città() { let _emoji = \"😀\"; }\n");
        let epoch_b = accensione(&mut app, &roots[1], fixture.path(), &program);
        assert!(epoch_b > epoch_a);
        b = Some(recipe(&app)); // owner conservato fuori dalle future cancellabili.
        let mut fetching_b = None;
        let mut fetching_reports_b = 0;
        loop {
            let message = evento(b.as_mut().unwrap()).await?;
            let begin = params75(&message).is_some_and(|p| {
                let ProgressParamsValue::WorkDone(value) = &p.value;
                if matches!(value, WorkDoneProgress::Begin(x) if x.title == "Fetching") {
                    fetching_b = Some(p.token.clone()); true
                } else { false }
            });
            apply75(&mut app, message, &mut oracle_b)?;
            if begin { break; }
        }
        assert_eq!(fetching_a, fetching_b, "stesso token nativo nei due progetti: End vecchio sarebbe distruttivo");
        let before = app.lsp.only().unwrap().indexing.clone();
        let status = crate::lsp_mode::lsp_status_text(app.lsp.only());
        assert!(!before.is_empty());
        let _ = app.update(old_end);
        assert_eq!(app.lsp.only().unwrap().indexing, before, "End autentico vecchio non cancella il lavoro successore");
        assert_eq!(crate::lsp_mode::lsp_status_text(app.lsp.only()), status);
        assert_eq!(app.lsp.only().unwrap().epoch, epoch_b);
        loop {
            let message = evento(b.as_mut().unwrap()).await?;
            let end = params75(&message).is_some_and(|p| {
                let ProgressParamsValue::WorkDone(value) = &p.value;
                if fetching_b.as_ref() == Some(&p.token) && matches!(value, WorkDoneProgress::Report(_)) { fetching_reports_b += 1; }
                fetching_b.as_ref() == Some(&p.token) && matches!(value, WorkDoneProgress::End(_))
            });
            apply75(&mut app, message, &mut oracle_b)?;
            if end { break; }
        }
        assert!(oracle_b.ends > 0);
        assert!(fetching_reports_a > 0 && fetching_reports_b > 0, "Report nativi dello specifico token Fetching in entrambi i progetti");
        // Ulteriori attività legittime (es. CrateGraph) non devono essere
        // cancellate dal test per fingere idle. Oracle/UI devono concordare.
        assert_eq!(app.lsp.only().unwrap().indexing.label(), oracle_b.label());
        for root in &roots { assert_eq!(std::fs::read_to_string(root.join("src/lib.rs"))?, "pub fn città() { let _emoji = \"😀\"; }\n"); }
        eprintln!("APP_REALE75:native_A_begin={};report={};end={};native_B_begin={};report={};end={};fetching_reports_A={fetching_reports_a};B={fetching_reports_b};old_same_token_end_rejected=true", oracle_a.begins, oracle_a.reports, oracle_a.ends, oracle_b.begins, oracle_b.reports, oracle_b.ends);
        Ok(())
    }, "progress75 scenario: due actor saranno raccolti indipendentemente").await;
    let shutdown = limitato(
        async {
            let _ = app.stop_lsp();
            Ok(())
        },
        "progress75 stop B incerto",
    )
    .await;
    let collect_a = limitato(
        raccolta67(&mut app, &mut a, &mut collected_a),
        "progress75 raccolta A incerta",
    )
    .await;
    let collect_b = if let Some(stream) = b.as_mut() {
        limitato(
            raccolta67(&mut app, stream, &mut collected_b),
            "progress75 raccolta B incerta",
        )
        .await
    } else {
        Ok(())
    };
    drop(a);
    drop(b);
    if let Some(e) = shutdown.err().or(collect_a.err()).or(collect_b.err()) {
        let residue = fixture.keep();
        eprintln!(
            "APP_REALE75:fixture_preservata={}; actor incerto",
            residue.display()
        );
        return Err(e);
    }
    scenario?;
    fixture.close()?;
    eprintln!(
        "APP_REALE75:sequenza_completa=actual_recipe_native_progress_dto_status_two_roots_same_token_old_end_rejected_native_new_end_two_exitstatus_drop"
    );
    Ok(())
}
