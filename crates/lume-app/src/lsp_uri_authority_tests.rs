//! URI tipizzate reali: test di conversione, non server/GUI/confinement.
use super::path_from_str;
#[cfg(unix)]
use super::uri_to_path;
#[cfg(unix)]
use lsp_types::Uri;
use std::path::PathBuf;

#[cfg(unix)]
#[test]
fn localhost_non_diventa_segmento_relativo_e_preserva_unicode()
-> Result<(), Box<dyn std::error::Error>> {
    let uri: Uri = "file://localhost/tmp/citt%C3%A0.rs".parse()?;
    let path = uri_to_path(&uri);
    assert_eq!(path, Some(PathBuf::from("/tmp/città.rs")));
    assert!(path.is_some_and(|p| p.is_absolute()));
    Ok(())
}
#[cfg(unix)]
#[test]
fn host_locale_case_insensitive_e_schema_file_case_insensitive() {
    assert_eq!(
        path_from_str("FILE://LOCALHOST/tmp/ok.rs"),
        Some(PathBuf::from("/tmp/ok.rs"))
    );
}
#[cfg(unix)]
#[test]
fn autorita_remota_non_si_spaccia_per_path_locale_relativo() {
    assert!(path_from_str("file://server/share/file.rs").is_none());
    assert!(path_from_str("file://127.0.0.1/tmp/file.rs").is_none());
}
#[test]
fn userinfo_port_fragment_query_non_diventano_nomi_di_file() {
    for uri in [
        "file://utente@localhost/tmp/file.rs",
        "file://localhost:42/tmp/file.rs",
        "file:///tmp/file.rs?query",
        "file:///tmp/file.rs#fragment",
    ] {
        assert!(path_from_str(uri).is_none(), "URI non ammessa: {uri}");
    }
}
#[test]
fn nul_percentuale_non_diventa_path_operativo() {
    assert!(path_from_str("file:///tmp/nome%00file.rs").is_none());
}
#[cfg(unix)]
#[test]
fn caratteri_riservati_codificati_restano_nel_nome_non_nella_uri() {
    assert_eq!(
        path_from_str("file://localhost/tmp/nome%23%3F%25%5C.rs"),
        Some(PathBuf::from("/tmp/nome#?%\\.rs"))
    );
}
#[cfg(windows)]
#[test]
fn localhost_con_lettera_di_unita_non_diventa_unc() {
    assert_eq!(
        path_from_str("file://localhost/C:/dir/nome.rs"),
        Some(PathBuf::from(r"C:\dir\nome.rs"))
    );
}
#[cfg(windows)]
#[test]
fn host_unc_distinto_da_localhost_preserva_condivisione() {
    assert_eq!(
        path_from_str("file://server/share/nome.rs"),
        Some(PathBuf::from(r"\\server\share\nome.rs"))
    );
}
