#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! «Ripristina» contro repository veri. E' l'unica operazione distruttiva del
//! pannello: i test coprono anche cosa **non** deve toccare.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use lume_git::{Discarded, GitRepository};

fn temp_root(tag: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lume_git_discard_{tag}_{nonce}"));
    fs::create_dir_all(&root).unwrap();
    root
}

/// Repository con un file gia' tracciato, messo in stage e **committato**:
/// senza il commit lo status resterebbe sporco per il solo fatto che l'index
/// contiene un file che HEAD non ha.
///
/// `autocrlf` si scrive sempre, in un senso o nell'altro: questa macchina ha
/// `core.autocrlf = true` **globale**, che un repository temporaneo eredita.
/// Lasciarlo implicito renderebbe i test dipendenti dalla configurazione di
/// chi li esegue.
fn repo_with_tracked(tag: &str, content: &[u8], autocrlf: bool) -> (PathBuf, GitRepository) {
    let root = temp_root(tag);
    gix::init(&root).unwrap();
    {
        let mut config = fs::OpenOptions::new()
            .append(true)
            .open(root.join(".git").join("config"))
            .unwrap();
        writeln!(
            config,
            "\tautocrlf = {autocrlf}\n[user]\n\tname = Lume Test\n\temail = lume@example.test"
        )
        .unwrap();
    }
    fs::write(root.join("a.txt"), content).unwrap();
    let repository = GitRepository::discover(&root).unwrap();
    repository.stage(["a.txt"]).unwrap();
    repository.commit("base").unwrap();
    (root, repository)
}

#[test]
fn ripristina_riporta_il_file_al_contenuto_dellindex() {
    let (root, repository) = repo_with_tracked("tracciato", b"originale\n", false);
    fs::write(root.join("a.txt"), b"rovinato\n").unwrap();

    let done = repository.discard(["a.txt"]).unwrap();

    assert_eq!(done, vec![Discarded::Restored(PathBuf::from("a.txt"))]);
    assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"originale\n");
    fs::remove_dir_all(root).unwrap();
}

/// Il caso che perde lavoro se sbagliato: scrivendo i byte grezzi del blob il
/// file tornerebbe a LF, risulterebbe modificato **subito dopo** il ripristino
/// e la riga successiva del pannello lo mostrerebbe ancora sporco.
#[test]
fn con_autocrlf_il_file_ripristinato_torna_pulito_nello_status() {
    let (root, repository) = repo_with_tracked("autocrlf", b"uno\r\ndue\r\n", true);
    assert!(
        repository.status().unwrap().is_empty(),
        "prima del pasticcio lo status e' pulito"
    );
    fs::write(root.join("a.txt"), b"uno\r\nROVINATO\r\n").unwrap();
    assert!(!repository.status().unwrap().is_empty());

    repository.discard(["a.txt"]).unwrap();

    assert_eq!(
        fs::read(root.join("a.txt")).unwrap(),
        b"uno\r\ndue\r\n",
        "il ritorno a capo Windows va rimesso: e' la pipeline di filtri a farlo"
    );
    assert!(
        repository.status().unwrap().is_empty(),
        "ripristinato vuol dire pulito, non «modificato in un altro modo»"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn un_file_non_versionato_si_ripristina_cancellandolo() {
    let root = temp_root("nonversionato");
    gix::init(&root).unwrap();
    fs::write(root.join("scarto.txt"), b"roba\n").unwrap();
    let repository = GitRepository::discover(&root).unwrap();

    let done = repository.discard(["scarto.txt"]).unwrap();

    assert_eq!(done, vec![Discarded::Deleted(PathBuf::from("scarto.txt"))]);
    assert!(!root.join("scarto.txt").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ripristinare_un_path_che_non_esiste_non_e_un_errore() {
    let root = temp_root("assente");
    gix::init(&root).unwrap();
    let repository = GitRepository::discover(&root).unwrap();

    let done = repository.discard(["mai_esistito.txt"]).unwrap();

    assert_eq!(
        done,
        vec![Discarded::Unchanged(PathBuf::from("mai_esistito.txt"))]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ripristina_non_esce_dal_worktree() {
    let (root, repository) = repo_with_tracked("fuori", b"x\n", false);
    let outside = root.parent().unwrap().join("fuori.txt");
    fs::write(&outside, b"non toccare\n").unwrap();

    assert!(matches!(
        repository.discard([&outside]),
        Err(lume_git::GitError::InvalidPath(_))
    ));
    assert!(outside.exists(), "il file fuori dal worktree resta intatto");

    fs::remove_file(outside).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ripristina_tocca_solo_i_path_chiesti() {
    let (root, repository) = repo_with_tracked("selettivo", b"originale\n", false);
    fs::write(root.join("b.txt"), b"altro\n").unwrap();
    repository.stage(["b.txt"]).unwrap();
    fs::write(root.join("a.txt"), b"rovinato\n").unwrap();
    fs::write(root.join("b.txt"), b"rovinato pure\n").unwrap();

    repository.discard(["a.txt"]).unwrap();

    assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"originale\n");
    assert_eq!(
        fs::read(root.join("b.txt")).unwrap(),
        b"rovinato pure\n",
        "un file non chiesto non si tocca"
    );
    fs::remove_dir_all(root).unwrap();
}

/// `git status --short` collassa una cartella non versionata in **una riga**
/// (`?? artifacts/`), e quella riga non sarebbe azionabile: non si mette in
/// stage, non se ne apre il diff, e «Ripristina» ci chiamava sopra
/// `remove_file` fallendo con «Accesso negato» — fermando il ripristino a
/// metà. Questo test inchioda l'impostazione che lo evita: `status()` emette
/// **un file per riga**, cartella per cartella.
#[test]
fn una_cartella_non_versionata_esce_dallo_status_file_per_file() {
    let (root, repository) = repo_with_tracked(
        "cartella", b"base
", false,
    );
    fs::create_dir_all(root.join("nuova").join("dentro")).unwrap();
    fs::write(
        root.join("nuova").join("uno.txt"),
        b"1
",
    )
    .unwrap();
    fs::write(
        root.join("nuova").join("dentro").join("due.txt"),
        b"2
",
    )
    .unwrap();

    let mut untracked: Vec<PathBuf> = repository
        .status()
        .unwrap()
        .into_iter()
        .filter(|file| file.is_untracked())
        .map(|file| file.path.clone())
        .collect();
    untracked.sort();
    assert_eq!(
        untracked,
        vec![
            PathBuf::from("nuova/dentro/due.txt"),
            PathBuf::from("nuova/uno.txt"),
        ],
        "lo status deve emettere i file, non la cartella che li contiene"
    );

    let done = repository.discard(untracked).unwrap();

    assert!(
        done.iter().all(|d| matches!(d, Discarded::Deleted(_))),
        "tutto doveva risultare cancellato: {done:?}"
    );
    assert!(
        !root.join("nuova").join("uno.txt").exists()
            && !root.join("nuova").join("dentro").join("due.txt").exists(),
        "«Ripristina» ha lasciato indietro dei file non versionati"
    );
    fs::remove_dir_all(root).unwrap();
}

/// La casella su un file non versionato lo mette in stage: e' l'altra meta'
/// della riga, e con lo status collassato arrivava qui una cartella.
#[test]
fn un_file_non_versionato_dentro_una_cartella_si_mette_in_stage() {
    let (root, repository) = repo_with_tracked(
        "stage_cartella",
        b"base
",
        false,
    );
    fs::create_dir_all(root.join("nuova")).unwrap();
    fs::write(
        root.join("nuova").join("uno.txt"),
        b"1
",
    )
    .unwrap();

    let untracked: Vec<PathBuf> = repository
        .status()
        .unwrap()
        .into_iter()
        .filter(|file| file.is_untracked())
        .map(|file| file.path.clone())
        .collect();
    repository.stage(&untracked).unwrap();

    let staged: Vec<PathBuf> = repository
        .status()
        .unwrap()
        .into_iter()
        .filter(|file| file.is_staged())
        .map(|file| file.path.clone())
        .collect();
    assert_eq!(staged, vec![PathBuf::from("nuova/uno.txt")]);
    fs::remove_dir_all(root).unwrap();
}

/// Una cartella non arriva mai dal pannello — `status()` emette i file uno per
/// uno — ma `discard` è API pubblica: cancellarla ricorsivamente porterebbe
/// via anche gli **ignorati** che contiene, cosa che `git clean -fd` non fa.
/// Meglio rifiutare che indovinare.
#[test]
fn ripristinare_una_cartella_e_un_rifiuto_non_una_cancellazione() {
    let (root, repository) = repo_with_tracked("rifiuto", b"base\n", false);
    fs::create_dir_all(root.join("nuova")).unwrap();
    fs::write(root.join("nuova").join("uno.txt"), b"1\n").unwrap();

    let esito = repository.discard(["nuova"]);

    assert!(esito.is_err(), "una cartella va rifiutata: {esito:?}");
    assert!(
        root.join("nuova").join("uno.txt").exists(),
        "il rifiuto non deve aver cancellato niente"
    );
    fs::remove_dir_all(root).unwrap();
}
