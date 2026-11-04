//! Percorso canonico confrontabile con i percorsi che l'app conosce.
//!
//! Su Windows `std::fs::canonicalize` restituisce la forma verbatim
//! (`\\?\D:\...`), mentre la radice del progetto, i documenti aperti e
//! `std::path::absolute` usano `D:\...`: un confronto o `starts_with` fra le
//! due forme fallisce sempre. `dunce` toglie il prefisso solo quando il
//! percorso resta rappresentabile senza (niente nomi riservati, lunghezza
//! entro i limiti); altrimenti lo conserva. Su Unix coincide con
//! `std::fs::canonicalize`.

use std::io;
use std::path::{Path, PathBuf};

/// Come [`std::fs::canonicalize`], ma nella forma Win32 ordinaria quando
/// possibile. Usarla ovunque il risultato si confronta con altri percorsi.
pub fn canonical_path(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    dunce::canonicalize(path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::canonical_path;

    #[test]
    fn il_percorso_canonico_resta_confrontabile_con_quello_assoluto() {
        let cartella = tempfile::tempdir().unwrap();
        let file = cartella.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let radice = canonical_path(cartella.path()).unwrap();
        let canonico = canonical_path(&file).unwrap();
        assert!(
            canonico.starts_with(&radice),
            "{canonico:?} fuori da {radice:?}"
        );
        assert_eq!(canonico, radice.join("a.txt"));
        #[cfg(windows)]
        assert!(
            !canonico.to_string_lossy().starts_with(r"\\?\"),
            "{canonico:?}"
        );
    }
}
