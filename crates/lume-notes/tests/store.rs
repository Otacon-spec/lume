#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

use lume_notes::{
    CodeLink, MAX_NOTE_BYTES, NoteError, NoteStore, SCAN_PREFIX_BYTES, SaveIsolation,
    filter_summaries,
};
use std::fs;
use std::path::Path;

fn store_at(path: &Path) -> NoteStore {
    NoteStore::open(path).unwrap()
}

#[test]
fn il_riepilogo_comprime_gli_spazi_e_porta_i_collegamenti() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let mut nota = store.create("Con collegamento").unwrap();
    nota.note.markdown = "# Titolo\n\n\n  riga   due\n".into();
    nota.note
        .add_link(CodeLink::new("C:/w", "src/main.rs", 3, 5, None));
    let nota = store.save(&nota).unwrap();

    let riepilogo = nota.summary();

    assert_eq!(riepilogo.preview, "# Titolo riga due");
    assert_eq!(riepilogo.links.len(), 1);
    assert_eq!(riepilogo.links[0].path, Path::new("src/main.rs"));
    assert_eq!(riepilogo.error, None);
}

#[test]
fn scan_di_una_nota_grande_da_lo_stesso_riepilogo_del_caricamento_intero() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let mut nota = store.create("Grande").unwrap();
    nota.note.markdown = format!("# Grande\n\n{}", "parola  altra\n".repeat(20_000));
    nota.note
        .add_link(CodeLink::new("C:/w", "src/lib.rs", 1, 2, Some("f")));
    let nota = store.save(&nota).unwrap();
    assert!(nota.original_bytes.len() as u64 > SCAN_PREFIX_BYTES);

    let scansionata = store.scan().unwrap().into_iter().next().unwrap();

    assert_eq!(scansionata, store.load(nota.path()).unwrap().summary());
    assert!(scansionata.preview.starts_with("# Grande parola altra"));
}

#[test]
fn se_l_intestazione_supera_il_prefisso_la_scansione_legge_tutto() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let mut nota = store.create("Molti collegamenti").unwrap();
    for i in 0..1500 {
        nota.note.add_link(CodeLink::new(
            "C:/w",
            format!("src/file_{i}.rs"),
            1,
            2,
            None,
        ));
    }
    let nota = store.save(&nota).unwrap();
    assert!(nota.original_bytes.len() as u64 > SCAN_PREFIX_BYTES);

    let riepilogo = store.scan().unwrap().into_iter().next().unwrap();

    assert_eq!(riepilogo.error, None);
    assert_eq!(riepilogo.links.len(), 1500);
    assert_eq!(riepilogo.id, nota.note.header.id);
}

#[test]
fn una_nota_oltre_il_limite_compare_nell_elenco_con_l_errore() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("grande.md");
    fs::File::create(&path)
        .unwrap()
        .set_len(MAX_NOTE_BYTES + 4096)
        .unwrap();

    let riepilogo = store_at(dir.path()).scan().unwrap().remove(0);

    assert_eq!(riepilogo.title, "grande");
    assert!(riepilogo.error.is_some());
}

#[test]
fn scan_include_solo_markdown_immediati_e_ordina_per_modifica() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("prima.md"), "# Prima\n\ncorpo cercabile\n").unwrap();
    fs::write(dir.path().join("ignora.txt"), "# Ignora\n").unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    fs::write(dir.path().join("nested").join("ignora.md"), "# Ignora\n").unwrap();

    let summaries = store_at(dir.path()).scan().unwrap();

    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].title, "Prima");
    assert!(summaries[0].preview.contains("corpo cercabile"));
}

#[test]
fn create_evade_collisioni_e_produce_file_markdown_reali() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());

    let first = store.create("Titolo uguale").unwrap();
    let second = store.create("Titolo uguale").unwrap();

    assert_ne!(first.path(), second.path());
    assert!(first.path().is_file());
    assert!(second.path().is_file());
    assert_eq!(first.note.header.title, "Titolo uguale");
    assert_eq!(second.note.header.title, "Titolo uguale");
}

#[test]
fn save_sostituisce_atomicamente_e_non_lascia_file_temporanei() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let mut loaded = store.create("Prima").unwrap();
    loaded.note.markdown = "Corpo nuovo\n".into();

    let saved = store.save(&loaded).unwrap();

    assert_eq!(
        store.load(saved.path()).unwrap().note.markdown,
        "Corpo nuovo\n"
    );
    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        names
            .iter()
            .all(|name| !name.to_string_lossy().ends_with(".tmp"))
    );
}

#[test]
fn save_aggiorna_la_data_di_modifica() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let mut nota = store.create("Prima").unwrap();
    nota.note.header.updated_unix_ms = 1;
    nota.note.markdown = "Corpo nuovo\n".into();

    let saved = store.save(&nota).unwrap();

    assert!(saved.note.header.updated_unix_ms > 1);
}

#[test]
fn un_salvataggio_non_sovrascrive_una_modifica_esterna() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let saved = store.create("Prima").unwrap();
    let mut loaded = store.load(saved.path()).unwrap();
    fs::write(saved.path(), "modifica esterna\n").unwrap();
    loaded.note.markdown = "modifica Lume\n".into();

    assert!(matches!(
        store.save(&loaded),
        Err(NoteError::Conflict { .. })
    ));
    assert_eq!(
        fs::read_to_string(saved.path()).unwrap(),
        "modifica esterna\n"
    );
}

#[test]
fn il_conflitto_si_legge_in_italiano() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let saved = store.create("Prima").unwrap();
    let mut loaded = store.load(saved.path()).unwrap();
    fs::write(saved.path(), "modifica esterna\n").unwrap();
    loaded.note.markdown = "modifica Lume\n".into();
    let Err(errore) = store.save(&loaded) else {
        panic!("il salvataggio doveva fallire con un conflitto");
    };
    assert!(matches!(errore, NoteError::Conflict { .. }));
    assert_eq!(errore.to_string(), "l'appunto è cambiato su disco");
}

#[test]
fn load_importa_markdown_semplice_e_preserva_fallback_malformato() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("idea.md");
    let malformed = dir.path().join("rotta.md");
    fs::write(&plain, "# Idea\n\nTesto **forte**.\n").unwrap();
    let malformed_bytes = b"+++\nid = [\n+++\ncorpo";
    fs::write(&malformed, malformed_bytes).unwrap();
    let store = store_at(dir.path());

    let imported = store.load(&plain).unwrap();
    let broken = store.load(&malformed).unwrap();

    assert_eq!(imported.note.header.title, "Idea");
    assert_eq!(imported.note.markdown, "# Idea\n\nTesto **forte**.\n");
    assert!(broken.read_only_error.is_some());
    assert_eq!(broken.original_bytes, malformed_bytes);
    assert!(matches!(
        store.save(&broken),
        Err(NoteError::ReadOnly { .. })
    ));
    assert_eq!(fs::read(&malformed).unwrap(), malformed_bytes);
}

#[test]
fn ricerca_e_case_insensitive_su_titolo_e_preview() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("uno.md"), "# Álgebra\n\nCorpo quieto\n").unwrap();
    fs::write(dir.path().join("due.md"), "# Seconda\n\nPAROLA rara\n").unwrap();
    let summaries = store_at(dir.path()).scan().unwrap();

    assert_eq!(filter_summaries(&summaries, "álg").len(), 1);
    assert_eq!(filter_summaries(&summaries, "parola RARA").len(), 1);
    assert_eq!(filter_summaries(&summaries, "assente").len(), 0);
}

#[test]
fn rename_del_titolo_evade_collisioni_e_rimuove_il_vecchio_nome() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let first = store.create("Prima").unwrap();
    let _occupied = store.create("Titolo nuovo").unwrap();

    let renamed = store.rename_for_title(&first, "Titolo nuovo").unwrap();

    assert!(!first.path().exists());
    assert!(renamed.path().exists());
    assert_eq!(renamed.note.header.title, "Titolo nuovo");
}

#[test]
fn save_with_title_rinomina_e_porta_con_se_il_testo_nuovo() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let mut nota = store.create("Prima").unwrap();
    nota.note.markdown = "# Dopo\n\ncorpo nuovo\n".into();

    let esito = store.save_with_title(&nota, "Dopo").unwrap();

    assert!(esito.rename_error.is_none());
    // Cartella sincronizzata dopo la pubblicazione, senza errori.
    assert!(esito.sync_error.is_none());
    assert_ne!(esito.loaded.path(), nota.path());
    assert!(!nota.path().exists());
    assert!(esito.loaded.path().is_file());
    assert_eq!(esito.loaded.note.header.title, "Dopo");
    assert!(esito.loaded.note.markdown.contains("corpo nuovo"));
    assert_eq!(store.scan().unwrap().len(), 1);
}

#[test]
fn save_with_title_con_lo_stesso_nome_base_non_crea_un_secondo_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let nota = store.create("Prima").unwrap();

    let esito = store.save_with_title(&nota, "PRIMA!").unwrap();

    assert!(esito.rename_error.is_none());
    // Cartella sincronizzata dopo la pubblicazione, senza errori.
    assert!(esito.sync_error.is_none());
    assert_eq!(esito.loaded.path(), nota.path());
    assert_eq!(esito.loaded.note.header.title, "PRIMA!");
    let file = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|voce| voce.path().extension().is_some_and(|e| e == "md"))
        .count();
    assert_eq!(file, 1);
}

#[test]
fn save_with_title_su_un_file_cambiato_fuori_resta_un_conflitto() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let nota = store.create("Prima").unwrap();
    fs::write(nota.path(), "# Altrove\n").unwrap();

    let esito = store.save_with_title(&nota, "Dopo");

    assert!(matches!(esito, Err(NoteError::Conflict { .. })));
    assert_eq!(fs::read_to_string(nota.path()).unwrap(), "# Altrove\n");
}

#[test]
fn save_copy_crea_una_nota_nuova_con_lo_stesso_testo_e_link() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let mut nota = store.create("Idea").unwrap();
    nota.note.markdown = "# Idea\n\ncorpo mio\n".into();
    nota.note.header.links.push(lume_notes::CodeLink::new(
        "C:/repo",
        "src/main.rs",
        3,
        5,
        Some("main"),
    ));

    let copia = store.save_copy(&nota).unwrap();

    assert_ne!(copia.note.header.id, nota.note.header.id);
    assert_ne!(copia.path(), nota.path());
    assert_eq!(copia.note.header.title, "Idea (copia)");
    assert!(copia.note.markdown.contains("corpo mio"));
    assert_eq!(copia.note.header.links.len(), 1);
    assert_eq!(store.scan().unwrap().len(), 2);
    assert_eq!(store.load(nota.path()).unwrap().note.header.title, "Idea");
}

#[test]
fn ogni_operazione_rifiuta_path_fuori_root_e_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("outside.md");
    fs::write(&outside_file, "# Fuori\n").unwrap();
    let store = store_at(dir.path());

    assert!(matches!(
        store.load(&outside_file),
        Err(NoteError::PathOutsideRoot { .. })
    ));

    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_file;
        let link = dir.path().join("link.md");
        if symlink_file(&outside_file, &link).is_ok() {
            assert!(matches!(
                store.load(&link),
                Err(NoteError::PathOutsideRoot { .. })
            ));
        }
    }
}

#[test]
fn nota_oltre_otto_mib_resta_apribile_come_sorgente_limitato_in_sola_lettura() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("grande.md");
    let file = fs::File::create(&path).unwrap();
    file.set_len(MAX_NOTE_BYTES + 4096).unwrap();
    let store = store_at(dir.path());

    let loaded = store.load(&path).unwrap();

    assert_eq!(loaded.original_bytes.len() as u64, MAX_NOTE_BYTES);
    assert_eq!(loaded.note.markdown.len() as u64, MAX_NOTE_BYTES);
    assert!(matches!(
        loaded.read_only_error,
        Some(NoteError::NoteTooLarge { .. })
    ));
    assert!(matches!(
        store.save(&loaded),
        Err(NoteError::ReadOnly { .. })
    ));
}

#[test]
fn capability_di_isolamento_salvataggio_e_esplicita() {
    let dir = tempfile::tempdir().unwrap();
    let isolation = store_at(dir.path()).save_isolation();
    #[cfg(windows)]
    assert_eq!(isolation, SaveIsolation::WindowsExclusiveTargetHandle);
    #[cfg(not(windows))]
    assert_eq!(isolation, SaveIsolation::BestEffortIdentityCheck);
}

#[cfg(windows)]
#[test]
fn delete_windows_sposta_un_file_reale_nel_cestino() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let note = store.create("Da cestinare").unwrap();
    let path = note.path().to_owned();

    assert_eq!(
        store.delete(&path).unwrap(),
        lume_notes::DeleteDisposition::Trashed
    );
    assert!(!path.exists());
}

#[test]
fn creazione_copia_e_salvataggio_riportano_la_sincronizzazione_della_cartella() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());

    let (nota, sync) = store.create_reporting("Nuova").unwrap();
    assert!(sync.is_none(), "{sync:?}");
    assert!(nota.path().is_file());

    let (copia, sync) = store.save_copy_reporting(&nota).unwrap();
    assert!(sync.is_none(), "{sync:?}");
    assert_ne!(copia.path(), nota.path());
    assert!(copia.path().is_file());

    let (salvata, sync) = store.save_reporting(&nota).unwrap();
    assert!(sync.is_none(), "{sync:?}");
    assert_eq!(salvata.path(), nota.path());
}
