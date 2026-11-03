//! Preflight dei percorsi prima delle scritture Git sul worktree.
//!
//! Nomi Git non lossy, protezioni .git/HFS/NTFS e metadata no-follow di
//! ogni antenato. Non è confinamento resistente a gare: un altro processo
//! può cambiare la topologia dopo il controllo. Nessun handle/temp su UI;
//! questi helper sono chiamati dal worker e andranno riusati dal pull.
use crate::GitError;
use gix::bstr::{BString, ByteSlice};
use std::{
    fs,
    path::{Component, Path},
};

pub(crate) fn git_path(path: &Path) -> Result<BString, GitError> {
    let raw =
        gix::path::try_into_bstr(path).map_err(|error| GitError::Worktree(error.to_string()))?;
    Ok(gix::path::to_unix_separators_on_windows(raw).into_owned())
}

pub(crate) fn check_safe_path(path: &Path) -> Result<(), GitError> {
    let raw = git_path(path)?;
    if raw.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(GitError::Worktree(
            "percorso vuoto, assoluto o relativo con componenti non sicuri".into(),
        ));
    }
    // components() normalizza '.' interni e separatori ripetuti: non usarlo
    // per accettare byte ostili provenienti da un albero remoto.
    for part in raw.split(|byte| *byte == b'/') {
        if part.contains(&0) {
            return Err(GitError::Worktree("il nome contiene NUL".into()));
        }
        gix::validate::path::component(
            part.as_bstr(),
            None,
            gix::validate::path::component::Options {
                protect_windows: cfg!(windows),
                protect_hfs: true,
                protect_ntfs: true,
            },
        )
        .map_err(|error| GitError::Worktree(format!("percorso worktree rifiutato: {error}")))?;
    }
    Ok(())
}

pub(crate) fn linklike(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// File regolare scrivibile oppure assente, senza seguire link/junction.
/// Parent assente è accettato: creazione e successivo controllo sono worker.
pub(crate) fn destination_metadata(
    root: &Path,
    relative: &Path,
) -> Result<Option<fs::Metadata>, GitError> {
    check_safe_path(relative)?;
    let meta = fs::symlink_metadata(root).map_err(|e| GitError::Worktree(e.to_string()))?;
    if !meta.is_dir() || linklike(&meta) {
        return Err(GitError::Worktree(
            "radice worktree non regolare o link/reparse point".into(),
        ));
    }
    let mut full = root.to_path_buf();
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        full.push(part);
        let meta = match fs::symlink_metadata(&full) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(GitError::Worktree(error.to_string())),
        };
        if linklike(&meta) {
            return Err(GitError::Worktree(
                "link simbolico o reparse point nel percorso: ripristino rifiutato".into(),
            ));
        }
        if parts.peek().is_none() {
            if !meta.is_file() {
                return Err(GitError::Worktree(
                    "destinazione non regolare: cartella/dispositivo non ripristinabile".into(),
                ));
            }
            if meta.permissions().readonly() {
                return Err(GitError::Worktree("destinazione in sola lettura".into()));
            }
            return Ok(Some(meta));
        }
        if !meta.is_dir() {
            return Err(GitError::Worktree("un antenato non è una cartella".into()));
        }
    }
    Err(GitError::Worktree("destinazione vuota".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadati_e_componenti_ostili_non_passano() {
        for path in [
            "",
            ".",
            "../fuori",
            "dir/../x",
            "dir/./x",
            "dir//x",
            "/root",
            ".git/config",
            "dir/.GIT/a",
            "git~1/config",
            ".git./a",
            ".git /a",
            ".g\u{200c}it/config",
            "NUL\0",
        ] {
            assert!(check_safe_path(Path::new(path)).is_err(), "{path:?}");
        }
    }
    #[test]
    fn nomi_normali_non_sono_normalizzati_o_rifiutati() {
        for path in [
            "a.txt",
            "dir/sub/file",
            ".github/ci.yml",
            "dir/.gitignore",
            "nome con spazi.sql",
        ] {
            assert!(check_safe_path(Path::new(path)).is_ok(), "{path:?}");
        }
    }
}
