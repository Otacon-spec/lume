//! Language server: ciclo di vita, sincronizzazione dei documenti, risposte e rinomina.
//!
//! Metodi di `Lume`: lo stato e i messaggi restano in `app.rs`.

use super::*;

impl Lume {
    // ---------------------------------------------------------- language server

    /// I dati con cui costruire lo stream, se ce n'è da costruire uno.
    ///
    /// ATTENZIONE: Questo è **il** seam del passo: `Subscription` è opaca e non si può
    /// asserire «contiene un ramo LSP», ma su questo sì. Restituire `None`
    /// vuol dire che iced droppa lo stream e il Job Object porta via il
    /// processo — è la forma che ha, qui, la promessa «niente processi da
    /// inattivo».
    pub(crate) fn lsp_spawns(&self) -> Vec<crate::lsp_mode::LspSpawn> {
        self.lsp
            .iter()
            .filter(|lsp| lsp.state.is_live())
            .filter_map(|lsp| {
                Some(crate::lsp_mode::LspSpawn {
                    server_id: lsp.spec.id.clone(),
                    epoch: lsp.epoch,
                    command: lsp.command.clone()?,
                    spec: lsp.initialize_spec(),
                })
            })
            .collect()
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_lsp_ask(&mut self, id: u64, query: &lume_mcp::AppQuery) -> Task<Message> {
        if let Err(error) = self.mcp_lsp_send(id, query)
            && let Some(mcp) = self.mcp.as_ref()
        {
            mcp.answer(id, Err(error));
        }
        Task::none()
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_lsp_send(
        &mut self,
        mcp_id: u64,
        query: &lume_mcp::AppQuery,
    ) -> Result<(), String> {
        let (kind, relative, at) =
            crate::mcp_lsp::Kind::of(query).ok_or("domanda LSP non riconosciuta")?;
        let root = self
            .mcp
            .as_ref()
            .map(|mcp| mcp.root.clone())
            .ok_or("il server MCP non è acceso")?;
        let path = lume_mcp::confine::to_slash(relative);
        let doc = self.mcp_doc_at(&root, relative).ok_or_else(|| {
            format!("{path} non è aperto nell'editor: aprilo con open_in_editor e riprova")
        })?;
        let lsp = self
            .lsp
            .holding(doc)
            .ok_or_else(|| format!("nessun language server ha aperto {path}"))?;
        if !kind.supported_by(lsp) {
            return Err(format!(
                "il language server di {path} non supporta {}",
                kind.tool()
            ));
        }

        // Prima inviamo le battute pendenti: posizione e risposta devono
        // riferirsi allo stesso testo che il server ha ricevuto.
        self.lsp_flush_all_changes();
        let (position, version) = {
            let document = self.workspace.get(doc).ok_or("il file non è più aperto")?;
            let position = match at {
                Some((line, column)) => {
                    crate::mcp_lsp::position_from_query(document.rope(), line, column)?
                }
                None => lsp_types::Position::default(),
            };
            (position, document.version())
        };
        let lsp = self
            .lsp
            .holding_mut(doc)
            .ok_or_else(|| format!("nessun language server ha aperto {path}"))?;
        let uri = lsp
            .opened
            .get(&doc)
            .cloned()
            .ok_or_else(|| format!("nessun language server ha aperto {path}"))?;
        let server = lsp.spec.id.clone();
        let request = lsp.ask_quiet(doc, version);
        if !lsp.send(lume_lsp::LspCommand::Ask {
            id: request,
            request: kind.request(uri, position),
        }) {
            let _ = lsp.answered(request);
            return Err("il language server non risponde più".to_owned());
        }
        if let Some(mcp) = self.mcp.as_mut() {
            mcp.lsp_waits.insert(&server, request, mcp_id);
        }
        Ok(())
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_lsp_claim(&mut self, server: &str, request: u64) -> Option<u64> {
        self.mcp
            .as_mut()
            .and_then(|mcp| mcp.lsp_waits.take(server, request))
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_lsp_answered(
        &mut self,
        mcp_id: u64,
        waiting: crate::lsp_mode::InFlight,
        result: Result<lume_lsp::LspAnswer, String>,
    ) -> Task<Message> {
        if self
            .mcp
            .as_ref()
            .is_some_and(|mcp| mcp.renames.contains_key(&mcp_id))
        {
            return self.mcp_rename_answered(mcp_id, waiting, result);
        }
        let answer = self
            .mcp_lsp_json(waiting, result)
            .map(lume_mcp::AppAnswer::Json);
        if let Some(mcp) = self.mcp.as_ref() {
            mcp.answer(mcp_id, answer);
        }
        Task::none()
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_lsp_json(
        &self,
        waiting: crate::lsp_mode::InFlight,
        result: Result<lume_lsp::LspAnswer, String>,
    ) -> Result<serde_json::Value, String> {
        let root = self
            .mcp
            .as_ref()
            .map(|mcp| mcp.root.clone())
            .ok_or("il server MCP si è fermato")?;
        let answer = result.map_err(|error| format!("language server: {error}"))?;
        if self
            .workspace
            .get(waiting.doc)
            .map(lume_core::Document::version)
            != Some(waiting.version)
        {
            return Err("il testo è cambiato mentre il server rispondeva: riprova".to_owned());
        }
        match answer {
            lume_lsp::LspAnswer::Hover(hover) => Ok(crate::mcp_lsp::hover_json(hover.as_ref())),
            lume_lsp::LspAnswer::Definition(definition) => Ok(crate::mcp_lsp::definitions_json(
                &root,
                crate::lsp_mode::definition_targets(definition),
            )),
            lume_lsp::LspAnswer::References(references) => {
                Ok(crate::mcp_lsp::references_json(&root, references))
            }
            lume_lsp::LspAnswer::Symbols(symbols) => Ok(crate::mcp_lsp::symbols_json(
                &crate::lsp_mode::outline_rows(symbols),
            )),
            _ => Err("risposta inattesa dal language server".to_owned()),
        }
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_lsp_sweep(&mut self, server: &str) {
        let ids = self
            .mcp
            .as_mut()
            .map(|mcp| mcp.lsp_waits.drain_server(server))
            .unwrap_or_default();
        for id in ids {
            let abandoned = self
                .mcp
                .as_mut()
                .and_then(|mcp| mcp.renames.remove(&id))
                .is_some_and(|rename| rename.abandoned);
            if !abandoned {
                self.mcp_reply(
                    id,
                    Err(format!(
                        "il language server {server} si è fermato prima di rispondere"
                    )),
                );
            }
        }
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_rename_start(
        &mut self,
        id: u64,
        conn: u64,
        query: &lume_mcp::AppQuery,
    ) -> Task<Message> {
        if let Err(error) = self.mcp_rename_send(id, conn, query) {
            self.mcp_reply(id, Err(error));
        }
        Task::none()
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_rename_send(
        &mut self,
        id: u64,
        conn: u64,
        query: &lume_mcp::AppQuery,
    ) -> Result<(), String> {
        let lume_mcp::AppQuery::Rename {
            relative,
            line,
            column,
            new_name,
        } = query
        else {
            return Err("domanda di rinomina non riconosciuta".to_owned());
        };
        crate::mcp_lsp::check_new_name(new_name)?;
        let (root, mode) = self
            .mcp
            .as_ref()
            .map(|mcp| (mcp.root.clone(), mcp.mode))
            .ok_or("il server MCP non è acceso")?;
        if let lume_mcp::Verdict::Refuse(reason) = lume_mcp::decide(
            mode,
            lume_mcp::Effect::Modify,
            crate::mcp_mode::target_of(query),
        ) {
            return Err(reason.message().to_owned());
        }
        let slash = lume_mcp::confine::to_slash(relative);
        let doc = self.mcp_doc_at(&root, relative).ok_or_else(|| {
            format!("{slash} non è aperto nell'editor: aprilo con open_in_editor e riprova")
        })?;
        let can_rename = self
            .lsp
            .holding(doc)
            .is_some_and(crate::lsp_mode::LspWorkspace::can_rename);
        if !can_rename {
            return Err(format!("il language server di {slash} non supporta rename"));
        }
        self.lsp_flush_all_changes();
        let (position, version) = {
            let document = self.workspace.get(doc).ok_or("il file non è più aperto")?;
            (
                crate::mcp_lsp::position_from_query(document.rope(), *line, *column)?,
                document.version(),
            )
        };
        let lsp = self
            .lsp
            .holding_mut(doc)
            .ok_or_else(|| format!("nessun language server ha aperto {slash}"))?;
        let uri = lsp
            .opened
            .get(&doc)
            .cloned()
            .ok_or_else(|| format!("nessun language server ha aperto {slash}"))?;
        let server = lsp.spec.id.clone();
        let request = lsp.ask_quiet(doc, version);
        if !lsp.send(lume_lsp::LspCommand::Ask {
            id: request,
            request: lume_lsp::LspRequest::Rename {
                uri,
                position,
                new_name: new_name.clone(),
            },
        }) {
            let _ = lsp.answered(request);
            return Err("il language server non risponde più".to_owned());
        }
        if let Some(mcp) = self.mcp.as_mut() {
            mcp.lsp_waits.insert(&server, request, id);
            mcp.renames.insert(
                id,
                crate::mcp_mode::RenameAsk {
                    conn,
                    new_name: new_name.clone(),
                    abandoned: false,
                },
            );
        }
        Ok(())
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_rename_answered(
        &mut self,
        id: u64,
        waiting: crate::lsp_mode::InFlight,
        result: Result<lume_lsp::LspAnswer, String>,
    ) -> Task<Message> {
        let Some(ask) = self.mcp.as_mut().and_then(|mcp| mcp.renames.remove(&id)) else {
            return Task::none();
        };
        if ask.abandoned {
            return Task::none();
        }
        let edit = (|| {
            let answer = result.map_err(|error| format!("language server: {error}"))?;
            let lume_lsp::LspAnswer::Rename(proposal) = answer else {
                return Err("risposta inattesa dal language server".to_owned());
            };
            let edit = proposal.ok_or("il server non propone modifiche")?;
            if self
                .workspace
                .get(waiting.doc)
                .map(lume_core::Document::version)
                != Some(waiting.version)
            {
                return Err("il testo è cambiato mentre il server rispondeva: riprova".to_owned());
            }
            Ok(edit)
        })();
        let edit = match edit {
            Ok(edit) => edit,
            Err(error) => {
                self.mcp_reply(id, Err(error));
                return Task::none();
            }
        };

        let sources = match self.rename_open_sources(&edit) {
            Ok(sources) => sources,
            Err(error) => {
                let error = self.mcp_rename_safe_error(error);
                self.mcp_reply(id, Err(error));
                return Task::none();
            }
        };
        let Some(root) = self.mcp.as_ref().map(|mcp| mcp.root.clone()) else {
            self.mcp_reply(id, Err("il server MCP si è fermato".to_owned()));
            return Task::none();
        };

        #[cfg(test)]
        {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    self.mcp_reply(id, Err(error.to_string()));
                    return Task::none();
                }
            };
            let (prepared, files) =
                match runtime.block_on(mcp_prepare_workspace_edit_async(root, edit, sources)) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        self.mcp_reply(id, Err(error));
                        return Task::none();
                    }
                };
            self.mcp_rename_admit(id, ask.conn, &ask.new_name, prepared, files)
        }

        #[cfg(not(test))]
        {
            let epoch = self.mcp.as_ref().map_or(0, |mcp| mcp.epoch);
            let conn = ask.conn;
            let new_name = ask.new_name;
            let Some(job_generation) = self.mcp.as_mut().and_then(|mcp| mcp.begin_rename_job(id))
            else {
                self.mcp_reply(id, Err("troppe rinomine in corso".to_owned()));
                return Task::none();
            };
            Task::perform(
                mcp_prepare_workspace_edit_async(root, edit, sources),
                move |result| Message::McpRenamePrepared {
                    epoch,
                    request_id: id,
                    job_generation,
                    conn,
                    new_name,
                    result: result.map(|(prepared, files)| (std::sync::Arc::new(prepared), files)),
                },
            )
        }
    }

    #[cfg(all(feature = "mcp", not(test)))]
    pub(super) fn mcp_rename_prepared(
        &mut self,
        epoch: u64,
        id: u64,
        job_generation: u64,
        conn: u64,
        new_name: &str,
        result: McpRenamePreparedResult,
    ) -> Task<Message> {
        let Some(mcp) = self.mcp.as_mut() else {
            return Task::none();
        };
        if mcp.epoch != epoch || !mcp.finish_rename_job(id, job_generation) {
            return Task::none();
        }
        if !mcp.clients.contains_key(&conn) {
            return Task::none();
        }
        let (prepared, files) = match result.and_then(|(prepared, files)| {
            std::sync::Arc::try_unwrap(prepared)
                .map(|prepared| (prepared, files))
                .map_err(|_| "piano condiviso".to_owned())
        }) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.mcp_reply(id, Err(error));
                return Task::none();
            }
        };
        self.mcp_rename_admit(id, conn, new_name, prepared, files)
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_rename_safe_error(&self, error: crate::rename::RenameError) -> String {
        let message = error.to_string();
        let root = self.mcp.as_ref().map(|mcp| mcp.root.to_string_lossy());
        root.map_or(message.clone(), |root| {
            message.replace(root.as_ref(), "[radice]")
        })
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_rename_admit(
        &mut self,
        id: u64,
        conn: u64,
        new_name: &str,
        edit: crate::rename::PreparedEdit,
        files: Vec<(String, usize)>,
    ) -> Task<Message> {
        use crate::mcp_gate::{Admission, Waiting};
        let (title, lines) = crate::mcp_card::describe_rename(new_name, &files);
        let Some(mcp) = self.mcp.as_mut() else {
            return Task::none();
        };
        let waiting = Waiting {
            id,
            conn,
            effect: lume_mcp::Effect::Modify,
            target: lume_mcp::Target::Inside,
            client: mcp.clients.get(&conn).cloned().unwrap_or_default(),
            title,
            lines,
        };
        let action = crate::mcp_mode::McpAction::Rename {
            edit: Box::new(edit),
            files,
        };
        match mcp.gate.admit(mcp.mode, waiting) {
            Admission::Run => self.mcp_execute(id, action),
            Admission::Queued => {
                mcp.actions.insert(id, action);
                Task::none()
            }
            Admission::Refused(reason) => {
                mcp.answer(id, Err(reason.message().to_owned()));
                Task::none()
            }
        }
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_rename_apply(
        &mut self,
        id: u64,
        prepared: crate::rename::PreparedEdit,
        files: &[(String, usize)],
    ) -> Task<Message> {
        let occurrences = prepared.occurrences();
        match self.apply_prepared(prepared) {
            Ok(touched) => {
                self.mcp_reply(
                    id,
                    Ok(lume_mcp::AppAnswer::Json(crate::mcp_lsp::rename_json(
                        files,
                    ))),
                );
                self.notify(
                    NoticeKind::Info,
                    format!("Rinominati {occurrences} riferimenti in {touched} file — da salvare"),
                )
            }
            Err(error) => {
                let message = self.mcp_rename_safe_error(error);
                self.mcp_reply(id, Err(message.clone()));
                self.notify_error(message)
            }
        }
    }

    #[cfg(feature = "mcp")]
    pub(super) fn mcp_lsp_cancel_all(&mut self) {
        let pending = self
            .mcp
            .as_mut()
            .map(|mcp| mcp.lsp_waits.drain_all())
            .unwrap_or_default();
        for (server, request, mcp_id) in pending {
            if let Some(lsp) = self.lsp.get_mut(&server) {
                let _ = lsp.answered(request);
            }
            let abandoned = self
                .mcp
                .as_mut()
                .and_then(|mcp| mcp.renames.remove(&mcp_id))
                .is_some_and(|rename| rename.abandoned);
            if !abandoned {
                self.mcp_reply(mcp_id, Err("il server MCP è stato fermato".to_owned()));
            }
        }
    }

    pub(crate) fn lsp_active(&self) -> Option<&crate::lsp_mode::LspWorkspace> {
        self.active_path().and_then(|path| self.lsp.serving(path))
    }

    pub(crate) fn lsp_focus(&self) -> Option<&crate::lsp_mode::LspWorkspace> {
        self.lsp.focus(self.active_path())
    }

    pub(super) fn start_lsp(&mut self) -> Task<Message> {
        use crate::lsp_mode::{LspStartAction, LspState, lsp_start_action, server_for};
        let Some(spec) = server_for(self.project_root.as_deref(), self.active_path()) else {
            return self.notify_error("Nessun language server per questo progetto".to_owned());
        };
        let root = self
            .project_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        self.lsp.ensure(spec, root);
        let action = lsp_start_action(self.lsp.get(&spec.id));
        if action == LspStartAction::Scarica {
            return self.download_lsp();
        }
        if action != LspStartAction::Avvia {
            return Task::none();
        }
        let Some(lsp) = self.lsp.get_mut(&spec.id) else {
            return Task::none();
        };
        let epoch = lsp.bump();
        let server = spec.id.clone();
        if lsp.command.is_some() {
            lsp.state = LspState::Avvio;
            return Task::none();
        }
        lsp.state = LspState::Ricerca;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || crate::lsp_mode::resolve_blocking(spec))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Message::LspResolved(server.clone(), epoch, result),
        )
    }

    pub(super) fn stop_lsp(&mut self) -> Task<Message> {
        use crate::lsp_mode::LspState;
        let Some(server) = self.lsp_focus().map(|lsp| lsp.spec.id.clone()) else {
            return Task::none();
        };
        let Some(lsp) = self.lsp.get_mut(&server) else {
            return Task::none();
        };
        if lsp.send(lume_lsp::LspCommand::Shutdown) {
            lsp.state = LspState::Arresto;
            return Task::none();
        }
        self.lsp_forget(&server, LspState::Ferma)
    }

    pub(super) fn lsp_forget(
        &mut self,
        server: &str,
        state: crate::lsp_mode::LspState,
    ) -> Task<Message> {
        #[cfg(feature = "mcp")]
        self.mcp_lsp_sweep(server);
        if self
            .definition_wait
            .as_ref()
            .is_some_and(|(wait_server, _, _)| wait_server == server)
        {
            self.definition_cancel();
        }
        let usages_doc = self
            .usages_wait
            .as_ref()
            .filter(|(wait_server, _, _)| wait_server == server)
            .map(|(_, _, doc)| *doc);
        if let Some(doc) = usages_doc {
            self.usages_cancel_for_doc(doc, "Ricerca interrotta: language server fermato.");
        }
        if let Some(lsp) = self.lsp.get_mut(server) {
            lsp.forget();
            lsp.state = state;
        }
        self.sync_problems_tool();
        let _ = self.sync_extensions_tool();
        Task::none()
    }

    pub(super) fn download_lsp(&mut self) -> Task<Message> {
        use crate::lsp_mode::{
            DownloadThrottle, FaseScarico, LspStartAction, LspState, lsp_start_action, server_for,
        };
        use lume_language_servers::CancelFlag;
        let Some(spec) = server_for(self.project_root.as_deref(), self.active_path()) else {
            return self.notify_error("Nessun language server per questo progetto".to_owned());
        };
        let root = self
            .project_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        self.lsp.ensure(spec, root);
        if lsp_start_action(self.lsp.get(&spec.id)) != LspStartAction::Scarica {
            return Task::none();
        }
        let Some(lsp) = self.lsp.get_mut(&spec.id) else {
            return Task::none();
        };
        let epoch = lsp.epoch;
        let cancel = CancelFlag::default();
        lsp.download_cancel = Some(cancel.clone());
        lsp.state = LspState::Scaricamento {
            received: 0,
            total: 0,
            fase: FaseScarico::Scarico,
        };
        let server = spec.id.clone();
        let (sender, eventi) = futures::channel::mpsc::unbounded();
        let installa = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut filtro = DownloadThrottle::new(std::time::Duration::from_millis(100));
                    crate::lsp_mode::install_blocking(spec, &cancel, |evento| {
                        if filtro.accept(std::time::Instant::now(), &evento) {
                            let _ = sender.unbounded_send(evento);
                        }
                    })
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            {
                let server = server.clone();
                move |result| Message::LspInstalled(server.clone(), epoch, result)
            },
        );
        let avanzamento = Task::run(eventi, move |evento| {
            Message::LspInstallProgress(server.clone(), epoch, evento)
        });
        Task::batch([installa, avanzamento])
    }

    pub(super) fn lsp_install_progress(
        &mut self,
        server: &str,
        epoch: u64,
        progress: &lume_language_servers::InstallProgress,
    ) -> Task<Message> {
        use crate::lsp_mode::{FaseScarico, LspState};
        use lume_language_servers::InstallProgress;
        let Some(lsp) = self.lsp.get_mut(server) else {
            return Task::none();
        };
        if lsp.epoch != epoch {
            return Task::none();
        }
        let (ricevuti, totale) = match &lsp.state {
            LspState::Scaricamento {
                received, total, ..
            } => (*received, *total),
            _ => return Task::none(),
        };
        lsp.state = match progress {
            InstallProgress::Downloading { received, total } => LspState::Scaricamento {
                received: *received,
                total: *total,
                fase: FaseScarico::Scarico,
            },
            InstallProgress::Verifying => LspState::Scaricamento {
                received: ricevuti,
                total: totale,
                fase: FaseScarico::Verifica,
            },
            InstallProgress::Extracting => LspState::Scaricamento {
                received: ricevuti,
                total: totale,
                fase: FaseScarico::Estrazione,
            },
            InstallProgress::Installed => return Task::none(),
        };
        let _ = self.sync_extensions_tool();
        Task::none()
    }

    pub(super) fn lsp_installed(
        &mut self,
        server: &str,
        epoch: u64,
        result: &Result<(), String>,
    ) -> Task<Message> {
        use crate::lsp_mode::LspState;
        let Some(lsp) = self.lsp.get_mut(server) else {
            return Task::none();
        };
        if lsp.epoch != epoch {
            return Task::none();
        }
        lsp.download_cancel = None;
        match result {
            Ok(()) => {
                lsp.state = LspState::Ferma;
                lsp.command = None;
            }
            Err(reason) => lsp.state = LspState::Errore(reason.clone()),
        }
        let _ = self.sync_extensions_tool();
        if result.is_ok() {
            return self.start_lsp();
        }
        Task::none()
    }

    pub(super) fn lsp_resolved(
        &mut self,
        server: &str,
        epoch: u64,
        result: &Result<lume_language_servers::ResolvedServer, String>,
    ) -> Task<Message> {
        use crate::lsp_mode::LspState;
        let Some(lsp) = self.lsp.get_mut(server) else {
            return Task::none();
        };
        if lsp.epoch != epoch {
            return Task::none();
        }
        match result {
            Ok(resolved) => {
                lsp.command = Some(resolved.command.clone());
                lsp.version = resolved.version.clone();
                lsp.state = LspState::Avvio;
            }
            Err(reason) => lsp.state = LspState::Assente(reason.clone()),
        }
        let _ = self.sync_extensions_tool();
        Task::none()
    }

    pub(super) fn lsp_event(
        &mut self,
        server: &str,
        epoch: u64,
        event: crate::lsp_mode::LspEvent,
    ) -> Task<Message> {
        use crate::lsp_mode::{LspEvent, LspState};
        use lume_lsp::ActorEvent;
        if self.lsp.get(server).is_none_or(|lsp| lsp.epoch != epoch) {
            return Task::none();
        }
        match event {
            LspEvent::Ready(mailbox) => {
                if let Some(lsp) = self.lsp.get_mut(server) {
                    lsp.mailbox = Some(mailbox);
                }
            }
            LspEvent::From(ActorEvent::Initialized(ready)) => {
                if let Some(lsp) = self.lsp.get_mut(server) {
                    lsp.availability = lume_lsp::FeatureAvailability::from_capabilities(
                        &ready.capabilities.server,
                    );
                    lsp.mark_ready(ready.peak_memory_bytes.map(crate::lsp_mode::peak_memory_mb));
                }
                self.lsp_open_all(server);
                self.sync_outlines();
                let _ = self.sync_extensions_tool();
            }
            LspEvent::From(ActorEvent::Progress(params)) => {
                if let Some(lsp) = self.lsp.get_mut(server) {
                    lsp.indexing.apply(&params);
                }
            }
            LspEvent::From(ActorEvent::Diagnostics(params)) => {
                let applied = self
                    .lsp
                    .get_mut(server)
                    .is_some_and(|lsp| crate::lsp_mode::publish(lsp, *params));
                if applied {
                    self.sync_problems_tool();
                }
            }
            LspEvent::From(ActorEvent::Log(_)) => {}
            LspEvent::From(ActorEvent::Answer(request, result)) => {
                return self.lsp_answered(server, request, *result);
            }
            LspEvent::From(ActorEvent::Failed(reason)) => {
                return self.lsp_forget(server, LspState::Errore(reason));
            }
            LspEvent::From(ActorEvent::ProcessExited {
                success: false,
                code,
            }) => {
                let reason = code.map_or_else(
                    || "il server LSP è terminato senza codice di uscita".to_owned(),
                    |code| format!("il server LSP è terminato con codice {code}"),
                );
                return self.lsp_forget(server, LspState::Errore(reason));
            }
            LspEvent::From(ActorEvent::ProcessExited { success: true, .. }) => {}
            LspEvent::From(ActorEvent::Stopped) => return self.lsp_forget(server, LspState::Ferma),
        }
        Task::none()
    }

    /// Rigioca i `didOpen` per tutto quel che è già aperto.
    ///
    /// ATTENZIONE: Va fatto a **ogni** avvio, non solo al primo: il `DocumentStore`
    /// muore con la sessione, e senza questo si avrebbe un server vivo e un
    /// editor muto — nessuna diagnostica, e nessun errore da nessuna parte.
    pub(super) fn lsp_open_all(&mut self, server: &str) {
        let Some(lsp) = self.lsp.get(server) else {
            return;
        };
        let paths: Vec<(DocId, PathBuf)> = self
            .groups
            .all()
            .filter(|id| !self.pending_restored_tabs.contains(*id))
            .filter_map(|id| {
                self.workspace
                    .get(id)
                    .and_then(lume_core::Document::path)
                    .map(|path| (id, path.to_path_buf()))
            })
            .collect();
        let candidates = crate::lsp_mode::documents_for(
            lsp,
            paths.iter().map(|(id, path)| (*id, path.as_path())),
        );
        for (id, uri) in candidates {
            self.lsp_send_open(server, id, uri);
        }
    }

    pub(super) fn lsp_send_open(&mut self, server: &str, id: DocId, uri: lsp_types::Uri) {
        let Some((text, version)) = self
            .workspace
            .get(id)
            .map(|document| (document.rope().to_string(), document.version()))
        else {
            return;
        };
        let opened = {
            let Some(lsp) = self.lsp.get_mut(server) else {
                return;
            };
            if lsp.opened.contains_key(&id) {
                return;
            }
            if lsp.send(lume_lsp::LspCommand::Open {
                uri: uri.clone(),
                language: lsp.language_id(),
                text,
            }) {
                lsp.mark_opened(id, uri, version);
                true
            } else {
                false
            }
        };
        if opened {
            self.refresh_marks(id);
        }
    }

    pub(super) fn lsp_did_open(&mut self, id: DocId) {
        use crate::lsp_mode::LspState;
        let Some(path) = self
            .workspace
            .get(id)
            .and_then(lume_core::Document::path)
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        let candidates: Vec<(String, DocId, lsp_types::Uri)> = self
            .lsp
            .iter()
            .filter(|lsp| matches!(lsp.state, LspState::Attivo { .. }))
            .flat_map(|lsp| {
                crate::lsp_mode::documents_for(lsp, [(id, path.as_path())])
                    .into_iter()
                    .map(move |(doc, uri)| (lsp.spec.id.clone(), doc, uri))
            })
            .collect();
        for (server, doc, uri) in candidates {
            self.lsp_send_open(&server, doc, uri);
        }
    }

    pub(super) fn lsp_did_close(&mut self, id: DocId) {
        self.usages_cancel_for_doc(id, "Ricerca interrotta: documento chiuso o rilegato.");
        self.usages_cancel_for_doc(id, "Ricerca interrotta: documento chiuso o rilegato.");
        if self
            .completion_wait
            .as_ref()
            .is_some_and(|wait| wait.doc == id)
            || self
                .completion
                .as_ref()
                .is_some_and(|completion| completion.session.doc == id)
        {
            self.completion_close();
        }
        if self.hover_wait.as_ref().is_some_and(|wait| wait.doc == id)
            || self.hover.as_ref().is_some_and(|shown| shown.doc == id)
        {
            self.hover_clear();
        }
        if self
            .signature_help_wait
            .as_ref()
            .is_some_and(|wait| wait.doc == id)
            || self
                .signature_help
                .as_ref()
                .is_some_and(|shown| shown.doc == id)
        {
            self.signature_help_clear();
        }
        if self
            .definition_wait
            .as_ref()
            .is_some_and(|(_, _, waiting_doc)| *waiting_doc == id)
        {
            self.definition_cancel();
        }
        if self.rename.as_ref().is_some_and(|rename| rename.doc == id) {
            self.close_rename();
        }
        for lsp in self.lsp.iter_mut() {
            if let Some(uri) = lsp.mark_closed(id) {
                lsp.send(lume_lsp::LspCommand::Close { uri });
            }
        }
    }

    pub(super) fn lsp_did_save(&mut self, id: DocId) {
        for lsp in self.lsp.iter() {
            if let Some(uri) = lsp.opened.get(&id).cloned() {
                lsp.send(lume_lsp::LspCommand::Save { uri });
            }
        }
    }

    /// Il `didChange` differito di 200 ms, la stessa attesa della ricerca:
    /// mandarne uno per battuta vorrebbe dire far ri-analizzare il file a
    /// ogni lettera.
    pub(super) fn lsp_change_task(&self) -> Task<Message> {
        let Some(id) = self.active_doc() else {
            return Task::none();
        };
        let Some(version) = self.workspace.get(id).map(lume_core::Document::version) else {
            return Task::none();
        };
        if self
            .lsp
            .holding(id)
            .and_then(|lsp| lsp.wants_change(id, version))
            .is_none()
        {
            return Task::none();
        }
        Task::perform(
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(SEARCH_DEBOUNCE_MS)).await;
                version
            },
            move |version| Message::LspChangeDebounced(id, version),
        )
    }

    pub(super) fn lsp_change_retry_task(&self, id: DocId, tentativi: u8) -> Task<Message> {
        Task::perform(
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(LSP_CHANGE_RETRY_MS)).await;
                tentativi
            },
            move |tentativi| Message::LspChangeRetry(id, tentativi),
        )
    }

    /// Manda il `didChange` in sospeso.
    ///
    /// `atteso` è la versione fotografata quando è partito il timer: se nel
    /// frattempo è cambiata, questo giro è già superato e tace — arriverà
    /// quello dopo. `None` **forza** l'invio, ed è quel che servirà prima di
    /// ogni richiesta posizionale, o le posizioni parlerebbero di un testo
    /// che il server non ha.
    pub(super) fn lsp_flush_change(&mut self, id: DocId, atteso: Option<u64>) -> bool {
        let Some(version) = self.workspace.get(id).map(lume_core::Document::version) else {
            return false;
        };
        if atteso.is_some_and(|atteso| atteso != version) {
            return false;
        }
        let Some(uri) = self
            .lsp
            .holding(id)
            .and_then(|lsp| lsp.wants_change(id, version))
        else {
            return false;
        };
        let Some(text) = self
            .workspace
            .get(id)
            .map(|document| document.rope().clone())
        else {
            return false;
        };
        let Some(lsp) = self.lsp.holding_mut(id) else {
            return false;
        };
        let retry = match lsp
            .mailbox
            .as_ref()
            .map(|mailbox| mailbox.send(lume_lsp::LspCommand::ChangeSnapshot { uri, text }))
        {
            Some(Ok(())) => {
                lsp.mark_sent(id, version);
                false
            }
            Some(Err(lume_lsp::MailboxError::Full)) => true,
            Some(Err(lume_lsp::MailboxError::Closed)) | None => false,
        };
        // il testo è cambiato: la struttura di prima parla di righe che non
        // esistono più, e questo è il momento in cui il server può darne una
        // nuova
        self.sync_outlines();
        retry
    }

    /// Svuota **tutti** i `didChange` in sospeso.
    ///
    /// ATTENZIONE: Non basta quello del documento sotto il cursore, e la ragione non è
    /// ovvia: la risposta parla di posizioni nel testo che il server ha in
    /// mano, e il file **di arrivo** può essere un altro — aperto, con
    /// battute non ancora mandate. Le sue righe sarebbero quelle di prima, e
    /// il salto atterrerebbe altrove senza che niente lo segnali.
    pub(super) fn lsp_flush_all_changes(&mut self) {
        let aperti: Vec<DocId> = self
            .lsp
            .iter()
            .flat_map(|lsp| lsp.opened.keys().copied())
            .collect();
        for id in aperti {
            self.lsp_flush_change(id, None);
        }
    }

    /// Chiude il popup di completamento e dimentica la domanda in volo: una
    /// risposta che arriva dopo non trova più nessuno che la aspetti.
    pub(super) fn completion_close(&mut self) {
        self.completion = None;
        self.completion_wait = None;
    }

    /// Revoca richiesta e reader successivo, anche se l'Answer è già consumato.
    /// Il ritorno alla stessa scheda non riabilita una navigazione revocata.
    pub(super) fn definition_cancel(&mut self) {
        self.definition_epoch = self.definition_epoch.and_then(|epoch| epoch.checked_add(1));
        if let Some((server, request, _)) = self.definition_wait.take()
            && let Some(lsp) = self.lsp.get_mut(&server)
        {
            let _ = lsp.answered(request);
        }
    }

    pub(super) fn usages_cancel_for_doc(&mut self, doc: DocId, reason: &str) {
        let Some((server, request, waiting_doc)) = self.usages_wait.as_ref() else {
            return;
        };
        if *waiting_doc != doc {
            return;
        }
        let (server, request) = (server.clone(), *request);
        self.usages_wait = None;
        if let Some(lsp) = self.lsp.get_mut(&server) {
            let _ = lsp.answered(request);
        }
        self.usages.status = reason.to_owned();
        self.sync_usages_tool();
    }

    /// Il popup di questo documento, **solo** se è quello col fuoco.
    pub(crate) fn completion_popup(&self, id: DocId) -> Option<&lume_ui::editor::EditorPopup> {
        self.completion
            .as_ref()
            .filter(|ui| ui.session.doc == id && self.groups.active() == Some(id))
            .map(|ui| &ui.popup)
    }

    /// Chiede i completamenti nel punto del cursore. Tace se non c'è niente
    /// da chiedere (nessun server, capability non dichiarata, selezione non
    /// vuota): la voce di menù in quei casi non c'è, e una scorciatoia che
    /// notifica «non disponibile» a ogni pressione sarebbe rumore.
    pub(super) fn request_completion_from_menu(&mut self) -> Task<Message> {
        let Some(id) = self.active_doc() else {
            return Task::none();
        };
        let action = DeferredRestoredAction::Completion;
        if self.pending_restored_tabs.contains(id) {
            return self.defer_restored_action(id, action);
        }
        self.forget_restored_action(id, &action);
        self.request_completion_for(id);
        Task::none()
    }

    pub(super) fn request_completion_for(&mut self, id: DocId) {
        use lume_lsp::{LspCommand, LspRequest};
        if self.active_doc() != Some(id) || self.pending_restored_tabs.contains(id) {
            return;
        }
        if self.lsp.holding(id).is_none_or(|lsp| !lsp.can_complete()) {
            return;
        }
        // ATTENZIONE: Prima il testo, poi la domanda: la posizione parla del testo
        // che il server ha in mano.
        self.lsp_flush_change(id, None);
        let Some(documento) = self.workspace.get(id) else {
            return;
        };
        if !documento.selection().is_empty() {
            return;
        }
        let testa = documento.selection().head;
        let Some(position) = crate::lsp_pos::position_of(documento.rope(), testa) else {
            return;
        };
        let version = documento.version();
        let anchor = crate::completion::word_start(documento.rope(), testa);
        let Some(lsp) = self.lsp.holding_mut(id) else {
            return;
        };
        let Some(uri) = lsp.opened.get(&id).cloned() else {
            return;
        };
        let server = lsp.spec.id.clone();
        let richiesta = lsp.ask_quiet(id, version);
        if !lsp.send(LspCommand::Ask {
            id: richiesta,
            request: LspRequest::Completion { uri, position },
        }) {
            // mailbox morta: la domanda non è partita
            lsp.answered(richiesta);
            return;
        }
        self.completion_wait = Some(crate::completion_ui::CompletionWait {
            server,
            request: richiesta,
            doc: id,
            anchor,
        });
    }

    /// La risposta del server: apre il popup se l'attesa è ancora quella e
    /// se il cursore è ancora nella parola di quando si è chiesto.
    pub(super) fn completion_answered(
        &mut self,
        server: &str,
        richiesta: u64,
        doc: DocId,
        risposta: Option<lsp_types::CompletionResponse>,
    ) {
        let Some(attesa) = self.completion_wait.take_if(|attesa| {
            attesa.server == server && attesa.request == richiesta && attesa.doc == doc
        }) else {
            return;
        };
        if self.active_doc() != Some(doc) {
            return;
        }
        let Some(risposta) = risposta else {
            return;
        };
        let Some(documento) = self.workspace.get(doc) else {
            return;
        };
        if !documento.selection().is_empty() {
            return;
        }
        let testa = documento.selection().head;
        let Some(sessione) = crate::completion::CompletionSession::new(
            doc,
            documento.version(),
            documento.rope(),
            testa,
            risposta,
        ) else {
            return;
        };
        if sessione.anchor() != attesa.anchor {
            return;
        }
        self.completion = Some(crate::completion_ui::CompletionUi::new(sessione));
        self.hover_clear();
    }

    /// Dopo un comando dell'editor: il popup segue (`follow`), e un carattere
    /// che apre da solo il completamento lo chiede.
    pub(super) fn completion_follow(
        &mut self,
        follow: crate::completion_ui::Follow,
        inserito: Option<&str>,
    ) {
        use crate::completion_ui::Follow;
        let Some(id) = self.active_doc() else {
            return;
        };
        // un popup rimasto a un altro documento non parla di questo testo
        if self
            .completion
            .as_ref()
            .is_some_and(|ui| ui.session.doc != id)
        {
            self.completion_close();
        }
        match follow {
            Follow::Keep => {}
            Follow::Close => self.completion_close(),
            Follow::Refilter => {
                let vivo = match (self.completion.as_mut(), self.workspace.get(id)) {
                    (Some(ui), Some(documento)) => {
                        let vivo = ui
                            .session
                            .refilter(documento.rope(), documento.selection().head);
                        if vivo {
                            ui.session.version = documento.version();
                            ui.sync();
                        }
                        Some(vivo)
                    }
                    _ => None,
                };
                if vivo == Some(false) {
                    self.completion_close();
                }
            }
        }
        if let Some(testo) = inserito
            && self.completion.is_none()
            && self.workspace.get(id).is_some_and(|documento| {
                crate::completion::should_open(
                    testo,
                    &crate::completion_ui::line_before(
                        documento.rope(),
                        documento.selection().head,
                    ),
                )
            })
        {
            self.request_completion_for(id);
        }
    }

    /// Chiude scheda e ritira timer/richiesta hover.
    pub(super) fn hover_clear(&mut self) {
        self.hover_gen = self.hover_gen.wrapping_add(1);
        self.hover = None;
        self.hover_wait = None;
    }

    pub(crate) fn hover_card(&self, id: DocId) -> Option<&lume_ui::editor::HoverCard> {
        self.signature_help
            .as_ref()
            .filter(|shown| shown.doc == id)
            .map(|shown| &shown.card)
            .or_else(|| {
                self.hover
                    .as_ref()
                    .filter(|shown| shown.doc == id)
                    .map(|shown| &shown.card)
            })
    }

    pub(super) fn signature_help_clear(&mut self) {
        self.signature_help_wait = None;
        self.signature_help = None;
    }

    pub(super) fn signature_help_at_cursor(&mut self) -> Task<Message> {
        let Some(id) = self.active_doc() else {
            return Task::none();
        };
        let action = DeferredRestoredAction::SignatureHelp;
        if self.pending_restored_tabs.contains(id) {
            return self.defer_restored_action(id, action);
        }
        self.forget_restored_action(id, &action);
        if self
            .lsp
            .holding(id)
            .is_none_or(|lsp| !lsp.can_signature_help())
        {
            return self.notify(
                NoticeKind::Info,
                "Il language server non offre signature help".to_owned(),
            );
        }
        self.signature_help_request(id, None);
        Task::none()
    }

    pub(super) fn signature_help_request(&mut self, id: DocId, trigger_character: Option<String>) {
        use lume_lsp::{LspCommand, LspRequest};
        self.signature_help_clear();
        if self.active_doc() != Some(id)
            || self.pending_restored_tabs.contains(id)
            || self
                .lsp
                .holding(id)
                .is_none_or(|lsp| !lsp.can_signature_help())
        {
            return;
        }
        self.lsp_flush_change(id, None);
        let Some(document) = self.workspace.get(id) else {
            return;
        };
        let anchor = document.selection().head;
        let Some(position) = crate::lsp_pos::position_of(document.rope(), anchor) else {
            return;
        };
        let version = document.version();
        let Some(lsp) = self.lsp.holding_mut(id) else {
            return;
        };
        let Some(uri) = lsp.opened.get(&id).cloned() else {
            return;
        };
        let server = lsp.spec.id.clone();
        let request = lsp.ask_quiet(id, version);
        if !lsp.send(LspCommand::Ask {
            id: request,
            request: LspRequest::SignatureHelp {
                uri,
                position,
                trigger_character,
            },
        }) {
            lsp.answered(request);
            return;
        }
        self.signature_help_wait = Some(crate::signature_help::SignatureHelpWait {
            server,
            request,
            doc: id,
            version,
            anchor,
        });
    }

    pub(super) fn signature_help_answered(
        &mut self,
        server: &str,
        request: u64,
        doc: DocId,
        answer: Option<lsp_types::SignatureHelp>,
    ) {
        let Some(wait) = self
            .signature_help_wait
            .take_if(|wait| wait.server == server && wait.request == request && wait.doc == doc)
        else {
            return;
        };
        if self.active_doc() != Some(doc)
            || self.workspace.get(doc).map(lume_core::Document::version) != Some(wait.version)
            || self.completion.is_some()
        {
            return;
        }
        let Some(card) = answer
            .as_ref()
            .and_then(|help| crate::signature_help::card(help, wait.anchor))
        else {
            return;
        };
        self.signature_help = Some(crate::signature_help::SignatureHelpShown { doc, card });
        self.hover_clear();
    }

    pub(super) fn hover_at(
        &mut self,
        group: crate::editor_groups::GroupId,
        pos: Option<usize>,
    ) -> Task<Message> {
        self.signature_help_clear();
        self.hover_clear();
        let Some(pos) = pos else { return Task::none() };
        let Some(id) = self.groups.active_in(group) else {
            return Task::none();
        };
        if self.pending_restored_tabs.contains(id) {
            return Task::none();
        }
        if self.completion.is_some() || self.lsp.holding(id).is_none_or(|lsp| !lsp.can_hover()) {
            return Task::none();
        }
        let generation = self.hover_gen;
        Task::perform(
            async { tokio::time::sleep(std::time::Duration::from_millis(HOVER_DELAY_MS)).await },
            move |()| Message::HoverDue(id, generation, pos),
        )
    }

    pub(super) fn hover_request(&mut self, id: DocId, pos: usize) {
        use lume_lsp::{LspCommand, LspRequest};
        if self.pending_restored_tabs.contains(id)
            || self.lsp.holding(id).is_none_or(|lsp| !lsp.can_hover())
        {
            return;
        }
        self.lsp_flush_change(id, None);
        let Some(document) = self.workspace.get(id) else {
            return;
        };
        let Some(position) = crate::lsp_pos::position_of(document.rope(), pos) else {
            return;
        };
        let version = document.version();
        let Some(lsp) = self.lsp.holding_mut(id) else {
            return;
        };
        let Some(uri) = lsp.opened.get(&id).cloned() else {
            return;
        };
        let server = lsp.spec.id.clone();
        let request = lsp.ask_quiet(id, version);
        if !lsp.send(LspCommand::Ask {
            id: request,
            request: LspRequest::Hover { uri, position },
        }) {
            lsp.answered(request);
            return;
        }
        self.hover_wait = Some(crate::hover::HoverWait {
            server,
            request,
            doc: id,
            anchor: pos,
            version,
        });
    }

    pub(super) fn hover_at_cursor(&mut self) -> Task<Message> {
        let Some(id) = self.active_doc() else {
            return Task::none();
        };
        let action = DeferredRestoredAction::QuickDoc;
        if self.pending_restored_tabs.contains(id) {
            return self.defer_restored_action(id, action);
        }
        self.forget_restored_action(id, &action);
        self.hover_at_cursor_for(id);
        Task::none()
    }

    pub(super) fn hover_at_cursor_for(&mut self, id: DocId) {
        if self.active_doc() != Some(id) || self.pending_restored_tabs.contains(id) {
            return;
        }
        let Some(document) = self.workspace.get(id) else {
            return;
        };
        let anchor = crate::completion::word_start(document.rope(), document.selection().head);
        self.hover_clear();
        self.hover_request(id, anchor);
    }

    pub(super) fn hover_answered(
        &mut self,
        server: &str,
        request: u64,
        doc: DocId,
        answer: Option<lsp_types::Hover>,
    ) {
        let Some(wait) = self
            .hover_wait
            .take_if(|w| w.server == server && w.request == request && w.doc == doc)
        else {
            return;
        };
        let Some(answer) = answer else { return };
        if self.workspace.get(doc).map(lume_core::Document::version) != Some(wait.version)
            || self.completion.is_some()
        {
            return;
        }
        let lines = crate::hover::hover_lines(&answer);
        if !lines.is_empty() {
            self.hover = Some(crate::hover::HoverShown {
                doc,
                card: lume_ui::editor::HoverCard {
                    anchor: wait.anchor,
                    lines,
                },
            });
        }
    }

    /// Ctrl+B: chiede dove è definito il simbolo sotto il cursore.
    ///
    /// Tace quando non c'è niente da chiedere — nessun server, non avviato,
    /// capability non dichiarata. La voce di menù in quei casi **non c'è**
    /// (`item_visible`), e una scorciatoia che notifica «non disponibile» a
    /// ogni pressione in un progetto senza server sarebbe rumore.
    pub(super) fn goto_definition(&mut self) -> Task<Message> {
        let Some(id) = self.groups.active() else {
            return Task::none();
        };
        let action = DeferredRestoredAction::GotoDefinition;
        if self.pending_restored_tabs.contains(id) {
            return self.defer_restored_action(id, action);
        }
        self.forget_restored_action(id, &action);
        self.goto_definition_for(id)
    }

    pub(super) fn goto_definition_for(&mut self, id: DocId) -> Task<Message> {
        if self.active_doc() != Some(id) || self.pending_restored_tabs.contains(id) {
            return Task::none();
        }
        if self.lsp.holding(id).is_none_or(|lsp| !lsp.can_define()) {
            return Task::none();
        }
        self.definition_cancel();
        if self.definition_epoch.is_none() {
            return self.notify(
                NoticeKind::Info,
                "Richieste di salto LSP esaurite: nessuna domanda avviata".into(),
            );
        }
        // ATTENZIONE: Prima la posizione, poi la domanda: il flush cambia quel che il
        // server ha in mano, non il testo, ma la revisione fotografata deve
        // essere quella **dopo** l'invio o la risposta sembrerebbe superata.
        self.lsp_flush_all_changes();
        let Some((position, version)) = self.workspace.get(id).and_then(|documento| {
            let position =
                crate::lsp_pos::position_of(documento.rope(), documento.selection().head)?;
            Some((position, documento.version()))
        }) else {
            return Task::none();
        };
        let Some(lsp) = self.lsp.holding_mut(id) else {
            return Task::none();
        };
        let Some(uri) = lsp.opened.get(&id).cloned() else {
            return Task::none();
        };
        let richiesta = lsp.ask(id, version);
        if !lsp.send(lume_lsp::LspCommand::Ask {
            id: richiesta,
            request: lume_lsp::LspRequest::Definition { uri, position },
        }) {
            // mailbox morta: la domanda non è partita, e lasciarla in sospeso
            // vorrebbe dire un'attesa che non finisce
            lsp.answered(richiesta);
        } else {
            self.definition_wait = Some((lsp.spec.id.clone(), richiesta, id));
        }
        Task::none()
    }

    /// La parola sotto il cursore.
    ///
    /// ATTENZIONE: **Non** è il simbolo del language server — quello lo conosce solo
    /// lui, e chiederglielo sarebbe un giro in più. Serve a un'intestazione,
    /// non a una richiesta: sul filo va solo la posizione. Se il cursore non
    /// è su una parola torna vuota, e l'intestazione lo sa.
    pub(super) fn parola_attorno(documento: &lume_core::Document, cursore: usize) -> String {
        let rope = documento.rope();
        let len = rope.len_chars();
        let cursore = cursore.min(len);
        let di_parola = |c: char| c.is_alphanumeric() || c == '_';
        let mut inizio = cursore;
        while inizio > 0 && rope.get_char(inizio - 1).is_some_and(di_parola) {
            inizio -= 1;
        }
        let mut fine = cursore;
        while fine < len && rope.get_char(fine).is_some_and(di_parola) {
            fine += 1;
        }
        rope.slice(inizio..fine).to_string()
    }

    /// Alt+F7: chiede dove compare il simbolo sotto il cursore.
    ///
    /// Il dock si apre **subito**, con scritto che sta cercando: su un
    /// simbolo molto usato la risposta può metterci secondi, e un comando che
    /// non dà segno di vita per tre secondi sembra non aver funzionato.
    pub(super) fn find_usages(&mut self) -> Task<Message> {
        let Some(id) = self.groups.active() else {
            return Task::none();
        };
        let action = DeferredRestoredAction::FindUsages;
        if self.pending_restored_tabs.contains(id) {
            return self.defer_restored_action(id, action);
        }
        self.forget_restored_action(id, &action);
        self.find_usages_for(id)
    }

    pub(super) fn find_usages_for(&mut self, id: DocId) -> Task<Message> {
        if self.active_doc() != Some(id) || self.pending_restored_tabs.contains(id) {
            return Task::none();
        }
        if self
            .lsp
            .holding(id)
            .is_none_or(|lsp| !lsp.can_find_usages())
        {
            return Task::none();
        }
        if let Some((_, _, previous_doc)) = self.usages_wait.clone() {
            self.usages_cancel_for_doc(
                previous_doc,
                "Ricerca sostituita da una richiesta più recente.",
            );
        }
        self.lsp_flush_all_changes();
        let Some((position, version, simbolo)) = self.workspace.get(id).and_then(|documento| {
            let testa = documento.selection().head;
            let position = crate::lsp_pos::position_of(documento.rope(), testa)?;
            Some((
                position,
                documento.version(),
                Self::parola_attorno(documento, testa),
            ))
        }) else {
            return Task::none();
        };
        let Some(uri) = self
            .lsp
            .holding(id)
            .and_then(|lsp| lsp.opened.get(&id).cloned())
        else {
            return Task::none();
        };
        let Some(lsp) = self.lsp.holding_mut(id) else {
            return Task::none();
        };
        let richiesta = lsp.ask(id, version);
        if !lsp.send(lume_lsp::LspCommand::Ask {
            id: richiesta,
            request: lume_lsp::LspRequest::References { uri, position },
        }) {
            lsp.answered(richiesta);
            return Task::none();
        }
        let server = lsp.spec.id.clone();
        self.usages_wait = Some((server, richiesta, id));
        self.usages = tools::usages::Snapshot {
            symbol: simbolo,
            groups: Vec::new(),
            status: "Ricerca in corso…".to_owned(),
        };
        self.sync_usages_tool();
        let requests = self.registry.set_active_dock(Some(tools::usages::ID));
        self.pending_tool_requests.extend(requests);
        Task::none()
    }

    pub(super) fn sync_usages_tool(&mut self) {
        let snapshot = self.usages.clone();
        if let Some(tool) = self.registry.get_mut(tools::usages::ID) {
            tool.update(&ToolEvent::new(tools::usages::Event::Replace(snapshot)));
        }
    }

    /// Maiusc+F6: apre il riquadro della rinomina.
    ///
    /// Passa da `prepareRename` quando il server la dichiara: così su una
    /// parola chiave il riquadro **non si apre affatto**, invece di aprirsi,
    /// far digitare un nome e dire di no dopo. Dove non c'è, si apre subito
    /// con la parola sotto il cursore.
    pub(super) fn start_rename(&mut self) -> Task<Message> {
        let Some(id) = self.groups.active() else {
            return Task::none();
        };
        let action = DeferredRestoredAction::Rename;
        if self.pending_restored_tabs.contains(id) {
            return self.defer_restored_action(id, action);
        }
        self.forget_restored_action(id, &action);
        self.start_rename_for(id)
    }

    pub(super) fn start_rename_for(&mut self, id: DocId) -> Task<Message> {
        if self.active_doc() != Some(id) || self.pending_restored_tabs.contains(id) {
            return Task::none();
        }
        if self.lsp.holding(id).is_none_or(|lsp| !lsp.can_rename()) {
            return Task::none();
        }
        self.lsp_flush_all_changes();
        let Some((position, version, parola)) = self.workspace.get(id).and_then(|documento| {
            let testa = documento.selection().head;
            let position = crate::lsp_pos::position_of(documento.rope(), testa)?;
            Some((
                position,
                documento.version(),
                Self::parola_attorno(documento, testa),
            ))
        }) else {
            return Task::none();
        };
        let prepara = self
            .lsp
            .holding(id)
            .is_some_and(crate::lsp_mode::LspWorkspace::can_prepare_rename);
        if !prepara {
            return self.open_rename_box(id, position, version, parola);
        }
        let Some(uri) = self
            .lsp
            .holding(id)
            .and_then(|lsp| lsp.opened.get(&id).cloned())
        else {
            return Task::none();
        };
        let Some(lsp) = self.lsp.holding_mut(id) else {
            return Task::none();
        };
        let richiesta = lsp.ask(id, version);
        if !lsp.send(lume_lsp::LspCommand::Ask {
            id: richiesta,
            request: lume_lsp::LspRequest::PrepareRename { uri, position },
        }) {
            lsp.answered(richiesta);
        }
        Task::none()
    }

    pub(super) fn open_rename_box(
        &mut self,
        doc: DocId,
        position: lsp_types::Position,
        version: u64,
        nome: String,
    ) -> Task<Message> {
        if nome.is_empty() {
            return Task::none();
        }
        self.rename = Some(crate::rename_box::RenameBox::new(
            doc, position, version, nome,
        ));
        self.menu_bar.close();
        self.apri_overlay(OverlayId::Rename);
        iced::widget::operation::focus(iced::widget::Id::new(crate::prompt::INPUT_ID))
    }

    /// Apre il menù contestuale, **se ha qualcosa da mostrare**.
    ///
    /// Restituisce `false` quando non si apre: è il seam su cui si asserisce,
    /// perché l'overlay è un `Element` opaco.
    pub(crate) fn open_context_menu(&mut self) -> bool {
        if self.context_menu.is_none() {
            return false;
        }
        let vuoto = match self.context_menu.as_ref() {
            Some(menu) => crate::context_menu::items(&menu.soggetto, self.menu_checks()).is_empty(),
            None => true,
        };
        if vuoto {
            self.context_menu = None;
            return false;
        }
        self.menu_bar.close();
        self.apri_overlay(OverlayId::Context);
        true
    }

    /// Apre il menù contestuale su un soggetto: una scheda o un nodo
    /// dell'albero. Il punto è quello del clic: il menù si apre dove si è
    /// premuto, e in iced 0.14 l'evento di pressione non lo porta.
    pub(super) fn apri_menu_contestuale(
        &mut self,
        soggetto: crate::context_menu::Soggetto,
        punto: iced::Point,
    ) {
        self.context_menu = Some(crate::context_menu::ContextMenu {
            x: punto.x,
            y: punto.y,
            soggetto,
        });
        self.open_context_menu();
    }

    pub(super) fn apply_rename_message(
        &mut self,
        message: crate::rename_box::RenameMessage,
    ) -> Task<Message> {
        use crate::rename_box::RenameMessage;
        match message {
            RenameMessage::Changed(testo) => {
                if let Some(riquadro) = self.rename.as_mut() {
                    riquadro.name = testo;
                }
                Task::none()
            }
            RenameMessage::Cancel => {
                self.close_rename();
                Task::none()
            }
            RenameMessage::Confirm => self.send_rename(),
        }
    }

    pub(crate) fn close_rename(&mut self) {
        self.rename = None;
        self.chiudi_overlay_se(OverlayId::Rename);
    }

    pub(super) fn send_rename(&mut self) -> Task<Message> {
        let Some(riquadro) = self.rename.as_ref() else {
            return Task::none();
        };
        if !riquadro.is_applicable() {
            return Task::none();
        }
        let (doc, position, nuovo) = (
            riquadro.doc,
            riquadro.position,
            riquadro.name.trim().to_owned(),
        );
        // ATTENZIONE: La revisione si rilegge **adesso**: fra l'apertura del riquadro
        // e l'invio l'utente può aver battuto nell'editor dietro, e la
        // posizione va misurata sul testo che il server ha in mano.
        let Some(version) = self.workspace.get(doc).map(lume_core::Document::version) else {
            return Task::none();
        };
        let Some(uri) = self
            .lsp
            .holding(doc)
            .filter(|lsp| lsp.can_rename())
            .and_then(|lsp| lsp.opened.get(&doc).cloned())
        else {
            return Task::none();
        };
        let Some(lsp) = self.lsp.holding_mut(doc) else {
            return Task::none();
        };
        let richiesta = lsp.ask(doc, version);
        if !lsp.send(lume_lsp::LspCommand::Ask {
            id: richiesta,
            request: lume_lsp::LspRequest::Rename {
                uri,
                position,
                new_name: nuovo,
            },
        }) {
            lsp.answered(richiesta);
            return Task::none();
        }
        if let Some(riquadro) = self.rename.as_mut() {
            riquadro.in_corso = true;
            riquadro.version = version;
        }
        Task::none()
    }

    /// La richiesta parte solo dopo l'ammissione del testo corrente nella mailbox.
    pub(super) fn format_document(&mut self) -> Task<Message> {
        let Some(doc) = self.active_doc() else {
            return self.notify_error("Nessun documento da formattare".to_owned());
        };
        if self.pending_restored_tabs.contains(doc) {
            return self.notify(
                NoticeKind::Info,
                "Documento in caricamento: riprova al termine".to_owned(),
            );
        }
        if self.lsp.holding(doc).is_none_or(|lsp| !lsp.can_format()) {
            return self
                .notify_error("Formattazione non disponibile per questo documento".to_owned());
        }
        self.lsp_flush_change(doc, None);
        let Some(version) = self.workspace.get(doc).map(lume_core::Document::version) else {
            return self.notify_error("Documento non più disponibile".to_owned());
        };
        let Some(lsp) = self.lsp.holding_mut(doc) else {
            return self.notify_error("Language server non più disponibile".to_owned());
        };
        if !lsp.is_synced(doc, version) {
            return self.notify_error("Testo non ancora inviato al server: riprova".to_owned());
        }
        let Some(uri) = lsp.opened.get(&doc).cloned() else {
            return self.notify_error("Documento non aperto dal language server".to_owned());
        };
        let request = lsp.ask(doc, version);
        if !lsp.send(lume_lsp::LspCommand::Ask {
            id: request,
            request: lume_lsp::LspRequest::Formatting {
                uri,
                tab_size: 4,
                insert_spaces: true,
            },
        }) {
            lsp.answered(request);
            return self.notify_error("Richiesta di formattazione non inviata: riprova".to_owned());
        }
        self.notify(NoticeKind::Info, "Formattazione in corso…".to_owned())
    }

    /// Formatta solo la selezione corrente: la richiesta va al server con il
    /// range della selezione, e la risposta usa lo stesso percorso di
    /// `format_document` (WorkspaceEdit con preflight di identità/revisione).
    pub(super) fn format_selection(&mut self) -> Task<Message> {
        let Some(doc) = self.active_doc() else {
            return self.notify_error("Nessun documento da formattare".to_owned());
        };
        let (selezione, rope) = match self.workspace.get(doc) {
            Some(documento) => (documento.selection(), documento.rope().clone()),
            None => return self.notify_error("Documento non più disponibile".to_owned()),
        };
        if self.pending_restored_tabs.contains(doc) {
            return self.notify(
                NoticeKind::Info,
                "Documento in caricamento: riprova al termine".to_owned(),
            );
        }
        if selezione.is_empty() {
            return self
                .notify_error("Nessuna selezione: seleziona il testo da formattare".to_owned());
        }
        if self
            .lsp
            .holding(doc)
            .is_none_or(|lsp| !lsp.can_format_range())
        {
            return self.notify_error(
                "Formattazione di un intervallo non disponibile per questo documento".to_owned(),
            );
        }
        self.lsp_flush_change(doc, None);
        let Some(version) = self.workspace.get(doc).map(lume_core::Document::version) else {
            return self.notify_error("Documento non più disponibile".to_owned());
        };
        let Some(lsp) = self.lsp.holding_mut(doc) else {
            return self.notify_error("Language server non più disponibile".to_owned());
        };
        if !lsp.is_synced(doc, version) {
            return self.notify_error("Testo non ancora inviato al server: riprova".to_owned());
        }
        let Some(uri) = lsp.opened.get(&doc).cloned() else {
            return self.notify_error("Documento non aperto dal language server".to_owned());
        };
        let Some(inizio) = crate::lsp_pos::position_of(&rope, selezione.min()) else {
            return self.notify_error("Selezione non valida".to_owned());
        };
        let Some(fine) = crate::lsp_pos::position_of(&rope, selezione.max()) else {
            return self.notify_error("Selezione non valida".to_owned());
        };
        let range = lsp_types::Range {
            start: inizio,
            end: fine,
        };
        let request = lsp.ask(doc, version);
        if !lsp.send(lume_lsp::LspCommand::Ask {
            id: request,
            request: lume_lsp::LspRequest::RangeFormatting {
                uri,
                range,
                tab_size: 4,
                insert_spaces: true,
            },
        }) {
            lsp.answered(request);
            return self.notify_error("Richiesta di formattazione non inviata: riprova".to_owned());
        }
        self.notify(
            NoticeKind::Info,
            "Formattazione della selezione in corso…".to_owned(),
        )
    }

    /// Chiede al server le azioni rapide disponibili per il documento o la
    /// selezione corrente. La risposta arriva come `LspAnswer::CodeAction` e
    /// apre l'elenco: da lì l'utente sceglie, e la scelta applica le edit
    /// (o esegue il comando) con lo stesso preflight della formattazione.
    pub(super) fn code_actions(&mut self) -> Task<Message> {
        let Some(doc) = self.active_doc() else {
            return self.notify_error("Nessun documento per le azioni rapide".to_owned());
        };
        if self.pending_restored_tabs.contains(doc) {
            return self.notify(
                NoticeKind::Info,
                "Documento in caricamento: riprova al termine".to_owned(),
            );
        }
        if self
            .lsp
            .holding(doc)
            .is_none_or(|lsp| !lsp.can_code_action())
        {
            return self
                .notify_error("Azioni rapide non disponibili per questo documento".to_owned());
        }
        self.lsp_flush_change(doc, None);
        let Some(version) = self.workspace.get(doc).map(lume_core::Document::version) else {
            return self.notify_error("Documento non più disponibile".to_owned());
        };
        let Some(lsp) = self.lsp.holding_mut(doc) else {
            return self.notify_error("Language server non più disponibile".to_owned());
        };
        if !lsp.is_synced(doc, version) {
            return self.notify_error("Testo non ancora inviato al server: riprova".to_owned());
        }
        let Some(uri) = lsp.opened.get(&doc).cloned() else {
            return self.notify_error("Documento non aperto dal language server".to_owned());
        };
        let selezione = self
            .workspace
            .get(doc)
            .map(|documento| documento.selection())
            .unwrap_or_default();
        let range = if selezione.is_empty() {
            let fine = self
                .workspace
                .get(doc)
                .and_then(|documento| {
                    crate::lsp_pos::position_of(documento.rope(), documento.rope().len_chars())
                })
                .unwrap_or(lsp_types::Position {
                    line: 0,
                    character: 0,
                });
            lsp_types::Range {
                start: lsp_types::Position {
                    line: 0,
                    character: 0,
                },
                end: fine,
            }
        } else {
            let Some(documento) = self.workspace.get(doc) else {
                return self.notify_error("Documento non più disponibile".to_owned());
            };
            let inizio = match crate::lsp_pos::position_of(documento.rope(), selezione.min()) {
                Some(p) => p,
                None => return self.notify_error("Selezione non valida".to_owned()),
            };
            let fine = match crate::lsp_pos::position_of(documento.rope(), selezione.max()) {
                Some(p) => p,
                None => return self.notify_error("Selezione non valida".to_owned()),
            };
            lsp_types::Range {
                start: inizio,
                end: fine,
            }
        };
        let request = lsp.ask(doc, version);
        if !lsp.send(lume_lsp::LspCommand::Ask {
            id: request,
            request: lume_lsp::LspRequest::CodeAction { uri, range },
        }) {
            lsp.answered(request);
            return self.notify_error("Richiesta di azioni rapide non inviata: riprova".to_owned());
        }
        self.notify(NoticeKind::Info, "Azioni rapide in corso…".to_owned())
    }

    /// Una voce scelta nell'elenco delle azioni rapide.
    pub(super) fn code_action_chosen(&mut self, indice: usize) -> Task<Message> {
        let Some(azione) = self.code_actions.get(indice).cloned() else {
            return Task::none();
        };
        // La scelta consuma il risultato, anche se la sua vista è stata sostituita.
        self.code_actions.clear();
        self.chiudi_overlay_se(OverlayId::CodeActions);
        match azione {
            lsp_types::CodeActionOrCommand::CodeAction(azione) => {
                if let Some(edit) = azione.edit {
                    return self.apply_workspace_edit_notified(&edit, None);
                }
                if let Some(comando) = azione.command {
                    return self.notify_error(format!(
                        "L'azione «{}» richiede un comando del server, non supportato in questo passo",
                        comando.title
                    ));
                }
                self.notify(
                    NoticeKind::Info,
                    "Nessuna modifica per questa azione".to_owned(),
                )
            }
            lsp_types::CodeActionOrCommand::Command(comando) => self.notify_error(format!(
                "L'azione «{}» richiede un comando del server, non supportato in questo passo",
                comando.title
            )),
        }
    }

    /// Applica una `WorkspaceEdit`: **tutto o niente**, e mai su disco.
    pub(super) fn apply_workspace_edit(
        &mut self,
        edit: &lsp_types::WorkspaceEdit,
    ) -> Task<Message> {
        self.apply_workspace_edit_notified(edit, None)
    }

    fn apply_workspace_edit_notified(
        &mut self,
        edit: &lsp_types::WorkspaceEdit,
        feedback: Option<String>,
    ) -> Task<Message> {
        #[cfg(test)]
        {
            let prepared = match self.prepare_workspace_edit(edit) {
                Ok(prepared) => prepared,
                Err(error) => return self.notify_error(error.to_string()),
            };
            self.finish_workspace_edit_notified(prepared, feedback)
        }

        #[cfg(not(test))]
        {
            let sources = match self.rename_open_sources(edit) {
                Ok(sources) => sources,
                Err(error) => return self.notify_error(error.to_string()),
            };
            let edit = edit.clone();
            let root_epoch = self.git_root_epoch;
            Task::perform(
                crate::rename::prepare_workspace_edit_async(edit, sources),
                move |result| Message::WorkspaceEditPrepared {
                    root_epoch,
                    feedback,
                    result: result.map(std::sync::Arc::new),
                },
            )
        }
    }

    #[cfg(not(test))]
    pub(super) fn apply_workspace_edit_prepared(
        &mut self,
        root_epoch: u64,
        feedback: Option<String>,
        result: Result<std::sync::Arc<crate::rename::PreparedEdit>, crate::rename::RenameError>,
    ) -> Task<Message> {
        if self.git_root_epoch != root_epoch {
            return self
                .notify_error("il progetto è cambiato durante la rinomina; riprova".to_owned());
        }
        let prepared = match result.and_then(|prepared| {
            std::sync::Arc::try_unwrap(prepared)
                .map_err(|_| crate::rename::RenameError::Worker("piano condiviso".to_owned()))
        }) {
            Ok(prepared) => prepared,
            Err(error) => return self.notify_error(error.to_string()),
        };
        self.finish_workspace_edit_notified(prepared, feedback)
    }

    fn finish_workspace_edit_notified(
        &mut self,
        prepared: crate::rename::PreparedEdit,
        feedback: Option<String>,
    ) -> Task<Message> {
        let occurrences = prepared.occurrences();
        match self.apply_prepared(prepared) {
            Ok(files) => self.notify(
                NoticeKind::Info,
                feedback.unwrap_or_else(|| {
                    format!("Rinominati {occurrences} riferimenti in {files} file — da salvare")
                }),
            ),
            Err(error) => self.notify_error(error.to_string()),
        }
    }

    #[cfg(test)]
    pub(super) fn prepare_workspace_edit(
        &self,
        edit: &lsp_types::WorkspaceEdit,
    ) -> Result<crate::rename::PreparedEdit, crate::rename::RenameError> {
        let sources = self.rename_open_sources(edit)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| crate::rename::RenameError::Worker(error.to_string()))?;
        runtime.block_on(crate::rename::prepare_workspace_edit_async(
            edit.clone(),
            sources,
        ))
    }

    pub(super) fn rename_open_sources(
        &self,
        edit: &lsp_types::WorkspaceEdit,
    ) -> Result<
        std::collections::BTreeMap<String, crate::rename::RenameSource>,
        crate::rename::RenameError,
    > {
        use crate::rename::{RenameError, rename_edits};
        let mut sources = std::collections::BTreeMap::new();
        for (uri, _) in rename_edits(edit)? {
            let path = crate::lsp_uri::uri_to_path(&uri)
                .ok_or_else(|| RenameError::NotAFile(uri.as_str().to_owned()))?;
            let Some(id) = self.workspace.get_by_path(&path) else {
                continue;
            };
            let document = self
                .workspace
                .get(id)
                .ok_or_else(|| RenameError::Stale(path.clone()))?;
            let version = document.version();
            let synced = self
                .lsp
                .holding(id)
                .is_some_and(|lsp| lsp.is_synced(id, version));
            sources.insert(
                uri.as_str().to_owned(),
                crate::rename::RenameSource {
                    path,
                    text: document.rope().clone(),
                    document: Some(id),
                    version: Some(version),
                    fingerprint: self.disk_fingerprints.get(&id).copied(),
                    synced,
                },
            );
        }
        Ok(sources)
    }

    /// È tornata una risposta. Vale ancora?
    pub(super) fn lsp_answered(
        &mut self,
        server: &str,
        richiesta: u64,
        esito: Result<lume_lsp::LspAnswer, String>,
    ) -> Task<Message> {
        // una domanda che non abbiamo fatto noi — o che abbiamo già ritirata
        // — non ha destinatario
        let Some(attesa) = self
            .lsp
            .get_mut(server)
            .and_then(|lsp| lsp.answered(richiesta))
        else {
            return Task::none();
        };
        #[cfg(feature = "mcp")]
        if let Some(mcp_id) = self.mcp_lsp_claim(server, richiesta) {
            return self.mcp_lsp_answered(mcp_id, attesa, esito);
        }
        if self
            .definition_wait
            .as_ref()
            .is_some_and(|(wait_server, wait_request, _)| {
                wait_server == server && *wait_request == richiesta
            })
        {
            self.definition_wait = None;
        }
        let references_waiting =
            self.usages_wait
                .as_ref()
                .is_some_and(|(wait_server, wait_request, _)| {
                    wait_server == server && *wait_request == richiesta
                });
        if references_waiting {
            self.usages_wait = None;
        }
        let risposta = match esito {
            Ok(risposta) => risposta,
            Err(motivo) => {
                if attesa.quiet {
                    if self
                        .completion_wait
                        .as_ref()
                        .is_some_and(|wait| wait.server == server && wait.request == richiesta)
                    {
                        self.completion_wait = None;
                    }
                    if self
                        .hover_wait
                        .as_ref()
                        .is_some_and(|wait| wait.server == server && wait.request == richiesta)
                    {
                        self.hover_wait = None;
                    }
                    return Task::none();
                }
                if references_waiting {
                    self.usages.status = format!("Ricerca fallita: {motivo}");
                    self.sync_usages_tool();
                }
                return self.notify_error(format!("Language server: {motivo}"));
            }
        };
        // ATTENZIONE: Il testo può essere cambiato mentre il server pensava, e una
        // posizione riferita a una revisione superata non è «quasi giusta»:
        // è sbagliata, e si scopre solo dopo essere atterrati altrove. Si
        // guarda la revisione del documento, non l'orologio.
        let superata = self
            .workspace
            .get(attesa.doc)
            .map(lume_core::Document::version)
            != Some(attesa.version);
        match risposta {
            lume_lsp::LspAnswer::Definition(risposta) => {
                if superata {
                    return self.notify(
                        NoticeKind::Info,
                        "Il testo è cambiato mentre il server rispondeva".to_owned(),
                    );
                }
                self.apri_definizione(risposta, (attesa.doc, attesa.version))
            }
            lume_lsp::LspAnswer::PrepareRename(risposta) => {
                if superata {
                    return Task::none();
                }
                let Some(documento) = self.workspace.get(attesa.doc) else {
                    return Task::none();
                };
                let parola = Self::parola_attorno(documento, documento.selection().head);
                let posizione =
                    crate::lsp_pos::position_of(documento.rope(), documento.selection().head);
                match crate::rename::rename_start(risposta, documento.rope()) {
                    crate::rename::RenameStart::Impossibile => self.notify(
                        NoticeKind::Info,
                        "Qui non c'è niente da rinominare".to_owned(),
                    ),
                    crate::rename::RenameStart::Suggerito(nome) => {
                        let Some(posizione) = posizione else {
                            return Task::none();
                        };
                        self.open_rename_box(attesa.doc, posizione, attesa.version, nome)
                    }
                    crate::rename::RenameStart::Predefinito => {
                        let Some(posizione) = posizione else {
                            return Task::none();
                        };
                        self.open_rename_box(attesa.doc, posizione, attesa.version, parola)
                    }
                }
            }
            lume_lsp::LspAnswer::Formatting { uri, edits } => {
                let current_uri = self
                    .workspace
                    .get(attesa.doc)
                    .and_then(|doc| doc.path())
                    .and_then(crate::lsp_uri::path_to_uri);
                if superata
                    || current_uri.as_ref() != Some(&uri)
                    || self
                        .lsp
                        .holding(attesa.doc)
                        .and_then(|lsp| lsp.opened.get(&attesa.doc))
                        != Some(&uri)
                {
                    return self.notify(
                        NoticeKind::Info,
                        "Formattazione scartata: il documento è cambiato, riprova".to_owned(),
                    );
                }
                let Some(edits) = edits.filter(|edits| !edits.is_empty()) else {
                    return self.notify(NoticeKind::Info, "Documento già formattato".to_owned());
                };
                let edit = lsp_types::WorkspaceEdit {
                    changes: Some(std::collections::HashMap::from([(uri, edits)])),
                    ..lsp_types::WorkspaceEdit::default()
                };
                self.apply_workspace_edit_notified(
                    &edit,
                    Some("Documento formattato — da salvare".to_owned()),
                )
            }
            lume_lsp::LspAnswer::CodeAction(azioni) => {
                if superata {
                    return self.notify(
                        NoticeKind::Info,
                        "Azioni rapide scartate: il documento è cambiato, riprova".to_owned(),
                    );
                }
                let correnti = azioni.unwrap_or_default();
                if correnti.is_empty() {
                    // Il risultato autenticato invalida il contenuto anche se
                    // nel frattempo la vista è stata sostituita.
                    self.code_actions.clear();
                    self.chiudi_overlay_se(OverlayId::CodeActions);
                    return self.notify(NoticeKind::Info, "Nessuna azione disponibile".to_owned());
                }
                self.code_actions = correnti;
                self.apri_overlay(OverlayId::CodeActions);
                Task::none()
            }
            lume_lsp::LspAnswer::Rename(edit) => {
                #[cfg(feature = "mcp")]
                if attesa.quiet {
                    // Se è arrivata fin qui, il destinatario MCP non esiste
                    // più: una risposta silenziosa non può rinominare UI.
                    return Task::none();
                }
                self.close_rename();
                if superata {
                    return self.notify(
                        NoticeKind::Info,
                        "Il testo è cambiato mentre il server rispondeva".to_owned(),
                    );
                }
                let Some(edit) = edit else {
                    return self.notify(NoticeKind::Info, "Niente da rinominare".to_owned());
                };
                self.apply_workspace_edit(&edit)
            }
            lume_lsp::LspAnswer::References(locations) => {
                if superata {
                    self.usages.status = "Ricerca annullata: il testo è cambiato.".to_owned();
                    self.sync_usages_tool();
                    return self.notify(
                        NoticeKind::Info,
                        "Il testo è cambiato mentre il server rispondeva".to_owned(),
                    );
                }
                let groups =
                    crate::lsp_mode::references_groups(locations, self.project_root.as_deref())
                        .into_iter()
                        .map(|gruppo| tools::usages::Group {
                            label: gruppo.label,
                            path: gruppo.path,
                            hits: gruppo.hits,
                        })
                        .collect::<Vec<_>>();
                self.usages.status = if groups.is_empty() {
                    if self.usages.symbol.is_empty() {
                        "Nessun utilizzo qui.".to_owned()
                    } else {
                        format!("Nessun utilizzo di «{}».", self.usages.symbol)
                    }
                } else {
                    String::new()
                };
                self.usages.groups = groups;
                self.sync_usages_tool();
                Task::none()
            }
            // ATTENZIONE: Politica diversa, e di proposito: la struttura è un
            // aggiornamento **di sfondo**, che l'utente non ha chiesto. Se è
            // superata si tace e il giro dopo ne chiede una sul testo nuovo;
            // notificarlo sarebbe rumore su una cosa che nessuno aspettava.
            lume_lsp::LspAnswer::Symbols(risposta) => {
                if !superata {
                    let rows = crate::lsp_mode::outline_rows(risposta);
                    if let Some(lsp) = self.lsp.get_mut(server) {
                        lsp.store_outline(
                            attesa.doc,
                            crate::lsp_mode::Outline {
                                version: attesa.version,
                                rows,
                            },
                        );
                    }
                    self.sync_structure_tool();
                }
                Task::none()
            }
            lume_lsp::LspAnswer::Completion(risposta) => {
                self.completion_answered(server, richiesta, attesa.doc, risposta);
                Task::none()
            }
            // Cablato nel Task 13.
            lume_lsp::LspAnswer::Hover(risposta) => {
                self.hover_answered(server, richiesta, attesa.doc, risposta);
                Task::none()
            }
            lume_lsp::LspAnswer::SignatureHelp(risposta) => {
                self.signature_help_answered(server, richiesta, attesa.doc, risposta);
                Task::none()
            }
        }
    }

    /// Chiede la struttura dei documenti che qualcuno sta guardando.
    ///
    /// «Qualcuno» sono al massimo **due**: l'attivo di ciascun gruppo. Il
    /// breadcrumb li mostra sempre, il pannello Struttura mostra quello del gruppo col fuoco.
    /// Chiederla per tutte le schede aperte sarebbe lavoro d'indice per
    /// righe che nessuno legge — esattamente quel che questo progetto serve
    /// a togliere.
    pub(super) fn sync_outlines(&mut self) {
        use crate::editor_groups::GroupId;
        let attivi: Vec<DocId> = [GroupId::Primary, GroupId::Secondary]
            .into_iter()
            .filter_map(|group| self.groups.active_in(group))
            .collect();
        for id in attivi {
            let Some(version) = self.workspace.get(id).map(lume_core::Document::version) else {
                continue;
            };
            let Some(uri) = self
                .lsp
                .holding(id)
                .filter(|lsp| lsp.wants_outline(id, version))
                .and_then(|lsp| lsp.opened.get(&id).cloned())
            else {
                continue;
            };
            let Some(lsp) = self.lsp.holding_mut(id) else {
                continue;
            };
            let richiesta = lsp.ask(id, version);
            if lsp.send(lume_lsp::LspCommand::Ask {
                id: richiesta,
                request: lume_lsp::LspRequest::DocumentSymbols { uri },
            }) {
                lsp.mark_outline_asked(id, version);
            } else {
                lsp.answered(richiesta);
            }
        }
    }

    /// La fotografia per il pannello Struttura, **solo da aperto**: come per
    /// Estensioni, una `Vec` di righe per frame di un pannello che nessuno
    /// guarda è lo spreco che questo lavoro toglie.
    pub(crate) fn sync_structure_tool(&mut self) -> bool {
        if self.registry.active_panel != Some(tools::structure::ID) {
            return false;
        }
        let snapshot = self.structure_snapshot();
        if let Some(tool) = self.registry.get_mut(tools::structure::ID) {
            tool.update(&ToolEvent::new(tools::structure::Event::Replace(snapshot)));
        }
        true
    }

    /// La scheda «Simboli» di Cerca ovunque: stessa fotografia di Struttura,
    /// un'altra vetrina sullo stesso outline già in cache. Chiamare solo
    /// quando la ricerca è aperta o sta per aprirsi — a differenza di
    /// `sync_structure_tool` non c'è una guardia sul pannello attivo, perché
    /// chi chiama (`Message::GlobalSearch`) lo sa già.
    pub(super) fn sync_search_symbols(&mut self) {
        let snapshot = self.structure_snapshot();
        self.global_search.set_symbol_results(&snapshot);
    }

    pub(super) fn structure_snapshot(&self) -> tools::structure::Snapshot {
        let mut snapshot = tools::structure::Snapshot::default();
        // #10: il documento del gruppo col fuoco, lo stesso su cui agiscono
        // Ctrl+S e la barra di stato. `sync_outlines` chiede già la struttura
        // per l'attivo di entrambi i gruppi.
        let Some(id) = self.groups.active() else {
            snapshot.empty_reason = "Nessun file aperto.";
            return snapshot;
        };
        let Some(documento) = self.workspace.get(id) else {
            return snapshot;
        };
        snapshot.path = documento.path().map(std::path::Path::to_path_buf);
        let Some(lsp) = self.lsp.holding(id).or_else(|| self.lsp_focus()) else {
            snapshot.empty_reason = "Language server non avviato.";
            return snapshot;
        };
        if !lsp.availability.document_symbols {
            snapshot.empty_reason = if matches!(lsp.state, crate::lsp_mode::LspState::Attivo { .. })
            {
                "Questo server non dichiara la struttura dei documenti."
            } else {
                "Language server non avviato."
            };
            return snapshot;
        }
        let Some(outline) = lsp.outline_of(id, documento.version()) else {
            snapshot.empty_reason = "Struttura in arrivo…";
            return snapshot;
        };
        snapshot.rows = outline
            .rows
            .iter()
            .map(|riga| tools::structure::Row {
                name: riga.name.clone(),
                detail: riga.detail.clone(),
                kind: crate::lsp_mode::kind_label(riga.kind),
                depth: riga.depth,
                // `OpenFileAt` conta le righe da 1, LSP da 0
                line: riga.selection.start.line as usize + 1,
            })
            .collect();
        snapshot
    }

    /// Le briciole di un gruppo: percorso più simboli che contengono il
    /// cursore.
    pub(crate) fn crumbs_of(&self, group: crate::editor_groups::GroupId) -> Vec<String> {
        let Some(id) = self.groups.active_in(group) else {
            return Vec::new();
        };
        let Some(documento) = self.workspace.get(id) else {
            return Vec::new();
        };
        let simboli = self
            .lsp
            .holding(id)
            .and_then(|lsp| lsp.outline_of(id, documento.version()))
            .and_then(|outline| {
                let position =
                    crate::lsp_pos::position_of(documento.rope(), documento.selection().head)?;
                Some(
                    crate::lsp_mode::symbol_trail(&outline.rows, position)
                        .into_iter()
                        .filter_map(|indice| outline.rows.get(indice))
                        .map(|riga| riga.name.clone())
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default();
        crate::breadcrumb::crumbs_for(self.project_root.as_deref(), documento.path(), &simboli)
    }

    pub(super) fn apri_definizione(
        &mut self,
        risposta: Option<lsp_types::GotoDefinitionResponse>,
        origine: (DocId, u64),
    ) -> Task<Message> {
        let mete = crate::lsp_mode::definition_targets(risposta);
        let Some((uri, range)) = mete.first().cloned() else {
            // «non lo so» è una risposta legittima, non un guasto
            return self.notify(NoticeKind::Info, "Nessuna definizione trovata".to_owned());
        };
        let Some(path) = crate::lsp_uri::uri_to_path(&uri) else {
            return self.notify_error(format!("Percorso non riconosciuto: {}", uri.as_str()));
        };
        let apertura = self.jump_to_task(&path, range.start, origine);
        if mete.len() > 1 {
            // l'elenco è il passo 4; qui si apre la prima e si dice che ce
            // n'erano altre, invece di far finta che fosse l'unica
            return Task::batch([
                apertura,
                self.notify(
                    NoticeKind::Info,
                    format!("{} definizioni: aperta la prima", mete.len()),
                ),
            ]);
        }
        apertura
    }

    pub(super) fn goto_lsp_position_for(&mut self, id: DocId, position: lsp_types::Position) {
        if let Some(documento) = self.workspace.get_mut(id)
            && let Some(indice) = crate::lsp_pos::char_index_of(documento.rope(), position)
        {
            set_primary(documento, Range::point(indice));
        }
    }

    pub(super) fn sync_problems_tool(&mut self) {
        let problems =
            lume_lsp::DiagnosticStore::merged_snapshot(self.lsp.iter().map(|lsp| &lsp.problems));
        if let Some(tool) = self.registry.get_mut(tools::problems::ID) {
            tool.update(&tools::problems::replace_event(problems));
        }
        self.refresh_all_marks();
    }
}
