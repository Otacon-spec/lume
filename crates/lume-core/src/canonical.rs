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

/// Come [`canonical_path`], ma accetta anche un percorso che non esiste:
/// risolve il più profondo antenato esistente e riusa la coda scritta.
///
/// Serve ai percorsi che l'app confronta prima che il file nasca (una
/// definizione LSP, una destinazione di salvataggio): un antenato symlink
/// cambia il prefisso, il tratto che manca no. Un'ultima componente che è un
/// link pendente resta il suo nome: non si segue un link senza destinazione.
pub fn canonical_path_lenient(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    match canonical_path(path) {
        Ok(canonical) => Ok(canonical),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let Some(name) = path.file_name() else {
                return Err(error);
            };
            let Some(parent) = path.parent() else {
                return Err(error);
            };
            let mut parent = canonical_path_lenient(parent)?;
            parent.push(name);
            Ok(parent)
        }
        Err(error) => Err(error),
    }
}

/// Vero se `path` sta sotto `root` (o coincide con essa).
///
/// Il confronto è sempre canonico: un antenato symlink non cambia
/// l'appartenenza, e una cartella link che punta fuori non «sembra» interna
/// solo perché il suo nome scritto comincia con quello della radice. Il
/// tratto finale può non esistere ancora; non è un confinamento anti-TOCTOU,
/// quindi il chiamante rivalida prima di scrivere.
pub fn path_is_within(root: &Path, path: &Path) -> bool {
    if path
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        return false;
    }
    if !root.is_absolute() || !path.is_absolute() {
        return false;
    }
    let Ok(root_canonico) = canonical_path(root) else {
        // Radice non (più) esistente: non c'è un'identità da risolvere. Resta
        // il confronto scritto, già ripulito da «..», per gli usi che devono
        // conservare una cartella appena rimossa (scope del watcher).
        return path.starts_with(root);
    };
    let Ok(path) = canonical_path_lenient(path) else {
        return false;
    };
    path.starts_with(&root_canonico)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::{canonical_path, canonical_path_lenient};

    #[test]
    fn il_percorso_canonico_risolve_anche_un_ultimo_tratto_assente() {
        let cartella = tempfile::tempdir().unwrap();
        let radice = canonical_path(cartella.path()).unwrap();
        let file = radice.join("assente").join("a.txt");
        assert_eq!(canonical_path_lenient(&file).unwrap(), file);
        assert_eq!(canonical_path_lenient(&radice).unwrap(), radice);
    }

    #[test]
    fn l_appartenenza_alla_radice_unisce_le_forme_scritte() {
        let cartella = tempfile::tempdir().unwrap();
        let radice = canonical_path(cartella.path()).unwrap();
        assert!(super::path_is_within(&radice, &radice));
        assert!(super::path_is_within(
            &radice,
            &radice.join("assente").join("a.txt")
        ));
        assert!(!super::path_is_within(&radice, &radice.join("..")));
        let rimossa = radice.join("rimossa");
        assert!(super::path_is_within(&rimossa, &rimossa.join("a.txt")));
        #[cfg(unix)]
        {
            let link = cartella.path().join("link");
            std::os::unix::fs::symlink(&radice, &link).unwrap();
            assert!(super::path_is_within(&link, &radice.join("a.txt")));
            assert!(super::path_is_within(
                &radice,
                &link.join("assente").join("a.txt")
            ));
        }
    }

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
