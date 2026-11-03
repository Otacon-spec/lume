#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Prova con il `jj` vero. Ogni repository sta in una cartella temporanea e la
//! firma di prova sta nella sua configurazione (`--repo`): né il repository né la
//! configurazione di chi lancia i test vengono toccati. Questi test sono
//! opt-in: per eseguirli serve `jj` e, per il push, anche `git`.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use lume_jj::{JjCli, JjCommand, JjWorkspace};

mod common;

struct Prova {
    cli: JjCli,
    repo: tempfile::TempDir,
}

impl Prova {
    fn nuova() -> Self {
        let cli = JjCli::locate().expect("jj richiesto dal gate testkit");
        let repo = tempfile::tempdir().unwrap();
        let prova = Self { cli, repo };
        prova.jj(prova.repo.path(), &["git", "init"]);
        // La firma sta nella configurazione **del repository temporaneo**
        // (`--repo`): quella dell'utente non si tocca e non serve.
        prova.jj(
            prova.repo.path(),
            &["config", "set", "--repo", "user.name", "Lume Test"],
        );
        prova.jj(
            prova.repo.path(),
            &[
                "config",
                "set",
                "--repo",
                "user.email",
                "lume-test@example.invalid",
            ],
        );
        prova
    }

    /// Lancia `jj` direttamente (non passa da `JjCli`: prepara la scena, e
    /// deve funzionare anche se `JjCli` avesse un difetto). Fa fallire il test
    /// se `jj` esce con un errore.
    fn jj(&self, dir: &Path, args: &[&str]) -> String {
        let out = Command::new(self.cli.program())
            .args(["--no-pager", "--color", "never"])
            .args(args)
            .current_dir(dir)
            .env("JJ_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "jj {args:?} è fallito: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn workspace(&self) -> JjWorkspace {
        JjWorkspace::new(self.cli.clone(), self.repo.path().to_path_buf())
    }
}

#[test]
#[ignore = "richiede jj; esegui con --ignored esplicitamente"]
fn snapshot_reale_di_un_repository_nuovo() {
    let _serial = common::serial_process_test();
    lume_testkit::richiedi!(lume_testkit::Strumento::Jujutsu);
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let prova = Prova::nuova();
    std::fs::write(prova.repo.path().join("a.txt"), "uno\n").unwrap();
    let no = AtomicBool::new(false);

    let snapshot = prova.workspace().snapshot(30, &no).unwrap();

    assert!(snapshot.status.contains("a.txt"), "{}", snapshot.status);
    let corrente = &snapshot.entries[0];
    assert!(corrente.working_copy, "{corrente:?}");
    assert!(
        !corrente.empty,
        "a.txt è nella copia di lavoro: {corrente:?}"
    );
    assert_eq!(corrente.description, "");
    assert_eq!(corrente.author, "Lume Test");
    assert_eq!(corrente.change.len(), 8, "{corrente:?}");
    assert_eq!(corrente.commit.len(), 12, "{corrente:?}");
    assert!(!corrente.when.is_empty());
    assert!(
        snapshot.entries.iter().any(|entry| entry.immutable),
        "la radice è immutabile: {:?}",
        snapshot.entries
    );
}

#[test]
#[ignore = "richiede jj; esegui con --ignored esplicitamente"]
fn describe_e_new_reali() {
    let _serial = common::serial_process_test();
    lume_testkit::richiedi!(lume_testkit::Strumento::Jujutsu);
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let prova = Prova::nuova();
    std::fs::write(prova.repo.path().join("a.txt"), "uno\n").unwrap();
    let ws = prova.workspace();
    let no = AtomicBool::new(false);

    ws.run_command(&JjCommand::Describe("prima\n\ncon corpo".to_owned()), &no)
        .unwrap();
    let dopo_describe = ws.snapshot(30, &no).unwrap();
    assert_eq!(dopo_describe.entries[0].description, "prima");

    ws.run_command(&JjCommand::New(None), &no).unwrap();
    let dopo_new = ws.snapshot(30, &no).unwrap();
    assert!(dopo_new.entries[0].working_copy);
    assert!(dopo_new.entries[0].empty, "{:?}", dopo_new.entries[0]);
    assert_eq!(dopo_new.entries[0].description, "");
    assert_eq!(dopo_new.entries[1].description, "prima");
}

#[test]
#[ignore = "richiede jj; esegui con --ignored esplicitamente"]
fn un_messaggio_che_comincia_con_un_trattino_resta_un_messaggio() {
    let _serial = common::serial_process_test();
    lume_testkit::richiedi!(lume_testkit::Strumento::Jujutsu);
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let prova = Prova::nuova();
    let ws = prova.workspace();
    let no = AtomicBool::new(false);

    ws.run_command(&JjCommand::Describe("--amend".to_owned()), &no)
        .unwrap();

    let snapshot = ws.snapshot(30, &no).unwrap();
    assert_eq!(snapshot.entries[0].description, "--amend");
}

#[test]
#[ignore = "richiede jj; esegui con --ignored esplicitamente"]
fn push_reale_verso_un_remote_locale() {
    let _serial = common::serial_process_test();
    lume_testkit::richiedi!(lume_testkit::Strumento::Jujutsu);
    lume_testkit::richiedi!(lume_testkit::Strumento::Git);
    let prova = Prova::nuova();
    let remoto = tempfile::tempdir().unwrap();
    let git = Command::new("git")
        .args(["init", "--bare"])
        .arg(remoto.path())
        .output()
        .unwrap();
    assert!(git.status.success());
    let repo = prova.repo.path();
    let remoto_str = remoto.path().to_str().unwrap();
    prova.jj(repo, &["git", "remote", "add", "origin", remoto_str]);

    // Primo push: un bookmark nuovo lo spinge l'utente da terminale (`-b`);
    // da lì in poi il bookmark è tracciato e il push di Lume lo segue.
    std::fs::write(repo.join("a.txt"), "uno\n").unwrap();
    prova.jj(repo, &["describe", "--message=uno"]);
    prova.jj(repo, &["bookmark", "create", "main", "-r", "@"]);
    prova.jj(repo, &["git", "push", "-b", "main"]);

    let ws = prova.workspace();
    let no = AtomicBool::new(false);
    // Il primo push lascia la working copy vuota e senza descrizione; descriverla
    // prima di creare il cambiamento successivo mantiene pushabile la catena.
    ws.run_command(&JjCommand::Describe("intermedio".to_owned()), &no)
        .unwrap();
    ws.run_command(&JjCommand::New(None), &no).unwrap();
    std::fs::write(repo.join("b.txt"), "due\n").unwrap();
    ws.run_command(&JjCommand::Describe("seconda".to_owned()), &no)
        .unwrap();
    prova.jj(repo, &["bookmark", "set", "main", "-r", "@"]);

    ws.run_command(&JjCommand::Push, &no).unwrap();

    let git_dir = format!("--git-dir={remoto_str}");
    let log = Command::new("git")
        .args([git_dir.as_str(), "log", "-1", "--format=%s", "main"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "seconda");

    // Secondo push: nulla da spingere, e non è un errore.
    ws.run_command(&JjCommand::Push, &no).unwrap();
}
