//! Appunti: caricamento, apertura, rinomina, salvataggio ed eliminazione delle note.
//!
//! Metodi di `Lume`: lo stato e i messaggi restano in `app.rs`.

use super::*;

impl Lume {
    pub(super) fn notes_autosave_schedule(&mut self) -> Task<Message> {
        let Some(doc) = self.active_doc() else {
            return Task::none();
        };
        let Some((dirty, version)) = self
            .workspace
            .get(doc)
            .map(|document| (document.is_dirty(), document.version()))
        else {
            return Task::none();
        };
        let Some(notes) = self.notes.as_mut() else {
            return Task::none();
        };
        let Some(note) = notes.tabs.get(&doc).cloned() else {
            return Task::none();
        };
        let read_only = notes
            .loaded
            .get(&note)
            .map(|loaded| loaded.read_only_error.is_some())
            .or_else(|| {
                notes
                    .open_saves
                    .get(&note)
                    .map(|saved| saved.read_only_error.is_some())
            })
            .unwrap_or(true);
        if read_only {
            return Task::none();
        }
        let state = notes.salvataggi.entry(doc).or_default();
        if !dirty {
            state.tornato_pulito();
            return Task::none();
        }
        Self::autosave_timer(doc, state.modificato(version))
    }

    pub(super) fn notes_autosave_due(&mut self, doc: DocId, timer_version: u64) -> Task<Message> {
        let Some((version, dirty)) = self
            .workspace
            .get(doc)
            .map(|document| (document.version(), document.is_dirty()))
        else {
            return Task::none();
        };
        let Some(notes) = self.notes.as_mut() else {
            return Task::none();
        };
        if !notes.tabs.contains_key(&doc) {
            return Task::none();
        }
        let Some(state) = notes.salvataggi.get_mut(&doc) else {
            return Task::none();
        };
        if !dirty {
            state.tornato_pulito();
            return Task::none();
        }
        if !state.scaduto(timer_version, version) {
            return Task::none();
        }
        self.queue_save_per(doc, crate::scritture::Causa::Automatico);
        Task::none()
    }

    pub(super) fn notes_resolve_conflict(&mut self, doc: DocId, copia: bool) -> Task<Message> {
        if !self.conflitto_della_scheda(doc) {
            return Task::none();
        }
        let Some((note_id, store, loaded, text)) = self.notes.as_ref().and_then(|notes| {
            let note_id = notes.tabs.get(&doc)?.clone();
            let loaded = notes.loaded.get(&note_id).cloned().or_else(|| {
                notes
                    .open_saves
                    .get(&note_id)
                    .map(|guard| guard.to_loaded(String::new()))
            })?;
            Some((
                note_id,
                notes.store.clone()?,
                loaded,
                self.workspace.get(doc)?.rope().to_string(),
            ))
        }) else {
            return self.notify_error(
                "Impossibile risolvere il conflitto: la nota non è più caricata".into(),
            );
        };
        let saved_copy = if copia {
            let mut copy_from = loaded.clone();
            copy_from.note.markdown = text;
            match store.save_copy_reporting(&copy_from) {
                Ok(copy) => Some(copy),
                Err(error) => return self.notify_error(format!("Copia non riuscita: {error}")),
            }
        } else {
            None
        };
        let mut from_disk = match store.load(loaded.path()) {
            Ok(note) => note,
            Err(error) => return self.notify_error(format!("Ricarica non riuscita: {error}")),
        };
        // Un Markdown senza front matter riceve un ID derivato dalla lettura;
        // la scheda aperta mantiene invece la propria identità stabile.
        from_disk.note.header.id = note_id.clone();
        let edit = self
            .workspace
            .get_mut(doc)
            .map(|document| document.reload_from_str(&from_disk.note.markdown));
        let Some(edit) = edit else {
            return Task::none();
        };
        self.resync_syntax(doc, edit);
        let scope = self
            .notes
            .as_ref()
            .map(|notes| notes.scan_generation)
            .unwrap_or_default();
        let followed = self.nota_salvata(doc, scope, &note_id, from_disk);
        if let Some(state) = self
            .notes
            .as_mut()
            .and_then(|notes| notes.salvataggi.get_mut(&doc))
        {
            state.risolto();
        }
        self.external_conflicts.remove(&doc);
        self.lsp_did_save(doc);
        self.sync_notes_tool();
        let copy_task = if let Some((copy, sync_error)) = saved_copy {
            let title = copy.note.header.title.clone();
            let (_, seguito) = self.open_loaded_note_con_seguito(copy);
            self.sync_notes_tool();
            let avviso = match sync_error {
                Some(error) => self.notify(
                    NoticeKind::Error,
                    format!(
                        "Copia salvata: «{title}», ma la cartella non è stata sincronizzata sul disco: {error}"
                    ),
                ),
                None => self.notify(NoticeKind::Info, format!("Copia salvata: «{title}»")),
            };
            Task::batch([seguito, avviso])
        } else {
            Task::none()
        };
        match followed {
            Ok(()) => copy_task,
            Err(error) => Task::batch([copy_task, self.notify_error(error)]),
        }
    }

    pub(super) fn begin_notes_load(&mut self) -> Option<u64> {
        if self.notes.is_some() {
            return None;
        }
        let generation = self.nuova_generazione_scansione_appunti()?;
        self.notes_library_sequence = generation;
        self.notes = Some(NotesState::loading(generation));
        Some(generation)
    }

    pub(super) fn notes_generation(&self) -> Option<u64> {
        self.notes.as_ref().map(|notes| notes.generation)
    }

    pub(super) fn notes_scan_generation(&self) -> Option<u64> {
        self.notes.as_ref().map(|notes| notes.scan_generation)
    }

    pub(super) fn next_notes_generation(&mut self) -> Option<u64> {
        let notes = self.notes.as_mut()?;
        notes.generation = notes.generation.wrapping_add(1);
        Some(notes.generation)
    }

    pub(super) fn load_notes(&mut self) -> Task<Message> {
        let Some(generation) = self
            .begin_notes_load()
            .or_else(|| self.restart_notes_first_load())
        else {
            // Riapertura dopo lo scarico: i riepiloghi non ci sono più e si
            // rileggono dal disco. La scansione ha una generazione sua: una
            // creazione o un'apertura nel frattempo (che fanno avanzare
            // `generation`) non devono farla scartare.
            //
            // Invariante: `view_unloaded` vale finché il pannello è chiuso.
            // Questo `take` lo azzera anche se `load_notes` parte da «Nuovo
            // appunto» a pannello chiuso; in quel caso lo rimette a posto lo
            // scarico del gestore `NotesScanned` (`!registry.is_open`).
            if let Some((generation, directory)) = self.take_notes_rescan() {
                return self.scan_notes_directory(generation, directory);
            }
            self.sync_notes_tool();
            return Task::none();
        };
        if let Some(directory) = self.notes_directory_scelta.clone() {
            return self.scan_notes_directory(generation, directory);
        }
        let Some(store) = self.store.clone() else {
            return self.scan_notes_directory(generation, default_notes_directory());
        };
        Task::perform(
            async move {
                store
                    .load_notes_directory()
                    .await
                    .map_err(|error| error.to_string())
            },
            move |result| Message::NotesPreferenceLoaded(generation, result),
        )
    }

    pub(super) fn scan_notes_directory(
        &self,
        generation: u64,
        directory: PathBuf,
    ) -> Task<Message> {
        Task::perform(
            async move {
                notes_scan_blocking(directory.clone())
                    .await
                    .map(|value| (directory, value))
            },
            move |result| match result {
                Ok((directory, value)) => Message::NotesScanned(generation, directory, Ok(value)),
                Err(error) => Message::NotesScanned(generation, PathBuf::new(), Err(error)),
            },
        )
    }

    pub(super) fn finish_notes_scan(
        &mut self,
        generation: u64,
        directory: PathBuf,
        store: NoteStore,
        summaries: Vec<NoteSummary>,
    ) -> bool {
        let Some(notes) = self
            .notes
            .as_mut()
            .filter(|notes| notes.scan_request == generation)
        else {
            return false;
        };
        self.notes_directory = directory.clone();
        notes.directory = Some(directory);
        notes.store = Some(store);
        if notes.view_unloaded {
            // Scansione partita prima della chiusura del pannello: il
            // risultato non deve rimettere in memoria ciò che si è scaricato.
            return true;
        }
        // La scansione può essere partita prima di una nota creata nel
        // frattempo: i riepiloghi già noti di schede aperte, assenti dalla
        // scansione, restano (in testa, come li inserisce l'apertura).
        let mut riepiloghi: Vec<NoteSummary> = std::mem::take(&mut notes.summaries)
            .into_iter()
            .filter(|noto| {
                notes.tabs.values().any(|id| *id == noto.id)
                    && !summaries
                        .iter()
                        .any(|scansionato| scansionato.id == noto.id)
            })
            .collect();
        riepiloghi.extend(summaries);
        notes.summaries = riepiloghi;
        self.sync_notes_tool();
        true
    }

    #[cfg(test)]
    pub(super) fn finish_notes_load(
        &mut self,
        generation: u64,
        directory: PathBuf,
        loaded: Vec<LoadedNote>,
    ) -> Result<(), String> {
        let store = NoteStore::open(&directory).map_err(|error| error.to_string())?;
        let summaries = store.scan().map_err(|error| error.to_string())?;
        self.finish_notes_scan(generation, directory, store, summaries);
        if let Some(notes) = self.notes.as_mut() {
            for note in loaded {
                notes.loaded.insert(note.note.header.id.clone(), note);
            }
        }
        Ok(())
    }

    pub(super) fn sync_notes_tool(&mut self) {
        let Some((view_unloaded, directory, summaries, generation)) =
            self.notes.as_mut().map(|notes| {
                notes.link_index = lume_notes::LinkIndex::build(&notes.summaries);
                (
                    notes.view_unloaded,
                    notes.directory.clone(),
                    notes.summaries.clone(),
                    notes.scan_generation,
                )
            })
        else {
            return;
        };
        self.aggiorna_marcatori();
        if view_unloaded {
            // Pannello chiuso: il tool non ha nulla da mostrare, e rimandargli
            // i riepiloghi ne rifarebbe la cache.
            return;
        }
        let Some(directory) = directory else {
            return;
        };
        if let Some(tool) = self.registry.get_mut(tools::notes::ID) {
            tool.update(&ToolEvent::new(tools::notes::Event::Replace {
                summaries,
                directory,
                origin: lume_ui::tool::NotesOrigin(generation),
            }));
        }
    }

    pub(super) fn appunti_del_file(&self, doc: DocId) -> Vec<lume_notes::LinkedNote> {
        let Some(root) = self.project_root.as_deref() else {
            return Vec::new();
        };
        let Some(path) = self.workspace.get(doc).and_then(lume_core::Document::path) else {
            return Vec::new();
        };
        let Some(relative) = lume_notes::relative_path(root, path) else {
            return Vec::new();
        };
        self.notes.as_ref().map_or_else(Vec::new, |notes| {
            notes.link_index.for_file(root, &relative).to_vec()
        })
    }

    pub(super) fn marcatori_di(&self, doc: DocId) -> Vec<std::ops::Range<usize>> {
        let mut ranges: Vec<_> = self
            .appunti_del_file(doc)
            .into_iter()
            .map(|linked| linked.from.saturating_sub(1)..linked.to)
            .filter(|range| !range.is_empty())
            .collect();
        ranges.sort_by_key(|range| (range.start, range.end));
        ranges
    }

    pub(super) fn aggiorna_marcatori_di(&mut self, doc: DocId) {
        if self
            .notes
            .as_ref()
            .is_some_and(|notes| notes.tabs.contains_key(&doc))
        {
            return;
        }
        let ranges = self.marcatori_di(doc);
        if let Some(notes) = self.notes.as_mut() {
            if ranges.is_empty() {
                notes.marcatori.remove(&doc);
            } else {
                notes.marcatori.insert(doc, ranges);
            }
        }
    }

    pub(super) fn aggiorna_marcatori(&mut self) {
        let docs: Vec<_> = self.workspace.ids().collect();
        if let Some(notes) = self.notes.as_mut() {
            notes.marcatori.clear();
        }
        for doc in docs {
            self.aggiorna_marcatori_di(doc);
        }
    }

    pub(super) fn click_note_marker(
        &mut self,
        group: crate::editor_groups::GroupId,
        line: usize,
    ) -> Task<Message> {
        let Some(doc) = self.groups.active_in(group) else {
            return Task::none();
        };
        let matching: Vec<_> = self
            .appunti_del_file(doc)
            .into_iter()
            .filter(|link| link.from.saturating_sub(1) <= line && line < link.to)
            .collect();
        match matching.as_slice() {
            [linked] => self.open_note(linked.note.clone()),
            [] => Task::none(),
            _ => self.notify(
                NoticeKind::Info,
                "Più appunti citano questa riga: scegli dalla striscia sopra l'editor".into(),
            ),
        }
    }

    /// Errore di una scansione (o della lettura della cartella): si confronta
    /// con la generazione delle scansioni, come il risultato corrispondente.
    pub(super) fn notes_scan_error(&mut self, generation: u64, error: String) {
        if self.notes.as_ref().map(|notes| notes.scan_request) != Some(generation) {
            return;
        }
        self.show_notes_error(error);
    }

    pub(super) fn notes_error(&mut self, generation: u64, error: String) {
        if self.notes_generation() != Some(generation) {
            return;
        }
        self.show_notes_error(error);
    }

    pub(super) fn show_notes_error(&mut self, error: String) {
        if let Some(tool) = self.registry.get_mut(tools::notes::ID) {
            tool.update(&ToolEvent::new(tools::notes::Event::Error(error.clone())));
        }
        self.persistence_error = Some(error);
    }

    pub(super) fn create_note(&mut self) -> Task<Message> {
        if self.scritture.guarda(None).in_volo.is_some() || self.git_operazione.is_some() {
            return self.notify_error(
                "Ci sono operazioni sui file in corso: attendi prima di creare una nota".into(),
            );
        }
        let Some(store) = self.notes.as_ref().and_then(|notes| notes.store.clone()) else {
            return self
                .notify_error("Appunti in caricamento: attendi prima di creare una nota".into());
        };
        let Some(generation) = self.next_notes_generation() else {
            return Task::none();
        };
        Task::perform(notes_create_blocking(store), move |result| {
            Message::NotesOpened(generation, result)
        })
    }

    pub(super) fn note_link_menu(&self) -> (bool, bool) {
        let Some(notes) = self.notes.as_ref().filter(|notes| notes.store.is_some()) else {
            return (false, false);
        };
        let Some(code) = self.active_doc() else {
            return (false, false);
        };
        if notes.tabs.contains_key(&code) || self.sql_console_name(code).is_some() {
            return (false, false);
        }
        let in_project = self
            .project_root
            .as_deref()
            .zip(self.workspace.get(code).and_then(lume_core::Document::path))
            .and_then(|(root, path)| lume_notes::relative_path(root, path))
            .is_some();
        if !in_project {
            return (false, false);
        }
        (self.nota_di_destinazione(code).is_some(), true)
    }

    pub(super) fn nota_di_destinazione(&self, code: DocId) -> Option<DocId> {
        let notes = self.notes.as_ref()?;
        let group = self.groups.group_of(code)?;
        [group.other(), group]
            .into_iter()
            .filter_map(|group| self.groups.active_in(group))
            .find(|doc| notes.tabs.contains_key(doc))
    }

    pub(super) fn code_link_for(
        &self,
        doc: DocId,
        selection: Option<Range>,
    ) -> Option<lume_notes::CodeLink> {
        let root = self.project_root.as_deref()?;
        let document = self.workspace.get(doc)?;
        let relative = lume_notes::relative_path(root, document.path()?)?;
        let selection = selection.unwrap_or_else(|| document.selection());
        let (from, to) = crate::note_links::righe_della_selezione(document.rope(), selection);
        lume_notes::CodeLink::try_new(root, relative, from, to, None).ok()
    }

    pub(super) fn prepara_collegamento(
        &mut self,
        doc: DocId,
        modify: impl FnOnce(&mut lume_notes::Note) -> Result<String, String>,
    ) -> Result<String, String> {
        if let Some(avviso) = self.rifiuto_salvataggio() {
            return Err(avviso);
        }
        let (note_id, mut loaded, text, version) = {
            let notes = self
                .notes
                .as_ref()
                .ok_or_else(|| "Gli Appunti non sono ancora caricati".to_owned())?;
            if notes.store.is_none() {
                return Err("Gli Appunti non sono ancora caricati".to_owned());
            }
            let note_id = notes
                .tabs
                .get(&doc)
                .cloned()
                .ok_or_else(|| "Questa scheda non è un appunto".to_owned())?;
            let loaded = notes
                .loaded
                .get(&note_id)
                .cloned()
                .or_else(|| {
                    notes
                        .open_saves
                        .get(&note_id)
                        .map(|state| state.to_loaded(String::new()))
                })
                .ok_or_else(|| "Questa nota non è caricata".to_owned())?;
            let document = self
                .workspace
                .get(doc)
                .ok_or_else(|| "La scheda non esiste più".to_owned())?;
            (
                note_id,
                loaded,
                document.rope().to_string(),
                document.version(),
            )
        };
        if loaded.read_only_error.is_some() {
            return Err("Questa nota è in sola lettura".into());
        }
        let notice = modify(&mut loaded.note)?;
        let Some(notes) = self.notes.as_mut() else {
            return Err("Gli Appunti sono stati chiusi".into());
        };
        if !notes.salvataggi.entry(doc).or_default().avviato(version) {
            return Err(
                "La nota si sta salvando o è in conflitto con il disco: riprova tra un istante"
                    .into(),
            );
        }
        loaded.note.markdown = text;
        if notes.view_unloaded {
            notes
                .open_saves
                .insert(note_id, OpenNoteSaveState::from_loaded(&loaded));
        } else {
            notes.loaded.insert(note_id, loaded);
        }
        notes.link_saves.insert(doc, notice.clone());
        self.queue_save(doc);
        Ok(notice)
    }

    pub(super) fn modifica_collegamenti(
        &mut self,
        doc: DocId,
        modify: impl FnOnce(&mut lume_notes::Note) -> Result<String, String>,
    ) -> Task<Message> {
        match self.prepara_collegamento(doc, modify) {
            Err(reason) => self.notify(NoticeKind::Info, reason),
            Ok(_) => Task::none(),
        }
    }

    pub(super) fn collega_a_nota_aperta(&mut self, selection: Option<Range>) -> Task<Message> {
        let Some(code) = self.active_doc() else {
            return Task::none();
        };
        let Some(note) = self.nota_di_destinazione(code) else {
            return Task::none();
        };
        let Some(link) = self.code_link_for(code, selection) else {
            return self.notify(
                NoticeKind::Info,
                "Questo file non è nella cartella del progetto".into(),
            );
        };
        self.modifica_collegamenti(note, move |note| {
            crate::note_links::aggiungi_collegamento(note, link)
        })
    }

    pub(super) fn collega_a_nota_nuova(&mut self, selection: Option<Range>) -> Task<Message> {
        let Some(code) = self.active_doc() else {
            return Task::none();
        };
        let Some(link) = self.code_link_for(code, selection) else {
            return self.notify(
                NoticeKind::Info,
                "Questo file non è nella cartella del progetto".into(),
            );
        };
        let Some(generation) = self.next_notes_generation() else {
            return Task::none();
        };
        let Some(store) = self.notes.as_ref().and_then(|notes| notes.store.clone()) else {
            return Task::none();
        };
        Task::perform(notes_create_linked_blocking(store, link), move |result| {
            Message::NotesOpened(generation, result)
        })
    }

    pub(super) fn open_note_link(&mut self, doc: DocId, index: usize) -> Task<Message> {
        let Some(link) = self
            .notes
            .as_ref()
            .and_then(|notes| {
                let id = notes.tabs.get(&doc)?;
                notes
                    .loaded
                    .get(id)
                    .map(|loaded| &loaded.note.header)
                    .or_else(|| notes.open_saves.get(id).map(|saved| &saved.header))
            })
            .and_then(|header| header.links.get(index))
            .cloned()
        else {
            return Task::none();
        };
        let path = match lume_notes::resolve_link(&link) {
            Ok(path) => path,
            Err(error) => return self.notify_error(format!("Collegamento non valido: {error}")),
        };
        self.record_jump(crate::navigazione::Posizione::new(
            path.clone(),
            link.from.saturating_sub(1),
        ));
        if let Some(code_doc) = self.workspace.get_by_path(&path) {
            self.groups.open(self.groups.focus(), code_doc);
            self.activate_editor_tab(code_doc);
            if let Some(document) = self.workspace.get_mut(code_doc) {
                let lines = crate::note_links::righe_del_testo(document.rope());
                let (start, end) =
                    crate::note_links::caratteri_delle_righe(document.rope(), link.from, link.to);
                set_primary(document, Range::new(start, end));
                if link.to > lines {
                    return self.notify(
                        NoticeKind::Info,
                        format!("Il file ha ora {lines} righe: il collegamento arrivava più in là"),
                    );
                }
            }
            return Task::none();
        }
        self.open_file_task(
            path,
            crate::file_read::DopoApertura::IntervalloRighe {
                da: link.from,
                a: link.to,
            },
        )
    }

    pub(super) fn striscia_collegamenti(
        &self,
        group: crate::editor_groups::GroupId,
    ) -> Option<Element<'_, Message>> {
        let notes = self.notes.as_ref()?;
        let doc = self.groups.active_in(group)?;
        let chips = if let Some(id) = notes.tabs.get(&doc) {
            let header = notes
                .loaded
                .get(id)
                .map(|loaded| &loaded.note.header)
                .or_else(|| notes.open_saves.get(id).map(|saved| &saved.header))?;
            crate::note_bars::chips_for_note(doc, &header.links)
        } else {
            if notes.link_index.is_empty() {
                return None;
            }
            let root = self.project_root.as_deref()?;
            let file = self.workspace.get(doc)?.path()?;
            crate::note_bars::chips_for_code(&notes.link_index, root, file)
        };
        if chips.is_empty() {
            return None;
        }
        Some(crate::note_bars::strip(self.theme(), chips))
    }

    /// La scheda già aperta per questa nota, se c'è: aprirla di nuovo la
    /// duplicherebbe, o peggio rimpiazzerebbe la bozza con il disco.
    pub(super) fn open_note_tab(&self, id: &NoteId) -> Option<DocId> {
        self.notes
            .as_ref()?
            .tabs
            .iter()
            .find_map(|(doc, note)| (note == id).then_some(*doc))
    }

    pub(super) fn activate_note_tab(&mut self, doc: DocId) {
        self.groups.open(self.groups.focus(), doc);
        self.activate_editor_tab(doc);
    }

    pub(super) fn open_note(&mut self, id: NoteId) -> Task<Message> {
        if let Some(doc) = self.open_note_tab(&id) {
            self.activate_note_tab(doc);
            return Task::none();
        }
        if self.scritture.guarda(None).in_volo.is_some() || self.git_operazione.is_some() {
            return self.notify_error(
                "Ci sono scritture in corso: attendi e ripeti l'apertura della nota".into(),
            );
        }
        if let Some(loaded) = self
            .notes
            .as_ref()
            .and_then(|notes| notes.loaded.get(&id))
            .cloned()
        {
            return self.open_loaded_note_con_seguito(loaded).1;
        }
        let Some(generation) = self.next_notes_generation() else {
            return Task::none();
        };
        let Some((store, path)) = self.notes.as_ref().and_then(|notes| {
            Some((
                notes.store.clone()?,
                notes
                    .summaries
                    .iter()
                    .find(|summary| summary.id == id)?
                    .path
                    .clone(),
            ))
        }) else {
            return self.notify_error(
                "La nota non è più disponibile: aggiorna gli Appunti e riprova".into(),
            );
        };
        let epoch = self.scritture.guarda(None).epoca;
        Task::perform(notes_open_blocking(store, path), move |result| {
            Message::NotesReadFinished(generation, epoch, result)
        })
    }

    pub(super) fn finish_notes_opened(
        &mut self,
        generation: u64,
        result: Result<LoadedNote, String>,
    ) -> Task<Message> {
        match result {
            Ok(note) if self.notes_generation() == Some(generation) => {
                if let Err(error) = self.check_file_binding(note.path()) {
                    return self.notify_error(error.to_string());
                }
                let (_, seguito) = self.open_loaded_note_con_seguito(note);
                self.sync_notes_tool();
                return seguito;
            }
            Ok(_) => {}
            Err(error) => self.notes_error(generation, error),
        }
        Task::none()
    }

    /// I seguiti appartengono solo al placeholder consumato da questa apertura.
    fn open_loaded_note_con_seguito(&mut self, loaded: LoadedNote) -> (DocId, Task<Message>) {
        let placeholder = self
            .workspace
            .get_by_path(loaded.path())
            .filter(|id| self.pending_restored_tabs.contains(*id));
        let doc = self.open_loaded_note(loaded);
        let seguito = if placeholder == Some(doc) && !self.pending_restored_tabs.contains(doc) {
            self.replay_restored_commands(doc)
        } else {
            Task::none()
        };
        (doc, seguito)
    }

    pub(super) fn open_loaded_note(&mut self, loaded: LoadedNote) -> DocId {
        let id = loaded.note.header.id.clone();
        if let Some(doc) = self.open_note_tab(&id) {
            // Stessa nota già in una scheda (per esempio un caricamento in
            // volo durante la chiusura del pannello): si attiva quella. Né
            // il testo né la guardia di salvataggio si toccano: la lettura
            // nuova ha un'identità più recente di quella che l'editor ha
            // davvero e nasconderebbe una modifica esterna.
            self.activate_note_tab(doc);
            return doc;
        }
        let path = loaded.path().to_path_buf();
        let doc = self.workspace.open(Some(path), &loaded.note.markdown);
        if self.pending_restored_tabs.take(doc).is_some()
            && let Some(document) = self.workspace.get_mut(doc)
        {
            document.reload_from_str(&loaded.note.markdown);
            self.concludi_materializzazione_ripristinata(doc);
        }
        if !loaded.original_bytes.is_empty() {
            let impronta =
                crate::file_read::Testo::from_note(&loaded.original_bytes, String::new())
                    .fingerprint();
            self.disk_fingerprints.insert(doc, impronta);
            self.external_conflicts.remove(&doc);
        }
        self.activate_note_tab(doc);
        if let Some(notes) = self.notes.as_mut() {
            notes
                .titles
                .insert(id.clone(), loaded.note.header.title.clone());
            notes.tabs.insert(doc, id.clone());
            if notes.view_unloaded {
                // Pannello chiuso: niente cache, solo la guardia minima.
                notes
                    .open_saves
                    .insert(id, OpenNoteSaveState::from_loaded(&loaded));
                return doc;
            }
            let summary = loaded.summary();
            if let Some(existing) = notes.summaries.iter_mut().find(|summary| summary.id == id) {
                *existing = summary;
            } else {
                notes.summaries.insert(0, summary);
            }
            notes.open_saves.remove(&id);
            notes.loaded.insert(id, loaded);
        }
        doc
    }

    /// Registra una scheda nota materializzata durante il ripristino di
    /// sessione: se il pannello Appunti è chiuso non popola la cache della
    /// vista (`loaded`/`summaries`), ma conserva `store` e guardia minima in
    /// `open_saves` per salvataggi e riletture del watcher.
    pub(super) fn registra_nota_ripristinata(
        &mut self,
        doc: DocId,
        directory: PathBuf,
        store: NoteStore,
        loaded: LoadedNote,
    ) {
        if self.notes.is_none() {
            let Some(generation) = self.nuova_generazione_scansione_appunti() else {
                return;
            };
            self.notes_library_sequence = generation;
            let mut state = NotesState::loading(generation);
            state.view_unloaded = true;
            self.notes = Some(state);
        }
        self.notes_directory = directory.clone();
        let Some(notes) = self.notes.as_mut() else {
            return;
        };
        if notes.directory.is_none() {
            notes.directory = Some(directory);
        }
        if notes.store.is_none() {
            notes.store = Some(store);
        }
        let note_id = loaded.note.header.id.clone();
        notes
            .titles
            .insert(note_id.clone(), loaded.note.header.title.clone());
        notes.tabs.insert(doc, note_id.clone());
        if notes.view_unloaded {
            notes
                .open_saves
                .insert(note_id, OpenNoteSaveState::from_loaded(&loaded));
        } else {
            notes.open_saves.remove(&note_id);
            notes.loaded.insert(note_id, loaded);
        }
    }

    /// Registra la versione effettivamente scritta e segue una rinomina nel
    /// percorso del documento e nel riepilogo della libreria.
    pub(super) fn nota_salvata(
        &mut self,
        doc: DocId,
        scope: u64,
        nota: &NoteId,
        salvata: LoadedNote,
    ) -> Result<(), String> {
        let nuovo_percorso = salvata.path().to_path_buf();
        let precedente = self
            .workspace
            .get(doc)
            .and_then(|document| document.path())
            .map(Path::to_path_buf);
        let aggiorna = self.notes.as_ref().is_some_and(|notes| {
            notes.scan_generation == scope && notes.tabs.get(&doc) == Some(nota)
        });
        if !aggiorna {
            return Err("la scheda Appunti è cambiata durante il salvataggio".into());
        }
        if precedente.as_deref() != Some(nuovo_percorso.as_path()) {
            self.workspace
                .rebind_path(doc, nuovo_percorso.clone())
                .map_err(|error| {
                    format!("la nota è salva ma la scheda non segue il nuovo nome: {error}")
                })?;
        }
        let Some(notes) = self.notes.as_mut() else {
            return Err("la libreria Appunti è stata chiusa durante il salvataggio".into());
        };
        let summary = salvata.summary();
        match notes.summaries.iter_mut().find(|entry| entry.id == *nota) {
            Some(entry) => *entry = summary,
            None if !notes.view_unloaded => notes.summaries.insert(0, summary),
            None => {}
        }
        notes
            .titles
            .insert(nota.clone(), salvata.note.header.title.clone());
        if notes.view_unloaded {
            notes
                .open_saves
                .insert(nota.clone(), OpenNoteSaveState::from_loaded(&salvata));
        } else {
            notes.loaded.insert(nota.clone(), salvata);
        }
        self.sync_notes_tool();
        Ok(())
    }

    /// Registra la nota riletta dal disco dopo una modifica esterna: il corpo
    /// è già nel buffer, qui seguono intestazione e identità perché il
    /// prossimo salvataggio usi la guardia giusta.
    pub(super) fn nota_riletta(&mut self, doc: DocId, letta: LoadedNote) {
        let nota = letta.note.header.id.clone();
        let Some(notes) = self.notes.as_mut() else {
            return;
        };
        if notes.tabs.get(&doc) != Some(&nota) {
            return;
        }
        let summary = letta.summary();
        match notes.summaries.iter_mut().find(|entry| entry.id == nota) {
            Some(entry) => *entry = summary,
            None if !notes.view_unloaded => notes.summaries.insert(0, summary),
            None => {}
        }
        notes
            .titles
            .insert(nota.clone(), letta.note.header.title.clone());
        if notes.view_unloaded {
            notes
                .open_saves
                .insert(nota, OpenNoteSaveState::from_loaded(&letta));
        } else {
            notes.loaded.insert(nota, letta);
        }
        self.sync_notes_tool();
    }

    /// Scarica la cache della vista Appunti alla chiusura del pannello.
    ///
    /// Per ogni nota ancora aperta in una scheda si tiene solo la guardia di
    /// salvataggio (intestazione, percorso, identità, sola lettura: niente
    /// corpo, niente `original_bytes`); `loaded` e `summaries` tornano vuoti
    /// con la memoria restituita. Store, cartella, schede e titoli restano
    /// finché vivono i loro proprietari.
    pub(super) fn unload_notes_view_cache(&mut self) {
        let Some(notes) = self.notes.as_mut() else {
            return;
        };
        let aperte: Vec<NoteId> = notes.tabs.values().cloned().collect();
        for id in aperte {
            if let Some(loaded) = notes.loaded.get(&id) {
                notes
                    .open_saves
                    .insert(id, OpenNoteSaveState::from_loaded(loaded));
            }
        }
        notes.loaded = HashMap::new();
        notes.summaries = Vec::new();
        notes.view_unloaded = true;
        if let Some(tool) = self.registry.get_mut(tools::notes::ID) {
            let _ = tool.update(&ToolEvent::new(tools::notes::Event::Clear));
        }
    }

    pub(crate) fn editor_kind(&self, doc: DocId) -> crate::editor_groups::EditorKind {
        if let Some(id) = self.notes.as_ref().and_then(|notes| notes.tabs.get(&doc)) {
            crate::editor_groups::EditorKind::Note(id.clone())
        } else if self.is_sql_console(doc) {
            crate::editor_groups::EditorKind::Sql
        } else {
            crate::editor_groups::EditorKind::Code
        }
    }

    pub(crate) fn editor_tab_name(&self, doc: DocId) -> Option<String> {
        match self.editor_kind(doc) {
            crate::editor_groups::EditorKind::Note(id) => {
                self.notes.as_ref()?.titles.get(&id).cloned()
            }
            crate::editor_groups::EditorKind::Code if self.e_senza_nome(doc) => self
                .senza_nome
                .get(&doc)
                .map(|numero| format!("senza nome {numero}")),
            _ => None,
        }
    }

    pub(super) fn delete_note(&mut self, id: NoteId) -> Task<Message> {
        self.delete_note_with(id, delete_note_from_store)
    }

    pub(super) fn delete_note_with(
        &mut self,
        id: NoteId,
        delete: fn(&NoteStore, &Path) -> Result<DeleteDisposition, String>,
    ) -> Task<Message> {
        if self.scritture.guarda(None).in_volo.is_some() || self.git_operazione.is_some() {
            return self.notify_error(
                "È in corso un'operazione file: attendi prima di eliminare la nota".into(),
            );
        }
        let Some((store, path, scope)) = self.notes.as_ref().and_then(|notes| {
            Some((
                notes.store.clone()?,
                notes
                    .summaries
                    .iter()
                    .find(|summary| summary.id == id)?
                    .path
                    .clone(),
                notes.scan_generation,
            ))
        }) else {
            return self.notify_error(
                "La nota non è più disponibile: aggiorna gli Appunti e riprova".into(),
            );
        };
        let documenti = self.note_delete_documenti();
        let noti: Vec<_> = documenti
            .iter()
            .filter(|doc| {
                doc.nota.as_ref() == Some(&id) || doc.path.as_deref() == Some(path.as_path())
            })
            .map(|doc| doc.id)
            .collect();
        let lease = match self
            .scritture
            .ammetti(crate::scritture::Op::EliminaAppunto { docs: noti })
        {
            Ok(crate::scritture::Permesso::Avvia(lease)) => lease,
            Err(rifiuto) => return self.notify(NoticeKind::Info, rifiuto.avviso()),
            Ok(_) => return self.notify_error("Eliminazione della nota non avviata".into()),
        };
        self.note_delete_sequence += 1;
        let request = self.note_delete_sequence;
        self.note_delete = Some(NoteDeletion {
            request,
            scope,
            root_epoch: self.git_root_epoch,
            id: id.clone(),
            lease,
            store: store.clone(),
            path: path.clone(),
            delete,
            documenti: documenti.clone(),
            correlati: None,
        });
        Task::perform(
            notes_delete_prepare_blocking(store, path, documenti),
            move |result| Message::NotesDeletePrepared(request, id.clone(), scope, lease, result),
        )
    }

    /// Fotografa i binding senza I/O; le revisioni estranee possono cambiare.
    fn note_delete_documenti(&self) -> Vec<NoteDeleteDocument> {
        let mut documenti: Vec<_> = self
            .workspace
            .ids()
            .filter_map(|id| {
                self.workspace.get(id).map(|doc| NoteDeleteDocument {
                    id,
                    path: doc.path().map(Path::to_path_buf),
                    nota: self
                        .notes
                        .as_ref()
                        .and_then(|notes| notes.tabs.get(&id).cloned()),
                    versione: doc.version(),
                    sporco: self.chiede_salvataggio(id),
                })
            })
            .collect();
        documenti.sort_by_key(|doc| doc.id.0);
        documenti
    }

    fn note_delete_preflight_fallito(&mut self, error: String) -> Task<Message> {
        let mut tasks = Vec::new();
        if let Some(deletion) = self.note_delete.take() {
            self.scritture
                .rilascia(deletion.lease, crate::scritture::Esito::NonAvviato);
            for doc in deletion.documenti {
                if !(self
                    .scritture
                    .guarda(Some((
                        doc.id,
                        self.workspace
                            .get(doc.id)
                            .and_then(lume_core::Document::path),
                    )))
                    .file
                    != crate::scritture::Presa::Libero
                    || self.git_operazione.is_some())
                {
                    tasks.push(self.reload_pending_fs_recheck(doc.id));
                }
            }
        }
        self.show_notes_error(error.clone());
        tasks.push(self.save_failed(error));
        Task::batch(tasks)
    }

    pub(super) fn note_delete_prepared(
        &mut self,
        request: u64,
        id: NoteId,
        scope: u64,
        lease: crate::scritture::Lease,
        result: Result<NoteDeletePrepared, String>,
    ) -> Task<Message> {
        let Some(deletion) = self.note_delete.as_ref().filter(|d| {
            d.request == request
                && d.id == id
                && d.scope == scope
                && d.lease == lease
                && d.correlati.is_none()
        }) else {
            return Task::none();
        };
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => return self.note_delete_preflight_fallito(error),
        };
        let corrente = self.note_delete_documenti();
        let binding_invariati = corrente.len() == deletion.documenti.len()
            && corrente
                .iter()
                .zip(&deletion.documenti)
                .all(|(ora, prima)| {
                    ora.id == prima.id && ora.path == prima.path && ora.nota == prima.nota
                });
        let scope_attuale = self.notes.as_ref().is_some_and(|notes| {
            notes.scan_generation == scope
                && notes
                    .summaries
                    .iter()
                    .any(|summary| summary.id == id && summary.path == deletion.path)
        });
        if !binding_invariati || !scope_attuale || self.git_root_epoch != deletion.root_epoch {
            return self.note_delete_preflight_fallito(
                "Schede, binding o cartella Appunti cambiati: eliminazione annullata, riprova"
                    .into(),
            );
        }
        let mut correlati = prepared.alias.clone();
        for doc in &deletion.documenti {
            if (doc.nota.as_ref() == Some(&id)
                || doc.path.as_deref() == Some(deletion.path.as_path()))
                && !correlati.contains(&doc.id)
            {
                correlati.push(doc.id);
            }
        }
        if correlati
            .iter()
            .any(|id| self.pending_restored_tabs.contains(*id))
        {
            return self.note_delete_preflight_fallito(
                "La nota o un suo alias è ancora in caricamento: attendi prima di eliminarla"
                    .into(),
            );
        }
        let cambiato = correlati.iter().any(|id| {
            let prima = deletion.documenti.iter().find(|doc| doc.id == *id);
            let ora = corrente.iter().find(|doc| doc.id == *id);
            match (prima, ora) {
                (Some(prima), Some(ora)) => {
                    prima.sporco
                        || ora.sporco
                        || prima.versione != ora.versione
                        || (self.scritture.contiene_salvataggio(*id)
                            || self.saves.active_document() == Some(*id))
                }
                _ => true,
            }
        });
        if cambiato {
            return self.note_delete_preflight_fallito("La nota è aperta o cambiata con modifiche non salvate, anche tramite alias: salvale prima di eliminarla".into());
        }
        if !self.scritture.estendi_presa(lease, &correlati) {
            return self
                .note_delete_preflight_fallito("Eliminazione della nota scaduta: riprova".into());
        }
        let Some(deletion) = self.note_delete.as_mut() else {
            return Task::none();
        };
        deletion.correlati = Some(correlati);
        let store = deletion.store.clone();
        let path = deletion.path.clone();
        let documenti = deletion.documenti.clone();
        let delete = deletion.delete;
        Task::perform(
            notes_delete_blocking(store, path, documenti, prepared, delete),
            move |result| Message::NotesDeleted(request, id.clone(), scope, lease, result),
        )
    }

    pub(super) fn note_deleted(
        &mut self,
        request: u64,
        id: NoteId,
        scope: u64,
        lease: crate::scritture::Lease,
        result: Result<DeleteDisposition, String>,
    ) -> Task<Message> {
        if !self.note_delete.as_ref().is_some_and(|d| {
            d.request == request
                && d.id == id
                && d.scope == scope
                && d.lease == lease
                && d.correlati.is_some()
        }) || !self.scritture.estendi_presa(lease, &[])
        {
            return Task::none();
        }
        let Some(deletion) = self.note_delete.take() else {
            return Task::none();
        };
        let da_ricontrollare = deletion.correlati.clone().unwrap_or_default();
        let esito = match &result {
            Ok(DeleteDisposition::Trashed) => crate::scritture::Esito::Riuscito,
            _ => crate::scritture::Esito::Fallito,
        };
        if matches!(&result, Ok(DeleteDisposition::Trashed)) {
            // Nessuna richiesta pendente deve ricreare il file spostato.
            for doc in deletion.correlati.as_deref().unwrap_or_default() {
                let stesso_binding = deletion
                    .documenti
                    .iter()
                    .find(|p| p.id == *doc)
                    .is_some_and(|prima| {
                        self.workspace
                            .get(*doc)
                            .is_some_and(|ora| ora.path() == prima.path.as_deref())
                    });
                if stesso_binding && self.scritture.revoca_salvataggi(Some(*doc)) > 0 {
                    self.save_origins.remove(doc);
                }
            }
        }
        self.scritture.rilascia(lease, esito);
        let mut tasks = Vec::new();
        match result {
            Ok(DeleteDisposition::Trashed) => {
                let related = deletion.correlati.unwrap_or_default();
                if self.git_root_epoch == deletion.root_epoch
                    && self
                        .notes
                        .as_ref()
                        .is_some_and(|notes| notes.scan_generation == deletion.scope)
                {
                    let mut dirty = false;
                    for doc in related {
                        let invariato = deletion
                            .documenti
                            .iter()
                            .find(|p| p.id == doc)
                            .is_some_and(|prima| {
                                self.workspace.get(doc).is_some_and(|ora| {
                                    ora.path() == prima.path.as_deref()
                                        && ora.version() == prima.versione
                                })
                            });
                        if !invariato || self.chiede_salvataggio(doc) {
                            dirty = true;
                        } else {
                            self.close_tab(doc);
                        }
                    }
                    if !dirty {
                        self.finish_note_delete(&id);
                    }
                    self.sync_notes_tool();
                    if dirty {
                        tasks.push(self.save_failed("Nota spostata nel Cestino, ma modifiche successive restano nei buffer: copiale in una nuova nota prima di chiudere".into()));
                    }
                } else {
                    tasks.push(self.save_failed("Nota spostata nel Cestino della cartella precedente: buffer correnti intatti, verifica i file".into()));
                }
            }
            Ok(DeleteDisposition::NeedsPermanentConfirmation) => {
                let error =
                    "Il Cestino non è disponibile: la nota non è stata eliminata".to_owned();
                self.show_notes_error(error.clone());
                tasks.push(self.notify_error(error));
            }
            Err(error) => {
                self.show_notes_error(error.clone());
                tasks.push(self.save_failed(error));
            }
        }
        for doc in da_ricontrollare {
            if !(self
                .scritture
                .guarda(Some((
                    doc,
                    self.workspace.get(doc).and_then(lume_core::Document::path),
                )))
                .file
                != crate::scritture::Presa::Libero
                || self.git_operazione.is_some())
            {
                tasks.push(self.reload_pending_fs_recheck(doc));
            }
        }
        Task::batch(tasks)
    }

    pub(super) fn finish_note_delete(&mut self, id: &NoteId) {
        let docs: Vec<DocId> = self
            .notes
            .as_ref()
            .into_iter()
            .flat_map(|notes| notes.tabs.iter())
            .filter_map(|(doc, note)| (note == id).then_some(*doc))
            .collect();
        for doc in docs {
            if self.chiede_salvataggio(doc) {
                continue;
            }
            self.lsp_did_close(doc);
            self.groups.close(doc);
            self.workspace.close(doc);
            self.syntax.remove(&doc);
            if let Some(notes) = self.notes.as_mut() {
                notes.tabs.remove(&doc);
            }
        }
        if let Some(notes) = self.notes.as_mut() {
            if notes.tabs.values().any(|note| note == id) {
                if let Some(loaded) = notes.loaded.get(id) {
                    notes
                        .open_saves
                        .insert(id.clone(), OpenNoteSaveState::from_loaded(loaded));
                }
            } else {
                notes.open_saves.remove(id);
                notes.titles.remove(id);
            }
            notes.loaded.remove(id);
            notes.summaries.retain(|summary| &summary.id != id);
        }
    }

    pub(super) fn close_all_note_tabs(&mut self) {
        let docs: Vec<DocId> = self
            .notes
            .as_ref()
            .into_iter()
            .flat_map(|notes| notes.tabs.keys().copied())
            .collect();
        for doc in docs {
            self.groups.close(doc);
            self.workspace.close(doc);
            self.syntax.remove(&doc);
        }
        if let Some(notes) = self.notes.as_mut() {
            notes.tabs.clear();
            notes.loaded.clear();
            notes.open_saves.clear();
            notes.titles.clear();
            notes.summaries.clear();
            notes.link_index = lume_notes::LinkIndex::default();
            notes.marcatori.clear();
            notes.store = None;
            notes.directory = None;
        }
    }

    /// Qualche scheda degli Appunti ha modifiche non salvate? Cambiare
    /// cartella o spegnere gli Appunti chiude **tutte** le loro schede in un
    /// colpo, e non è il momento di una domanda per ciascuna: si rifiuta.
    pub(super) fn appunti_sporchi(&self) -> bool {
        self.note_delete.is_some()
            || self.notes.as_ref().is_some_and(|notes| {
                notes.tabs.keys().any(|&doc| {
                    self.chiede_salvataggio(doc)
                        || (self
                            .scritture
                            .guarda(Some((
                                doc,
                                self.workspace.get(doc).and_then(lume_core::Document::path),
                            )))
                            .file
                            != crate::scritture::Presa::Libero
                            || self.git_operazione.is_some())
                })
            })
    }

    pub(super) const APPUNTI_SPORCHI: &'static str =
        "Ci sono Appunti con modifiche non salvate: salvali o chiudili prima";

    pub(super) fn switch_notes_directory(&mut self, directory: PathBuf) -> Task<Message> {
        if self.appunti_sporchi() {
            return self.notify_error(Self::APPUNTI_SPORCHI.to_owned());
        }
        // Prima di mutare il cambio, si riserva l'identità della scansione:
        // se è esaurita, la cartella e l'intento precedenti restano intatti.
        let Some(generation) = self.next_notes_scan_generation() else {
            return Task::none();
        };
        // L'esistenza dello stato è già garantita dalla stessa allocazione.
        if self.next_notes_generation().is_none() {
            return Task::none();
        }
        // I placeholder della libreria precedente non sono ancora schede
        // nota: ricordarne l'origine impedisce un successivo salvataggio come
        // file Markdown ordinario dopo che il DB avrà registrato la nuova.
        let precedente = self.notes_directory.clone();
        if precedente != directory {
            let placeholder: Vec<(DocId, bool)> = self
                .workspace
                .ids()
                .filter_map(|id| {
                    let pending = self.pending_restored_tabs.get(id)?;
                    pending
                        .path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
                        .then_some((id, pending.path.parent() == Some(precedente.as_path())))
                })
                .collect();
            for (id, diretto) in placeholder {
                // Le origini appartengono solo al placeholder e si liberano
                // al ripristino o alla chiusura. Il worker riconosce gli alias.
                let candidati = self.restored_notes_candidates.entry(id).or_default();
                if !candidati.contains(&precedente) {
                    candidati.push(precedente.clone());
                }
                if diretto {
                    self.restored_notes_revoked.insert(id, precedente.clone());
                    self.pending_restored_tabs.set_failure(id, "La cartella Appunti è cambiata: torna alla cartella precedente per ripristinare la nota".into());
                }
            }
        }
        self.notes_directory = directory.clone();
        self.notes_directory_scelta = Some(directory.clone());
        let annullato = self
            .notes
            .as_mut()
            .is_some_and(|notes| std::mem::take(&mut notes.create_after_load));
        self.close_all_note_tabs();
        if let Some(tool) = self.registry.get_mut(tools::notes::ID) {
            let _ = tool.update(&ToolEvent::new(tools::notes::Event::Loading(
                directory.clone(),
            )));
        }
        let avviso = if annullato {
            self.notify(
                NoticeKind::Info,
                "Nuovo appunto annullato: hai cambiato la cartella Appunti".into(),
            )
        } else {
            Task::none()
        };
        let save = self.store.clone().map_or_else(Task::none, |store| {
            let saved = directory.clone();
            Task::perform(
                async move {
                    store
                        .save_notes_directory(saved)
                        .await
                        .map_err(|e| e.to_string())
                },
                Message::NotesDirectorySaved,
            )
        });
        Task::batch([
            avviso,
            save,
            self.scan_notes_directory(generation, directory),
        ])
    }

    pub(super) fn notes_autosave_completed(
        &mut self,
        id: DocId,
        saved_version: u64,
    ) -> Task<Message> {
        let Some(current_version) = self.workspace.get(id).map(|doc| doc.version()) else {
            return Task::none();
        };
        let Some(state) = self
            .notes
            .as_mut()
            .and_then(|notes| notes.salvataggi.get_mut(&id))
        else {
            return Task::none();
        };
        Self::autosave_timer(id, state.completato(saved_version, current_version))
    }
}

/// Rilettura di una nota per il watcher: dal `NoteStore` arriva solo il corpo,
/// mai il file serializzato. Rifiuta un'identità diversa da quella della
/// scheda e le note illeggibili: la guardia di salvataggio resta valida.
/// Un file senza intestazione non ha un'identità serializzata: come nella
/// riapertura da disco, la scheda conserva la propria.
pub(crate) fn nota_da_rileggere(
    store: &NoteStore,
    nota: &NoteId,
    path: &Path,
) -> Result<LoadedNote, String> {
    let mut letta = store.load(path).map_err(|errore| errore.to_string())?;
    if let Some(errore) = &letta.read_only_error {
        return Err(format!(
            "la nota non è leggibile ({errore}): riaprila o correggi il file"
        ));
    }
    if intestazione_presente(&letta.original_bytes) {
        if letta.note.header.id != *nota {
            return Err(
                "la nota sul disco ha un'identità diversa da questa scheda: riaprila dalla libreria"
                    .to_owned(),
            );
        }
    } else {
        letta.note.header.id = nota.clone();
    }
    Ok(letta)
}

/// Se il file comincia con l'apertura di un front matter, anche con
/// terminazioni CRLF: lo stesso criterio di `format::parse`.
fn intestazione_presente(bytes: &[u8]) -> bool {
    let testo = String::from_utf8_lossy(bytes);
    testo.starts_with("+++\n") || testo.starts_with("+++\r\n")
}

/// Se `path` è un file `.md` direttamente nella cartella degli Appunti
/// `notes_dir`, lo legge tramite `NoteStore` così che il ripristino di
/// sessione separi l'intestazione `+++` dal corpo Markdown.
pub(crate) fn carica_nota_ripristinata(
    notes_dir: &Path,
    path: &Path,
) -> Option<Result<(PathBuf, NoteStore, LoadedNote), String>> {
    let is_md = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
    if !is_md || !notes_dir.is_dir() {
        return None;
    }
    let parent = path.parent()?;
    let in_notes_dir = parent == notes_dir
        || lume_core::canonical_path(parent)
            .ok()
            .zip(lume_core::canonical_path(notes_dir).ok())
            .is_some_and(|(a, b)| a == b);
    if !in_notes_dir {
        return None;
    }
    Some(
        NoteStore::open(notes_dir)
            .and_then(|store| {
                let loaded = store.load(path)?;
                Ok((notes_dir.to_path_buf(), store, loaded))
            })
            .map_err(|error| error.to_string()),
    )
}
