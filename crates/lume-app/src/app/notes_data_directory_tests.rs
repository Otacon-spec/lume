//! Prove della radice Appunti: ambiente modificato solo nel figlio posseduto.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

#[test]
fn la_cartella_predefinita_degli_appunti_e_quella_dello_store() {
    let attesa = lume_store::application_data_dir()
        .map(|radice| radice.join("notes"))
        .unwrap_or_else(|_| PathBuf::from("notes"));
    assert_eq!(default_notes_directory(), attesa);
}

#[cfg(target_os = "linux")]
#[test]
fn un_processo_isolato_crea_appunti_solo_sotto_l_override() {
    let directory = tempfile::tempdir().unwrap();
    let radice = directory.path().join("dati isolati è");
    let profilo = directory.path().join("profilo");
    let note_esistenti = profilo.join("lume/notes");
    std::fs::create_dir_all(&note_esistenti).unwrap();
    let precedente = note_esistenti.join("esistente.md");
    let contenuto = b"Nota precedente da conservare byte per byte\n";
    std::fs::write(&precedente, contenuto).unwrap();

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "app::notes_data_directory_tests::figlio_crea_nota_nella_radice_isolata",
            "--ignored",
            "--nocapture",
        ])
        .env(lume_store::DATA_DIR_OVERRIDE_ENV, &radice)
        .env("LUME_TEST_APPUNTI_RADICE", &radice)
        .env("XDG_DATA_HOME", &profilo)
        .env("HOME", directory.path().join("home"))
        .output()
        .unwrap();
    // output() attende il solo figlio posseduto e raccoglie il suo esito.
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
        "il filtro deve eseguire il figlio, non zero test"
    );
    assert_eq!(std::fs::read(&precedente).unwrap(), contenuto);
    assert_eq!(std::fs::read_dir(&note_esistenti).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(radice.join("notes")).unwrap().count(), 1);
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "helper avviato e verificato dal test padre con --exact --ignored"]
fn figlio_crea_nota_nella_radice_isolata() {
    let radice = PathBuf::from(
        std::env::var_os("LUME_TEST_APPUNTI_RADICE").expect("radice del solo helper"),
    );
    assert_eq!(lume_store::application_data_dir().unwrap(), radice);
    let note = default_notes_directory();
    // Prima dell'asserzione non si apre la cartella: il codice difettoso non
    // deve scrivere nel profilo (nemmeno in quello finto della fixture).
    assert_eq!(note, radice.join("notes"));
    let store = lume_notes::NoteStore::open(&note).unwrap();
    let caricata = store.create("Nota isolata").unwrap();
    // Lo store canonicalizza la radice (un antenato symlink cambia la forma):
    // l'ipotesi «stesso prefisso scritto» è della fixture, non del prodotto.
    let note_canonica = std::fs::canonicalize(&note).unwrap();
    assert!(caricata.path().starts_with(&note_canonica));
    assert_eq!(
        store.load(caricata.path()).unwrap().note.header.title,
        "Nota isolata"
    );
    assert_eq!(store.scan().unwrap().len(), 1);
}
