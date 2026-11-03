//! Coordinatore del ciclo di vita delle funzionalità.
//!
//! Visibilità, risorse, comandi e conferme; stato e subscription restano
//! posseduti da Lume. Le politiche pure restano in features.rs e idle.rs.

use super::*;

impl Lume {
    /// Una funzionalità ha almeno una vista in uso. Si guarda solo
    /// l'appartenenza al registro (il pannello o il dock aperto: una ricerca
    /// lineare fra i tool) e, per il Database, la tab della console
    /// (`groups.contains`, che scorre le schede). Non scorre i documenti
    /// aperti né chiede niente ai tool.
    ///
    /// Serve alla tabella d'inattività. Per chi non ha timer (`idle: None`)
    /// il valore non cambia nulla, ma resta vero per tutte: «in uso» significa
    /// la stessa cosa ovunque.
    pub(crate) fn feature_is_in_use(&self, feature: crate::features::Feature) -> bool {
        match feature {
            #[cfg(feature = "sql")]
            crate::features::Feature::Database => self.sql_is_open(),
            other => crate::features::spec(other)
                .tools
                .iter()
                .any(|&id| self.registry.is_open(id)),
        }
    }

    /// Il timer d'inattività di una funzionalità sta girando. `Subscription`
    /// è opaca e non si può interrogare: questo è il seam, ed è esattamente
    /// la condizione che pilota il ramo in `subscription()`. Esiste solo per
    /// i test, per questo è `cfg(test)`.
    #[cfg(all(test, feature = "sql"))]
    pub(super) fn idle_timer_active(&self, feature: crate::features::Feature) -> bool {
        self.idle.is_dormant(feature)
    }

    /// Allinea la tabella d'inattività alla visibilità corrente, per ogni
    /// funzionalità. Stessa forma di `refresh_stale_git`, stesso punto di
    /// chiamata: la coda di `update`. Idempotente — il fronte si riconosce
    /// dalla tabella, che lo restituisce una volta sola.
    pub(super) fn reconcile_feature_visibility(&mut self) {
        use crate::features::Feature;
        let now = std::time::Instant::now();
        for feature in Feature::ALL {
            // Senza timer la tabella non ha nulla da decidere (il timeout e'
            // statico, quindi non puo' esserci stato residuo): si evita
            // anche di calcolare i predicati.
            let Some(timeout) = crate::features::spec(feature).idle else {
                continue;
            };
            let live = self.feature_holds_resources(feature);
            let in_use = self.feature_is_in_use(feature);
            let _armed = self
                .idle
                .reconcile(feature, live, in_use, now, Some(timeout));
            // fronte aperto→chiuso, una volta sola: la parte grossa e
            // invisibile (la griglia dei risultati) si libera subito, il
            // socket resta fino alla scadenza
            #[cfg(feature = "sql")]
            if _armed.is_some()
                && feature == Feature::Database
                && let Some(workspace) = self.sql.as_mut()
            {
                workspace.release_results();
            }
        }
    }

    /// La funzionalità tiene qualcosa che vale la pena far scadere. Per il
    /// Database è l'esistenza dello `SqlWorkspace` (anche senza connessioni
    /// aperte tiene riquadri e risultati); per le altre è il conteggio reale
    /// di chi possiede le risorse.
    pub(super) fn feature_holds_resources(&self, feature: crate::features::Feature) -> bool {
        match feature {
            #[cfg(feature = "sql")]
            crate::features::Feature::Database => self.sql.is_some(),
            other => self.feature_live_count(other).unwrap_or(0) > 0,
        }
    }

    /// Il ponte dei risvegli PTY serve solo se c'è qualcuno da svegliare:
    /// dock aperto **e** almeno una sessione che tiene ancora il PTY. Non
    /// basta «shell viva» (`live_count`): il `Tick` che legge `Exited` la
    /// porta a zero, ma il lettore PTY può consegnare l'ultimo output dopo
    /// l'uscita, e senza ponte la vista resterebbe vecchia. Il ponte si chiude
    /// quando il tool rilascia il PTY (Arresta, chiusura della scheda). A dock
    /// chiuso le shell restano vive e i loro risvegli si accumulano in un solo
    /// permesso di `Notify`, consegnato alla riapertura.
    pub(super) fn terminal_needs_wakeups(&self) -> bool {
        self.registry.is_open(tools::terminal::ID)
            && self
                .registry
                .get(tools::terminal::ID)
                .is_some_and(lume_ui::tool::Tool::needs_wakeups)
    }

    /// Quante risorse vive tiene una funzionalità **adesso**.
    ///
    /// Si deriva da chi le possiede davvero invece di memorizzarla: uno stato
    /// duplicato andrebbe fuori sincrono al primo percorso che qualcuno
    /// dimentica di aggiornare, e qui il numero finisce sotto gli occhi
    /// dell'utente.
    pub(super) fn feature_live_count(&self, feature: crate::features::Feature) -> Option<usize> {
        use crate::features::Feature;
        match feature {
            // ATTENZIONE: Arm esplicito: il ramo generico sommerebbe i `live_count` dei
            // tool, e per Problemi quello è **il numero di problemi**. Un
            // server fermo con tre diagnostiche vecchie risulterebbe vivo.
            Feature::LanguageServer => Some(usize::from(self.lsp.any_live())),
            #[cfg(feature = "mcp")]
            Feature::Mcp => Some(usize::from(self.mcp.is_some())),
            #[cfg(feature = "sql")]
            Feature::Database => Some(
                self.sql
                    .as_ref()
                    .map_or(0, sql_mode::SqlWorkspace::connection_count),
            ),
            other => crate::features::spec(other)
                .tools
                .iter()
                .filter_map(|&id| {
                    self.registry
                        .get(id)
                        .and_then(lume_ui::tool::Tool::live_count)
                })
                .reduce(|a, b| a + b),
        }
    }

    pub(super) fn feature_is_enabled(&self, feature: crate::features::Feature) -> bool {
        !self
            .registry
            .disabled_features()
            .contains(crate::features::spec(feature).key)
    }

    /// Esegue un comando su una funzionalità. La **decisione** sta in
    /// `feature_action`, che è pura e testabile; qui c'è solo l'esecuzione.
    pub(super) fn apply_feature_command(
        &mut self,
        command: crate::features::FeatureCommand,
    ) -> Task<Message> {
        use crate::features::{FeatureAction, FeatureCommand};
        let command = crate::features::confirm_replays(self.feature_stop_pending, command);
        let feature = command.feature();
        let spec = crate::features::spec(feature);
        let live = self.feature_live_count(feature).unwrap_or(0) > 0;
        let action = crate::features::feature_action(
            self.feature_is_enabled(feature),
            live,
            spec.startable,
            command,
        );
        // Spegnere gli Appunti ne chiude le schede: con modifiche non salvate
        // si rifiuta prima di tutto, anche della conferma sui processi.
        if feature == crate::features::Feature::Notes
            && matches!(
                action,
                FeatureAction::Disable | FeatureAction::StopThenDisable
            )
            && self.appunti_sporchi()
        {
            return self.notify_error(Self::APPUNTI_SPORCHI.to_owned());
        }
        // Fermare può buttare via una build in corso: si chiede, e la
        // richiesta di conferma sostituisce il tasto finché non si risponde.
        // Stessa forma della conferma di «Ripristina» nel pannello Commit.
        if matches!(action, FeatureAction::Stop | FeatureAction::StopThenDisable)
            && self.feature_needs_confirmation(feature)
            && self.feature_stop_pending.is_none()
        {
            self.feature_stop_pending = Some(command);
            let _ = self.sync_extensions_tool();
            return Task::none();
        }
        self.feature_stop_pending = None;
        let task = match action {
            FeatureAction::Start => match feature {
                crate::features::Feature::LanguageServer => self.start_lsp(),
                #[cfg(feature = "mcp")]
                crate::features::Feature::Mcp => self.start_mcp(),
                _ => Task::none(),
            },
            FeatureAction::Stop => self.stop_feature(feature),
            FeatureAction::StopThenDisable => {
                let task = self.stop_feature(feature);
                self.set_feature_enabled(feature, false);
                Task::batch([task, self.persist_session()])
            }
            FeatureAction::Disable => {
                self.set_feature_enabled(feature, false);
                self.persist_session()
            }
            FeatureAction::Enable => {
                self.set_feature_enabled(feature, true);
                self.persist_session()
            }
            FeatureAction::Nothing => {
                // Un comando innocuo non deve lasciare in giro una conferma
                // aperta su un'altra funzionalità.
                let _ = FeatureCommand::Start(feature);
                Task::none()
            }
        };
        self.mcp_sync_tools();
        let _ = self.sync_extensions_tool();
        task
    }

    /// Fermare questa funzionalità butterebbe via un processo vivo?
    pub(super) fn feature_needs_confirmation(&self, feature: crate::features::Feature) -> bool {
        feature == crate::features::Feature::Terminal
            && self.feature_live_count(feature).unwrap_or(0) > 0
    }

    pub(super) fn stop_feature(&mut self, feature: crate::features::Feature) -> Task<Message> {
        use crate::features::Feature;
        match feature {
            // Il tasto salta la coda: fa adesso quel che il timeout
            // d'inattività farebbe fra cinque minuti.
            #[cfg(feature = "sql")]
            Feature::Database => {
                self.hibernate_sql();
                Task::none()
            }
            // L'app non guarda dentro `TerminalTool`: gli manda un evento
            // tipizzato. È la stessa strada di `sync_terminal_shell`, ed è il
            // motivo per cui la parete del `Registry` non è d'intralcio.
            Feature::Terminal => {
                if let Some(tool) = self.registry.get_mut(tools::terminal::ID) {
                    let requests = tool.update(&ToolEvent::new(tools::terminal::Event::StopAll));
                    let queued: Vec<_> = requests
                        .into_iter()
                        .map(|request| (tools::terminal::ID, request))
                        .collect();
                    self.pending_tool_requests.extend(queued);
                }
                self.drain_tool_requests()
            }
            // Due stati soli, Attivo e Fermo: **niente ibernazione**. Il costo
            // di un language server *è* il suo indice, e risvegliarlo vorrebbe
            // dire ri-indicizzare — un timeout renderebbe il fermo peggiore
            // del non fermare.
            Feature::LanguageServer => self.stop_lsp(),
            #[cfg(feature = "mcp")]
            Feature::Mcp => {
                self.mcp_shutdown_cards();
                self.mcp_lsp_cancel_all();
                self.mcp = None;
                self.notify(NoticeKind::Info, "Server MCP fermato".to_owned())
            }
            _ => Task::none(),
        }
    }

    pub(super) fn set_feature_enabled(&mut self, feature: crate::features::Feature, enabled: bool) {
        if feature == crate::features::Feature::Notes && !enabled {
            self.close_all_note_tabs();
            self.notes = None;
        }
        let key = crate::features::spec(feature).key;
        let requests = self.registry.set_feature_enabled(key, enabled);
        self.pending_tool_requests.extend(requests);
    }

    /// Il tasto «Avvia» (o «Scarica») ha davvero un verso? Solo `Niente`
    /// lo nasconde: già in volo, o già attivo.
    pub(super) fn feature_can_start(&self, feature: crate::features::Feature) -> bool {
        use crate::lsp_mode::{LspStartAction, lsp_start_action};
        match feature {
            crate::features::Feature::LanguageServer => {
                !matches!(lsp_start_action(self.lsp_active()), LspStartAction::Niente)
            }
            crate::features::Feature::Mcp => self.mcp_can_start(),
            _ => true,
        }
    }

    /// Il cronometro d'avvio, dove esiste. Solo il language server si
    /// **avvia**: per le altre il numero misurerebbe un'altra cosa.
    pub(super) fn feature_startup_ms(&self, feature: crate::features::Feature) -> Option<u32> {
        match feature {
            crate::features::Feature::LanguageServer => {
                self.lsp_active().and_then(|lsp| lsp.startup_ms)
            }
            _ => None,
        }
    }
}
