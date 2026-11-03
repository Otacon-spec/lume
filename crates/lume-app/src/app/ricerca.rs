//! Ricerca e navigazione: nel file, nel progetto, vai a riga, cronologia dei salti.
//!
//! Metodi di `Lume`: lo stato e i messaggi restano in `app.rs`.

use super::*;

impl Lume {
    /// Dove siamo adesso, se siamo da qualche parte.
    pub(crate) fn posizione_corrente(&self) -> Option<crate::navigazione::Posizione> {
        let id = self.groups.active()?;
        let documento = self.workspace.get(id)?;
        let path = documento.path()?;
        let rope = documento.rope();
        let riga = rope.char_to_line(documento.selection().head.min(rope.len_chars()));
        Some(crate::navigazione::Posizione::new(path.to_path_buf(), riga))
    }

    /// Registra un salto nella cronologia di navigazione.
    ///
    /// ATTENZIONE: Entra **anche la posizione di partenza**, e prima. Senza,
    /// «indietro» dopo il primo salto non avrebbe dove tornare: la pila
    /// conterrebbe solo l'arrivo. I doppioni non sono un problema — `push`
    /// scarta due volte di fila la stessa posizione.
    pub(super) fn record_jump(&mut self, verso: crate::navigazione::Posizione) {
        if let Some(da) = self.posizione_corrente() {
            self.navigazione.push(da);
        }
        self.navigazione.push(verso);
    }

    /// Ctrl+Alt+←/→: un passo nella cronologia.
    ///
    /// ATTENZIONE: Il salto **non si registra**: navigare dentro la cronologia non è
    /// navigare, e registrarlo la farebbe crescere a ogni passo indietro
    /// fino a rendere impossibile tornare davvero.
    pub(super) fn navigate(&mut self, delta: i32) -> Task<Message> {
        let meta = if delta < 0 {
            self.navigazione.indietro().cloned()
        } else {
            self.navigazione.avanti().cloned()
        };
        let Some(meta) = meta else {
            return Task::none();
        };
        self.open_navigation_target(meta)
    }

    /// Porta il caret all'inizio di una riga **0-based** del documento
    /// attivo. `goto_line` conta da 1: qui si conta come il testo.
    pub(super) fn goto_line_index(&mut self, riga: usize) {
        let Some(id) = self.groups.active() else {
            return;
        };
        self.goto_line_index_for(id, riga);
    }

    pub(super) fn goto_line_index_for(&mut self, id: DocId, riga: usize) {
        let Some(documento) = self.workspace.get_mut(id) else {
            return;
        };
        let rope = documento.rope();
        let riga = riga.min(rope.len_lines().saturating_sub(1));
        let posizione = rope.line_to_char(riga);
        set_primary(documento, Range::point(posizione));
    }

    pub(super) fn open_goto_line(&mut self) -> Task<Message> {
        let group = self.groups.focus();
        let Some(id) = self.groups.active_in(group) else {
            return Task::none();
        };
        if self.pending_restored_tabs.contains(id) {
            return self.defer_restored_action(id, DeferredRestoredAction::OpenGotoLine(group));
        }
        self.forget_restored_action(id, &DeferredRestoredAction::OpenGotoLine(group));
        self.open_goto_line_for(id, group)
    }

    pub(super) fn open_goto_line_for(
        &mut self,
        id: DocId,
        group: crate::editor_groups::GroupId,
    ) -> Task<Message> {
        if self.groups.focus() != group
            || self.groups.active_in(group) != Some(id)
            || self.pending_restored_tabs.contains(id)
            || self.workspace.get(id).is_none()
        {
            return Task::none();
        }
        self.goto_line = Some(String::new());
        self.menu_bar.close();
        self.apri_overlay(OverlayId::GotoLine);
        iced::widget::operation::focus(iced::widget::Id::new(crate::prompt::INPUT_ID))
    }

    pub(crate) fn goto_prompt(&self) -> crate::prompt::Prompt<'_> {
        let valore = self.goto_line.as_deref().unwrap_or_default();
        let righe = self
            .groups
            .active()
            .and_then(|id| self.workspace.get(id))
            .map(|documento| documento.rope().len_lines())
            .unwrap_or(0);
        crate::prompt::Prompt {
            title: "Vai a riga".to_owned(),
            placeholder: "numero di riga",
            value: valore,
            // il numero di righe è l'unica cosa che serve sapere per
            // rispondere alla domanda, e costa una lettura del rope
            hint: format!("{righe} righe"),
            ready: crate::prompt::riga_richiesta(valore).is_some(),
            confirm_label: "Vai",
        }
    }

    pub(super) fn apply_goto_message(
        &mut self,
        message: &crate::prompt::PromptMessage,
    ) -> Task<Message> {
        use crate::prompt::PromptMessage;
        match message {
            PromptMessage::Changed(testo) => {
                if self.goto_line.is_some() {
                    self.goto_line = Some(testo.clone());
                }
                Task::none()
            }
            PromptMessage::Cancel => {
                self.goto_line = None;
                self.chiudi_overlay_se(OverlayId::GotoLine);
                Task::none()
            }
            PromptMessage::Confirm => {
                let riga = self
                    .goto_line
                    .as_deref()
                    .and_then(crate::prompt::riga_richiesta);
                let Some(riga) = riga else {
                    return Task::none();
                };
                self.goto_line = None;
                self.chiudi_overlay_se(OverlayId::GotoLine);
                // saltare a una riga **è** navigare: si registra, o
                // «indietro» non riporterebbe dov'eri
                if let Some(path) = self
                    .groups
                    .active()
                    .and_then(|id| self.workspace.get(id))
                    .and_then(lume_core::Document::path)
                    .map(std::path::Path::to_path_buf)
                {
                    self.record_jump(crate::navigazione::Posizione::new(path, riga));
                }
                self.goto_line_index(riga);
                Task::none()
            }
        }
    }

    /// Ctrl+F (`replace_mode = false`) o Ctrl+R (`true`): apre il riquadro
    /// sul documento attivo. Precompila la query con la selezione corrente,
    /// se non è vuota — evita di dover ridigitare quel che si era già scelto.
    pub(super) fn open_find(&mut self, replace_mode: bool) -> Task<Message> {
        let Some(id) = self.active_doc() else {
            return Task::none();
        };
        let action = DeferredRestoredAction::OpenFind(replace_mode);
        if self.pending_restored_tabs.contains(id) {
            return self.defer_restored_action(id, action);
        }
        self.forget_restored_action(id, &action);
        self.open_find_for(id, replace_mode)
    }

    pub(super) fn open_find_for(&mut self, id: DocId, replace_mode: bool) -> Task<Message> {
        if self.active_doc() != Some(id) || self.pending_restored_tabs.contains(id) {
            return Task::none();
        }
        let Some(doc) = self.workspace.get(id) else {
            return Task::none();
        };
        let sel = doc.selection();
        let iniziale = if sel.is_empty() {
            String::new()
        } else {
            doc.rope().slice(sel.min()..sel.max()).to_string()
        };
        self.find = Some(crate::find_replace::FindState::new(
            id,
            iniziale,
            replace_mode,
            doc.rope(),
        ));
        self.menu_bar.close();
        self.apri_overlay(OverlayId::Find);
        iced::widget::operation::focus(iced::widget::Id::new(crate::find_replace::INPUT_ID))
    }

    pub(super) fn apply_find_message(
        &mut self,
        message: crate::find_replace::FindMessage,
    ) -> Task<Message> {
        use crate::find_replace::FindIntent;

        let Some(stato) = self.find.as_mut() else {
            return Task::none();
        };
        let id = stato.doc;
        let Some(rope) = self.workspace.get(id).map(lume_core::Document::rope) else {
            self.find = None;
            self.chiudi_overlay_se(OverlayId::Find);
            return Task::none();
        };
        let intento = stato.update(message, rope);
        match intento {
            FindIntent::None => Task::none(),
            FindIntent::Closed => {
                self.find = None;
                self.chiudi_overlay_se(OverlayId::Find);
                Task::none()
            }
            FindIntent::JumpTo(range) => {
                if let Some(doc) = self.workspace.get_mut(id) {
                    set_primary(doc, lume_core::Range::new(range.start, range.end));
                }
                Task::none()
            }
            FindIntent::ReplaceOne(range, testo) => {
                self.editor_replace_range(id, range.start, range.end, &testo);
                let da_posizione = range.start + testo.chars().count();
                if let Some(rope) = self.workspace.get(id).map(lume_core::Document::rope) {
                    let prossimo = self
                        .find
                        .as_mut()
                        .and_then(|stato| stato.recompute_dopo_sostituzione(rope, da_posizione));
                    if let Some(prossimo) = prossimo
                        && let Some(doc) = self.workspace.get_mut(id)
                    {
                        set_primary(doc, lume_core::Range::new(prossimo.start, prossimo.end));
                    }
                }
                self.lsp_change_task()
            }
            FindIntent::ReplaceAll {
                ranges,
                replacement,
                truncated,
            } => {
                let edits: Vec<(usize, usize, String)> = ranges
                    .into_iter()
                    .map(|r| (r.start, r.end, replacement.clone()))
                    .collect();
                if let Some(doc) = self.workspace.get_mut(id) {
                    let len = doc.rope().len_chars();
                    if let Some(changes) = lume_core::ChangeSet::replace_many(&edits, len) {
                        let edit = doc.apply(lume_core::Transaction::new(changes));
                        self.resync_syntax(id, edit);
                    }
                }
                self.refresh_find_matches(id);
                let lsp = self.lsp_change_task();
                if !truncated {
                    return lsp;
                }
                // #8: chi crede di aver sostituito tutto deve sapere che no.
                // Non si invita a ripetere: con una sostituzione che contiene
                // il testo cercato, ripetere sostituirebbe il già sostituito.
                let avviso = self.notify(
                    NoticeKind::Info,
                    format!(
                        "Sostituite le prime {} occorrenze: il file ne ha altre, che restano.",
                        lume_core::MAX_FIND_MATCHES
                    ),
                );
                Task::batch([lsp, avviso])
            }
        }
    }

    /// Dopo una sostituzione il testo è cambiato: i range dei match
    /// precedenti non sono più validi (posizioni shiftate). Si ricalcola,
    /// non si tenta di rimapparli a mano.
    pub(super) fn refresh_find_matches(&mut self, id: DocId) {
        let Some(rope) = self.workspace.get(id).map(lume_core::Document::rope) else {
            return;
        };
        if let Some(stato) = self.find.as_mut() {
            let _ = stato.update(
                crate::find_replace::FindMessage::QueryChanged(stato.options.needle.clone()),
                rope,
            );
        }
    }

    /// Porta il fuoco su una posizione precisa, aprendo il file se serve.
    ///
    /// ATTENZIONE: Un solo punto per tutti i «vai a», e non è pedanteria: qui si
    /// appoggia la cronologia di navigazione (indietro, avanti, recenti), e
    /// due percorsi diversi vorrebbero dire metà dei salti non registrati.
    pub(super) fn jump_to_task(
        &mut self,
        path: &Path,
        position: lsp_types::Position,
        origine: (DocId, u64),
    ) -> Task<Message> {
        self.record_jump(crate::navigazione::Posizione::new(
            path.to_path_buf(),
            position.line as usize,
        ));
        if let Some(id) = self.workspace.get_by_path(path) {
            if !self.groups.contains(id) {
                self.groups.open(self.groups.focus(), id);
            }
            self.activate_editor_tab(id);
            if self.pending_restored_tabs.contains(id) {
                return self
                    .defer_restored_action(id, DeferredRestoredAction::GotoLspPosition(position));
            }
            self.goto_lsp_position_for(id, position);
            return Task::none();
        }
        let Some(navigazione) = self.definition_epoch else {
            return self.notify(
                NoticeKind::Info,
                "Richieste di salto LSP esaurite: nessuna lettura avviata".into(),
            );
        };
        self.open_file_task(
            path.to_path_buf(),
            crate::file_read::DopoApertura::PosizioneLsp {
                riga: position.line,
                carattere: position.character,
                progetto: self.git_root_epoch,
                origine,
                navigazione,
            },
        )
    }

    #[cfg(test)]
    pub(super) fn jump_to(
        &mut self,
        path: &Path,
        position: lsp_types::Position,
    ) -> Result<(), String> {
        self.record_jump(crate::navigazione::Posizione::new(
            path.to_path_buf(),
            position.line as usize,
        ));
        self.open_file(path)
            .map_err(|error| format!("Apertura di {} fallita: {error}", path.display()))?;
        if let Some(id) = self.groups.active() {
            self.goto_lsp_position_for(id, position);
        }
        Ok(())
    }

    /// Apre l'overlay Cerca ovunque sulla scheda data, con autofocus.
    pub(super) fn open_search(&mut self, tab: SearchTab) -> Task<Message> {
        self.menu_bar.close();
        self.environment_picker
            .update(EnvironmentPickerMessage::Dismiss);
        self.settings_panel.update(SettingsMessage::Dismiss);
        self.global_search.update(GlobalSearchMessage::Open(tab));
        self.apri_overlay(OverlayId::Search);
        let focus = self.focus_global_search();
        let ricerca = if self.shell.mostra(OverlayId::Search) {
            let task = if self.global_search.file_query().is_some() {
                self.run_global_file_search()
            } else {
                Task::none()
            };
            self.sync_search_symbols();
            task
        } else {
            Task::none()
        };
        Task::batch([focus, ricerca])
    }

    pub(super) fn focus_global_search(&mut self) -> Task<Message> {
        self.menu_bar.close();
        self.environment_picker
            .update(EnvironmentPickerMessage::Dismiss);
        self.settings_panel.update(SettingsMessage::Dismiss);
        self.apri_overlay(OverlayId::Search);
        iced::widget::operation::focus(iced::widget::Id::new(search::INPUT_ID))
    }

    pub(super) fn run_global_file_search(&mut self) -> Task<Message> {
        use std::sync::atomic::Ordering;

        let generation = self.global_search_gen.clone();
        let my_generation = generation.fetch_add(1, Ordering::SeqCst) + 1;
        let Some(query) = self.global_search.file_query() else {
            self.global_search.cancel_file_search();
            return Task::none();
        };
        let Some(root) = self.project_root.clone() else {
            self.global_search.cancel_file_search();
            return Task::none();
        };

        self.global_search.begin_file_search(my_generation);
        self.ensure_file_palette_worker();
        let Some(worker) = self.global_file_search_worker.as_ref() else {
            let error = self
                .global_file_search_worker_error
                .clone()
                .unwrap_or_else(|| "stato worker non disponibile".into());
            self.global_search.finish_file_search(
                my_generation,
                Err(search::FileSearchError::WorkerUnavailable(error).to_string()),
            );
            return Task::none();
        };
        let receiver = match worker.search(root, query, generation, my_generation) {
            Ok(receiver) => receiver,
            Err(error) => {
                self.global_search
                    .finish_file_search(my_generation, Err(error.to_string()));
                return Task::none();
            }
        };
        Task::perform(
            async move {
                match receiver.await {
                    Ok(Ok(entries)) => Ok(entries),
                    Ok(Err(error)) => Err(error.to_string()),
                    Err(_) => Err("worker ricerca file terminato senza risultato".into()),
                }
            },
            move |results| Message::GlobalFileSearch(my_generation, results),
        )
    }

    pub(super) fn cancel_global_file_search(&mut self) {
        use std::sync::atomic::Ordering;

        self.global_search_gen.fetch_add(1, Ordering::SeqCst);
        self.global_search.cancel_file_search();
    }

    pub(super) fn suspend_global_file_search(&mut self) {
        use std::sync::atomic::Ordering;

        self.global_search_gen.fetch_add(1, Ordering::SeqCst);
        self.global_search.suspend_file_search();
    }

    /// Avvia la ricerca nel progetto con cancellazione a generazioni.
    /// I batch arrivano in streaming con un solo packet in volo, consumato
    /// una volta e ack dopo l'applicazione UI. Source std async del core,
    /// nessun reader/relay; raccolta del producer posseduta separatamente.
    /// Ricerca nei file solo a digitazione ferma: ogni battuta invalida il
    /// token precedente, il walk del progetto parte dopo 200 ms di quiete.
    pub(super) fn debounce_search(
        &mut self,
        options: lume_core::SearchOptions,
        area: lume_core::SearchArea,
    ) -> Task<Message> {
        self.search_debounce = self.search_debounce.wrapping_add(1);
        let token = self.search_debounce;
        Task::perform(
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(SEARCH_DEBOUNCE_MS)).await;
                (token, options, area)
            },
            |(token, options, area)| Message::SearchDebounced(token, Box::new(options), area),
        )
    }

    /// Revoca e notifica il consumer corrente, senza join UI; l'owner del
    /// collector resta nella registry anche se la data Task viene droppata.
    pub(super) fn cancel_search(&mut self) {
        use std::sync::atomic::Ordering;
        self.search_jobs.cancel();
        self.search_gen.fetch_add(1, Ordering::SeqCst);
        self.search_debounce = self.search_debounce.wrapping_add(1);
    }

    /// I percorsi dei documenti aperti, per l'ambito «Aperti».
    ///
    /// La console SQL non ha un percorso e resta fuori da sé: cercare dentro
    /// un buffer che non è un file darebbe una riga di risultato che non si
    /// può nemmeno riaprire.
    pub(super) fn percorsi_aperti(&self) -> Vec<PathBuf> {
        self.groups
            .all()
            .filter_map(|id| self.workspace.get(id))
            .filter_map(lume_core::Document::path)
            .map(std::path::Path::to_path_buf)
            .collect()
    }

    /// Su cosa gira un giro di ricerca, o `None` se non c'è niente su cui
    /// girarlo.
    ///
    /// ATTENZIONE: **«File aperti» non ha bisogno di una cartella**: cammina una lista,
    /// non un albero, e i documenti aperti esistono anche senza progetto —
    /// `Ctrl+O` su un file solo è un modo normale di usare Lume. È «Progetto»
    /// l'unico ambito che può restare senza soggetto.
    ///
    /// Sta fuori da `run_search` perché `Task` è opaco e questa decisione no.
    pub(super) fn search_scope(
        &self,
        area: lume_core::SearchArea,
    ) -> Option<lume_core::SearchScope> {
        match area {
            lume_core::SearchArea::Aperti => {
                Some(lume_core::SearchScope::Aperti(self.percorsi_aperti()))
            }
            lume_core::SearchArea::Sorgenti => self
                .project_root
                .clone()
                .map(lume_core::SearchScope::Sorgenti),
            lume_core::SearchArea::Progetto => self
                .project_root
                .clone()
                .map(lume_core::SearchScope::Albero),
        }
    }

    pub(super) fn run_search(
        &mut self,
        options: lume_core::SearchOptions,
        area: lume_core::SearchArea,
    ) -> Task<Message> {
        self.search_exit = None; // nuova ricerca esplicita revoca l'uscita differita
        let Some(scope) = self.search_scope(area) else {
            // ATTENZIONE: Un ritorno muto qui è un vicolo cieco: il pannello non cambia
            // di una riga e sembra che la ricerca stia girando per sempre. Lo
            // dice dove dice l'errore di regex — nel pannello, non in una
            // notifica.
            if let Some(tool) = self.registry.get_mut(tools::search_tool::ID) {
                tool.update(&ToolEvent::new(tools::search_tool::Event::SenzaAmbito));
            }
            return Task::none();
        };
        use std::sync::atomic::Ordering;
        let gen_arc = self.search_gen.clone();
        let my_gen = gen_arc.fetch_add(1, Ordering::SeqCst) + 1;
        let job = match self.search_jobs.reserve(scope, options, gen_arc, my_gen) {
            Ok(job) => job,
            Err(error) => {
                self.deliver_search_update(my_gen, lume_core::SearchUpdate::Error(error));
                return Task::none();
            }
        };
        let collector = job.clone();
        let ticket = job.id();
        Task::batch([
            Task::run(crate::search_events::deliveries(job), move |delivery| {
                Message::SearchUpdate(my_gen, delivery)
            }),
            Task::perform(async move { collector.collect().await }, move |result| {
                Message::SearchCollected(ticket, result)
            }),
        ])
    }

    /// Sostituisce nei file quel che la ricerca ha **già mostrato**.
    ///
    /// Due fasi, e la prima non scrive niente: si legge ogni file chiuso, si
    /// controlla ogni riga contro quella che la ricerca aveva letto, si
    /// costruisce ogni contenuto nuovo. Un solo no e non è successo niente.
    ///
    /// ATTENZIONE: **Prima il disco, poi la memoria.** Le modifiche ai documenti
    /// aperti non possono fallire; le scritture sì. Facendole per ultime, una
    /// scrittura andata male lascerebbe metà progetto sul disco **e** dei
    /// documenti cambiati in memoria — due stati parziali invece di uno. Così
    /// il parziale è uno solo, ed è quello che il messaggio d'errore elenca.
    ///
    /// A differenza della rinomina qui si scrive davvero: ogni file viene
    /// sostituito atomicamente, ma non esiste una transazione multi-file.
    /// Se un commit fallisce, si dichiara dove ci si è fermati.
    pub(super) fn replace_all(
        &mut self,
        matches: &[lume_core::FileMatches],
        replacement: &str,
    ) -> Task<Message> {
        use crate::replace_batch::{OpenInput, Phase, Report, State};
        if self.scritture.guarda(None).in_volo.is_some() || self.git_operazione.is_some() {
            return self.notify_error(
                "Ci sono scritture in corso: attendi prima di sostituire nei file".into(),
            );
        }
        let files: Vec<_> = matches
            .iter()
            .filter(|f| !f.hits.is_empty())
            .cloned()
            .collect();
        if let Some(id) = files
            .iter()
            .filter_map(|file| self.workspace.get_by_path(&file.path))
            .find(|id| self.pending_restored_tabs.contains(*id))
        {
            return self.defer_restored_action(
                id,
                DeferredRestoredAction::ReplaceAll {
                    matches: files,
                    replacement: replacement.to_owned(),
                },
            );
        }
        let occurrences = files.iter().map(|f| f.hits.len()).sum();
        if occurrences == 0 {
            return Task::none();
        }
        let mapping = self.replace_mapping();
        let mut opened = Vec::new();
        for file in &files {
            if let Some(id) = self.workspace.get_by_path(&file.path)
                && let Some(doc) = self.workspace.get_mut(id)
            {
                let clean = !doc.is_dirty();
                opened.push(OpenInput {
                    path: file.path.clone(),
                    snapshot: doc.snapshot_for_save(),
                    clean,
                });
            }
        }
        let op = crate::scritture::Op::Sostituisci {
            docs: opened.iter().map(|o| o.snapshot.id()).collect(),
            percorsi: files.iter().map(|f| f.path.clone()).collect(),
        };
        let lease = match self.scritture.ammetti(op) {
            Ok(crate::scritture::Permesso::Avvia(lease)) => lease,
            Err(rifiuto) => return self.notify(NoticeKind::Info, rifiuto.avviso()),
            Ok(_) => {
                return self.notify_error("Sostituzione non avviata: permesso non valido".into());
            }
        };
        self.replacement_sequence += 1;
        let id = self.replacement_sequence;
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.replace_root_epoch = self.git_root_epoch;
        self.replacement = Some(State {
            id,
            lease,
            root: self.project_root.clone(),
            mapping: mapping.clone(),
            opened: opened.clone(),
            cancel: cancel.clone(),
            phase: Phase::Preparing,
            autosaves: HashMap::new(),
            report: Report::default(),
            occurrences,
            files: files.len(),
            dirty: 0,
            reloads: Vec::new(),
        });
        let job = crate::replace_batch::Job::new(
            id,
            files,
            replacement.to_owned(),
            opened,
            mapping,
            cancel,
        );
        let delivery = job.delivery();
        self.replace_job = Some(job);
        Task::run(delivery, Self::replace_event)
    }

    /// Porta un aggiornamento del motore al pannello. Restituisce se l'ha
    /// consegnato: `Task` è opaco e il `Registry` dà solo `&dyn Tool`, quindi
    /// il bool è il seam su cui si asserisce che una generazione superata
    /// venga scartata.
    ///
    /// ATTENZIONE: L'errore va **nel pannello**, non in una notifica. In modalità
    /// regex ogni battuta intermedia è un'espressione a metà — `foo(` non
    /// compila — e una notifica per battuta sarebbe rumore addosso a chi sta
    /// ancora scrivendo.
    pub(super) fn deliver_search_update(
        &mut self,
        generation: u64,
        update: lume_core::SearchUpdate,
    ) -> bool {
        use std::sync::atomic::Ordering;
        if generation != self.search_gen.load(Ordering::SeqCst) {
            return false;
        }
        let evento = match update {
            lume_core::SearchUpdate::Batch(batch) => tools::search_tool::Event::Append(batch),
            lume_core::SearchUpdate::Done {
                files,
                matches,
                truncated,
            } => tools::search_tool::Event::Done {
                files,
                matches,
                truncated,
            },
            lume_core::SearchUpdate::Error(motivo) => tools::search_tool::Event::Failed(motivo),
        };
        if let Some(tool) = self.registry.get_mut(tools::search_tool::ID) {
            tool.update(&ToolEvent::new(evento));
            true
        } else {
            false
        }
    }

    /// Porta il cursore del documento attivo all'inizio di `line` (1-based);
    /// l'ensure-visible del widget scrolla di conseguenza.
    pub(super) fn goto_line(&mut self, line: usize) {
        let Some(doc) = self.active_doc().and_then(|id| self.workspace.get_mut(id)) else {
            return;
        };
        let rope = doc.rope();
        let l = line
            .saturating_sub(1)
            .min(rope.len_lines().saturating_sub(1));
        let pos = rope.line_to_char(l);
        set_primary(doc, Range::point(pos));
    }

    /// Applica il risultato selezionato del Cerca ovunque e chiude l'overlay.
    pub(super) fn activate_search_entry(&mut self, i: usize) -> Task<Message> {
        let Some(entry) = self.global_search.entries().into_iter().nth(i) else {
            return Task::none();
        };
        let mut apertura = Task::none();
        let apre_un_file = entry.path.is_some();
        if let Some(path) = entry.path.as_deref() {
            // Una entry Simboli porta anche la riga: aprire il file senza
            // spostarci il cursore lascerebbe l'utente a cercarla a mano.
            apertura = self.open_file_task(
                path.to_path_buf(),
                crate::file_read::DopoApertura::Ricerca { riga: entry.line },
            );
        }
        let act = entry.act;
        self.global_search.close();
        self.cancel_global_file_search();
        self.chiudi_overlay_se(OverlayId::Search);
        self.reconcile_file_palette();
        if let Some(action) = act {
            let intent = self.menu_bar.update(MenuBarMessage::Activate(action));
            return Task::batch([apertura, self.apply_menu_intent(intent)]);
        }
        // Se si apre un file, la sessione si salva a lettura finita
        // (`DopoApertura::Ricerca`): salvarla adesso fotograferebbe lo stato di prima.
        let sessione = if apre_un_file {
            Task::none()
        } else {
            self.persist_session()
        };
        Task::batch([apertura, sessione])
    }
}
