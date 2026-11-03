//! Task blame dell'app con due commit reali; non renderer/GUI o lifecycle totale.
use super::*;
use iced::Task;
use lume_ui::editor::{EditorCommand, overlay::blame_author_on_line};
use lume_ui::tool::ToolRequest;
use std::path::Path;
const HEAD_TEXT: &str = "prima\nnuova\n";

fn git_fixture(program: &Path, root: &Path, author: &str, args: &[&str]) -> Esito {
    // CLI esplicita SOLO fixture in tests: non passa dalla policy produzione
    // e non la allenta. Tutti gli effetti confinati al repository posseduto.
    let mut hooks = std::ffi::OsString::from("core.hooksPath=");
    hooks.push(root.join("hooks-vuoti"));
    let status = std::process::Command::new(program)
        .arg("-c")
        .arg("gc.auto=0")
        .arg("-c")
        .arg("maintenance.auto=false")
        .arg("-c")
        .arg("commit.gpgSign=false")
        .arg("-c")
        .arg(hooks)
        .args(args)
        .current_dir(root)
        .env_clear()
        .env("HOME", root.join("home-privata"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join("config-vuota"))
        .env("GIT_AUTHOR_NAME", author)
        .env("GIT_COMMITTER_NAME", author)
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .env("GIT_AUTHOR_DATE", "2026-10-08T12:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2026-10-08T12:00:00+00:00")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(
            io::Error::other("Git fixture reale non riuscita, log grezzo non pubblicato").into(),
        );
    }
    Ok(())
}

async fn output_blame(task: Task<Message>) -> Esito<Message> {
    let mut stream = iced_runtime::task::into_stream(task)
        .ok_or_else(|| io::Error::other("nessun Task reale di blame"))?;
    let action = tokio::time::timeout(ATTESA, stream.next())
        .await?
        .ok_or_else(|| io::Error::other("Task blame senza output"))?;
    let iced_runtime::Action::Output(message @ Message::EditorBlameLoaded(..)) = action else {
        return Err(io::Error::other("output non EditorBlameLoaded reale").into());
    };
    if let Some(extra) = tokio::time::timeout(ATTESA, stream.next()).await? {
        let kind = match extra {
            iced_runtime::Action::Output(Message::EditorBlameLoaded(..)) => {
                "secondo EditorBlameLoaded"
            }
            iced_runtime::Action::Output(_) => "altro output app",
            iced_runtime::Action::Widget(_) => "azione widget",
            _ => "altra azione runtime",
        };
        return Err(io::Error::other(format!("Task blame con effetto ulteriore: {kind}")).into());
    }
    Ok(message)
}

fn check_output(
    app: &Lume,
    message: &Message,
    doc: lume_core::DocId,
    path: &Path,
    expected: &[Option<&str>],
) {
    let Message::EditorBlameLoaded(id, generation, actual_path, result) = message else {
        panic!("manca output blame autentico");
    };
    assert_eq!(*id, doc);
    assert_eq!(actual_path, path);
    assert_eq!(app.editor_blame_generation.get(&doc), Some(generation));
    let rows = result
        .as_ref()
        .unwrap_or_else(|_| panic!("blame reale fallito, errore grezzo non pubblicato"));
    assert_eq!(
        rows.iter()
            .map(|row| row.as_ref().map(|r| r.author_name.as_str()))
            .collect::<Vec<_>>(),
        expected
    );
}

fn authors(app: &Lume, doc: lume_core::DocId) -> Vec<Option<&str>> {
    let rows = app.editor_blame_cache.get(&doc).unwrap();
    (0..rows.len())
        .map(|line| blame_author_on_line(rows, line))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "blame app reale opt-in: test-app-blame-real"]
async fn task_blame_autori_sporco_edit_late_close_reali() -> Esito {
    let git = lume_git::GitCli::locate()
        .ok_or_else(|| io::Error::other("Git reale mancante, nessuno skip"))?;
    let fixture = tempfile::Builder::new()
        .prefix("lume-blame70-città spazio-")
        .tempdir()?;
    let root = fixture.path();
    let path = root.join("a.txt");
    let setup = (|| -> Esito {
        std::fs::create_dir(root.join("hooks-vuoti"))?;
        std::fs::create_dir(root.join("home-privata"))?;
        std::fs::write(root.join("config-vuota"), "")?;
        git_fixture(
            git.program(),
            root,
            "Ada",
            &["init", "--quiet", "--initial-branch=main"],
        )?;
        std::fs::write(&path, "prima\nseconda\n")?;
        git_fixture(git.program(), root, "Ada", &["add", "--", "a.txt"])?;
        git_fixture(
            git.program(),
            root,
            "Ada",
            &["commit", "--quiet", "--no-gpg-sign", "-m", "prima"],
        )?;
        std::fs::write(&path, HEAD_TEXT)?;
        git_fixture(git.program(), root, "Bruno", &["add", "--", "a.txt"])?;
        git_fixture(
            git.program(),
            root,
            "Bruno",
            &["commit", "--quiet", "--no-gpg-sign", "-m", "seconda"],
        )?;
        let repository = lume_git::GitRepository::discover(root)?;
        let backend = repository.blame(&path)?;
        assert_eq!(
            backend
                .iter()
                .map(|r| r.author_name.as_str())
                .collect::<Vec<_>>(),
            vec!["Ada", "Bruno"]
        );
        Ok(())
    })();
    if let Err(error) = setup {
        let residue = fixture.keep();
        eprintln!(
            "BLAME_REALE70:setup_fixture_preservata={}; CLI/setup incerto",
            residue.display()
        );
        return Err(error);
    }
    let mut app = app();
    // Cambio progetto reale; Task Git/Store restituiti qui non pollati.
    let _ = app.update(Message::FolderPicked(Some(root.to_path_buf())));
    assert!(
        app.git_available,
        "discovery CLI reale all'apertura del progetto"
    );
    app.open_file(&path).unwrap();
    let doc = app.active_doc().unwrap();
    let group = app.groups.group_of(doc).unwrap();
    assert!(
        app.editor_blame_available,
        "repo reale confermato dal consumer apertura"
    );
    let mut maybe_started = false;
    let mut collected = false;
    let scenario = limitato(
        async {
            let task = app.update(Message::MenuAction(crate::menu::MenuAct::ToggleEditorBlame));
            assert!(app.editor_blame_enabled);
            assert!(!app.editor_blame_cache.contains_key(&doc));
            assert_eq!(app.editor_blame_generation.len(), 1);
            maybe_started = true;
            let message = output_blame(task).await?;
            collected = true;
            let _ = app.update(message);
            assert_eq!(authors(&app, doc), vec![Some("Ada"), Some("Bruno")]);
            assert!(app.editor_blame_generation.is_empty());
            app.workspace
                .get_mut(doc)
                .unwrap()
                .set_selection(lume_core::Range::point(0));
            let _ = app.update(Message::EditorIn(
                group,
                EditorCommand::Insert("inserita\n".into()),
            ));
            assert!(
                !app.editor_blame_cache.contains_key(&doc),
                "edit invalida cache senza ricalcolo"
            );
            assert!(app.editor_blame_generation.is_empty());
            let generation = app.editor_blame_next_generation;
            let _ = app.update(Message::EditorIn(group, EditorCommand::SelectAll));
            assert_eq!(
                app.editor_blame_next_generation, generation,
                "altro input non avvia blame"
            );
            app.workspace
                .get_mut(doc)
                .unwrap()
                .set_selection(lume_core::Range::point(0));
            assert!(
                !app.pending_tool_requests
                    .iter()
                    .any(|(_, r)| matches!(r, ToolRequest::GitBlameFile(_)))
            );
            let _ = app.update(Message::MenuAction(crate::menu::MenuAct::ToggleEditorBlame));
            let task = app.update(Message::MenuAction(crate::menu::MenuAct::ToggleEditorBlame));
            collected = false;
            let message = output_blame(task).await?;
            collected = true;
            let _ = app.update(message);
            assert_eq!(authors(&app, doc), vec![None, Some("Ada"), Some("Bruno")]);
            // Nuova richiesta vera, output trattenuto: editing durante caricamento
            // deve revocare la generazione e non ristabilire attribuzioni vecchie.
            let _ = app.update(Message::MenuAction(crate::menu::MenuAct::ToggleEditorBlame));
            let task = app.update(Message::MenuAction(crate::menu::MenuAct::ToggleEditorBlame));
            collected = false;
            let late = output_blame(task).await?;
            collected = true;
            check_output(&app, &late, doc, &path, &[None, Some("Ada"), Some("Bruno")]);
            let _ = app.update(Message::EditorIn(
                group,
                EditorCommand::Insert("altra\n".into()),
            ));
            assert!(
                !app.editor_blame_generation.contains_key(&doc),
                "revoca prima della risposta tardiva"
            );
            let _ = app.update(late);
            assert!(
                !app.editor_blame_cache.contains_key(&doc),
                "blame autentico vecchio non ricrea cache dopo edit"
            );
            assert!(app.editor_blame_generation.is_empty());
            let _ = app.update(Message::EditorIn(group, EditorCommand::Undo));
            let _ = app.update(Message::EditorIn(group, EditorCommand::Undo));
            assert_eq!(
                app.workspace.get(doc).unwrap().rope().to_string(),
                HEAD_TEXT
            );
            assert!(!app.workspace.get(doc).unwrap().is_dirty());
            let _ = app.update(Message::MenuAction(crate::menu::MenuAct::ToggleEditorBlame));
            let task = app.update(Message::MenuAction(crate::menu::MenuAct::ToggleEditorBlame));
            collected = false;
            let late = output_blame(task).await?;
            collected = true;
            check_output(&app, &late, doc, &path, &[Some("Ada"), Some("Bruno")]);
            app.close_tab(doc);
            assert!(app.workspace.get(doc).is_none());
            let _ = app.update(late);
            assert!(!app.editor_blame_cache.contains_key(&doc));
            assert!(!app.editor_blame_generation.contains_key(&doc));
            assert_eq!(std::fs::read_to_string(&path)?, HEAD_TEXT);
            Ok(())
        },
        "blame70 scenario: reader forse avviato da verificare",
    )
    .await;
    if maybe_started && !collected {
        let residue = fixture.keep();
        eprintln!(
            "BLAME_REALE70:fixture_preservata={}; worker incerto",
            residue.display()
        );
        scenario?;
        return Err(io::Error::other("worker blame forse avviato non raccolto").into());
    }
    scenario?;
    fixture.close()?;
    eprintln!(
        "BLAME_REALE70:sequenza_completa=app_task_git_real_authors_edit_dirty_aligned_late_revoked_close"
    );
    Ok(())
}
