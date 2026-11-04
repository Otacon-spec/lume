#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lume_notes::{
    CodeLink, LinkIndex, Note, NoteError, NoteId, NoteSummary, clamp_lines, relative_path,
    resolve_link,
};

fn summary(id: &str, title: &str, links: Vec<CodeLink>) -> NoteSummary {
    let mut note = Note::empty(NoteId::from(id));
    note.header.title = title.to_owned();
    for link in links {
        note.add_link(link);
    }
    note.summary(None)
}

fn link(path: &str, from: usize, to: usize) -> CodeLink {
    CodeLink::try_new("/w/proj", path, from, to, None).unwrap()
}

fn literal(workspace: &str, path: &str, from: usize, to: usize) -> CodeLink {
    CodeLink {
        workspace: PathBuf::from(workspace),
        path: PathBuf::from(path),
        from,
        to,
        symbol: None,
        extra: BTreeMap::new(),
    }
}

#[test]
fn indice_raggruppa_per_file_e_ordina_per_riga() {
    let summaries = vec![
        summary(
            "b",
            "Beta",
            vec![link("src/main.rs", 5, 6), link("src/lib.rs", 1, 1)],
        ),
        summary("a", "Alfa", vec![link("src/main.rs", 2, 3)]),
    ];
    let index = LinkIndex::build(&summaries);
    let main = index.for_file(Path::new("/w/proj"), Path::new("src/main.rs"));
    let titles: Vec<&str> = main.iter().map(|item| item.title.as_str()).collect();
    assert_eq!(titles, ["Alfa", "Beta"]);
    assert_eq!((main[0].from, main[0].to), (2, 3));
    assert_eq!(main[0].note.as_str(), "a");
    assert_eq!(
        index
            .for_file(Path::new("/w/proj"), Path::new("src/lib.rs"))
            .len(),
        1
    );
    assert!(
        index
            .for_file(Path::new("/w/proj"), Path::new("other.rs"))
            .is_empty()
    );
    assert!(
        index
            .for_file(Path::new("/w/other"), Path::new("src/main.rs"))
            .is_empty()
    );
    assert!(!index.is_empty());
    assert!(LinkIndex::default().is_empty());
}

#[test]
fn a_parita_di_riga_l_ordine_segue_il_titolo() {
    let summaries = vec![
        summary("z", "Zeta", vec![link("a.rs", 4, 4)]),
        summary("y", "Alfa", vec![link("a.rs", 4, 4)]),
    ];
    let index = LinkIndex::build(&summaries);
    let titles: Vec<&str> = index
        .for_file(Path::new("/w/proj"), Path::new("a.rs"))
        .iter()
        .map(|item| item.title.as_str())
        .collect();
    assert_eq!(titles, ["Alfa", "Zeta"]);
}

#[test]
fn il_prefisso_verbatim_non_cambia_la_chiave() {
    let link = CodeLink::try_new("//?/C:/proj", "src/a.rs", 1, 1, None).unwrap();
    let index = LinkIndex::build(&[summary("n", "N", vec![link])]);
    assert_eq!(
        index
            .for_file(Path::new("C:/proj"), Path::new("src/a.rs"))
            .len(),
        1
    );
}

#[cfg(windows)]
#[test]
fn su_windows_le_maiuscole_non_contano() {
    let link = CodeLink::try_new("C:/Proj", "Src/A.rs", 1, 1, None).unwrap();
    let index = LinkIndex::build(&[summary("n", "N", vec![link])]);
    assert_eq!(
        index
            .for_file(Path::new("c:/proj"), Path::new("src/a.rs"))
            .len(),
        1
    );
}

#[test]
fn il_percorso_relativo_si_calcola_per_componenti() {
    let root = Path::new("/w/proj");
    assert_eq!(
        relative_path(root, Path::new("/w/proj/src/main.rs")),
        Some(PathBuf::from("src").join("main.rs"))
    );
    assert_eq!(relative_path(root, Path::new("/w/other/x.rs")), None);
    assert_eq!(relative_path(root, Path::new("/w/proj")), None);
    assert_eq!(relative_path(root, Path::new("/w/proj/../x.rs")), None);
    assert_eq!(
        relative_path(Path::new("/w/pro"), Path::new("/w/proj/x.rs")),
        None
    );
}

#[cfg(unix)]
#[test]
fn il_percorso_relativo_unisce_un_antenato_symlink() {
    let cartella = tempfile::tempdir().unwrap();
    let radice = lume_core::canonical_path(cartella.path())
        .unwrap()
        .join("progetto");
    std::fs::create_dir_all(radice.join("src")).unwrap();
    std::fs::write(radice.join("src/a.rs"), b"fn a() {}\n").unwrap();
    let link = cartella.path().join("alias");
    std::os::unix::fs::symlink(&radice, &link).unwrap();
    assert_eq!(
        relative_path(&radice, &link.join("src/a.rs")),
        Some(PathBuf::from("src").join("a.rs"))
    );
    let esterna = tempfile::tempdir().unwrap();
    let fuori = esterna.path().join("fuori.rs");
    std::fs::write(&fuori, b"x").unwrap();
    assert_eq!(relative_path(&radice, &fuori), None);
}

#[test]
fn risolvere_un_collegamento_rivalida_il_percorso() {
    let valid = link("src/main.rs", 1, 2);
    assert_eq!(
        resolve_link(&valid).unwrap(),
        Path::new("/w/proj").join("src").join("main.rs")
    );
    assert_eq!(
        resolve_link(&literal("/w/proj", "../x.rs", 1, 1)),
        Err(NoteError::ParentTraversal)
    );
    assert_eq!(
        resolve_link(&literal("/w/../proj", "x.rs", 1, 1)),
        Err(NoteError::ParentTraversal)
    );
    assert_eq!(
        resolve_link(&literal("/w/proj", "x.rs", 0, 1)),
        Err(NoteError::LineZero)
    );
    assert_eq!(
        resolve_link(&literal("/w/proj", "/etc/x", 1, 1)),
        Err(NoteError::PathNotWorkspaceRelative)
    );
    assert_eq!(
        resolve_link(&literal("/w/proj", "", 1, 1)),
        Err(NoteError::PathNotWorkspaceRelative)
    );
}

#[test]
fn le_righe_si_tagliano_al_testo() {
    assert_eq!(clamp_lines(3, 5, 10), (2, 4));
    assert_eq!(clamp_lines(8, 20, 10), (7, 9));
    assert_eq!(clamp_lines(30, 40, 10), (9, 9));
    assert_eq!(clamp_lines(1, 1, 0), (0, 0));
    assert_eq!(clamp_lines(1, 1, 1), (0, 0));
}

#[test]
fn si_rimuove_un_collegamento_per_indice() {
    let mut note = Note::empty(NoteId::from("n"));
    note.add_link(link("a.rs", 1, 1));
    note.add_link(link("b.rs", 2, 2));
    let removed = note.remove_link(0).unwrap();
    assert_eq!(removed.path, PathBuf::from("a.rs"));
    assert_eq!(note.header.links.len(), 1);
    assert_eq!(note.header.links[0].path, PathBuf::from("b.rs"));
    assert!(note.remove_link(5).is_none());
    assert_eq!(note.header.links.len(), 1);
}
