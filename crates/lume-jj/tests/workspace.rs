#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod common;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use lume_jj::{JjCommand, JjError, JjWorkspace, LOG_TEMPLATE, find_workspace_root};

const ENTRY: &str = r#"{"change":"kkmpptxz","commit":"3f2a91c0d4e5","description":"prima","author":"Rossi","when":"2026-09-29 10:15","bookmarks":"main","working_copy":true,"empty":false,"conflict":false,"immutable":false}"#;

fn workspace(fake: &common::Fake, root: &Path) -> JjWorkspace {
    JjWorkspace::new(fake.cli(), root.to_path_buf())
}

fn globali(resto: &[&str]) -> Vec<String> {
    ["--no-pager", "--color", "never"]
        .into_iter()
        .chain(resto.iter().copied())
        .map(str::to_owned)
        .collect()
}

#[test]
fn snapshot_lancia_log_e_poi_status() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    fake.script("log", &format!("{ENTRY}\n"), 0);
    fake.script("status", "Working copy changes:\nA a.txt\n", 0);
    let radice = tempfile::tempdir().unwrap();
    let no = AtomicBool::new(false);

    let snapshot = workspace(&fake, radice.path()).snapshot(30, &no).unwrap();

    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.entries[0].description, "prima");
    assert_eq!(snapshot.status, "Working copy changes:\nA a.txt");
    let calls = fake.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0].args,
        globali(&["log", "--no-graph", "-n", "30", "--template", LOG_TEMPLATE])
    );
    assert_eq!(calls[1].args, globali(&["status"]));
}

#[test]
fn i_comandi_girano_nella_radice_del_workspace() {
    let _serial = common::serial_process_test();
    // Monorepo: si apre `repo/servizi/api`, il `.jj` sta in `repo`.
    let fake = common::Fake::new();
    fake.script("log", "", 0);
    fake.script("status", "", 0);
    fake.script("git-push", "", 0);
    let base = tempfile::tempdir().unwrap();
    let repo = base.path().join("repo");
    let sotto = repo.join("servizi").join("api");
    std::fs::create_dir_all(repo.join(".jj")).unwrap();
    std::fs::create_dir_all(&sotto).unwrap();
    let no = AtomicBool::new(false);

    let radice = find_workspace_root(&sotto).unwrap();
    let ws = workspace(&fake, &radice);
    ws.snapshot(30, &no).unwrap();
    ws.run_command(&JjCommand::Push, &no).unwrap();

    let calls = fake.calls();
    assert_eq!(calls.len(), 3);
    for call in &calls {
        assert!(common::same_dir(&call.cwd, &repo), "{:?}", call.cwd);
    }
}

#[test]
fn un_log_fallito_non_lancia_lo_status() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    fake.script("log", "", 1);
    fake.script_err(
        "log",
        "Error: Failed to parse template: Keyword `foo` doesn't exist\n",
    );
    fake.script("status", "non deve essere chiamato", 0);
    let radice = tempfile::tempdir().unwrap();
    let no = AtomicBool::new(false);

    let errore = workspace(&fake, radice.path())
        .snapshot(30, &no)
        .unwrap_err();

    match errore {
        JjError::Failed(testo) => assert!(testo.contains("Failed to parse template"), "{testo}"),
        altro => panic!("atteso Failed, trovato {altro:?}"),
    }
    assert_eq!(fake.calls().len(), 1, "lo status non doveva partire");
}

#[test]
fn un_output_non_json_e_un_errore_di_parse() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    fake.script("log", "@  kkmpptxz rossi 2026-09-29\n", 0);
    let radice = tempfile::tempdir().unwrap();
    let no = AtomicBool::new(false);

    let errore = workspace(&fake, radice.path())
        .snapshot(30, &no)
        .unwrap_err();

    assert!(matches!(errore, JjError::Parse(_)), "{errore:?}");
    assert_eq!(fake.calls().len(), 1, "lo status non doveva partire");
}

#[test]
fn uno_status_fallito_e_un_errore() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    fake.script("log", &format!("{ENTRY}\n"), 0);
    fake.script("status", "", 1);
    fake.script_err("status", "Error: The working copy is stale\n");
    let radice = tempfile::tempdir().unwrap();
    let no = AtomicBool::new(false);

    let errore = workspace(&fake, radice.path())
        .snapshot(30, &no)
        .unwrap_err();

    assert!(
        errore.to_string().contains("working copy is stale"),
        "{errore}"
    );
}

#[test]
fn describe_passa_il_messaggio_come_un_solo_argomento() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    fake.script("describe", "", 0);
    let radice = tempfile::tempdir().unwrap();
    let ws = workspace(&fake, radice.path());
    let no = AtomicBool::new(false);
    let messaggi = [
        "--amend",
        "- voce\n- altra voce",
        "dice \"ciao\" e 'tu' con --message=x",
    ];

    for messaggio in messaggi {
        ws.run_command(&JjCommand::Describe(messaggio.to_owned()), &no)
            .unwrap();
    }

    let calls = fake.calls();
    assert_eq!(calls.len(), messaggi.len());
    for (call, messaggio) in calls.iter().zip(messaggi) {
        assert_eq!(
            call.args,
            globali(&["describe", &format!("--message={messaggio}")]),
            "il messaggio deve restare un solo argomento"
        );
    }
}

#[test]
fn describe_con_messaggio_vuoto_non_parte() {
    let fake = common::Fake::new();
    let radice = tempfile::tempdir().unwrap();
    let no = AtomicBool::new(false);

    let errore = workspace(&fake, radice.path())
        .run_command(&JjCommand::Describe("  \n ".to_owned()), &no)
        .unwrap_err();

    assert!(matches!(errore, JjError::Failed(_)), "{errore:?}");
    assert!(
        fake.calls().is_empty(),
        "nessun processo per un messaggio vuoto"
    );
}

#[test]
fn new_con_e_senza_messaggio() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    fake.script("new", "", 0);
    let radice = tempfile::tempdir().unwrap();
    let ws = workspace(&fake, radice.path());
    let no = AtomicBool::new(false);

    ws.run_command(&JjCommand::New(None), &no).unwrap();
    ws.run_command(&JjCommand::New(Some("  ".to_owned())), &no)
        .unwrap();
    ws.run_command(&JjCommand::New(Some("titolo".to_owned())), &no)
        .unwrap();

    let calls = fake.calls();
    assert_eq!(calls[0].args, globali(&["new"]));
    assert_eq!(
        calls[1].args,
        globali(&["new"]),
        "un messaggio vuoto non si passa"
    );
    assert_eq!(calls[2].args, globali(&["new", "--message=titolo"]));
}

#[test]
fn git_push_riporta_l_esito_di_jj() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    let radice = tempfile::tempdir().unwrap();
    let ws = workspace(&fake, radice.path());
    let no = AtomicBool::new(false);

    // Nulla da spingere: jj scrive su stderr ed esce con 0. Non è un errore.
    fake.script("git-push", "", 0);
    fake.script_err("git-push", "Nothing changed.\n");
    let testo = ws.run_command(&JjCommand::Push, &no).unwrap();
    assert_eq!(testo, "Nothing changed.");
    assert_eq!(fake.calls()[0].args, globali(&["git", "push"]));

    // Push rifiutato: il testo di jj è l'errore.
    fake.script("git-push", "", 1);
    fake.script_err("git-push", "Error: No git remote named 'origin'\n");
    let errore = ws.run_command(&JjCommand::Push, &no).unwrap_err();
    match errore {
        JjError::Failed(testo) => assert!(testo.contains("No git remote"), "{testo}"),
        altro => panic!("atteso Failed, trovato {altro:?}"),
    }
}

#[test]
fn un_comando_senza_output_risponde_fatto() {
    let _serial = common::serial_process_test();
    let fake = common::Fake::new();
    fake.script("new", "", 0);
    let radice = tempfile::tempdir().unwrap();
    let no = AtomicBool::new(false);

    let testo = workspace(&fake, radice.path())
        .run_command(&JjCommand::New(None), &no)
        .unwrap();

    assert_eq!(testo, "Fatto.");
}

#[test]
fn annullato_prima_di_partire_non_lancia_niente() {
    let fake = common::Fake::new();
    let radice = tempfile::tempdir().unwrap();
    let ws = workspace(&fake, radice.path());
    let si = AtomicBool::new(true);

    assert!(matches!(ws.snapshot(30, &si), Err(JjError::Cancelled)));
    assert!(matches!(
        ws.run_command(&JjCommand::Push, &si),
        Err(JjError::Cancelled)
    ));
    assert!(fake.calls().is_empty());
}
