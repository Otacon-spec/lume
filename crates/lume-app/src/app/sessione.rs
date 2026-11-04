//! Sessione: salvataggio e ripristino, progetto e file system, file recenti, uscita.
//!
//! Metodi di `Lume`: lo stato e i messaggi restano in `app.rs`.

use super::*;

impl Lume {
    /// Lancia la rilevazione WSL se non è già partita (una volta per sessione).
    pub(super) fn rileva_wsl_se_serve(&mut self) -> Task<Message> {
        if self.wsl.avvia() {
            Self::detect_environments()
        } else {
            Task::none()
        }
    }

    pub(super) fn session_snapshot(&self) -> SessionSnapshot {
        use crate::editor_groups::GroupId;
        let tabs = self.group_paths(GroupId::Primary);
        let active_tab = self.doc_path(self.groups.active_in(GroupId::Primary));
        // `tabs`/`active_tab` restano il gruppo 0: un binario vecchio riapre
        // comunque la sessione, mostrando quel che sa leggere.
        let secondary_group = self.groups.is_split().then(|| lume_store::TabGroupRecord {
            tabs: self.group_paths(GroupId::Secondary),
            active: self.doc_path(self.groups.active_in(GroupId::Secondary)),
        });
        SessionSnapshot {
            workspace_root: self.project_root.clone(),
            tabs,
            active_tab,
            appearance: AppearancePreferences::new(
                self.theme_state.current().id.clone(),
                self.icon_themes.preferred_id(),
            ),
            panel_width: self.layout.panel_width,
            left_tools: self
                .registry
                .left
                .iter()
                .map(|id| id.0.to_string())
                .collect(),
            right_tools: self
                .registry
                .right
                .iter()
                .map(|id| id.0.to_string())
                .collect(),
            active_panel: self.registry.active_panel.map(|id| id.0.to_string()),
            active_dock: self.registry.active_dock.map(|id| id.0.to_string()),
            dock_height: self.registry.dock_height,
            environment_id: self.environment_picker.selected_id().to_string(),
            bookmarks: self.bookmarks.clone(),
            history: self
                .history
                .iter()
                .take(tools::history::PERSISTED_ENTRIES)
                .cloned()
                .collect(),
            // Solo le funzionalità **cablate**: le altre sono spente per
            // costruzione, e salvarle le congelerebbe spente anche il giorno
            // in cui il backend arriva.
            secondary_group,
            focus_secondary: self.groups.focus() == GroupId::Secondary,
            split_ratio: self.layout.split_ratio,
            disabled_features: self
                .registry
                .disabled_features()
                .iter()
                .filter(|key| crate::features::by_key(key).is_some_and(|spec| spec.wired))
                .map(|key| (*key).to_string())
                .collect(),
            #[cfg(feature = "sql")]
            sql_console: self.sql_console_snapshot(),
            #[cfg(not(feature = "sql"))]
            sql_console: self.preserved_sql_console.clone(),
        }
    }

    /// I path delle schede di un gruppo, saltando i documenti senza file
    /// (la console SQL): la sessione parla di file.
    pub(super) fn group_paths(&self, group: crate::editor_groups::GroupId) -> Vec<PathBuf> {
        self.groups
            .tabs(group)
            .iter()
            .filter_map(|&id| self.doc_path(Some(id)))
            .collect()
    }

    pub(super) fn doc_path(&self, id: Option<DocId>) -> Option<PathBuf> {
        id.and_then(|id| self.workspace.get(id))
            .and_then(|document| document.path().map(Path::to_path_buf))
    }

    pub(super) fn persist_session(&self) -> Task<Message> {
        let Some(store) = self.store.clone() else {
            return Task::none();
        };
        let snapshot = self.session_snapshot();
        Task::perform(
            async move {
                store
                    .save_session(snapshot)
                    .await
                    .map_err(|error| error.to_string())
            },
            Message::SessionSaved,
        )
    }

    #[cfg(test)]
    pub(super) fn restore_snapshot(&mut self, snapshot: SessionSnapshot) {
        let active = self.restore_snapshot_state(snapshot);
        for id in active {
            if let Err(error) = self.materialize_restored_tab(id) {
                let (path, detail) = self
                    .pending_restored_tabs
                    .get(id)
                    .map(|tab| {
                        (
                            tab.path.display().to_string(),
                            tab.failure.clone().unwrap_or_else(|| error.to_string()),
                        )
                    })
                    .unwrap_or_else(|| ("file ripristinato".to_owned(), error.to_string()));
                self.notify_persistente(
                    NoticeKind::Error,
                    format!("Impossibile riaprire {path}: {detail}"),
                );
            }
        }
    }

    pub(super) fn restore_snapshot_async(&mut self, snapshot: SessionSnapshot) -> Task<Message> {
        let active = self.restore_snapshot_state(snapshot);
        Task::batch(
            active
                .into_iter()
                .map(|id| self.restored_tab_read_task(id, false)),
        )
    }

    pub(super) fn restore_snapshot_state(&mut self, snapshot: SessionSnapshot) -> Vec<DocId> {
        #[cfg(not(feature = "sql"))]
        {
            self.preserved_sql_console = snapshot.sql_console;
        }
        self.restored_tab_reads.clear();
        self.restored_tab_delivery.clear();
        self.restored_tab_commands.clear();
        self.restored_tab_actions.clear();
        if let Some(theme_index) = self
            .theme_state
            .list
            .iter()
            .position(|theme| theme.id == snapshot.appearance.color_theme_id)
        {
            self.theme_state.sel = theme_index;
            self.theme_state.iced = self.theme_state.current().iced_theme();
            self.settings_panel.set_selected_color_theme(theme_index);
        }
        self.icon_themes
            .apply_preference(snapshot.appearance.icon_theme_id.clone());
        self.settings_panel
            .set_selected_icon_theme(snapshot.appearance.icon_theme_id);

        self.layout.panel_width = snapshot.panel_width.clamp(PANEL_MIN, PANEL_MAX);

        if let Some(root) = snapshot.workspace_root.clone().filter(|root| root.is_dir()) {
            self.set_project_root(root);
        }
        // Si registrano placeholder path-backed senza leggere i file, poi si
        // **ricostruisce** la divisione in un colpo solo. Solo le schede che
        // saranno visibili nel primo frame vengono materializzate.
        let mut secondary = Vec::new();
        for path in snapshot
            .tabs
            .iter()
            .chain(snapshot.secondary_group.iter().flat_map(|g| g.tabs.iter()))
        {
            if self.workspace.get_by_path(path).is_none() {
                let id = self.workspace.open(Some(path.clone()), "");
                self.pending_restored_tabs.insert(id, path.clone());
            }
        }
        let doc_of = |app: &Self, path: &Path| app.workspace.get_by_path(path);
        let primary: Vec<DocId> = snapshot
            .tabs
            .iter()
            .filter_map(|path| doc_of(self, path))
            .collect();
        if let Some(group) = snapshot.secondary_group.as_ref() {
            secondary = group
                .tabs
                .iter()
                .filter_map(|path| doc_of(self, path))
                .collect();
        }
        let primary_active = snapshot
            .active_tab
            .as_deref()
            .and_then(|path| doc_of(self, path));
        let secondary_active = snapshot
            .secondary_group
            .as_ref()
            .and_then(|group| group.active.as_deref())
            .and_then(|path| doc_of(self, path));
        let secondary = (!secondary.is_empty()).then_some((secondary, secondary_active));
        self.groups
            .restore(primary, primary_active, secondary, snapshot.focus_secondary);
        self.layout.split_ratio = snapshot
            .split_ratio
            .clamp(crate::layout::SPLIT_MIN, crate::layout::SPLIT_MAX);

        let active: Vec<DocId> = [
            self.groups
                .active_in(crate::editor_groups::GroupId::Primary),
            self.groups
                .active_in(crate::editor_groups::GroupId::Secondary),
        ]
        .into_iter()
        .flatten()
        .collect();

        // **Prima** delle barre: `is_available` deve già dire la verità
        // quando si sceglie quale pannello riaprire, o si ripristinerebbe il
        // pannello di una funzionalità spenta.
        for key in &snapshot.disabled_features {
            if let Some(spec) = crate::features::by_key(key).filter(|spec| spec.wired) {
                // il ripristino non è una transizione: le risorse non sono
                // ancora nate, quindi le richieste di chiusura sono vuote per
                // costruzione e non c'è niente da drenare
                let _ = self.registry.set_feature_enabled(spec.key, false);
            }
        }

        if !snapshot.left_tools.is_empty() || !snapshot.right_tools.is_empty() {
            let mut left: Vec<ToolId> = snapshot
                .left_tools
                .iter()
                .filter_map(|value| tools::persisted_id(value))
                .collect();
            let mut right: Vec<ToolId> = snapshot
                .right_tools
                .iter()
                .filter_map(|value| tools::persisted_id(value))
                .filter(|id| !left.contains(id))
                .collect();
            // Le barre registrate sono la fonte delle icone predefinite:
            // una sessione vecchia non può far sparire tool aggiunti dopo.
            for (id, side) in self
                .registry
                .left
                .iter()
                .copied()
                .map(|id| (id, Side::Left))
                .chain(
                    self.registry
                        .right
                        .iter()
                        .copied()
                        .map(|id| (id, Side::Right)),
                )
            {
                if !left.contains(&id) && !right.contains(&id) {
                    match side {
                        Side::Left => left.push(id),
                        Side::Right => right.push(id),
                    }
                }
            }
            left.dedup();
            right.dedup();
            self.registry.left = left;
            self.registry.right = right;
        }

        // stato ricostruito, non una transizione: la notifica di apertura la
        // fa una volta sola `notify_initial_visibility`
        self.registry.active_panel = snapshot
            .active_panel
            .as_deref()
            .and_then(tools::persisted_id)
            .filter(|&id| self.registry.is_available(id));
        self.registry.active_dock = snapshot
            .active_dock
            .as_deref()
            .and_then(tools::persisted_id)
            .filter(|&id| self.registry.kind_of(id) == Some(ToolKind::Dock))
            .filter(|&id| self.registry.is_available(id));
        self.registry.set_dock_height(snapshot.dock_height);
        self.environment_picker.select_id(&snapshot.environment_id);
        self.sync_terminal_shell();
        // dopo la riapertura delle schede: le voci «Apertura» generate dal
        // ripristino vengono sostituite dalla cronologia salvata
        self.bookmarks = snapshot.bookmarks;
        self.history = snapshot.history;
        self.sync_bookmarks_tool();
        self.sync_history_tool();
        #[cfg(feature = "sql")]
        if let Some(console) = snapshot.sql_console {
            self.sql = Some(crate::sql_snapshot::restore_workspace(&console));
            let id = self.workspace.open(None, &console.editor_text);
            self.groups.open(crate::editor_groups::GroupId::Primary, id);
            self.sql_console = Some(id);
            self.syntax.insert(id, SyntaxLanguage(LanguageId::Sql));
        }
        active
    }

    /// Apre (o riattiva) un file nel workspace; registra il linguaggio se la
    /// lingua è riconosciuta. `Err` se il file non è leggibile: il chiamante
    /// decide se notificare (l'utente deve vedere il fallimento).
    #[cfg(test)]
    pub fn open_file(&mut self, path: &Path) -> Result<(), std::io::Error> {
        self.check_file_binding(path)?;
        let content = crate::file_read::read_testo_capped(path, crate::file_read::MAX_FILE_BYTES)?;
        let fingerprint = content.fingerprint();
        self.apri_con_testo(path, &content);
        if let Some(id) = self.workspace.get_by_path(path) {
            self.disk_fingerprints.insert(id, fingerprint);
            self.external_conflicts.remove(&id);
        }
        self.refresh_editor_blame_availability();
        Ok(())
    }

    /// Una nuova scheda/alias non può adottare una lettura superata da una
    /// scrittura in volo. Nessun canonicalize in UI: barriera conservativa
    /// per nuovi binding; i buffer già aperti si possono ancora riattivare.
    pub(super) fn check_file_binding(&self, path: &Path) -> Result<(), std::io::Error> {
        if (self.scritture.guarda(None).in_volo.is_some() || self.git_operazione.is_some())
            && self.workspace.get_by_path(path).is_none()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Ci sono scritture in corso: attendi e ripeti l'apertura del file",
            ));
        }
        Ok(())
    }

    /// Il resto dell'apertura, dato il testo già letto: un solo posto per il
    /// percorso sincrono (`open_file`) e per quello che ha letto altrove.
    #[cfg(test)]
    pub(super) fn apri_con_testo(&mut self, path: &Path, content: &str) {
        let id = self.workspace.open(Some(path.to_path_buf()), content);
        if self.pending_restored_tabs.take(id).is_some()
            && let Some(document) = self.workspace.get_mut(id)
        {
            self.restored_tab_reads.remove(&id);
            self.restored_tab_delivery.remove(&id);
            self.restored_tab_actions.remove(&id);
            document.reload_from_str(content);
        }
        self.attiva_file_aperto(id, path);
    }

    pub(super) fn apri_preparato(
        &mut self,
        path: &Path,
        content: &crate::file_read::Testo,
    ) -> Result<(), String> {
        let adopts_content = self
            .workspace
            .get_by_path(path)
            .is_none_or(|id| self.pending_restored_tabs.contains(id));
        let id = if let Some(id) = self.workspace.get_by_path(path) {
            if self.pending_restored_tabs.contains(id) {
                let document = self
                    .workspace
                    .get_mut(id)
                    .ok_or("documento non più disponibile")?;
                content
                    .apply_reload(document)
                    .ok_or("lettura preparata obsoleta o già consumata")?;
                self.pending_restored_tabs.take(id);
                self.restored_tab_reads.remove(&id);
                self.restored_tab_delivery.remove(&id);
                self.restored_tab_actions.remove(&id);
            }
            id
        } else {
            let prepared = content
                .take_open()
                .ok_or("lettura non preparata o già consumata")?;
            self.workspace
                .open_prepared(Some(path.to_path_buf()), prepared)
        };
        // L'impronta identifica i byte adottati dal documento. Una lettura
        // duplicata attiva soltanto il buffer esistente e non sana conflitti.
        if adopts_content {
            self.disk_fingerprints.insert(id, content.fingerprint());
            self.external_conflicts.remove(&id);
        }
        self.attiva_file_aperto(id, path);
        Ok(())
    }

    fn attiva_file_aperto(&mut self, id: DocId, path: &Path) {
        if !self.groups.contains(id) {
            self.groups.open(self.groups.focus(), id);
        }
        if let std::collections::hash_map::Entry::Vacant(e) = self.syntax.entry(id)
            && let Some(lang) = lume_syntax::detect(path)
            && self.workspace.get(id).is_some()
        {
            e.insert(SyntaxLanguage(lang));
        }
        self.activate_editor_tab(id);
        self.close_diff_if_primary();
        self.lsp_did_open(id);
        self.refresh_editor_blame_availability();
        self.queue_editor_blame(id);
        self.aggiorna_marcatori_di(id);
        self.push_history(
            lume_store::HistoryAction::Opened,
            tools::bookmarks::display_name(path),
            Some(path.to_path_buf()),
        );
    }

    /// Apre un file **senza fermare l'interfaccia**: la lettura va su un
    /// thread di lavoro e il resto succede in `Message::FileLetto`.
    pub(crate) fn open_file_task(
        &mut self,
        path: PathBuf,
        dopo: crate::file_read::DopoApertura,
    ) -> Task<Message> {
        if let Err(error) = self.check_file_binding(&path) {
            return self.notify_error(error.to_string());
        }
        let epoch = self.scritture.guarda(None).epoca;
        let document_version = self.workspace.get_by_path(&path).and_then(|id| {
            self.workspace
                .get(id)
                .map(|document| (id, document.version()))
        });
        let worker_path = path.clone();
        let snapshot = self
            .workspace
            .get_by_path(&path)
            .filter(|id| self.pending_restored_tabs.contains(*id))
            .and_then(|id| self.workspace.get(id))
            .map(lume_core::Document::read_snapshot);
        let reading = self.readers.read(move || {
            crate::file_read::leggi_per_apertura_con_snapshot(worker_path, dopo, snapshot)
        });
        Task::perform(
            async move {
                reading
                    .await
                    .unwrap_or_else(|errore| crate::file_read::LetturaFile {
                        path,
                        dopo,
                        esito: Err(errore.to_string()),
                    })
            },
            move |reading| Message::FileReadFinished(epoch, document_version, reading),
        )
    }

    pub(super) fn file_read_superseded(
        &self,
        path: &Path,
        document_version: Option<(DocId, u64)>,
    ) -> bool {
        let Some((id, version)) = document_version else {
            // Se nel frattempo è comparso un placeholder lazy, deve vincere il
            // reader dedicato che porta anche la versione del documento.
            return self
                .workspace
                .get_by_path(path)
                .is_some_and(|id| self.pending_restored_tabs.contains(id));
        };
        self.workspace.get_by_path(path) != Some(id)
            || !self.workspace.get(id).is_some_and(|document| {
                document.path() == Some(path) && document.version() == version
            })
    }

    pub(super) fn file_letto(&mut self, lettura: crate::file_read::LetturaFile) -> Task<Message> {
        use crate::file_read::DopoApertura;
        let crate::file_read::LetturaFile { path, dopo, esito } = lettura;
        if matches!(
            &dopo,
            DopoApertura::Avvio {
                fuori_progetto: true
            }
        ) && self.albero_eliminazione_in_corso()
        {
            return self.notify_error(
                "Spostamento nel Cestino in corso: attendi prima di cambiare progetto".into(),
            );
        }
        if matches!(
            &dopo,
            DopoApertura::Avvio {
                fuori_progetto: true
            }
        ) && (self.save_as_picker.is_some() || self.save_as_flight.is_some())
        {
            return self.notify_error(
                "Salva con nome è in corso: attendi prima di cambiare progetto".into(),
            );
        }
        // L'Answer LSP può essere già consumato ma il reader arrivare dopo
        // un cambio progetto. Non aprire/attivare il target nel nuovo scope.
        if let DopoApertura::PosizioneLsp { progetto, .. } = dopo
            && progetto != self.git_root_epoch
        {
            return self.notify(
                NoticeKind::Info,
                "Lettura LSP superata da un cambio progetto: ripeti il salto".into(),
            );
        }
        if let DopoApertura::PosizioneLsp {
            origine: (id, versione),
            ..
        } = dopo
            && self.workspace.get(id).map(lume_core::Document::version) != Some(versione)
        {
            return self.notify(
                NoticeKind::Info,
                "Lettura LSP superata da una modifica o chiusura del documento sorgente: ripeti il salto".into(),
            );
        }
        if let DopoApertura::PosizioneLsp { navigazione, .. } = dopo
            && self.definition_epoch != Some(navigazione)
        {
            return self.notify(
                NoticeKind::Info,
                "Lettura LSP superata da una nuova richiesta o cambio di scheda: ripeti il salto"
                    .into(),
            );
        }
        // La lettura potrebbe essere partita prima della scrittura: rivalida
        // anche alla consegna, non soltanto quando viene avviato il Task.
        if let Err(error) = self.check_file_binding(&path) {
            return self.notify_error(error.to_string());
        }
        let testo = match esito {
            Ok(testo) => testo,
            Err(motivo) => {
                if matches!(dopo, DopoApertura::Navigazione { .. }) {
                    self.navigazione.forget(&path);
                }
                return self
                    .notify_error(format!("Apertura di {} fallita: {motivo}", path.display()));
            }
        };
        if let Err(error) = self.apri_preparato(&path, &testo) {
            return self.notify_error(error);
        }
        match dopo {
            DopoApertura::Niente => Task::none(),
            DopoApertura::Riga(riga) => {
                self.goto_line(riga);
                Task::none()
            }
            DopoApertura::IntervalloRighe { da, a } => {
                if let Some(id) = self.workspace.get_by_path(&path)
                    && let Some(document) = self.workspace.get_mut(id)
                {
                    let lines = crate::note_links::righe_del_testo(document.rope());
                    let (start, end) =
                        crate::note_links::caratteri_delle_righe(document.rope(), da, a);
                    document.set_selection(Range::new(start, end));
                    if a > lines {
                        return self.notify(
                            NoticeKind::Info,
                            format!(
                                "Il file ha ora {lines} righe: il collegamento arrivava più in là"
                            ),
                        );
                    }
                }
                Task::none()
            }
            DopoApertura::Navigazione { riga } => {
                self.goto_line_index(riga);
                Task::none()
            }
            DopoApertura::PosizioneLsp {
                riga, carattere, ..
            } => {
                if let Some(id) = self.active_doc() {
                    self.goto_lsp_position_for(id, lsp_types::Position::new(riga, carattere));
                }
                Task::none()
            }
            DopoApertura::Ricerca { riga } => {
                if let Some(riga) = riga {
                    self.goto_line(riga);
                }
                self.persist_session()
            }
            DopoApertura::Avvio { fuori_progetto } => {
                if fuori_progetto
                    && self.save_as_flight.is_none()
                    && let Some(parent) = path.parent()
                {
                    self.set_project_root(parent.to_path_buf());
                    // Il `git_task` di `new()` ha visto la radice di prima e
                    // `set_project_root` ha invalidato le letture in volo: il
                    // refresh sulla radice nuova va rilanciato qui, come in
                    // `FolderPicked`.
                    return Task::batch([
                        self.git_dopo_cambio_radice(),
                        self.persist_session(),
                        self.drain_tool_requests(),
                    ]);
                }
                self.persist_session()
            }
        }
    }

    /// Applica il file ricevuto dalla shell del sistema operativo.
    ///
    /// Il documento esplicito ha precedenza sulla sessione ripristinata. Se
    /// non appartiene al progetto corrente, la sua cartella diventa il nuovo
    /// progetto: albero, Git e terminale restano cosi' coerenti col file che
    /// l'utente ha scelto in Esplora file.
    pub(super) fn open_startup_file(&mut self, path: &Path) -> Task<Message> {
        let path = match lume_core::canonical_path(path) {
            Ok(path) => path,
            Err(error) => {
                return self
                    .notify_error(format!("Apertura di {} fallita: {error}", path.display()));
            }
        };
        let readable_file = path.metadata().is_ok_and(|metadata| metadata.is_file())
            && std::fs::File::open(&path).is_ok();
        if !readable_file {
            return self.notify_error(format!(
                "Apertura di {} fallita: non e' un file regolare leggibile",
                path.display()
            ));
        }
        let outside_project = self
            .project_root
            .as_deref()
            .is_none_or(|root| !lume_core::path_is_within(root, &path));
        self.open_file_task(
            path,
            crate::file_read::DopoApertura::Avvio {
                fuori_progetto: outside_project,
            },
        )
    }

    pub(super) fn open_navigation_target(
        &mut self,
        posizione: crate::navigazione::Posizione,
    ) -> Task<Message> {
        if let Some(id) = self.workspace.get_by_path(&posizione.path) {
            if !self.groups.contains(id) {
                self.groups.open(self.groups.focus(), id);
            }
            self.activate_editor_tab(id);
            if self.pending_restored_tabs.contains(id) {
                return self.defer_restored_action(
                    id,
                    DeferredRestoredAction::GotoLineIndex(posizione.riga),
                );
            }
            self.goto_line_index_for(id, posizione.riga);
            return Task::none();
        }

        self.open_file_task(
            posizione.path,
            crate::file_read::DopoApertura::Navigazione {
                riga: posizione.riga,
            },
        )
    }

    pub(super) fn open_recents(&mut self) -> Task<Message> {
        self.recents_selected = 0;
        self.menu_bar.close();
        self.apri_overlay(OverlayId::Recents);
        Task::none()
    }

    /// Le righe dei recenti, **senza quella dove siamo già**: «vai al file
    /// recente» che offre come prima scelta il file aperto sarebbe un giro a
    /// vuoto, e Ctrl+E serve proprio a tornare all'altro.
    pub(crate) fn recents_rows(&self) -> Vec<crate::recents::Row> {
        let qui = self
            .groups
            .active()
            .and_then(|id| self.workspace.get(id))
            .and_then(lume_core::Document::path);
        self.navigazione
            .recenti()
            .into_iter()
            .filter(|posizione| Some(posizione.path.as_path()) != qui)
            .map(|posizione| crate::recents::row_for(&posizione.path, posizione.riga))
            .collect()
    }

    pub(super) fn apply_recents_message(
        &mut self,
        message: crate::recents::RecentsMessage,
    ) -> Task<Message> {
        use crate::recents::RecentsMessage;
        match message {
            RecentsMessage::Open(indice) => {
                let qui = self
                    .groups
                    .active()
                    .and_then(|id| self.workspace.get(id))
                    .and_then(lume_core::Document::path)
                    .map(std::path::Path::to_path_buf);
                let meta = self
                    .navigazione
                    .recenti()
                    .into_iter()
                    .filter(|posizione| Some(posizione.path.as_path()) != qui.as_deref())
                    .nth(indice)
                    .cloned();
                self.close_recents();
                let Some(meta) = meta else {
                    return Task::none();
                };
                self.open_navigation_target(meta)
            }
        }
    }

    /// Quali tasti della ricerca globale appartengono all'elenco dei
    /// recenti mentre è aperto. È un seam: `Subscription` è opaca, e la
    /// mappatura dei tasti si asserisce qui.
    pub(crate) fn recents_key(message: &GlobalSearchMessage) -> Option<crate::recents::RecentsKey> {
        use crate::recents::RecentsKey;
        match message {
            GlobalSearchMessage::Navigate(delta) => Some(RecentsKey::Move(*delta)),
            GlobalSearchMessage::Accept => Some(RecentsKey::Accept),
            _ => None,
        }
    }

    pub(super) fn apply_recents_key(&mut self, key: crate::recents::RecentsKey) -> Task<Message> {
        use crate::recents::RecentsKey;
        match key {
            RecentsKey::Move(delta) => {
                self.recents_selected = crate::recents::move_selection(
                    self.recents_selected,
                    delta,
                    self.recents_rows().len(),
                );
                Task::none()
            }
            RecentsKey::Accept => {
                let scelta = self.recents_selected;
                self.apply_recents_message(crate::recents::RecentsMessage::Open(scelta))
            }
        }
    }

    pub(super) fn close_recents(&mut self) {
        self.chiudi_overlay_se(OverlayId::Recents);
        self.recents_selected = 0;
    }

    /// Chiudere questo documento butterebbe via qualcosa che non è su disco?
    ///
    /// Ogni documento sporco **tranne la console SQL**: la sua query
    /// sopravvive alla scheda e viaggia nella sessione. I file nuovi chiedono
    /// comunque: non avere ancora un path non autorizza a perderne il testo.
    /// Gli Appunti sono file veri; il diff non è un documento.
    pub(super) fn chiede_salvataggio(&self, id: DocId) -> bool {
        !self.is_sql_console(id)
            && self
                .workspace
                .get(id)
                .is_some_and(lume_core::Document::is_dirty)
            || self.pending_editor_input(id)
    }

    /// I documenti che la chiusura della finestra butterebbe via, nell'ordine
    /// delle schede; in coda quelli senza scheda, se ce ne fossero.
    pub(super) fn documenti_sporchi(&self) -> Vec<DocId> {
        let mut sporchi: Vec<DocId> = self
            .groups
            .all()
            .filter(|&id| self.chiede_salvataggio(id))
            .collect();
        let mut senza_scheda: Vec<DocId> = self
            .workspace
            .ids()
            .filter(|id| !sporchi.contains(id) && self.chiede_salvataggio(*id))
            .collect();
        senza_scheda.sort_by_key(|id| id.0);
        sporchi.extend(senza_scheda);
        sporchi
    }

    /// Il nome con cui l'utente conosce un documento: lo stesso della scheda.
    pub(crate) fn nome_documento(&self, id: DocId) -> String {
        self.editor_tab_name(id)
            .or_else(|| {
                self.workspace
                    .get(id)
                    .and_then(lume_core::Document::path)
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
            })
            // la console SQL è un documento senza path: il nome non può
            // venire dal filesystem, glielo dà l'app
            .or_else(|| self.sql_console_name(id).map(str::to_owned))
            .unwrap_or_else(|| "senza nome".into())
    }

    /// Apre il riquadro «Salvare le modifiche?». Chiude ogni altro overlay:
    /// ce n'è uno solo, e questo è una domanda a cui va data risposta.
    pub(super) fn chiedi_chiusura(&mut self, oggetto: DaChiudere) {
        self.close_epoch += 1;
        self.close_after_save = None;
        self.menu_bar.close();
        self.environment_picker
            .update(EnvironmentPickerMessage::Dismiss);
        self.settings_panel.update(SettingsMessage::Dismiss);
        self.global_search.close();
        self.cancel_global_file_search();
        self.reconcile_file_palette();
        self.close_rename();
        self.goto_line = None;
        self.find = None;
        self.context_menu = None;
        self.chiusura = Some(ChiusuraInSospeso {
            oggetto,
            errore: None,
            notifiche_dal: self.notice_seq,
        });
        self.apri_overlay(OverlayId::ConfermaChiusura);
    }

    /// La domanda in sospeso, se il suo riquadro è ancora quello sullo
    /// schermo. Un altro overlay aperto nel frattempo l'ha sostituita: una
    /// risposta arrivata dopo non deve agire su una domanda che non si vede.
    pub(super) fn chiusura_attiva(&self) -> Option<&ChiusuraInSospeso> {
        (self.shell.mostra(OverlayId::ConfermaChiusura))
            .then_some(self.chiusura.as_ref())
            .flatten()
    }

    /// Il riquadro da disegnare, se la domanda è quella sullo schermo.
    pub(crate) fn riquadro_chiusura(&self) -> Option<crate::conferma_chiusura::Riquadro> {
        let chiusura = self.chiusura_attiva()?;
        let errore = chiusura.errore.clone();
        let salvataggio_in_corso =
            self.scritture.guarda(None).in_volo.is_some() || self.git_operazione.is_some();
        Some(match chiusura.oggetto {
            DaChiudere::Scheda(id) => {
                let nome = self.nome_documento(id);
                crate::conferma_chiusura::Riquadro {
                    titolo: format!("Salvare le modifiche a {nome}?"),
                    nomi: vec![nome],
                    errore,
                    salvataggio_in_corso,
                    etichetta_salva: "Salva",
                    etichetta_scarta: if salvataggio_in_corso {
                        "Non salvare altre modifiche"
                    } else {
                        "Non salvare"
                    },
                }
            }
            DaChiudere::Finestra(_) => {
                let nomi: Vec<String> = self
                    .documenti_sporchi()
                    .into_iter()
                    .map(|id| self.nome_documento(id))
                    .collect();
                crate::conferma_chiusura::Riquadro {
                    titolo: if nomi.is_empty() {
                        "Chiusura in attesa".to_owned()
                    } else {
                        format!("{} file con modifiche non salvate", nomi.len())
                    },
                    nomi,
                    errore,
                    salvataggio_in_corso,
                    etichetta_salva: "Salva tutto ed esci",
                    etichetta_scarta: if salvataggio_in_corso {
                        "Esci senza altri salvataggi"
                    } else {
                        "Esci senza salvare"
                    },
                }
            }
        })
    }

    /// Richiesta di chiudere la finestra, dalla X della barra del titolo o
    /// dal sistema. La **decisione** sta qui ed è testabile; l'uscita vera è
    /// in `chiudi_finestra`.
    pub(super) fn richiesta_uscita(&mut self, finestra: Option<iced::window::Id>) -> Uscita {
        if self
            .workspace
            .ids()
            .any(|id| self.pending_editor_input(id) && !self.restored_input_failed(id))
        {
            self.notify_persistente(
                NoticeKind::Info,
                "Input in attesa: completa il caricamento prima di uscire".into(),
            );
            return Uscita::Chiedi;
        }
        if self.documenti_sporchi().is_empty()
            && !(self.scritture.guarda(None).in_volo.is_some() || self.git_operazione.is_some())
        {
            Uscita::Esci
        } else if self
            .chiusura_attiva()
            .is_some_and(|chiusura| matches!(chiusura.oggetto, DaChiudere::Finestra(_)))
        {
            // un secondo Alt+F4 sulla stessa domanda: non si riapre da capo,
            // o l'errore dell'ultimo «Salva tutto» sparirebbe dal riquadro
            Uscita::Chiedi
        } else {
            self.chiedi_chiusura(DaChiudere::Finestra(finestra));
            Uscita::Chiedi
        }
    }

    pub(super) fn chiudi_finestra(&mut self, finestra: Option<iced::window::Id>) -> Task<Message> {
        match self.richiesta_uscita(finestra) {
            Uscita::Esci => self.esci(finestra),
            Uscita::Chiedi => Task::none(),
        }
    }

    /// Esce davvero. La sessione si scrive **prima**: è l'ultima occasione, e
    /// porta con sé la query della console SQL, che non è un file.
    pub(super) fn esci(&mut self, finestra: Option<iced::window::Id>) -> Task<Message> {
        self.invalida_attese_uscita();
        // La Card dipende dal gate, che può svuotarsi durante la raccolta.
        // Nascondere la vista non decide né rimuove la richiesta del gate.
        #[cfg(feature = "mcp")]
        self.chiudi_overlay_se(OverlayId::CardMcp);
        self.uscita_in_corso = true;
        self.mcp_forget_discovery();
        self.cancel_search();
        self.jj_cancel
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(job) = &self.jj_pending_action {
            job.revoke();
        }
        if let Some(job) = &self.jj_pending_load {
            job.revoke();
        }
        use futures::FutureExt;
        // Conserva la raccolta precedente anche su richieste ripetute d'uscita.
        let syntax = self
            .search_exit
            .as_ref()
            .and_then(|exit| exit.syntax.clone())
            .or_else(|| {
                if self.syntax_pool.started_threads() == 0 {
                    return None;
                }
                let collectors = std::mem::take(&mut self.syntax_pool).shutdown();
                self.syntax_expected.clear();
                self.syntax_documents.clear();
                self.syntax_snapshots.clear();
                Some(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let mut success = true;
                            for collector in collectors {
                                success &= collector.join().is_ok();
                            }
                            if success {
                                Ok(())
                            } else {
                                Err("Worker sintattico terminato con panic".to_owned())
                            }
                        })
                        .await
                        .map_err(|error| format!("Raccolta sintassi: {error}"))?
                    }
                    .boxed()
                    .shared(),
                )
            });
        // Revoca anche le future cold della generazione precedente. L'app
        // riceve un owner nuovo per continuare se la chiusura viene invalidata.
        let successor = self.readers.successor();
        let owner = std::mem::replace(&mut self.readers, successor);
        owner.close();
        // Il future condiviso può trasferire owner a spawn_blocking. Conserva
        // quindi un fallback nell'app fino al join, anche se Task/runtime o
        // gate vengono persi. Rimuovi solo raccolte già finite senza attesa UI.
        self.reader_collections.retain(|owner| !owner.finished());
        if owner.has_workers() {
            self.reader_collections.push(owner.collection_owner());
        }
        let previous = self
            .search_exit
            .as_ref()
            .and_then(|exit| exit.readers.clone());
        let readers = if owner.has_workers() || previous.is_some() {
            Some(
                async move {
                    let joined = tokio::task::spawn_blocking(move || owner.collect())
                        .await
                        .map_err(|error| format!("Raccolta letture: {error}"))?;
                    if let Some(previous) = previous {
                        previous.await?;
                    }
                    if joined {
                        Ok(())
                    } else {
                        Err("Raccolta letture fallita".to_owned())
                    }
                }
                .boxed()
                .shared(),
            )
        } else {
            None
        };
        let jj_action = self.jj_pending_action.as_ref().map(|job| job.collector());
        let jj_load = self.jj_pending_load.as_ref().map(|job| job.collector());
        // Anche se Pending e collector della subscription sono già scomparsi,
        // il worker ammesso vive nel registro finale dell'app: l'uscita non
        // può dichiararsi pronta finché non ha raccolto quel JoinHandle.
        let jj_final = self.jj_final.clone();
        let jj_final_pending = jj_final.has_pending();
        if !self.search_jobs.busy()
            && jj_action.is_none()
            && jj_load.is_none()
            && !jj_final_pending
            && syntax.is_none()
            && readers.is_none()
            && !lume_jj::quarantine_has_children()
        {
            self.search_exit = None;
            return self.close_window_task(finestra);
        }
        let Some(epoch) = self.close_epoch.checked_add(1) else {
            self.uscita_in_corso = false;
            return self.notify_error("Identificativi di chiusura esauriti".into());
        };
        self.close_epoch = epoch;
        let jobs = self.search_jobs.snapshot();
        let joins = jobs.clone();
        let action_join = jj_action.clone();
        let load_join = jj_load.clone();
        let syntax_join = syntax.clone();
        let reader_join = readers.clone();
        let completion = crate::search_events::ExitCompletion::new(
            epoch,
            async move {
                let mut success = true;
                for job in joins.0 {
                    success &= job.collect().await.is_ok();
                }
                if let Some(job) = action_join {
                    job.collect().await;
                    success &= job.collected();
                }
                if let Some(job) = load_join {
                    job.collect().await;
                    success &= job.collected();
                }
                if let Some(join) = syntax_join {
                    success &= join.await.is_ok();
                }
                if let Some(join) = reader_join {
                    success &= join.await.is_ok();
                }
                // Raccolta finale e quarantena sul blocking pool, mai nel
                // thread UI: il registro attende lo stesso JoinHandle dei
                // Collector anche quando l'app ne aveva perso ogni altro owner.
                let cleanup = tokio::task::spawn_blocking(move || {
                    let mut success = jj_final.collect_pending();
                    let report = lume_jj::collect_quarantine();
                    success &= report.pending == 0 && report.uncertain == 0;
                    success
                })
                .await;
                success &= cleanup.is_ok_and(|ok| ok);
                success
            }
            .boxed()
            .shared(),
        );
        self.search_exit = Some(crate::search_events::Exit {
            jobs: jobs.clone(),
            completion: completion.clone(),
            jj_action: jj_action.clone(),
            jj_load: jj_load.clone(),
            syntax: syntax.clone(),
            readers: readers.clone(),
            overlay: self.shell.active_overlay(),
            epoch,
            window: finestra,
            file_epoch: self.scritture.guarda(None).epoca,
            root: self.project_root.clone(),
            documents: self
                .groups
                .all()
                .collect::<Vec<_>>()
                .into_iter()
                .filter_map(|id| {
                    self.workspace
                        .get_mut(id)
                        .map(|doc| (id, doc.snapshot_for_save()))
                })
                .collect(),
        });
        Task::perform(completion.collect(), move |success| {
            Message::SearchExitReady(epoch, success)
        })
    }

    pub(super) fn close_window_task(&self, finestra: Option<iced::window::Id>) -> Task<Message> {
        let chiudi = match finestra {
            Some(id) => iced::window::close(id),
            None => iced::window::latest().and_then(iced::window::close),
        };
        self.persist_session().chain(chiudi)
    }

    /// La risposta al riquadro «Salvare le modifiche?».
    pub(super) fn rispondi_chiusura(
        &mut self,
        risposta: crate::conferma_chiusura::ConfermaMessage,
    ) -> Task<Message> {
        use crate::conferma_chiusura::ConfermaMessage;
        let Some(oggetto) = self.chiusura_attiva().map(|chiusura| chiusura.oggetto) else {
            // domanda scaduta: niente da fare, e niente da lasciare in giro
            self.chiusura = None;
            return Task::none();
        };
        match risposta {
            ConfermaMessage::Annulla => {
                self.close_after_save = None;
                self.chiusura = None;
                self.chiudi_overlay_se(OverlayId::ConfermaChiusura);
                Task::none()
            }
            ConfermaMessage::NonSalvare => {
                // Una scrittura già partita non è cancellabile in sicurezza:
                // si aspetta l'esito, senza promettere rollback del disco.
                if let Some(batch) = &mut self.replacement {
                    batch.cancel();
                    let active = self.saves.active_document();
                    let removed: Vec<_> = batch
                        .autosaves
                        .iter()
                        .filter(|(id, _)| {
                            Some(**id) != active
                                && match oggetto {
                                    DaChiudere::Scheda(target) => **id == target,
                                    DaChiudere::Finestra(_) => true,
                                }
                        })
                        .map(|(id, p)| (*id, p.clone()))
                        .collect();
                    for (id, path) in removed {
                        batch.autosaves.remove(&id);
                        batch.report.not_attempted.push(path);
                        batch.report.error =
                            Some("salvataggi restanti revocati dalla chiusura".into());
                    }
                }
                let document = match oggetto {
                    DaChiudere::Scheda(id) => Some(id),
                    DaChiudere::Finestra(_) => None,
                };
                let revocati: Vec<_> = self
                    .save_origins
                    .keys()
                    .copied()
                    .filter(|id| document.is_none_or(|doc| doc == *id))
                    .collect();
                self.scritture.revoca_salvataggi(document);
                for id in revocati {
                    self.termina_autosave_non_avviato(
                        id,
                        Some("Salvataggio revocato dalla chiusura"),
                    );
                }
                match document {
                    Some(id) => {
                        self.save_origins.remove(&id);
                    }
                    None => self.save_origins.clear(),
                }
                if let Some(job) = &self.save_job {
                    job.discard_pending(document);
                }
                self.close_after_save = Some((self.close_epoch, oggetto, false));
                Task::batch([
                    self.finish_replace_report(),
                    self.finish_close_after_saves(),
                ])
            }
            ConfermaMessage::Salva => {
                if let Some(chiusura) = self.chiusura.as_mut() {
                    chiusura.errore = None;
                }
                self.close_after_save = Some((self.close_epoch, oggetto, true));
                match oggetto {
                    DaChiudere::Scheda(id) => self.queue_save(id),
                    DaChiudere::Finestra(_) => {
                        for id in self.documenti_sporchi() {
                            self.queue_save(id);
                        }
                    }
                }
                Task::none()
            }
        }
    }

    /// Imposta il root progetto e notifica il ProjectTool.
    pub(super) fn set_project_root(&mut self, root: PathBuf) {
        self.git_available = lume_git::GitCli::locate().is_some();
        self.cancel_global_file_search();
        self.cancel_search();
        // Il cambio scope è distinto dalla visibilità: una Cerca già aperta
        // non riceverebbe on_visibility_changed. Revoca anche lavoro ancora
        // nella coda UI; non revoca una scrittura già partita.
        self.pending_tool_requests.retain(|(id, request)| {
            *id != tools::search_tool::ID
                || !matches!(
                    request,
                    ToolRequest::RunSearch { .. }
                        | ToolRequest::CancelSearch
                        | ToolRequest::ReplaceAll { .. }
                )
        });
        let visible = self.registry.is_open(tools::search_tool::ID);
        if let Some(tool) = self.registry.get_mut(tools::search_tool::ID) {
            let requests =
                tool.update(&ToolEvent::new(tools::search_tool::Event::ProjectChanged {
                    visible,
                }));
            self.pending_tool_requests.extend(
                requests
                    .into_iter()
                    .map(|request| (tools::search_tool::ID, request)),
            );
        }
        self.cancel_git_action();
        self.project_root = Some(root.clone());
        #[cfg(feature = "sql")]
        {
            self.clear_idea();
            self.pending_tool_requests
                .retain(|(_, request)| !matches!(request, ToolRequest::SqlDetectIdea));
            self.pending_tool_requests
                .push((tools::db::ID, ToolRequest::SqlDetectIdea));
        }
        self.definition_cancel();
        // Le richieste del server precedente sono legate alla sua radice;
        // non lasciare un popup o una domanda di completamento appesa al
        // documento attivo dopo aver distrutto quel workspace LSP.
        self.completion_close();
        self.editor_blame_cache.clear();
        self.editor_blame_generation.clear();
        self.refresh_editor_blame_availability();
        if let Some(id) = self.groups.active() {
            self.queue_editor_blame(id);
        }
        self.syntax_revoke_root();
        self.git_root_epoch += 1;
        self.branch = None;
        // Le letture Git in volo sono del progetto di prima: a viste chiuse
        // dopo il cambio non parte nessun refresh che le superi, quindi si
        // invalidano qui, altrimenti sovrascriverebbero ramo e snapshot.
        for generation in [&self.git_gen, &self.git_log_gen, &self.git_branch_gen] {
            generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        self.project_watch_directories.clear();
        self.project_watch_directories.insert(root.clone());
        if let Some(tool) = self.registry.get_mut(tools::project::ID) {
            tool.update(&ToolEvent::new(tools::project::Event::SetRoot(
                root.clone(),
            )));
        }
        if let Some(tool) = self.registry.get_mut(tools::git_native::ID) {
            tool.update(&ToolEvent::new(tools::git_native::Event::SetRoot(
                root.clone(),
            )));
        }
        self.git_log_reference = tools::gitlog::HEAD.to_string();
        if let Some(tool) = self.registry.get_mut(tools::gitlog::ID) {
            tool.update(&ToolEvent::new(tools::gitlog::Event::SetRoot(root.clone())));
        }
        if let Some(tool) = self.registry.get_mut(tools::terminal::ID) {
            tool.update(&ToolEvent::new(tools::terminal::Event::SetRoot(Some(root))));
        }
        // ATTENZIONE: Il language server è **legato alla radice**: quella dichiarata
        // all'`initialize` non si cambia a caldo. Restare accesi vorrebbe
        // dire un rust-analyzer che indicizza il progetto di prima e ne
        // pubblica le diagnostiche nel dock di quello nuovo — righe che
        // puntano a file fuori dall'albero, e che nessun salvataggio può più
        // chiudere. Si lascia cadere tutto: senza `lsp` il ramo della
        // subscription non viene più prodotto, iced droppa lo stream e il Job
        // Object porta via l'albero dei processi. Il tasto resta lì per
        // riaccenderlo sulla radice giusta.
        if let Some((_, _, doc)) = self.usages_wait.clone() {
            self.usages_cancel_for_doc(doc, "Ricerca interrotta: progetto cambiato.");
        }
        self.lsp.clear();
        self.mcp_drop_for_new_root();
        self.sync_problems_tool();
        let _ = self.sync_extensions_tool();
        self.sync_jj_workspace();
        self.aggiorna_marcatori();
    }

    pub(super) fn set_project_watch_directories(&mut self, paths: Vec<PathBuf>) {
        let Some(root) = self.project_root.as_deref() else {
            self.project_watch_directories.clear();
            return;
        };
        self.project_watch_directories = paths
            .into_iter()
            .filter(|path| lume_core::path_is_within(root, path))
            .collect();
        self.project_watch_directories.insert(root.to_path_buf());
    }

    pub(super) fn fs_watch_directories(&self) -> Vec<PathBuf> {
        let mut directories = self.project_watch_directories.clone();
        for id in &self.groups.all().collect::<Vec<_>>() {
            if let Some(parent) = self
                .workspace
                .get(*id)
                .and_then(|document| document.path())
                .and_then(Path::parent)
            {
                directories.insert(parent.to_path_buf());
            }
        }
        // `.git` e' escluso dall'albero del progetto (fs_tree.rs) e i watch
        // non sono ricorsivi: senza queste due voci un commit dal terminale
        // integrato non aggiorna ne' la pill del branch ne' il log.
        // La radice copre HEAD, index, packed-refs, ORIG_HEAD; i ref sciolti
        // stanno sotto refs/heads, che la radice non vede.
        if let Some(root) = &self.project_root {
            let git_dir = root.join(".git");
            if git_dir.is_dir() {
                let heads = git_dir.join("refs").join("heads");
                directories.insert(git_dir);
                if heads.is_dir() {
                    directories.insert(heads);
                }
            }
        }
        directories.into_iter().collect()
    }

    pub(super) fn refresh_project_tree(&mut self, paths: &[PathBuf]) {
        let refreshed_watch_directories =
            self.registry.get_mut(tools::project::ID).and_then(|tool| {
                tool.update(&ToolEvent::new(tools::project::Event::Refresh(
                    paths.to_vec(),
                )))
                .into_iter()
                .find_map(|request| match request {
                    ToolRequest::SetProjectWatchDirectories(dirs) => Some(dirs),
                    _ => None,
                })
            });
        if let Some(dirs) = refreshed_watch_directories {
            self.set_project_watch_directories(dirs);
        }
    }

    pub(super) fn fs_watcher_subscription(&self) -> Subscription<Message> {
        let directories = self.fs_watch_directories();
        let mut cached = self.watcher_scope.borrow_mut();
        if directories.is_empty() {
            *cached = None;
            return Subscription::none();
        }
        if !cached
            .as_ref()
            .is_some_and(|scope| scope.matches(self.git_root_epoch, &directories))
        {
            *cached = Some(super::fs_bridge::WatchScope::new(
                self.git_root_epoch,
                directories,
            ));
        }
        match cached.as_ref() {
            Some(scope) => super::fs_subscription(scope.clone()),
            None => Subscription::none(),
        }
    }

    pub(super) fn accepts_watch_scope(&self, scope: &super::fs_bridge::WatchScope) -> bool {
        self.watcher_scope
            .borrow()
            .as_ref()
            .is_some_and(|current| scope.same_recipe(current))
            && scope.matches(self.git_root_epoch, &self.fs_watch_directories())
    }

    pub(crate) fn watcher_error(&self) -> Option<&str> {
        let (scope, error) = self.watcher_error.as_ref()?;
        self.accepts_watch_scope(scope).then_some(error.as_str())
    }

    pub(super) fn fs_rescan_paths(&self, directories: Vec<PathBuf>) -> Vec<PathBuf> {
        let current: BTreeSet<_> = self.fs_watch_directories().into_iter().collect();
        let scope: BTreeSet<_> = directories
            .into_iter()
            .filter(|p| current.contains(p))
            .collect();
        let mut paths = scope.clone();
        for id in self.groups.all() {
            if let Some(path) = self.workspace.get(id).and_then(lume_core::Document::path)
                && path.parent().is_some_and(|parent| scope.contains(parent))
            {
                paths.insert(path.to_path_buf());
            }
        }
        paths.into_iter().collect()
    }

    pub(super) fn fs_changed_task(
        &mut self,
        paths: Vec<PathBuf>,
        allow_echo: bool,
    ) -> Task<Message> {
        let git_echo = allow_echo && self.is_git_echo(&paths);
        // un evento dentro `.git` puo' essere un checkout: la pill del
        // ramo va riletta anche a pannello Git chiuso
        let tocca_git = paths
            .iter()
            .any(|path| path.components().any(|c| c.as_os_str() == ".git"));
        let reloads = self.on_fs_changed(paths);
        // Budget: niente lavoro per una vista chiusa. Ogni burst FS
        // costava due discover + status + log(20) + branches +
        // log_from(200) anche con entrambe le viste Git chiuse.
        let mut tasks = vec![reloads];
        if !git_echo {
            if self.registry.is_open(tools::git_native::ID) {
                tasks.push(self.run_git(GitOperation::Refresh));
            } else {
                self.git_stale = true;
                if tocca_git && self.feature_is_enabled(crate::features::Feature::Git) {
                    tasks.push(self.run_git_branch());
                }
            }
            if self.registry.is_open(tools::gitlog::ID) {
                tasks.push(self.run_git_log());
            } else {
                self.git_log_stale = true;
            }
        }
        Task::batch(tasks)
    }

    /// Eventi del watcher: refresh albero e confronto dei byte dei documenti aperti.
    pub(super) fn on_fs_changed(&mut self, paths: Vec<PathBuf>) -> Task<Message> {
        if paths.is_empty() {
            return Task::none();
        }
        self.refresh_project_tree(&paths);
        let mut tasks = Vec::new();
        for p in &paths {
            if let Some(id) = self.workspace.get_by_path(p) {
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
                    self.pending_fs_rechecks.insert(id);
                } else {
                    tasks.push(self.reload_doc(id, p.clone()));
                }
            }
        }
        Task::batch(tasks)
    }

    pub(super) fn reload_pending_fs_recheck(&mut self, id: DocId) -> Task<Message> {
        if !self.pending_fs_rechecks.contains(&id)
            || self.git_operazione.is_some()
            || self
                .scritture
                .guarda(Some((
                    id,
                    self.workspace.get(id).and_then(lume_core::Document::path),
                )))
                .file
                != crate::scritture::Presa::Libero
        {
            return Task::none();
        }
        self.pending_fs_rechecks.remove(&id);
        let Some(path) = self
            .workspace
            .get(id)
            .and_then(lume_core::Document::path)
            .map(Path::to_path_buf)
        else {
            return Task::none();
        };
        self.reload_doc(id, path)
    }

    /// Ricarica dal disco un documento non sporco, in-place: DocId, tab,
    /// Metadati sintattici e storico undo restano gli stessi (il reload è un passo
    /// undo regolare). La lettura va su un thread di lavoro; un errore è
    /// notificato, non ingoiato.
    pub(super) fn reload_doc(&mut self, id: DocId, path: PathBuf) -> Task<Message> {
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
            return Task::none();
        }
        // La versione alla richiesta: se al ritorno è cambiata (l'utente ha
        // scritto e magari salvato), la lettura è più vecchia del documento.
        let Some(versione) = self.workspace.get(id).map(lume_core::Document::version) else {
            return Task::none();
        };
        let per_esito = path.clone();
        let accepted = self.disk_fingerprints.get(&id).copied();
        let snapshot = self
            .workspace
            .get(id)
            .map(lume_core::Document::read_snapshot);
        // Le schede Appunti si rileggono dal NoteStore: nell'editor entra solo
        // il corpo, mai l'intestazione serializzata.
        let nota = self.notes.as_ref().and_then(|notes| {
            notes
                .tabs
                .get(&id)
                .cloned()
                .map(|nota| (nota, notes.store.clone()))
        });
        let reading = self.readers.read(move || match nota {
            Some((nota, Some(store))) => crate::app::notes::nota_da_rileggere(&store, &nota, &path)
                .map(|letta| {
                    let testo = crate::file_read::Testo::from_note(
                        &letta.original_bytes,
                        letta.note.markdown.clone(),
                    )
                    .prepare_reload_unless_unchanged(snapshot, accepted);
                    Ricaricato {
                        testo,
                        nota: Some(letta),
                    }
                }),
            Some((_, None)) => Err("la libreria Appunti non è caricata".to_owned()),
            None => crate::file_read::read_testo_capped(&path, crate::file_read::MAX_FILE_BYTES)
                .map(|testo| Ricaricato {
                    testo: testo.prepare_reload_unless_unchanged(snapshot, accepted),
                    nota: None,
                })
                .map_err(|errore| errore.to_string()),
        });
        Task::perform(
            async move { (id, per_esito, reading.await.unwrap_or_else(Err)) },
            move |(id, path, esito)| Message::DocRicaricato(id, path, versione, esito),
        )
    }

    pub(super) fn doc_ricaricato(
        &mut self,
        id: DocId,
        path: &Path,
        versione: u64,
        esito: Result<Ricaricato, String>,
    ) -> Task<Message> {
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
            // La lettura avviata prima del save può arrivare mentre il writer
            // possiede il documento: non applicarla, ma confrontare di nuovo
            // il disco alla completion. Ignorare risposte di altri binding.
            if self
                .workspace
                .get(id)
                .is_some_and(|doc| doc.path() == Some(path))
            {
                self.pending_fs_rechecks.insert(id);
            }
            return Task::none();
        }
        // Anche gli errori appartengono alla lettura richiesta: una risposta
        // obsoleta non può creare conflitti o avvisi sul documento successore.
        let Some(doc) = self.workspace.get(id) else {
            return Task::none();
        };
        if doc.version() != versione || doc.path() != Some(path) {
            return Task::none();
        }
        let Ricaricato { testo, nota } = match esito {
            Ok(content) => content,
            Err(motivo) => {
                if self.workspace.get(id).is_some_and(|doc| doc.is_dirty()) {
                    self.external_conflicts.insert(id);
                    self.notify_persistente(
                        NoticeKind::Error,
                        format!(
                            "Non riesco a verificare {} sul disco ({motivo}): il buffer modificato è conservato e il salvataggio è bloccato; usa «Salva con nome» o riapri il file",
                            path.display()
                        ),
                    );
                    return Task::none();
                }
                return self.notify_error(format!(
                    "Ricaricamento di {} fallito: {motivo}",
                    path.display()
                ));
            }
        };
        let fingerprint = testo.fingerprint();
        // L'eco del nostro rename atomico ha gli stessi byte dell'ultima
        // versione accettata: non crea una revisione undo artificiale.
        if self.disk_fingerprints.get(&id) == Some(&fingerprint) {
            return Task::none();
        }
        if doc.is_dirty() {
            self.external_conflicts.insert(id);
            self.notify_persistente(
                NoticeKind::Error,
                format!(
                    "{} è cambiato sul disco mentre il buffer aveva modifiche non salvate: il testo locale è conservato e il salvataggio è bloccato; usa «Salva con nome» o riapri il file",
                    path.display()
                ),
            );
            return Task::none();
        }
        // L'impronta accettata può essere cambiata dopo la richiesta. Il
        // risultato classificato sul worker non autorizza un reload senza
        // payload: rileggere usando il binding e lo snapshot correnti.
        if testo.unchanged_reload() {
            return self.reload_doc(id, path.to_path_buf());
        }
        let Some(doc) = self.workspace.get_mut(id) else {
            return Task::none();
        };
        let Some(edit) = testo.apply_reload(doc) else {
            return self
                .notify_error("Ricaricamento preparato obsoleto o già consumato".to_owned());
        };
        self.resync_syntax(id, edit);
        if let Some(letta) = nota {
            self.nota_riletta(id, letta);
        }
        self.disk_fingerprints.insert(id, fingerprint);
        self.external_conflicts.remove(&id);
        if let Some(state) = self
            .notes
            .as_mut()
            .and_then(|notes| notes.salvataggi.get_mut(&id))
        {
            state.risolto();
        }
        Task::none()
    }

    /// Risolve un path assoluto o relativo sul project root.
    pub(super) fn resolve_path(&self, path: &Path) -> Option<PathBuf> {
        if path.is_absolute() {
            return Some(path.to_path_buf());
        }
        let root = self.project_root.as_deref()?;
        let in_src = root.join("src").join(path);
        if in_src.is_file() {
            return Some(in_src);
        }
        let in_root = root.join(path);
        in_root.is_file().then_some(in_root)
    }
}

#[cfg(all(test, not(feature = "sql")))]
mod sql_disabled_snapshot_tests {
    use super::*;

    #[test]
    fn sql_disabled_preserves_console_snapshot_without_opening_editor() {
        let (mut app, _) = Lume::new_without_store();
        let mut snapshot = app.session_snapshot();
        let console = lume_store::SqlConsoleSnapshot {
            editor_text: "select 'città 😀';".into(),
            compare: true,
            sync_scroll: true,
            panes: Vec::new(),
        };
        snapshot.sql_console = Some(console.clone());
        snapshot.bookmarks.push(lume_store::BookmarkRecord {
            path: PathBuf::from("source.rs"),
            line: 3,
            preview: "fn main() {}".into(),
        });
        let expected_bookmarks = snapshot.bookmarks.clone();
        app.restore_snapshot_state(snapshot);
        let saved = app.session_snapshot();
        assert_eq!(saved.sql_console, Some(console));
        assert_eq!(saved.bookmarks, expected_bookmarks);
        assert!(saved.tabs.is_empty());
        let mut cleared = saved;
        cleared.sql_console = None;
        app.restore_snapshot_state(cleared);
        assert!(app.session_snapshot().sql_console.is_none());
    }

    #[tokio::test]
    async fn sql_disabled_persists_query_and_result_sets_through_sqlite_store()
    -> Result<(), Box<dyn std::error::Error>> {
        use futures::StreamExt;
        let directory = tempfile::tempdir()?;
        let store =
            lume_store::StoreHandle::open_at(directory.path().join("session.sqlite3")).await?;
        let (mut app, _) = Lume::new_without_store();
        let result = lume_store::SqlResultSnapshot {
            columns: vec![lume_store::SqlColumnSnapshot {
                name: "valore".into(),
                type_name: "text".into(),
            }],
            rows: vec![vec![lume_store::SqlCellSnapshot::Text("città 😀".into())]],
            elapsed_ms: 7,
        };
        let console = lume_store::SqlConsoleSnapshot {
            editor_text: "select 'città 😀'; select null;".into(),
            compare: true,
            sync_scroll: true,
            panes: vec![lume_store::SqlConsolePaneSnapshot {
                connection_id: Some("connessione-salvata".into()),
                page: 2,
                status: lume_store::SqlPaneStatusSnapshot::Ok,
                results: Some(result.clone()),
                additional_results: vec![result],
                selected_result_set: 1,
            }],
        };
        let mut original = app.session_snapshot();
        original.sql_console = Some(console.clone());
        let source = directory.path().join("source.rs");
        std::fs::write(&source, "fn main() {}\n")?;
        original.tabs = vec![source.clone()];
        original.active_tab = Some(source.clone());
        store.save_session(original).await?;
        let (loaded, _) = store.load_session().await?;
        app.restore_snapshot_state(loaded);
        app.store = Some(store.clone());
        let mut stream = iced_runtime::task::into_stream(app.persist_session())
            .ok_or("Task di persistenza assente")?;
        let output = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await?
            .ok_or("completion di persistenza assente")?;
        let iced_runtime::Action::Output(Message::SessionSaved(result)) = output else {
            return Err("completion di persistenza inattesa".into());
        };
        result?;
        let (saved, _) = store.load_session().await?;
        assert_eq!(saved.sql_console, Some(console));
        assert_eq!(saved.tabs, vec![source.clone()]);
        assert_eq!(saved.active_tab, Some(source));
        Ok(())
    }
}
