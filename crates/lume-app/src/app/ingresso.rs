//! Classificazione unica dei messaggi: ingresso modale e punto di persistenza.
use super::*;
use crate::overlay_manager::Ingresso;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Persistenza {
    Nessuna,
    DopoGestione,
    /// Il gestore decide se persistere in base ad azione e stato: la coda
    /// comune non aggiunge scritture. Per i contenitori la classe è una
    /// sovrastima delle possibilità del gestore, non una promessa per ogni
    /// variante (per esempio MenuBar(Escape) non scrive).
    NelGestore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PoliticaMessaggio {
    pub ingresso: Ingresso,
    pub persistenza: Persistenza,
}

impl Message {
    pub(super) fn politica(&self) -> PoliticaMessaggio {
        use Ingresso::*;
        use Persistenza::*;
        let (ingresso, persistenza) = match self {
            Self::SyntaxViewport(_) | Self::Syntax(_) => (Asincrono, Nessuna),
            Self::Forge(_) => (Gesto, Nessuna),
            Self::Configurazioni(_) => (Gesto, Nessuna),
            Self::Tool(id, evento) => (
                if *id == tools::terminal::ID
                    && matches!(
                        evento.downcast::<tools::terminal::Event>(),
                        Some(tools::terminal::Event::Tick)
                    )
                {
                    Asincrono
                } else {
                    Gesto
                },
                NelGestore,
            ),
            Self::Dock(m) => match m {
                DockMessage::FeatureStart(_)
                | DockMessage::FeatureStop(_)
                | DockMessage::FeatureStopConfirmed(_) => (Gesto, NelGestore),
                _ => (Gesto, DopoGestione),
            },
            Self::ClosePanel | Self::ShowExtensions => (Gesto, DopoGestione),
            Self::MenuBar(m) => match m {
                MenuBarMessage::HoverClassic(_) | MenuBarMessage::HoverSub(_) => (Sfondo, Nessuna),
                MenuBarMessage::Activate(MenuAct::Exit) => (ChiusuraFinestra, NelGestore),
                _ => (Gesto, NelGestore),
            },
            Self::CloseAll => (Annulla, Nessuna),
            Self::CopyAbout => (Proprio(Modale::About), Nessuna),
            Self::GlobalSearch(m) => match m {
                GlobalSearchMessage::ShiftPressed => (Sfondo, Nessuna),
                GlobalSearchMessage::Accept | GlobalSearchMessage::Activate(_) => {
                    (Gesto, NelGestore)
                }
                _ => (Gesto, Nessuna),
            },
            Self::Statusbar(m) => match m {
                StatusbarMessage::Environment(EnvironmentPickerMessage::Select(_))
                | StatusbarMessage::ShowExtensions => (Gesto, DopoGestione),
                _ => (Gesto, Nessuna),
            },
            Self::Settings(m) => match m {
                SettingsMessage::ColorThemePicked(_) | SettingsMessage::IconThemePicked(_) => {
                    (Gesto, DopoGestione)
                }
                _ => (Gesto, Nessuna),
            },
            Self::ActivityBar(m) => match m {
                ActivityBarMessage::Hover(_) | ActivityBarMessage::CursorMoved(_) => {
                    (Sfondo, Nessuna)
                }
                ActivityBarMessage::Release => (Gesto, DopoGestione),
                _ => (Gesto, Nessuna),
            },
            Self::Titlebar(m) => match m {
                TitlebarMessage::HoverClassic(_) => (Sfondo, Nessuna),
                TitlebarMessage::Drag
                | TitlebarMessage::DoubleClick
                | TitlebarMessage::Minimize
                | TitlebarMessage::ToggleMaximize => (Finestra, Nessuna),
                TitlebarMessage::Close => (ChiusuraFinestra, NelGestore),
                _ => (Gesto, Nessuna),
            },
            Self::FolderPicked(root) => {
                (Asincrono, if root.is_some() { NelGestore } else { Nessuna })
            }
            #[cfg(feature = "sql")]
            Self::IdeaPicked(..) | Self::IdeaLoaded(..) => (Asincrono, Nessuna),
            #[cfg(test)]
            Self::SaveAsPicked(..) => (Asincrono, NelGestore),
            #[cfg(test)]
            Self::SaveDocFixture(_) | Self::SaveAllFixture => (Gesto, Nessuna),
            Self::SavePickerCollected(_) => (Asincrono, NelGestore),
            Self::NotesDirectoryPicked(_)
            | Self::NotesPreferenceLoaded(..)
            | Self::NotesScanned(..)
            | Self::NotesOpened(..)
            | Self::NotesReadFinished(..)
            | Self::NotesDeletePrepared(..)
            | Self::NotesDeleted(..)
            | Self::NotesAutosaveDue(..)
            | Self::NotesDirectorySaved(_) => (Asincrono, Nessuna),
            Self::NotesOpenLinked(_)
            | Self::NotesOpenCode { .. }
            | Self::NotesRemoveLink { .. }
            | Self::NotesResolveConflict { .. }
            | Self::NewNote => (Gesto, Nessuna),
            Self::EditorTabBar(m) => match m {
                EditorTabBarMessage::Hover(..) | EditorTabBarMessage::CursorMoved(_) => {
                    (Sfondo, Nessuna)
                }
                EditorTabBarMessage::Close(_) | EditorTabBarMessage::Release => {
                    (Gesto, DopoGestione)
                }
                _ => (Gesto, Nessuna),
            },
            Self::LspResolved(..)
            | Self::LspInstalled(..)
            | Self::LspInstallProgress(..)
            | Self::Lsp(..)
            | Self::Debug(..)
            | Self::DebugSetup(..)
            | Self::DebugSnippet(..)
            | Self::EseguiCompilato(..)
            | Self::ConfigurazioniSalvate(..)
            | Self::LspChangeDebounced(..)
            | Self::LspChangeRetry(..)
            | Self::HoverDue(..) => (Asincrono, Nessuna),
            #[cfg(feature = "mcp")]
            Self::Mcp(..)
            | Self::McpModeLoaded(..)
            | Self::McpModeSaved(_)
            | Self::McpOpenFinished { .. }
            | Self::McpDocReloaded(..) => (Asincrono, Nessuna),
            #[cfg(all(feature = "mcp", not(test)))]
            Self::McpRenamePrepared { .. } => (Asincrono, Nessuna),
            #[cfg(feature = "mcp")]
            Self::ConsensoMcp(_) => (Proprio(Modale::ConsensoMcp), Nessuna),
            #[cfg(feature = "mcp")]
            Self::McpCard(_) => (Proprio(Modale::CardMcp), Nessuna),
            #[cfg(feature = "mcp")]
            Self::McpSetMode(_) => (Gesto, Nessuna),
            Self::Rename(_)
            | Self::GotoLine(_)
            | Self::Albero(_)
            | Self::FoldToggle
            | Self::UnfoldAll
            | Self::CodeActionChosen(_) => (Gesto, Nessuna),
            #[cfg(not(test))]
            Self::WorkspaceEditPrepared { .. } => (Asincrono, Nessuna),
            Self::AlberoElimina(_) => (Proprio(Modale::ConfermaElimina), Nessuna),
            Self::AlberoRinomina(_) => (Gesto, NelGestore),
            Self::AlberoEliminato(..) | Self::OccurrenceSelected(..) => (Asincrono, Nessuna),
            Self::RestoredTabAction(..) => (Asincrono, NelGestore),
            Self::Recents(_) | Self::Find(_) => (Gesto, Nessuna),
            Self::EditorIn(_, cmd) => match cmd {
                EditorCommand::HoverAt(_) | EditorCommand::KeepPrimary => (Sfondo, Nessuna),
                _ => (Gesto, Nessuna),
            },
            Self::SaveActive => (Gesto, Nessuna),
            #[cfg(test)]
            Self::SaveFinished(_) => (Asincrono, NelGestore),
            Self::SaveCollected(_) | Self::SaveReady(_) => (Asincrono, NelGestore),
            Self::ReplaceReady(_) | Self::ReplaceCollected(_) => (Asincrono, Nessuna),
            Self::MenuAction(action) => match action {
                MenuAct::Exit => (ChiusuraFinestra, NelGestore),
                _ => (Gesto, NelGestore),
            },
            Self::ToggleBookmark | Self::ClipboardPasted(_) => (Gesto, Nessuna),
            Self::SearchUpdate(..)
            | Self::SearchCollected(..)
            | Self::SearchDebounced(..)
            | Self::NoticeExpired(_)
            | Self::UpdateChecked(_)
            | Self::UpdateDownloaded(_) => (Asincrono, Nessuna),
            Self::SearchExitReady(..) | Self::FileReadFinished(..) => (Asincrono, NelGestore),
            Self::RestoredTabReadFinished(..)
            | Self::DocRicaricato(..)
            | Self::GlobalFileSearch(..) => (Asincrono, Nessuna),
            #[cfg(test)]
            Self::FileLetto(_) => (Asincrono, NelGestore),
            Self::GitLoaded(..) => (Asincrono, Nessuna),
            Self::GitWriteFinished(..) => (Asincrono, NelGestore),
            Self::GitLogLoaded(..)
            | Self::JjLoaded(..)
            | Self::JjLoadCollected(_)
            | Self::JjActionDone(..)
            | Self::JjActionCollected(_)
            | Self::GitBranchLoaded(..)
            | Self::IdleElapsed(..) => (Asincrono, Nessuna),
            #[cfg(feature = "sql")]
            Self::SqlSecretRecalled { .. }
            | Self::SqlVaultWritten(_)
            | Self::SqlResponse { .. }
            | Self::SqlQueryEvent { .. } => (Asincrono, Nessuna),
            Self::Feature(_) | Self::FeatureStopConfirmed(_) => (Gesto, NelGestore),
            Self::FeatureStopCancelled => (Gesto, Nessuna),
            #[cfg(feature = "sql")]
            Self::Sql(_) => (Gesto, Nessuna),
            Self::GitCommitFilesLoaded(..) | Self::DiffLoaded { .. } => (Asincrono, Nessuna),
            Self::CloseDiff
            | Self::StageHunk { .. }
            | Self::UnstageHunk { .. }
            | Self::SwitchDiffBase(_) => (Gesto, Nessuna),
            Self::FsChanged(_)
            | Self::FsRescan(_)
            | Self::FsWatchFailed(..)
            | Self::EnvironmentsDetected(_) => (Asincrono, Nessuna),
            Self::WindowResizeEdge(_) | Self::WindowResized(_) => (Finestra, Nessuna),
            Self::DragStart(_) | Self::DragMoved(_) => (Sfondo, Nessuna),
            Self::DragEnd => (Sfondo, DopoGestione),
            Self::SessionSaved(_)
            | Self::GitBlameLoaded(..)
            | Self::EditorBlameLoaded(..)
            | Self::GitStashLoaded(..)
            | Self::PeakMemoryRead(_) => (Asincrono, Nessuna),
            Self::ConfermaChiusura(_) => (Proprio(Modale::ConfermaChiusura), NelGestore),
            Self::WindowCloseRequested(_) => (ChiusuraFinestra, NelGestore),
        };
        PoliticaMessaggio {
            ingresso,
            persistenza,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay_manager::Ingresso::*;
    use Persistenza::*;

    #[test]
    fn messaggi_rappresentativi_coprono_ingressi_e_punti_di_persistenza() {
        let casi = [
            (Message::CloseAll, Annulla, Nessuna),
            (Message::CopyAbout, Proprio(Modale::About), Nessuna),
            (
                Message::ConfermaChiusura(crate::conferma_chiusura::ConfermaMessage::Annulla),
                Proprio(Modale::ConfermaChiusura),
                NelGestore,
            ),
            (
                Message::AlberoElimina(crate::conferma_elimina::ConfermaMessage::Annulla),
                Proprio(Modale::ConfermaElimina),
                Nessuna,
            ),
            (Message::MenuAction(MenuAct::NewFile), Gesto, NelGestore),
            (
                Message::MenuAction(MenuAct::Exit),
                ChiusuraFinestra,
                NelGestore,
            ),
            (
                Message::MenuBar(MenuBarMessage::Activate(MenuAct::Exit)),
                ChiusuraFinestra,
                NelGestore,
            ),
            (
                Message::Titlebar(TitlebarMessage::Close),
                ChiusuraFinestra,
                NelGestore,
            ),
            (Message::Titlebar(TitlebarMessage::Drag), Finestra, Nessuna),
            (
                Message::GlobalSearch(GlobalSearchMessage::ShiftPressed),
                Sfondo,
                Nessuna,
            ),
            (
                Message::GlobalSearch(GlobalSearchMessage::AnyKey),
                Gesto,
                Nessuna,
            ),
            (Message::DragEnd, Sfondo, DopoGestione),
            (
                Message::WindowCloseRequested(iced::window::Id::unique()),
                ChiusuraFinestra,
                NelGestore,
            ),
            (
                Message::GlobalSearch(GlobalSearchMessage::Navigate(1)),
                Gesto,
                Nessuna,
            ),
            (
                Message::SyntaxViewport(syntax::ViewportRequest {
                    group: crate::editor_groups::GroupId::Primary,
                    document: DocId(1),
                    epoch: 1,
                    revision: 1,
                    window: 0..1,
                }),
                Asincrono,
                Nessuna,
            ),
            #[cfg(feature = "sql")]
            (
                Message::Sql(crate::sql_console::SqlMessage::GridScrolled {
                    pane: 0,
                    offset_y: 2.0,
                }),
                Gesto,
                Nessuna,
            ),
            (
                Message::FolderPicked(Some(PathBuf::from("progetto"))),
                Asincrono,
                NelGestore,
            ),
            (Message::FolderPicked(None), Asincrono, Nessuna),
            (Message::SessionSaved(Ok(())), Asincrono, Nessuna),
            (Message::ToggleBookmark, Gesto, Nessuna),
            (
                Message::Tool(
                    tools::terminal::ID,
                    ToolEvent::new(tools::terminal::Event::Tick),
                ),
                Asincrono,
                NelGestore,
            ),
            (
                Message::Tool(
                    tools::terminal::ID,
                    ToolEvent::new(tools::terminal::Event::Input(vec![1])),
                ),
                Gesto,
                NelGestore,
            ),
            (
                Message::Tool(
                    tools::notes::ID,
                    ToolEvent::new(tools::terminal::Event::Tick),
                ),
                Gesto,
                NelGestore,
            ),
        ];
        for (messaggio, ingresso, persistenza) in casi {
            assert_eq!(
                messaggio.politica(),
                PoliticaMessaggio {
                    ingresso,
                    persistenza
                },
                "{messaggio:?}"
            );
        }
    }
}
