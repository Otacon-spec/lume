#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]
//! Repository temporanei per i test di `lume-git`. Ogni funzione crea cartelle
//! dentro una `TempDir` che sparisce a fine test: **nessun test tocca il
//! repository vero**. Il `git` usato per preparare gli scenari è quello vero
//! (serve su PATH, precondizione P3 del piano).

use std::path::Path;
use std::process::Command;

use lume_git::GitCli;
use tempfile::TempDir;

/// Il `git` che Lume userebbe. Senza, i test falliscono con un messaggio chiaro.
pub fn cli() -> GitCli {
    GitCli::locate().expect("serve git su PATH per questi test (precondizione P3)")
}

/// Lancia `git` con configurazione isolata: nessun file di configurazione
/// globale o di sistema dell'utente (niente `autocrlf`, niente firma dei
/// commit) e un'identità fissa. Restituisce lo stdout senza il ritorno a capo
/// finale; fallisce il test, con lo stderr, se il comando fallisce.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", null)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Lume Test")
        .env("GIT_AUTHOR_EMAIL", "lume@example.test")
        .env("GIT_COMMITTER_NAME", "Lume Test")
        .env("GIT_COMMITTER_EMAIL", "lume@example.test")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} fallito in {}:\nstdout: {}\nstderr: {}",
        args.join(" "),
        dir.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end_matches(['\r', '\n'])
        .to_owned()
}

fn configure(dir: &Path) {
    git(dir, &["config", "user.name", "Lume Test"]);
    git(dir, &["config", "user.email", "lume@example.test"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

/// Scrive `nome` con `contenuto`, lo mette in stage e crea un commit.
pub fn commit_file(dir: &Path, name: &str, content: &str, message: &str) {
    if let Some(parent) = dir.join(name).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(dir.join(name), content).unwrap();
    git(dir, &["add", "--", name]);
    git(dir, &["commit", "-m", message]);
}

/// Repository con un solo commit («base») che contiene `a.txt` = «uno».
/// Il branch si chiama `main`.
pub fn init_repo() -> TempDir {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init", "-b", "main"]);
    configure(temp.path());
    commit_file(temp.path(), "a.txt", "uno\n", "base");
    temp
}

/// Repository bare vuoto, da usare come `origin`.
pub fn bare_origin() -> TempDir {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init", "--bare", "-b", "main"]);
    temp
}

/// Clone di `origin` in una cartella nuova, già configurato per i commit.
pub fn clone_of(origin: &Path) -> TempDir {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["clone", &origin.to_string_lossy(), "."]);
    configure(temp.path());
    temp
}

/// `(origin, lavoro)`: `lavoro` è `init_repo()` con `origin` come remote e
/// `main` già pubblicato con upstream `origin/main`.
pub fn repo_with_origin() -> (TempDir, TempDir) {
    let origin = bare_origin();
    let work = init_repo();
    git(
        work.path(),
        &["remote", "add", "origin", &origin.path().to_string_lossy()],
    );
    git(work.path(), &["push", "-u", "origin", "main"]);
    (origin, work)
}
