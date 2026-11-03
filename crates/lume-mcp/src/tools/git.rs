//! I tool Git, stateless: ogni chiamata apre il repository dalla radice,
//! come `run_git` nell'app (`crates/lume-app/src/app.rs:5564-5578`), e gira in
//! `spawn_blocking`. Nessuna scrittura: niente stage, commit, checkout.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use lume_git::{ChangeKind, DiffRow, DiffSign, FileDiff, GitRepository};
use serde_json::{Value, json};

use crate::catalog::{Args, ToolError, arg_bool, arg_opt_str, arg_opt_u64, arg_str};
use crate::confine::{Confined, Root, canonical, join_components};

/// Come `tools::gitlog::LOG_LIMIT` (`crates/lume-app/src/tools/gitlog.rs:24`).
const LOG_MAX: usize = 200;
const LOG_DEFAULT: usize = 50;
/// Con i file toccati ogni commit costa un diff: se ne chiedono pochi.
const LOG_WITH_FILES_MAX: usize = 20;
const BLAME_MAX: usize = 5000;

fn git_error(error: lume_git::GitError) -> ToolError {
    ToolError::new(format!("Git: {error}"))
}

fn open(root: &Root) -> Result<(GitRepository, PathBuf), ToolError> {
    let repo = GitRepository::discover(root.path())
        .map_err(|error| ToolError::new(format!("nessun repository Git qui: {error}")))?;
    let work_dir = canonical(repo.work_dir())
        .map_err(|error| ToolError::new(format!("cartella di lavoro non leggibile: {error}")))?;
    Ok((repo, work_dir))
}

/// Il percorso relativo al `work_dir` **canonico**: è la forma che
/// `relative_path` di lume-git accetta senza sorprese.
fn in_repo(work_dir: &Path, file: &Confined) -> Result<PathBuf, ToolError> {
    file.absolute
        .strip_prefix(work_dir)
        .map(Path::to_path_buf)
        .map_err(|_| ToolError::new("il file non sta nel repository"))
}

fn kind(change: Option<ChangeKind>) -> Value {
    let Some(change) = change else {
        return Value::Null;
    };
    Value::from(match change {
        ChangeKind::Added => "added",
        ChangeKind::Removed => "removed",
        ChangeKind::Modified => "modified",
        ChangeKind::TypeChanged => "type_changed",
        ChangeKind::Renamed => "renamed",
        ChangeKind::Copied => "copied",
        ChangeKind::IntentToAdd => "intent_to_add",
        ChangeKind::Conflict => "conflict",
    })
}

pub fn git_status(root: &Root, _args: &Args) -> Result<Value, ToolError> {
    let (repo, work_dir) = open(root)?;
    let branch = repo.branch_name().map_err(git_error)?;
    let mut files = Vec::new();
    for status in repo.status().map_err(git_error)? {
        // Radice MCP = sottocartella del repo: il resto del repo non si vede.
        let Some(found) = root.relative_of(&join_components(&work_dir, &status.path)) else {
            continue;
        };
        files.push(json!({
            "path": found.slash(),
            "index": kind(status.index),
            "worktree": kind(status.worktree),
        }));
    }
    Ok(json!({"branch": branch, "files": files}))
}

pub fn git_diff_file(root: &Root, args: &Args) -> Result<Value, ToolError> {
    let file = root.resolve_lenient(arg_str(args, "path")?)?;
    let (repo, work_dir) = open(root)?;
    let relative = in_repo(&work_dir, &file)?;
    let base = arg_opt_str(args, "base")?.unwrap_or("worktree");
    let diff = match base {
        "worktree" => repo.diff_worktree_file(&relative),
        "index" => repo.diff_index_file(&relative),
        "commit" => repo.diff_commit_file(arg_str(args, "commit")?, &relative),
        other => {
            return Err(ToolError::new(format!(
                "base sconosciuta: {other} (worktree, index o commit)"
            )));
        }
    }
    .map_err(git_error)?;
    let path = file.slash();
    Ok(json!({
        "path": path,
        "base": base,
        "added": diff.added,
        "removed": diff.removed,
        "binary": diff.binary,
        "truncated": diff.truncated,
        "diff": unified(&diff, &path),
    }))
}

pub fn git_log(root: &Root, args: &Args) -> Result<Value, ToolError> {
    let (repo, work_dir) = open(root)?;
    let reference = arg_opt_str(args, "reference")?.unwrap_or("HEAD");
    let include_files = arg_bool(args, "include_files")?;
    let max = if include_files {
        LOG_WITH_FILES_MAX
    } else {
        LOG_MAX
    };
    let limit = arg_opt_u64(args, "limit")?
        .map_or(LOG_DEFAULT, |n| usize::try_from(n).unwrap_or(max))
        .clamp(1, max);
    let mut commits = Vec::new();
    for commit in repo.log_from(reference, limit).map_err(git_error)? {
        let mut entry = json!({
            "id": commit.id,
            "parents": commit.parent_ids,
            "subject": commit.subject,
            "author": commit.author_name,
            "email": commit.author_email,
            "time": commit.author_time,
        });
        if include_files {
            let files: Vec<Value> = repo
                .commit_files(&commit.id)
                .map_err(git_error)?
                .into_iter()
                .filter_map(|changed| {
                    let found = root.relative_of(&join_components(&work_dir, &changed.path))?;
                    Some(json!({"path": found.slash(), "kind": kind(Some(changed.kind))}))
                })
                .collect();
            if let Value::Object(map) = &mut entry {
                map.insert("files".to_owned(), Value::from(files));
            }
        }
        commits.push(entry);
    }
    Ok(json!({"commits": commits}))
}

pub fn git_blame(root: &Root, args: &Args) -> Result<Value, ToolError> {
    let file = root.resolve(arg_str(args, "path")?)?;
    let (repo, work_dir) = open(root)?;
    let relative = in_repo(&work_dir, &file)?;
    let lines = repo.blame(&relative).map_err(git_error)?;
    let dirty = repo.file_is_dirty(&relative);
    let truncated = lines.len() > BLAME_MAX;
    let lines: Vec<Value> = lines
        .iter()
        .take(BLAME_MAX)
        .map(|line| {
            json!({
                "line": line.line_number,
                "commit": line.commit_id,
                "author": line.author_name,
                "time": line.author_time,
            })
        })
        .collect();
    Ok(json!({"path": file.slash(), "dirty": dirty, "lines": lines, "truncated": truncated}))
}

/// Testo unified da `FileDiff::rows`: un blocco per ogni tratto fra due
/// `Gap`. Gli inizi sono il primo numero di riga del blocco (0 per un file
/// nuovo o cancellato, come fa git).
pub fn unified(diff: &FileDiff, path: &str) -> String {
    let mut out = String::new();
    if diff.binary {
        let _ = writeln!(out, "File binari diversi: {path}");
        return out;
    }
    let _ = writeln!(out, "--- a/{path}");
    let _ = writeln!(out, "+++ b/{path}");
    let mut block: Vec<&DiffRow> = Vec::new();
    for row in &diff.rows {
        match row {
            DiffRow::Gap { .. } => {
                flush(&mut out, &block);
                block.clear();
            }
            DiffRow::Line { .. } => block.push(row),
        }
    }
    flush(&mut out, &block);
    out
}

fn flush(out: &mut String, block: &[&DiffRow]) {
    if block.is_empty() {
        return;
    }
    let old_start = block
        .iter()
        .find_map(|row| match row {
            DiffRow::Line { old_line, .. } => *old_line,
            DiffRow::Gap { .. } => None,
        })
        .unwrap_or(0);
    let new_start = block
        .iter()
        .find_map(|row| match row {
            DiffRow::Line { new_line, .. } => *new_line,
            DiffRow::Gap { .. } => None,
        })
        .unwrap_or(0);
    let old_count = block
        .iter()
        .filter(|row| {
            matches!(
                row,
                DiffRow::Line {
                    sign: DiffSign::Context | DiffSign::Removed,
                    ..
                }
            )
        })
        .count();
    let new_count = block
        .iter()
        .filter(|row| {
            matches!(
                row,
                DiffRow::Line {
                    sign: DiffSign::Context | DiffSign::Added,
                    ..
                }
            )
        })
        .count();
    let _ = writeln!(
        out,
        "@@ -{old_start},{old_count} +{new_start},{new_count} @@"
    );
    for row in block {
        if let DiffRow::Line { sign, text, .. } = row {
            let mark = match sign {
                DiffSign::Context => ' ',
                DiffSign::Added => '+',
                DiffSign::Removed => '-',
            };
            let _ = writeln!(out, "{mark}{text}");
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use lume_git::{DiffBase, DiffRow, DiffSign, FileDiff};
    use serde_json::json;
    use std::fs;
    use std::io::Write as _;

    /// Un repository con un commit: `a.txt` e `sotto/b.txt`. Come i test di
    /// lume-git (`crates/lume-git/tests/commit.rs:9-35`): `gix::init` e
    /// l'identità scritta nella config, niente CLI git.
    fn repo_con_un_commit() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        gix::init(dir.path()).unwrap();
        let mut config = fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join(".git").join("config"))
            .unwrap();
        writeln!(
            config,
            "\n[user]\n\tname = Lume Test\n\temail = lume@example.test"
        )
        .unwrap();
        drop(config);
        fs::create_dir_all(dir.path().join("sotto")).unwrap();
        fs::write(dir.path().join("a.txt"), "prima\n").unwrap();
        fs::write(dir.path().join("sotto").join("b.txt"), "bi\n").unwrap();
        let repo = lume_git::GitRepository::discover(dir.path()).unwrap();
        repo.stage(["a.txt", "sotto/b.txt"]).unwrap();
        repo.commit("primo commit").unwrap();
        dir
    }

    fn args(valore: serde_json::Value) -> Args {
        serde_json::from_value(valore).unwrap()
    }

    #[test]
    fn git_status_vede_branch_e_file_modificato() {
        let dir = repo_con_un_commit();
        fs::write(dir.path().join("a.txt"), "seconda\n").unwrap();
        let root = Root::new(dir.path()).unwrap();
        let stato = git_status(&root, &Args::new()).unwrap();
        assert!(stato["branch"].is_string());
        assert_eq!(
            stato["files"],
            json!([{"path": "a.txt", "index": null, "worktree": "modified"}])
        );
    }

    #[test]
    fn git_status_da_una_sottocartella_elenca_solo_la_sottocartella() {
        let dir = repo_con_un_commit();
        fs::write(dir.path().join("a.txt"), "seconda\n").unwrap();
        fs::write(dir.path().join("sotto").join("b.txt"), "bi bis\n").unwrap();
        let root = Root::new(&dir.path().join("sotto")).unwrap();
        let stato = git_status(&root, &Args::new()).unwrap();
        assert_eq!(
            stato["files"],
            json!([{"path": "b.txt", "index": null, "worktree": "modified"}])
        );
    }

    #[test]
    fn git_diff_file_da_testo_unified() {
        let dir = repo_con_un_commit();
        fs::write(dir.path().join("a.txt"), "seconda\n").unwrap();
        let root = Root::new(dir.path()).unwrap();
        let diff = git_diff_file(&root, &args(json!({"path": "a.txt"}))).unwrap();
        assert_eq!(diff["base"], "worktree");
        assert_eq!(diff["added"], 1);
        assert_eq!(diff["removed"], 1);
        let testo = diff["diff"].as_str().unwrap();
        assert!(
            testo.starts_with("--- a/a.txt\n+++ b/a.txt\n@@ "),
            "{testo}"
        );
        assert!(testo.contains("\n-prima\n"), "{testo}");
        assert!(testo.contains("\n+seconda\n"), "{testo}");
        assert!(git_diff_file(&root, &args(json!({"path": "a.txt", "base": "boh"}))).is_err());
    }

    #[test]
    fn git_diff_di_un_file_cancellato_si_fa_lo_stesso() {
        let dir = repo_con_un_commit();
        fs::remove_file(dir.path().join("a.txt")).unwrap();
        let root = Root::new(dir.path()).unwrap();
        let diff = git_diff_file(&root, &args(json!({"path": "a.txt"}))).unwrap();
        assert_eq!(diff["removed"], 1);
        assert_eq!(diff["added"], 0);
    }

    #[test]
    fn git_log_e_blame() {
        let dir = repo_con_un_commit();
        let root = Root::new(dir.path()).unwrap();
        let log = git_log(&root, &args(json!({"include_files": true}))).unwrap();
        let commits = log["commits"].as_array().unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0]["subject"], "primo commit");
        assert_eq!(commits[0]["author"], "Lume Test");
        assert_eq!(commits[0]["files"].as_array().unwrap().len(), 2);
        let blame = git_blame(&root, &args(json!({"path": "a.txt"}))).unwrap();
        assert_eq!(blame["dirty"], false);
        assert_eq!(blame["lines"][0]["line"], 1);
        fs::write(dir.path().join("a.txt"), "cambiata\n").unwrap();
        assert_eq!(
            git_blame(&root, &args(json!({"path": "a.txt"}))).unwrap()["dirty"],
            true
        );
    }

    /// Senza un repository raggiungibile il tool risponde con un errore, mai
    /// con un panic né con una lista vuota. La radice sparita dal disco fa
    /// fallire `discover` **sempre** (`InaccessibleDirectory`,
    /// gix-discover-0.53.0 `src/upwards/mod.rs:56-62`), ovunque stia la
    /// cartella temporanea. Il caso «cartella fuori da ogni repository» passa
    /// dallo stesso ramo di `open`, ma non si prova in modo deterministico:
    /// `%TEMP%` può stare sotto un repository dell'utente, e `discover`
    /// risalirebbe fin lì.
    #[test]
    fn senza_repository_raggiungibile_e_un_errore_del_tool() {
        let dir = tempfile::tempdir().unwrap();
        let cartella = dir.path().join("radice");
        fs::create_dir(&cartella).unwrap();
        let root = Root::new(&cartella).unwrap();
        fs::remove_dir(&cartella).unwrap();
        let errore = git_status(&root, &Args::new()).unwrap_err();
        assert!(errore.0.contains("nessun repository Git qui"), "{errore}");
    }

    #[test]
    fn unified_dalle_righe_del_diff() {
        let riga = |old_line, new_line, sign, text: &str| DiffRow::Line {
            old_line,
            new_line,
            sign,
            text: text.to_owned(),
            hunk_idx: None,
        };
        let diff = FileDiff {
            path: PathBuf::from("a.txt"),
            base: DiffBase::Worktree,
            rows: vec![
                riga(Some(1), Some(1), DiffSign::Context, "uno"),
                riga(Some(2), None, DiffSign::Removed, "due"),
                riga(None, Some(2), DiffSign::Added, "DUE"),
                DiffRow::Gap { skipped: 10 },
                riga(Some(13), Some(13), DiffSign::Context, "tredici"),
            ],
            hunks: Vec::new(),
            added: 1,
            removed: 1,
            binary: false,
            truncated: false,
        };
        assert_eq!(
            unified(&diff, "a.txt"),
            "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n uno\n-due\n+DUE\n@@ -13,1 +13,1 @@\n tredici\n"
        );
    }
}
