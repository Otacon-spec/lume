//! Esecuzione delle azioni Git lunghe fuori dal thread UI.
//!
//! Il worker possiede il flag condiviso di annullamento; le operazioni native
//! restano in `lume-git`, mentre i comandi della CLI sono limitati alle azioni
//! dichiarate da `GitAction::needs_git_cli`.

use std::path::Path;
use std::sync::{Arc, atomic::AtomicBool};

use lume_git::{GitCli, GitError, GitRepository, MergeOutcome, PullOutcome, StashApplyOutcome};
use lume_ui::tool::GitAction;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeKind {
    Ok,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionReport {
    pub kind: NoticeKind,
    pub text: String,
    pub needs_merge: bool,
}

impl ActionReport {
    pub fn ok(text: impl Into<String>) -> Self {
        Self::with(NoticeKind::Ok, text)
    }

    pub fn warning(text: impl Into<String>) -> Self {
        Self::with(NoticeKind::Warning, text)
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self::with(NoticeKind::Error, text)
    }

    fn with(kind: NoticeKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            needs_merge: false,
        }
    }
}

pub fn execute(root: &Path, action: &GitAction, cancel: Arc<AtomicBool>) -> ActionReport {
    execute_with(root, GitCli::locate(), action, cancel)
}

pub fn execute_with(
    root: &Path,
    cli: Option<GitCli>,
    action: &GitAction,
    cancel: Arc<AtomicBool>,
) -> ActionReport {
    let repository = match GitRepository::discover(root) {
        Ok(repository) => repository,
        Err(error) => return ActionReport::error(error.to_string()),
    };
    match run(&repository, cli.as_ref(), action, &cancel) {
        Ok(report) => report,
        Err(GitError::Cancelled) => ActionReport::warning("Annullato."),
        Err(error) => ActionReport::error(error.to_string()),
    }
}

fn run(
    repository: &GitRepository,
    cli: Option<&GitCli>,
    action: &GitAction,
    cancel: &Arc<AtomicBool>,
) -> Result<ActionReport, GitError> {
    let git = || cli.ok_or_else(|| GitError::Cli("git non trovato su PATH".to_owned()));
    let report = match action {
        GitAction::Fetch => {
            let remote = repository.fetch_origin_with(cancel)?;
            ActionReport::ok(format!("Fetch da «{remote}» completato."))
        }
        GitAction::Pull => match repository.pull_with(Arc::clone(cancel)) {
            Ok(PullOutcome::UpToDate { .. }) => ActionReport::ok("Già aggiornato."),
            Ok(PullOutcome::LocalAhead { .. }) => {
                ActionReport::warning("Il branch locale è avanti rispetto all'upstream.")
            }
            Ok(PullOutcome::FastForward { .. }) => {
                ActionReport::ok("Aggiornato (avanzamento rapido).")
            }
            Ok(PullOutcome::Merged { .. }) => ActionReport::ok("Aggiornato con un merge."),
            Ok(PullOutcome::NeedsMerge { conflicts, .. }) => ActionReport {
                needs_merge: true,
                ..ActionReport::warning(format!(
                    "Il pull ha trovato conflitti in {} file; avvia il merge per risolverli.",
                    conflicts.len()
                ))
            },
            Err(error)
                if error.publish.is_none() && error.message == GitError::Cancelled.to_string() =>
            {
                ActionReport::warning("Annullato.")
            }
            Err(error) => return Ok(ActionReport::error(error.to_string())),
        },
        GitAction::Push => ActionReport::ok(repository.push(git()?, cancel)?),
        GitAction::MergeUpstream => {
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                return Err(GitError::Cancelled);
            }
            let output = git()?.run(
                repository.work_dir(),
                &["merge", "--no-edit", "@{upstream}"],
                cancel,
            )?;
            if output.success() {
                ActionReport::ok("Merge completato.")
            } else if repository.merge_in_progress() {
                let conflicts = repository.merge_conflicts()?;
                ActionReport::warning(format!(
                    "Merge in corso: {} file in conflitto. Risolvili, mettili in stage e continua.",
                    conflicts.len()
                ))
            } else {
                return Err(GitError::Merge(output.failure_text()));
            }
        }
        GitAction::MergeContinue(message) => {
            if !repository.merge_in_progress() {
                return Err(GitError::MergeNotInProgress);
            }
            if !repository.merge_conflicts()?.is_empty() {
                return Err(GitError::ConflictedIndex);
            }
            let output = git()?.run(repository.work_dir(), &["commit", "-m", message], cancel)?;
            if !output.success() {
                return Err(GitError::Merge(output.failure_text()));
            }
            ActionReport::ok("Merge concluso.")
        }
        GitAction::MergeAbort => {
            repository.merge_abort(git()?, cancel)?;
            ActionReport::ok("Merge annullato.")
        }
        GitAction::MergeBranch(name) => match repository.merge_branch(git()?, name, cancel)? {
            MergeOutcome::UpToDate { .. } => ActionReport::ok("Branch già integrato."),
            MergeOutcome::FastForward { .. } => {
                ActionReport::ok("Merge completato con fast-forward.")
            }
            MergeOutcome::Merged { .. } => ActionReport::ok("Merge completato."),
            MergeOutcome::Conflicts { paths, .. } => ActionReport::warning(format!(
                "Merge in corso: {} file in conflitto. Risolvili e continua.",
                paths.len()
            )),
        },
        GitAction::SwitchBranch(name) => {
            repository.switch_branch(git()?, name, cancel)?;
            ActionReport::ok(format!("Sei su «{name}»."))
        }
        GitAction::SwitchRemoteBranch { remote, name } => {
            let local = repository.switch_remote_branch(git()?, remote, name, cancel)?;
            ActionReport::ok(format!("Creato «{local}» da {remote}/{name}."))
        }
        GitAction::StashPush(message) => {
            let created = repository.stash_save(git()?, message.as_deref(), cancel)?;
            if created {
                ActionReport::ok("Modifiche messe da parte nello stash.")
            } else {
                ActionReport::warning("Nessuna modifica da salvare nello stash.")
            }
        }
        GitAction::StashApply(index) => {
            match repository.stash_apply(git()?, *index, Arc::clone(cancel))? {
                StashApplyOutcome::Applied => {
                    ActionReport::ok(format!("stash@{{{index}}} applicato."))
                }
                StashApplyOutcome::Conflicts(files) => ActionReport::warning(format!(
                    "stash@{{{index}}} applicato con {} file in conflitto; lo stash è rimasto nell'elenco.",
                    files.len()
                )),
            }
        }
        GitAction::StashPop(index) => {
            match repository.stash_pop(git()?, *index, Arc::clone(cancel))? {
                StashApplyOutcome::Applied => {
                    ActionReport::ok(format!("stash@{{{index}}} applicato e rimosso."))
                }
                StashApplyOutcome::Conflicts(files) => ActionReport::warning(format!(
                    "stash@{{{index}}} applicato con {} file in conflitto; lo stash è rimasto nell'elenco.",
                    files.len()
                )),
            }
        }
        GitAction::StashDrop(index) => {
            repository.stash_drop(*index)?;
            ActionReport::ok(format!("stash@{{{index}}} eliminato."))
        }
    };
    Ok(report)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use std::sync::atomic::AtomicBool;

    fn git(dir: &Path, args: &[&str]) -> String {
        let cli = GitCli::locate().expect("git disponibile per i test");
        let mut configured = vec![
            "-c",
            "user.name=Lume Test",
            "-c",
            "user.email=lume@example.test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.quotepath=false",
        ];
        configured.extend_from_slice(args);
        let stop = AtomicBool::new(false);
        let output = cli
            .run(dir, &configured, &stop)
            .expect("eseguire git per i test");
        assert!(output.success(), "git {args:?}: {}", output.failure_text());
        output.stdout.trim().to_owned()
    }

    fn configure(dir: &Path) {
        git(dir, &["config", "user.name", "Lume Test"]);
        git(dir, &["config", "user.email", "lume@example.test"]);
        git(dir, &["config", "commit.gpgsign", "false"]);
        git(dir, &["config", "core.autocrlf", "false"]);
    }

    fn commit_file(dir: &Path, name: &str, contents: &str, message: &str) {
        std::fs::write(dir.join(name), contents).expect("write file");
        git(dir, &["add", name]);
        git(dir, &["commit", "-m", message]);
    }

    fn repo_con_origin() -> (tempfile::TempDir, tempfile::TempDir) {
        let origin = tempfile::tempdir().expect("origin tempdir");
        git(origin.path(), &["init", "--bare", "-b", "main"]);
        let work = tempfile::tempdir().expect("work tempdir");
        git(work.path(), &["init", "-b", "main"]);
        configure(work.path());
        commit_file(work.path(), "a.txt", "uno\n", "primo");
        let url = origin.path().to_str().expect("origin path UTF-8");
        git(work.path(), &["remote", "add", "origin", url]);
        git(work.path(), &["push", "-u", "origin", "main"]);
        (origin, work)
    }

    #[test]
    fn cli_assente_e_errore_solo_per_le_azioni_che_la_richiedono() {
        let root = tempfile::tempdir().expect("tempdir");
        let cli = GitCli::locate().expect("git disponibile per inizializzare il test");
        let init = cli
            .run(root.path(), &["init"], &AtomicBool::new(false))
            .expect("git init");
        assert!(init.success(), "{}", init.failure_text());
        let report = execute_with(
            root.path(),
            None,
            &GitAction::Push,
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(report.kind, NoticeKind::Error);
        assert!(report.text.contains("git non trovato"), "{}", report.text);
    }

    #[test]
    fn fetch_annullato_prima_dell_avvio_e_un_avviso() {
        let root = tempfile::tempdir().expect("tempdir");
        let cli = GitCli::locate().expect("git disponibile per inizializzare il test");
        let init = cli
            .run(root.path(), &["init"], &AtomicBool::new(false))
            .expect("git init");
        assert!(init.success(), "{}", init.failure_text());
        let report = execute_with(
            root.path(),
            None,
            &GitAction::Fetch,
            Arc::new(AtomicBool::new(true)),
        );
        assert_eq!(report.kind, NoticeKind::Warning);
        assert_eq!(report.text, "Annullato.");
    }

    #[test]
    fn repository_assente_produce_un_errore_mostrabile() {
        let root = tempfile::tempdir().expect("tempdir");
        let report = execute_with(
            root.path(),
            None,
            &GitAction::Fetch,
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(report.kind, NoticeKind::Error);
        assert!(!report.text.is_empty());
    }

    #[test]
    fn push_pubblica_il_commit_sul_remote_reale() {
        let (origin, work) = repo_con_origin();
        commit_file(work.path(), "b.txt", "due\n", "secondo");
        let report = execute_with(
            work.path(),
            GitCli::locate(),
            &GitAction::Push,
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(report.kind, NoticeKind::Ok, "{}", report.text);
        assert_eq!(
            git(origin.path(), &["rev-parse", "main"]),
            git(work.path(), &["rev-parse", "HEAD"])
        );
    }

    #[test]
    fn pull_avanza_il_branch_locale_da_un_remote_reale() {
        let (origin, work) = repo_con_origin();
        let colleague = tempfile::tempdir().expect("colleague tempdir");
        let origin_path = origin.path().to_str().expect("origin path UTF-8");
        git(colleague.path(), &["clone", origin_path, "."]);
        configure(colleague.path());
        commit_file(colleague.path(), "b.txt", "due\n", "secondo");
        git(colleague.path(), &["push", "origin", "main"]);

        let report = execute_with(
            work.path(),
            None,
            &GitAction::Pull,
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(report.kind, NoticeKind::Ok, "{}", report.text);
        assert!(work.path().join("b.txt").is_file());
    }

    #[test]
    fn pull_conflictuale_merge_e_continue_usano_il_remote_e_lasciamo_la_risoluzione() {
        let (origin, work) = repo_con_origin();
        let colleague = tempfile::tempdir().expect("colleague tempdir");
        let origin_path = origin.path().to_str().expect("origin path UTF-8");
        git(colleague.path(), &["clone", origin_path, "."]);
        configure(colleague.path());
        commit_file(colleague.path(), "a.txt", "remoto\n", "remote edit");
        git(colleague.path(), &["push", "origin", "main"]);
        commit_file(work.path(), "a.txt", "locale\n", "local edit");

        let pull = execute_with(
            work.path(),
            None,
            &GitAction::Pull,
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(pull.kind, NoticeKind::Warning, "{}", pull.text);
        assert!(pull.needs_merge);
        assert_eq!(
            std::fs::read_to_string(work.path().join("a.txt")).expect("read local file"),
            "locale\n",
            "pull conflict must not alter the working tree"
        );

        let merge = execute_with(
            work.path(),
            GitCli::locate(),
            &GitAction::MergeUpstream,
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(merge.kind, NoticeKind::Warning, "{}", merge.text);
        assert!(
            GitRepository::discover(work.path())
                .expect("repository")
                .merge_in_progress()
        );

        std::fs::write(work.path().join("a.txt"), "risolto\n").expect("resolve conflict");
        git(work.path(), &["add", "a.txt"]);
        let continued = execute_with(
            work.path(),
            GitCli::locate(),
            &GitAction::MergeContinue("merge finale".into()),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(continued.kind, NoticeKind::Ok, "{}", continued.text);
        assert_eq!(
            git(work.path(), &["log", "-1", "--format=%s"]),
            "merge finale"
        );
        assert!(
            !GitRepository::discover(work.path())
                .expect("repository")
                .merge_in_progress()
        );
    }

    #[test]
    fn stash_push_e_apply_operano_sul_worktree_reale() {
        let (_origin, work) = repo_con_origin();
        let file = work.path().join("a.txt");
        std::fs::write(&file, "modificato\n").expect("modify tracked file");

        let saved = execute_with(
            work.path(),
            GitCli::locate(),
            &GitAction::StashPush(None),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(saved.kind, NoticeKind::Ok, "{}", saved.text);
        assert_eq!(std::fs::read_to_string(&file).expect("read file"), "uno\n");

        let applied = execute_with(
            work.path(),
            GitCli::locate(),
            &GitAction::StashApply(0),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(applied.kind, NoticeKind::Ok, "{}", applied.text);
        assert_eq!(
            std::fs::read_to_string(&file).expect("read file"),
            "modificato\n"
        );
        assert!(!git(work.path(), &["stash", "list"]).is_empty());
    }

    #[test]
    fn il_report_pull_conflictuale_chiede_il_merge() {
        let report = ActionReport {
            needs_merge: true,
            ..ActionReport::warning("conflitti")
        };
        assert_eq!(report.kind, NoticeKind::Warning);
        assert!(report.needs_merge);
    }
}
