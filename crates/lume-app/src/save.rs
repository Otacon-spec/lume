//! Scritture su richiesta, serializzate dall'app; nessun worker permanente.
//! Il testo viene materializzato solo dentro spawn_blocking.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};

use futures::{Future, FutureExt, Stream, future::BoxFuture};
use lume_core::{DocId, SaveSnapshot};
use lume_notes::{LoadedNote, NoteId, NoteStore};
use sha2::{Digest, Sha256};

mod as_path;
pub(crate) mod gate;
mod job;
pub(crate) use as_path::identity;
pub(crate) use job::{Event, Job, Ready, Receipt, completed};

pub(crate) struct Picker {
    pub sequence: u64,
    pub document: DocId,
    pub lease: crate::scritture::Lease,
    pub job: PickerJob,
    pub source: Option<PathBuf>,
    pub root: Option<PathBuf>,
    pub root_epoch: u64,
    pub close_epoch: u64,
    pub ending: lume_core::LineEnding,
    pub bom: bool,
    pub snapshot: SaveSnapshot,
}

pub(crate) struct Rebind {
    pub ticket: Ticket,
    pub lease: crate::scritture::Lease,
    pub source: Option<PathBuf>,
    pub root: Option<PathBuf>,
    pub root_epoch: u64,
    pub mapping: Vec<(DocId, PathBuf)>,
    pub close_epoch: u64,
}

/// Il risultato è conservato dall'app anche se la Task perde il suo output.
#[derive(Clone)]
pub(crate) struct PickerEvent {
    sequence: u64,
    path: Option<PathBuf>,
    abbandonato: bool,
    owner: Weak<PickerInner>,
}

impl fmt::Debug for PickerEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PickerEvent")
            .field("sequence", &self.sequence)
            .finish_non_exhaustive()
    }
}

impl PickerEvent {
    pub(crate) fn into_parts(self) -> (u64, Option<PathBuf>, bool) {
        (self.sequence, self.path, self.abbandonato)
    }
}

type PickerFuture = futures::future::Shared<BoxFuture<'static, Option<PathBuf>>>;

enum PickerPhase {
    Pending(Option<BoxFuture<'static, Option<PathBuf>>>),
    Running(PickerFuture),
    Done(PickerEvent),
}

struct PickerInner {
    sequence: u64,
    phase: Mutex<PickerPhase>,
    wake: tokio::sync::Notify,
}

/// La recipe raccoglie lo stesso dialogo avviato dalla Task, mai un secondo.
#[derive(Clone)]
pub(crate) struct PickerJob(Arc<PickerInner>);

impl PartialEq for PickerJob {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for PickerJob {}
impl Hash for PickerJob {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

impl PickerJob {
    pub(crate) fn new(sequence: u64, directory: Option<PathBuf>, filename: Option<String>) -> Self {
        let future = async move {
            let mut dialog = rfd::AsyncFileDialog::new();
            if let Some(directory) = directory {
                dialog = dialog.set_directory(directory);
            }
            if let Some(filename) = filename {
                dialog = dialog.set_file_name(filename);
            }
            dialog
                .save_file()
                .await
                .map(|handle| handle.path().to_path_buf())
        }
        .boxed();
        Self(Arc::new(PickerInner {
            sequence,
            phase: Mutex::new(PickerPhase::Pending(Some(future))),
            wake: tokio::sync::Notify::new(),
        }))
    }

    fn start(&self) {
        let mut phase = self
            .0
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let PickerPhase::Pending(future) = &mut *phase
            && let Some(future) = future.take()
        {
            *phase = PickerPhase::Running(future.shared());
            self.0.wake.notify_waiters();
        }
    }

    fn abandon_pending(&self) {
        let mut phase = self
            .0
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(&*phase, PickerPhase::Pending(_)) {
            *phase = PickerPhase::Done(PickerEvent {
                sequence: self.0.sequence,
                path: None,
                abbandonato: true,
                owner: Arc::downgrade(&self.0),
            });
            self.0.wake.notify_waiters();
        }
    }

    async fn collect(&self) -> PickerEvent {
        loop {
            let notified = self.0.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let future = {
                let phase = self
                    .0
                    .phase
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match &*phase {
                    PickerPhase::Pending(_) => None,
                    PickerPhase::Running(future) => Some(future.clone()),
                    PickerPhase::Done(event) => return event.clone(),
                }
            };
            if let Some(future) = future {
                let event = PickerEvent {
                    sequence: self.0.sequence,
                    path: future.await,
                    abbandonato: false,
                    owner: Arc::downgrade(&self.0),
                };
                {
                    let mut phase = self
                        .0
                        .phase
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *phase = PickerPhase::Done(event.clone());
                }
                self.0.wake.notify_waiters();
                return event;
            }
            notified.await;
        }
    }

    pub(crate) fn delivery(&self) -> PickerDelivery {
        PickerDelivery {
            job: self.clone(),
            future: None,
        }
    }

    pub(crate) fn matches(&self, event: &PickerEvent) -> bool {
        event.sequence == self.0.sequence
            && Weak::ptr_eq(&event.owner, &Arc::downgrade(&self.0))
            && matches!(
                &*self
                    .0
                    .phase
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                PickerPhase::Done(_)
            )
    }
}

pub(crate) struct PickerDelivery {
    job: PickerJob,
    future: Option<BoxFuture<'static, PickerEvent>>,
}
impl Future for PickerDelivery {
    type Output = PickerEvent;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.future.is_none() {
            this.job.start();
            let job = this.job.clone();
            this.future = Some(async move { job.collect().await }.boxed());
        }
        match this.future.as_mut() {
            Some(future) => future.as_mut().poll(cx),
            None => Poll::Pending,
        }
    }
}
impl Drop for PickerDelivery {
    fn drop(&mut self) {
        self.job.abandon_pending();
    }
}

pub(crate) fn picker_completed(job: &PickerJob) -> impl Stream<Item = PickerEvent> + Send + use<> {
    let job = job.clone();
    futures::stream::once(async move { job.collect().await })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket {
    pub sequence: u64,
    pub document: DocId,
}

/// Autentica la ricevuta I/O, senza decidere ammissione o avanzamento.
#[derive(Debug, Default)]
pub struct Autenticazione {
    active: Option<Ticket>,
    sequence: u64,
}

impl Autenticazione {
    pub fn start(&mut self, document: DocId) -> Ticket {
        debug_assert!(self.active.is_none());
        self.sequence += 1;
        let ticket = Ticket {
            sequence: self.sequence,
            document,
        };
        self.active = Some(ticket);
        ticket
    }

    pub fn finish(&mut self, ticket: Ticket) -> bool {
        if self.active != Some(ticket) {
            return false;
        }
        self.active = None;
        true
    }

    pub fn active_document(&self) -> Option<DocId> {
        self.active.map(|ticket| ticket.document)
    }
}

pub struct NoteSave {
    pub scope: u64,
    pub id: NoteId,
    pub store: NoteStore,
    /// Solo header/identità/path; niente body o original_bytes copiati in UI.
    pub loaded: LoadedNote,
}

pub struct Request {
    pub ticket: Ticket,
    pub path: PathBuf,
    pub snapshot: SaveSnapshot,
    pub note: Option<NoteSave>,
    /// Risoluzione degli alias solo sul worker; None conserva il save ordinario.
    pub aliases: Option<Vec<(DocId, PathBuf)>>,
    /// Impronta dei byte letti all'apertura per i save ordinari.
    pub expected_fingerprint: Option<[u8; 32]>,
}

#[derive(Clone)]
pub struct SavedNote {
    pub scope: u64,
    pub id: NoteId,
    pub loaded: LoadedNote,
    pub rename_error: Option<String>,
}

#[derive(Clone)]
pub struct Completion {
    pub ticket: Ticket,
    pub path: PathBuf,
    pub snapshot: SaveSnapshot,
    pub result: Result<Option<SavedNote>, String>,
    /// Impronta dei byte pubblicati, calcolata sul worker insieme al payload.
    pub fingerprint: Option<[u8; 32]>,
    /// Destinazione effettiva SaveAs, distinta dal nome scelto.
    pub resolved: Option<PathBuf>,
    /// Il file è pubblicato, ma la sincronizzazione della directory è
    /// fallita: da mostrare in status bar, non è un salvataggio fallito.
    pub warning: Option<String>,
}

impl fmt::Debug for Completion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaveCompletion")
            .field("ticket", &self.ticket)
            .field("snapshot", &self.snapshot)
            .field("riuscito", &self.result.is_ok())
            .finish_non_exhaustive()
    }
}

impl Request {
    // Fixture diretta per i test storici; produzione passa dall'owner Job.
    #[cfg(test)]
    pub async fn run(self) -> Completion {
        self.run_on(tokio::runtime::Handle::current(), None).await
    }

    async fn run_on(
        self,
        runtime: tokio::runtime::Handle,
        gate: Option<job::WorkerGate>,
    ) -> Completion {
        let ticket = self.ticket;
        let path = self.path.clone();
        let snapshot = self.snapshot.clone();
        let output = runtime.spawn_blocking(move || {
            if let Some(mut note) = self.note {
                note.loaded.note.markdown = self.snapshot.normalized_text();
                let title = lume_notes::title_from_markdown(&note.loaded.note.markdown)
                    .filter(|title| *title != note.loaded.note.header.title);
                let request_path = self.path.clone();
                let request_snapshot = self.snapshot.clone();
                let request_ticket = self.ticket;
                let mut commit_started = false;
                let saved = if gate.is_some() {
                    note.store.save_with_title_authorized(
                        &note.loaded,
                        title.as_deref().unwrap_or(&note.loaded.note.header.title),
                        |resolved| {
                            if commit_started {
                                return true;
                            }
                            let allowed = gate.as_ref().is_some_and(|gate| {
                                gate.ready(job::Metadata {
                                    ticket: request_ticket,
                                    path: request_path.clone(),
                                    snapshot: request_snapshot.clone(),
                                    resolved: resolved.to_path_buf(),
                                    note: Some(job::NoteAuthority {
                                        scope: note.scope,
                                        id: note.id.clone(),
                                    }),
                                })
                            });
                            commit_started = allowed
                                && gate
                                    .as_ref()
                                    .is_some_and(job::WorkerGate::begin_commit);
                            commit_started
                        },
                    )
                } else if let Some(title) = title.as_deref() {
                    note.store.save_with_title(&note.loaded, title)
                } else {
                    // Stesso titolo: salva senza rinominare, ma con l'esito
                    // della sincronizzazione della cartella.
                    note.store
                        .save_with_title(&note.loaded, &note.loaded.note.header.title)
                };
                if let Some(gate) = &gate {
                    gate.done();
                }
                saved
                    .map(|result| {
                        // L'impronta è dei byte del file riletto dopo la
                        // pubblicazione: è lei a riconoscere l'eco del
                        // watcher della nota senza una revisione undo finta.
                        let fingerprint: Option<[u8; 32]> =
                            result.loaded.read_only_error.is_none().then(|| {
                                Sha256::digest(&result.loaded.original_bytes).into()
                            });
                        let warning = result.sync_error.map(|error| {
                            format!(
                                "Appunto {} salvato, ma la cartella non è stata sincronizzata sul disco: {error}",
                                result.loaded.path().display()
                            )
                        });
                        (Some(SavedNote {
                            scope: note.scope,
                            id: note.id,
                            loaded: result.loaded,
                            rename_error: result.rename_error.map(|error| error.to_string()),
                        }), None, fingerprint, warning)
                    })
                    .map_err(|error| {
                        format!(
                            "Salvataggio della nota {} fallito: {error}",
                            self.path.display()
                        )
                    })
            } else {
                let resolved = self.aliases.as_ref().map(|opened| {
                    as_path::destination(&self.path, self.ticket.document, opened)
                }).transpose()?;
                let payload = self.snapshot.payload();
                let fingerprint: [u8; 32] = Sha256::digest(&payload).into();
                if let Some(destination) = &resolved {
                    // Il nome già risolto non concede un nuovo attraversamento
                    // del leaf. La fotografia del target e i due confronti
                    // restano sul worker fino al commit.
                    let expected = lume_core::file_write::fingerprint_if_regular(destination)
                        .map_err(|error| format!("Destinazione {} non verificabile: {error}", destination.display()))?;
                    let prepared = lume_core::file_write::prepare_atomic_resolved_if_unchanged(
                        destination,
                        &payload,
                        expected,
                    )
                        .map_err(|e| format!("Preparazione di {} fallita: {e}", self.path.display()))?;
                    let allowed = gate.as_ref().is_some_and(|gate| gate.ready(job::Metadata {
                        ticket: self.ticket,
                        path: self.path.clone(),
                        snapshot: self.snapshot.clone(),
                        resolved: destination.clone(),
                        note: None,
                    }));
                    let result = if allowed {
                        // Anche dopo il permesso, ri-validare gli alias reali:
                        // il modello cooperativo non congela attori filesystem.
                        as_path::publish_authorized(prepared, &self.path, self.ticket.document,
                            self.aliases.as_deref().unwrap_or_default(), destination,
                            || gate.as_ref().is_some_and(job::WorkerGate::begin_commit))
                    } else {
                        match prepared.abort() {
                            Ok(()) => Err("Salvataggio con nome revocato prima del commit".into()),
                            Err(error) => Err(format!("Salvataggio con nome revocato; pulizia del preparato fallita: {error}")),
                        }
                    };
                    if let Some(gate) = &gate { gate.done(); }
                    result
                } else {
                    // I save normali seguono ancora i link manuali e
                    // rivalidano l'impronta. Il worker prepara prima il file;
                    // la UI può revocare senza pubblicare fino a CommitStarted.
                    let prepared = match self.expected_fingerprint {
                        Some(expected) => lume_core::file_write::prepare_atomic_if_unchanged(
                            &self.path,
                            &payload,
                            expected,
                        ),
                        None => lume_core::file_write::prepare_atomic(&self.path, &payload),
                    }
                    .map_err(|error| {
                        format!("Preparazione di {} fallita: {error}", self.path.display())
                    });
                    match (prepared, gate.as_ref()) {
                        (Ok(prepared), Some(gate)) => {
                            let allowed = gate.ready(job::Metadata {
                                ticket: self.ticket,
                                path: self.path.clone(),
                                snapshot: self.snapshot.clone(),
                                resolved: self.path.clone(),
                                note: None,
                            });
                            let result = if allowed && gate.begin_commit() {
                                commit_with_warning(prepared, &self.path)
                            } else {
                                match prepared.abort() {
                                    Ok(()) => Err("Salvataggio revocato prima del commit".to_string()),
                                    Err(error) => Err(format!(
                                        "Salvataggio revocato; pulizia del preparato fallita: {error}"
                                    )),
                                }
                            };
                            gate.done();
                            result
                        }
                        (Ok(prepared), None) => commit_with_warning(prepared, &self.path),
                        (Err(error), _) => Err(error),
                    }
                }
                .map(|warning| (None, resolved, Some(fingerprint), warning))
            }
        })
        .await
        .unwrap_or_else(|error| Err(format!("Il worker di salvataggio è terminato: {error}; esito del file da verificare, nessun rollback")));
        let (result, resolved, fingerprint, warning) = match output {
            Ok((note, resolved, fingerprint, warning)) => {
                (Ok(note), resolved, fingerprint, warning)
            }
            Err(error) => (Err(error), None, None, None),
        };
        Completion {
            ticket,
            path,
            snapshot,
            result,
            fingerprint,
            resolved,
            warning,
        }
    }
}

/// Pubblica il file; l'errore di sincronizzazione della directory, a rename
/// avvenuto, diventa un avviso e non un fallimento del salvataggio.
pub(super) fn commit_with_warning(
    prepared: lume_core::file_write::PreparedWrite,
    path: &Path,
) -> Result<Option<String>, String> {
    prepared
        .commit_reporting()
        .map(|warning| {
            warning.map(|error| {
                format!(
                    "{} salvato, ma la cartella non è stata sincronizzata sul disco: {error}",
                    path.display()
                )
            })
        })
        .map_err(|error| format!("Salvataggio di {} fallito: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_coda_fonde_duplicati_e_non_sovrappone_lavori() {
        use crate::scritture::{Causa, Coordinatore, Esito, Op, Permesso};
        let mut q = Coordinatore::default();
        let salva = |document, versione| Op::Salva {
            doc: document,
            versione,
            causa: Causa::Utente,
        };
        let Ok(Permesso::Avvia(ticket)) = q.ammetti(salva(DocId(1), 0)) else {
            panic!("primo save");
        };
        let _ = q.seguito();
        for version in 0..1000 {
            assert_eq!(q.ammetti(salva(DocId(1), version)), Ok(Permesso::Accodato));
        }
        assert_eq!(q.ammetti(salva(DocId(2), 0)), Ok(Permesso::Accodato));
        q.rilascia(ticket, Esito::Riuscito);
        let primo = q.seguito();
        assert!(matches!(
            primo.avvia.first(),
            Some((
                _,
                Op::Salva {
                    doc: DocId(1),
                    versione: 999,
                    ..
                }
            ))
        ));
        let Some((secondo, _)) = primo.avvia.first() else {
            panic!("secondo save assente");
        };
        q.rilascia(ticket, Esito::Riuscito);
        assert!(q.possiede(*secondo, crate::scritture::Tipo::Salvataggio));
        q.rilascia(*secondo, Esito::Riuscito);
        let prossimo = q.seguito();
        assert!(matches!(
            prossimo.avvia.first(),
            Some((
                _,
                Op::Salva {
                    doc: DocId(2),
                    versione: 0,
                    ..
                }
            ))
        ));
        assert!(!prossimo.quiete);
        let Some((ultimo, _)) = prossimo.avvia.first() else {
            panic!("ultimo save assente");
        };
        q.rilascia(*ultimo, Esito::Riuscito);
        assert!(q.seguito().quiete);
    }
}
