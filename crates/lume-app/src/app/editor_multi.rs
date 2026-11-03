//! Coordinamento dei gesti multipli; nessuno stato di selezione nella vista.
use super::*;

impl Lume {
    pub(super) fn request_editor_focus(&mut self) {
        self.editor_focus_epoch = self.editor_focus_epoch.wrapping_add(1);
    }

    pub(super) fn pending_editor_input(&self, id: DocId) -> bool {
        self.restored_tab_commands
            .get(&id)
            .is_some_and(|queue| !queue.is_empty())
    }
    pub(super) fn restored_input_failed(&self, id: DocId) -> bool {
        self.pending_restored_tabs
            .get(id)
            .is_some_and(|tab| tab.failure.is_some())
            && !self.restored_tab_reads.contains_key(&id)
    }
    pub(super) fn editor_multi_edit(
        &mut self,
        id: DocId,
        edit: lume_core::multi_edit::MultiEdit<'_>,
    ) {
        let Some(doc) = self.workspace.get_mut(id) else {
            return;
        };
        match lume_core::multi_edit::apply(doc, edit) {
            Ok(Some(edit)) => {
                self.resync_syntax(id, edit);
                self.workspace.trim_histories_to_budget();
            }
            Ok(None) => {}
            Err(error) => self.notify_persistente(NoticeKind::Error, error.to_string()),
        }
    }
}

#[derive(Clone)]
pub(crate) struct OccurrenceTicket {
    pub(super) sequence: u64,
    pub(super) doc: DocId,
    pub(super) snapshot: lume_core::SaveSnapshot,
    pub(super) root_epoch: u64,
    pub(super) expected: lume_core::SelectionSet,
}
impl std::fmt::Debug for OccurrenceTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OccurrenceTicket")
            .field("sequence", &self.sequence)
            .field("doc", &self.doc)
            .finish_non_exhaustive()
    }
}
impl Lume {
    pub(super) fn next_occurrence_for(&mut self, id: DocId) -> Task<Message> {
        if self.pending_restored_tabs.contains(id) {
            let _ = self.queue_restored_editor_command(id, EditorCommand::AddNextOccurrence);
            return self.restored_tab_read_task(id, false);
        }
        if self.multi_occurrence.is_some() {
            self.notify_persistente(NoticeKind::Info, "Ricerca occorrenza già in corso".into());
            return Task::none();
        }
        let Some(doc) = self.workspace.get(id) else {
            return Task::none();
        };
        let Some(sequence) = self.multi_occurrence_sequence.checked_add(1) else {
            return self.notify_error("Contatore delle ricerche esaurito".into());
        };
        self.multi_occurrence_sequence = sequence;
        let ticket = OccurrenceTicket {
            sequence,
            doc: id,
            snapshot: doc.read_snapshot(),
            root_epoch: self.git_root_epoch,
            expected: doc.selections().clone(),
        };
        let snapshot = ticket.snapshot.clone();
        let expected = ticket.expected.clone();
        self.multi_occurrence = Some(ticket.clone());
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    lume_core::occurrence::next(snapshot.rope(), &expected)
                })
                .await
                .unwrap_or(Err(
                    lume_core::occurrence::SelectionActionError::SearchFailed,
                ))
            },
            move |result| Message::OccurrenceSelected(ticket.clone(), result),
        )
    }

    pub(super) fn occurrence_selected(
        &mut self,
        ticket: OccurrenceTicket,
        result: Result<lume_core::SelectionSet, lume_core::occurrence::SelectionActionError>,
    ) -> Task<Message> {
        if !self
            .multi_occurrence
            .as_ref()
            .is_some_and(|current| current.sequence == ticket.sequence && current.doc == ticket.doc)
        {
            self.notify_persistente(NoticeKind::Info, "Risultato di ricerca già superato".into());
            return Task::none();
        }
        self.multi_occurrence = None;
        let valid = self.git_root_epoch == ticket.root_epoch
            && self.workspace.get(ticket.doc).is_some_and(|doc| {
                doc.matches_snapshot(&ticket.snapshot) && doc.selections() == &ticket.expected
            });
        if !valid {
            self.notify_persistente(
                NoticeKind::Info,
                "Ricerca scartata: documento o selezioni cambiati".into(),
            );
        } else {
            match result {
                Ok(set) => {
                    if let Some(doc) = self.workspace.get_mut(ticket.doc) {
                        doc.set_selections(set);
                    }
                }
                Err(error) => self.notify_persistente(NoticeKind::Info, error.to_string()),
            }
        }
        self.replay_restored_commands(ticket.doc)
    }

    /// Ferma il replay sulla ricerca: il seguito appartiene al risultato,
    /// non al reader che ha appena consegnato il testo.
    pub(super) fn replay_restored_commands(&mut self, id: DocId) -> Task<Message> {
        while let Some(command) = self
            .restored_tab_commands
            .get_mut(&id)
            .and_then(VecDeque::pop_front)
        {
            if command == EditorCommand::AddNextOccurrence {
                if self.multi_occurrence.is_some() {
                    self.notify_persistente(NoticeKind::Info, "Ricerca occorrenza rifiutata: un'altra ricerca è in corso; input successivo conservato".into());
                    continue;
                }
                return self
                    .resume_restored_tab_action(id, DeferredRestoredAction::SelectNextOccurrence);
            }
            self.apply_editor_command_for(id, command);
        }
        self.restored_tab_commands.remove(&id);
        Task::batch([self.lsp_change_task(), self.restored_tab_action_task(id)])
    }

    pub(super) fn editor_selection_action(&mut self, id: DocId, command: EditorCommand) {
        let Some(doc) = self.workspace.get_mut(id) else {
            return;
        };
        let result = match command {
            EditorCommand::ToggleCursor { pos } => {
                lume_core::occurrence::toggle(doc.selections(), pos, doc.rope().len_chars())
            }
            EditorCommand::AddCursorLine { above } => {
                lume_core::occurrence::adjacent(doc.rope(), doc.selections(), above)
            }
            EditorCommand::KeepPrimary => Ok(doc.selections().keep_primary()),
            _ => return,
        };
        match result {
            Ok(set) => doc.set_selections(set),
            Err(error) => self.notify_persistente(NoticeKind::Info, error.to_string()),
        }
    }
}

/// Navigazione/LSP sostituiscono solo il principale; la normalizzazione
/// dell'insieme resta responsabilità del modello.
pub(super) fn set_primary(doc: &mut lume_core::Document, range: Range) {
    let primary = doc.selections().primary_index();
    let mut ranges = doc.selections().ranges().to_vec();
    if let Some(main) = ranges.get_mut(primary) {
        *main = range;
    }
    if let Ok(set) = lume_core::SelectionSet::try_new(ranges, primary, doc.rope().len_chars()) {
        doc.set_selections(set);
    }
}
