//! Input reali/FD/TempDir: nessun reader/backend simulato.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use std::{
    fs::{File, OpenOptions},
    io::Write,
};
fn engine(path: PathBuf) -> Vec<SearchUpdate> {
    let (sender, receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        run_search(
            &SearchScope::Aperti(vec![path]),
            &SearchOptions::literal("ago"),
            &AtomicU64::new(1),
            1,
            &Emitter::Legacy(sender),
        );
    });
    let updates = receiver.iter().collect();
    worker.join().unwrap();
    updates
}
#[test]
fn range_su_riga_lossy_e_riferito_al_testo_visualizzato_non_ai_byte_originali() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("non_utf8.txt");
    std::fs::write(&path, b"\xffago\n").unwrap();
    let updates = engine(path);
    let hit = updates
        .iter()
        .find_map(|update| match update {
            SearchUpdate::Batch(files) => files.first().and_then(|f| f.hits.first()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        hit.line_text.get(hit.range_in_line.clone()),
        Some("ago"),
        "range UTF-8 deve indicare il match nel testo realmente visualizzato"
    );
}
#[test]
fn file_cresciuto_dopo_metadata_non_pubblica_prefisso_entro_il_tetto() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crescita.txt");
    std::fs::write(&path, b"ago\n").unwrap();
    let mut file = File::open(&path).unwrap();
    assert!(file.metadata().unwrap().len() <= MAX_FILE_SIZE);
    let mut writer = OpenOptions::new().append(true).open(&path).unwrap();
    writer
        .write_all(&vec![b'x'; usize::try_from(MAX_FILE_SIZE).unwrap() * 4])
        .unwrap();
    writer.sync_all().unwrap();
    let mut buffer = Vec::new();
    let result = read_search_content(&mut file, &mut buffer, || false).unwrap();
    let skipped = result.is_none();
    assert!(
        skipped,
        "metadata piccola non autorizza a leggere/accettare file ora troppo grande"
    );
    assert!(
        buffer.len() <= usize::try_from(MAX_FILE_SIZE + 1).unwrap(),
        "input residente non deve crescere oltre soglia+sentinella"
    );
}
#[test]
fn limite_esatto_accettato_byte_extra_rifiutato_e_buffer_riutilizzato() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("esatto.txt");
    let max = usize::try_from(MAX_FILE_SIZE).unwrap();
    std::fs::write(&path, vec![b'x'; max]).unwrap();
    let mut buffer = Vec::new();
    let mut file = File::open(&path).unwrap();
    assert_eq!(
        read_search_content(&mut file, &mut buffer, || false)
            .unwrap()
            .unwrap()
            .len(),
        max
    );
    let pointer = buffer.as_ptr();
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"x")
        .unwrap();
    let mut file = File::open(&path).unwrap();
    assert!(
        read_search_content(&mut file, &mut buffer, || false)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        buffer.as_ptr(),
        pointer,
        "seconda lettura non deve riallocare input oltre buffer fisso"
    );
}
#[test]
fn cancellazione_prima_del_read_non_alloca_o_legge_input() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cancel.txt");
    std::fs::write(&path, b"ago\n").unwrap();
    let mut file = File::open(path).unwrap();
    let mut buffer = Vec::new();
    assert!(
        read_search_content(&mut file, &mut buffer, || true)
            .unwrap()
            .is_none()
    );
    assert!(buffer.is_empty());
}
#[test]
fn fd_aperto_letto_senza_riaprire_per_nome_dopo_sostituzione_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bersaglio.txt");
    std::fs::write(&path, b"originale\n").unwrap();
    let mut file = File::open(&path).unwrap();
    let meta = file.metadata().unwrap();
    assert!(meta.is_file());
    std::fs::rename(&path, dir.path().join("precedente.txt")).unwrap();
    std::fs::write(&path, b"nuovo\n").unwrap();
    let mut buffer = Vec::new();
    assert_eq!(
        read_search_content(&mut file, &mut buffer, || false)
            .unwrap()
            .unwrap(),
        b"originale\n"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"nuovo\n");
    // È identità dell'handle letto, non confinamento/identità del nome ora.
}
#[test]
fn bom_utf16_decoder_pre_esistente_conservato_con_input_raw_limitato() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("utf16.txt");
    std::fs::write(&path, b"\xff\xfea\0g\0o\0\r\0\n\0").unwrap();
    let updates = engine(path);
    let hit = updates
        .iter()
        .find_map(|update| match update {
            SearchUpdate::Batch(files) => files.first().and_then(|file| file.hits.first()),
            _ => None,
        })
        .unwrap();
    assert_eq!(hit.line_text.as_ref(), "ago");
    assert_eq!(hit.line_text.get(hit.range_in_line.clone()), Some("ago"));
}

#[test]
fn crescita_oversize_normale_e_file_a_limite_cercati_da_engine_stesso() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("limite.txt");
    let mut content = vec![b'x'; usize::try_from(MAX_FILE_SIZE).unwrap()];
    content.get_mut(..4).unwrap().copy_from_slice(b"ago\n");
    std::fs::write(&path, &content).unwrap();
    let updates = engine(path.clone());
    assert!(matches!(
        updates.last(),
        Some(SearchUpdate::Done {
            files: 1,
            matches: 1,
            truncated: false
        })
    ));
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"x")
        .unwrap();
    let updates = engine(path);
    assert_eq!(
        updates,
        vec![SearchUpdate::Done {
            files: 0,
            matches: 0,
            truncated: false
        }]
    );
}
