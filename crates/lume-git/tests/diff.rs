#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Diff di un file contro repository veri, non finti.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lume_git::{DiffRow, DiffSign, GitRepository};

fn temp_root(tag: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_git_diff_{tag}_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    root
}

fn signature() -> gix::actor::SignatureRef<'static> {
    gix::actor::SignatureRef::from_bytes(b"Lume Test <lume@example.test> 1700000000 +0100").unwrap()
}

/// Scrive i file **nel worktree** e li committa, cosi' il worktree e HEAD
/// partono allineati come in un repository vero.
fn commit_files(
    repository: &gix::Repository,
    root: &Path,
    files: &[(&str, &[u8])],
    parents: Vec<gix::ObjectId>,
    message: &str,
) -> gix::ObjectId {
    let mut entries = Vec::new();
    for (name, content) in files {
        fs::write(root.join(name), content).unwrap();
        let blob = repository.write_blob(content).unwrap().detach();
        entries.push(gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Blob.into(),
            filename: (*name).into(),
            oid: blob,
        });
    }
    entries.sort_by(|a, b| a.filename.cmp(&b.filename));
    let tree = repository
        .write_object(gix::objs::Tree { entries })
        .unwrap()
        .detach();
    repository
        .commit_as(signature(), signature(), "HEAD", message, tree, parents)
        .unwrap()
        .detach()
}

fn lines(diff: &lume_git::FileDiff) -> Vec<(Option<u32>, Option<u32>, DiffSign, String)> {
    diff.rows
        .iter()
        .filter_map(|row| match row {
            DiffRow::Line {
                old_line,
                new_line,
                sign,
                text,
                ..
            } => Some((*old_line, *new_line, *sign, text.clone())),
            DiffRow::Gap { .. } => None,
        })
        .collect()
}

#[test]
fn una_riga_cambiata_da_una_rimozione_e_unaggiunta_con_i_numeri_giusti() {
    let root = temp_root("modificato");
    let repository = gix::init(&root).unwrap();
    commit_files(
        &repository,
        &root,
        &[("a.txt", b"uno\ndue\ntre\n")],
        Vec::new(),
        "primo",
    );
    fs::write(root.join("a.txt"), b"uno\nDUE\ntre\n").unwrap();

    let git = GitRepository::discover(&root).unwrap();
    let diff = git.diff_worktree_file(Path::new("a.txt")).unwrap();

    assert!(!diff.binary);
    assert_eq!((diff.added, diff.removed), (1, 1));
    assert_eq!(
        lines(&diff),
        vec![
            (Some(1), Some(1), DiffSign::Context, "uno".to_owned()),
            (Some(2), None, DiffSign::Removed, "due".to_owned()),
            (None, Some(2), DiffSign::Added, "DUE".to_owned()),
            (Some(3), Some(3), DiffSign::Context, "tre".to_owned()),
        ]
    );
}

/// **Il test che discrimina.** Con `core.autocrlf` acceso il blob in HEAD e' a
/// LF e il file su disco a CRLF: leggendo i byte grezzi ogni riga
/// risulterebbe cambiata. Passando dalla pipeline di filtri di gitoxide, no.
#[test]
fn con_autocrlf_il_ritorno_a_capo_windows_non_sporca_il_diff() {
    let root = temp_root("autocrlf");
    let repository = gix::init(&root).unwrap();
    {
        let mut config = fs::OpenOptions::new()
            .append(true)
            .open(root.join(".git").join("config"))
            .unwrap();
        writeln!(config, "\tautocrlf = true").unwrap();
    }
    commit_files(
        &repository,
        &root,
        &[("a.txt", b"uno\ndue\ntre\n")],
        Vec::new(),
        "primo",
    );
    // Il worktree come lo lascerebbe un editor su Windows: CRLF ovunque, e
    // una sola riga davvero cambiata.
    fs::write(root.join("a.txt"), b"uno\r\nDUE\r\ntre\r\n").unwrap();

    let git = GitRepository::discover(&root).unwrap();
    let diff = git.diff_worktree_file(Path::new("a.txt")).unwrap();

    assert_eq!(
        (diff.added, diff.removed),
        (1, 1),
        "solo la riga cambiata deve contare: i CRLF sono lavoro della pipeline, non differenze"
    );
}

#[test]
fn un_file_non_versionato_e_tutto_in_aggiunta() {
    let root = temp_root("nuovo");
    let repository = gix::init(&root).unwrap();
    commit_files(
        &repository,
        &root,
        &[("a.txt", b"uno\n")],
        Vec::new(),
        "primo",
    );
    fs::write(root.join("nuovo.txt"), b"alfa\nbeta\n").unwrap();

    let git = GitRepository::discover(&root).unwrap();
    let diff = git.diff_worktree_file(Path::new("nuovo.txt")).unwrap();

    assert_eq!((diff.added, diff.removed), (2, 0));
    assert!(
        lines(&diff)
            .iter()
            .all(|(old, _, sign, _)| old.is_none() && *sign == DiffSign::Added)
    );
}

#[test]
fn un_file_cancellato_e_tutto_in_rimozione_senza_errori_di_lettura() {
    let root = temp_root("cancellato");
    let repository = gix::init(&root).unwrap();
    commit_files(
        &repository,
        &root,
        &[("a.txt", b"uno\ndue\n")],
        Vec::new(),
        "primo",
    );
    fs::remove_file(root.join("a.txt")).unwrap();

    let git = GitRepository::discover(&root).unwrap();
    let diff = git
        .diff_worktree_file(Path::new("a.txt"))
        .expect("un file che non c'e' piu' non e' un errore, e' una rimozione");

    assert_eq!((diff.added, diff.removed), (0, 2));
}

#[test]
fn un_binario_non_produce_righe_ma_lo_dichiara() {
    let root = temp_root("binario");
    let repository = gix::init(&root).unwrap();
    commit_files(
        &repository,
        &root,
        &[("blob.bin", b"\x00\x01\x02\x00stuff")],
        Vec::new(),
        "primo",
    );
    fs::write(root.join("blob.bin"), b"\x00\x01\x02\x00altro").unwrap();

    let git = GitRepository::discover(&root).unwrap();
    let diff = git.diff_worktree_file(Path::new("blob.bin")).unwrap();

    assert!(diff.binary);
    assert!(diff.rows.is_empty());
}

#[test]
fn fra_due_modifiche_lontane_il_contesto_si_interrompe_con_un_salto() {
    let root = temp_root("salto");
    let repository = gix::init(&root).unwrap();
    let mut before = String::new();
    for index in 1..=40 {
        before.push_str(&format!("riga {index}\n"));
    }
    commit_files(
        &repository,
        &root,
        &[("lungo.txt", before.as_bytes())],
        Vec::new(),
        "primo",
    );
    let after = before
        .replace("riga 2\n", "RIGA 2\n")
        .replace("riga 39\n", "RIGA 39\n");
    fs::write(root.join("lungo.txt"), after.as_bytes()).unwrap();

    let git = GitRepository::discover(&root).unwrap();
    let diff = git.diff_worktree_file(Path::new("lungo.txt")).unwrap();

    assert_eq!((diff.added, diff.removed), (2, 2));
    let gaps: Vec<u32> = diff
        .rows
        .iter()
        .filter_map(|row| match row {
            DiffRow::Gap { skipped } => Some(*skipped),
            DiffRow::Line { .. } => None,
        })
        .collect();
    assert_eq!(gaps.len(), 1, "un solo salto, fra le due modifiche");
    assert!(
        diff.rows.len() < 20,
        "senza contesto limitato ci sarebbero 40+ righe, non {}",
        diff.rows.len()
    );
}

#[test]
fn il_diff_di_un_commit_usa_il_primo_parent_come_base() {
    let root = temp_root("commit");
    let repository = gix::init(&root).unwrap();
    let first = commit_files(
        &repository,
        &root,
        &[("a.txt", b"uno\ndue\n")],
        Vec::new(),
        "primo",
    );
    let second = commit_files(
        &repository,
        &root,
        &[("a.txt", b"uno\nDUE\n")],
        vec![first],
        "secondo",
    );

    let git = GitRepository::discover(&root).unwrap();
    let diff = git
        .diff_commit_file(&second.to_string(), Path::new("a.txt"))
        .unwrap();

    assert_eq!((diff.added, diff.removed), (1, 1));
    assert_eq!(diff.base, lume_git::DiffBase::Commit(second.to_string()));
}

#[test]
fn nel_commit_radice_tutto_il_file_e_aggiunta() {
    let root = temp_root("radice");
    let repository = gix::init(&root).unwrap();
    let first = commit_files(
        &repository,
        &root,
        &[("a.txt", b"uno\ndue\ntre\n")],
        Vec::new(),
        "primo",
    );

    let git = GitRepository::discover(&root).unwrap();
    let diff = git
        .diff_commit_file(&first.to_string(), Path::new("a.txt"))
        .unwrap();

    assert_eq!((diff.added, diff.removed), (3, 0));
}

/// Non e' un test del prodotto: controlla che il test qui sopra sia
/// *discriminante*, cioe' che l'approccio ingenuo (byte grezzi) fallirebbe.
#[test]
fn prova_del_nove_i_byte_grezzi_vedrebbero_tutto_cambiato() {
    let root = temp_root("prova9");
    let repository = gix::init(&root).unwrap();
    {
        let mut config = fs::OpenOptions::new()
            .append(true)
            .open(root.join(".git").join("config"))
            .unwrap();
        writeln!(config, "\tautocrlf = true").unwrap();
    }
    commit_files(
        &repository,
        &root,
        &[("a.txt", b"uno\ndue\ntre\n")],
        Vec::new(),
        "primo",
    );
    fs::write(root.join("a.txt"), b"uno\r\nDUE\r\ntre\r\n").unwrap();

    let on_disk = fs::read(root.join("a.txt")).unwrap();
    let in_head = b"uno\ndue\ntre\n".to_vec();
    let input = gix::diff::blob::InternedInput::new(in_head.as_slice(), on_disk.as_slice());
    let naive = gix::diff::blob::Diff::compute(gix::diff::blob::Algorithm::Histogram, &input);
    assert_eq!(
        (naive.count_additions(), naive.count_removals()),
        (3, 3),
        "se questo non fosse 3/3 il test sull'autocrlf non proverebbe nulla"
    );
}

/// `status()` emette i file uno per uno, quindi dal pannello una cartella non
/// arriva mai qui (vedi `tests/discard.rs`). Ma `diff_worktree_file` e' API
/// pubblica: su una cartella deve venirne fuori un errore dichiarato, non un
/// panico ne' un diff vuoto spacciato per file identico.
#[test]
fn il_diff_di_una_cartella_e_un_errore_non_un_panico() {
    let root = temp_root("cartella");
    let repository = gix::init(&root).unwrap();
    commit_files(
        &repository,
        &root,
        &[(
            "a.txt", b"uno
",
        )],
        Vec::new(),
        "primo",
    );
    fs::create_dir_all(root.join("nuova")).unwrap();
    fs::write(
        root.join("nuova").join("dentro.txt"),
        b"x
",
    )
    .unwrap();

    let git = GitRepository::discover(&root).unwrap();
    let esito = git.diff_worktree_file(Path::new("nuova"));

    assert!(esito.is_err(), "una cartella non e' un file: {esito:?}");
}

/// Il caso che nasce dal ricalcolo automatico: diff aperto su un file **non
/// versionato**, «Ripristina» lo cancella, lo stato si aggiorna e il diff si
/// richiede da solo su un path che ora non esiste ne' in HEAD ne' su disco.
/// Qui e' un errore dichiarato; e' l'app che, trattandosi di un ricalcolo e
/// non di un gesto dell'utente, chiude la vista invece di notificarlo.
#[test]
fn il_diff_di_un_path_che_non_esiste_da_nessuna_parte_e_un_errore() {
    let root = temp_root("inesistente");
    let repository = gix::init(&root).unwrap();
    commit_files(
        &repository,
        &root,
        &[(
            "a.txt", b"uno
",
        )],
        Vec::new(),
        "primo",
    );

    let git = GitRepository::discover(&root).unwrap();
    let esito = git.diff_worktree_file(Path::new("sparito.txt"));

    assert!(esito.is_err(), "niente da nessuna parte: {esito:?}");
}
