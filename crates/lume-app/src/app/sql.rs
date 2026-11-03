//! SQL: connessioni, console, query, vault dei segreti e ibernazione.
//!
//! Metodi di `Lume`: lo stato e i messaggi restano in `app.rs`.

use super::*;

impl Lume {
    #[cfg(feature = "sql")]
    pub(super) fn sql_console_snapshot(&self) -> Option<lume_store::SqlConsoleSnapshot> {
        let workspace = self.sql.as_ref()?;
        let text = self
            .sql_console
            .and_then(|id| self.workspace.get(id))
            .map(|document| document.rope().to_string())
            .unwrap_or_default();
        Some(crate::sql_snapshot::build_snapshot(workspace, &text))
    }

    /// La modalità SQL è in uso. **Due sorgenti, un solo predicato**: la
    /// console è una tab dell'editor, l'albero Database è un pannello
    /// laterale, e si chiudono da due strade diverse. Basta che uno dei due
    /// ci sia perché la modalità conti come in uso.
    ///
    /// Volutamente `tabs.contains` e non «è la tab attiva»: passare un minuto
    /// su `main.rs` non deve buttare giù la connessione e farla ripagare al
    /// ritorno. È la *chiusura* che conta, non il fuoco.
    #[cfg(feature = "sql")]
    pub(crate) fn sql_is_open(&self) -> bool {
        self.registry.is_open(tools::db::ID)
            || self.sql_console.is_some_and(|id| self.groups.contains(id))
    }

    /// Nome della tab per il documento della console (che non ha path).
    pub(crate) fn sql_console_name(&self, id: DocId) -> Option<&'static str> {
        #[cfg(feature = "sql")]
        {
            (self.sql_console == Some(id)).then_some(SQL_CONSOLE_NAME)
        }
        #[cfg(not(feature = "sql"))]
        {
            let _ = id;
            None
        }
    }

    pub(super) fn is_sql_console(&self, id: DocId) -> bool {
        #[cfg(feature = "sql")]
        {
            self.sql_console == Some(id)
        }
        #[cfg(not(feature = "sql"))]
        {
            let _ = id;
            false
        }
    }

    #[cfg(feature = "sql")]
    pub(crate) fn sql_workspace(&self) -> Option<&sql_mode::SqlWorkspace> {
        self.sql.as_ref()
    }

    #[cfg(feature = "sql")]
    pub(crate) fn sql_profile(&self, id: &str) -> Option<&lume_sql::ConnectionProfile> {
        self.sql_profiles.iter().find(|profile| profile.id == id)
    }

    #[cfg(feature = "sql")]
    pub(crate) fn sql_profiles(&self) -> &[lume_sql::ConnectionProfile] {
        &self.sql_profiles
    }

    /// Comandi della console. Nessuno di questi apre socket: quelli arrivano
    /// solo da `Run` e dall'espansione dell'albero, cioè da un'azione esplicita.
    #[cfg(feature = "sql")]
    pub(super) fn apply_sql_message(
        &mut self,
        message: crate::sql_console::SqlMessage,
    ) -> Task<Message> {
        use crate::sql_console::SqlMessage as M;
        // `Run` va servito **prima** del prestito qui sotto: è l'unico che
        // accoda lavoro, e per farlo gli serve `&mut self` intero.
        if message == M::Run {
            return self.run_sql_query();
        }
        if message == M::DiscardResult {
            if let Some(workspace) = self.sql.as_mut() {
                workspace.discard_running();
            }
            return Task::none();
        }
        if let M::ContinueResult(index) = message {
            if let Some(workspace) = self.sql.as_mut()
                && workspace.continue_query(index).is_ok()
                && let Some(pane) = workspace.pane_mut(index)
            {
                pane.status = sql_mode::PaneStatus::Running;
            }
            return Task::none();
        }
        if let M::StopResult(index) = message {
            if let Some(workspace) = self.sql.as_mut() {
                let _ = workspace.stop_query(index);
            }
            return Task::none();
        }
        let Some(workspace) = self.sql.as_mut() else {
            return Task::none();
        };
        match message {
            M::ToggleCompare => {
                workspace.compare = !workspace.compare;
            }
            M::ToggleSyncScroll => {
                workspace.sync_scroll = !workspace.sync_scroll;
            }
            M::SwapPanes => workspace.swap_panes(),
            M::SelectConnection(index, id) => {
                if let Some(pane) = workspace.pane_mut(index) {
                    pane.connection = Some(id);
                    // la griglia era di un'altra connessione: non ha più senso
                    pane.clear_results();
                    pane.status = sql_mode::PaneStatus::Idle;
                }
            }
            M::SelectResultSet(pane_index, result_index) => {
                if let Some(pane) = workspace.pane_mut(pane_index) {
                    let _ = pane.select_result_set(result_index);
                }
            }
            M::Sort(index, column) => {
                if let Some(pane) = workspace.pane_mut(index) {
                    let sort = match pane.sort.take() {
                        Some(previous) if previous.column == column => sql_mode::Sort {
                            column,
                            direction: previous.direction.reversed(),
                        },
                        _ => sql_mode::Sort {
                            column,
                            direction: sql_mode::SortDirection::Ascending,
                        },
                    };
                    pane.set_sort(sort);
                }
            }
            M::PreviousPage(index) => {
                if let Some(pane) = workspace.pane_mut(index) {
                    pane.set_page(pane.page.saturating_sub(1));
                }
            }
            M::NextPage(index) => {
                if let Some(pane) = workspace.pane_mut(index) {
                    let pages = pane
                        .results
                        .as_ref()
                        .map(|set| set.row_count().div_ceil(sql_mode::PAGE_SIZE).max(1))
                        .unwrap_or(1);
                    pane.set_page((pane.page + 1).min(pages.saturating_sub(1)));
                }
            }
            M::ToggleConnectionMenu(index) => {
                if let Some(pane) = workspace.pane_mut(index) {
                    pane.connection_menu_open = !pane.connection_menu_open;
                }
            }
            M::GridScrolled { pane, offset_y } => {
                workspace.grid_scroll_y = offset_y;
                if !workspace.compare || !workspace.sync_scroll {
                    return Task::none();
                }
                if workspace.scroll_echo == Some(pane) {
                    workspace.scroll_echo = None;
                    return Task::none();
                }
                let other = 1 - pane;
                workspace.scroll_echo = Some(other);
                use iced::widget::operation::{AbsoluteOffset, scroll_to};
                return scroll_to(
                    crate::sql_console::grid_scroll_id(other),
                    AbsoluteOffset {
                        x: Some(0.0),
                        y: Some(offset_y),
                    },
                );
            }
            // servito sopra, prima del prestito di `self.sql`
            M::Run | M::DiscardResult | M::ContinueResult(_) | M::StopResult(_) => {}
        }
        Task::none()
    }

    /// «Esegui»: manda il testo della console a **ogni riquadro attivo**, che
    /// in modalità confronto sono due e devono correre davvero in parallelo —
    /// per questo il worker è per connessione e non per workspace.
    #[cfg(feature = "sql")]
    pub(super) fn run_sql_query(&mut self) -> Task<Message> {
        let Some(sql) = self
            .sql_console
            .and_then(|id| self.workspace.get(id))
            .map(|document| document.rope().to_string())
        else {
            return Task::none();
        };
        if sql.trim().is_empty() {
            return Task::none();
        }
        let targets: Vec<(usize, Option<String>)> = match self.sql.as_ref() {
            Some(workspace) => workspace
                .active_panes()
                .iter()
                .enumerate()
                .map(|(index, pane)| (index, pane.connection.clone()))
                .collect(),
            // Nessun workspace: non ci sono nemmeno riquadri su cui scrivere
            // lo stato, quindi «Esegui» tacerebbe del tutto.
            None => {
                return self.notify_error(
                    "Nessuna connessione aperta: espandi un'origine dati nel pannello Database."
                        .to_owned(),
                );
            }
        };
        let mut tasks = Vec::new();
        for (index, connection) in targets {
            match connection {
                Some(id)
                    if self
                        .sql
                        .as_ref()
                        .is_some_and(|w| w.connection(&id).is_some()) =>
                {
                    tasks.push(self.sql_submit_query_stream(&id, sql.clone(), index));
                }
                // Riquadro senza connessione viva: lo dice la sua barra di
                // stato, non una notifica che coprirebbe l'altro riquadro.
                _ => {
                    if let Some(pane) = self.sql.as_mut().and_then(|w| w.pane_mut(index)) {
                        pane.status = sql_mode::PaneStatus::Error(
                            "Nessuna connessione aperta per questo riquadro.".to_owned(),
                        );
                    }
                }
            }
        }
        Task::batch(tasks)
    }

    /// Crea un profilo dal modulo del pannello e lo persiste. L'id si ricava
    /// dall'etichetta e resta stabile: risalvarlo aggiorna invece di
    /// duplicare.
    #[cfg(feature = "sql")]
    pub(super) fn create_sql_connection(
        &mut self,
        draft: &lume_ui::tool::SqlConnectionDraft,
    ) -> Task<Message> {
        let profile = lume_sql::ConnectionProfile {
            id: sql_profile_id(&draft.label, &self.sql_profiles),
            label: draft.label.trim().to_owned(),
            engine: if draft.oracle {
                lume_sql::Engine::Oracle
            } else {
                lume_sql::Engine::AzureSql
            },
            host: draft.host.trim().to_owned(),
            database: draft.database.trim().to_owned(),
            auth: if draft.service_principal {
                lume_sql::AuthKind::ServicePrincipal {
                    tenant_id: draft.tenant_id.trim().to_owned(),
                    client_id: draft.client_id.trim().to_owned(),
                }
            } else {
                lume_sql::AuthKind::SqlLogin {
                    user: draft.user.trim().to_owned(),
                }
            },
        };
        let record = sql_record_from_profile(&profile);
        self.sql_profiles.push(profile);
        self.sync_db_tool();
        self.persist_sql_connection(record)
    }

    #[cfg(feature = "sql")]
    pub(super) fn persist_sql_connection(
        &self,
        record: lume_store::SqlConnectionRecord,
    ) -> Task<Message> {
        let Some(store) = self.store.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                store
                    .save_sql_connection(record)
                    .await
                    .map_err(|error| error.to_string())
            },
            Message::SessionSaved,
        )
    }

    #[cfg(feature = "sql")]
    pub(super) fn delete_sql_connection(&mut self, index: usize) -> Task<Message> {
        if index >= self.sql_profiles.len() {
            return Task::none();
        }
        let profile = self.sql_profiles.remove(index);
        // Il socket va giù con il profilo: tenerlo vivo senza la riga che lo
        // rappresenta lo renderebbe irraggiungibile e ineliminabile. Non
        // basta togliere il profilo — la connessione vive nello
        // `SqlWorkspace` e resterebbe fino all'ibernazione.
        if let Some(workspace) = self.sql.as_mut() {
            workspace.close(&profile.id);
        }
        self.sql_expanded
            .retain(|key| key != &profile.id && !key.starts_with(&format!("{}:", profile.id)));
        if self.sql_secret_for.as_deref() == Some(profile.id.as_str()) {
            self.sql_secret_for = None;
        }
        if self
            .sql_pending_secret
            .as_ref()
            .is_some_and(|(id, _)| id == &profile.id)
        {
            self.sql_pending_secret = None;
        }
        self.sync_db_tool();
        // Il segreto se ne va con il profilo: lasciarlo nel magazzino del
        // sistema sarebbe una password orfana che nessuna interfaccia di Lume
        // può più né usare né cancellare.
        let mut tasks = vec![self.sql_forget_secret(&profile.id)];
        if let Some(store) = self.store.clone() {
            tasks.push(Task::perform(
                async move {
                    store
                        .delete_sql_connection(profile.id)
                        .await
                        .map_err(|error| error.to_string())
                },
                Message::SessionSaved,
            ));
        }
        Task::batch(tasks)
    }

    /// Accoda una richiesta al worker di una connessione e la trasforma in un
    /// `Message`. Non blocca: il `oneshot` diventa un `Task`.
    #[cfg(feature = "sql")]
    pub(super) fn sql_submit(
        &mut self,
        connection: &str,
        request: lume_sql::worker::Request,
        pane: Option<usize>,
    ) -> Task<Message> {
        let Some(workspace) = self.sql.as_ref() else {
            return Task::none();
        };
        match workspace.submit(connection, request) {
            Ok((receiver, generation)) => {
                if let Some(pane) = pane.and_then(|index| {
                    self.sql
                        .as_mut()
                        .and_then(|workspace| workspace.pane_mut(index))
                }) {
                    pane.query_gen = generation;
                    pane.status = sql_mode::PaneStatus::Running;
                }
                let connection = connection.to_owned();
                Task::perform(
                    async move {
                        // Il mittente è caduto: il worker si è chiuso senza
                        // rispondere, e la richiesta è comunque superata.
                        receiver
                            .await
                            .unwrap_or(Err(lume_sql::SqlError::Superseded))
                            .map_err(|error| error.to_string())
                    },
                    move |result| Message::SqlResponse {
                        connection: connection.clone(),
                        pane,
                        generation,
                        result,
                    },
                )
            }
            Err(error) => self.notify_error(error.to_string()),
        }
    }

    #[cfg(feature = "sql")]
    pub(super) fn sql_submit_query_stream(
        &mut self,
        connection: &str,
        sql: String,
        pane: usize,
    ) -> Task<Message> {
        let Some(workspace) = self.sql.as_ref() else {
            return Task::none();
        };
        match workspace.submit_query_events(connection, sql) {
            Ok((receiver, generation)) => {
                if let Some(target) = self
                    .sql
                    .as_mut()
                    .and_then(|workspace| workspace.pane_mut(pane))
                {
                    target.query_gen = generation;
                    target.status = sql_mode::PaneStatus::Running;
                    target.clear_results();
                    target.bind_stream(receiver.origin());
                }
                let connection = connection.to_owned();
                Task::run(crate::sql_events::deliveries(receiver), move |delivery| {
                    Message::SqlQueryEvent {
                        connection: connection.clone(),
                        pane,
                        generation,
                        delivery,
                    }
                })
            }
            Err(error) => self.notify_error(error.to_string()),
        }
    }

    /// Apre la connessione di un profilo e ne chiede subito le tabelle.
    /// Se manca il segreto, invece, lo chiede all'utente: è l'unico punto in
    /// cui una password entra in memoria.
    /// Che fare della cassaforte, dato com'è andata la connessione.
    ///
    /// `Task` è opaco: la decisione sta qui, dove si può asserire. La
    /// risposta alle tabelle è la prima prova che la connessione è viva; il
    /// fallimento **per autenticazione** è la prova che il segreto non vale
    /// più. Un host irraggiungibile non è né l'una né l'altra cosa, e
    /// buttare via il segreto lì costringerebbe a ridigitare una password
    /// giusta ogni volta che la rete fa i capricci.
    #[cfg(feature = "sql")]
    pub(super) fn sql_vault_action(&self, connection: &str, connected: bool) -> VaultAction {
        if connected {
            return VaultAction::Remember;
        }
        if self
            .sql
            .as_ref()
            .and_then(|workspace| workspace.connection(connection))
            .is_some_and(lume_sql::Connection::auth_failed)
        {
            VaultAction::Forget
        } else {
            VaultAction::Nothing
        }
    }

    /// Chiede il segreto alla cassaforte **fuori dal thread della UI**.
    ///
    /// Su Windows è una chiamata locale da microsecondi, ma su Linux la prima
    /// lettura può fermarsi sul dialogo di sblocco del portachiavi finché
    /// l'utente non digita: è un'attesa senza limite, come una `connect`, e
    /// non può stare né sul thread della UI né su un worker dell'executor.
    #[cfg(feature = "sql")]
    pub(super) fn sql_recall_secret(&self, id: &str) -> Task<Message> {
        let vault = self.sql_vault.clone();
        let connection = id.to_owned();
        let key = connection.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || vault.get(&key))
                    .await
                    .unwrap_or_else(|error| Err(lume_sql::VaultError::Failed(error.to_string())))
            },
            move |result| match result {
                Ok(secret) => Message::SqlSecretRecalled {
                    connection: connection.clone(),
                    secret: secret
                        .map(|secret| lume_ui::tool::Redacted::new(secret.expose().to_owned())),
                    error: None,
                },
                Err(error) => Message::SqlSecretRecalled {
                    connection: connection.clone(),
                    secret: None,
                    error: Some(error.to_string()),
                },
            },
        )
    }

    /// Mette in cassaforte il segreto rimasto in sospeso, se è di questa
    /// connessione. Si chiama **solo** quando la connessione ha risposto.
    #[cfg(feature = "sql")]
    pub(super) fn sql_remember_secret(&mut self, id: &str) -> Task<Message> {
        let Some((pending, secret)) = self.sql_pending_secret.take() else {
            return Task::none();
        };
        if pending != id {
            // Non è di questa connessione: torna dov'era, il suo esito deve
            // ancora arrivare.
            self.sql_pending_secret = Some((pending, secret));
            return Task::none();
        }
        let vault = self.sql_vault.clone();
        let key = pending;
        let stored = lume_sql::Secret::new(secret.expose().to_owned());
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || vault.set(&key, &stored))
                    .await
                    .unwrap_or_else(|error| Err(lume_sql::VaultError::Failed(error.to_string())))
            },
            |result| Message::SqlVaultWritten(result.map_err(|error| error.to_string())),
        )
    }

    /// Toglie dalla cassaforte un segreto che il server ha respinto. Senza
    /// questo una password ruotata renderebbe quel nodo inapribile per
    /// sempre: la cassaforte continuerebbe a ridare quella vecchia.
    #[cfg(feature = "sql")]
    pub(super) fn sql_forget_secret(&self, id: &str) -> Task<Message> {
        let vault = self.sql_vault.clone();
        let key = id.to_owned();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || vault.forget(&key))
                    .await
                    .unwrap_or_else(|error| Err(lume_sql::VaultError::Failed(error.to_string())))
            },
            |result| Message::SqlVaultWritten(result.map_err(|error| error.to_string())),
        )
    }

    #[cfg(feature = "sql")]
    pub(super) fn sql_connect(&mut self, id: &str, secret: lume_sql::Secret) -> Task<Message> {
        let Some(profile) = self.sql_profile(id).cloned() else {
            return Task::none();
        };
        let workspace = self.sql.get_or_insert_with(sql_mode::SqlWorkspace::new);
        // `open` è idempotente sull'id: senza buttare via il tentativo
        // fallito, riprovare non riconnetterebbe mai.
        if workspace
            .connection(id)
            .is_some_and(|c| c.state() == lume_sql::worker::State::Failed)
        {
            workspace.close(id);
        }
        if let Err(error) = workspace.open(profile, secret, self.sql_factory) {
            self.sync_db_tool();
            return self.notify_error(error.to_string());
        }
        self.reconcile_feature_visibility();
        self.sync_db_tool();
        self.sql_submit(id, lume_sql::worker::Request::Tables, None)
    }

    #[cfg(feature = "sql")]
    pub(super) fn apply_sql_response(
        &mut self,
        connection: &str,
        pane: Option<usize>,
        generation: u64,
        result: Result<lume_sql::worker::Response, String>,
    ) -> Task<Message> {
        use lume_sql::worker::Response;
        // La risposta di un riquadro vale solo se è ancora quella attesa: nel
        // frattempo può essere partita un'altra query, o l'ibernazione può
        // aver liberato la griglia.
        //
        // `pane` e non `active_panes`: uscire da «Confronta» con una query in
        // volo su B non deve lasciare quel riquadro `Running` per sempre, che
        // è quel che si vedrebbe riattivando il confronto.
        if let Some(index) = pane
            && self
                .sql
                .as_ref()
                .and_then(|workspace| workspace.pane(index))
                .is_none_or(|pane| pane.query_gen != generation)
        {
            return Task::none();
        }
        let error = match result {
            Ok(Response::Tables(tables)) => {
                if let Some(workspace) = self.sql.as_mut() {
                    workspace.set_tables(connection, tables);
                }
                None
            }
            Ok(Response::Columns { table, columns }) => {
                if let Some(workspace) = self.sql.as_mut() {
                    workspace.set_columns(connection, table, columns);
                }
                None
            }
            Err(error) => Some(error),
        };
        // La cassaforte si tocca **qui**, dove si sa com'è andata davvero.
        let vault = match self.sql_vault_action(connection, error.is_none()) {
            VaultAction::Remember => self.sql_remember_secret(connection),
            VaultAction::Forget => {
                self.sql_pending_secret = None;
                // Il campo torna subito sotto la riga: costringere a
                // richiuderla e riaprirla sarebbe un rebus. Ma solo se la
                // riga è ancora aperta — l'esito arriva dal thread del
                // worker, e nel frattempo il nodo può essere stato compresso.
                if self.sql_expanded.contains(connection) {
                    self.sql_secret_for = Some(connection.to_owned());
                }
                self.sql_forget_secret(connection)
            }
            VaultAction::Nothing => Task::none(),
        };
        self.sync_db_tool();
        let reported = match error {
            None => Task::none(),
            // Un errore di query si legge nella barra di stato del riquadro,
            // dove sta la query che l'ha causato; uno di schema no, quel
            // riquadro non esiste.
            Some(error) => match pane.and_then(|index| {
                self.sql
                    .as_mut()
                    .and_then(|workspace| workspace.pane_mut(index))
            }) {
                Some(pane) => {
                    pane.status = sql_mode::PaneStatus::Error(error);
                    pane.query_gen = sql_mode::NO_QUERY;
                    Task::none()
                }
                None => self.notify_error(error),
            },
        };
        Task::batch([vault, reported])
    }

    #[cfg(feature = "sql")]
    pub(super) fn apply_sql_query_event(
        &mut self,
        _connection: &str,
        index: usize,
        generation: u64,
        event: lume_sql::QueryEvent,
    ) -> Task<Message> {
        let Some(pane) = self
            .sql
            .as_mut()
            .and_then(|workspace| workspace.pane_mut(index))
        else {
            return Task::none();
        };
        if pane.query_gen != generation {
            return Task::none();
        }
        match event {
            lume_sql::QueryEvent::Started { columns } => pane.begin_stream(columns),
            lume_sql::QueryEvent::ResultSetStarted { index, columns } => {
                pane.begin_result_set(index, columns)
            }
            lume_sql::QueryEvent::Batch(batch) => pane.append_batch(batch),
            lume_sql::QueryEvent::BudgetReached {
                rows: _,
                accounted_bytes: _,
            } => {
                // Il worker ha smesso di estrarre: le righe già visibili sono
                // vere e il loro stato è esplicitamente incompleto. I comandi
                // Continua/Ferma verranno esposti solo con il protocollo di
                // pausa del cursore, non come pulsanti senza consumer.
                pane.incomplete = true;
                pane.status = sql_mode::PaneStatus::Paused;
            }
            lume_sql::QueryEvent::Completed { elapsed } => {
                pane.set_results_elapsed(elapsed);
                pane.status = sql_mode::PaneStatus::Ok;
                pane.query_gen = sql_mode::NO_QUERY;
            }
            lume_sql::QueryEvent::Stopped { .. } => {
                pane.incomplete = true;
                pane.status = sql_mode::PaneStatus::Ok;
                pane.query_gen = sql_mode::NO_QUERY;
            }
            lume_sql::QueryEvent::Failed(error) => {
                pane.incomplete = true;
                pane.status = sql_mode::PaneStatus::Error(error);
                pane.query_gen = sql_mode::NO_QUERY;
            }
        }
        if pane.query_gen == sql_mode::NO_QUERY {
            pane.finish_stream();
        }
        Task::none()
    }

    /// Libera il Database: è il percorso comune di «Ferma» e della scadenza
    /// del timer d'inattività (entrambi passano da `stop_feature`). Droppare
    /// lo `SqlWorkspace` chiude i socket e fa uscire i thread; il ramo del
    /// timer smette di essere prodotto e la subscription sparisce con lui.
    #[cfg(feature = "sql")]
    pub(super) fn hibernate_sql(&mut self) {
        self.sql = None;
        self.reconcile_feature_visibility();
        self.sync_db_tool();
    }

    /// Ricompone l'albero del pannello Database dai profili (che esistono
    /// sempre) più lo schema della connessione, se è viva. Da freddo si
    /// vedono le sole righe di connessione: espanderne una riconnette.
    #[cfg(feature = "sql")]
    pub(super) fn sql_tree(&self) -> Vec<tools::db::ConnectionView> {
        use tools::db::{ColumnView, ConnectionState, ConnectionView, TableView};
        self.sql_profiles
            .iter()
            .map(|profile| {
                let state = match self.sql.as_ref().and_then(|w| w.connection(&profile.id)) {
                    None => ConnectionState::Disconnected,
                    Some(connection) => match connection.state() {
                        lume_sql::State::Connecting => ConnectionState::Connecting,
                        lume_sql::State::Connected => ConnectionState::Connected,
                        lume_sql::State::Failed => ConnectionState::Error(
                            connection.error().unwrap_or_else(|| "errore".into()),
                        ),
                    },
                };
                let schema = self.sql.as_ref().and_then(|w| w.schema_for(&profile.id));
                let tables = schema
                    .map(|schema| {
                        schema
                            .tables
                            .iter()
                            .map(|name| {
                                let key = format!("{}:{name}", profile.id);
                                TableView {
                                    name: name.clone(),
                                    expanded: self.sql_expanded.contains(&key),
                                    columns: schema
                                        .columns
                                        .get(name)
                                        .map(|columns| {
                                            columns
                                                .iter()
                                                .map(|column| ColumnView {
                                                    name: column.name.clone(),
                                                    type_name: column.type_name.clone(),
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                ConnectionView {
                    id: profile.id.clone(),
                    label: profile.label.clone(),
                    engine: profile.engine.dialect().to_owned(),
                    host: profile.host.clone(),
                    expanded: self.sql_expanded.contains(&profile.id),
                    state,
                    tables,
                    asking_secret: self.sql_secret_for.as_deref() == Some(profile.id.as_str()),
                    secret_label: match &profile.auth {
                        lume_sql::AuthKind::SqlLogin { user } => format!("password di {user}"),
                        lume_sql::AuthKind::ServicePrincipal { client_id, .. } => {
                            format!("client secret di {client_id}")
                        }
                    },
                    cleartext: lume_sql::oracle::is_cleartext(profile),
                }
            })
            .collect()
    }

    #[cfg(feature = "sql")]
    pub(super) fn sync_db_tool(&mut self) {
        let tree = self.sql_tree();
        if let Some(tool) = self.registry.get_mut(tools::db::ID) {
            tool.update(&ToolEvent::new(tools::db::Event::Replace(tree)));
        }
    }

    /// Apre (o riattiva) la tab `console.sql`. È un documento **senza path**:
    /// `Workspace::open(None, …)` esiste già ed è testato, e così la console
    /// eredita barra tab, rope, undo ed evidenziazione SQL senza codice nuovo.
    #[cfg(feature = "sql")]
    pub(super) fn open_sql_console(&mut self, connection: Option<String>, table: Option<String>) {
        let seed = match (&connection, &table) {
            (Some(id), Some(table)) => self
                .sql_profiles
                .iter()
                .find(|profile| &profile.id == id)
                .map(|profile| profile.engine.select_top(table, 500))
                .unwrap_or_default(),
            _ => String::new(),
        };
        // il riuso si decide sul **documento**, non sulla tab: la tab puo'
        // essere stata chiusa e il testo essere ancora li'.
        let id = match self
            .sql_console
            .filter(|id| self.workspace.get(*id).is_some())
        {
            Some(id) => {
                if !seed.is_empty()
                    && let Some(document) = self.workspace.get_mut(id)
                {
                    document.reload_from_str(&seed);
                }
                if !self.groups.contains(id) {
                    self.groups.open(crate::editor_groups::GroupId::Primary, id);
                }
                id
            }
            None => {
                let id = self.workspace.open(None, &seed);
                self.groups.open(crate::editor_groups::GroupId::Primary, id);
                self.sql_console = Some(id);
                id
            }
        };
        // l'evidenziazione va (ri)costruita anche sul ramo di riuso: il testo
        // e' appena cambiato sotto di lei
        self.syntax.insert(id, SyntaxLanguage(LanguageId::Sql));
        self.close_diff();
        // il documento non ha path: `active_path` lo deriva e restituisce
        // `None` da sé, senza che nessuno debba ricordarsene
        self.activate_editor_tab(id);
        if let Some(connection) = connection {
            let workspace = self.sql.get_or_insert_with(sql_mode::SqlWorkspace::new);
            if let Some(pane) = workspace.pane_mut(0) {
                pane.connection = Some(connection);
            }
        }
        self.reconcile_feature_visibility();
    }
}
