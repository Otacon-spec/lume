//! Git: comandi, stato, differenze, blame, stash e cronologia.
//!
//! Metodi di `Lume`: lo stato e i messaggi restano in `app.rs`.

use super::*;

pub(super) fn snapshot_of(
    repository: &lume_git::GitRepository,
) -> Result<tools::git_native::Snapshot, String> {
    let merge_conflicts = if repository.merge_in_progress() {
        Some(
            repository
                .merge_conflicts()
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    let mut files = repository.status().map_err(|error| error.to_string())?;
    for path in merge_conflicts.iter().flatten() {
        if !files.iter().any(|file| &file.path == path) {
            files.push(lume_git::FileStatus {
                path: path.clone(),
                index: Some(lume_git::ChangeKind::Conflict),
                worktree: Some(lume_git::ChangeKind::Conflict),
            });
        }
    }
    Ok(tools::git_native::Snapshot {
        branch: repository
            .branch_name()
            .map_err(|error| error.to_string())?,
        local_branches: repository
            .branches()
            .map_err(|error| error.to_string())?
            .local
            .into_iter()
            .map(|branch| branch.name)
            .collect(),
        has_origin: repository.has_origin(),
        merge_conflicts,
        has_upstream: repository
            .upstream()
            .map(|upstream| upstream.is_some())
            .unwrap_or(false),
        files,
        commits: repository.log(20).map_err(|error| error.to_string())?,
        head_message: repository
            .head_message()
            .map_err(|error| error.to_string())?,
    })
}

impl Lume {
    fn git_action_requires_clean_buffers(action: &lume_ui::tool::GitAction) -> bool {
        matches!(
            action,
            lume_ui::tool::GitAction::Pull
                | lume_ui::tool::GitAction::MergeUpstream
                | lume_ui::tool::GitAction::MergeContinue(_)
                | lume_ui::tool::GitAction::MergeAbort
                | lume_ui::tool::GitAction::MergeBranch(_)
                | lume_ui::tool::GitAction::SwitchBranch(_)
                | lume_ui::tool::GitAction::SwitchRemoteBranch { .. }
                | lume_ui::tool::GitAction::StashPush(_)
                | lume_ui::tool::GitAction::StashApply(_)
                | lume_ui::tool::GitAction::StashPop(_)
        )
    }

    pub(super) fn cancel_git_action(&self) {
        if let Some(cancel) = &self.git_action_cancel {
            cancel.cancel();
        }
    }

    pub(super) fn run_git_action(&mut self, action: lume_ui::tool::GitAction) -> Task<Message> {
        if self.git_action_busy {
            return Task::none();
        }
        if Self::git_action_requires_clean_buffers(&action)
            && self
                .workspace
                .iter()
                .any(|document| document.is_dirty() && !self.is_sql_console(document.id()))
        {
            let report = crate::git_actions::ActionReport::warning(
                "Salva o chiudi i buffer con modifiche non salvate, poi ripeti l'operazione Git.",
            );
            if let Some(tool) = self.registry.get_mut(tools::git_native::ID) {
                tool.update(&ToolEvent::new(tools::git_native::Event::ActionFinished(
                    action,
                    report.clone(),
                )));
            }
            return self.notify(NoticeKind::Info, report.text);
        }
        let Some(root) = self.project_root.clone() else {
            if let Some(tool) = self.registry.get_mut(tools::git_native::ID) {
                tool.update(&ToolEvent::new(tools::git_native::Event::ActionFinished(
                    action.clone(),
                    crate::git_actions::ActionReport::error("Nessun progetto aperto."),
                )));
            }
            return Task::none();
        };
        let Some(ticket) =
            self.begin_git_write_per(Self::git_action_requires_clean_buffers(&action))
        else {
            let report = crate::git_actions::ActionReport::warning(
                "Un'operazione Git o di salvataggio è già in corso.",
            );
            if let Some(tool) = self.registry.get_mut(tools::git_native::ID) {
                tool.update(&ToolEvent::new(tools::git_native::Event::ActionFinished(
                    action.clone(),
                    report.clone(),
                )));
            }
            return self.notify(NoticeKind::Info, report.text);
        };
        self.git_action_seq = self.git_action_seq.wrapping_add(1);
        if let Some(tool) = self.registry.get_mut(tools::git_native::ID) {
            tool.update(&ToolEvent::new(tools::git_native::Event::ActionStarted(
                action.clone(),
            )));
        }
        let action_for_worker = action.clone();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.git_action_cancel = Some(FetchCancellation(cancel.clone()));
        self.git_action_busy = true;
        Task::perform(
            async move {
                let worker_cancel = cancel;
                let worker = tokio::task::spawn_blocking(move || {
                    crate::git_actions::execute(&root, &action_for_worker, worker_cancel)
                });
                let report = worker.await.unwrap_or_else(|error| {
                    crate::git_actions::ActionReport::error(format!(
                        "Worker Git non disponibile: {error}"
                    ))
                });
                (action, report)
            },
            move |(action, report)| {
                Message::GitWriteFinished(ticket, Box::new(GitWriteResult::Action(action, report)))
            },
        )
    }

    /// Recupera il lavoro Git saltato mentre la vista era chiusa. I flag si
    /// azzerano all'uso, quindi la chiamata e' idempotente.
    pub(super) fn refresh_stale_git(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        if self.git_stale && self.registry.is_open(tools::git_native::ID) {
            self.git_stale = false;
            let already_queued = self.pending_tool_requests.iter().any(|(id, request)| {
                *id == tools::git_native::ID
                    && matches!(request, lume_ui::tool::ToolRequest::GitRefresh)
            });
            if !already_queued {
                tasks.push(self.run_git(GitOperation::Refresh));
            }
        }
        if self.git_log_stale && self.registry.is_open(tools::gitlog::ID) {
            self.git_log_stale = false;
            let already_queued = self.pending_tool_requests.iter().any(|(id, request)| {
                *id == tools::gitlog::ID && matches!(request, lume_ui::tool::ToolRequest::GitLog(_))
            });
            if !already_queued {
                tasks.push(self.run_git_log());
            }
        }
        Task::batch(tasks)
    }

    /// Vero se il burst tocca solo `.git/` subito dopo una nostra lettura
    /// git: e' l'eco delle nostre scritture, non un cambiamento esterno.
    /// Senza questa guardia si innesca l'anello
    /// `FsChanged -> run_git -> scrittura index -> FsChanged`.
    pub(super) fn is_git_echo(&self, paths: &[PathBuf]) -> bool {
        let Some(finished) = self.last_git_operation else {
            return false;
        };
        if finished.elapsed() >= std::time::Duration::from_millis(1500) {
            return false;
        }
        !paths.is_empty()
            && paths
                .iter()
                .all(|path| path.components().any(|c| c.as_os_str() == ".git"))
    }

    #[cfg(test)]
    pub(super) fn begin_git_write(&mut self) -> Option<u64> {
        self.begin_git_write_per(true)
    }

    fn begin_git_write_per(&mut self, worktree: bool) -> Option<u64> {
        if self.scritture.guarda(None).in_volo.is_some() || self.git_operazione.is_some() {
            return None;
        }
        if worktree {
            let Ok(crate::scritture::Permesso::Avvia(lease)) =
                self.scritture.ammetti(crate::scritture::Op::Git)
            else {
                return None;
            };
            self.git_write = Some(lease);
        }
        self.git_write_sequence += 1;
        self.git_operazione = Some(GitWriteLease {
            worktree,
            ticket: self.git_write_sequence,
            root_epoch: self.git_root_epoch,
        });
        if !worktree {
            self.scritture.evento_git_esterno();
        }
        Some(self.git_write_sequence)
    }

    pub(super) fn finish_git_write(
        &mut self,
        ticket: u64,
        result: GitWriteResult,
    ) -> Task<Message> {
        let Some(owner) = self.git_operazione.filter(|owner| owner.ticket == ticket) else {
            return Task::none();
        };
        self.git_operazione = None;
        if let Some(lease) = self.git_write.take() {
            let esito = match &result {
                GitWriteResult::Snapshot(Err(_), _) => crate::scritture::Esito::Fallito,
                GitWriteResult::Action(_, report)
                    if report.kind == crate::git_actions::NoticeKind::Error =>
                {
                    crate::scritture::Esito::Fallito
                }
                _ => crate::scritture::Esito::Riuscito,
            };
            self.scritture.rilascia(lease, esito);
        } else {
            self.scritture.evento_git_esterno();
            self.git_seguito_legacy = true;
        }
        if owner.root_epoch != self.git_root_epoch {
            let error = match result {
                GitWriteResult::Snapshot(result, committed) => {
                    if committed {
                        self.pending_commit_message = None;
                    }
                    result.err()
                }
                GitWriteResult::Action(_, report) => {
                    self.git_action_busy = false;
                    self.git_action_cancel = None;
                    (report.kind == crate::git_actions::NoticeKind::Error).then_some(report.text)
                }
            };
            let notice = error.map(|error| self.save_failed(format!("Operazione Git precedente terminata con errore: esito del repository da verificare. {error}"))).unwrap_or_else(Task::none);
            return notice;
        }
        // Il risultato dopo una mutazione prevale sui refresh iniziati prima
        // della sua fine. Il loro contatore non è l'epoca della radice.
        let message = match result {
            GitWriteResult::Snapshot(result, committed) => {
                let generation = self
                    .git_gen
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    + 1;
                Message::GitLoaded(generation, result, committed)
            }
            GitWriteResult::Action(action, report) => {
                self.git_action_busy = false;
                self.git_action_cancel = None;
                if let Some(tool) = self.registry.get_mut(tools::git_native::ID) {
                    tool.update(&ToolEvent::new(tools::git_native::Event::ActionFinished(
                        action.clone(),
                        report.clone(),
                    )));
                }
                let notice = match report.kind {
                    crate::git_actions::NoticeKind::Error => self.save_failed(report.text),
                    crate::git_actions::NoticeKind::Ok
                    | crate::git_actions::NoticeKind::Warning => {
                        self.notify(NoticeKind::Info, report.text)
                    }
                };
                let refresh = self.run_git(GitOperation::Refresh);
                let log = self.run_git_log_se_aperto();
                let stash = if action.touches_stash() {
                    self.run_git_stash_list()
                } else {
                    Task::none()
                };
                return Task::batch([notice, refresh, log, stash]);
            }
        };
        let error = match &message {
            Message::GitLoaded(_, Err(error), _) => Some(error.clone()),
            _ => None,
        };
        if matches!(&message, Message::GitLoaded(_, Err(_), true)) {
            // L'azione è conclusa: la bozza resta nel tool, non duplicare
            // indefinitamente la richiesta di commit fallita nel coordinatore.
            self.pending_commit_message = None;
        }
        // Lo smistamento interno non deve consumare il seguito: l'update
        // esterno lo applica dopo aver pubblicato l'eventuale errore Git.
        let update = self.update_messaggio(message);
        let notice = error.map(|error| self.save_failed(format!("Operazione Git terminata con errore: esito del repository da verificare. {error}"))).unwrap_or_else(Task::none);
        Task::batch([update, notice])
    }

    pub(super) fn run_git(&mut self, operation: GitOperation) -> Task<Message> {
        if matches!(&operation, GitOperation::Discard(_))
            && self
                .workspace
                .iter()
                .any(|document| document.is_dirty() && !self.is_sql_console(document.id()))
        {
            return self.notify(
                NoticeKind::Info,
                "Salva o chiudi i buffer con modifiche non salvate, poi ripeti lo scarto Git."
                    .into(),
            );
        }
        let Some(root) = self.project_root.clone() else {
            return Task::none();
        };
        let committed = matches!(operation, GitOperation::Commit { .. });
        let ticket = if matches!(operation, GitOperation::Refresh) {
            None
        } else {
            let Some(ticket) =
                self.begin_git_write_per(matches!(operation, GitOperation::Discard(_)))
            else {
                return Task::done(Message::GitLoaded(
                    self.git_gen.load(std::sync::atomic::Ordering::SeqCst),
                    Err("Un'operazione file è in corso: attendi prima di modificare il repository Git".into()), committed,
                ));
            };
            if let GitOperation::Commit { message, .. } = &operation {
                self.pending_commit_message = Some(message.clone());
            }
            Some(ticket)
        };
        let generation = self
            .git_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        // gix e' sincrono: dentro un `async move` nudo bloccherebbe un worker
        // del runtime. Stesso schema gia' usato per il vault e per lsp_mode --
        // il JoinError non si fa mai unwrap, si collassa nell'errore di dominio.
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let repository = lume_git::GitRepository::discover(&root)
                        .map_err(|error| error.to_string())?;
                    match operation {
                        GitOperation::Refresh => {}
                        GitOperation::Stage(paths) => {
                            repository.stage(paths).map_err(|error| error.to_string())?
                        }
                        GitOperation::Unstage(paths) => repository
                            .unstage(paths)
                            .map_err(|error| error.to_string())?,
                        GitOperation::Discard(paths) => {
                            repository
                                .discard(paths)
                                .map_err(|error| error.to_string())?;
                        }
                        GitOperation::StageHunk { path, hunk } => {
                            repository
                                .stage_hunk(&path, &hunk)
                                .map_err(|error| error.to_string())?;
                        }
                        GitOperation::UnstageHunk { path, hunk } => {
                            repository
                                .unstage_hunk(&path, &hunk)
                                .map_err(|error| error.to_string())?;
                        }
                        GitOperation::Commit { message, amend } => {
                            repository
                                .commit_with(&message, amend)
                                .map_err(|error| error.to_string())?;
                        }
                    }
                    snapshot_of(&repository)
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| match ticket {
                Some(ticket) => Message::GitWriteFinished(
                    ticket,
                    Box::new(GitWriteResult::Snapshot(result, committed)),
                ),
                None => Message::GitLoaded(generation, result, committed),
            },
        )
    }

    // ── Blame ───────────────────────────────────────────────────────

    pub(super) fn run_git_blame(&mut self, path: PathBuf) -> Task<Message> {
        let Some(root) = self.project_root.clone() else {
            return Task::none();
        };
        let blame_seq = self
            .blame_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let task_path = path.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let repository = match lume_git::GitRepository::discover(&root) {
                        Ok(r) => r,
                        Err(e) => return (Err(e.to_string()), false),
                    };
                    let dirty = repository.file_is_dirty(&task_path);
                    let lines = repository.blame(&task_path).map_err(|e| e.to_string());
                    (lines, dirty)
                })
                .await
                .unwrap_or_else(|error| (Err(error.to_string()), false))
            },
            move |(result, dirty)| Message::GitBlameLoaded(blame_seq, path, result, dirty),
        )
    }

    pub(super) fn refresh_editor_blame_availability(&mut self) {
        let Some(id) = self.groups.active() else {
            self.editor_blame_available = false;
            return;
        };
        if self.pending_restored_tabs.contains(id) {
            self.editor_blame_available = false;
            return;
        }
        let Some(path) = self.workspace.get(id).and_then(|doc| doc.path()) else {
            self.editor_blame_available = false;
            return;
        };
        self.editor_blame_available = self.git_available
            && path
                .parent()
                .is_some_and(|root| lume_git::GitRepository::discover(root).is_ok());
    }

    pub(super) fn queue_editor_blame(&mut self, id: DocId) {
        if !self.editor_blame_enabled
            || !self.editor_blame_available
            || self.pending_restored_tabs.contains(id)
            || self.editor_blame_cache.contains_key(&id)
            || self.editor_blame_generation.contains_key(&id)
        {
            return;
        }
        if let Some(path) = self.workspace.get(id).and_then(|doc| doc.path()) {
            self.pending_tool_requests.push((
                tools::git_native::ID,
                ToolRequest::GitBlameFile(path.to_path_buf()),
            ));
        }
    }

    pub(super) fn run_editor_blame(&mut self, id: DocId, path: PathBuf) -> Task<Message> {
        if !self.editor_blame_enabled
            || !self.editor_blame_available
            || self.pending_restored_tabs.contains(id)
        {
            return Task::none();
        }
        let Some((captured_path, text)) = self
            .workspace
            .get(id)
            .and_then(|doc| Some((doc.path()?.to_path_buf(), doc.rope().to_string())))
        else {
            return Task::none();
        };
        if captured_path != path {
            return Task::none();
        }
        let Some(root) = path.parent().map(Path::to_path_buf) else {
            return Task::none();
        };
        self.editor_blame_next_generation = self.editor_blame_next_generation.saturating_add(1);
        let generation = self.editor_blame_next_generation;
        self.editor_blame_generation.insert(id, generation);
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    lume_git::GitRepository::discover(root)
                        .and_then(|repo| repo.blame_aligned(&path, &text))
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Message::EditorBlameLoaded(id, generation, captured_path, result),
        )
    }

    // ── Stash ────────────────────────────────────────────────────────

    pub(super) fn run_git_stash_list(&mut self) -> Task<Message> {
        let Some(root) = self.project_root.clone() else {
            return Task::none();
        };
        let stash_seq = self
            .git_stash_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let repository =
                        lume_git::GitRepository::discover(&root).map_err(|e| e.to_string())?;
                    repository.stash_list().map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Message::GitStashLoaded(stash_seq, result),
        )
    }

    /// Calcola un diff fuori dal thread UI e lo apre nell'area centrale.
    ///
    /// La richiesta viene ricordata: `DiffLoaded` scarta quel che arriva se
    /// nel frattempo l'utente ha aperto un altro file. Stessa forma della
    /// generazione usata per Git e per SQL, ma qui la chiave naturale e' la
    /// richiesta stessa.
    pub(super) fn open_diff(
        &mut self,
        path: PathBuf,
        base: lume_git::DiffBase,
        refresh: bool,
    ) -> Task<Message> {
        let Some(root) = self.project_root.clone() else {
            return Task::none();
        };
        let request = (path, base);
        self.diff_request = Some(request.clone());
        let for_task = request.clone();
        Task::perform(
            async move {
                // Come gli altri handler git: gix e' sincrono e non deve
                // bloccare un worker del runtime.
                tokio::task::spawn_blocking(move || {
                    let repository = lume_git::GitRepository::discover(&root)
                        .map_err(|error| error.to_string())?;
                    let (path, base) = for_task;
                    let diff = match base {
                        lume_git::DiffBase::Worktree => repository.diff_worktree_file(&path),
                        lume_git::DiffBase::Index => repository.diff_index_file(&path),
                        lume_git::DiffBase::Commit(commit) => {
                            repository.diff_commit_file(&commit, &path)
                        }
                    }
                    .map_err(|error| error.to_string())?;
                    Ok(Box::new(diff))
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Message::DiffLoaded {
                request: request.clone(),
                result,
                refresh,
            },
        )
    }

    /// Che fare del diff aperto quando arriva uno stato git nuovo.
    ///
    /// La regola è che **un diff del worktree o dell'index è la vista di una
    /// riga del pannello Commit, e vive finché vive quella riga**: se il file
    /// non è più fra i modificati, le sue modifiche sono state committate o
    /// buttate via e non c'è più niente da guardare. Finché la riga c'è si
    /// ricalcola e si mostra quel che esce — vuoto o binario che sia.
    ///
    /// Con due viste serve una regola in più: **se muore il lato, non muore la
    /// riga.** Tolto dallo stage l'ultimo hunk, la vista «In stage» non ha più
    /// niente da dire, ma il file ha ancora modifiche locali: si ricade
    /// sull'altra vista invece di chiudere. Vale in tutte e due le direzioni.
    ///
    /// `Task` è opaco come `Subscription`: la decisione sta qui, dove si può
    /// asserire. Un diff contro un commit è immutabile per costruzione e non
    /// entra mai in questo giro.
    pub(super) fn diff_to_refresh(&self, status: Option<&[lume_git::FileStatus]>) -> DiffRefresh {
        let Some((path, base)) = self.diff_request.as_ref() else {
            return DiffRefresh::Keep;
        };
        if matches!(base, lume_git::DiffBase::Commit(_)) {
            return DiffRefresh::Keep;
        }
        // Uno status fallito non dice niente sul worktree: meglio lasciare
        // quel che c'è che chiudere per un errore di lettura.
        let Some(status) = status else {
            return DiffRefresh::Keep;
        };
        let Some(file) = status.iter().find(|file| &file.path == path) else {
            return DiffRefresh::Close;
        };
        let risolta = match base {
            lume_git::DiffBase::Index if file.index.is_none() => lume_git::DiffBase::Worktree,
            lume_git::DiffBase::Worktree if file.worktree.is_none() && file.index.is_some() => {
                lume_git::DiffBase::Index
            }
            altra => altra.clone(),
        };
        DiffRefresh::Refresh(path.clone(), risolta)
    }

    /// Chiude il diff e restituisce l'area centrale all'editor o alla console.
    /// Attivare un documento **nel gruppo 0** è uscire dal diff: quell'area
    /// la occupa il diff, che vince su tutto, e il file appena aperto non si
    /// vedrebbe. Nel gruppo 1 no — il flusso utile è proprio quello, il file
    /// a destra e il suo diff a sinistra.
    pub(super) fn close_diff_if_primary(&mut self) {
        if self.groups.focus() == crate::editor_groups::GroupId::Primary {
            self.close_diff();
        }
    }

    pub(super) fn close_diff(&mut self) {
        self.diff = None;
        self.diff_request = None;
    }

    /// Il diff aperto nella forma che i tool leggono per evidenziare la riga.
    ///
    /// Worktree e Index mappano **entrambi** su `None`: sono due viste della
    /// stessa riga del pannello Commit, e la riga resta evidenziata in tutte e
    /// due. Il Log Git confronta l'hash e su `None` non scatta mai.
    pub(crate) fn open_diff_ref(&self) -> Option<(&Path, Option<&str>)> {
        self.diff_request.as_ref().map(|(path, base)| {
            let commit = match base {
                lume_git::DiffBase::Commit(id) => Some(id.as_str()),
                lume_git::DiffBase::Worktree | lume_git::DiffBase::Index => None,
            };
            (path.as_path(), commit)
        })
    }

    /// Git dopo un cambio di radice (avvio o cartella scelta): solo quel che
    /// una vista aperta mostra. A viste chiuse basta il ramo per la titlebar;
    /// il resto lo recuperano i flag stantii alla riapertura (L.11). Con
    /// «git» spenta non si legge niente, ma i flag si alzano lo stesso.
    pub(super) fn git_dopo_cambio_radice(&mut self) -> Task<Message> {
        if self.project_root.is_none() {
            return Task::none();
        }
        let mut tasks = Vec::new();
        if self.registry.is_open(tools::git_native::ID) {
            tasks.push(self.run_git(GitOperation::Refresh));
        } else {
            self.git_stale = true;
            if self.feature_is_enabled(crate::features::Feature::Git) {
                tasks.push(self.run_git_branch());
            }
        }
        tasks.push(self.run_git_log_se_aperto());
        Task::batch(tasks)
    }

    /// Solo il ramo corrente: `discover` + `branch_name`, niente status né log.
    pub(super) fn run_git_branch(&self) -> Task<Message> {
        let Some(root) = self.project_root.clone() else {
            return Task::none();
        };
        let generation = self
            .git_branch_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || leggi_ramo(&root))
                    .await
                    .unwrap_or(None)
            },
            move |branch| Message::GitBranchLoaded(generation, branch),
        )
    }

    /// Il log(200) solo se il dock Log Git lo mostra; altrimenti si segna
    /// stantio e lo recupera `refresh_stale_git` alla riapertura.
    pub(super) fn run_git_log_se_aperto(&mut self) -> Task<Message> {
        if self.registry.is_open(tools::gitlog::ID) {
            self.run_git_log()
        } else {
            self.git_log_stale = true;
            Task::none()
        }
    }

    /// Branch + log dal riferimento corrente del dock Log Git (gix, fuori dal
    /// thread UI). Il riferimento è ricordato per i refresh successivi.
    pub(super) fn run_git_log(&self) -> Task<Message> {
        let Some(root) = self.project_root.clone() else {
            return Task::none();
        };
        let reference = self.git_log_reference.clone();
        let generation = self
            .git_log_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let repository = lume_git::GitRepository::discover(&root)
                        .map_err(|error| error.to_string())?;
                    let branches = repository.branches().map_err(|error| error.to_string())?;
                    let commits = repository
                        .log_from(&reference, tools::gitlog::LOG_LIMIT)
                        .map_err(|error| error.to_string())?;
                    Ok(tools::gitlog::LogSnapshot {
                        reference,
                        branches,
                        commits,
                    })
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Message::GitLogLoaded(generation, result),
        )
    }

    /// File modificati di un commit (diff contro il primo parent).
    pub(super) fn run_git_commit_files(&self, id: String) -> Task<Message> {
        let Some(root) = self.project_root.clone() else {
            return Task::none();
        };
        let commit_id = id.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let repository = lume_git::GitRepository::discover(&root)
                        .map_err(|error| error.to_string())?;
                    repository
                        .commit_files(&id)
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Message::GitCommitFilesLoaded(commit_id.clone(), result),
        )
    }
}
