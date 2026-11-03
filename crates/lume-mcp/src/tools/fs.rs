//! `list_files`, `search_text` e la lettura da disco di `read_file`.
//!
//! Le opzioni del walker sono quelle dei walker che l'app ha già
//! (`crates/lume-core/src/search.rs:321-330`, `crates/lume-core/src/fs_tree.rs:151-155`):
//! dotfile inclusi, `.gitignore` rispettato anche fuori da un repo, `.git`
//! saltata. I link non si seguono (default di `ignore`), quindi una junction
//! che esce dalla radice non si attraversa.

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use serde_json::{Value, json};

use crate::catalog::{Args, ToolError, arg_bool, arg_opt_bool, arg_opt_str, arg_opt_u64, arg_str};
use crate::confine::{Confined, Root};

/// Come `MAX_FILE_SIZE` della ricerca (`crates/lume-core/src/search.rs:27`).
pub const MAX_READ_BYTES: usize = 1024 * 1024;
pub const LIST_DEFAULT: usize = 500;
pub const LIST_MAX: usize = 5000;
/// Una riga di risultato non porta con sé un file minificato intero.
const LINE_TEXT_MAX: usize = 300;

/// Il testo di un file: dal buffer dell'editor o dal disco.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSnapshot {
    pub text: String,
    pub dirty: bool,
    pub truncated: bool,
}

fn base_dir(root: &Root, args: &Args) -> Result<Confined, ToolError> {
    let base = match arg_opt_str(args, "path")?.map(str::trim) {
        Some(path) if !path.is_empty() && path != "." => root.resolve(path)?,
        _ => Confined {
            absolute: root.path().to_path_buf(),
            relative: PathBuf::new(),
        },
    };
    if !base.absolute.is_dir() {
        return Err(ToolError::new(format!(
            "non è una cartella: {}",
            base.slash()
        )));
    }
    Ok(base)
}

pub fn list_files(root: &Root, args: &Args) -> Result<Value, ToolError> {
    let base = base_dir(root, args)?;
    let limit = arg_opt_u64(args, "limit")?.map_or(LIST_DEFAULT, |n| {
        usize::try_from(n).unwrap_or(LIST_MAX).clamp(1, LIST_MAX)
    });
    let glob = match arg_opt_str(args, "glob")? {
        Some(pattern) => {
            let mut builder = ignore::overrides::OverrideBuilder::new(root.path());
            builder
                .add(pattern)
                .map_err(|error| ToolError::new(format!("glob non valido: {error}")))?;
            Some(
                builder
                    .build()
                    .map_err(|error| ToolError::new(format!("glob non valido: {error}")))?,
            )
        }
        None => None,
    };
    let mut files = Vec::new();
    let mut truncated = false;
    let walker = ignore::WalkBuilder::new(&base.absolute)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .sort_by_file_name(|a, b| a.cmp(b))
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Some(found) = root.relative_of(entry.path()) else {
            continue;
        };
        if let Some(glob) = &glob
            && !glob.matched(&found.relative, false).is_whitelist()
        {
            continue;
        }
        if files.len() == limit {
            truncated = true;
            break;
        }
        files.push(found.slash());
    }
    Ok(json!({"files": files, "truncated": truncated, "limit": limit}))
}

pub fn search_text(root: &Root, args: &Args) -> Result<Value, ToolError> {
    let query = arg_str(args, "query")?;
    if query.is_empty() {
        return Err(ToolError::new("query vuota"));
    }
    let base = base_dir(root, args)?;
    let options = lume_core::SearchOptions {
        needle: query.to_owned(),
        case_sensitive: arg_opt_bool(args, "case_sensitive")?,
        whole_word: arg_bool(args, "whole_word")?,
        regex: arg_bool(args, "regex")?,
    };
    let receiver = lume_core::spawn_search(
        lume_core::SearchScope::Albero(base.absolute.clone()),
        options,
        Arc::new(AtomicU64::new(1)),
        1,
    );
    let mut matches = Vec::new();
    let (mut files, mut total, mut truncated) = (0, 0, false);
    for update in receiver {
        match update {
            lume_core::SearchUpdate::Batch(batch) => {
                for file in batch {
                    let Some(found) = root.relative_of(&file.path) else {
                        continue;
                    };
                    for hit in file.hits {
                        let column = hit
                            .line_text
                            .get(..hit.range_in_line.start)
                            .map_or(1, |before| before.chars().count() + 1);
                        let text: String = hit.line_text.chars().take(LINE_TEXT_MAX).collect();
                        matches.push(json!({
                            "path": found.slash(),
                            "line": hit.line_number,
                            "column": column,
                            "text": text,
                        }));
                    }
                }
            }
            lume_core::SearchUpdate::Done {
                files: done_files,
                matches: done_matches,
                truncated: done_truncated,
            } => {
                files = done_files;
                total = done_matches;
                truncated = done_truncated;
                break;
            }
            lume_core::SearchUpdate::Error(error) => {
                return Err(ToolError::new(format!("ricerca non eseguibile: {error}")));
            }
        }
    }
    Ok(json!({"matches": matches, "files": files, "total": total, "truncated": truncated}))
}

pub fn read_disk(path: &Path) -> Result<TextSnapshot, ToolError> {
    let file = std::fs::File::open(path)
        .map_err(|error| ToolError::new(format!("lettura fallita: {error}")))?;
    let mut bytes = Vec::new();
    let limit = u64::try_from(MAX_READ_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    file.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|error| ToolError::new(format!("lettura fallita: {error}")))?;
    let truncated = bytes.len() > MAX_READ_BYTES;
    bytes.truncate(MAX_READ_BYTES);
    if bytes.contains(&0) {
        return Err(ToolError::new("file binario: read_file legge solo testo"));
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            let utf8 = error.utf8_error();
            // Solo un carattere spezzato **in fondo** a un file troncato è
            // colpa del tetto; ogni altro byte invalido è un file non UTF-8.
            if !(truncated && utf8.error_len().is_none()) {
                return Err(ToolError::new(
                    "file non UTF-8: read_file legge solo testo UTF-8",
                ));
            }
            let mut bytes = error.into_bytes();
            bytes.truncate(utf8.valid_up_to());
            String::from_utf8(bytes).map_err(|_| ToolError::new("file non UTF-8"))?
        }
    };
    let text = match text.strip_prefix('\u{feff}') {
        Some(without_bom) => without_bom.to_owned(),
        None => text,
    };
    Ok(TextSnapshot {
        text,
        dirty: false,
        truncated,
    })
}

pub fn render_read(
    relative: &str,
    source: &str,
    snapshot: &TextSnapshot,
    args: &Args,
) -> Result<Value, ToolError> {
    let start = arg_opt_u64(args, "start_line")?.unwrap_or(1).max(1);
    let end = arg_opt_u64(args, "end_line")?;
    if let Some(end) = end
        && end < start
    {
        return Err(ToolError::new("end_line precede start_line"));
    }
    let numbered = arg_bool(args, "line_numbers")?;
    let mut text = String::new();
    let mut last = 0_u64;
    let mut total = 0_u64;
    for (index, line) in snapshot.text.lines().enumerate() {
        let number = u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1);
        total = number;
        if number < start || end.is_some_and(|end| number > end) {
            continue;
        }
        if numbered {
            let _ = write!(text, "{number:>6}\t");
        }
        text.push_str(line);
        text.push('\n');
        last = number;
    }
    Ok(json!({
        "path": relative,
        "source": source,
        "dirty": snapshot.dirty,
        "truncated": snapshot.truncated,
        "total_lines": total,
        "start_line": start,
        "end_line": last,
        "text": text,
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use serde_json::json;
    use std::fs;

    fn args(valore: Value) -> Args {
        serde_json::from_value(valore).unwrap()
    }

    fn progetto() -> (tempfile::TempDir, Root) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(
            dir.path().join(".git").join("HEAD"),
            "ref: refs/heads/main\n",
        )
        .unwrap();
        fs::write(dir.path().join(".gitignore"), "ignorato.txt\n").unwrap();
        fs::write(dir.path().join("ignorato.txt"), "hello nascosto\n").unwrap();
        fs::write(dir.path().join("a.txt"), "hello mondo\nniente\n").unwrap();
        fs::write(
            dir.path().join("src").join("main.rs"),
            "// perché hello\nfn main() {}\n",
        )
        .unwrap();
        let root = Root::new(dir.path()).unwrap();
        (dir, root)
    }

    #[test]
    fn list_files_rispetta_gitignore_e_salta_git() {
        let (_dir, root) = progetto();
        let esito = list_files(&root, &Args::new()).unwrap();
        assert_eq!(
            esito["files"],
            json!([".gitignore", "a.txt", "src/main.rs"])
        );
        assert_eq!(esito["truncated"], false);
    }

    #[test]
    fn list_files_con_limite_sottocartella_e_glob() {
        let (_dir, root) = progetto();
        let uno = list_files(&root, &args(json!({"limit": 1}))).unwrap();
        assert_eq!(uno["files"].as_array().unwrap().len(), 1);
        assert_eq!(uno["truncated"], true);
        let sotto = list_files(&root, &args(json!({"path": "src"}))).unwrap();
        assert_eq!(sotto["files"], json!(["src/main.rs"]));
        let rs = list_files(&root, &args(json!({"glob": "*.rs"}))).unwrap();
        assert_eq!(rs["files"], json!(["src/main.rs"]));
        assert!(list_files(&root, &args(json!({"path": "../"}))).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn list_files_non_entra_in_una_junction_che_esce() {
        let (dir, root) = progetto();
        let altra = tempfile::tempdir().unwrap();
        fs::write(altra.path().join("segreto.txt"), "x").unwrap();
        let esito = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(dir.path().join("fuga"))
            .arg(altra.path())
            .output()
            .unwrap();
        assert!(esito.status.success());
        let elenco = list_files(&root, &Args::new()).unwrap();
        assert!(!elenco.to_string().contains("segreto.txt"), "{elenco}");
    }

    #[test]
    fn search_text_da_riga_e_colonna_in_caratteri_e_salta_gli_ignorati() {
        let (_dir, root) = progetto();
        let esito = search_text(&root, &args(json!({"query": "hello"}))).unwrap();
        assert_eq!(esito["total"], 2);
        let matches = esito["matches"].as_array().unwrap();
        let rs = matches.iter().find(|m| m["path"] == "src/main.rs").unwrap();
        assert_eq!(rs["line"], 1);
        // «// perché » sono 10 caratteri (è vale uno, non due byte)
        assert_eq!(rs["column"], 11);
        assert!(!esito.to_string().contains("ignorato.txt"));
    }

    #[test]
    fn ricerca_mcp_scopre_testo_nei_file_sql_del_progetto() {
        let (dir, root) = progetto();
        fs::create_dir_all(dir.path().join("db")).unwrap();
        fs::write(
            dir.path().join("db/procedure.sql"),
            "CREATE PROCEDURE hello_world AS SELECT 1;\n",
        )
        .unwrap();

        let matches = search_text(&root, &args(json!({"query": "hello_world"}))).unwrap();

        assert_eq!(matches["total"], 1);
        assert_eq!(matches["matches"][0]["path"], "db/procedure.sql");
        assert_eq!(matches["matches"][0]["line"], 1);
    }

    #[test]
    fn search_text_con_regex_non_valida_e_un_errore() {
        let (_dir, root) = progetto();
        assert!(search_text(&root, &args(json!({"query": "(", "regex": true}))).is_err());
        assert!(search_text(&root, &args(json!({"query": ""}))).is_err());
    }

    #[test]
    fn read_disk_oltre_un_mib_e_troncato() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("grande.txt");
        fs::write(&file, "a".repeat(MAX_READ_BYTES + 10)).unwrap();
        let letto = read_disk(&file).unwrap();
        assert!(letto.truncated);
        assert_eq!(letto.text.len(), MAX_READ_BYTES);
        assert!(!letto.dirty);
    }

    /// Review Focus: il tetto cade in mezzo a «è» (due byte in UTF-8).
    #[test]
    fn read_disk_taglia_senza_spezzare_un_carattere() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("accenti.txt");
        let mut testo = "a".repeat(MAX_READ_BYTES - 1);
        testo.push_str("èfine");
        fs::write(&file, &testo).unwrap();
        let letto = read_disk(&file).unwrap();
        assert!(letto.truncated);
        assert_eq!(letto.text, "a".repeat(MAX_READ_BYTES - 1));
    }

    /// Review Focus: binario o non UTF-8 → errore del tool, mai spazzatura.
    #[test]
    fn read_disk_binario_o_non_utf8_e_un_errore_chiaro() {
        let dir = tempfile::tempdir().unwrap();
        let binario = dir.path().join("x.bin");
        fs::write(&binario, [0x89, b'P', b'N', b'G', 0, 0, 1]).unwrap();
        assert!(read_disk(&binario).unwrap_err().0.contains("binario"));
        let latin1 = dir.path().join("vecchio.txt");
        fs::write(&latin1, [b'c', b'a', b'f', 0xE8, b'\n']).unwrap();
        assert!(read_disk(&latin1).unwrap_err().0.contains("UTF-8"));
    }

    #[test]
    fn render_read_con_righe_numerate_e_intervallo() {
        let istantanea = TextSnapshot {
            text: "uno\ndue\ntre\nquattro\n".to_owned(),
            dirty: true,
            truncated: false,
        };
        let esito = render_read(
            "a.txt",
            "buffer",
            &istantanea,
            &args(json!({"start_line": 2, "end_line": 3, "line_numbers": true})),
        )
        .unwrap();
        assert_eq!(esito["text"], "     2\tdue\n     3\ttre\n");
        assert_eq!(esito["total_lines"], 4);
        assert_eq!(esito["start_line"], 2);
        assert_eq!(esito["end_line"], 3);
        assert_eq!(esito["dirty"], true);
        assert_eq!(esito["source"], "buffer");
        assert!(
            render_read(
                "a.txt",
                "disk",
                &istantanea,
                &args(json!({"start_line": 3, "end_line": 2}))
            )
            .is_err()
        );
    }
}
