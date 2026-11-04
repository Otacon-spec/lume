//! Binding del chooser/receipt reali; il dialogo nativo non è pollato qui.
use super::*;
use futures::StreamExt;
async fn ready60<S>(stream: &mut S) -> crate::save::Ready
where
    S: futures::Stream<Item = iced_runtime::Action<Message>> + Unpin,
{
    match tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
        .await
        .unwrap()
        .unwrap()
    {
        iced_runtime::Action::Output(Message::SaveReady(ready)) => ready,
        _ => panic!("Ready autentico del worker assente"),
    }
}

async fn done60<S>(app: &mut Lume, stream: &mut S)
where
    S: futures::Stream<Item = iced_runtime::Action<Message>> + Unpin,
{
    let action = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
        .await
        .unwrap()
        .unwrap();
    let iced_runtime::Action::Output(message @ Message::SaveCollected(_)) = action else {
        panic!("ricevuta terminale autentica assente");
    };
    drop(app.update(message));
    assert!(stream.next().await.is_none());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[test]
fn ready_reale_autorizzato_da_update_preserva_edit_nuovi_e_rifiuta_replay() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo(&mut app);
    let path = dir.path().join("confermato.txt");
    let task = app.save_as(id, path.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let ready = ready60(&mut stream).await;
        assert!(!path.exists(), "prepare non pubblica la destinazione");
        assert!(app.workspace.get(id).unwrap().path().is_none());
        drop(app.update(Message::EditorIn(
            crate::editor_groups::GroupId::Primary,
            EditorCommand::Insert("Y".into()),
        )));
        let recent = app.workspace.get(id).unwrap().rope().to_string();
        assert!(
            app.workspace
                .get(id)
                .unwrap()
                .can_mark_snapshot_saved(&ready.metadata().snapshot)
        );
        let replay = ready.clone();
        drop(app.update(Message::SaveReady(ready)));
        drop(app.update(Message::SaveReady(replay)));
        done60(&mut app, &mut stream).await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "X");
        assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), recent);
        assert!(app.workspace.get(id).unwrap().is_dirty());
        assert_eq!(app.workspace.get(id).unwrap().path(), Some(path.as_path()));
        app.workspace.get_mut(id).unwrap().undo().unwrap();
        assert!(!app.workspace.get(id).unwrap().is_dirty());
    });
}

#[test]
fn ready_reale_stantio_per_epoche_mapping_reload_o_binding_aba_non_pubblica() {
    // Mutazioni controllate del modello; file/prepare/abort e update sono reali.
    for caso in 0..6 {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        let source = write_and_open(&mut app, dir.path(), "origine.txt", "origine");
        let id = app.active_doc().unwrap();
        let path = dir.path().join("protetto.txt");
        std::fs::write(&path, "proteggi").unwrap();
        let task = app.save_as(id, path.clone());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut stream = iced_runtime::task::into_stream(task).unwrap();
            let ready = ready60(&mut stream).await;
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "proteggi");
            match caso {
                0 => app.git_root_epoch += 1,
                1 => {
                    let lease = app.save_as_flight.as_ref().unwrap().lease;
                    app.scritture
                        .rilascia(lease, crate::scritture::Esito::NonAvviato);
                    assert_eq!(
                        app.scritture
                            .ammetti(crate::scritture::Op::Rinomina { docs: vec![] }),
                        Ok(crate::scritture::Permesso::Subito)
                    );
                }
                2 => app.close_epoch += 1,
                3 => {
                    app.workspace
                        .rebind_path(id, dir.path().join("passaggio.txt"))
                        .unwrap();
                    app.workspace.rebind_path(id, source.clone()).unwrap();
                }
                4 => {
                    app.workspace
                        .open(Some(dir.path().join("altro.txt")), "altro");
                }
                _ => {
                    app.workspace
                        .get_mut(id)
                        .unwrap()
                        .reload_from_str("esterna");
                }
            }
            let text = app.workspace.get(id).unwrap().rope().to_string();
            drop(app.update(Message::SaveReady(ready)));
            done60(&mut app, &mut stream).await;
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "proteggi",
                "caso {caso}"
            );
            assert_eq!(std::fs::read_to_string(&source).unwrap(), "origine");
            assert_eq!(
                app.workspace.get(id).unwrap().path(),
                Some(source.as_path())
            );
            assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), text);
            assert!(app.save_as_flight.is_none());
        });
    }
}

#[test]
fn granted_reale_revocato_dal_menu_esci_prima_di_commit_started_aborta() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo(&mut app);
    let path = dir.path().join("revocato.txt");
    let task = app.save_as(id, path.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let ready = ready60(&mut stream).await;
        let pause = app.save_job.as_ref().unwrap().pause().unwrap();
        drop(app.update(Message::SaveReady(ready)));
        assert!(app.save_as_permission.is_some());
        assert!(!path.exists());
        drop(app.update(Message::MenuAction(MenuAct::Exit)));
        assert!(
            (app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()),
            "flight conservato fino al ritorno del worker"
        );
        pause.finish(true);
        done60(&mut app, &mut stream).await;
        assert!(!path.exists());
        assert!(app.workspace.get(id).unwrap().path().is_none());
        assert!(app.workspace.get(id).unwrap().is_dirty());
    });
}

fn picker(app: &mut Lume, id: lume_core::DocId) -> u64 {
    drop(app.pick_save_path(id));
    app.save_as_picker.as_ref().unwrap().sequence
}
fn output(app: &mut Lume, id: lume_core::DocId, path: std::path::PathBuf) -> Message {
    let task = app.save_as(id, path);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        loop {
            let iced_runtime::Action::Output(m) = stream.next().await.unwrap() else {
                panic!("output assente");
            };
            if matches!(m, Message::SaveCollected(_)) {
                return m;
            }
            assert!(matches!(m, Message::SaveReady(_)));
            // Guidare la UI prima di attendere Done: nessun auto-commit fixture.
            drop(app.update(m));
        }
    })
}

#[test]
fn chooser_stantio_per_edits_non_scrive_ne_assegna_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo(&mut app);
    let ticket = picker(&mut app, id);
    let epoch = app.scritture.guarda(None).epoca;
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("Y".into()),
    ));
    let path = dir.path().join("rifiutato.txt");
    let _ = app.update(Message::SaveAsPicked(ticket, Some(path.clone())));
    assert!(!path.exists());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert_eq!(app.scritture.guarda(None).epoca, epoch + 1);
    assert!(app.workspace.get(id).unwrap().path().is_none());
    assert!(testo_avviso(&app).contains("scaduta"));
}
#[test]
fn chooser_precedente_non_consuma_successivo_e_replay_non_scrive() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo(&mut app);
    let first = picker(&mut app, id);
    drop(app.pick_save_path(id));
    assert_eq!(app.save_as_picker.as_ref().unwrap().sequence, first);
    assert!(testo_avviso(&app).contains("corso"));
    drop(app.update(Message::SaveAsPicked(first, None)));
    let next = picker(&mut app, id);
    let path = dir.path().join("valido.txt");
    let _ = app.update(Message::SaveAsPicked(first, Some(path.clone())));
    assert_eq!(app.save_as_picker.as_ref().unwrap().sequence, next);
    assert!(!path.exists());
    let task = app.update(Message::SaveAsPicked(next, Some(path.clone())));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "X");
    let other = dir.path().join("replay.txt");
    let _ = app.update(Message::SaveAsPicked(next, Some(other.clone())));
    assert!(!other.exists());
    assert_eq!(app.workspace.get(id).unwrap().path(), Some(path.as_path()));
}
#[test]
fn chooser_root_binding_aba_e_owner_stantio_sono_invalidanti() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo(&mut app);
    let path = dir.path().join("no.txt");
    let ticket = picker(&mut app, id);
    // Stato controllato root ABA, non switch di progetto GUI.
    app.git_root_epoch += 2;
    let _ = app.update(Message::SaveAsPicked(ticket, Some(path.clone())));
    assert!(!path.exists());
    let ticket = picker(&mut app, id);
    let lease = app.save_as_picker.as_ref().unwrap().lease;
    app.scritture
        .rilascia(lease, crate::scritture::Esito::NonAvviato);
    let successore = match app
        .scritture
        .ammetti(crate::scritture::Op::Cestino { docs: vec![] })
    {
        Ok(crate::scritture::Permesso::Avvia(lease)) => lease,
        altro => panic!("successore non ammesso: {altro:?}"),
    };
    let epoca = app.scritture.guarda(None).epoca;
    let _ = app.update(Message::SaveAsPicked(ticket, Some(path.clone())));
    assert!(!path.exists());
    assert!(
        app.scritture
            .possiede(successore, crate::scritture::Tipo::Cestino)
    );
    assert_eq!(app.scritture.guarda(None).epoca, epoca);
    // La risposta stantia non consuma il picker; una nuova app isola il caso ABA.
    let mut app = super::app();
    let id = nuovo(&mut app);
    app.workspace
        .rebind_path(id, dir.path().join("origine.txt"))
        .unwrap();
    let ticket = picker(&mut app, id);
    app.workspace
        .rebind_path(id, dir.path().join("passaggio.txt"))
        .unwrap();
    app.workspace
        .rebind_path(id, dir.path().join("origine.txt"))
        .unwrap();
    let _ = app.update(Message::SaveAsPicked(ticket, Some(path.clone())));
    assert!(!path.exists());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}
#[test]
fn chooser_overflow_e_cancel_non_modificano_file_o_saved() {
    let mut app = app();
    let id = nuovo(&mut app);
    let ticket = picker(&mut app, id);
    let _ = app.update(Message::SaveAsPicked(ticket, None));
    assert!(app.save_as_picker.is_none());
    assert!(app.workspace.get(id).unwrap().is_dirty());
    app.save_as_sequence = u64::MAX;
    drop(app.pick_save_path(id));
    assert!(app.save_as_picker.is_none());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(testo_avviso(&app).contains("esaurite"));
}
#[test]
fn chooser_legato_al_documento_originale_non_alla_scheda_attiva() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo(&mut app);
    let ticket = picker(&mut app, id);
    let altro = app.new_untitled().unwrap();
    let path = dir.path().join("primo.txt");
    let task = app.update(Message::SaveAsPicked(ticket, Some(path.clone())));
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "X");
    assert_eq!(app.active_doc(), Some(altro));
    assert_eq!(app.workspace.get(id).unwrap().path(), Some(path.as_path()));
    assert!(app.workspace.get(altro).unwrap().path().is_none());
}
#[test]
fn save_as_cold_drop_e_non_salvare_non_pubblicano() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo(&mut app);
    let path = dir.path().join("no.txt");
    let task = app.save_as(id, path.clone());
    let job = app.save_job.clone().unwrap();
    let _ = app.update(Message::MenuAction(MenuAct::Exit));
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    ));
    drop(task);
    let receipt = futures::executor::block_on(job.collect());
    let _ = app.update(Message::SaveCollected(receipt));
    assert!(!path.exists());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.save_as_flight.is_none());
}
#[test]
fn save_as_snapshot_preserva_edit_piu_recenti_e_rifiuta_save_sul_vecchio_nome() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let old = write_and_open(&mut app, dir.path(), "prima.txt", "prima");
    let id = app.active_doc().unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));
    let path = dir.path().join("dopo.txt");
    let done = output(&mut app, id, path.clone());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");
    assert_eq!(std::fs::read_to_string(&old).unwrap(), "prima");
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("Y".into()),
    ));
    let _ = app.update(Message::SaveDocFixture(id)); // Non accodare il salvataggio sul vecchio legame.
    assert!(testo_avviso(&app).contains("Salva con nome"));
    let _ = app.update(done);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(app.workspace.get(id).unwrap().rope().to_string(), "XYprima");
    assert_eq!(app.workspace.get(id).unwrap().path(), Some(path.as_path()));
    assert!(app.workspace.get_by_path(&old).is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xprima");
    assert!(app.workspace.get_mut(id).unwrap().undo().is_some());
    assert!(!app.workspace.get(id).unwrap().is_dirty());
}
#[test]
fn save_as_effetto_reale_e_mapping_cambiato_non_fingono_ack_o_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo(&mut app);
    let path = dir.path().join("scritto.txt");
    let done = output(&mut app, id, path.clone());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "X");
    // Violazione controllata della barriera model, non ammissione GUI.
    let other = app.workspace.open(Some(path.clone()), "altro");
    let _ = app.update(done);
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.workspace.get(id).unwrap().path().is_none());
    assert!(app.workspace.get(id).unwrap().is_dirty());
    assert_eq!(app.workspace.get_by_path(&path), Some(other));
    assert!(testo_avviso(&app).contains("Destinazione scritta"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "X");
}
#[test]
fn save_as_cold_non_salvare_con_task_ancora_vivo_revoca_la_scrittura() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo(&mut app);
    let path = dir.path().join("no.txt");
    let task = app.save_as(id, path.clone());
    let _ = app.update(Message::MenuAction(MenuAct::Exit));
    let _ = app.update(Message::ConfermaChiusura(
        crate::conferma_chiusura::ConfermaMessage::NonSalvare,
    ));
    app.completa_salvataggi(task);
    assert!(!path.exists());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
    assert!(app.save_as_flight.is_none());
}

#[test]
fn save_as_folder_picked_tardivo_non_sposta_root_prima_della_scrittura() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo(&mut app);
    let root = app.project_root.clone();
    let epoch = app.git_root_epoch;
    let path = dir.path().join("nuovo.txt");
    let task = app.save_as(id, path.clone());
    let _ = app.update(Message::FolderPicked(Some(dir.path().to_path_buf())));
    assert_eq!(app.project_root, root);
    assert_eq!(app.git_root_epoch, epoch);
    assert!(!path.exists());
    app.notice = None;
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "X");
    assert!(!app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn save_as_admission_blocca_open_alias_e_altro_writer() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo(&mut app);
    let path = dir.path().join("scritto.txt");
    let task = app.save_as(id, path.clone());
    let other = app.new_untitled().unwrap();
    drop(app.save_as(other, dir.path().join("altro.txt")));
    assert!(
        app.save_as_flight
            .as_ref()
            .is_some_and(|r| r.ticket.document == id)
    );
    assert_eq!(
        app.check_file_binding(&path).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    app.notice = None;
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "X");
}
#[cfg(unix)]
#[test]
fn save_as_hardlink_proprio_e_copia_al_nome_nuovo_non_save_sul_vecchio() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let old = write_and_open(&mut app, dir.path(), "old.txt", "prima");
    let id = app.active_doc().unwrap();
    let alias = dir.path().join("alias.txt");
    std::fs::hard_link(&old, &alias).unwrap();
    let _ = app.update(Message::EditorIn(
        crate::editor_groups::GroupId::Primary,
        EditorCommand::Insert("X".into()),
    ));
    let task = app.save_as(id, alias.clone());
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(&old).unwrap(), "prima");
    assert_eq!(std::fs::read_to_string(&alias).unwrap(), "Xprima");
    assert_eq!(app.workspace.get(id).unwrap().path(), Some(alias.as_path()));
    assert!(app.workspace.get_by_path(&old).is_none());
    assert!(!app.workspace.get(id).unwrap().is_dirty());
}

#[test]
fn chooser_dopo_close_reopen_non_riconosce_docid_o_testo_simile() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let id = nuovo(&mut app);
    let ticket = picker(&mut app, id);
    app.groups.close(id);
    app.workspace.close(id).unwrap();
    let next = app.new_untitled().unwrap();
    assert_ne!(next, id);
    let path = dir.path().join("no.txt");
    drop(app.update(Message::SaveAsPicked(ticket, Some(path.clone()))));
    assert!(!path.exists());
    assert!(app.workspace.get(next).unwrap().path().is_none());
    assert!(!(app.scritture.guarda(None).in_volo.is_some() || app.git_operazione.is_some()));
}

#[cfg(unix)]
#[test]
fn save_as_private_bom_crlf_non_utf8_e_readonly_reali() {
    use std::{
        ffi::OsString,
        os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    };
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    let old = write_and_open(&mut app, dir.path(), "old.txt", "\u{feff}prima\r\n");
    let id = app.active_doc().unwrap();
    // APFS rifiuta per costruzione i nomi non UTF-8 (EILSEQ): su macOS resta
    // la barra rovesciata, altrove anche il byte non UTF-8.
    let nome: &[u8] = if cfg!(target_vendor = "apple") {
        b"nuovo\\x.txt"
    } else {
        b"nuovo\\\xff.txt"
    };
    let path = dir.path().join(OsString::from_vec(nome.to_vec()));
    let task = app.save_as(id, path.clone());
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read(&path).unwrap(), b"\xef\xbb\xbfprima\r\n");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(app.workspace.get(id).unwrap().path(), Some(path.as_path()));
    assert_eq!(std::fs::read(&old).unwrap(), b"\xef\xbb\xbfprima\r\n");
    let readonly = dir.path().join("protetto.txt");
    std::fs::write(&readonly, "non cambiare").unwrap();
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o444)).unwrap();
    let task = app.save_as(id, readonly.clone());
    app.completa_salvataggi(task);
    assert_eq!(std::fs::read_to_string(&readonly).unwrap(), "non cambiare");
    assert_eq!(app.workspace.get(id).unwrap().path(), Some(path.as_path()));
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o600)).unwrap();
}
