//! Sonda degli handle di input/output standard: un exe
//! `windows_subsystem = "windows"` (come `lume.exe` in release,
//! `crates/lume-app/src/main.rs`) riceve gli handle stdio che un client gli
//! passa con `STARTF_USESTDHANDLES`? Legge una riga e risponde
//! `pong <riga>`. Se la risposta arriva, il ponte può stare dentro lume.exe.
#![windows_subsystem = "windows"]

use std::io::{BufRead, Write};

fn main() {
    let mut riga = String::new();
    let letti = std::io::stdin().lock().read_line(&mut riga).unwrap_or(0);
    let mut uscita = std::io::stdout().lock();
    let _ = writeln!(uscita, "pong {}", riga.trim_end());
    let _ = uscita.flush();
    std::process::exit(if letti > 0 { 0 } else { 1 });
}
