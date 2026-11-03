#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Controlli sul `Cargo.lock`: leggerlo costa niente e cattura due errori che
//! un `cargo build` non segnala (due copie di gix; due provider TLS).

use std::path::PathBuf;

fn lockfile() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("Cargo.lock");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Versioni con cui un pacchetto compare nel lockfile, nell'ordine del file.
fn versions_of(lock: &str, package: &str) -> Vec<String> {
    let header = format!("name = \"{package}\"");
    let mut found = Vec::new();
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim_end() == header
            && let Some(version) = lines.next().and_then(|l| l.strip_prefix("version = "))
        {
            found.push(version.trim_matches('"').to_owned());
        }
    }
    found
}

#[test]
fn gix_e_una_sola_versione_0_88() {
    let versions = versions_of(&lockfile(), "gix");
    assert_eq!(
        versions.len(),
        1,
        "gix deve comparire una volta: {versions:?}"
    );
    assert!(
        versions[0].starts_with("0.88."),
        "gix deve essere 0.88.x, e' {}",
        versions[0]
    );
}

/// Un solo `reqwest` vuol dire un solo provider TLS (precondizione P2 del
/// piano, voce L.3): con due versioni, la prima connessione HTTPS entra in
/// panic perché due provider rustls si contendono il valore predefinito
/// (commento a `Cargo.toml` su reqwest).
#[test]
fn reqwest_e_una_sola_versione() {
    let versions = versions_of(&lockfile(), "reqwest");
    assert_eq!(
        versions.len(),
        1,
        "reqwest deve comparire una volta sola (Step 3b del Task 3: \
         portare reqwest a 0.13 nel Cargo.toml del workspace): {versions:?}"
    );
}
