//! Editor: comandi, sintassi, salvataggio e chiusura delle schede.
//!
//! Metodi di `Lume`: lo stato e i messaggi restano in `app.rs`.

use super::*;

impl Lume {
    /// Avvia la lettura fuori dal thread UI; la sintassi resta al worker. Una sola
    /// richiesta per scheda: click ripetuti condividono il lavoro già avviato.
    pub(super) fn restored_tab_read_task(&mut self, id: DocId, announce: bool) -> Task<Message> {
        let Some(path) = self
            .pending_restored_tabs
            .get(id)
            .map(|pending| pending.path.clone())
        else {
            return Task::none();
        };
        if self.git_write.is_some() {
            self.scarta_aperture_ripristinate(id);
            return self.notify_error(format!(
                "Impossibile riaprire {} durante l'operazione Git; ripeti l'apertura al termine",
                path.display()
            ));
        }
        if self.restored_tab_reads.contains_key(&id) {
            if !self
                .restored_tab_delivery
                .get(&id)
                .is_some_and(crate::reader_jobs::DeliveryStatus::abandoned)
            {
                return Task::none();
            }
            self.restored_tab_reads.remove(&id);
            self.restored_tab_delivery.remove(&id);
        }
        let Some(request) = self.restored_tab_request.checked_add(1) else {
            self.scarta_aperture_ripristinate(id);
            return self.notify_error("Richieste di ripristino esaurite".into());
        };
        self.restored_tab_request = request;
        self.restored_tab_reads.insert(id, request);
        let epoch = self.scritture.guarda(None).epoca;
        let Some(version) = self.workspace.get(id).map(lume_core::Document::version) else {
            self.scarta_aperture_ripristinate(id);
            self.restored_tab_reads.remove(&id);
            self.restored_tab_delivery.remove(&id);
            return Task::none();
        };
        let worker_path = path.clone();
        let display_path = path.clone();
        let snapshot = self
            .workspace
            .get(id)
            .map(lume_core::Document::read_snapshot);
        let known_notes_dir = self.notes_directory_scelta.clone().or_else(|| {
            self.notes
                .as_ref()
                .and_then(|notes| notes.directory.clone())
        });
        let fallback_notes_dir = self.notes_directory.clone();
        let mut precedenti = self
            .restored_notes_candidates
            .get(&id)
            .cloned()
            .unwrap_or_default();
        if let Some(originale) = self.restored_notes_revoked.get(&id)
            && !precedenti.contains(originale)
        {
            precedenti.push(originale.clone());
        }
        let notes_sequence = self.notes_library_sequence;
        let notes_selection = self.notes_directory_scelta.clone();
        let notes_context = (notes_sequence, notes_selection.clone());
        let store_handle = self.store.clone();
        let reading = self.readers.read(move || {
            let is_md = worker_path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
            if is_md {
                let notes_dir = known_notes_dir
                    .or_else(|| {
                        store_handle.as_ref().and_then(|store| {
                            futures::executor::block_on(store.load_notes_directory())
                                .ok()
                                .flatten()
                        })
                    })
                    .unwrap_or(fallback_notes_dir);
                let note_result = precedenti
                    .iter()
                    .find_map(|directory| {
                        crate::app::notes::carica_nota_ripristinata(directory, &worker_path)
                    })
                    .or_else(|| {
                        crate::app::notes::carica_nota_ripristinata(&notes_dir, &worker_path)
                    });
                if let Some(note_result) = note_result {
                    let (notes_dir, store, loaded) = note_result?;
                    let note_matches_selection = notes_selection.as_ref().is_none_or(|scelta| {
                        lume_core::canonical_path(&notes_dir)
                            .ok()
                            .zip(lume_core::canonical_path(scelta).ok())
                            .is_some_and(|(origine, selezione)| origine == selezione)
                    });
                    let text = crate::file_read::Testo::from_note(
                        &loaded.original_bytes,
                        loaded.note.markdown.clone(),
                    )
                    .prepare(snapshot);
                    let syntax = lume_syntax::detect(&worker_path).map(SyntaxLanguage);
                    return Ok::<_, String>(RestoredTabPayloadMessage::new(RestoredTabPayload {
                        text,
                        syntax,
                        note: Some((notes_dir, store, loaded)),
                        notes_selection,
                        note_matches_selection,
                    }));
                }
            }
            let text =
                crate::file_read::read_testo_capped(&worker_path, crate::file_read::MAX_FILE_BYTES)
                    .map(|text| text.prepare(snapshot))
                    .map_err(|error| error.to_string())?;
            let syntax = lume_syntax::detect(&worker_path).map(SyntaxLanguage);
            Ok::<_, String>(RestoredTabPayloadMessage::new(RestoredTabPayload {
                text,
                syntax,
                note: None,
                notes_selection,
                note_matches_selection: true,
            }))
        });
        let (status, delivery) = crate::reader_jobs::DeliveryStatus::new();
        self.restored_tab_delivery.insert(id, status);
        let read = Task::perform(
            async move {
                let result = reading.await.unwrap_or_else(Err);
                RestoredTabReadOutcome::new(result, delivery, notes_context)
            },
            move |result| {
                Message::RestoredTabReadFinished(id, path, request, epoch, version, result)
            },
        );
        if announce {
            let notice = self.notify(
                NoticeKind::Info,
                format!("lettura in corso: {}", display_path.display()),
            );
            Task::batch([read, notice])
        } else {
            read
        }
    }

    pub(super) fn restored_tab_read_finished(
        &mut self,
        id: DocId,
        path: &Path,
        request: u64,
        epoch: u64,
        version: u64,
        outcome: RestoredTabReadOutcome,
    ) -> Task<Message> {
        outcome.delivered();
        let notes_context = outcome.notes_context;
        let result = outcome.result;
        if self.restored_tab_reads.get(&id) != Some(&request) {
            return Task::none();
        }
        self.restored_tab_reads.remove(&id);
        self.restored_tab_delivery.remove(&id);
        if !self.groups.contains(id)
            || self.workspace.get(id).and_then(lume_core::Document::path) != Some(path)
        {
            return Task::none();
        }
        let Some(pending) = self.pending_restored_tabs.get(id) else {
            return self.restored_tab_action_task(id);
        };
        if pending.path != path {
            return Task::none();
        }
        if self.git_write.is_some() || epoch != self.scritture.guarda(None).epoca {
            self.scarta_aperture_ripristinate(id);
            let error = "lettura superata da un'operazione sui file; ripeti l'apertura";
            self.pending_restored_tabs.set_failure(id, error.to_owned());
            self.notify_persistente(
                NoticeKind::Error,
                format!("Impossibile riaprire {}: {error}", path.display()),
            );
            return Task::none();
        }
        if self
            .workspace
            .get(id)
            .is_none_or(|document| document.version() != version)
        {
            self.scarta_aperture_ripristinate(id);
            let error = "documento aggiornato durante la lettura; ripeti l'apertura";
            self.pending_restored_tabs.set_failure(id, error.to_owned());
            self.notify_persistente(
                NoticeKind::Error,
                format!("Impossibile riaprire {}: {error}", path.display()),
            );
            return Task::none();
        }
        let result = result.and_then(|message| {
            message
                .take()
                .ok_or_else(|| "risultato di lettura già consumato".to_owned())
        });
        // Autentica anche Err prima di cambiare il fallimento o il seguito.
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            && notes_context.as_ref().is_some_and(|(sequenza, scelta)| {
                *sequenza != self.notes_library_sequence || *scelta != self.notes_directory_scelta
            })
        {
            if let Ok(payload) = &result
                && let Some((directory, _, _)) = payload.note.as_ref()
            {
                let candidati = self.restored_notes_candidates.entry(id).or_default();
                if !candidati.contains(directory) {
                    candidati.push(directory.clone());
                }
            }
            return self.restored_tab_read_task(id, false);
        }
        let mut payload = match result {
            Ok(payload) => payload,
            Err(error) => {
                self.scarta_aperture_ripristinate(id);
                self.pending_restored_tabs.set_failure(id, error.clone());
                self.notify_persistente(
                    NoticeKind::Error,
                    format!("Impossibile riaprire {}: {error}", path.display()),
                );
                return Task::none();
            }
        };
        // L'identità viene calcolata sul worker e vale solo per la scelta e
        // la sequenza appena autenticate. Nessun corpo senza guardia nota.
        let origine_superata = payload
            .note
            .as_ref()
            .filter(|_| !payload.note_matches_selection)
            .map(|(directory, _, _)| directory.clone())
            .or_else(|| {
                payload
                    .note
                    .is_none()
                    .then(|| self.restored_notes_revoked.get(&id).cloned())
                    .flatten()
            });
        if let Some(directory) = origine_superata {
            self.restored_notes_candidates.remove(&id);
            self.restored_notes_revoked.insert(id, directory);
            self.scarta_aperture_ripristinate(id);
            let error = "la cartella Appunti è cambiata; torna alla cartella precedente";
            self.pending_restored_tabs.set_failure(id, error.to_owned());
            self.notify_persistente(
                NoticeKind::Error,
                format!("Impossibile riaprire {}: {error}", path.display()),
            );
            return Task::none();
        }
        if let Some((directory, _, _)) = payload.note.as_mut()
            && let Some(scelta) = payload.notes_selection.as_ref()
        {
            // Le due directory sono la stessa identità filesystem; la scelta
            // esplicita resta il percorso autorevole mostrato e persistito.
            directory.clone_from(scelta);
        }
        if let Some(document) = self.workspace.get_mut(id) {
            if payload.text.apply_reload(document).is_none() {
                return self
                    .notify_error("Ripristino preparato obsoleto o già consumato".to_owned());
            }
        } else {
            return Task::none();
        }
        self.disk_fingerprints
            .insert(id, payload.text.fingerprint());
        self.external_conflicts.remove(&id);
        if let Some(syntax) = payload.syntax {
            self.syntax.insert(id, syntax);
        }
        if let Some((notes_dir, store, loaded)) = payload.note {
            self.registra_nota_ripristinata(id, notes_dir, store, loaded);
        }
        self.pending_restored_tabs.take(id);
        self.concludi_materializzazione_ripristinata(id);
        self.lsp_did_open(id);
        self.refresh_editor_blame_availability();
        self.queue_editor_blame(id);
        self.replay_restored_commands(id)
    }

    /// Una materializzazione valida invalida il reader e libera le origini;
    /// comandi e seguiti restano disponibili per il replay del chiamante.
    pub(super) fn concludi_materializzazione_ripristinata(&mut self, id: DocId) {
        self.restored_tab_reads.remove(&id);
        self.restored_tab_delivery.remove(&id);
        self.restored_notes_revoked.remove(&id);
        self.restored_notes_candidates.remove(&id);
    }

    pub(super) fn restored_tab_action_task(&mut self, id: DocId) -> Task<Message> {
        let action = self
            .restored_tab_actions
            .get_mut(&id)
            .and_then(VecDeque::pop_front);
        if self
            .restored_tab_actions
            .get(&id)
            .is_some_and(VecDeque::is_empty)
        {
            self.restored_tab_actions.remove(&id);
        }
        action.map_or_else(Task::none, |action| {
            Task::done(Message::RestoredTabAction(id, action))
        })
    }

    pub(super) fn defer_restored_action(
        &mut self,
        id: DocId,
        action: DeferredRestoredAction,
    ) -> Task<Message> {
        let actions = self.restored_tab_actions.entry(id).or_default();
        if !actions.contains(&action) {
            actions.push_back(action);
        }
        self.restored_tab_read_task(id, false)
    }

    pub(super) fn queue_restored_editor_command(
        &mut self,
        id: DocId,
        command: EditorCommand,
    ) -> bool {
        let queue = self.restored_tab_commands.entry(id).or_default();
        let queued_bytes: usize = queue.iter().map(editor_command_text_bytes).sum();
        let next_bytes = editor_command_text_bytes(&command);
        if queue.len() < 128 && queued_bytes.saturating_add(next_bytes) <= 1_048_576 {
            queue.push_back(command);
            true
        } else {
            self.notify_persistente(
                NoticeKind::Info,
                "Input in attesa oltre il limite; ripeti dopo il caricamento".into(),
            );
            false
        }
    }

    fn scarta_aperture_ripristinate(&mut self, id: DocId) {
        if let Some(actions) = self.restored_tab_actions.get_mut(&id) {
            actions.retain(|action| {
                !matches!(action, DeferredRestoredAction::RiprendiApertura { .. })
            });
            if actions.is_empty() {
                self.restored_tab_actions.remove(&id);
            }
        }
    }

    pub(super) fn forget_restored_action(&mut self, id: DocId, action: &DeferredRestoredAction) {
        let Some(actions) = self.restored_tab_actions.get_mut(&id) else {
            return;
        };
        actions.retain(|queued| queued != action);
        if actions.is_empty() {
            self.restored_tab_actions.remove(&id);
        }
    }

    pub(super) fn resume_restored_tab_action(
        &mut self,
        id: DocId,
        action: DeferredRestoredAction,
    ) -> Task<Message> {
        match action {
            DeferredRestoredAction::OpenGotoLine(group) => self.open_goto_line_for(id, group),
            DeferredRestoredAction::GotoLineIndex(line) => {
                if self.active_doc() == Some(id) {
                    self.goto_line_index_for(id, line);
                }
                Task::none()
            }
            DeferredRestoredAction::GotoLspPosition(position) => {
                if self.active_doc() == Some(id) {
                    self.goto_lsp_position_for(id, position);
                }
                Task::none()
            }
            DeferredRestoredAction::OpenFind(replace_mode) => {
                if self.active_doc() == Some(id) {
                    self.open_find_for(id, replace_mode)
                } else {
                    Task::none()
                }
            }
            DeferredRestoredAction::ToggleBookmark => {
                if self.active_doc() == Some(id) {
                    self.toggle_bookmark_for(id)
                } else {
                    Task::none()
                }
            }
            DeferredRestoredAction::CopySelection => {
                if self.active_doc() == Some(id) {
                    self.copy_selection_for(id)
                } else {
                    Task::none()
                }
            }
            DeferredRestoredAction::CutSelection => {
                if self.active_doc() != Some(id) {
                    return Task::none();
                }
                let copy = self.copy_selection_for(id);
                self.apply_editor_command_for(id, EditorCommand::Cut);
                Task::batch([copy, self.lsp_change_task()])
            }
            DeferredRestoredAction::SaveDocument => {
                if self.active_doc() == Some(id) {
                    self.save_doc(id)
                } else {
                    Task::none()
                }
            }
            DeferredRestoredAction::SaveAs => {
                if self.active_doc() == Some(id) {
                    self.pick_save_path(id)
                } else {
                    Task::none()
                }
            }
            DeferredRestoredAction::GotoDefinition => {
                if self.active_doc() == Some(id) {
                    self.goto_definition_for(id)
                } else {
                    Task::none()
                }
            }
            DeferredRestoredAction::FindUsages => {
                if self.active_doc() == Some(id) {
                    self.find_usages_for(id)
                } else {
                    Task::none()
                }
            }
            DeferredRestoredAction::Rename => {
                if self.active_doc() == Some(id) {
                    self.start_rename_for(id)
                } else {
                    Task::none()
                }
            }
            DeferredRestoredAction::Completion => {
                if self.active_doc() == Some(id) && !self.pending_restored_tabs.contains(id) {
                    self.request_completion_for(id);
                }
                Task::none()
            }
            DeferredRestoredAction::QuickDoc => {
                if self.active_doc() == Some(id) && !self.pending_restored_tabs.contains(id) {
                    self.hover_at_cursor_for(id);
                }
                Task::none()
            }
            DeferredRestoredAction::SignatureHelp => {
                if self.active_doc() == Some(id) && !self.pending_restored_tabs.contains(id) {
                    return self.signature_help_at_cursor();
                }
                Task::none()
            }
            DeferredRestoredAction::SelectNextOccurrence => self.next_occurrence_for(id),
            DeferredRestoredAction::RiprendiApertura { path, dopo } => {
                if self.workspace.get_by_path(&path) == Some(id) && self.groups.contains(id) {
                    self.open_file_task(path, dopo)
                } else {
                    self.notify_error(
                        "Apertura superata: la scheda è cambiata, ripeti l'azione".into(),
                    )
                }
            }
            DeferredRestoredAction::ReplaceAll {
                matches,
                replacement,
            } => self.replace_all(&matches, &replacement),
        }
    }

    /// Materializzazione sincrona conservata solo per gli scenari legacy dei
    /// test. Tutti i percorsi di produzione usano `restored_tab_read_task`.
    #[cfg(test)]
    pub(super) fn materialize_restored_tab(&mut self, id: DocId) -> Result<(), std::io::Error> {
        let Some(pending) = self.pending_restored_tabs.get(id).cloned() else {
            return Ok(());
        };
        if let Err(error) = self.check_file_binding(&pending.path) {
            self.pending_restored_tabs
                .set_failure(id, error.to_string());
            return Err(error);
        }
        let notes_dir = self
            .notes
            .as_ref()
            .and_then(|notes| notes.directory.clone())
            .or_else(|| {
                self.store.as_ref().and_then(|store| {
                    futures::executor::block_on(store.load_notes_directory())
                        .ok()
                        .flatten()
                })
            })
            .unwrap_or_else(|| self.notes_directory.clone());
        let note_loaded = crate::app::notes::carica_nota_ripristinata(&notes_dir, &pending.path);
        let (content, note_state) = match note_loaded {
            Some(Ok((dir, store, loaded))) => (
                crate::file_read::Testo::from_note(
                    &loaded.original_bytes,
                    loaded.note.markdown.clone(),
                ),
                Some((dir, store, loaded)),
            ),
            Some(Err(error)) => {
                self.pending_restored_tabs.set_failure(id, error.clone());
                return Err(std::io::Error::other(error));
            }
            None => match crate::file_read::read_testo_capped(
                &pending.path,
                crate::file_read::MAX_FILE_BYTES,
            ) {
                Ok(content) => (content, None),
                Err(error) => {
                    let error = std::io::Error::from(error);
                    self.pending_restored_tabs
                        .set_failure(id, error.to_string());
                    return Err(error);
                }
            },
        };
        let Some(document) = self.workspace.get_mut(id) else {
            let error = std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "la scheda ripristinata non è più nel workspace",
            );
            self.pending_restored_tabs
                .set_failure(id, error.to_string());
            return Err(error);
        };
        document.reload_from_str(&content);
        self.disk_fingerprints.insert(id, content.fingerprint());
        self.external_conflicts.remove(&id);
        if let Some((dir, store, loaded)) = note_state {
            self.registra_nota_ripristinata(id, dir, store, loaded);
        }
        if let Some(language) = lume_syntax::detect(&pending.path) {
            self.syntax.insert(id, SyntaxLanguage(language));
        }
        self.pending_restored_tabs.take(id);
        if let Some(commands) = self.restored_tab_commands.remove(&id) {
            for command in commands {
                self.apply_editor_command_for(id, command);
            }
        }
        self.lsp_did_open(id);
        Ok(())
    }

    /// Perché il documento attivo non ha i colori: la sua lingua è
    /// riconosciuta ma la query di evidenziazione non compila. Un documento
    /// senza path o di una lingua sconosciuta non ha errori di questo tipo.
    pub fn active_syntax_error(&self) -> Option<&'static str> {
        lume_syntax::spec_error(lume_syntax::detect(self.active_path()?)?)
    }

    pub(super) fn autosave_timer(doc: DocId, version: Option<u64>) -> Task<Message> {
        let Some(version) = version else {
            return Task::none();
        };
        Task::perform(
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(
                    crate::autosave::RITARDO_MS,
                ))
                .await;
                version
            },
            move |version| Message::NotesAutosaveDue(doc, version),
        )
    }

    /// Cambiare documento attivo revoca risultati LSP legati alla vista
    /// precedente. Il solo controllo dell'active doc alla risposta non basta:
    /// l'utente può cambiare scheda e tornare prima che il server risponda.
    pub(super) fn activate_editor_tab(&mut self, id: DocId) {
        if self.active_doc() != Some(id) {
            self.completion_close();
            self.definition_cancel();
            self.signature_help_clear();
        }
        self.groups.activate(id);
        self.syntax_reconcile_groups();
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_open_in_editor(
        &mut self,
        epoch: u64,
        request_id: u64,
        relative: PathBuf,
        line: Option<usize>,
    ) -> Task<Message> {
        let Some(root) = self
            .mcp
            .as_ref()
            .filter(|mcp| mcp.epoch == epoch)
            .map(|mcp| mcp.root.clone())
        else {
            return Task::none();
        };
        let requested = lume_mcp::confine::to_slash(&relative);
        let path = match lume_mcp::Root::new(&root).and_then(|root| {
            root.resolve_lenient(&requested).map_err(|error| {
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, error.to_string())
            })
        }) {
            Ok(confined) => confined.absolute,
            Err(error) => {
                if let Some(mcp) = self.mcp.as_ref() {
                    mcp.answer(request_id, Err(error.to_string()));
                }
                return Task::none();
            }
        };

        if let Some(id) = self.mcp_doc_at(&root, &relative) {
            self.activate_editor_tab(id);
            self.close_diff_if_primary();
            self.refresh_editor_blame_availability();
            self.queue_editor_blame(id);
            self.push_history(
                lume_store::HistoryAction::Opened,
                tools::bookmarks::display_name(&path),
                Some(path.clone()),
            );
            if let Some(line) = line {
                self.goto_line(line);
            }
            let opened = crate::mcp_mode::relative_to_root(&root, &path)
                .map(|path| lume_mcp::confine::to_slash(&path))
                .unwrap_or_else(|| requested.clone());
            if let Some(mcp) = self.mcp.as_ref() {
                mcp.answer(
                    request_id,
                    Ok(lume_mcp::AppAnswer::Json(serde_json::json!({
                        "opened": opened,
                        "line": line,
                    }))),
                );
            }
            return Task::none();
        }

        if let Err(error) = self.check_file_binding(&path) {
            if let Some(mcp) = self.mcp.as_ref() {
                mcp.answer(request_id, Err(error.to_string()));
            }
            return Task::none();
        }
        let path_for_message = path.clone();
        let reading = self.readers.read(move || {
            crate::file_read::read_testo_capped(&path, crate::file_read::MAX_FILE_BYTES)
                .map(|text| text.prepare(None))
                .map_err(|error| error.to_string())
        });
        Task::perform(
            async move { reading.await.unwrap_or_else(Err) },
            move |result| Message::McpOpenFinished {
                epoch,
                request_id,
                path: path_for_message,
                line,
                result,
            },
        )
    }

    /// Sostituisce esattamente `from..to` con `text` — la versione a range
    /// esplicito di `editor_replace_selection`, che invece usa sempre la
    /// selezione corrente. Serve perché il range da sostituire qui è quello
    /// del match trovato, non necessariamente quello selezionato.
    pub(super) fn editor_replace_range(&mut self, id: DocId, from: usize, to: usize, text: &str) {
        if self.rifiuta_modifica_per_scrittura(id) {
            return;
        }
        let Some(doc) = self.workspace.get_mut(id) else {
            return;
        };
        let len = doc.rope().len_chars();
        let edit = doc.apply(lume_core::Transaction::with_selection(
            lume_core::ChangeSet::replace_range(from, to, text, len),
            lume_core::Range::point(from + text.chars().count()),
        ));
        self.resync_syntax(id, edit);
    }

    /// Applica il drop di un'icona: barra di destinazione + slot dalla y.
    /// Esito di un gesto sulla barra delle schede. Sta in un metodo perché
    /// arriva da due strade — il messaggio della barra e il rilascio del
    /// mouse durante un trascinamento — e due copie divergerebbero.
    /// Se una scheda può stare nel gruppo 1.
    ///
    /// La console SQL no: non è un file ma una **vista del gruppo 0**
    /// (`central_view_of` dà sempre `Editor` a destra), e trascinarcela
    /// mostrerebbe la query come testo grezzo, senza barra né griglia. Il
    /// modulo `editor_groups` è puro e non può saperlo: la regola sta qui,
    /// e vale per il trascinamento come per «Dividi a destra».
    pub(super) fn si_puo_dividere(&self, id: DocId) -> bool {
        !self.is_sql_console(id)
    }

    pub(super) fn apply_tab_intent(&mut self, intent: EditorTabBarIntent) -> Task<Message> {
        match intent {
            EditorTabBarIntent::None => Task::none(),
            EditorTabBarIntent::Activate(id) => {
                self.request_editor_focus();
                self.activate_tab(id)
            }
            EditorTabBarIntent::Drop { id, group, before } => {
                if group == crate::editor_groups::GroupId::Secondary && !self.si_puo_dividere(id) {
                    return Task::none();
                }
                self.groups.move_before(group, id, before);
                self.close_diff_if_primary();
                Task::none()
            }
            EditorTabBarIntent::Close(id) => {
                self.chiudi_scheda(id);
                Task::none()
            }
            EditorTabBarIntent::ContextMenu(id, punto) => {
                let path = self
                    .workspace
                    .get(id)
                    .and_then(|documento| documento.path().map(std::path::Path::to_path_buf));
                self.apri_menu_contestuale(
                    crate::context_menu::Soggetto::Scheda { id, path },
                    punto,
                );
                Task::none()
            }
        }
    }

    /// Chiude una scheda, chiedendo prima del salvataggio se il documento
    /// è sporco. È il percorso del pulsante `×`, del tasto centrale e della
    /// voce «Chiudi scheda» del menù contestuale: una sola domanda, una sola
    /// conferma.
    pub(super) fn chiudi_scheda(&mut self, id: DocId) {
        if self.pending_editor_input(id) && !self.restored_input_failed(id) {
            self.notify_persistente(
                NoticeKind::Info,
                "Input in attesa: completa il caricamento prima di chiudere".into(),
            );
            return;
        }
        if self.chiede_salvataggio(id) {
            self.chiedi_chiusura(DaChiudere::Scheda(id));
            if self.restored_input_failed(id)
                && let Some(close) = self.chiusura.as_mut()
            {
                close.errore = Some("Lettura fallita: l'input in attesa non è stato applicato. Annulla per riprovare il caricamento, oppure scegli Non salvare per scartarlo.".into());
            }
        } else {
            self.close_tab(id);
        }
    }

    /// Chiude le altre schede dello stesso gruppo, tenendo questa.
    ///
    /// Una scheda con modifiche non salvate **non** viene buttata via: si dice
    /// quale e il comando si ferma. Chiudere le altre è una scorciatoia, non un
    /// modo per perdere lavoro.
    pub(super) fn chiudi_altre_schede(&mut self, tenere: DocId) -> Task<Message> {
        let Some(gruppo) = self.groups.group_of(tenere) else {
            return self.notify_error("La scheda non è in nessun gruppo".to_owned());
        };
        let altre: Vec<DocId> = self
            .groups
            .tabs(gruppo)
            .iter()
            .copied()
            .filter(|id| *id != tenere)
            .collect();
        if altre.is_empty() {
            return self.notify(
                super::NoticeKind::Info,
                "Non ci sono altre schede da chiudere".to_owned(),
            );
        }
        // Prima le sporche: se una ha modifiche, il comando si ferma prima di
        // chiudere qualcosa, e l'utente sa quale scheda salvare.
        if let Some(sporca) = altre
            .iter()
            .copied()
            .find(|id| self.chiede_salvataggio(*id))
        {
            let nome = self.nome_documento(sporca);
            return self.notify_error(format!(
                "«{nome}» ha modifiche non salvate: salvala o chiudi la scheda prima di chiudere le altre"
            ));
        }
        for id in altre {
            self.close_tab(id);
        }
        self.persist_session()
    }

    pub(super) fn activate_tab(&mut self, id: DocId) -> Task<Message> {
        if !self.groups.contains(id) {
            return Task::none();
        }
        let changed = self.active_doc() != Some(id);
        self.activate_editor_tab(id);
        let loading = self.restored_tab_read_task(id, true);
        self.close_diff_if_primary();
        if changed {
            self.request_editor_focus();
            self.refresh_editor_blame_availability();
            self.queue_editor_blame(id);
        }
        loading
    }

    /// Chiude davvero una scheda, **senza domande**: chi chiama ha già
    /// deciso che le modifiche, se ce n'erano, si possono lasciare.
    pub(super) fn close_tab(&mut self, id: DocId) {
        if self
            .scritture
            .guarda(Some((
                id,
                self.workspace.get(id).and_then(lume_core::Document::path),
            )))
            .file
            != crate::scritture::Presa::Libero
            || self.git_operazione.is_some()
        {
            self.chiedi_chiusura(DaChiudere::Scheda(id));
            return;
        }
        // prima della chiusura: dopo, il documento non c'è più e con
        // lui il path da cui si ricava l'URI
        self.syntax_forget_document(id);
        self.lsp_did_close(id);
        self.pending_restored_tabs.take(id);
        self.restored_notes_revoked.remove(&id);
        self.restored_notes_candidates.remove(&id);
        self.restored_tab_reads.remove(&id);
        self.restored_tab_delivery.remove(&id);
        self.restored_tab_commands.remove(&id);
        self.restored_tab_actions.remove(&id);
        self.doc_marks.remove(&id);
        self.editor_blame_cache.remove(&id);
        self.editor_blame_generation.remove(&id);
        if self
            .completion
            .as_ref()
            .is_some_and(|ui| ui.session.doc == id)
        {
            self.completion_close();
        }
        self.hover_clear();
        self.groups.close(id);
        // La console SQL non e' un file: chiudere la tab non deve
        // buttare via la query scritta, che si ritrova riaprendola.
        // L'albero tree-sitter se ne va comunque — e' la parte
        // grossa, e `open_sql_console` lo ricostruisce.
        if !self.is_sql_console(id) {
            self.workspace.close(id);
            self.disk_fingerprints.remove(&id);
            self.external_conflicts.remove(&id);
            self.pending_fs_rechecks.remove(&id);
        }
        self.syntax.remove(&id);
        self.senza_nome.remove(&id);
        self.save_origins.remove(&id);
        self.scritture.revoca_salvataggi(Some(id));
        if let Some(notes) = self.notes.as_mut()
            && let Some(nota) = notes.tabs.remove(&id)
        {
            // senza scheda non c'e' piu' nulla da salvare: la guardia se ne va
            notes.open_saves.remove(&nota);
        }
        if let Some(notes) = self.notes.as_mut() {
            notes.marcatori.remove(&id);
        }
    }

    /// Derivato dall'identità della console e dal documento, non dalla mappa
    /// delle etichette: anche un buffer pathless non registrato è un file nuovo.
    pub(crate) fn e_senza_nome(&self, id: DocId) -> bool {
        !self.is_sql_console(id)
            && self
                .workspace
                .get(id)
                .is_some_and(|doc| doc.path().is_none())
    }

    /// Apre nel gruppo col focus; rifiuta l'esaurimento prima di mutare lo stato.
    pub(super) fn new_untitled(&mut self) -> Result<DocId, String> {
        let numero = self.senza_nome_seq.checked_add(1).ok_or_else(|| {
            "Numeri dei file senza nome esauriti: impossibile creare un nuovo file".to_owned()
        })?;
        let id = self.workspace.open(None, "");
        self.senza_nome_seq = numero;
        self.senza_nome.insert(id, numero);
        self.groups.open(self.groups.focus(), id);
        self.activate_editor_tab(id);
        self.close_diff_if_primary();
        Ok(id)
    }

    pub(super) fn replace_save_not_started(&mut self, id: DocId, error: String) {
        if let Some(state) = self.replacement.as_mut()
            && let Some(path) = state.autosaves.remove(&id)
        {
            state.report.not_attempted.push(path);
            state.report.error = Some(error);
        }
    }

    // ------------------------------------------------- editor: comandi

    /// Registra il linguaggio senza creare parser o alberi sul thread UI.
    /// Aggancia l'evidenziazione a un documento appena aperto, se il
    /// linguaggio si riconosce dal nome.
    pub(super) fn attach_syntax(&mut self, id: DocId, path: &Path) {
        if let std::collections::hash_map::Entry::Vacant(posto) = self.syntax.entry(id)
            && let Some(lingua) = lume_syntax::detect(path)
            && self.workspace.get(id).is_some()
        {
            posto.insert(SyntaxLanguage(lingua));
        }
    }

    /// Aggiorna i metadati dopo rinomina; il worker rivalida testo e lingua.
    pub(super) fn rebuild_syntax(&mut self, id: DocId, path: &Path) {
        self.editor_blame_cache.remove(&id);
        self.editor_blame_generation.remove(&id);
        self.syntax.remove(&id);
        self.attach_syntax(id, path);
    }

    pub(super) fn resync_syntax(&mut self, id: DocId, _edit: AppliedEdit) {
        // Il blame è dello snapshot richiesto: un edit revoca anche la
        // risposta in volo, senza accodare una nuova lettura Git.
        self.editor_blame_cache.remove(&id);
        self.editor_blame_generation.remove(&id);
    }

    /// Sostituisce la selezione del documento attivo con `text`, caret in coda.
    pub(super) fn editor_replace_selection(&mut self, id: DocId, text: &str) {
        if self.rifiuta_modifica_per_scrittura(id) {
            return;
        }
        self.editor_multi_edit(id, lume_core::multi_edit::MultiEdit::Replace(text));
    }

    pub(super) fn documento_bloccato_da_scrittura(&self, id: DocId) -> bool {
        let path = self.workspace.get(id).and_then(lume_core::Document::path);
        let vista = self.scritture.guarda(Some((id, path)));
        matches!(
            vista.in_volo,
            Some(
                crate::scritture::Tipo::EliminaAppunto
                    | crate::scritture::Tipo::Cestino
                    | crate::scritture::Tipo::Sostituisci
            )
        ) && vista.file == crate::scritture::Presa::Bloccato
    }

    pub(super) fn rifiuta_modifica_per_scrittura(&mut self, id: DocId) -> bool {
        if !self.documento_bloccato_da_scrittura(id) {
            return false;
        }
        let testo = match self.scritture.guarda(None).in_volo {
            Some(crate::scritture::Tipo::Cestino) => {
                "Modifica non possibile: spostamento nel Cestino in corso"
            }
            Some(crate::scritture::Tipo::Sostituisci) => {
                "Modifica non possibile: sostituzione nei file in corso"
            }
            _ => "Modifica non possibile: eliminazione dell'appunto in corso",
        };
        self.notify_persistente(NoticeKind::Info, testo.into());
        true
    }

    pub(super) fn rifiuta_comando_per_scrittura(&mut self, id: DocId, cmd: &EditorCommand) -> bool {
        matches!(
            cmd,
            EditorCommand::Insert(_)
                | EditorCommand::Paste(_)
                | EditorCommand::Tab
                | EditorCommand::Enter
                | EditorCommand::Backspace
                | EditorCommand::Delete
                | EditorCommand::Cut
                | EditorCommand::Undo
                | EditorCommand::Redo
                | EditorCommand::PopupAccept
        ) && self.rifiuta_modifica_per_scrittura(id)
    }

    /// Consuma i segnali di budget di tutti i documenti: più revisioni
    /// espulse nello stesso gesto producono una sola notifica non modale.
    pub(super) fn take_history_trimmed(&mut self) -> bool {
        let ids: Vec<_> = self.workspace.ids().collect();
        ids.into_iter().any(|id| {
            self.workspace
                .get_mut(id)
                .is_some_and(lume_core::Document::take_history_trimmed)
        })
    }

    pub(super) fn history_trim_notice(&mut self) -> Task<Message> {
        self.notify(
            NoticeKind::Info,
            "Cronologia undo ridotta per rispettare il budget di memoria.".to_owned(),
        )
    }

    pub(super) fn editor_apply_motion(&mut self, id: DocId, motion: Motion, extend: bool) {
        let nascoste = self.blocchi_compressi(id);
        let Some(doc) = self.workspace.get_mut(id) else {
            return;
        };
        let rope = doc.rope();
        let ranges = doc
            .selections()
            .ranges()
            .iter()
            .map(|sel| {
                let head = sel.head;
                let col = head - rope.line_to_char(rope.char_to_line(head.min(rope.len_chars())));
                let new_head = match motion {
                    Motion::Left => movement::prev_grapheme(rope, head),
                    Motion::Right => movement::next_grapheme(rope, head),
                    Motion::WordLeft => movement::word_left(rope, head),
                    Motion::WordRight => movement::word_right(rope, head),
                    Motion::Up => movement::line_up(rope, head, col),
                    Motion::Down => movement::line_down(rope, head, col),
                    Motion::Home => movement::line_home_smart(rope, head),
                    Motion::End => movement::line_end(rope, head),
                    Motion::DocStart => movement::doc_start(rope),
                    Motion::DocEnd => movement::doc_end(rope),
                };
                let avanti = matches!(
                    motion,
                    Motion::Right | Motion::WordRight | Motion::Down | Motion::End | Motion::DocEnd
                );
                let colonna = matches!(motion, Motion::Up | Motion::Down).then_some(col);
                let new_head =
                    fuori_dalle_righe_nascoste(rope, &nascoste, new_head, avanti, colonna);
                if extend {
                    Range::new(sel.anchor, new_head)
                } else {
                    Range::point(new_head)
                }
            })
            .collect();
        if let Ok(set) = lume_core::SelectionSet::try_new(
            ranges,
            doc.selections().primary_index(),
            rope.len_chars(),
        ) {
            doc.set_selections(set);
        }
    }

    pub(super) fn editor_apply_page(
        &mut self,
        id: DocId,
        direction: PageDirection,
        lines: usize,
        extend: bool,
    ) {
        let nascoste = self.blocchi_compressi(id);
        let Some(doc) = self.workspace.get_mut(id) else {
            return;
        };
        let rope = doc.rope();
        let ranges = doc
            .selections()
            .ranges()
            .iter()
            .map(|sel| {
                let head = sel.head;
                let col = head - rope.line_to_char(rope.char_to_line(head.min(rope.len_chars())));
                let proiezione = lume_ui::editor::FoldProjectionRef::new(&nascoste);
                let riga = rope.char_to_line(head.min(rope.len_chars()));
                let visiva = proiezione.doc_to_visual(riga);
                let destinazione = match direction {
                    PageDirection::Up => visiva.saturating_sub(lines.max(1)),
                    PageDirection::Down => visiva.saturating_add(lines.max(1)),
                };
                let destinazione = destinazione.min(
                    proiezione
                        .visible_line_count(rope.len_lines())
                        .saturating_sub(1),
                );
                let riga = proiezione.visual_to_doc(destinazione, rope.len_lines());
                let new_head =
                    rope.line_to_char(riga) + col.min(movement::line_content_len(rope, riga));
                if extend {
                    Range::new(sel.anchor, new_head)
                } else {
                    Range::point(new_head)
                }
            })
            .collect();
        if let Ok(set) = lume_core::SelectionSet::try_new(
            ranges,
            doc.selections().primary_index(),
            rope.len_chars(),
        ) {
            doc.set_selections(set);
        }
    }

    /// Copia la selezione del documento attivo nella clipboard di sistema.
    /// Serve solo al percorso da **menù**: da tastiera la clipboard la
    /// scrive già il widget (`command_for_key` in `lume-ui`) prima di pubblicare
    /// `EditorCommand::Copy`, che a livello app è apposta un no-op
    /// (`EditorCommand::Copy => {}`, poco sopra in questo file).
    pub(super) fn copy_selection_to_clipboard(&mut self) -> Task<Message> {
        let Some(id) = self.active_doc() else {
            return Task::none();
        };
        let action = DeferredRestoredAction::CopySelection;
        if self.pending_restored_tabs.contains(id) {
            return self.defer_restored_action(id, action);
        }
        self.forget_restored_action(id, &action);
        self.copy_selection_for(id)
    }

    pub(super) fn copy_selection_for(&mut self, id: DocId) -> Task<Message> {
        if self.active_doc() != Some(id) || self.pending_restored_tabs.contains(id) {
            return Task::none();
        }
        let Some(doc) = self.workspace.get(id) else {
            return Task::none();
        };
        let Some(testo) = lume_core::multi_edit::selected_text(doc) else {
            return Task::none();
        };
        iced::clipboard::write(testo)
    }

    pub(super) fn cut_selection_to_clipboard(&mut self) -> Task<Message> {
        let Some(id) = self.active_doc() else {
            return Task::none();
        };
        let action = DeferredRestoredAction::CutSelection;
        if self.pending_restored_tabs.contains(id) {
            return self.defer_restored_action(id, action);
        }
        self.forget_restored_action(id, &action);
        let copy = self.copy_selection_for(id);
        self.apply_editor_command_for(id, EditorCommand::Cut);
        Task::batch([copy, self.lsp_change_task()])
    }

    pub(super) fn apply_editor_command(&mut self, cmd: EditorCommand) -> Task<Message> {
        let Some(id) = self.active_doc() else {
            return Task::none();
        };
        if cmd == EditorCommand::Save
            && let Some(testo) = self.rifiuto_salvataggio()
        {
            return self.notify(NoticeKind::Info, testo);
        }
        if self.rifiuta_comando_per_scrittura(id, &cmd) {
            return Task::none();
        }
        if cmd == EditorCommand::AddNextOccurrence {
            return self.next_occurrence_for(id);
        }
        if self.pending_restored_tabs.contains(id) {
            match &cmd {
                EditorCommand::Copy => {
                    return self.defer_restored_action(id, DeferredRestoredAction::CopySelection);
                }
                EditorCommand::Cut => {
                    return self.defer_restored_action(id, DeferredRestoredAction::CutSelection);
                }
                EditorCommand::TriggerComplete => {
                    return self.defer_restored_action(id, DeferredRestoredAction::Completion);
                }
                EditorCommand::Save => {
                    return self.defer_restored_action(id, DeferredRestoredAction::SaveDocument);
                }
                _ if can_defer_restored_command(&cmd) => {
                    let _ = self.queue_restored_editor_command(id, cmd);
                    return self.restored_tab_read_task(id, false);
                }
                _ => {
                    self.notify_persistente(
                        NoticeKind::Info,
                        "Comando rinviato: attendi il caricamento".into(),
                    );
                    return Task::none();
                }
            }
        }
        self.apply_editor_command_for(id, cmd);
        Task::none()
    }

    pub(super) fn apply_editor_command_for(&mut self, id: DocId, cmd: EditorCommand) {
        if self.rifiuta_comando_per_scrittura(id, &cmd) {
            return;
        }
        match cmd {
            EditorCommand::ToggleCursor { .. }
            | EditorCommand::AddCursorLine { .. }
            | EditorCommand::KeepPrimary => self.editor_selection_action(id, cmd),
            EditorCommand::AddNextOccurrence => self.notify_persistente(
                NoticeKind::Info,
                "La ricerca richiede il percorso asincrono".into(),
            ),
            EditorCommand::Insert(s) | EditorCommand::Paste(s) => {
                self.editor_replace_selection(id, &s)
            }
            EditorCommand::Tab => self.editor_multi_edit(id, lume_core::multi_edit::MultiEdit::Tab),
            EditorCommand::Enter => {
                self.editor_multi_edit(id, lume_core::multi_edit::MultiEdit::Enter)
            }
            EditorCommand::Backspace => {
                self.editor_multi_edit(id, lume_core::multi_edit::MultiEdit::Backspace)
            }
            EditorCommand::Delete => {
                self.editor_multi_edit(id, lume_core::multi_edit::MultiEdit::Delete)
            }
            EditorCommand::Cut => self.editor_multi_edit(id, lume_core::multi_edit::MultiEdit::Cut),
            // la clipboard è già stata scritta dal widget
            EditorCommand::Copy => {}
            EditorCommand::TriggerComplete => self.request_completion_for(id),
            EditorCommand::PopupUp => {
                if let Some(ui) = self.completion.as_mut() {
                    ui.session.select_prev();
                    ui.sync();
                }
            }
            EditorCommand::PopupDown => {
                if let Some(ui) = self.completion.as_mut() {
                    ui.session.select_next();
                    ui.sync();
                }
            }
            EditorCommand::PopupAccept => {
                let Some(ui) = self.completion.take() else {
                    return;
                };
                self.completion_wait = None;
                if ui.session.doc != id {
                    return;
                }
                let Some(documento) = self.workspace.get(id) else {
                    return;
                };
                let Some(sostituzione) = ui
                    .session
                    .accept(documento.rope(), documento.selection().head)
                else {
                    return;
                };
                self.editor_replace_range(
                    id,
                    sostituzione.range.start,
                    sostituzione.range.end,
                    &sostituzione.text,
                );
            }
            EditorCommand::PopupDismiss => self.completion_close(),
            // Cablato nel Task 13 (hover).
            // Gestito da `hover_at` in `update`: qui non arriva mai.
            EditorCommand::HoverAt(_) => {}
            // Gestito dal ramo dedicato in `update`, che può aprire l'appunto.
            EditorCommand::NoteMarkerClicked { .. } => {}
            // Gestito dal ramo dedicato in `update`, che conosce il gruppo.
            EditorCommand::FoldClicked { .. } => {}
            EditorCommand::ContextMenu {
                pos,
                screen_x,
                screen_y,
            } => {
                let selezione = self.workspace.get(id).and_then(|doc| {
                    doc.selections()
                        .ranges()
                        .iter()
                        .find(|range| (range.min()..=range.max()).contains(&pos))
                        .copied()
                });
                if let Some(doc) = self.workspace.get_mut(id) {
                    if let Some(range) = selezione {
                        // Diventa principale la selezione cliccata, senza
                        // cancellare il resto dell'insieme.
                        let primary = doc
                            .selections()
                            .ranges()
                            .iter()
                            .position(|candidate| *candidate == range)
                            .unwrap_or(0);
                        if let Ok(set) = lume_core::SelectionSet::try_new(
                            doc.selections().ranges().to_vec(),
                            primary,
                            doc.rope().len_chars(),
                        ) {
                            doc.set_selections(set);
                        }
                    } else {
                        doc.set_selection(Range::point(pos.min(doc.rope().len_chars())));
                    }
                }
                let selezione = selezione.filter(|range| !range.is_empty());
                self.context_menu = Some(crate::context_menu::ContextMenu {
                    x: screen_x as f32,
                    y: screen_y as f32,
                    soggetto: crate::context_menu::Soggetto::Editor { pos, selezione },
                });
                self.open_context_menu();
            }
            EditorCommand::Move(motion, extend) => self.editor_apply_motion(id, motion, extend),
            EditorCommand::MovePage {
                direction,
                lines,
                extend,
            } => self.editor_apply_page(id, direction, lines, extend),
            EditorCommand::SetCursor { pos, extend } => {
                let Some(doc) = self.workspace.get_mut(id) else {
                    return;
                };
                let sel = doc.selection();
                doc.set_selection(if extend {
                    Range::new(sel.anchor, pos)
                } else {
                    Range::point(pos)
                });
            }
            EditorCommand::Undo => {
                if let Some(edit) = self.workspace.get_mut(id).and_then(|d| d.undo()) {
                    self.resync_syntax(id, edit);
                }
            }
            EditorCommand::Redo => {
                if let Some(edit) = self.workspace.get_mut(id).and_then(|d| d.redo()) {
                    self.resync_syntax(id, edit);
                }
            }
            EditorCommand::SelectAll => {
                if let Some(doc) = self.workspace.get_mut(id) {
                    let len = doc.rope().len_chars();
                    doc.set_selection(Range::new(0, len));
                }
            }
            EditorCommand::Save => {
                // il salvataggio vero è nel ramo Message::Editor (ha bisogno del Task)
                if let Some(doc) = self.workspace.get_mut(id) {
                    doc.commit_undo_group();
                }
            }
        }
    }

    pub(super) fn toggle_bookmark_for(&mut self, id: DocId) -> Task<Message> {
        if self.active_doc() != Some(id) || self.pending_restored_tabs.contains(id) {
            return Task::none();
        }
        let Some(doc) = self.workspace.get(id) else {
            return Task::none();
        };
        let Some(path) = doc.path().map(Path::to_path_buf) else {
            return Task::none();
        };
        let rope = doc.rope();
        let line_index = rope.char_to_line(doc.selection().head.min(rope.len_chars()));
        let preview: String = rope
            .line(line_index)
            .to_string()
            .trim()
            .chars()
            .take(60)
            .collect();
        let bookmark = lume_store::BookmarkRecord {
            path,
            line: line_index + 1,
            preview,
        };
        let location = format!(
            "{}:{}",
            tools::bookmarks::display_name(&bookmark.path),
            bookmark.line
        );
        let added = tools::bookmarks::toggle(&mut self.bookmarks, bookmark);
        self.sync_bookmarks_tool();
        let text = if added {
            format!("Segnalibro aggiunto: {location}")
        } else {
            format!("Segnalibro rimosso: {location}")
        };
        self.notify(NoticeKind::Info, text)
    }

    /// Avviso comune ai salvataggi richiesti durante le scritture migrate.
    pub(super) fn rifiuto_salvataggio(&self) -> Option<String> {
        if let Some(in_volo) = self.scritture.guarda(None).in_volo
            && in_volo != crate::scritture::Tipo::Salvataggio
        {
            return Some(
                crate::scritture::Rifiuto {
                    in_volo,
                    richiesto: crate::scritture::Tipo::Salvataggio,
                    rimandato: false,
                }
                .avviso(),
            );
        }
        self.git_operazione.map(|op| {
            if op.worktree {
                "Salvataggio non possibile: operazione Git sul worktree in corso"
            } else {
                "Salvataggio non possibile: operazione Git in corso"
            }
            .to_owned()
        })
    }

    pub(super) fn queue_save(&mut self, id: DocId) {
        self.queue_save_per(id, crate::scritture::Causa::Utente);
    }

    pub(super) fn queue_save_per(&mut self, id: DocId, causa: crate::scritture::Causa) {
        use crate::scritture::{Op, Permesso};
        let Some(doc) = self.workspace.get(id) else {
            return;
        };
        let op = Op::Salva {
            doc: id,
            versione: doc.version(),
            causa,
        };
        let origin = SaveOrigin {
            epoch: self.git_root_epoch,
            path: doc.path().map(Path::to_path_buf),
            note: self.notes.as_ref().and_then(|n| {
                n.tabs
                    .get(&id)
                    .map(|note| (n.scan_generation, note.clone()))
            }),
        };
        let permission =
            if self.git_operazione.is_some() && self.scritture.guarda(None).in_volo.is_none() {
                Err(self.scritture.rimanda_esterno(op))
            } else {
                self.scritture.ammetti(op)
            };
        match permission {
            Ok(Permesso::Avvia(lease)) => self.save_active_origin = Some((lease, origin)),
            Ok(Permesso::Accodato) => {
                self.save_origins.insert(id, origin);
                self.notify_persistente(
                    NoticeKind::Info,
                    "Salvataggio accodato: riparte dopo quello in corso".into(),
                );
            }
            Err(rifiuto) => {
                if rifiuto.rimandato {
                    self.save_origins.insert(id, origin);
                }
                self.notify_persistente(NoticeKind::Info, rifiuto.avviso());
            }
            Ok(_) => self.notify_persistente(
                NoticeKind::Error,
                "Salvataggio non avviato: permesso non valido".into(),
            ),
        }
    }

    pub(super) fn save_doc(&mut self, id: DocId) -> Task<Message> {
        self.queue_save(id);
        Task::none()
    }

    /// Prepara soltanto metadati e Rope condivisa: nessun I/O sul thread UI.
    pub(super) fn prepare_save(
        &mut self,
        id: DocId,
        version: u64,
    ) -> Result<Option<crate::save::Request>, String> {
        if self.pending_restored_tabs.contains(id) {
            return Err("Testo non caricato: annulla la chiusura e riprova il caricamento, oppure scarta esplicitamente l'input".into());
        }
        if self.is_sql_console(id) {
            return Ok(None);
        }
        let Some(doc) = self.workspace.get(id) else {
            return Ok(None);
        };
        if self.external_conflicts.contains(&id) {
            return Err(format!(
                "{} è cambiato sul disco mentre il buffer era modificato: usa «Salva con nome» o ricarica il file prima di salvare",
                self.nome_documento(id)
            ));
        }
        // Dire «salvato» qui perderebbe il buffer dal riquadro di chiusura.
        // Nessun ticket/epoch o I/O per i file nuovi, puliti o sporchi.
        if self.e_senza_nome(id) && doc.is_dirty() {
            return Err(format!("{} non ha ancora un file", self.nome_documento(id)));
        }
        let Some(path) = doc.path().map(Path::to_path_buf) else {
            return Ok(None);
        };
        if doc.version() != version {
            return Err(format!(
                "Il testo di {} è cambiato in attesa del salvataggio: salva di nuovo",
                path.display()
            ));
        }
        let note_metadata_save = self
            .notes
            .as_ref()
            .is_some_and(|notes| notes.link_saves.contains_key(&id));
        if !doc.is_dirty() && !note_metadata_save {
            return Ok(None);
        }
        let note = if let Some(notes) = self.notes.as_ref()
            && let Some(nota) = notes.tabs.get(&id)
        {
            let store = notes.store.clone().ok_or_else(|| {
                format!(
                    "Salvataggio di {} non riuscito: la nota non è più caricata",
                    path.display()
                )
            })?;
            let loaded = notes
                .loaded
                .get(nota)
                .map(OpenNoteSaveState::from_loaded)
                .map(|guard| guard.to_loaded(String::new()))
                .or_else(|| {
                    notes
                        .open_saves
                        .get(nota)
                        .map(|guard| guard.to_loaded(String::new()))
                })
                .ok_or_else(|| {
                    format!(
                        "Salvataggio di {} non riuscito: la nota non è più caricata",
                        path.display()
                    )
                })?;
            Some(crate::save::NoteSave {
                scope: notes.scan_generation,
                id: nota.clone(),
                store,
                loaded,
            })
        } else {
            None
        };
        let expected_fingerprint = if note.is_none() {
            Some(self.disk_fingerprints.get(&id).copied().ok_or_else(|| {
                format!(
                    "Impronta del file {} non disponibile: riapri il file prima di salvarlo",
                    path.display()
                )
            })?)
        } else {
            None
        };
        let Some(doc) = self.workspace.get_mut(id) else {
            return Ok(None);
        };
        let snapshot = doc.snapshot_for_save();
        let ticket = self.saves.start(id);
        Ok(Some(crate::save::Request {
            ticket,
            path,
            snapshot,
            note,
            aliases: None,
            expected_fingerprint,
        }))
    }

    pub(super) fn save_failed(&mut self, error: String) -> Task<Message> {
        // «Non salvare» aspetta solo il worker; «Salva» non chiude su errore.
        if self.close_after_save.is_some_and(|(_, _, save)| save) {
            self.close_after_save = None;
        }
        if let Some(close) = self.chiusura.as_mut() {
            close.errore = Some(error.clone());
        }
        self.notify_error(error)
    }

    pub(super) fn finish_save(&mut self, mut completion: crate::save::Completion) -> Task<Message> {
        if !self.saves.finish(completion.ticket) {
            return Task::none();
        }
        let causa = if self
            .save_causa
            .is_some_and(|(ticket, _)| ticket == completion.ticket)
        {
            self.save_causa.take().map(|(_, causa)| causa)
        } else {
            None
        };
        let esito = if completion.result.is_ok() {
            crate::scritture::Esito::Riuscito
        } else {
            crate::scritture::Esito::Fallito
        };
        let lease = self.save_lease.take();
        if lease.is_none()
            && self.save_as_flight.is_none()
            && let Some(state) = self.replacement.as_ref()
            && state.autosaves.contains_key(&completion.ticket.document)
        {
            self.scritture.avanzamento_sostituzione(state.lease);
        }
        if self
            .save_as_flight
            .as_ref()
            .is_some_and(|r| r.ticket == completion.ticket)
            && let Some(rebind) = self.save_as_flight.take()
        {
            return self.finish_save_as(completion, rebind);
        }
        let id = completion.ticket.document;
        let snapshot_version = completion.snapshot.version();
        let autosave = self
            .notes
            .as_ref()
            .and_then(|notes| match notes.salvataggi.get(&id) {
                Some(crate::autosave::StatoSalvataggio::InCorso { versione })
                    if *versione == snapshot_version =>
                {
                    Some(true)
                }
                _ => None,
            })
            .is_some();
        let link_save = self
            .notes
            .as_ref()
            .and_then(|notes| notes.link_saves.get(&id).cloned());
        if let Some(state) = self.replacement.as_mut()
            && let Some(path) = state.autosaves.remove(&id)
        {
            if completion.result.is_ok() {
                state.report.committed.push(path);
                if self
                    .workspace
                    .get(id)
                    .is_some_and(|d| d.version() != completion.snapshot.version())
                {
                    state.dirty += 1;
                }
            } else {
                state.report.save_failures.push((
                    path,
                    completion
                        .result
                        .as_ref()
                        .err()
                        .cloned()
                        .unwrap_or_default(),
                ));
            }
        }
        let mut tasks = Vec::new();
        // Prima degli esiti: un errore successivo non deve essere coperto
        // dall'avviso, che resta comunque nel registro errori.
        if let Some(warning) = completion.warning.take() {
            tasks.push(self.notify(NoticeKind::Error, warning));
        }
        match completion.result {
            Err(error)
                if error.contains("cambiato sul disco") || error.contains("cambiato su disco") =>
            {
                self.external_conflicts.insert(id);
                let note_conflict = self
                    .notes
                    .as_ref()
                    .is_some_and(|notes| notes.tabs.contains_key(&id));
                if autosave || note_conflict {
                    if let Some(notes) = self.notes.as_mut()
                        && let Some(state) = notes.salvataggi.get_mut(&id)
                    {
                        state.in_conflitto();
                    }
                    if link_save.is_some() {
                        if let Some(notes) = self.notes.as_mut() {
                            notes.link_saves.remove(&id);
                        }
                        tasks.push(self.save_failed(error));
                    } else {
                        tasks.push(self.save_failed(error));
                    }
                } else {
                    tasks.push(self.save_failed(format!(
                    "{} è cambiato sul disco durante il salvataggio: il buffer locale resta conservato e il save è bloccato; usa «Salva con nome» o ricarica il file",
                    completion.path.display()
                    )));
                }
            }
            Err(error) => {
                if autosave
                    && let Some(notes) = self.notes.as_mut()
                    && let Some(state) = notes.salvataggi.get_mut(&id)
                {
                    state.fallito(error.clone());
                }
                tasks.push(self.save_failed(error));
                if link_save.is_some() {
                    if let Some(notes) = self.notes.as_mut() {
                        notes.link_saves.remove(&id);
                    }
                    tasks.push(self.save_failed(
                        "Non è stato possibile salvare il collegamento all'appunto".into(),
                    ));
                }
            }
            Ok(note) => {
                let same_path = self
                    .workspace
                    .get(id)
                    .is_some_and(|doc| doc.path() == Some(completion.path.as_path()));
                let mut saved_path = completion.path.clone();
                let mut note_update_error = None;
                let mut rename_warning = None;
                let snapshot_accepted = self
                    .workspace
                    .get_mut(id)
                    .is_some_and(|doc| same_path && doc.mark_snapshot_saved(&completion.snapshot));
                if same_path {
                    if let Some(fingerprint) = completion.fingerprint {
                        self.disk_fingerprints.insert(id, fingerprint);
                    }
                    self.external_conflicts.remove(&id);
                    if let Some(saved) = note {
                        saved_path = saved.loaded.path().to_path_buf();
                        rename_warning = saved.rename_error;
                        note_update_error = self
                            .nota_salvata(id, saved.scope, &saved.id, saved.loaded)
                            .err();
                    }
                }
                let accepted = snapshot_accepted && note_update_error.is_none();
                if accepted {
                    if !autosave
                        && let Some(state) = self
                            .notes
                            .as_mut()
                            .and_then(|notes| notes.salvataggi.get_mut(&id))
                    {
                        state.tornato_pulito();
                    }
                    self.lsp_did_save(id);
                    if !autosave || causa == Some(crate::scritture::Causa::Utente) {
                        self.push_history(
                            lume_store::HistoryAction::Saved,
                            tools::bookmarks::display_name(&saved_path),
                            Some(saved_path),
                        );
                    }
                    if autosave {
                        tasks.push(self.notes_autosave_completed(id, snapshot_version));
                    }
                } else {
                    if let Some(state) = self.replacement.as_mut() {
                        state.report.error = Some("documento o percorso cambiato dopo la scrittura: verifica il contenuto".into());
                    }
                    if autosave {
                        // La snapshot è stata scritta con successo ma il buffer
                        // è cambiato durante l'I/O: la completion riarmarà il timer.
                        tasks.push(self.notes_autosave_completed(id, snapshot_version));
                    } else if note_update_error.is_none() {
                        tasks.push(self.save_failed("Il file è stato scritto, ma il documento o il percorso è cambiato: verifica il contenuto".into()));
                    }
                }
                let note_update_ok = note_update_error.is_none();
                if let Some(error) = note_update_error {
                    tasks.push(self.save_failed(error));
                }
                if let Some(warning) = rename_warning {
                    tasks.push(self.notify(
                        NoticeKind::Info,
                        format!("Il file non è stato rinominato: {warning}"),
                    ));
                }
                if let Some(notice) = link_save {
                    if let Some(notes) = self.notes.as_mut() {
                        notes.link_saves.remove(&id);
                    }
                    if same_path && note_update_ok {
                        tasks.push(self.notify(NoticeKind::Info, notice));
                    }
                }
            }
        }
        if let Some(lease) = lease {
            self.scritture.rilascia(lease, esito);
        }
        tasks.push(self.reload_pending_fs_recheck(id));
        tasks.push(self.start_replace_save());
        tasks.push(self.finish_close_after_saves());
        Task::batch(tasks)
    }

    pub(super) fn finish_save_as(
        &mut self,
        completion: crate::save::Completion,
        rebind: crate::save::Rebind,
    ) -> Task<Message> {
        let id = completion.ticket.document;
        let path = completion.path;
        let mut tasks = Vec::new();
        let proprietario = self
            .scritture
            .possiede(rebind.lease, crate::scritture::Tipo::SalvaConNome);
        if proprietario {
            let esito = if completion.result.is_ok() {
                crate::scritture::Esito::Riuscito
            } else {
                crate::scritture::Esito::Fallito
            };
            self.scritture.rilascia(rebind.lease, esito);
        }
        if let Some(warning) = completion.warning {
            tasks.push(self.notify(NoticeKind::Error, warning));
        }
        if let Err(error) = completion.result {
            tasks.push(self.save_failed(format!(
                "Salvataggio di {} fallito: {error}; esito della destinazione da verificare, nessun rollback",
                path.display()
            )));
        } else {
            let current = proprietario
                && self.project_root == rebind.root
                && self.git_root_epoch == rebind.root_epoch
                && self.replace_mapping() == rebind.mapping
                && self
                    .workspace
                    .get_by_path(&path)
                    .is_none_or(|other| other == id)
                && !completion.resolved.as_deref().is_some_and(|p| {
                    self.workspace
                        .get_by_path(p)
                        .is_some_and(|other| other != id)
                });
            let accepted = current
                && self.workspace.get_mut(id).is_some_and(|doc| {
                    doc.path() == rebind.source.as_deref()
                        && doc.mark_snapshot_saved(&completion.snapshot)
                });
            if accepted {
                // Nessun await/interleave tra preflight, ack e rebind. L'ack
                // precede la revoca dell'identità Arc del vecchio legame.
                match self.workspace.rebind_path(id, path.clone()) {
                    Ok(()) => {
                        if let Some(fingerprint) = completion.fingerprint {
                            self.disk_fingerprints.insert(id, fingerprint);
                        }
                        self.external_conflicts.remove(&id);
                        let changed = rebind.source.as_deref() != Some(path.as_path());
                        if changed {
                            self.editor_blame_cache.remove(&id);
                            self.editor_blame_generation.remove(&id);
                            self.lsp_did_close(id);
                            self.senza_nome.remove(&id);
                            self.syntax.remove(&id);
                            self.attach_syntax(id, &path);
                            self.lsp_did_open(id);
                            self.refresh_editor_blame_availability();
                            self.queue_editor_blame(id);
                            if self.scritture.revoca_salvataggi(Some(id)) > 0 {
                                self.save_origins.remove(&id);
                                self.termina_autosave_non_avviato(id, None);
                                tasks.push(self.notify(NoticeKind::Info, "Salvataggi in attesa sul vecchio nome revocati: le modifiche più recenti restano da salvare".into()));
                            }
                        } else {
                            self.lsp_did_save(id);
                        }
                        self.refresh_project_tree(std::slice::from_ref(&path));
                        self.push_history(lume_store::HistoryAction::Saved, tools::bookmarks::display_name(&path), Some(path.clone()));
                        tasks.push(self.persist_session());
                    }
                    Err(error) => tasks.push(self.save_failed(format!("Destinazione scritta e snapshot riconosciuto, ma documento non rilegato: {error}; verifica lo stato, nessun rollback"))),
                }
            } else {
                self.refresh_project_tree(std::slice::from_ref(&path));
                tasks.push(self.save_failed("Destinazione scritta, ma documento/revisione/legame o progetto non più attuali: nessun rebind o rollback, verifica il contenuto".into()));
            }
        }
        tasks.push(self.reload_pending_fs_recheck(id));
        Task::batch(tasks)
    }

    pub(super) fn finish_close_after_saves(&mut self) -> Task<Message> {
        if self.scritture.guarda(None).in_volo.is_some() || self.git_operazione.is_some() {
            return Task::none();
        }
        let Some((epoch, object, save)) = self.close_after_save.take() else {
            return self.chiusura_senza_piu_motivo();
        };
        if epoch != self.close_epoch
            || !self
                .chiusura_attiva()
                .is_some_and(|close| close.oggetto == object)
        {
            return Task::none();
        }
        let dirty = match object {
            DaChiudere::Scheda(id) => self.chiede_salvataggio(id),
            DaChiudere::Finestra(_) => !self.documenti_sporchi().is_empty(),
        };
        if save && dirty {
            return self.save_failed("Il testo è cambiato durante il salvataggio: controlla e salva di nuovo prima di chiudere".into());
        }
        self.chiusura = None;
        if matches!(object, DaChiudere::Finestra(_)) {
            self.invalida_attese_uscita();
        }
        self.chiudi_overlay_se(OverlayId::ConfermaChiusura);
        match object {
            DaChiudere::Scheda(id) => {
                self.close_tab(id);
                self.persist_session()
            }
            DaChiudere::Finestra(window) => self.esci(window),
        }
    }

    /// Il riquadro «Salvare le modifiche?» aperto mentre una scrittura era in
    /// volo: finite le scritture, se non resta nulla da perdere la chiusura
    /// procede come se l'utente l'avesse chiesta adesso. Con un errore nel
    /// riquadro o un file ancora sporco la domanda resta aperta.
    pub(super) fn chiusura_senza_piu_motivo(&mut self) -> Task<Message> {
        let Some(chiusura) = self.chiusura_attiva() else {
            return Task::none();
        };
        if chiusura.errore.is_some() {
            return Task::none();
        }
        let notifiche_dal = chiusura.notifiche_dal;
        // Un errore comparso mentre il riquadro era aperto (scrittura, Git,
        // sincronizzazione della cartella) resta visibile nel riquadro.
        let errore_recente = self
            .notice
            .as_ref()
            .filter(|(id, notice)| *id > notifiche_dal && matches!(notice.kind, NoticeKind::Error))
            .map(|(_, notice)| notice.text.clone());
        if let Some(errore) = errore_recente {
            if let Some(chiusura) = self.chiusura.as_mut() {
                chiusura.errore = Some(errore);
            }
            return Task::none();
        }
        let oggetto = chiusura.oggetto;
        let sporco = match oggetto {
            DaChiudere::Scheda(id) => self.chiede_salvataggio(id),
            DaChiudere::Finestra(_) => !self.documenti_sporchi().is_empty(),
        };
        if sporco {
            return Task::none();
        }
        self.chiusura = None;
        if matches!(oggetto, DaChiudere::Finestra(_)) {
            self.invalida_attese_uscita();
        }
        self.chiudi_overlay_se(OverlayId::ConfermaChiusura);
        match oggetto {
            DaChiudere::Scheda(id) => {
                self.close_tab(id);
                self.persist_session()
            }
            DaChiudere::Finestra(window) => self.esci(window),
        }
    }

    /// Aiutante dei test storici: guida gli stessi Task/worker/completion
    /// della UI, non un secondo percorso sincrono di scrittura.
    #[cfg(test)]
    #[allow(clippy::expect_used)]
    pub(super) fn completa_salvataggi(&mut self, task: Task<Message>) {
        use futures::StreamExt;
        // Il chooser è occupato ma non è un worker: i test non pollano il
        // dialogo nativo. La sua risposta viene guidata separatamente.
        if self.save_as_picker.is_some() {
            return;
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime test");
        runtime.block_on(async {
            let mut streams = futures::stream::SelectAll::new();
            if let Some(stream) = iced_runtime::task::into_stream(task) {
                streams.push(stream);
            }
            while (self.scritture.guarda(None).in_volo.is_some() || self.git_operazione.is_some())
                && self.save_as_picker.is_none()
            {
                let action = streams.next().await.expect("completion del worker");
                if let iced_runtime::Action::Output(message) = action {
                    let next = self.update(message);
                    if let Some(stream) = iced_runtime::task::into_stream(next) {
                        streams.push(stream);
                    }
                }
            }
        });
    }

    #[cfg(test)]
    pub(super) fn write_doc(&mut self, id: DocId) -> Result<(), String> {
        let task = self.update(Message::SaveDocFixture(id));
        self.completa_salvataggi(task);
        if self.chiede_salvataggio(id) {
            Err(self
                .notice
                .as_ref()
                .map(|(_, notice)| notice.text.clone())
                .unwrap_or_else(|| "Salvataggio non riuscito".into()))
        } else {
            Ok(())
        }
    }

    pub(super) fn save_doc_active(&mut self) -> Task<Message> {
        if let Some(testo) = self.rifiuto_salvataggio() {
            return self.notify(NoticeKind::Info, testo);
        }
        match self.active_doc() {
            Some(id) if self.e_senza_nome(id) => self.pick_save_path(id),
            Some(id) if self.pending_restored_tabs.contains(id) => {
                self.defer_restored_action(id, DeferredRestoredAction::SaveDocument)
            }
            Some(id) => {
                self.forget_restored_action(id, &DeferredRestoredAction::SaveDocument);
                self.save_doc(id)
            }
            None => Task::none(),
        }
    }

    pub(super) fn puo_salvare_con_nome(&self, id: DocId) -> bool {
        self.workspace.get(id).is_some()
            && matches!(self.editor_kind(id), crate::editor_groups::EditorKind::Code)
    }

    pub(super) fn save_as_active(&mut self) -> Task<Message> {
        match self
            .active_doc()
            .filter(|&id| self.puo_salvare_con_nome(id))
        {
            Some(id) => self.pick_save_path(id),
            None => self.notify(
                NoticeKind::Info,
                "Salva con nome: nessun file aperto nell'editor".to_owned(),
            ),
        }
    }

    pub(super) fn pick_save_path(&mut self, id: DocId) -> Task<Message> {
        if !self.puo_salvare_con_nome(id) {
            return self.notify_error("«Salva con nome» vale per i file dell'editor: la console SQL e gli Appunti si salvano da soli".into());
        }
        if self.pending_restored_tabs.contains(id) {
            if self.scritture.guarda(None).in_volo.is_some()
                || self.git_operazione.is_some()
                || self.chiusura.is_some()
            {
                return self.notify_error("Un'operazione file o una chiusura è in corso: attendi prima di scegliere il nome".into());
            }
            return self.defer_restored_action(id, DeferredRestoredAction::SaveAs);
        }
        self.forget_restored_action(id, &DeferredRestoredAction::SaveAs);
        let Some(sequence) = self.save_as_sequence.checked_add(1) else {
            return self
                .notify_error("Richieste Salva con nome esaurite: nessun dialogo avviato".into());
        };
        let Some(doc) = self.workspace.get(id) else {
            return Task::none();
        };
        let current = doc.path().map(Path::to_path_buf);
        let ending = doc.line_ending();
        let bom = doc.bom();
        let directory = current
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.project_root.clone());
        // Solo hint UTF8 esatto: niente nome operativo lossy.
        let filename = current
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .map(str::to_owned);
        let lease = match self.ammetti_salva_con_nome(id) {
            Ok(lease) => lease,
            Err(testo) => return self.notify(NoticeKind::Info, testo),
        };
        let Some(doc) = self.workspace.get_mut(id) else {
            self.scritture
                .rilascia(lease, crate::scritture::Esito::NonAvviato);
            return self
                .notify_error("Salva con nome non avviato: documento non disponibile".into());
        };
        let snapshot = doc.snapshot_for_save();
        let job = crate::save::PickerJob::new(sequence, directory, filename);
        let delivery = job.delivery();
        self.save_as_sequence = sequence;
        self.save_as_picker = Some(crate::save::Picker {
            sequence,
            document: id,
            lease,
            job,
            source: current,
            root: self.project_root.clone(),
            root_epoch: self.git_root_epoch,
            close_epoch: self.close_epoch,
            ending,
            bom,
            snapshot,
        });
        Task::perform(delivery, Self::save_picker_event)
    }

    pub(super) fn save_as_picked(&mut self, sequence: u64, path: Option<PathBuf>) -> Task<Message> {
        if self.save_as_picker.as_ref().is_none_or(|p| {
            p.sequence != sequence
                || !self
                    .scritture
                    .possiede(p.lease, crate::scritture::Tipo::SalvaConNome)
        }) {
            return Task::none(); // Non consumare il picker successivo.
        }
        let Some(picker) = self.save_as_picker.take() else {
            return Task::none();
        };
        let Some(path) = path else {
            self.scritture
                .rilascia(picker.lease, crate::scritture::Esito::NonAvviato);
            return self.notify(
                NoticeKind::Info,
                "Scelta Salva con nome annullata o finestra non disponibile".into(),
            );
        };
        let valid = self.workspace.get(picker.document).is_some_and(|doc| {
            doc.path() == picker.source.as_deref()
                && doc.matches_snapshot(&picker.snapshot)
                && doc.line_ending() == picker.ending
                && doc.bom() == picker.bom
        }) && self.project_root == picker.root
            && self.git_root_epoch == picker.root_epoch
            && self.close_epoch == picker.close_epoch;
        if !valid {
            self.scritture
                .rilascia(picker.lease, crate::scritture::Esito::NonAvviato);
            return self.notify_error("Scelta Salva con nome scaduta: documento, revisione o progetto cambiati; scegli di nuovo".into());
        }
        self.save_as_con_lease(picker.document, path, picker.lease)
    }

    pub(super) fn save_as_abbandonato(&mut self, sequence: u64) -> Task<Message> {
        if self.save_as_picker.as_ref().is_none_or(|p| {
            p.sequence != sequence
                || !self
                    .scritture
                    .possiede(p.lease, crate::scritture::Tipo::SalvaConNome)
        }) {
            return Task::none();
        }
        if let Some(picker) = self.save_as_picker.take() {
            self.scritture
                .rilascia(picker.lease, crate::scritture::Esito::NonAvviato);
        }
        self.notify_error("Scelta Salva con nome annullata: finestra non avviata, riprova".into())
    }

    pub(super) fn save_picker_event(event: crate::save::PickerEvent) -> Message {
        Message::SavePickerCollected(event)
    }

    pub(super) fn save_picker_collected(
        &mut self,
        event: crate::save::PickerEvent,
    ) -> Task<Message> {
        if self
            .save_as_picker
            .as_ref()
            .is_none_or(|picker| !picker.job.matches(&event))
        {
            return Task::none();
        }
        let (sequence, path, abbandonato) = event.into_parts();
        if abbandonato {
            self.save_as_abbandonato(sequence)
        } else {
            self.save_as_picked(sequence, path)
        }
    }

    pub(super) fn save_event(event: crate::save::Event) -> Message {
        match event {
            crate::save::Event::Ready(ready) => Message::SaveReady(ready),
            crate::save::Event::Done(receipt) => Message::SaveCollected(receipt),
        }
    }

    pub(super) fn save_as_model_current(&self) -> bool {
        self.save_as_flight.as_ref().is_some_and(|flight| {
            self.scritture
                .possiede(flight.lease, crate::scritture::Tipo::SalvaConNome)
                && self.saves.active_document() == Some(flight.ticket.document)
                && self.project_root == flight.root
                && self.git_root_epoch == flight.root_epoch
                && self.close_epoch == flight.close_epoch
                && self.replace_mapping() == flight.mapping
                && self.chiusura.is_none()
                && self.close_after_save.is_none()
                && self.git_operazione.is_none()
                && !self.pending_restored_tabs.contains(flight.ticket.document)
        })
    }

    pub(super) fn save_job_model_current(&self) -> bool {
        let Some(job) = self.save_job.as_ref() else {
            return false;
        };
        let ticket = job.ticket();
        let id = ticket.document;
        self.saves.active_document() == Some(id)
            && (self.save_lease.is_some_and(|lease| {
                self.scritture
                    .possiede(lease, crate::scritture::Tipo::Salvataggio)
            }) || self.save_as_flight.is_some()
                || self.replacement.is_some())
            && self.workspace.get(id).is_some()
            && self.save_job_root_epoch == Some(self.git_root_epoch)
            && self
                .close_after_save
                .is_none_or(|(_, target, should_save)| {
                    should_save || matches!(target, DaChiudere::Scheda(other) if other != id)
                })
            && !self.pending_restored_tabs.contains(id)
            && self.replacement.as_ref().is_none_or(|replacement| {
                self.scritture
                    .possiede(replacement.lease, crate::scritture::Tipo::Sostituisci)
                    && replacement.autosaves.contains_key(&id)
                    && replacement.phase == crate::replace_batch::Phase::Saving
                    && !replacement
                        .cancel
                        .load(std::sync::atomic::Ordering::Acquire)
            })
    }

    pub(super) fn save_ready_current(&self, ready: &crate::save::Ready) -> bool {
        let metadata = ready.metadata();
        let note_current = metadata.note.as_ref().is_none_or(|note| {
            self.notes.as_ref().is_some_and(|notes| {
                notes.scan_generation == note.scope
                    && notes.store.is_some()
                    && notes.tabs.get(&metadata.ticket.document) == Some(&note.id)
                    && (notes.loaded.contains_key(&note.id)
                        || notes.open_saves.contains_key(&note.id))
            })
        });
        if !note_current {
            return false;
        }
        if let Some(flight) = self.save_as_flight.as_ref() {
            return self.save_as_model_current()
                && flight.ticket == metadata.ticket
                && self
                    .workspace
                    .get(metadata.ticket.document)
                    .is_some_and(|doc| {
                        doc.path() == flight.source.as_deref()
                            && doc.can_mark_snapshot_saved(&metadata.snapshot)
                    })
                && [&metadata.path, &metadata.resolved]
                    .into_iter()
                    .all(|path| {
                        self.workspace
                            .get_by_path(path)
                            .is_none_or(|other| other == metadata.ticket.document)
                    });
        }
        self.save_job_model_current()
            && self
                .save_job
                .as_ref()
                .is_some_and(|job| job.ticket() == metadata.ticket)
            && self
                .workspace
                .get(metadata.ticket.document)
                .is_some_and(|doc| {
                    doc.path() == Some(metadata.path.as_path())
                        && doc.can_mark_snapshot_saved(&metadata.snapshot)
                })
    }

    pub(super) fn save_as_ready(&mut self, ready: crate::save::Ready) -> Task<Message> {
        // Prima autenticare l'Arc effettiva, poi decidere. Foreign/replay non
        // revocano né consumano la lease del job corrente.
        if !self
            .save_job
            .as_ref()
            .is_some_and(|job| job.matches_ready(&ready))
        {
            return Task::none();
        }
        if !self.save_ready_current(&ready) {
            ready.deny();
        } else if ready.grant() {
            self.save_as_permission = Some(ready);
        }
        // Nessun ack/rebind: attendere la scrittura e la ricevuta autentica.
        Task::none()
    }

    fn ammetti_salva_con_nome(&mut self, id: DocId) -> Result<crate::scritture::Lease, String> {
        if let Some(in_volo) = self.scritture.guarda(None).in_volo {
            return Err(crate::scritture::Rifiuto {
                in_volo,
                richiesto: crate::scritture::Tipo::SalvaConNome,
                rimandato: false,
            }
            .avviso());
        }
        if self.scritture.guarda(None).in_volo.is_some()
            || self.git_operazione.is_some()
            || self.chiusura.is_some()
        {
            return Err(
                "Un'operazione file o una chiusura è in corso: attendi e ripeti Salva con nome"
                    .into(),
            );
        }
        match self
            .scritture
            .ammetti(crate::scritture::Op::SalvaConNome { doc: id })
        {
            Ok(crate::scritture::Permesso::Avvia(lease)) => Ok(lease),
            Err(rifiuto) => Err(rifiuto.avviso()),
            Ok(_) => Err("Salva con nome non avviato: permesso non valido".into()),
        }
    }

    // L'ingresso diretto resta disponibile per i chiamanti che hanno già un percorso.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn save_as(&mut self, id: DocId, path: PathBuf) -> Task<Message> {
        if !self.puo_salvare_con_nome(id) {
            return self.notify_error("«Salva con nome» vale per i file dell'editor: la console SQL e gli Appunti si salvano da soli".to_owned());
        }
        let lease = match self.ammetti_salva_con_nome(id) {
            Ok(lease) => lease,
            Err(testo) => return self.notify(NoticeKind::Info, testo),
        };
        self.save_as_con_lease(id, path, lease)
    }

    fn save_as_con_lease(
        &mut self,
        id: DocId,
        path: PathBuf,
        lease: crate::scritture::Lease,
    ) -> Task<Message> {
        if !self
            .scritture
            .possiede(lease, crate::scritture::Tipo::SalvaConNome)
        {
            return self.notify_error("Salva con nome non avviato: richiesta scaduta".into());
        }
        if !self.puo_salvare_con_nome(id) {
            self.scritture
                .rilascia(lease, crate::scritture::Esito::NonAvviato);
            return self
                .notify_error("Salva con nome non avviato: documento non disponibile".into());
        }
        if let Some(other) = self.workspace.get_by_path(&path)
            && other != id
        {
            self.scritture
                .rilascia(lease, crate::scritture::Esito::NonAvviato);
            return self.notify_error(format!(
                "{} è già aperto in un'altra scheda: chiudila o scegli un altro nome",
                tools::bookmarks::display_name(&path)
            ));
        }
        if self.saves.active_document().is_some()
            || self.save_job.is_some()
            || self.save_as_flight.is_some()
            || self.git_operazione.is_some()
            || self.chiusura.is_some()
        {
            self.scritture
                .rilascia(lease, crate::scritture::Esito::NonAvviato);
            return self.notify_error(
                "Un'operazione file o una chiusura è in corso: attendi e ripeti Salva con nome"
                    .into(),
            );
        }
        let opened = self.replace_mapping();
        let Some(doc) = self.workspace.get_mut(id) else {
            self.scritture
                .rilascia(lease, crate::scritture::Esito::NonAvviato);
            return self
                .notify_error("Salva con nome non avviato: documento non disponibile".into());
        };
        let source = doc.path().map(Path::to_path_buf);
        let snapshot = doc.snapshot_for_save();
        let ticket = self.saves.start(id);
        self.save_as_flight = Some(crate::save::Rebind {
            ticket,
            lease,
            source,
            root: self.project_root.clone(),
            root_epoch: self.git_root_epoch,
            mapping: opened.clone(),
            close_epoch: self.close_epoch,
        });
        let request = crate::save::Request {
            ticket,
            path,
            snapshot,
            note: None,
            aliases: Some(opened),
            expected_fingerprint: None,
        };
        let job = crate::save::Job::new(request);
        let delivery = job.delivery();
        self.save_job = Some(job);
        Task::run(delivery, Self::save_event)
    }

    pub(super) fn save_all(&mut self) -> Task<Message> {
        if let Some(testo) = self.rifiuto_salvataggio() {
            return self.notify(NoticeKind::Info, testo);
        }
        let (nuovi, con_file): (Vec<_>, Vec<_>) =
            self.groups.all().partition(|&id| self.e_senza_nome(id));
        let saltati = nuovi
            .into_iter()
            .filter(|&id| self.chiede_salvataggio(id))
            .count();
        // Prima dei worker: un eventuale errore di scrittura ha precedenza.
        let avviso = if saltati > 0 {
            let testo = if saltati == 1 {
                "1 file nuovo non è stato salvato: non ha ancora un file".to_owned()
            } else {
                format!("{saltati} file nuovi non sono stati salvati: non hanno ancora un file")
            };
            self.notify(NoticeKind::Info, testo)
        } else {
            Task::none()
        };
        for id in con_file {
            self.queue_save(id);
        }
        avviso
    }

    /// Le regioni ripiegabili del documento attivo, lette dallo snapshot
    /// sintattico. Vuote se il documento non ha blocchi o non è ancora
    /// stato analizzato.
    pub(super) fn regioni_fold(&self, id: DocId) -> Vec<lume_syntax::FoldRegion> {
        for (doc, _, snapshot) in self.syntax_snapshots.values() {
            if *doc == id {
                return snapshot.folding().to_vec();
            }
        }
        Vec::new()
    }

    /// Si può comprimere il blocco `start..=end`? Rifiuta se un estremo
    /// della selezione cade dentro le righe che verrebbero nascoste: il
    /// cursore non può finire su una riga invisibile.
    pub(super) fn puo_collassare(
        &self,
        id: DocId,
        inizio: usize,
        fine: usize,
        selezione: lume_core::Range,
        rope: &ropey::Rope,
    ) -> bool {
        let estremi = [selezione.anchor, selezione.head];
        for estremo in estremi {
            let riga = rope.char_to_line(estremo.min(rope.len_chars()));
            if riga > inizio && riga <= fine {
                return false;
            }
        }
        let _ = id;
        true
    }

    /// Comprime il blocco che contiene il cursore, se c'è e se il cursore
    /// non è dentro di esso.
    pub(super) fn fold_toggle(&mut self) -> Task<Message> {
        let Some(id) = self.active_doc() else {
            return self.notify_error("Nessun documento da ripiegare".to_owned());
        };
        let Some(documento) = self.workspace.get(id) else {
            return Task::none();
        };
        let rope = documento.rope().clone();
        let selezione = documento.selection();
        let cursore = selezione.head;
        let riga_cursore = rope.char_to_line(cursore.min(rope.len_chars()));
        let regioni = self.regioni_fold(id);
        // il blocco più interno che contiene la riga del cursore
        let blocco = regioni
            .iter()
            .rev()
            .find(|r| r.start_line <= riga_cursore && riga_cursore <= r.end_line);
        let Some(blocco) = blocco else {
            return self.notify(
                NoticeKind::Info,
                "Nessun blocco da comprimere qui".to_owned(),
            );
        };
        let collassati = self.folded.entry(id).or_default();
        let contiene = collassati.contains(&blocco.start_line);
        if contiene {
            collassati.remove(&blocco.start_line);
            self.aggiorna_proiezione_fold(id);
            self.notify(NoticeKind::Info, "Blocco espanso".to_owned())
        } else {
            let si_puo =
                self.puo_collassare(id, blocco.start_line, blocco.end_line, selezione, &rope);
            if !si_puo {
                return self.notify_error(
                    "Il cursore è dentro il blocco: spostalo prima di comprimerlo".to_owned(),
                );
            }
            let collassati = self.folded.entry(id).or_default();
            collassati.insert(blocco.start_line);
            self.aggiorna_proiezione_fold(id);
            self.notify(NoticeKind::Info, "Blocco compresso".to_owned())
        }
    }

    /// Clic sulla freccia del margine: comprime o espande il blocco che
    /// inizia alla riga `line` del documento attivo nel gruppo. Stesse regole
    /// di [`Self::fold_toggle`]: non si comprime un blocco con il cursore dentro.
    pub(super) fn click_fold_marker(
        &mut self,
        group: crate::editor_groups::GroupId,
        line: usize,
    ) -> Task<Message> {
        let Some(id) = self.groups.active_in(group) else {
            return Task::none();
        };
        let Some(documento) = self.workspace.get(id) else {
            return Task::none();
        };
        let rope = documento.rope().clone();
        let selezione = documento.selection();
        let regioni = self.regioni_fold(id);
        let Some(blocco) = regioni.iter().find(|r| r.start_line == line) else {
            return self.notify(
                NoticeKind::Info,
                "Nessun blocco da comprimere qui".to_owned(),
            );
        };
        let (inizio, fine) = (blocco.start_line, blocco.end_line);
        if self.folded.get(&id).is_some_and(|c| c.contains(&inizio)) {
            if let Some(collassati) = self.folded.get_mut(&id) {
                collassati.remove(&inizio);
            }
            self.aggiorna_proiezione_fold(id);
            return self.notify(NoticeKind::Info, "Blocco espanso".to_owned());
        }
        if !self.puo_collassare(id, inizio, fine, selezione, &rope) {
            return self.notify_error(
                "Il cursore è dentro il blocco: spostalo prima di comprimerlo".to_owned(),
            );
        }
        self.folded.entry(id).or_default().insert(inizio);
        self.aggiorna_proiezione_fold(id);
        self.notify(NoticeKind::Info, "Blocco compresso".to_owned())
    }

    pub(super) fn unfold_all(&mut self) -> Task<Message> {
        let Some(id) = self.active_doc() else {
            return Task::none();
        };
        self.dimentica_folding(id);
        self.notify(NoticeKind::Info, "Tutti i blocchi espansi".to_owned())
    }

    /// I blocchi compressi del documento, come li vede il widget.
    pub(super) fn blocchi_compressi(&self, id: DocId) -> Vec<lume_ui::editor::FoldSpan> {
        self.fold_projections
            .get(&id)
            .map_or_else(Vec::new, |p| p.spans().to_vec())
    }

    /// Ricalcola la proiezione che il widget usa per disegnare: senza, lo
    /// stato `folded` cambia ma l'editor mostra ancora tutte le righe.
    /// Va chiamata dopo ogni modifica dei blocchi collassati del documento.
    pub(super) fn aggiorna_proiezione_fold(&mut self, id: DocId) {
        let regioni = self.regioni_fold(id);
        let proiezione = match self.folded.get(&id) {
            Some(collassati) if !collassati.is_empty() => {
                lume_ui::editor::FoldProjection::from_regions(&regioni, |riga| {
                    collassati.contains(&riga)
                })
            }
            _ => lume_ui::editor::FoldProjection::default(),
        };
        if proiezione.spans().is_empty() {
            self.fold_projections.remove(&id);
        } else {
            self.fold_projections.insert(id, proiezione);
        }
        if let Some(documento) = self.workspace.get(id) {
            self.fold_versione.insert(id, documento.version());
        }
    }

    /// Toglie ogni stato di folding del documento.
    pub(super) fn dimentica_folding(&mut self, id: DocId) {
        self.folded.remove(&id);
        self.fold_projections.remove(&id);
        self.fold_versione.remove(&id);
    }

    /// Rimette in coerenza il folding dopo ogni messaggio. Un solo punto
    /// copre tutti i percorsi (modifiche, undo, Trova, vai alla riga,
    /// multicursore, definizione LSP) invece di correggerli uno per uno:
    /// - documento chiuso: lo stato si libera;
    /// - testo cambiato: le righe dei blocchi non valgono più, si espande tutto;
    /// - una testa di selezione in una riga nascosta: si espande quel blocco;
    /// - proiezione assente con blocchi compressi (snapshot arrivato dopo):
    ///   si ricalcola.
    ///
    /// Ogni espansione automatica lo dice in status bar.
    pub(super) fn coerenza_folding(&mut self) -> Task<Message> {
        if self.folded.is_empty() {
            return Task::none();
        }
        let ids: Vec<DocId> = self.folded.keys().copied().collect();
        let mut testo_cambiato = false;
        let mut cursore_mostrato = false;
        for id in ids {
            let Some(documento) = self.workspace.get(id) else {
                self.dimentica_folding(id);
                continue;
            };
            let versione = documento.version();
            if self.fold_versione.get(&id).is_some_and(|v| *v != versione) {
                self.dimentica_folding(id);
                testo_cambiato = true;
                continue;
            }
            let rope = documento.rope();
            let teste: Vec<usize> = documento
                .selections()
                .ranges()
                .iter()
                .map(|r| rope.char_to_line(r.head.min(rope.len_chars())))
                .collect();
            // i blocchi annidati: tolto il contenitore, un figlio compresso
            // può nascondere ancora la stessa testa; il ciclo è limitato.
            for _ in 0..16 {
                let nascondono: Vec<usize> = self
                    .blocchi_compressi(id)
                    .iter()
                    .filter(|b| teste.iter().any(|t| b.start_line < *t && *t <= b.end_line))
                    .map(|b| b.start_line)
                    .collect();
                if nascondono.is_empty() {
                    break;
                }
                if let Some(collassati) = self.folded.get_mut(&id) {
                    for riga in &nascondono {
                        collassati.remove(riga);
                    }
                }
                self.aggiorna_proiezione_fold(id);
                cursore_mostrato = true;
            }
            let compressi = self.folded.get(&id).is_some_and(|c| !c.is_empty());
            if compressi && !self.fold_projections.contains_key(&id) {
                self.aggiorna_proiezione_fold(id);
            }
            if !compressi {
                self.dimentica_folding(id);
            }
        }
        let mut avvisi = Vec::new();
        if testo_cambiato {
            avvisi.push(self.notify(
                NoticeKind::Info,
                "Blocchi espansi: il testo è cambiato".to_owned(),
            ));
        }
        if cursore_mostrato {
            avvisi.push(self.notify(
                NoticeKind::Info,
                "Blocco espanso per mostrare il cursore".to_owned(),
            ));
        }
        Task::batch(avvisi)
    }
}

/// Se `head` cade in una riga nascosta da un blocco compresso, lo riporta su
/// una riga visibile: il caret non deve mai finire dove l'utente non vede, o
/// il testo battuto finirebbe in righe invisibili.
///
/// In avanti va alla prima riga dopo il blocco, indietro alla riga
/// d'intestazione. Con `colonna` (Su/Giù) conserva la colonna, limitata alla
/// lunghezza della riga; senza, va a inizio riga in avanti e a fine riga
/// indietro. Se dopo il blocco non c'è altra riga, resta sull'intestazione.
fn fuori_dalle_righe_nascoste(
    rope: &ropey::Rope,
    nascoste: &[lume_ui::editor::FoldSpan],
    head: usize,
    avanti: bool,
    colonna: Option<usize>,
) -> usize {
    let riga = rope.char_to_line(head.min(rope.len_chars()));
    let Some(blocco) = nascoste
        .iter()
        .find(|b| b.start_line < riga && riga <= b.end_line)
    else {
        return head;
    };
    let in_riga = |riga: usize, colonna: usize| {
        let linea = rope.line(riga);
        let mut lunghezza = linea.len_chars();
        if lunghezza > 0 && linea.char(lunghezza - 1) == '\n' {
            lunghezza -= 1;
        }
        rope.line_to_char(riga) + colonna.min(lunghezza)
    };
    let dopo = blocco.end_line.saturating_add(1);
    if avanti && dopo < rope.len_lines() {
        in_riga(dopo, colonna.unwrap_or(0))
    } else {
        in_riga(blocco.start_line, colonna.unwrap_or(usize::MAX))
    }
}

#[cfg(test)]
mod righe_nascoste_tests {
    use super::fuori_dalle_righe_nascoste;
    use lume_ui::editor::FoldSpan;

    const TESTO: &str = "fn main() {\n    let a = 1;\n    let b = 2;\n}\nfn altro() {}\n";

    fn blocco() -> Vec<FoldSpan> {
        vec![FoldSpan {
            start_line: 0,
            end_line: 3,
            hidden_count: 3,
            hidden_before: 0,
        }]
    }

    #[test]
    fn giu_dentro_il_blocco_porta_alla_riga_dopo_con_la_stessa_colonna() {
        let rope = ropey::Rope::from_str(TESTO);
        let dentro = rope.line_to_char(1) + 2;
        let esito = fuori_dalle_righe_nascoste(&rope, &blocco(), dentro, true, Some(2));
        assert_eq!(esito, rope.line_to_char(4) + 2);
    }

    #[test]
    fn su_dentro_il_blocco_torna_all_intestazione() {
        let rope = ropey::Rope::from_str(TESTO);
        let dentro = rope.line_to_char(2) + 4;
        let esito = fuori_dalle_righe_nascoste(&rope, &blocco(), dentro, false, Some(4));
        assert_eq!(esito, 4);
    }

    #[test]
    fn sinistra_dentro_il_blocco_va_a_fine_intestazione() {
        let rope = ropey::Rope::from_str(TESTO);
        let dentro = rope.line_to_char(3);
        let esito = fuori_dalle_righe_nascoste(&rope, &blocco(), dentro, false, None);
        assert_eq!(esito, "fn main() {".chars().count());
    }

    #[test]
    fn una_riga_visibile_non_si_tocca() {
        let rope = ropey::Rope::from_str(TESTO);
        let visibile = rope.line_to_char(4) + 1;
        let esito = fuori_dalle_righe_nascoste(&rope, &blocco(), visibile, true, Some(1));
        assert_eq!(esito, visibile);
    }

    #[test]
    fn blocco_in_fondo_resta_sull_intestazione() {
        let rope = ropey::Rope::from_str("fn main() {\n    let a = 1;\n}");
        let span = [FoldSpan {
            start_line: 0,
            end_line: 2,
            hidden_count: 2,
            hidden_before: 0,
        }];
        let dentro = rope.line_to_char(1);
        let esito = fuori_dalle_righe_nascoste(&rope, &span, dentro, true, Some(0));
        assert_eq!(esito, 0);
    }
}
