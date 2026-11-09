//! Coordinatore delle richieste dei tool: worklist, dispatch e persistenza.
//!
//! Stato e messaggi restano in app.rs; i singoli domini possiedono il lavoro.

use super::*;

impl Lume {
    /// Esegue le richieste di un tool. Worklist e non ciclo semplice:
    /// `CloseSelf` puo' generare a sua volta richieste dal tool che si
    /// chiude, via `on_visibility_changed`.
    pub(super) fn apply_tool_requests(
        &mut self,
        requests: Vec<(ToolId, ToolRequest)>,
    ) -> Task<Message> {
        let mut persist_tool_state = false;
        let mut tasks = Vec::new();
        let mut queue: VecDeque<(ToolId, ToolRequest)> = requests.into_iter().collect();
        while let Some((id, req)) = queue.pop_front() {
            match req {
                ToolRequest::NotesLoad => tasks.push(self.load_notes()),
                ToolRequest::NotesUnload => self.unload_notes_view_cache(),
                ToolRequest::NotesCreate(origin) => {
                    if self.notes_scan_generation() == Some(origin.0) {
                        tasks.push(self.create_note());
                    } else {
                        tasks.push(self.notify_error(
                            "La cartella Appunti è cambiata: ripeti l'azione".into(),
                        ));
                    }
                }
                ToolRequest::NotesOpen(origin, note) => {
                    if self.notes_scan_generation() == Some(origin.0) {
                        tasks.push(self.open_note(note));
                    } else {
                        tasks.push(self.notify_error(
                            "La cartella Appunti è cambiata: ripeti l'azione".into(),
                        ));
                    }
                }
                ToolRequest::NotesDelete(origin, note) => {
                    if self.notes_scan_generation() == Some(origin.0) {
                        tasks.push(self.delete_note(note));
                    } else {
                        tasks.push(self.notify_error(
                            "La cartella Appunti è cambiata: ripeti l'azione".into(),
                        ));
                    }
                }
                ToolRequest::NotesChooseDirectory => tasks.push(Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .pick_folder()
                            .await
                            .map(|handle| handle.path().to_path_buf())
                    },
                    Message::NotesDirectoryPicked,
                )),
                ToolRequest::OpenFile(path) => {
                    if let Some(p) = self.resolve_path(&path) {
                        tasks.push(self.open_file_task(p, crate::file_read::DopoApertura::Niente));
                    } else {
                        tasks.push(self.notify_error(format!(
                            "Non si apre {}: il file non c'è più o non è del progetto",
                            path.display()
                        )));
                    }
                }
                ToolRequest::Debug(action) => tasks.push(self.debug_action(action)),
                ToolRequest::OpenFileAt { path, line } => {
                    if let Some(p) = self.resolve_path(&path) {
                        tasks.push(
                            self.open_file_task(p, crate::file_read::DopoApertura::Riga(line)),
                        );
                    } else {
                        tasks.push(self.notify_error(format!(
                            "Non si apre {}: il file non c'è più o non è del progetto",
                            path.display()
                        )));
                    }
                }
                ToolRequest::ContextMenu { path, is_dir, x, y } => {
                    self.apri_menu_contestuale(
                        crate::context_menu::Soggetto::Albero { path, is_dir },
                        iced::Point::new(x, y),
                    );
                }
                ToolRequest::CloseSelf => {
                    queue.extend(self.registry.close(id));
                    persist_tool_state = true;
                }
                ToolRequest::RemoveBookmark(index) => {
                    if index < self.bookmarks.len() {
                        self.bookmarks.remove(index);
                        self.sync_bookmarks_tool();
                        persist_tool_state = true;
                    }
                }
                ToolRequest::RunSearch { options, area } => {
                    tasks.push(self.debounce_search(options, area));
                }
                ToolRequest::CancelSearch => self.cancel_search(),
                ToolRequest::ReplaceAll {
                    matches,
                    replacement,
                } => tasks.push(self.replace_all(&matches, &replacement)),
                ToolRequest::GitRefresh => tasks.push(self.run_git(GitOperation::Refresh)),
                ToolRequest::GitStage(paths) => {
                    tasks.push(self.run_git(GitOperation::Stage(paths)));
                }
                ToolRequest::GitDiscard(paths) => {
                    tasks.push(self.run_git(GitOperation::Discard(paths)));
                }
                ToolRequest::GitOpenDiff { path, commit } => {
                    // I tool chiedono due sole basi: la riga del pannello
                    // Commit (nessun commit) o un commit del Log Git. La terza
                    // e' una scelta dell'utente dentro il visualizzatore,
                    // quindi `ToolRequest` resta a due stati.
                    //
                    // L'eccezione: se la vista «In stage» e' gia' aperta su
                    // **questo** file, riaprire la stessa riga la conserva --
                    // altrimenti un refresh del pannello riporterebbe a
                    // «Locali» sotto le dita. Un file diverso riparte da
                    // «Locali», che e' l'ingresso naturale.
                    let base = match commit {
                        Some(id) => lume_git::DiffBase::Commit(id),
                        None if matches!(
                            self.diff_request.as_ref(),
                            Some((aperto, lume_git::DiffBase::Index)) if *aperto == path
                        ) =>
                        {
                            lume_git::DiffBase::Index
                        }
                        None => lume_git::DiffBase::Worktree,
                    };
                    tasks.push(self.open_diff(path, base, false));
                }
                ToolRequest::GitUnstage(path) => {
                    tasks.push(self.run_git(GitOperation::Unstage(path)))
                }
                ToolRequest::GitCommit { message, amend } => {
                    tasks.push(self.run_git(GitOperation::Commit { message, amend }))
                }
                ToolRequest::GitLog(reference) => {
                    self.git_log_reference = reference;
                    tasks.push(self.run_git_log());
                }
                ToolRequest::GitCommitFiles(id) => tasks.push(self.run_git_commit_files(id)),
                // Blame
                ToolRequest::GitBlame(path) => {
                    tasks.push(self.run_git_blame(path));
                }
                ToolRequest::GitBlameFile(path) => {
                    let id = self.workspace.ids().find(|id| {
                        self.workspace.get(*id).and_then(|doc| doc.path()) == Some(path.as_path())
                    });
                    if let Some(id) = id {
                        tasks.push(self.run_editor_blame(id, path));
                    }
                }
                ToolRequest::GitCloseBlame => {
                    // Nessuna operazione: chiusura solo UI.
                }
                ToolRequest::GitAction(action) => {
                    tasks.push(self.run_git_action(action));
                }
                ToolRequest::GitCancel => {
                    self.cancel_git_action();
                }
                ToolRequest::GitStashRefresh => {
                    tasks.push(self.run_git_stash_list());
                }
                // RAM
                ToolRequest::RefreshPeakMemory => {
                    tasks.push(self.run_read_peak_memory());
                }
                ToolRequest::JjLoad => tasks.push(self.run_jj_load()),
                ToolRequest::JjAction(action) => tasks.push(self.run_jj_action(action)),
                ToolRequest::JjCancel => {
                    if let Some(pending) = &self.jj_pending_load {
                        pending.revoke();
                    }
                    self.jj_load_pending = false;
                    self.jj_cancel
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                ToolRequest::OpenExternal(url) => {
                    if let Err(error) = crate::external::open(&url) {
                        self.persistence_error = Some(error);
                    }
                }
                ToolRequest::SetProjectWatchDirectories(paths) => {
                    self.set_project_watch_directories(paths);
                }
                #[cfg(feature = "sql")]
                ToolRequest::SqlToggleConnection(index) => {
                    if let Some(id) = self.sql_profiles.get(index).map(|p| p.id.clone()) {
                        let expanding = !self.sql_expanded.remove(&id);
                        if expanding {
                            self.sql_expanded.insert(id.clone());
                        }
                        // Espandere una connessione chiusa la apre: è il
                        // gesto naturale, ed è anche il modo di rientrare
                        // dopo l'ibernazione. Prima però serve il segreto.
                        //
                        // `Failed` non conta come viva: una password sbagliata
                        // deve poter essere ridigitata subito, non dopo il
                        // timeout d'ibernazione.
                        let live = self.sql.as_ref().is_some_and(|w| {
                            w.connection(&id)
                                .is_some_and(|c| c.state() != lume_sql::worker::State::Failed)
                        });
                        if expanding && !live {
                            // Il campo password non compare più per riflesso:
                            // prima si guarda in cassaforte. È tutto il senso
                            // della feature — se il segreto c'è, l'utente non
                            // vede nulla e la connessione parte.
                            tasks.push(self.sql_recall_secret(&id));
                        }
                        self.sync_db_tool();
                    }
                }
                #[cfg(feature = "sql")]
                ToolRequest::SqlToggleTable(connection, table) => {
                    if let Some((id, name)) =
                        self.sql_profiles.get(connection).and_then(|profile| {
                            self.sql
                                .as_ref()
                                .and_then(|w| w.schema_for(&profile.id))
                                .and_then(|schema| schema.tables.get(table))
                                .map(|name| (profile.id.clone(), name.clone()))
                        })
                    {
                        let key = format!("{id}:{name}");
                        let expanding = !self.sql_expanded.remove(&key);
                        if expanding {
                            self.sql_expanded.insert(key);
                        }
                        // Le colonne si chiedono una volta sola: la cache
                        // vive finché non scatta l'ibernazione, ed è ciò che rende
                        // istantanea una riapertura prima del timeout.
                        let known = self
                            .sql
                            .as_ref()
                            .and_then(|w| w.schema_for(&id))
                            .is_some_and(|schema| schema.columns.contains_key(&name));
                        self.sync_db_tool();
                        if expanding && !known {
                            tasks.push(self.sql_submit(
                                &id,
                                lume_sql::worker::Request::Columns(name),
                                None,
                            ));
                        }
                    }
                }
                #[cfg(feature = "sql")]
                ToolRequest::SqlOpenConsole { connection, table } => {
                    self.open_sql_console(connection, table);
                    persist_tool_state = true;
                }
                #[cfg(feature = "sql")]
                ToolRequest::SqlSecretSubmitted(secret) => {
                    if let Some(id) = self.sql_secret_for.take() {
                        let text = secret.expose();
                        // Tenuto da parte, non ancora salvato: in cassaforte
                        // ci va solo quello che il server ha accettato.
                        self.sql_pending_secret =
                            Some((id.clone(), lume_sql::Secret::new(text.clone())));
                        tasks.push(self.sql_connect(&id, lume_sql::Secret::new(text)));
                    }
                }
                #[cfg(feature = "sql")]
                ToolRequest::SqlSecretCancelled => {
                    // Annullare chiude anche il nodo: lasciarlo aperto e
                    // vuoto sembrerebbe un database senza tabelle.
                    if let Some(id) = self.sql_secret_for.take() {
                        self.sql_expanded.remove(&id);
                    }
                    self.sync_db_tool();
                }
                #[cfg(feature = "sql")]
                ToolRequest::SqlRefreshSchema => {
                    let live: Vec<String> = self
                        .sql_profiles
                        .iter()
                        .map(|profile| profile.id.clone())
                        .filter(|id| {
                            self.sql.as_ref().is_some_and(|w| {
                                w.connection(id)
                                    .is_some_and(|c| c.state() != lume_sql::worker::State::Failed)
                            })
                        })
                        .collect();
                    for id in live {
                        tasks.push(self.sql_submit(&id, lume_sql::worker::Request::Tables, None));
                    }
                }
                // Il modulo vive nel tool finché non si conferma: qui non
                // arriva nulla da fare.
                #[cfg(feature = "sql")]
                ToolRequest::SqlCreateConnection(draft) => {
                    tasks.push(self.create_sql_connection(&draft));
                    persist_tool_state = true;
                }
                #[cfg(feature = "sql")]
                ToolRequest::SqlImportIdea => tasks.push(self.pick_idea()),
                #[cfg(feature = "sql")]
                ToolRequest::SqlDetectIdea => tasks.push(self.detect_idea()),
                #[cfg(feature = "sql")]
                ToolRequest::SqlConfirmIdea => tasks.push(self.confirm_idea()),
                #[cfg(feature = "sql")]
                ToolRequest::SqlCancelIdea => self.clear_idea(),
                #[cfg(feature = "sql")]
                ToolRequest::SqlIdeaUser { index, user } => self.set_idea_user(index, user),
                #[cfg(feature = "sql")]
                ToolRequest::SqlDeleteConnection(index) => {
                    tasks.push(self.delete_sql_connection(index));
                }
                #[cfg(not(feature = "sql"))]
                ToolRequest::SqlToggleConnection(_)
                | ToolRequest::SqlToggleTable(_, _)
                | ToolRequest::SqlOpenConsole { .. }
                | ToolRequest::SqlSecretSubmitted(_)
                | ToolRequest::SqlSecretCancelled
                | ToolRequest::SqlRefreshSchema
                | ToolRequest::SqlImportIdea
                | ToolRequest::SqlDetectIdea
                | ToolRequest::SqlConfirmIdea
                | ToolRequest::SqlCancelIdea
                | ToolRequest::SqlIdeaUser { .. }
                | ToolRequest::SqlCreateConnection(_)
                | ToolRequest::SqlDeleteConnection(_) => {}
                // Il pannello parla per chiavi; la traduzione in `Feature` sta
                // qui, dove la tabella è raggiungibile. Una chiave che non
                // esiste è un refuso, non un caso: si ignora in silenzio, e il
                // test di coerenza della tabella lo prende prima.
                ToolRequest::SetFeatureEnabled { feature, enabled } => {
                    if let Some(spec) = crate::features::by_key(feature) {
                        tasks.push(self.apply_feature_command(
                            crate::features::FeatureCommand::SetEnabled(spec.feature, enabled),
                        ));
                    }
                }
                ToolRequest::StartFeature(feature) => {
                    if let Some(spec) = crate::features::by_key(feature) {
                        tasks.push(self.apply_feature_command(
                            crate::features::FeatureCommand::Start(spec.feature),
                        ));
                    }
                }
                ToolRequest::StopFeature(feature) => {
                    if let Some(spec) = crate::features::by_key(feature) {
                        tasks.push(self.apply_feature_command(
                            crate::features::FeatureCommand::Stop(spec.feature),
                        ));
                    }
                }
                ToolRequest::CancelStopFeature => {
                    self.feature_stop_pending = None;
                    let _ = self.sync_extensions_tool();
                }
                ToolRequest::WriteClipboard(testo) => {
                    tasks.push(iced::clipboard::write(testo));
                    tasks.push(self.notify(NoticeKind::Info, "Copiato negli appunti".to_owned()));
                }
            }
        }
        if persist_tool_state {
            tasks.push(self.persist_session());
        }
        // Anche qui, e non solo in `drain_tool_requests`: `Message::Tool`
        // ritorna da questa funzione senza passare dalla coda di `update`,
        // quindi un `CloseSelf` del pannello Database rimanderebbe il fronte
        // al messaggio successivo. È idempotente.
        self.reconcile_feature_visibility();
        let _ = self.sync_extensions_tool();
        self.sync_outlines();
        let _ = self.sync_structure_tool();
        Task::batch(tasks)
    }
}
