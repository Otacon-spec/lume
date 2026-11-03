#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::fs;

use lume_jj::find_workspace_root;

/// Se un antenato della cartella temporanea avesse a sua volta un `.jj`, un
/// «nessun workspace» assoluto sarebbe falso: si verifica solo che la radice
/// trovata non stia **dentro** la cartella di prova.
fn fuori_dalla_prova(trovata: Option<std::path::PathBuf>, prova: &std::path::Path) -> bool {
    trovata.is_none_or(|radice| !radice.starts_with(prova))
}

#[test]
fn find_workspace_root_sale_gli_antenati() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir_all(repo.join(".jj").join("repo")).unwrap();
    let sotto = repo.join("servizi").join("api");
    fs::create_dir_all(&sotto).unwrap();

    assert_eq!(find_workspace_root(&sotto), Some(repo.clone()));
    assert_eq!(find_workspace_root(&repo), Some(repo));
}

#[test]
fn find_workspace_root_senza_jj_non_trova_niente_nella_prova() {
    let dir = tempfile::tempdir().unwrap();
    let progetto = dir.path().join("progetto");
    // Un repository git non è un workspace jj.
    fs::create_dir_all(progetto.join(".git")).unwrap();

    assert!(fuori_dalla_prova(
        find_workspace_root(&progetto),
        dir.path()
    ));
}

#[test]
fn un_file_chiamato_jj_non_e_un_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let progetto = dir.path().join("progetto");
    fs::create_dir_all(&progetto).unwrap();
    fs::write(progetto.join(".jj"), b"non sono una cartella").unwrap();

    assert!(fuori_dalla_prova(
        find_workspace_root(&progetto),
        dir.path()
    ));
}

#[test]
fn una_cartella_che_non_esiste_non_trova_niente_nella_prova() {
    let dir = tempfile::tempdir().unwrap();
    let assente = dir.path().join("non-esiste");

    assert!(fuori_dalla_prova(find_workspace_root(&assente), dir.path()));
}
