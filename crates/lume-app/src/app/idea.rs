//! Importazione esplicita dei profili JetBrains, senza aprire connessioni.
use super::*;

impl Lume {
    pub(super) fn clear_idea(&mut self) {
        self.idea_request = self.idea_request.wrapping_add(1);
        self.idea_pending = None;
        self.idea_preview(Vec::new(), Vec::new());
    }

    fn idea_preview(&mut self, profiles: Vec<(String, String)>, skipped: Vec<String>) {
        if let Some(tool) = self.registry.get_mut(tools::db::ID) {
            tool.update(&ToolEvent::new(tools::db::Event::IdeaPreview {
                profiles,
                skipped,
            }));
        }
    }

    pub(super) fn pick_idea(&mut self) -> Task<Message> {
        self.clear_idea();
        let request = self.idea_request;
        let root = self.project_root.clone();
        Task::perform(
            async move {
                let mut dialog = rfd::AsyncFileDialog::new()
                    .set_title("Importa connessioni da .idea/dataSources.xml")
                    .add_filter("Configurazione JetBrains", &["xml"]);
                if let Some(root) = root {
                    dialog = dialog.set_directory(root.join(".idea"));
                }
                dialog
                    .pick_file()
                    .await
                    .map(|file| file.path().to_path_buf())
            },
            move |path| Message::IdeaPicked(request, path),
        )
    }

    pub(super) fn detect_idea(&mut self) -> Task<Message> {
        if !self.registry.is_available(tools::db::ID) {
            return Task::none();
        }
        let Some(root) = self.project_root.as_ref() else {
            return Task::none();
        };
        self.read_idea(self.idea_request, root.join(".idea/dataSources.xml"))
    }

    pub(super) fn read_idea(&self, request: u64, path: PathBuf) -> Task<Message> {
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let report = crate::idea_import::load(&path).map_err(str::to_owned)?;
                    let source = path.canonicalize().unwrap_or(path);
                    Ok((source, report))
                })
                .await
                .unwrap_or_else(|_| Err("Lettura .idea interrotta".into()))
            },
            move |result| Message::IdeaLoaded(request, result),
        )
    }

    pub(super) fn idea_loaded(
        &mut self,
        request: u64,
        result: Result<(PathBuf, crate::idea_import::Report), String>,
    ) -> Task<Message> {
        if request != self.idea_request {
            return Task::none();
        }
        match result {
            Err(error) => self.notify_error(error),
            Ok((source, mut report)) => {
                report.connections.retain(|c| {
                    !self
                        .sql_profiles
                        .iter()
                        .any(|p| p.id == imported_id(&source, &c.uuid))
                });
                if report.connections.is_empty() && report.skipped.is_empty() {
                    return Task::none();
                }
                self.idea_preview(preview_profiles(&report), report.skipped.clone());
                self.idea_pending = Some((source, report));
                let requests = self.registry.set_active_panel(Some(tools::db::ID));
                self.pending_tool_requests.extend(requests);
                self.notify_persistente(
                    NoticeKind::Info,
                    "Connessioni .idea rilevate: conferma l’importazione nel pannello Database"
                        .into(),
                );
                self.drain_tool_requests()
            }
        }
    }

    pub(super) fn set_idea_user(&mut self, index: usize, user: String) {
        if let Some((_, report)) = self.idea_pending.as_mut() {
            if let Some(profile) = report.connections.get_mut(index) {
                profile.user = user;
            }
            let profiles = preview_profiles(report);
            let skipped = report.skipped.clone();
            self.idea_preview(profiles, skipped);
        }
    }

    pub(super) fn confirm_idea(&mut self) -> Task<Message> {
        if self
            .idea_pending
            .as_ref()
            .is_some_and(|(_, r)| r.connections.iter().any(|c| c.user.trim().is_empty()))
        {
            return self
                .notify_error("Completa l’utente database prima di importare i profili".into());
        }
        let Some((source, report)) = self.idea_pending.take() else {
            return Task::none();
        };
        self.clear_idea();
        let mut tasks = Vec::new();
        for c in report.connections {
            let id = imported_id(&source, &c.uuid);
            if self.sql_profiles.iter().any(|p| p.id == id) {
                continue;
            }
            let profile = lume_sql::ConnectionProfile {
                id,
                label: c.label,
                engine: if c.oracle {
                    lume_sql::Engine::Oracle
                } else {
                    lume_sql::Engine::AzureSql
                },
                host: c.host,
                database: c.database,
                auth: lume_sql::AuthKind::SqlLogin {
                    user: c.user.trim().to_owned(),
                },
            };
            tasks.push(self.persist_sql_connection(sql_record_from_profile(&profile)));
            self.sql_profiles.push(profile);
        }
        self.sync_db_tool();
        tasks.push(self.notify(
            NoticeKind::Info,
            "Profili .idea importati; le credenziali saranno richieste al collegamento".into(),
        ));
        Task::batch(tasks)
    }
}

fn imported_id(source: &Path, uuid: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(source.as_os_str().as_encoded_bytes());
    hash.update([0]);
    hash.update(uuid.as_bytes());
    format!("idea-{:x}", hash.finalize())
}

fn preview_profiles(report: &crate::idea_import::Report) -> Vec<(String, String)> {
    report
        .connections
        .iter()
        .map(|c| {
            (
                format!(
                    "{} · {} [{}]",
                    c.label,
                    c.database,
                    c.uuid.chars().take(8).collect::<String>()
                ),
                c.user.clone(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::*;

    fn report() -> crate::idea_import::Report {
        crate::idea_import::Report {
            connections: vec![crate::idea_import::Connection {
                uuid: "one".into(),
                label: "Work".into(),
                oracle: false,
                host: "localhost:1433".into(),
                database: "work".into(),
                user: "alice".into(),
            }],
            skipped: Vec::new(),
        }
    }

    #[test]
    fn conferma_importa_una_volta_senza_aprire_socket() {
        let (mut app, _) = Lume::new_without_store();
        let source = PathBuf::from("/fixture/.idea/dataSources.xml");
        app.idea_pending = Some((source.clone(), report()));
        let _ = app.confirm_idea();
        assert_eq!(app.sql_profiles.len(), 1);
        assert!(app.sql.is_none());
        app.idea_pending = Some((source, report()));
        let _ = app.confirm_idea();
        assert_eq!(app.sql_profiles.len(), 1);
    }

    #[test]
    fn risultato_superato_non_propone_importazione() {
        let (mut app, _) = Lume::new_without_store();
        let request = app.idea_request;
        app.clear_idea();
        let _ = app.idea_loaded(
            request,
            Ok((PathBuf::from("/old/dataSources.xml"), report())),
        );
        assert!(app.idea_pending.is_none());
        assert!(app.sql_profiles.is_empty());
    }

    #[test]
    fn uuid_uguali_in_progetti_diversi_non_collidono() {
        assert_ne!(
            imported_id(Path::new("/one/dataSources.xml"), "a"),
            imported_id(Path::new("/two/dataSources.xml"), "a")
        );
        assert_eq!(
            imported_id(Path::new("/one/dataSources.xml"), "a"),
            imported_id(Path::new("/one/dataSources.xml"), "a")
        );
    }

    #[test]
    fn utente_mancante_richiede_completamento_prima_di_importare() {
        let (mut app, _) = Lume::new_without_store();
        let mut pending = report();
        pending.connections[0].user.clear();
        app.idea_pending = Some((PathBuf::from("/fixture/dataSources.xml"), pending));
        let _ = app.confirm_idea();
        assert!(app.sql_profiles.is_empty());
        assert!(app.idea_pending.is_some());
    }

    #[test]
    fn annullamento_revoca_letture_e_conferme() {
        let (mut app, _) = Lume::new_without_store();
        let request = app.idea_request;
        app.idea_pending = Some((PathBuf::from("/fixture/dataSources.xml"), report()));
        app.clear_idea();
        let _ = app.confirm_idea();
        let _ = app.idea_loaded(
            request,
            Ok((PathBuf::from("/fixture/dataSources.xml"), report())),
        );
        assert!(app.idea_pending.is_none());
        assert!(app.sql_profiles.is_empty());
    }

    #[test]
    fn importazione_manuale_non_chiude_il_pannello_database() {
        let (mut app, _) = Lume::new_without_store();
        let _ = app.registry.set_active_panel(Some(tools::db::ID));
        let _ = app.idea_loaded(
            app.idea_request,
            Ok((PathBuf::from("/fixture/dataSources.xml"), report())),
        );
        assert_eq!(app.registry.active_panel, Some(tools::db::ID));
        assert!(app.idea_pending.is_some());
    }
}
