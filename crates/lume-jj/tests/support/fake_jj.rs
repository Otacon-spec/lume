#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Un finto `jj` per i test di `lume-jj`. Il test copia questo eseguibile come
//! `jj` (`jj.exe` su Windows) in una cartella temporanea e vi scrive accanto i
//! file che lo guidano, uno per «comando» (la chiave è il primo argomento che
//! non sia un'opzione globale; per `git push` è `git-push`; senza argomenti
//! `vuoto`):
//!
//! - `out-<chiave>.txt`: stampato su stdout;
//! - `err-<chiave>.txt`: stampato su stderr;
//! - `code-<chiave>.txt`: codice di uscita (default 0);
//! - `sleep-<chiave>.txt`: se esiste, dorme 30 s prima di rispondere.
//!
//! Ogni chiamata aggiunge una riga JSON a `calls.jsonl` (cartella di lavoro,
//! argomenti, variabili d'ambiente che Lume deve impostare).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

fn main() {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().to_path_buf();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let key = key_of(&args);
    record(&dir, &args);

    let read = |prefix: &str| std::fs::read_to_string(dir.join(format!("{prefix}-{key}.txt"))).ok();
    if read("sleep").is_some() {
        std::thread::sleep(Duration::from_secs(30));
    }
    if let Some(out) = read("out") {
        print!("{out}");
        let _ = std::io::stdout().flush();
    }
    if let Some(err) = read("err") {
        eprint!("{err}");
        let _ = std::io::stderr().flush();
    }
    let code = read("code")
        .and_then(|text| text.trim().parse::<i32>().ok())
        .unwrap_or(0);
    std::process::exit(code);
}

/// Gli argomenti senza le opzioni globali che Lume antepone (`--no-pager`,
/// `--color <quando>`), poi la chiave del comando.
fn key_of(args: &[String]) -> String {
    let mut rest = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--no-pager" => {}
            "--color" => {
                iter.next();
            }
            _ => rest.push(arg.clone()),
        }
    }
    match (rest.first().map(String::as_str), rest.get(1)) {
        (Some("git"), Some(second)) => format!("git-{second}"),
        (Some(first), _) => first.to_owned(),
        (None, _) => "vuoto".to_owned(),
    }
}

fn record(dir: &Path, args: &[String]) {
    let var = |name: &str| std::env::var(name).ok();
    let line = serde_json::json!({
        "cwd": std::env::current_dir().unwrap().to_string_lossy(),
        "args": args,
        "env": {
            "JJ_EDITOR": var("JJ_EDITOR"),
            "GIT_TERMINAL_PROMPT": var("GIT_TERMINAL_PROMPT"),
            "GIT_EDITOR": var("GIT_EDITOR"),
        },
    });
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("calls.jsonl"))
        .unwrap();
    writeln!(file, "{line}").unwrap();
}
