//! L'elenco dei cambiamenti (`jj log`) come JSON: un oggetto per riga.
//!
//! Lume non interpreta mai l'output a colonne di `jj log`, che cambia da una
//! versione all'altra: chiede a `jj` di stampare un oggetto JSON per
//! cambiamento con un template, e lo legge con `serde_json`.

use serde::Deserialize;

use crate::JjError;

/// Il template per `jj log --template`, scritto sulla documentazione di jj
/// 0.45.1: i letterali a singolo apice sono grezzi, quindi il newline finale è
/// il letterale a doppio apice `"\n"`; `escape_json()` trasforma una stringa
/// in una stringa JSON completa di virgolette; `json(bool)` stampa
/// `true`/`false`; `local_bookmarks` si riduce a una stringa con
/// `map(..).join(" ")`. Se `jj` lo rifiutasse, si corregge **solo** questa
/// costante (la convalida il test `tests/jj_reale.rs`).
pub const LOG_TEMPLATE: &str = concat!(
    r#"'{"change":' ++ change_id.short(8).escape_json()"#,
    r#" ++ ',"commit":' ++ commit_id.short(12).escape_json()"#,
    r#" ++ ',"description":' ++ description.first_line().escape_json()"#,
    r#" ++ ',"author":' ++ author.name().escape_json()"#,
    r#" ++ ',"when":' ++ author.timestamp().local().format("%Y-%m-%d %H:%M").escape_json()"#,
    r#" ++ ',"bookmarks":' ++ stringify(local_bookmarks.map(|b| b.name()).join(" ")).escape_json()"#,
    r#" ++ ',"working_copy":' ++ json(current_working_copy)"#,
    r#" ++ ',"empty":' ++ json(empty)"#,
    r#" ++ ',"conflict":' ++ json(conflict)"#,
    r#" ++ ',"immutable":' ++ json(immutable)"#,
    r#" ++ '}' ++ "\n""#,
);

/// Un cambiamento (revisione) dell'elenco.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct JjLogEntry {
    /// ID del cambiamento, 8 caratteri (stabile quando il cambiamento si riscrive).
    pub change: String,
    /// ID del commit, 12 caratteri.
    pub commit: String,
    /// La prima riga della descrizione (vuota se non ce n'è una).
    pub description: String,
    pub author: String,
    /// Data e ora locali dell'autore, `AAAA-MM-GG HH:MM`.
    pub when: String,
    /// I bookmark locali che puntano qui, separati da spazio.
    pub bookmarks: String,
    /// È la copia di lavoro (`@`).
    pub working_copy: bool,
    pub empty: bool,
    pub conflict: bool,
    /// Non riscrivibile (radice o già pubblicato secondo `jj`).
    pub immutable: bool,
}

/// Legge l'output di `jj log --template LOG_TEMPLATE`: un oggetto JSON per
/// riga, righe vuote ignorate. Una riga che non è nel formato atteso (versione
/// di `jj` diversa, template rifiutato, un avviso in mezzo) è un errore che
/// dice quale riga e ne mostra l'inizio: mai una lista vuota fatta passare per
/// «nessun cambiamento».
pub fn parse_log(text: &str) -> Result<Vec<JjLogEntry>, JjError> {
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry = serde_json::from_str::<JjLogEntry>(line).map_err(|error| {
            let inizio: String = line.chars().take(80).collect();
            JjError::Parse(format!("riga {}: {error} («{inizio}»)", index + 1))
        })?;
        entries.push(entry);
    }
    Ok(entries)
}
