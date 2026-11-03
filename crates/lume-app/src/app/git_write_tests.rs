#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Task Iced/Tokio, file e Git veri; nessuna finestra nativa certificata.
use super::*;
use std::sync::atomic::AtomicBool;

fn git(dir: &Path, args: &[&str]) -> String {
    let result = lume_git::GitCli::locate()
        .unwrap()
        .run(dir, args, &AtomicBool::new(false))
        .unwrap();
    assert!(result.success(), "{}", result.failure_text());
    result.stdout.trim().to_owned()
}
fn fixture() -> (tempfile::TempDir, Lume, DocId, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Lume Test"]);
    git(dir.path(), &["config", "user.email", "lume@example.test"]);
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "base\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-m", "base"]);
    let (mut app, _) = Lume::new_without_store();
    app.set_project_root(dir.path().to_path_buf());
    app.open_file(&path).unwrap();
    let id = app.active_doc().unwrap();
    (dir, app, id, path)
}
fn dirty(app: &mut Lume, id: DocId) {
    let doc = app.workspace.get_mut(id).unwrap();
    doc.apply(Transaction::new(ChangeSet::insert_at(
        0,
        "X",
        doc.rope().len_chars(),
    )));
}

#[test]
fn rinomina_file_chiuso_attende_stage_git_reale_e_si_riprova() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    use crate::prompt::PromptMessage;
    let (dir, mut app, _, _) = fixture();
    let target = dir.path().join("chiuso.txt");
    let nuovo = dir.path().join("dopo.txt");
    std::fs::write(&target, "da indicizzare\n").unwrap();
    assert!(
        !app.workspace
            .iter()
            .any(|doc| doc.path() == Some(target.as_path()))
    );
    let task = app.run_git(GitOperation::Stage(vec![target.clone()]));
    let owner = app.git_operazione.expect("Stage ammesso");
    assert!(!owner.worktree);
    assert!(app.git_write.is_none());
    let vista = app.scritture.guarda(None);
    assert!(
        vista.in_volo.is_none(),
        "Stage usa ancora la guardia legacy"
    );

    drop(app.update(Message::Tool(
        crate::tools::project::ID,
        lume_ui::tool::ToolEvent::new(crate::tools::project::Event::RowRightClicked(
            target.clone(),
            false,
            10.0,
            20.0,
        )),
    )));
    drop(app.update(Message::MenuAction(crate::menu::MenuAct::RenameTreeFile)));
    drop(app.update(Message::AlberoRinomina(PromptMessage::Changed(
        "dopo.txt".into(),
    ))));
    drop(app.update(Message::AlberoRinomina(PromptMessage::Confirm)));
    assert_eq!(app.git_operazione.unwrap().ticket, owner.ticket);
    assert_eq!(app.scritture.guarda(None), vista);
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "da indicizzare\n"
    );
    assert!(!nuovo.exists());
    let richiesta = app.albero_rinomina.as_ref().unwrap();
    assert_eq!(richiesta.percorso, target);
    assert_eq!(richiesta.valore, "dopo.txt");
    assert!(richiesta.errore.is_none());
    assert!(app.albero_rinomina_prompt().unwrap().ready);
    assert!(app.notice.as_ref().unwrap().1.text.contains("Git"));

    app.completa_salvataggi(task);
    assert!(app.git_operazione.is_none());
    assert_eq!(git(dir.path(), &["show", ":chiuso.txt"]), "da indicizzare");
    assert!(app.albero_rinomina_prompt().unwrap().ready);
    drop(app.update(Message::AlberoRinomina(PromptMessage::Confirm)));
    assert!(!target.exists());
    assert_eq!(std::fs::read_to_string(&nuovo).unwrap(), "da indicizzare\n");
    assert_eq!(git(dir.path(), &["show", ":chiuso.txt"]), "da indicizzare");
    assert_eq!(app.scritture.guarda(None).epoca, vista.epoca + 2);
    assert!(app.scritture.guarda(None).in_volo.is_none());
    assert!(app.albero_rinomina.is_none());
}

/// Aspetta uno snapshot Git esplicito. `completa_salvataggi` si ferma quando
/// termina il writer; il refresh avviato in coda all'azione è invece un Task
/// ticketless e può ancora essere in volo a quel punto.
fn completa_refresh_git(app: &mut Lume) {
    use futures::StreamExt;
    let task = app.run_git(GitOperation::Refresh);
    let mut stream = iced_runtime::task::into_stream(task).expect("stream refresh Git");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime test");
    runtime.block_on(async {
        let action = stream.next().await.expect("risultato refresh Git");
        if let iced_runtime::Action::Output(message) = action {
            let _ = app.update(message);
        } else {
            panic!("refresh Git non ha prodotto un messaggio");
        }
    });
}

#[test]
fn azioni_git_che_riscrivono_worktree_rifiutano_buffer_sporchi() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, id, path) = fixture();
    dirty(&mut app, id);
    let original_file = std::fs::read(&path).unwrap();
    let original_index = std::fs::read(dir.path().join(".git/index")).unwrap();
    let original_head = git(dir.path(), &["rev-parse", "HEAD"]);
    let original_stash = git(dir.path(), &["stash", "list"]);
    let actions = [
        lume_ui::tool::GitAction::Pull,
        lume_ui::tool::GitAction::SwitchBranch("main".into()),
        lume_ui::tool::GitAction::SwitchRemoteBranch {
            remote: "origin".into(),
            name: "main".into(),
        },
        lume_ui::tool::GitAction::MergeUpstream,
        lume_ui::tool::GitAction::MergeContinue("merge".into()),
        lume_ui::tool::GitAction::MergeAbort,
        lume_ui::tool::GitAction::MergeBranch("main".into()),
        lume_ui::tool::GitAction::StashPush(None),
        lume_ui::tool::GitAction::StashApply(0),
        lume_ui::tool::GitAction::StashPop(0),
    ];

    for action in actions {
        let sequence = app.git_action_seq;
        let _task = app.run_git_action(action);
        assert!(app.git_operazione.is_none());
        assert!(!app.git_action_busy);
        assert_eq!(app.git_action_seq, sequence);
        assert!(app.workspace.get(id).unwrap().is_dirty());
        assert_eq!(std::fs::read(&path).unwrap(), original_file);
        assert_eq!(
            std::fs::read(dir.path().join(".git/index")).unwrap(),
            original_index
        );
        assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), original_head);
        assert_eq!(git(dir.path(), &["stash", "list"]), original_stash);
    }
}

#[test]
fn push_non_riscrive_worktree_e_resta_ammesso_con_buffer_sporco() {
    let (_dir, mut app, id, _path) = fixture();
    dirty(&mut app, id);

    let task = app.run_git_action(lume_ui::tool::GitAction::Push);

    assert!(app.git_action_busy);
    let ticket = app.git_operazione.unwrap().ticket;
    drop(task);
    let _completion = app.update(Message::GitWriteFinished(
        ticket,
        Box::new(GitWriteResult::Action(
            lume_ui::tool::GitAction::Push,
            crate::git_actions::ActionReport::warning("nessun upstream nella fixture"),
        )),
    ));
    assert!(!app.git_action_busy);
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[cfg(unix)]
#[test]
fn alias_sporchi_bloccano_tutte_le_mutazioni_worktree_prima_del_ticket() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    for symlink in [true, false] {
        let (dir, mut app, _, path) = fixture();
        let alias = dir.path().join("alias.txt");
        if symlink {
            std::os::unix::fs::symlink(&path, &alias).unwrap();
        } else {
            std::fs::hard_link(&path, &alias).unwrap();
        }
        app.open_file(&alias).unwrap();
        let id = app.active_doc().unwrap();
        dirty(&mut app, id);
        let index = std::fs::read(dir.path().join(".git/index")).unwrap();
        let head = git(dir.path(), &["rev-parse", "HEAD"]);
        let sequence = app.git_action_seq;
        for action in [
            lume_ui::tool::GitAction::Pull,
            lume_ui::tool::GitAction::SwitchBranch("main".into()),
            lume_ui::tool::GitAction::SwitchRemoteBranch {
                remote: "origin".into(),
                name: "main".into(),
            },
            lume_ui::tool::GitAction::MergeUpstream,
            lume_ui::tool::GitAction::MergeContinue("merge".into()),
            lume_ui::tool::GitAction::MergeAbort,
            lume_ui::tool::GitAction::MergeBranch("main".into()),
            lume_ui::tool::GitAction::StashPush(None),
            lume_ui::tool::GitAction::StashApply(0),
            lume_ui::tool::GitAction::StashPop(0),
        ] {
            drop(app.run_git_action(action));
            assert!(app.git_operazione.is_none());
            assert!(!app.git_action_busy);
            assert_eq!(app.git_action_seq, sequence);
            assert!(app.workspace.get(id).unwrap().is_dirty());
        }
        drop(app.run_git(GitOperation::Discard(vec![path.clone()])));
        assert!(app.git_operazione.is_none());
        assert_eq!(std::fs::read(&path).unwrap(), b"base\n");
        assert_eq!(std::fs::read(&alias).unwrap(), b"base\n");
        assert_eq!(std::fs::read(dir.path().join(".git/index")).unwrap(), index);
        assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
        assert_eq!(git(dir.path(), &["stash", "list"]), "");
        assert!(app.workspace.get(id).unwrap().is_dirty());
        assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "Xbase\n");
    }
}

#[test]
fn buffer_senza_nome_blocca_switch_branch() {
    let (_dir, mut app, _opened, _path) = fixture();
    let id = app.workspace.open(None, "bozza");
    dirty(&mut app, id);

    let _task = app.run_git_action(lume_ui::tool::GitAction::SwitchBranch("main".into()));

    assert!(app.git_operazione.is_none());
    assert!(!app.git_action_busy);
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[cfg(feature = "sql")]
#[test]
fn console_sql_sporca_non_blocca_switch_del_worktree() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, _, path) = fixture();
    git(dir.path(), &["branch", "feature"]);
    app.open_sql_console(None, None);
    let console = app.sql_console.unwrap();
    app.groups.activate(console);
    dirty(&mut app, console);
    let task = app.run_git_action(lume_ui::tool::GitAction::SwitchBranch("feature".into()));
    assert!(
        app.git_operazione.is_some(),
        "SQL dirty non è un buffer file"
    );
    app.completa_salvataggi(task);
    assert_eq!(git(dir.path(), &["branch", "--show-current"]), "feature");
    assert_eq!(std::fs::read(&path).unwrap(), b"base\n");
    std::fs::write(&path, "worktree modificato\n").unwrap();
    let discard = app.run_git(GitOperation::Discard(vec![path.clone()]));
    assert!(
        app.git_operazione.is_some(),
        "SQL dirty non blocca discard file"
    );
    app.completa_salvataggi(discard);
    assert_eq!(std::fs::read(&path).unwrap(), b"base\n");
    assert!(app.workspace.get(console).unwrap().is_dirty());
    assert_eq!(app.workspace.get(console).unwrap().rope().to_string(), "X");
}

#[test]
fn save_attivo_rifiuta_stage_discard_commit_e_stash_senza_io_git() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, id, path) = fixture();
    dirty(&mut app, id);
    let save = app.update(Message::SaveDocFixture(id));
    let index = std::fs::read(dir.path().join(".git/index")).unwrap();
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let stage = app.run_git(GitOperation::Stage(vec![path.clone()]));
    let discard = app.run_git(GitOperation::Discard(vec![path.clone()]));
    let commit = app.run_git(GitOperation::Commit {
        message: "non partire".into(),
        amend: false,
    });
    let stash = app.run_git_action(lume_ui::tool::GitAction::StashPush(None));
    let drop_stash = app.run_git_action(lume_ui::tool::GitAction::StashDrop(0));
    assert!(app.git_operazione.is_none());
    assert!(app.pending_commit_message.is_none());
    app.completa_salvataggi(Task::batch([
        stage, discard, commit, stash, drop_stash, save,
    ]));
    assert_eq!(std::fs::read(&path).unwrap(), b"Xbase\n");
    assert_eq!(std::fs::read(dir.path().join(".git/index")).unwrap(), index);
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(git(dir.path(), &["stash", "list"]), "");
}

#[test]
fn discard_rifiuta_buffer_sporco_senza_ticket_o_modifiche_repository() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, id, path) = fixture();
    dirty(&mut app, id);
    let original_file = std::fs::read(&path).unwrap();
    let original_index = std::fs::read(dir.path().join(".git/index")).unwrap();
    let original_head = git(dir.path(), &["rev-parse", "HEAD"]);

    let _task = app.run_git(GitOperation::Discard(vec![path.clone()]));

    assert!(app.git_operazione.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), original_file);
    assert_eq!(
        std::fs::read(dir.path().join(".git/index")).unwrap(),
        original_index
    );
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), original_head);
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn stage_unstage_e_commit_restano_ammessi_con_buffer_sporco() {
    for operation in [
        GitOperation::Stage(vec![PathBuf::from("a.txt")]),
        GitOperation::Unstage(vec![PathBuf::from("a.txt")]),
        GitOperation::Commit {
            message: "test".into(),
            amend: false,
        },
    ] {
        let (_dir, mut app, id, _path) = fixture();
        dirty(&mut app, id);

        let task = app.run_git(operation);

        assert!(app.git_operazione.is_some());
        assert!(app.git_write.is_none());
        assert!(app.workspace.get(id).unwrap().is_dirty());
        drop(task);
    }
}
#[test]
fn salva_attivo_durante_discard_rifiutato_senza_scrittura_ritardata() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (_dir, mut app, id, path) = fixture();
    std::fs::write(&path, "esterno\n").unwrap();
    let task = app.run_git(GitOperation::Discard(vec![path.clone()]));
    let lease = app.git_write.unwrap();
    let ticket = app.git_operazione.unwrap().ticket;
    let _ = app.update(Message::GitWriteFinished(
        ticket + 1,
        Box::new(GitWriteResult::Snapshot(Err("estraneo".into()), false)),
    ));
    assert_eq!(app.git_write, Some(lease));
    dirty(&mut app, id);
    let _ = app.update(Message::SaveActive);
    assert!(
        !(app.scritture.contiene_salvataggio(id) || app.saves.active_document() == Some(id)),
        "il salvataggio rifiutato non entra in coda"
    );
    assert!(app.notice.as_ref().unwrap().1.text.contains("Git"));
    app.completa_salvataggi(task);
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"base\n");
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn errore_discard_reale_rilascia_lease_worktree() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, _id, path) = fixture();
    let task = app.run_git(GitOperation::Discard(vec![path.clone()]));
    assert!(app.git_write.is_some());
    std::fs::remove_dir_all(dir.path().join(".git")).unwrap();
    app.completa_salvataggi(task);
    assert!(app.git_write.is_none());
    assert!(app.git_operazione.is_none());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("da verificare")
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"base\n");
}

#[test]
fn cambio_radice_durante_discard_preserva_lease_fino_al_risultato() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (_dir, mut app, _id, path) = fixture();
    std::fs::write(&path, "esterno\n").unwrap();
    let task = app.run_git(GitOperation::Discard(vec![path.clone()]));
    let lease = app.git_write.unwrap();
    let nuova = tempfile::tempdir().unwrap();
    app.set_project_root(nuova.path().to_path_buf());
    assert_eq!(app.git_write, Some(lease));
    assert!(app.begin_git_write().is_none());
    app.completa_salvataggi(task);
    assert!(app.git_write.is_none());
    assert!(app.git_operazione.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), b"base\n");
    assert!(app.branch.is_none());
}

#[test]
fn conferma_salva_in_chiusura_durante_git_non_accoda() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (_dir, mut app, id, path) = fixture();
    std::fs::write(&path, "esterno\n").unwrap();
    let task = app.run_git(GitOperation::Discard(vec![path.clone()]));
    dirty(&mut app, id);
    app.close_tab(id);
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::Salva,
    ));
    assert!(!(app.scritture.contiene_salvataggio(id) || app.saves.active_document() == Some(id)));
    assert!(app.notice.as_ref().unwrap().1.text.contains("Git"));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(&path).unwrap(), b"base\n");
    assert!(app.groups.contains(id));
    assert!(app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn edit_dopo_ammissione_switch_preserva_buffer_e_nuovo_worktree() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, id, path) = fixture();
    git(dir.path(), &["switch", "-c", "feature"]);
    std::fs::write(&path, "feature\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-m", "feature"]);
    let feature_commit = git(dir.path(), &["rev-parse", "HEAD"]);
    git(dir.path(), &["switch", "main"]);

    assert!(!app.workspace.get(id).unwrap().is_dirty());
    let task = app.run_git_action(lume_ui::tool::GitAction::SwitchBranch("feature".into()));
    assert!(
        app.git_operazione.is_some(),
        "switch ammesso prima dell'edit"
    );
    // Task ancora fredda: ordine deterministico, senza attese temporali.
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "Xbase\n");
    app.completa_salvataggi(task);
    completa_refresh_git(&mut app);

    let document = app.workspace.get(id).unwrap();
    assert!(document.is_dirty());
    assert_eq!(document.rope().to_string(), "Xbase\n");
    assert_eq!(std::fs::read(&path).unwrap(), b"feature\n");
    assert_eq!(git(dir.path(), &["show", ":a.txt"]), "feature");
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), feature_commit);
    assert_eq!(app.branch.as_deref(), Some("feature"));
    assert!(app.git_operazione.is_none());
    assert!(
        !(app.scritture.guarda(None).in_volo == Some(crate::scritture::Tipo::Salvataggio)
            || app.saves.active_document().is_some())
    );
}

#[test]
fn edit_buffer_durante_mutazione_git_resta_dirty_e_non_viene_salvato() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (_dir, mut app, id, path) = fixture();
    std::fs::write(&path, "disco\n").unwrap();
    dirty(&mut app, id);
    let task = app.run_git(GitOperation::Stage(vec![path.clone()]));
    assert!(app.git_operazione.is_some());

    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("Y".into()),
    ));
    let document = app.workspace.get(id).unwrap();
    assert!(document.is_dirty());
    assert_eq!(document.rope().to_string(), "XYbase\n");

    app.completa_salvataggi(task);

    let document = app.workspace.get(id).unwrap();
    assert!(document.is_dirty());
    assert_eq!(document.rope().to_string(), "XYbase\n");
    assert_eq!(std::fs::read(&path).unwrap(), b"disco\n");
    assert_eq!(git(_dir.path(), &["show", ":a.txt"]), "disco");
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edit_durante_switch_con_hook_vivo_preserva_buffer_e_disco() {
    use futures::StreamExt;
    use std::{io::Write, os::unix::fs::PermissionsExt, time::Duration};
    struct Release(std::fs::File);
    impl Drop for Release {
        fn drop(&mut self) {
            let _ = self.0.write_all(b"release\n");
        }
    }
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, id, path) = fixture();
    git(dir.path(), &["switch", "-c", "feature"]);
    std::fs::write(&path, "feature\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-m", "feature"]);
    git(dir.path(), &["switch", "main"]);
    let fifo = dir.path().join(".git/lume-test-release");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let release = Release(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo)
            .unwrap(),
    );
    let hook = dir.path().join(".git/hooks/post-checkout");
    std::fs::write(&hook, "#!/bin/sh\nprintf '%s\\n' \"$$\" > .git/lume-test-ready\nread -r released < .git/lume-test-release\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    let task = app.run_git_action(lume_ui::tool::GitAction::SwitchBranch("feature".into()));
    let mut stream = iced_runtime::task::into_stream(task).unwrap();
    let mut first = Box::pin(stream.next());
    assert!(futures::poll!(first.as_mut()).is_pending());
    let ready = dir.path().join(".git/lume-test-ready");
    let pid = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::select! {
            _ = first.as_mut() => panic!("switch completato prima del segnale hook"),
            pid = async {
                loop {
                    if let Ok(text) = std::fs::read_to_string(&ready)
                        && let Ok(pid) = text.trim().parse::<u32>() {
                        break pid;
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            } => pid,
        }
    })
    .await
    .unwrap();
    assert!(
        PathBuf::from(format!("/proc/{pid}/stat")).is_file(),
        "hook reale ancora vivo"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"feature\n");
    assert!(app.git_operazione.is_some());
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "Xbase\n");
    drop(release);
    let output = tokio::time::timeout(Duration::from_secs(15), first)
        .await
        .unwrap()
        .unwrap();
    let iced_runtime::Action::Output(message) = output else {
        panic!("completion Git attesa")
    };
    let _ = app.update(message);
    assert!(stream.next().await.is_none());
    assert!(app.git_operazione.is_none());
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "Xbase\n");
    assert_eq!(std::fs::read(&path).unwrap(), b"feature\n");
    assert_eq!(git(dir.path(), &["show", ":a.txt"]), "feature");
    assert_eq!(git(dir.path(), &["branch", "--show-current"]), "feature");
    assert!(
        !PathBuf::from(format!("/proc/{pid}/stat")).exists(),
        "hook raccolto"
    );
}

#[test]
fn non_salvare_aspetta_discard_gia_chiesto_e_revoca_il_save_pendente() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (_dir, mut app, id, path) = fixture();
    std::fs::write(&path, "esterno\n").unwrap();
    let task = app.run_git(GitOperation::Discard(vec![path.clone()]));
    dirty(&mut app, id);
    let _ = app.update(Message::SaveDocFixture(id));
    app.close_tab(id);
    let _ = app.rispondi_chiusura(crate::conferma_chiusura::ConfermaMessage::NonSalvare);
    assert!(app.groups.contains(id));
    assert!(app.git_operazione.is_some());
    app.completa_salvataggi(task);
    assert!(!app.groups.contains(id));
    assert!(app.git_operazione.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), b"base\n");
}
#[test]
fn commit_del_progetto_precedente_non_aggiunge_storia_o_snapshot_al_nuovo() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, _id, path) = fixture();
    std::fs::write(&path, "commit\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    let before = app.history.len();
    let task = app.run_git(GitOperation::Commit {
        message: "vecchio progetto".into(),
        amend: false,
    });
    let next = tempfile::tempdir().unwrap();
    app.set_project_root(next.path().to_path_buf());
    app.completa_salvataggi(task);
    assert_eq!(
        git(dir.path(), &["log", "-1", "--format=%s"]),
        "vecchio progetto"
    );
    assert_eq!(app.history.len(), before);
    assert!(app.branch.is_none());
    assert!(app.pending_commit_message.is_none());
    assert!(app.git_operazione.is_none());
}
#[test]
fn refresh_durante_commit_non_perde_storia_e_non_prevale_sul_post_mutazione() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, _id, path) = fixture();
    std::fs::write(&path, "commit\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    let before = app.history.len();
    let task = app.run_git(GitOperation::Commit {
        message: "vero commit".into(),
        amend: false,
    });
    let refresh = app.run_git(GitOperation::Refresh);
    app.completa_salvataggi(Task::batch([refresh, task]));
    assert_eq!(app.history.len(), before + 1);
    assert_eq!(app.branch.as_deref(), Some("main"));
    assert!(app.pending_commit_message.is_none());
    assert!(app.git_operazione.is_none());
}
#[test]
fn stage_rimanda_autosave_reale_e_lo_riprende_con_testo_attuale() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, _, path) = fixture();
    let note_dir = tempfile::tempdir().unwrap();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(note_dir.path()).unwrap();
    app.finish_notes_load(generation, note_dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let note_path = loaded.path().to_path_buf();
    let id = app.open_loaded_note(loaded);
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("uno".into()),
    ));
    let versione = app.workspace.get(id).unwrap().version();
    std::fs::write(&path, "stage reale\n").unwrap();
    let stage = app.run_git(GitOperation::Stage(vec![path]));
    assert!(app.git_operazione.is_some());
    assert_eq!(app.scritture.guarda(None).in_volo, None);
    let _ = app.update(Message::NotesAutosaveDue(id, versione));
    assert!(app.scritture.contiene_salvataggio(id));
    assert!(app.notice.as_ref().unwrap().1.text.contains("rimandato"));
    let _ = app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert(" due".into()),
    ));
    let testo = app.workspace.get(id).unwrap().rope().to_string();
    app.completa_salvataggi(stage);
    assert!(app.notice.as_ref().unwrap().1.text.contains("ripartito"));
    assert_eq!(store.load(note_path).unwrap().note.markdown, testo);
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    drop(dir);
}

#[test]
fn errore_git_reale_rifiuta_save_utente_e_permette_un_nuovo_tentativo() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, id, path) = fixture();
    std::fs::remove_dir_all(dir.path().join(".git")).unwrap();
    dirty(&mut app, id);
    let task = app.run_git(GitOperation::Stage(vec![path.clone()]));
    let _ = app.update(Message::SaveDocFixture(id));
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("non è possibile")
    );
    app.completa_salvataggi(task);
    assert!(app.git_operazione.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), b"base\n");
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("da verificare")
    );
    let retry = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(retry);
    assert_eq!(std::fs::read(&path).unwrap(), b"Xbase\n");
}
#[test]
fn stash_mutatore_e_posseduto_e_una_seconda_scrittura_non_parte() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, id, path) = fixture();
    std::fs::write(&path, "modificato\n").unwrap();
    let task = app.run_git_action(lume_ui::tool::GitAction::StashPush(Some("reale".into())));
    let ticket = app.git_operazione.unwrap().ticket;
    let rejected = app.run_git_action(lume_ui::tool::GitAction::StashDrop(0));
    dirty(&mut app, id);
    let _ = app.update(Message::SaveDocFixture(id));
    assert_eq!(app.git_operazione.unwrap().ticket, ticket);
    app.completa_salvataggi(Task::batch([rejected, task]));
    assert!(git(dir.path(), &["stash", "list", "--format=%gs"]).ends_with("reale"));
    assert_eq!(git(dir.path(), &["show", "stash@{0}:a.txt"]), "modificato");
    assert_eq!(std::fs::read(&path).unwrap(), b"base\n");
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert!(app.git_operazione.is_none());
    let drop = app.run_git_action(lume_ui::tool::GitAction::StashDrop(0));
    assert!(app.git_operazione.is_some());
    app.completa_salvataggi(drop);
    assert_eq!(git(dir.path(), &["stash", "list"]), "");
}

#[test]
fn stash_apply_passa_dal_worker_git_write_e_aggiorna_il_worktree() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, _id, path) = fixture();
    std::fs::write(&path, "stash\n").unwrap();
    git(dir.path(), &["stash", "push", "-m", "wip"]);
    assert_eq!(std::fs::read(&path).unwrap(), b"base\n");

    let task = app.run_git_action(lume_ui::tool::GitAction::StashApply(0));
    assert!(app.git_operazione.is_some());
    assert!(app.git_action_cancel.is_some());
    app.completa_salvataggi(task);

    assert_eq!(std::fs::read(&path).unwrap(), b"stash\n");
    assert_eq!(git(dir.path(), &["stash", "list"]).lines().count(), 1);
    assert!(app.git_operazione.is_none());
    assert!(app.git_action_cancel.is_none());

    git(dir.path(), &["reset", "--hard", "HEAD"]);
    let task = app.run_git_action(lume_ui::tool::GitAction::StashPop(0));
    app.completa_salvataggi(task);
    assert_eq!(git(dir.path(), &["stash", "list"]), "");
    assert_eq!(std::fs::read(&path).unwrap(), b"stash\n");
}

#[test]
fn pull_locale_aggiorna_worktree_head_e_pannello_fuori_dalla_ui() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (local, mut app, _id, path) = fixture();
    let remote = tempfile::tempdir().unwrap();
    let bare = remote.path().join("remote.git");
    git(
        remote.path(),
        &[
            "init",
            "--bare",
            "--initial-branch=main",
            bare.to_str().unwrap(),
        ],
    );
    git(
        local.path(),
        &["remote", "add", "origin", bare.to_str().unwrap()],
    );
    git(local.path(), &["push", "-u", "origin", "main"]);
    let checkout = remote.path().join("checkout");
    git(
        remote.path(),
        &["clone", bare.to_str().unwrap(), checkout.to_str().unwrap()],
    );
    git(&checkout, &["config", "user.name", "Lume Test"]);
    git(&checkout, &["config", "user.email", "lume@example.test"]);
    std::fs::write(checkout.join("a.txt"), "dal remoto\n").unwrap();
    git(&checkout, &["commit", "-am", "aggiornamento remoto"]);
    git(&checkout, &["push"]);

    let task = app.run_git_action(lume_ui::tool::GitAction::Pull);
    assert!(app.git_operazione.is_some());
    assert!(app.git_action_cancel.is_some());
    app.completa_salvataggi(task);

    assert_eq!(std::fs::read(&path).unwrap(), b"dal remoto\n");
    assert_eq!(
        git(local.path(), &["log", "-1", "--format=%s"]),
        "aggiornamento remoto"
    );
    assert!(app.git_operazione.is_none());
    assert!(app.git_action_cancel.is_none());
}

#[test]
fn switch_e_merge_branch_reali_completano_con_buffer_pulito() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, id, path) = fixture();
    git(dir.path(), &["switch", "-c", "feature"]);
    std::fs::write(dir.path().join("feature.txt"), "feature\n").unwrap();
    git(dir.path(), &["add", "feature.txt"]);
    git(dir.path(), &["commit", "-m", "feature"]);
    let feature_commit = git(dir.path(), &["rev-parse", "HEAD"]);
    git(dir.path(), &["switch", "main"]);

    let switch = app.run_git_action(lume_ui::tool::GitAction::SwitchBranch("feature".into()));
    assert!(app.git_operazione.is_some());
    app.completa_salvataggi(switch);
    completa_refresh_git(&mut app);
    assert_eq!(git(dir.path(), &["branch", "--show-current"]), "feature");
    assert_eq!(app.branch.as_deref(), Some("feature"));
    assert!(!app.workspace.get(id).unwrap().is_dirty());

    let switch_back = app.run_git_action(lume_ui::tool::GitAction::SwitchBranch("main".into()));
    app.completa_salvataggi(switch_back);
    completa_refresh_git(&mut app);
    assert_eq!(app.branch.as_deref(), Some("main"));
    let merge = app.run_git_action(lume_ui::tool::GitAction::MergeBranch("feature".into()));
    assert!(app.git_operazione.is_some());
    app.completa_salvataggi(merge);
    completa_refresh_git(&mut app);

    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), feature_commit);
    assert_eq!(app.branch.as_deref(), Some("main"));
    assert_eq!(
        std::fs::read(dir.path().join("feature.txt")).unwrap(),
        b"feature\n"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"base\n");
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert!(app.git_operazione.is_none());
}

#[test]
fn merge_conflittuale_e_abort_reali_passano_dall_app_e_preservano_il_branch() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Lume Test"]);
    git(dir.path(), &["config", "user.email", "lume@example.test"]);
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "base\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-m", "base"]);
    git(dir.path(), &["switch", "-c", "feature"]);
    std::fs::write(&path, "feature\n").unwrap();
    git(dir.path(), &["commit", "-am", "feature"]);
    git(dir.path(), &["switch", "main"]);
    std::fs::write(&path, "main\n").unwrap();
    git(dir.path(), &["commit", "-am", "main"]);
    let main_commit = git(dir.path(), &["rev-parse", "HEAD"]);

    let (mut app, _) = Lume::new_without_store();
    app.set_project_root(dir.path().to_path_buf());
    app.open_file(&path).unwrap();
    let id = app.active_doc().unwrap();
    let merge = app.run_git_action(lume_ui::tool::GitAction::MergeBranch("feature".into()));
    assert!(app.git_operazione.is_some());
    app.completa_salvataggi(merge);
    completa_refresh_git(&mut app);

    let conflicted = std::fs::read_to_string(&path).unwrap();
    assert!(conflicted.contains("<<<<<<< HEAD"));
    assert!(conflicted.contains("=======\nfeature"));
    assert!(dir.path().join(".git/MERGE_HEAD").is_file());
    assert_eq!(git(dir.path(), &["branch", "--show-current"]), "main");
    assert_eq!(app.branch.as_deref(), Some("main"));
    assert!(!app.workspace.get(id).unwrap().is_dirty());

    let abort = app.run_git_action(lume_ui::tool::GitAction::MergeAbort);
    assert!(app.git_operazione.is_some());
    app.completa_salvataggi(abort);
    completa_refresh_git(&mut app);

    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), main_commit);
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
    assert_eq!(std::fs::read(&path).unwrap(), b"main\n");
    assert!(!dir.path().join(".git/MERGE_HEAD").exists());
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert!(app.git_operazione.is_none());
}

#[test]
fn merge_risolto_e_continue_reali_creano_commit_a_due_genitori() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Lume Test"]);
    git(dir.path(), &["config", "user.email", "lume@example.test"]);
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "base\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-m", "base"]);
    git(dir.path(), &["switch", "-c", "feature"]);
    std::fs::write(&path, "feature\n").unwrap();
    git(dir.path(), &["commit", "-am", "feature"]);
    let feature_commit = git(dir.path(), &["rev-parse", "HEAD"]);
    git(dir.path(), &["switch", "main"]);
    std::fs::write(&path, "main\n").unwrap();
    git(dir.path(), &["commit", "-am", "main"]);
    let main_commit = git(dir.path(), &["rev-parse", "HEAD"]);

    let (mut app, _) = Lume::new_without_store();
    app.set_project_root(dir.path().to_path_buf());
    app.open_file(&path).unwrap();
    let id = app.active_doc().unwrap();
    let merge = app.run_git_action(lume_ui::tool::GitAction::MergeBranch("feature".into()));
    app.completa_salvataggi(merge);
    completa_refresh_git(&mut app);
    assert!(dir.path().join(".git/MERGE_HEAD").is_file());

    std::fs::write(&path, "risolto dall'utente\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    let continue_merge = app.run_git_action(lume_ui::tool::GitAction::MergeContinue(
        "merge risolto".into(),
    ));
    assert!(app.git_operazione.is_some());
    app.completa_salvataggi(continue_merge);
    completa_refresh_git(&mut app);

    let parents = git(dir.path(), &["rev-list", "--parents", "-n", "1", "HEAD"]);
    let parents: Vec<_> = parents.split_whitespace().collect();
    assert_eq!(
        parents.len(),
        3,
        "il commit di merge deve avere due genitori"
    );
    assert_eq!(parents[1], main_commit);
    assert_eq!(parents[2], feature_commit);
    assert_eq!(
        git(dir.path(), &["show", "HEAD:a.txt"]),
        "risolto dall'utente"
    );
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
    assert!(!dir.path().join(".git/MERGE_HEAD").exists());
    assert_eq!(app.branch.as_deref(), Some("main"));
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert!(app.git_operazione.is_none());
}

#[test]
fn hunk_stage_e_unstage_usano_lo_stesso_writer_e_preservano_il_save_successivo() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, id, path) = fixture();
    std::fs::write(&path, "modificato\n").unwrap();
    let version = app.workspace.get(id).unwrap().version();
    let testo = crate::file_read::read_testo_capped(&path, crate::file_read::MAX_FILE_BYTES)
        .map(|testo| Ricaricato { testo, nota: None })
        .map_err(|error| error.to_string());
    let _ = app.doc_ricaricato(id, &path, version, testo);
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "modificato\n"
    );
    let repo = lume_git::GitRepository::discover(dir.path()).unwrap();
    let hunk = repo.diff_worktree_file(&path).unwrap().hunks[0].clone();
    let task = app.run_git(GitOperation::StageHunk {
        path: path.clone(),
        hunk,
    });
    assert!(app.git_operazione.is_some());
    dirty(&mut app, id);
    let _ = app.update(Message::SaveDocFixture(id));
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("non è possibile")
    );
    app.completa_salvataggi(task);
    assert_eq!(git(dir.path(), &["show", ":a.txt"]), "modificato");
    assert_eq!(std::fs::read(&path).unwrap(), b"modificato\n");
    let retry = app.update(Message::SaveDocFixture(id));
    app.completa_salvataggi(retry);
    assert_eq!(std::fs::read(&path).unwrap(), b"Xmodificato\n");
    let repo = lume_git::GitRepository::discover(dir.path()).unwrap();
    let hunk = repo.diff_index_file(&path).unwrap().hunks[0].clone();
    let task = app.run_git(GitOperation::UnstageHunk {
        path: path.clone(),
        hunk,
    });
    assert!(app.git_operazione.is_some());
    app.completa_salvataggi(task);
    assert_eq!(git(dir.path(), &["diff", "--cached", "--name-only"]), "");
    assert_eq!(std::fs::read(&path).unwrap(), b"Xmodificato\n");
}

#[test]
fn lettura_avviata_prima_di_git_non_si_lega_dopo_la_sua_completion() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (dir, mut app, _id, path) = fixture();
    let other = dir.path().join("other.txt");
    std::fs::write(&other, "vero\n").unwrap();
    let epoch = app.scritture.guarda(None).epoca;
    let reading =
        crate::file_read::leggi_per_apertura(other.clone(), crate::file_read::DopoApertura::Niente);
    let task = app.run_git(GitOperation::Stage(vec![path]));
    assert!(app.open_file(&other).is_err());
    app.completa_salvataggi(task);
    let _ = app.update(Message::FileReadFinished(epoch, None, reading));
    assert!(
        !app.workspace
            .ids()
            .any(|id| app.workspace.get(id).unwrap().path() == Some(other.as_path()))
    );
    app.open_file(&other).unwrap();
    assert_eq!(
        app.workspace.get(app.active_doc().unwrap()).unwrap().path(),
        Some(other.as_path())
    );
}

#[test]
fn lettura_dentro_stage_reale_e_superata_alla_fine_senza_lease_git() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (_dir, mut app, id, path) = fixture();
    let task = app.run_git(GitOperation::Stage(vec![path.clone()]));
    let owner = app.git_operazione.unwrap();
    assert!(!owner.worktree);
    assert!(app.git_write.is_none());
    assert_eq!(app.scritture.guarda(None).in_volo, None);
    let epoca = app.scritture.guarda(None).epoca;
    use futures::StreamExt;
    let read_task = app.open_file_task(path.clone(), crate::file_read::DopoApertura::Niente);
    let mut stream = iced_runtime::task::into_stream(read_task).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let iced_runtime::Action::Output(read) = runtime.block_on(stream.next()).unwrap() else {
        panic!("lettura reale assente");
    };
    app.completa_salvataggi(task);
    assert_eq!(app.scritture.guarda(None).epoca, epoca + 1);
    assert_eq!(app.scritture.guarda(None).in_volo, None);
    drop(app.update(read));
    assert_eq!(app.workspace.get(id).unwrap().path(), Some(path.as_path()));
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("Lettura superata")
    );
}

#[test]
fn commit_fallito_rilascia_richiesta_e_non_fabbrica_storia_di_successo() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let (_dir, mut app, _id, _path) = fixture();
    let before = app.history.len();
    let task = app.run_git(GitOperation::Commit {
        message: "senza stage".into(),
        amend: false,
    });
    assert!(app.pending_commit_message.is_some());
    app.completa_salvataggi(task);
    assert!(app.pending_commit_message.is_none());
    assert!(app.git_operazione.is_none());
    assert_eq!(app.history.len(), before);
    assert!(
        app.notice
            .as_ref()
            .unwrap()
            .1
            .text
            .contains("da verificare")
    );
}

fn errore_snapshot_git_con_autosave_e_chiusura(stage: bool, chiusura_prima: bool) {
    use futures::StreamExt;
    let (root, mut app, _, path) = fixture();
    let note_dir = tempfile::tempdir().unwrap();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(note_dir.path()).unwrap();
    app.finish_notes_load(generation, note_dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let note_path = loaded.path().to_path_buf();
    let id = app.open_loaded_note(loaded);
    // Discard rifiuta due volte lo stesso percorso; Stage rifiuta un path
    // esterno al repository. Entrambi passano dal backend gix reale.
    let op = if stage {
        GitOperation::Stage(vec![note_path.clone()])
    } else {
        GitOperation::Discard(vec![path.clone(), path.clone()])
    };
    let task_git = app.run_git(op);
    let owner = app.git_operazione.unwrap();
    assert_eq!(owner.worktree, !stage);
    drop(app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("testo".into()),
    )));
    let versione = app.workspace.get(id).unwrap().version();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let due = runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(app.notes_autosave_schedule()).unwrap();
        let iced_runtime::Action::Output(message) = stream.next().await.unwrap() else {
            panic!("timer reale")
        };
        message
    });
    assert!(matches!(due, Message::NotesAutosaveDue(doc, v) if doc == id && v == versione));
    drop(app.update(due));
    assert!(app.scritture.contiene_salvataggio(id));
    if chiusura_prima {
        drop(app.update(Message::WindowCloseRequested(iced::window::Id::unique())));
        assert!(app.chiusura_attiva().is_some());
    }
    let result = runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(task_git).unwrap();
        let iced_runtime::Action::Output(message) = stream.next().await.unwrap() else {
            panic!("ricevuta Git reale")
        };
        message
    });
    let Message::GitWriteFinished(ticket, ref esito) = result else {
        panic!("receipt Git")
    };
    assert_eq!(ticket, owner.ticket);
    let GitWriteResult::Snapshot(Err(errore), false) = &**esito else {
        panic!("Snapshot errore reale")
    };
    let errore = errore.clone();
    let replay = result.clone();
    let seq = app.notice_seq;
    let seguito = app.update(result);
    assert!(app.git_operazione.is_none());
    assert_eq!(app.saves.active_document(), Some(id));
    let (notice_id, notice) = app.notice.as_ref().unwrap();
    assert!(notice.text.contains(&errore));
    assert_eq!(
        notice
            .text
            .matches("Salvataggio automatico ripartito")
            .count(),
        1,
        "l'errore Snapshot deve comunicare anche la ripartenza"
    );
    assert_eq!(
        *notice_id,
        seq + 1,
        "la ripartenza non deve riemettere l'errore"
    );
    if chiusura_prima {
        assert!(
            app.chiusura_attiva()
                .unwrap()
                .errore
                .as_deref()
                .is_some_and(|e| e.contains(&errore))
        );
    } else {
        drop(app.update(Message::WindowCloseRequested(iced::window::Id::unique())));
        assert!(app.chiusura_attiva().is_some());
    }
    app.completa_salvataggi(seguito);
    assert_eq!(store.load(note_path).unwrap().note.markdown, "testo");
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "base\n");
    assert_eq!(app.chiusura_attiva().is_some(), chiusura_prima);
    let successor = app.run_git(GitOperation::Stage(vec![path]));
    let successor_ticket = app.git_operazione.unwrap().ticket;
    assert_ne!(successor_ticket, owner.ticket);
    let notice = app
        .notice
        .as_ref()
        .map(|(id, notice)| (*id, notice.text.clone()));
    drop(app.update(replay));
    assert_eq!(app.git_operazione.unwrap().ticket, successor_ticket);
    assert_eq!(
        app.notice
            .as_ref()
            .map(|(id, notice)| (*id, notice.text.clone())),
        notice
    );
    app.completa_salvataggi(successor);
    assert!(app.git_operazione.is_none());
    assert_eq!(git(root.path(), &["show", ":a.txt"]), "base");
}

#[test]
fn errore_snapshot_discard_annuncia_autosave_e_preserva_chiusura() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    for chiusura_prima in [true, false] {
        errore_snapshot_git_con_autosave_e_chiusura(false, chiusura_prima);
    }
}

#[test]
fn errore_snapshot_stage_annuncia_autosave_e_preserva_chiusura() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    for chiusura_prima in [true, false] {
        errore_snapshot_git_con_autosave_e_chiusura(true, chiusura_prima);
    }
}

fn errore_git_con_autosave_e_chiusura(chiudere: bool) {
    let (_dir, mut app, _, _) = fixture();
    let note_dir = tempfile::tempdir().unwrap();
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(note_dir.path()).unwrap();
    app.finish_notes_load(generation, note_dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let loaded = store.create("Nota").unwrap();
    let path = loaded.path().to_path_buf();
    let id = app.open_loaded_note(loaded);
    let altra = store.create("Altra").unwrap();
    let altro_path = altra.path().to_path_buf();
    let altro_id = app.open_loaded_note(altra);
    let git = app.run_git_action(lume_ui::tool::GitAction::SwitchBranch(
        "ramo-inesistente".into(),
    ));
    assert!(app.git_write.is_some());
    use futures::StreamExt;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for nota in [id, altro_id] {
        drop(app.activate_tab(nota));
        drop(app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert("testo".into()),
        )));
        let versione = app.workspace.get(nota).unwrap().version();
        let due = runtime.block_on(async {
            let mut timer = iced_runtime::task::into_stream(app.notes_autosave_schedule()).unwrap();
            let iced_runtime::Action::Output(message) = timer.next().await.unwrap() else {
                panic!("timer reale")
            };
            message
        });
        assert!(matches!(due, Message::NotesAutosaveDue(doc, v) if doc == nota && v == versione));
        drop(app.update(due));
        assert!(app.scritture.contiene_salvataggio(nota));
    }
    if chiudere {
        drop(app.update(Message::WindowCloseRequested(iced::window::Id::unique())));
        assert!(app.chiusura_attiva().is_some());
    }
    let result = runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(git).unwrap();
        let iced_runtime::Action::Output(message) = stream.next().await.unwrap() else {
            panic!("risultato Git")
        };
        message
    });
    let Message::GitWriteFinished(_, ref esito) = result else {
        panic!("ricevuta Git")
    };
    let GitWriteResult::Action(_, report) = &**esito else {
        panic!("report Git")
    };
    assert_eq!(report.kind, crate::git_actions::NoticeKind::Error);
    let errore = report.text.clone();
    let replay = result.clone();
    let seguito = app.update(result);
    let notice = &app.notice.as_ref().unwrap().1;
    assert!(
        notice.text.contains(&errore),
        "errore Git coperto dalla ripartenza: {}",
        notice.text
    );
    assert!(
        notice.text.contains("ripartito"),
        "ripartenza non comunicata: {}",
        notice.text
    );
    if chiudere {
        assert!(
            app.chiusura_attiva()
                .unwrap()
                .errore
                .as_deref()
                .is_some_and(|e| e.contains(&errore))
        );
    }
    app.completa_salvataggi(seguito);
    assert_eq!(store.load(path).unwrap().note.markdown, "testo");
    assert_eq!(store.load(altro_path).unwrap().note.markdown, "testo");
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert!(!app.workspace.get(altro_id).unwrap().is_dirty());
    if chiudere {
        assert!(
            app.chiusura_attiva().is_some(),
            "la finestra non deve uscire su errore Git"
        );
    }
    assert!(app.notice.as_ref().unwrap().1.text.contains(&errore));
    if !chiudere {
        let successore = app.run_git_action(lume_ui::tool::GitAction::SwitchBranch("main".into()));
        let ticket = app.git_operazione.unwrap().ticket;
        let notifica = app.notice.as_ref().unwrap().1.text.clone();
        drop(app.update(replay));
        assert_eq!(app.git_operazione.unwrap().ticket, ticket);
        assert_eq!(app.notice.as_ref().unwrap().1.text, notifica);
        app.completa_salvataggi(successore);
    }
}

#[test]
fn errore_git_e_autosave_ripartito_restano_visibili_con_chiusura() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    errore_git_con_autosave_e_chiusura(true);
}

#[test]
fn errore_git_e_autosave_ripartito_restano_visibili_senza_chiusura() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    errore_git_con_autosave_e_chiusura(false);
}

fn ripartenze_dopo_errore_git(
    radice_cambiata: bool,
    persistente: bool,
    controlla_id_iniziale: bool,
) {
    use futures::StreamExt;
    let (_root_a, mut app, _, _) = fixture();
    let root_b = tempfile::tempdir().unwrap();
    let note_dir = tempfile::tempdir().unwrap();
    let git = app.run_git_action(lume_ui::tool::GitAction::SwitchBranch(
        "ramo-inesistente".into(),
    ));
    let owner = app.git_operazione.unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let risultato = runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(git).unwrap();
        let iced_runtime::Action::Output(message) = stream.next().await.unwrap() else {
            panic!("risultato Git reale")
        };
        message
    });
    let Message::GitWriteFinished(ticket, ref esito) = risultato else {
        panic!("ricevuta Git")
    };
    assert_eq!(ticket, owner.ticket);
    let GitWriteResult::Action(_, report) = &**esito else {
        panic!("report Git")
    };
    assert_eq!(report.kind, crate::git_actions::NoticeKind::Error);
    let errore = report.text.clone();
    let replay = risultato.clone();
    if radice_cambiata {
        drop(app.update(Message::FolderPicked(Some(root_b.path().to_path_buf()))));
        assert_ne!(app.git_root_epoch, owner.root_epoch);
    }
    let generation = app.begin_notes_load().unwrap();
    let store = lume_notes::NoteStore::open(note_dir.path()).unwrap();
    app.finish_notes_load(generation, note_dir.path().to_path_buf(), Vec::new())
        .unwrap();
    let mut note = Vec::new();
    for titolo in ["Uno", "Due"] {
        let loaded = store.create(titolo).unwrap();
        let path = loaded.path().to_path_buf();
        let id = app.open_loaded_note(loaded);
        drop(app.update(Message::EditorIn(
            app.groups.focus(),
            EditorCommand::Insert(titolo.into()),
        )));
        let versione = app.workspace.get(id).unwrap().version();
        let due = runtime.block_on(async {
            let mut stream =
                iced_runtime::task::into_stream(app.notes_autosave_schedule()).unwrap();
            let iced_runtime::Action::Output(message) = stream.next().await.unwrap() else {
                panic!("timer reale")
            };
            message
        });
        assert!(matches!(due, Message::NotesAutosaveDue(doc, v) if doc == id && v == versione));
        drop(app.update(due));
        note.push((id, path, titolo));
    }
    let prima = app.notice_seq;
    let task = app.update(risultato);
    let (id_errore, notice) = app.notice.as_ref().unwrap();
    assert!(notice.text.contains(&errore));
    assert!(notice.text.contains("ripartito"));
    if radice_cambiata {
        assert!(notice.text.contains("Git precedente"));
    }
    if controlla_id_iniziale {
        assert_eq!(
            *id_errore,
            prima + 1,
            "l'errore originale non deve essere riemesso"
        );
    }
    if persistente {
        app.notify_persistente(NoticeKind::Error, "Errore precedente persistente".into());
    }
    let id_visibile = app.notice.as_ref().unwrap().0;
    let sequenza = app.notice_seq;
    drop(app.update(Message::WindowCloseRequested(iced::window::Id::unique())));
    assert!(app.chiusura_attiva().is_some());
    let mut primo_stream = iced_runtime::task::into_stream(task).unwrap();
    let secondo = runtime.block_on(async {
        loop {
            match primo_stream.next().await.unwrap() {
                iced_runtime::Action::Output(Message::SaveReady(ready)) => {
                    drop(app.update(Message::SaveReady(ready)));
                }
                iced_runtime::Action::Output(done @ Message::SaveCollected(_)) => {
                    break app.update(done);
                }
                iced_runtime::Action::Output(other) => {
                    drop(app.update(other));
                }
                _ => {}
            }
        }
    });
    assert_eq!(app.saves.active_document(), Some(note[1].0));
    let (id_seconda, notice) = app.notice.as_ref().unwrap();
    let id_seconda = *id_seconda;
    let sequenza_seconda = app.notice_seq;
    let persistente_seconda = notice.persistente;
    let suffissi = notice
        .text
        .matches("Salvataggio automatico ripartito")
        .count();
    app.completa_salvataggi(secondo);
    for (id, path, testo) in &note {
        assert_eq!(store.load(path).unwrap().note.markdown, *testo);
        assert!(!app.workspace.get(*id).unwrap().is_dirty());
    }
    assert!(
        app.chiusura_attiva().is_none(),
        "errore precedente alla richiesta non blocca la chiusura"
    );
    assert_eq!(
        id_seconda, id_visibile,
        "la seconda ripartenza non crea un nuovo errore"
    );
    assert_eq!(sequenza_seconda, sequenza);
    assert_eq!(persistente_seconda, persistente);
    assert_eq!(suffissi, 1);
    if !persistente {
        let scadenza = runtime.block_on(async {
            loop {
                if let iced_runtime::Action::Output(message @ Message::NoticeExpired(id)) =
                    primo_stream.next().await.unwrap()
                    && id == prima + 1
                {
                    break message;
                }
            }
        });
        drop(app.update(scadenza));
        assert!(
            app.notice.is_none(),
            "il timer originale deve ancora scadere"
        );
    }
    let successore = app.run_git_action(lume_ui::tool::GitAction::SwitchBranch("main".into()));
    let successore_ticket = app.git_operazione.unwrap().ticket;
    drop(app.update(replay));
    assert_eq!(app.git_operazione.unwrap().ticket, successore_ticket);
    app.completa_salvataggi(successore);
}

#[test]
fn errore_git_precedente_non_rinasce_alla_seconda_nota_ne_blocca_chiusura() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    ripartenze_dopo_errore_git(false, false, false);
}

#[test]
fn errore_git_vecchia_radice_conserva_provenienza_id_e_scadenza() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    ripartenze_dopo_errore_git(true, false, true);
}

#[test]
fn errore_persistente_precedente_resta_persistente_alla_seconda_ripartenza() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    ripartenze_dopo_errore_git(false, true, false);
}

#[test]
fn lettura_lazy_durante_switch_git_non_adotta_vecchi_byte_prima_della_ricevuta() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    use futures::StreamExt;
    use lume_store::SessionSnapshot;
    let (dir, _, _, active) = fixture();
    let lazy = dir.path().join("lazy.txt");
    std::fs::write(&lazy, "vecchio\n").unwrap();
    git(dir.path(), &["add", "lazy.txt"]);
    git(dir.path(), &["commit", "-m", "lazy"]);
    git(dir.path(), &["switch", "-c", "feature"]);
    std::fs::write(&lazy, "nuovo\n").unwrap();
    git(dir.path(), &["commit", "-am", "nuovo"]);
    git(dir.path(), &["switch", "main"]);
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(dir.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), lazy.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);
    let id = app.workspace.get_by_path(&lazy).unwrap();
    assert!(app.pending_restored_tabs.contains(id));
    let switch = app.run_git_action(lume_ui::tool::GitAction::SwitchBranch("feature".into()));
    assert!(app.git_write.is_some());
    let read = app.activate_tab(id);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let vecchio = if app.restored_tab_reads.contains_key(&id) {
        Some(runtime.block_on(async {
            let mut stream = iced_runtime::task::into_stream(read).unwrap();
            let iced_runtime::Action::Output(message) = stream.next().await.unwrap() else {
                panic!("lettura reale")
            };
            message
        }))
    } else {
        assert!(app.notice.as_ref().unwrap().1.text.contains("Git"));
        None
    };
    drop(app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("X".into()),
    )));
    let receipt = runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(switch).unwrap();
        let iced_runtime::Action::Output(message) = stream.next().await.unwrap() else {
            panic!("ricevuta Git")
        };
        message
    });
    assert_eq!(std::fs::read(&lazy).unwrap(), b"nuovo\n");
    if let Some(vecchio) = vecchio {
        drop(app.update(vecchio));
    }
    assert!(
        app.pending_restored_tabs.contains(id),
        "lettura vecchia adottata durante Git"
    );
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    let seguito = app.update(receipt);
    app.completa_salvataggi(seguito);
    let retry = app.activate_tab(id);
    runtime.block_on(async {
        let mut streams = futures::stream::SelectAll::new();
        streams.push(iced_runtime::task::into_stream(retry).unwrap());
        while app.pending_restored_tabs.contains(id) || app.restored_tab_commands.contains_key(&id)
        {
            let iced_runtime::Action::Output(message) = streams.next().await.unwrap() else {
                continue;
            };
            if let Some(next) = iced_runtime::task::into_stream(app.update(message)) {
                streams.push(next);
            }
        }
    });
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "Xnuovo\n"
    );
}

#[test]
fn lettura_lazy_precedente_a_git_non_riproduce_input_durante_il_worktree() {
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    use futures::StreamExt;
    use lume_store::SessionSnapshot;
    let (dir, _, _, active) = fixture();
    let lazy = dir.path().join("lazy.txt");
    std::fs::write(&lazy, "vecchio\n").unwrap();
    git(dir.path(), &["add", "lazy.txt"]);
    git(dir.path(), &["commit", "-m", "lazy"]);
    git(dir.path(), &["switch", "-c", "feature"]);
    std::fs::write(&lazy, "nuovo\n").unwrap();
    git(dir.path(), &["commit", "-am", "nuovo"]);
    git(dir.path(), &["switch", "main"]);
    let (mut app, _) = Lume::new_without_store();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.workspace_root = Some(dir.path().to_path_buf());
    snapshot.tabs = vec![active.clone(), lazy.clone()];
    snapshot.active_tab = Some(active);
    app.restore_snapshot(snapshot);
    let id = app.workspace.get_by_path(&lazy).unwrap();
    let read = app.activate_tab(id);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let old = runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(read).unwrap();
        let iced_runtime::Action::Output(message) = stream.next().await.unwrap() else {
            panic!("lettura reale")
        };
        message
    });
    let switch = app.run_git_action(lume_ui::tool::GitAction::SwitchBranch("feature".into()));
    assert!(app.git_write.is_some());
    drop(app.update(Message::EditorIn(
        app.groups.focus(),
        EditorCommand::Insert("X".into()),
    )));
    let receipt = runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(switch).unwrap();
        let iced_runtime::Action::Output(message) = stream.next().await.unwrap() else {
            panic!("ricevuta Git")
        };
        message
    });
    assert_eq!(std::fs::read(&lazy).unwrap(), b"nuovo\n");
    drop(app.update(old));
    assert!(app.pending_restored_tabs.contains(id));
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "");
    assert!(!app.workspace.get(id).unwrap().is_dirty());
    assert!(!app.restored_tab_reads.contains_key(&id));
    let seguito = app.update(receipt);
    app.completa_salvataggi(seguito);
    let retry = app.activate_tab(id);
    runtime.block_on(async {
        let mut streams = futures::stream::SelectAll::new();
        streams.push(iced_runtime::task::into_stream(retry).unwrap());
        while app.pending_restored_tabs.contains(id) || app.restored_tab_commands.contains_key(&id)
        {
            let iced_runtime::Action::Output(message) = streams.next().await.unwrap() else {
                continue;
            };
            if let Some(next) = iced_runtime::task::into_stream(app.update(message)) {
                streams.push(next);
            }
        }
    });
    assert_eq!(
        app.workspace.get(id).unwrap().rope().to_string(),
        "Xnuovo\n"
    );
}
