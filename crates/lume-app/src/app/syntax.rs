//! Consegna sintattica: le guardie si confrontano sempre con workspace vivo.
use super::*;
use lume_ui::editor::{syntax_delivery::SyntaxIdentity, syntax_subscription::SyntaxEvent};

#[derive(Clone, Debug)]
pub struct ViewportRequest {
    pub group: crate::editor_groups::GroupId,
    pub document: DocId,
    pub epoch: u64,
    pub revision: u64,
    pub window: std::ops::Range<usize>,
}

impl Lume {
    pub(super) fn syntax_viewport(&mut self, request: ViewportRequest) -> Task<Message> {
        if self.search_exit.is_some()
            || request.epoch != self.git_root_epoch
            || self.groups.active_in(request.group) != Some(request.document)
        {
            return Task::none();
        }
        let Some(doc) = self.workspace.get(request.document) else {
            return Task::none();
        };
        if doc.version() != request.revision
            || request.window.start > request.window.end
            || request.window.end > doc.rope().len_bytes()
        {
            return Task::none();
        }
        let Some(language) = self.syntax.get(&request.document).map(SyntaxLanguage::lang) else {
            return Task::none();
        };
        let lane = match request.group {
            crate::editor_groups::GroupId::Primary => lume_syntax::SyntaxLane::Primary,
            crate::editor_groups::GroupId::Secondary => lume_syntax::SyntaxLane::Secondary,
        };
        let edit = doc
            .last_edit()
            .zip(request.revision.checked_sub(1))
            .map(|(edit, base)| {
                (
                    base,
                    InputEditRaw {
                        start_byte: edit.start_byte,
                        old_end_byte: edit.old_end_byte,
                        new_end_byte: edit.new_end_byte,
                        start_point: edit.start_point,
                        old_end_point: edit.old_end_point,
                        new_end_point: edit.new_end_point,
                    },
                )
            });
        let job = lume_syntax::SyntaxRequest {
            document: request.document.0,
            epoch: request.epoch,
            revision: request.revision,
            language,
            source: doc.rope().clone(),
            window: request.window,
            edit,
        };
        self.syntax_reconcile_groups();
        let owner = self.syntax_pool.receiver(lane);
        match self.syntax_pool.submit(lane, job.clone()) {
            Ok(ticket) => {
                self.syntax_expected.insert(
                    lane,
                    lume_ui::editor::syntax_delivery::SyntaxRequestGuard::new(owner, ticket, &job),
                );
                self.syntax_documents.insert(lane, request.document);
                if self
                    .syntax_snapshots
                    .get(&lane)
                    .is_some_and(|(id, epoch, snapshot)| {
                        *id != request.document
                            || *epoch != request.epoch
                            || snapshot.language() != language
                            || snapshot.revision() != request.revision
                    })
                {
                    self.syntax_snapshots.remove(&lane);
                }
                Task::none()
            }
            Err(error) => self.notify_error(format!("Avvio sintassi: {error}")),
        }
    }

    /// Una lane conserva solo il documento effettivamente mostrato dal gruppo.
    pub(super) fn syntax_reconcile_groups(&mut self) {
        use crate::editor_groups::GroupId;
        use lume_syntax::SyntaxLane;
        for (lane, group) in [
            (SyntaxLane::Primary, GroupId::Primary),
            (SyntaxLane::Secondary, GroupId::Secondary),
        ] {
            let active = self.groups.active_in(group);
            let matches = self
                .syntax_expected
                .get(&lane)
                .map(|guard| active.is_some_and(|id| guard.targets(id.0)))
                .unwrap_or_else(|| {
                    self.syntax_documents
                        .get(&lane)
                        .copied()
                        .or_else(|| self.syntax_snapshots.get(&lane).map(|(id, _, _)| *id))
                        == active
                });
            let owned = self.syntax_expected.contains_key(&lane)
                || self.syntax_documents.contains_key(&lane)
                || self.syntax_snapshots.contains_key(&lane);
            if owned && !matches {
                self.syntax_expected.remove(&lane);
                self.syntax_documents.remove(&lane);
                self.syntax_snapshots.remove(&lane);
                if let Err(error) = self.syntax_pool.revoke(lane) {
                    let _ = self.notify_error(format!("Revoca sintassi gruppo: {error}"));
                }
            }
        }
    }

    /// Riusa i due thread fissi: revoca lavoro e cache nel loro owner.
    /// La radice non possiede thread separati; il join resta all'uscita app.
    pub(super) fn syntax_revoke_root(&mut self) {
        self.syntax_expected.clear();
        self.syntax_documents.clear();
        self.syntax_snapshots.clear();
        for lane in [
            lume_syntax::SyntaxLane::Primary,
            lume_syntax::SyntaxLane::Secondary,
        ] {
            if let Err(error) = self.syntax_pool.revoke(lane) {
                let _ = self.notify_error(format!("Revoca sintassi radice: {error}"));
            }
        }
    }

    pub(super) fn syntax_forget_document(&mut self, id: DocId) {
        for lane in [
            lume_syntax::SyntaxLane::Primary,
            lume_syntax::SyntaxLane::Secondary,
        ] {
            // Una guardia nuova prevale sul vecchio albero/proiezione della lane.
            let revoke = self
                .syntax_expected
                .get(&lane)
                .map(|guard| guard.targets(id.0))
                .unwrap_or_else(|| self.syntax_documents.get(&lane) == Some(&id));
            if revoke {
                self.syntax_expected.remove(&lane);
                self.syntax_documents.remove(&lane);
                if let Err(error) = self.syntax_pool.revoke(lane) {
                    let _ = self.notify_error(format!("Revoca sintassi: {error}"));
                }
            }
            if self
                .syntax_snapshots
                .get(&lane)
                .is_some_and(|(document, _, _)| *document == id)
            {
                self.syntax_snapshots.remove(&lane);
            }
        }
    }

    pub(super) fn syntax_event(&mut self, event: SyntaxEvent) -> Task<Message> {
        let (lane, owner) = event.origin();
        if *owner != self.syntax_pool.receiver(lane) {
            return Task::none();
        }
        match event {
            SyntaxEvent::Stopped(_, _) => self.notify_error(
                "Il worker sintattico è terminato; gli highlight non sono disponibili".into(),
            ),
            SyntaxEvent::Complete(_, owner, completion) => {
                let id = DocId(completion.document);
                let Some(document) = self.workspace.get(id) else {
                    return Task::none();
                };
                let Some(language) = self.syntax.get(&id).map(SyntaxLanguage::lang) else {
                    return Task::none();
                };
                let current = SyntaxIdentity {
                    document: id.0,
                    epoch: self.git_root_epoch,
                    revision: document.version(),
                    language,
                };
                if !self
                    .syntax_expected
                    .get(&lane)
                    .is_some_and(|guard| guard.accepts(&owner, &completion, current))
                {
                    return Task::none();
                }
                self.syntax_expected.remove(&lane);
                self.syntax_documents.insert(lane, id);
                if let Some(result) = completion.result {
                    self.syntax_snapshots
                        .insert(lane, (id, current.epoch, result.snapshot));
                    Task::none()
                } else {
                    self.syntax_snapshots.remove(&lane);
                    self.notify_error(
                        "Gli highlight sintattici non sono disponibili per questa revisione".into(),
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn viewport_expansion_preserves_current_partial_snapshot() -> Result<(), &'static str> {
        use crate::editor_groups::GroupId;
        use lume_syntax::{LanguageId, SyntaxLane, SyntaxState};
        let (mut app, _) = Lume::new_without_store();
        let id = app.workspace.open(None, "fn first() {}\nfn second() {}\n");
        app.groups.open(GroupId::Primary, id);
        app.syntax.insert(id, SyntaxLanguage(LanguageId::Rust));
        let doc = app.workspace.get(id).ok_or("doc")?;
        let revision = doc.version();
        let end = doc.rope().len_bytes();
        let snapshot = SyntaxState::new(LanguageId::Rust, doc.rope().slice(..))
            .ok_or("parser")?
            .highlight_snapshot(doc.rope().slice(..), revision, 3..12)
            .ok_or("snapshot")?;
        app.syntax_snapshots.insert(
            SyntaxLane::Primary,
            (id, app.git_root_epoch, snapshot.clone()),
        );
        drop(app.syntax_viewport(ViewportRequest {
            group: GroupId::Primary,
            document: id,
            epoch: app.git_root_epoch,
            revision,
            window: 0..end,
        }));
        let retained = &app
            .syntax_snapshots
            .get(&SyntaxLane::Primary)
            .ok_or("partial snapshot discarded")?
            .2;
        assert!(retained.same_projection(&snapshot));
        assert!(!retained.covers(LanguageId::Rust, revision, 0..end));
        let source = app.workspace.get(id).ok_or("doc")?.rope().clone();
        let stale = SyntaxState::new(LanguageId::Rust, source.slice(..))
            .ok_or("parser")?
            .highlight_snapshot(source.slice(..), revision.wrapping_add(1), 3..12)
            .ok_or("stale snapshot")?;
        app.syntax_snapshots
            .insert(SyntaxLane::Primary, (id, app.git_root_epoch, stale));
        drop(app.syntax_viewport(ViewportRequest {
            group: GroupId::Primary,
            document: id,
            epoch: app.git_root_epoch,
            revision,
            window: 0..end,
        }));
        assert!(!app.syntax_snapshots.contains_key(&SyntaxLane::Primary));
        Ok(())
    }
    use super::*;
    use lume_syntax::{LanguageId, SyntaxCompletion, SyntaxLane, SyntaxRequest, SyntaxResult};
    use lume_ui::editor::syntax_delivery::SyntaxRequestGuard;

    #[test]
    fn split_revokes_the_lane_that_no_longer_displays_its_document() -> Result<(), &'static str> {
        use crate::editor_groups::GroupId;
        let (mut app, _) = Lume::new_without_store();
        let id = app.workspace.open(None, "fn main() {}\n");
        app.groups.open(GroupId::Primary, id);
        let doc = app.workspace.get(id).ok_or("doc")?;
        let request = SyntaxRequest {
            document: id.0,
            epoch: app.git_root_epoch,
            revision: doc.version(),
            language: LanguageId::Rust,
            source: doc.rope().clone(),
            window: 0..doc.rope().len_bytes(),
            edit: None,
        };
        app.syntax_expected.insert(
            SyntaxLane::Primary,
            SyntaxRequestGuard::new(app.syntax_pool.receiver(SyntaxLane::Primary), 1, &request),
        );
        app.syntax_documents.insert(SyntaxLane::Primary, id);
        drop(app.update(Message::MenuAction(MenuAct::SplitEditor)));
        assert_eq!(app.groups.active_in(GroupId::Secondary), Some(id));
        assert!(!app.syntax_expected.contains_key(&SyntaxLane::Primary));
        assert!(!app.syntax_documents.contains_key(&SyntaxLane::Primary));
        app.syntax_expected.insert(
            SyntaxLane::Secondary,
            SyntaxRequestGuard::new(app.syntax_pool.receiver(SyntaxLane::Secondary), 2, &request),
        );
        app.syntax_documents.insert(SyntaxLane::Secondary, id);
        let other = app.workspace.open(None, "fn other() {}\n");
        app.groups.open(GroupId::Primary, other);
        app.activate_editor_tab(other);
        assert!(
            app.syntax_expected.contains_key(&SyntaxLane::Secondary),
            "peer attivo preservato"
        );
        drop(app.update(Message::MenuAction(MenuAct::UnsplitEditor)));
        assert!(!app.syntax_expected.contains_key(&SyntaxLane::Secondary));
        assert!(!app.syntax_documents.contains_key(&SyntaxLane::Secondary));
        assert_eq!(app.syntax_pool.started_threads(), 0);
        Ok(())
    }

    #[test]
    fn cold_exit_collector_survives_runtime_loss_and_drops_with_last_app_owner()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (app, weak, receivers) = runtime.block_on(async {
            let (mut app, _) = Lume::new_without_store();
            let mut receivers = Vec::new();
            for lane in [SyntaxLane::Primary, SyntaxLane::Secondary] {
                let source = ropey::Rope::from_str(&"fn main() {}\n".repeat(20_000));
                receivers.push(app.syntax_pool.receiver(lane));
                app.syntax_pool.submit(
                    lane,
                    SyntaxRequest {
                        document: 1,
                        epoch: app.git_root_epoch,
                        revision: 1,
                        language: LanguageId::Rust,
                        window: 0..source.len_bytes(),
                        source,
                        edit: None,
                    },
                )?;
            }
            assert_eq!(app.syntax_pool.started_threads(), 2);
            let task = app.esci(Some(iced::window::Id::unique()));
            let syntax = app
                .search_exit
                .as_ref()
                .ok_or("exit")?
                .syntax
                .as_ref()
                .ok_or("syntax collector")?;
            assert!(syntax.peek().is_none(), "collector non eseguito");
            let weak = syntax.downgrade().ok_or("weak collector")?;
            drop(task);
            Ok::<_, Box<dyn std::error::Error>>((app, weak, receivers))
        })?;
        drop(runtime);
        assert!(
            weak.upgrade().is_some(),
            "app conserva il collector senza runtime"
        );
        let waker = futures::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        for receiver in receivers {
            // Una completion prodotta prima dello shutdown può essere ancora
            // nella mailbox; dopo quella lettura il receiver deve essere chiuso.
            let _ = receiver.poll_result(&mut cx);
            assert!(matches!(
                receiver.poll_result(&mut cx),
                std::task::Poll::Ready(None)
            ));
        }
        drop(app);
        assert!(
            weak.upgrade().is_none(),
            "ultimo owner rilascia future e collector"
        );
        Ok(())
    }

    #[test]
    fn dispatched_syntax_collector_survives_runtime_shutdown_queue()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::{sync::mpsc, time::Duration};
        struct ShutdownGuard {
            release: Option<mpsc::Sender<()>>,
            shutdown: Option<std::thread::JoinHandle<()>>,
        }
        impl Drop for ShutdownGuard {
            fn drop(&mut self) {
                if let Some(release) = self.release.take() {
                    let _ = release.send(());
                }
                if let Some(shutdown) = self.shutdown.take() {
                    let _ = shutdown.join();
                }
            }
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()?;
        let (release_tx, release_rx) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let mut guard = ShutdownGuard {
            release: Some(release_tx),
            shutdown: None,
        };
        let (mut app, completion, blocker) = runtime.block_on(async {
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = entered_tx.send(());
                let _ = release_rx.recv();
            });
            entered_rx.recv_timeout(Duration::from_secs(5))?;
            let (mut app, _) = Lume::new_without_store();
            let source = ropey::Rope::from_str("fn main() {}\n");
            app.syntax_pool.submit(
                SyntaxLane::Primary,
                SyntaxRequest {
                    document: 1,
                    epoch: app.git_root_epoch,
                    revision: 1,
                    language: LanguageId::Rust,
                    window: 0..source.len_bytes(),
                    source,
                    edit: None,
                },
            )?;
            drop(app.esci(Some(iced::window::Id::unique())));
            let exit = app.search_exit.as_ref().ok_or("exit")?;
            let completion = exit.completion.clone();
            let mut collect = Box::pin(completion.collect());
            assert!(futures::poll!(collect.as_mut()).is_pending());
            assert!(
                exit.syntax
                    .as_ref()
                    .ok_or("syntax collector")?
                    .peek()
                    .is_none()
            );
            drop(collect);
            Ok::<_, Box<dyn std::error::Error>>((app, completion, blocker))
        })?;
        let epoch = app.close_epoch;
        drop(app.update(Message::SearchExitReady(epoch, true)));
        assert!(app.search_exit.is_some(), "ACK anticipato rifiutato");
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        guard.shutdown = Some(std::thread::spawn(move || {
            drop(runtime);
            let _ = shutdown_tx.send(());
        }));
        assert!(matches!(
            shutdown_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        guard.release.take().ok_or("release")?.send(())?;
        shutdown_rx.recv_timeout(Duration::from_secs(5))?;
        guard
            .shutdown
            .take()
            .ok_or("shutdown")?
            .join()
            .map_err(|_| "shutdown panic")?;
        let replacement = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        assert!(replacement.block_on(completion.collect()));
        replacement.block_on(blocker)?;
        drop(app.update(Message::SearchExitReady(epoch, true)));
        assert!(app.search_exit.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn two_language_documents_deliver_independent_oracles_through_app_recipes()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::editor_groups::GroupId;
        use futures::StreamExt;
        use iced::advanced::subscription::{Hasher, Recipe, into_recipes};
        use std::hash::Hasher as _;
        let hash = |recipe: &dyn Recipe<Output = Message>| {
            let mut hasher = Hasher::default();
            recipe.hash(&mut hasher);
            hasher.finish()
        };
        let (mut app, _) = Lume::new_without_store();
        let rust = app
            .workspace
            .open(None, "// città 😀\nfn rust_doc() { let x = 42; }\n");
        let toml = app
            .workspace
            .open(None, "[document]\nname = \"città 😀\"\nenabled = true\n");
        app.groups.open(GroupId::Primary, rust);
        app.groups.open(GroupId::Primary, toml);
        assert!(app.groups.split());
        let cases = [
            (
                SyntaxLane::Primary,
                GroupId::Primary,
                rust,
                LanguageId::Rust,
            ),
            (
                SyntaxLane::Secondary,
                GroupId::Secondary,
                toml,
                LanguageId::Toml,
            ),
        ];
        let mut oracles = Vec::new();
        for (lane, group, id, language) in cases {
            app.syntax.insert(id, SyntaxLanguage(language));
            let doc = app.workspace.get(id).ok_or("doc")?;
            let window = 0..doc.rope().len_bytes();
            let oracle = lume_syntax::SyntaxState::new(language, doc.rope().slice(..))
                .ok_or("oracle parse")?
                .highlight_snapshot(doc.rope().slice(..), doc.version(), window.clone())
                .ok_or("oracle snapshot")?;
            let request = ViewportRequest {
                group,
                document: id,
                epoch: app.git_root_epoch,
                revision: doc.version(),
                window,
            };
            oracles.push((lane, id, oracle));
            drop(app.update(Message::SyntaxViewport(request)));
        }
        assert_eq!(app.syntax_pool.started_threads(), 2);
        for (lane, id, oracle) in oracles {
            let expected = lume_ui::editor::syntax_subscription::subscription(
                lane,
                app.syntax_pool.receiver(lane),
            )
            .map(Message::Syntax);
            let expected = into_recipes(expected).pop().ok_or("expected recipe")?;
            let actual = into_recipes(app.subscription())
                .into_iter()
                .find(|recipe| hash(&**recipe) == hash(&*expected))
                .ok_or("actual recipe")?;
            let mut stream = actual.stream(Box::pin(futures::stream::empty()));
            let message = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                .await?
                .ok_or("completion")?;
            drop(app.update(message));
            let (document, epoch, snapshot) = app
                .syntax_snapshots
                .get(&lane)
                .ok_or("installed snapshot")?;
            assert_eq!(*document, id);
            assert_eq!(*epoch, app.git_root_epoch);
            assert_eq!(snapshot.tokens(), oracle.tokens());
            assert_eq!(snapshot.language(), oracle.language());
        }
        let peer = app
            .syntax_snapshots
            .get(&SyntaxLane::Secondary)
            .ok_or("peer")?
            .2
            .clone();
        app.close_tab(rust);
        assert!(!app.syntax_snapshots.contains_key(&SyntaxLane::Primary));
        assert!(
            app.syntax_snapshots
                .get(&SyntaxLane::Secondary)
                .ok_or("peer preserved")?
                .2
                .same_projection(&peer)
        );
        drop(app.esci(Some(iced::window::Id::unique())));
        assert!(
            app.search_exit
                .as_ref()
                .ok_or("exit")?
                .completion
                .collect()
                .await
        );
        Ok(())
    }

    #[tokio::test]
    async fn app_exit_waits_for_owned_syntax_join_before_ack()
    -> Result<(), Box<dyn std::error::Error>> {
        use futures::StreamExt;
        let (mut app, _) = Lume::new_without_store();
        let id = app.workspace.open(None, "fn main() {}\n");
        let doc = app.workspace.get(id).ok_or("doc")?;
        app.syntax_pool.submit(
            SyntaxLane::Primary,
            SyntaxRequest {
                document: id.0,
                epoch: app.git_root_epoch,
                revision: doc.version(),
                language: LanguageId::Rust,
                source: doc.rope().clone(),
                window: 0..doc.rope().len_bytes(),
                edit: None,
            },
        )?;
        let task = app.esci(Some(iced::window::Id::unique()));
        let epoch = app.close_epoch;
        assert!(
            app.search_exit
                .as_ref()
                .is_some_and(|exit| exit.syntax.is_some())
        );
        let _ = app.update(Message::SearchExitReady(epoch, true));
        assert!(
            app.search_exit.is_some(),
            "ACK prima del poll del collector deve essere rifiutato"
        );
        drop(task); // nessuna seconda richiesta: la ricetta reale guida l'attesa
        use iced::advanced::subscription::{Hasher, Recipe, into_recipes};
        use std::hash::Hasher as _;
        let hash = |recipe: &dyn Recipe<Output = Message>| {
            let mut hasher = Hasher::default();
            recipe.hash(&mut hasher);
            hasher.finish()
        };
        let completion = app
            .search_exit
            .as_ref()
            .ok_or("exit guard")?
            .completion
            .clone();
        let expected =
            iced::Subscription::run_with(completion, crate::search_events::exit_completed)
                .map(Lume::exit_ready);
        let expected = into_recipes(expected).pop().ok_or("expected recipe")?;
        let actual = into_recipes(app.subscription())
            .into_iter()
            .find(|recipe| hash(&**recipe) == hash(&*expected))
            .ok_or("actual exit recipe")?;
        let mut stream = actual.stream(Box::pin(futures::stream::empty()));
        let message = stream.next().await.ok_or("exit output")?;
        assert!(matches!(message, Message::SearchExitReady(e, true) if e == epoch));
        let _close = app.update(message);
        assert!(app.search_exit.is_none());
        assert_eq!(app.syntax_pool.started_threads(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn viewport_admission_rejects_stale_and_delivers_current_request()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::editor_groups::GroupId;
        let (mut app, _) = Lume::new_without_store();
        let id = app.workspace.open(None, "fn main() {}\n");
        app.groups.open(GroupId::Primary, id);
        let doc = app.workspace.get(id).ok_or("doc")?;
        let state = SyntaxLanguage(LanguageId::Rust);
        let request = ViewportRequest {
            group: GroupId::Primary,
            document: id,
            epoch: app.git_root_epoch,
            revision: doc.version(),
            window: 0..doc.rope().len_bytes(),
        };
        app.syntax.insert(id, state);
        let mut wrong_epoch = request.clone();
        wrong_epoch.epoch = wrong_epoch.epoch.wrapping_add(1);
        let mut wrong_version = request.clone();
        wrong_version.revision = wrong_version.revision.wrapping_add(1);
        let mut wrong_group = request.clone();
        wrong_group.group = GroupId::Secondary;
        let mut wrong_range = request.clone();
        wrong_range.window.end += 1;
        for stale in [wrong_epoch, wrong_version, wrong_group, wrong_range] {
            let _ = app.update(Message::SyntaxViewport(stale));
            assert_eq!(app.syntax_pool.started_threads(), 0);
        }
        let _ = app.update(Message::SyntaxViewport(request.clone()));
        assert_eq!(app.syntax_pool.started_threads(), 1);
        assert!(app.syntax_expected.contains_key(&SyntaxLane::Primary));
        let receiver = app.syntax_pool.receiver(SyntaxLane::Primary);
        let completion = std::future::poll_fn(|cx| receiver.poll_result(cx))
            .await
            .ok_or("completion")?;
        let _ = app.update(Message::Syntax(SyntaxEvent::Complete(
            SyntaxLane::Primary,
            receiver,
            completion,
        )));
        assert!(app.syntax_snapshots.contains_key(&SyntaxLane::Primary));
        let exit = app.esci(Some(iced::window::Id::unique()));
        let _ = app.update(Message::SyntaxViewport(request));
        assert_eq!(
            app.syntax_pool.started_threads(),
            0,
            "nessuna ammissione durante uscita"
        );
        drop(exit);
        let completion = app.search_exit.as_ref().ok_or("exit")?.completion.clone();
        assert!(completion.collect().await);
        Ok(())
    }

    #[test]
    fn root_change_revokes_both_syntax_lanes_without_starting_workers() -> Result<(), &'static str>
    {
        let (mut app, _) = Lume::new_without_store();
        for lane in [SyntaxLane::Primary, SyntaxLane::Secondary] {
            let id = app.workspace.open(None, "fn main() {}\n");
            let doc = app.workspace.get(id).ok_or("document")?;
            let request = SyntaxRequest {
                document: id.0,
                epoch: app.git_root_epoch,
                revision: doc.version(),
                language: LanguageId::Rust,
                source: doc.rope().clone(),
                window: 0..doc.rope().len_bytes(),
                edit: None,
            };
            app.syntax_documents.insert(lane, id);
            app.syntax_expected.insert(
                lane,
                SyntaxRequestGuard::new(app.syntax_pool.receiver(lane), 1, &request),
            );
        }
        app.set_project_root(std::env::temp_dir());
        assert!(app.syntax_expected.is_empty());
        assert!(app.syntax_documents.is_empty());
        assert!(app.syntax_snapshots.is_empty());
        assert_eq!(app.syntax_pool.started_threads(), 0);
        Ok(())
    }

    #[test]
    fn closing_old_tab_preserves_new_request_in_same_lane_and_other_group()
    -> Result<(), &'static str> {
        let (mut app, _) = Lume::new_without_store();
        let old = app.workspace.open(None, "fn old() {}\n");
        let newer = app.workspace.open(None, "fn newer() {}\n");
        let peer = app.workspace.open(None, "fn peer() {}\n");
        app.syntax_documents.insert(SyntaxLane::Primary, old);
        app.syntax_documents.insert(SyntaxLane::Secondary, peer);
        for (lane, id) in [(SyntaxLane::Primary, newer), (SyntaxLane::Secondary, peer)] {
            let doc = app.workspace.get(id).ok_or("doc")?;
            let request = SyntaxRequest {
                document: id.0,
                epoch: app.git_root_epoch,
                revision: doc.version(),
                language: LanguageId::Rust,
                source: doc.rope().clone(),
                window: 0..doc.rope().len_bytes(),
                edit: None,
            };
            app.syntax_expected.insert(
                lane,
                SyntaxRequestGuard::new(app.syntax_pool.receiver(lane), 1, &request),
            );
        }
        app.close_tab(old);
        assert!(
            app.syntax_expected
                .get(&SyntaxLane::Primary)
                .is_some_and(|g| g.targets(newer.0))
        );
        assert!(
            app.syntax_expected
                .get(&SyntaxLane::Secondary)
                .is_some_and(|g| g.targets(peer.0))
        );
        app.close_tab(newer);
        assert!(!app.syntax_expected.contains_key(&SyntaxLane::Primary));
        assert!(!app.syntax_documents.contains_key(&SyntaxLane::Primary));
        assert!(app.syntax_expected.contains_key(&SyntaxLane::Secondary));
        assert_eq!(app.syntax_pool.started_threads(), 0);
        Ok(())
    }

    #[test]
    fn app_subscription_has_one_stable_recipe_per_syntax_lane() -> Result<(), &'static str> {
        use iced::advanced::subscription::{Hasher, Recipe, into_recipes};
        use std::hash::Hasher as _;
        let (app, _) = Lume::new_without_store();
        let hash = |recipe: &dyn Recipe<Output = Message>| {
            let mut hasher = Hasher::default();
            recipe.hash(&mut hasher);
            hasher.finish()
        };
        let actual = into_recipes(app.subscription());
        for lane in [SyntaxLane::Primary, SyntaxLane::Secondary] {
            let expected = lume_ui::editor::syntax_subscription::subscription(
                lane,
                app.syntax_pool.receiver(lane),
            )
            .map(Message::Syntax);
            let recipe = into_recipes(expected)
                .pop()
                .ok_or("missing expected recipe")?;
            assert_eq!(
                actual
                    .iter()
                    .filter(|r| hash(&***r) == hash(&*recipe))
                    .count(),
                1
            );
        }
        assert_eq!(app.syntax_pool.started_threads(), 0);
        Ok(())
    }

    #[test]
    fn app_syntax_rejects_old_failure_and_accepts_only_current_guard() -> Result<(), &'static str> {
        let (mut app, _) = Lume::new_without_store();
        let id = app.workspace.open(None, "fn main() {}\n");
        let doc = app.workspace.get(id).ok_or("doc")?;
        let state = SyntaxLanguage(LanguageId::Rust);
        let request = SyntaxRequest {
            document: id.0,
            epoch: app.git_root_epoch,
            revision: doc.version(),
            language: LanguageId::Rust,
            source: doc.rope().clone(),
            window: 0..doc.rope().len_bytes(),
            edit: None,
        };
        let snapshot = lume_syntax::SyntaxState::new(LanguageId::Rust, doc.rope().slice(..))
            .ok_or("parse oracle")?
            .highlight_snapshot(doc.rope().slice(..), doc.version(), request.window.clone())
            .ok_or("snapshot")?;
        app.syntax.insert(id, state);
        let owner = app.syntax_pool.receiver(SyntaxLane::Primary);
        app.syntax_expected.insert(
            SyntaxLane::Primary,
            SyntaxRequestGuard::new(owner.clone(), 2, &request),
        );
        let mut completion = SyntaxCompletion {
            ticket: 1,
            document: id.0,
            epoch: request.epoch,
            revision: request.revision,
            result: None,
        };
        let _ = app.update(Message::Syntax(SyntaxEvent::Complete(
            SyntaxLane::Primary,
            owner.clone(),
            completion.clone(),
        )));
        assert!(app.syntax_expected.contains_key(&SyntaxLane::Primary));
        assert!(app.syntax_snapshots.is_empty());
        completion.ticket = 2;
        completion.result = Some(SyntaxResult {
            document: id.0,
            epoch: request.epoch,
            snapshot,
        });
        let _ = app.update(Message::Syntax(SyntaxEvent::Complete(
            SyntaxLane::Primary,
            owner,
            completion,
        )));
        assert!(!app.syntax_expected.contains_key(&SyntaxLane::Primary));
        assert_eq!(
            app.syntax_snapshots
                .get(&SyntaxLane::Primary)
                .map(|(doc, _, _)| *doc),
            Some(id)
        );
        assert_eq!(app.syntax_pool.started_threads(), 0);
        app.close_tab(id);
        assert!(
            app.syntax_snapshots.is_empty(),
            "snapshot della scheda chiusa deve essere revocato"
        );
        Ok(())
    }
}
