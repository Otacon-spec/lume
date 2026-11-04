//! Due fasi off-UI: temporanei protetti prima, commit per file dopo consenso
//! della revisione UI. Nessun thread permanente e nessun rollback multi-file.

use lume_core::{ChangeSet, DocId, FileMatches, SaveSnapshot};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::fmt;

mod job;
pub use job::{Event, Job, Ready, Receipt, completed};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[cfg(test)]
use std::sync::Mutex;

/// Messaggi Iced clonabili, ma un solo proprietario può consumare i handle.
/// Tutti i rami con Plan non vuoto devono prenderlo e dropparlo sul worker.
#[cfg(test)]
pub struct Shared<T>(Arc<Mutex<Option<T>>>);
#[cfg(test)]
impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
#[cfg(test)]
impl<T> fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BatchCondiviso { .. }")
    }
}
#[cfg(test)]
impl<T> Shared<T> {
    fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }
    pub fn take(&self) -> Option<T> {
        // Recuperare il possesso anche se un panic ha avvelenato il mutex:
        // non abbandonare i handle o tentare un secondo commit.
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

#[derive(Clone)]
pub struct OpenInput {
    pub path: PathBuf,
    pub snapshot: SaveSnapshot,
    pub clean: bool,
}
pub struct OpenChange {
    pub input: OpenInput,
    pub changes: ChangeSet,
}
struct Closed {
    path: PathBuf,
    digest: [u8; 32],
    write: lume_core::file_write::PreparedWrite,
}
pub struct Plan {
    opened: Vec<OpenChange>,
    closed: Vec<Closed>,
}
#[derive(Debug, Default)]
pub struct Report {
    pub committed: Vec<PathBuf>,
    pub failed: Option<(PathBuf, String)>,
    pub save_failures: Vec<(PathBuf, String)>,
    pub cleanup_failures: Vec<(PathBuf, String)>,
    pub not_attempted: Vec<PathBuf>,
    pub error: Option<String>,
    /// File scritti, ma con la directory non sincronizzata: non è un
    /// fallimento, va comunque mostrato.
    pub sync_warnings: Vec<(PathBuf, String)>,
}
impl Report {
    /// Avviso sulle directory non sincronizzate, se ce ne sono.
    pub fn sync_warning(&self) -> Option<String> {
        (!self.sync_warnings.is_empty()).then(|| {
            let files = self
                .sync_warnings
                .iter()
                .map(|(p, e)| format!("{}: {e}", p.display()))
                .collect::<Vec<_>>()
                .join("; ");
            format!("File scritti, ma la cartella non è stata sincronizzata sul disco: {files}")
        })
    }
    pub fn ok(&self) -> bool {
        self.failed.is_none()
            && self.save_failures.is_empty()
            && self.cleanup_failures.is_empty()
            && self.error.is_none()
    }
    pub fn describe(&self) -> String {
        let names = |paths: &[PathBuf]| {
            if paths.is_empty() {
                "nessuno".into()
            } else {
                paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        };
        let reason = self
            .failed
            .as_ref()
            .map(|(p, e)| format!("{}: {e}; esito del file fallito da verificare", p.display()))
            .or_else(|| self.error.clone())
            .unwrap_or_else(|| {
                if self.save_failures.is_empty() {
                    "completata".into()
                } else {
                    "parziale".into()
                }
            });
        let failures = self
            .save_failures
            .iter()
            .map(|(p, e)| format!("{}: {e}; esito da verificare", p.display()))
            .collect::<Vec<_>>()
            .join("; ");
        let cleanup = self
            .cleanup_failures
            .iter()
            .map(|(p, e)| format!("{}: {e}; residui da verificare", p.display()))
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "Sostituzione {reason}. Commit riusciti: {}. Non tentati: {}. Salvataggi falliti: {}. Cleanup falliti: {}",
            names(&self.committed),
            names(&self.not_attempted),
            if failures.is_empty() {
                "nessuno"
            } else {
                &failures
            },
            if cleanup.is_empty() {
                "nessuno"
            } else {
                &cleanup
            }
        )
    }
}
pub struct Outcome {
    pub opened: Vec<OpenChange>,
    pub report: Report,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    Committing,
    Aborting,
    Saving,
}
pub struct State {
    pub id: u64,
    pub lease: crate::scritture::Lease,
    pub root: Option<PathBuf>,
    pub mapping: Vec<(DocId, PathBuf)>,
    pub opened: Vec<OpenInput>,
    pub cancel: Arc<AtomicBool>,
    pub phase: Phase,
    pub autosaves: HashMap<DocId, PathBuf>,
    pub report: Report,
    pub occurrences: usize,
    pub files: usize,
    pub dirty: usize,
    pub reloads: Vec<(DocId, PathBuf)>,
}
impl State {
    pub fn blocks_saves(&self) -> bool {
        self.phase != Phase::Saving
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

#[cfg(test)]
pub async fn prepare(
    matches: Vec<FileMatches>,
    replacement: String,
    opened: Vec<OpenInput>,
    mapping: Vec<(DocId, PathBuf)>,
    cancel: Arc<AtomicBool>,
) -> Result<Shared<Plan>, String> {
    tokio::task::spawn_blocking(move || prepare_sync(matches, replacement, opened, mapping, cancel))
        .await
        .map_err(|e| format!("Worker preparazione terminato: {e}"))?
        .map(Shared::new)
        .map_err(|report| report.describe())
}
fn prepare_sync(
    matches: Vec<FileMatches>,
    replacement: String,
    opened: Vec<OpenInput>,
    mapping: Vec<(DocId, PathBuf)>,
    cancel: Arc<AtomicBool>,
) -> Result<Plan, Box<Report>> {
    let targets: Vec<_> = matches
        .iter()
        .filter(|file| !file.hits.is_empty())
        .map(|file| file.path.clone())
        .collect();
    let mut aliases = HashMap::new();
    for (_, path) in mapping {
        if cancel.load(Ordering::Acquire) {
            return Err(Box::new(Report {
                error: Some(
                    "non avviata: annullata prima dei commit; nessun handle restituito".into(),
                ),
                not_attempted: targets,
                ..Report::default()
            }));
        }
        if let Ok(canonical) = lume_core::canonical_path(&path) {
            aliases.insert(canonical, path);
        }
    }
    let mut plan = Plan {
        opened: Vec::new(),
        closed: Vec::new(),
    };
    let result = (|| -> Result<(), String> {
        let mut seen = HashSet::new();
        let mut resolved = HashSet::new();
        for file in matches.into_iter().filter(|f| !f.hits.is_empty()) {
            if cancel.load(Ordering::Acquire) {
                return Err("annullata prima dei commit".into());
            }
            if !seen.insert(file.path.clone()) {
                return Err(format!("Target duplicato: {}", file.path.display()));
            }
            if let Some(input) = opened.iter().find(|o| o.path == file.path) {
                let changes = super::replace::changeset_for(
                    &file.path,
                    input.snapshot.rope(),
                    &file.hits,
                    &replacement,
                )
                .map_err(|e| e.to_string())?;
                plan.opened.push(OpenChange {
                    input: input.clone(),
                    changes,
                });
            } else {
                let canonical = lume_core::canonical_path(&file.path)
                    .map_err(|e| format!("Lettura di {} fallita: {e}", file.path.display()))?;
                if !resolved.insert(canonical.clone()) {
                    return Err(format!("Target fisico duplicato: {}", file.path.display()));
                }
                if let Some(alias) = aliases.get(&canonical) {
                    return Err(format!(
                        "{} è già aperto tramite {}: usa il buffer aperto e rifai la ricerca",
                        file.path.display(),
                        alias.display()
                    ));
                }
                let bytes = crate::file_read::read_bytes_capped(
                    &file.path,
                    crate::file_read::MAX_FILE_BYTES,
                )
                .map_err(|e| {
                    super::replace::ReplaceError::Unreadable(file.path.clone(), e.to_string())
                        .to_string()
                })?;
                let digest = Sha256::digest(&bytes).into();
                let content =
                    super::replace::replace_in_bytes(&file.path, &bytes, &file.hits, &replacement)
                        .map_err(|e| e.to_string())?;
                let write = lume_core::file_write::prepare_atomic(&file.path, &content)
                    .map_err(|e| format!("Preparazione di {} fallita: {e}", file.path.display()))?;
                plan.closed.push(Closed {
                    path: file.path,
                    digest,
                    write,
                });
            }
        }
        if cancel.load(Ordering::Acquire) {
            return Err("annullata prima dei commit".into());
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut report = Report {
            error: Some(format!(
                "non avviata: {error}; nessun commit avviato, residui da verificare"
            )),
            not_attempted: targets,
            ..Report::default()
        };
        abort_all(plan.closed, &mut report);
        return Err(Box::new(report));
    }
    Ok(plan)
}
fn unchanged(file: &Closed) -> Result<(), String> {
    if lume_core::canonical_path(&file.path).map_err(|e| e.to_string())? != file.write.destination()
    {
        return Err("percorso o collegamento cambiato dopo la preparazione".into());
    }
    let bytes = crate::file_read::read_bytes_capped(&file.path, crate::file_read::MAX_FILE_BYTES)
        .map_err(|e| e.to_string())?;
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    if digest != file.digest {
        return Err("contenuto cambiato dopo la preparazione".into());
    }
    Ok(())
}
/// Consuma ogni handle ancora posseduto, senza promettere unlink o rollback.
fn abort_all(closed: impl IntoIterator<Item = Closed>, report: &mut Report) {
    for file in closed {
        if let Err(error) = file.write.abort() {
            report.cleanup_failures.push((file.path, error.to_string()));
        }
    }
}
fn aborted(plan: Plan, reason: String) -> Outcome {
    let mut report = Report {
        error: Some(reason),
        not_attempted: plan
            .closed
            .iter()
            .map(|file| file.path.clone())
            .chain(plan.opened.iter().map(|open| open.input.path.clone()))
            .collect(),
        ..Report::default()
    };
    abort_all(plan.closed, &mut report);
    Outcome {
        opened: Vec::new(),
        report,
    }
}
fn preflight(plan: &Plan, cancel: &AtomicBool) -> Result<(), String> {
    for file in &plan.closed {
        if cancel.load(Ordering::Acquire) {
            return Err("annullata prima dei commit".into());
        }
        unchanged(file).map_err(|error| format!("{}: {error}", file.path.display()))?;
    }
    if cancel.load(Ordering::Acquire) {
        return Err("annullata prima dei commit".into());
    }
    Ok(())
}
// Controlli puntuali, non prenotazioni dei file: resta la gara fingerprint/rename.
fn commit_sync(plan: Plan, cancel: &AtomicBool) -> Outcome {
    if let Err(error) = preflight(&plan, cancel) {
        return aborted(plan, error);
    }
    commit_with_boundary(
        plan,
        cancel,
        #[cfg(test)]
        |_| {},
    )
}
fn commit_with_boundary(
    plan: Plan,
    cancel: &AtomicBool,
    #[cfg(test)] mut boundary: impl FnMut(usize),
) -> Outcome {
    let mut report = Report::default();
    let mut closed = plan.closed.into_iter();
    #[cfg(test)]
    let mut index = 0;
    while let Some(file) = closed.next() {
        #[cfg(test)]
        {
            boundary(index);
            index += 1;
        }
        let check = if cancel.load(Ordering::Acquire) {
            Err("annullata fra i commit; nessun rollback".into())
        } else {
            unchanged(&file).map_err(|error| format!("{}: {error}", file.path.display()))
        };
        if let Err(error) = check {
            report.error = Some(error);
            let remaining = std::iter::once(file).chain(closed).collect::<Vec<_>>();
            report
                .not_attempted
                .extend(remaining.iter().map(|file| file.path.clone()));
            abort_all(remaining, &mut report);
            break;
        }
        match file.write.commit_reporting() {
            Ok(warning) => {
                if let Some(error) = warning {
                    report
                        .sync_warnings
                        .push((file.path.clone(), error.to_string()));
                }
                report.committed.push(file.path);
            }
            Err(error) => {
                // commit consuma questo handle: il suo esito/residuo resta incerto.
                report.failed = Some((file.path, error.to_string()));
                let remaining = closed.collect::<Vec<_>>();
                report
                    .not_attempted
                    .extend(remaining.iter().map(|file| file.path.clone()));
                abort_all(remaining, &mut report);
                break;
            }
        }
    }
    if cancel.load(Ordering::Acquire) && report.ok() {
        report.error = Some("annullata dopo i commit avviati; nessun rollback".into());
    }
    Outcome {
        opened: plan.opened,
        report,
    }
}
fn worker_error(reason: String) -> Outcome {
    Outcome {
        opened: Vec::new(),
        report: Report {
            error: Some(reason),
            ..Report::default()
        },
    }
}
#[cfg(test)]
pub async fn abort(plan: Shared<Plan>) -> Result<Report, String> {
    tokio::task::spawn_blocking(move || {
        let mut report = Report::default();
        if let Some(plan) = plan.take() {
            abort_all(plan.closed, &mut report);
        } else {
            report.error = Some("batch già consumato".into());
        }
        report
    })
    .await
    .map_err(|error| format!("Worker cleanup terminato: {error}; residui da verificare"))
}
#[cfg(test)]
pub async fn commit(plan: Shared<Plan>, cancel: Arc<AtomicBool>) -> Shared<Outcome> {
    let outcome = tokio::task::spawn_blocking(move || {
        let Some(plan) = plan.take() else {
            return worker_error("batch già consumato".into());
        };
        if let Err(error) = preflight(&plan, &cancel) {
            return aborted(plan, error);
        }
        commit_sync(plan, &cancel)
    })
    .await
    .unwrap_or_else(|error| {
        worker_error(format!(
            "Worker commit terminato: {error}; esiti disco e residui da verificare"
        ))
    });
    Shared::new(outcome)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }
    fn found(path: PathBuf) -> FileMatches {
        FileMatches {
            path,
            hits: vec![lume_core::SearchHit {
                line_number: 1,
                line_text: "ago".into(),
                range_in_line: 0..3,
            }],
        }
    }
    fn source(dir: &std::path::Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"ago\n").unwrap();
        p
    }

    #[test]
    fn preparazione_e_abort_reali_non_pubblicano_e_puliscono() {
        let dir = tempfile::tempdir().unwrap();
        let path = source(dir.path(), "file.txt");
        runtime().block_on(async {
            let p = prepare(
                vec![found(path.clone())],
                "AGO".into(),
                vec![],
                vec![],
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"ago\n");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
            assert!(abort(p).await.unwrap().cleanup_failures.is_empty());
            assert_eq!(std::fs::read(&path).unwrap(), b"ago\n");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        });
    }

    #[test]
    fn secondo_commit_readonly_dopo_prepare_riporta_primo_e_non_tentati() {
        let dir = tempfile::tempdir().unwrap();
        let first = source(dir.path(), "first.txt");
        let second = source(dir.path(), "second.txt");
        let third = source(dir.path(), "third.txt");
        let permissions = std::fs::metadata(&second).unwrap().permissions();
        runtime().block_on(async {
            let c = Arc::new(AtomicBool::new(false));
            let p = prepare(
                vec![
                    found(first.clone()),
                    found(second.clone()),
                    found(third.clone()),
                ],
                "AGO".into(),
                vec![],
                vec![],
                c.clone(),
            )
            .await
            .unwrap();
            let mut readonly = permissions.clone();
            readonly.set_readonly(true);
            std::fs::set_permissions(&second, readonly).unwrap();
            let outcome = commit(p, c).await.take().unwrap();
            std::fs::set_permissions(&second, permissions).unwrap();
            assert_eq!(outcome.report.committed, vec![first.clone()]);
            assert_eq!(outcome.report.failed.as_ref().unwrap().0, second);
            assert_eq!(outcome.report.not_attempted, vec![third.clone()]);
            assert_eq!(std::fs::read(first).unwrap(), b"AGO\n");
            assert_eq!(std::fs::read(&second).unwrap(), b"ago\n");
            assert_eq!(std::fs::read(third).unwrap(), b"ago\n");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
        });
    }

    #[test]
    fn fingerprint_stantio_sul_secondo_ferma_prima_del_primo_commit() {
        let dir = tempfile::tempdir().unwrap();
        let first = source(dir.path(), "first.txt");
        let second = source(dir.path(), "second.txt");
        runtime().block_on(async {
            let c = Arc::new(AtomicBool::new(false));
            let p = prepare(
                vec![found(first.clone()), found(second.clone())],
                "AGO".into(),
                vec![],
                vec![],
                c.clone(),
            )
            .await
            .unwrap();
            std::fs::write(&second, b"esterno\n").unwrap();
            let o = commit(p, c).await.take().unwrap();
            assert!(o.report.committed.is_empty());
            assert_eq!(o.report.not_attempted, vec![first.clone(), second.clone()]);
            assert_eq!(std::fs::read(first).unwrap(), b"ago\n");
            assert_eq!(std::fs::read(second).unwrap(), b"esterno\n");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        });
    }

    #[test]
    fn cancellazione_prima_del_commit_non_pubblica_niente() {
        let dir = tempfile::tempdir().unwrap();
        let path = source(dir.path(), "file.txt");
        runtime().block_on(async {
            let c = Arc::new(AtomicBool::new(false));
            let p = prepare(
                vec![found(path.clone())],
                "AGO".into(),
                vec![],
                vec![],
                c.clone(),
            )
            .await
            .unwrap();
            c.store(true, Ordering::Release);
            let o = commit(p, c).await.take().unwrap();
            assert!(o.report.committed.is_empty());
            assert_eq!(o.report.not_attempted, vec![path.clone()]);
            assert_eq!(std::fs::read(path).unwrap(), b"ago\n");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        });
    }

    #[test]
    fn un_target_duplicato_rifiuta_e_pulisce_tutto_il_preflight() {
        let dir = tempfile::tempdir().unwrap();
        let path = source(dir.path(), "file.txt");
        runtime().block_on(async {
            assert!(
                prepare(
                    vec![found(path.clone()), found(path.clone())],
                    "AGO".into(),
                    vec![],
                    vec![],
                    Arc::new(AtomicBool::new(false))
                )
                .await
                .is_err()
            );
            assert_eq!(std::fs::read(path).unwrap(), b"ago\n");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        });
    }
}
