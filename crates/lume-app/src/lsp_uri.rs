//! Una sola forma canonica di URI, in andata e in ritorno.
//!
//! ATTENZIONE: È il punto dove si sbaglia **in silenzio**. Se il `didOpen` manda una
//! URI anche solo scritta diversamente da quella con cui tornano i
//! `publishDiagnostics`, rust-analyzer vede **due file**: i problemi si
//! duplicano, «vai alla definizione» apre una scheda in più, e niente lo
//! segnala — il protocollo non ha modo di accorgersene. Le due conversioni
//! stanno quindi nello stesso posto, con un test di andata e ritorno.
//!
//! Il percorso `file://` non è una banalità su Windows: una lettera di unità
//! vuole una barra in più (`file:///C:/…`), un percorso UNC ne vuole una in
//! meno (`file://server/condivisione/…`), e tutto quel che non è un carattere
//! di percorso va codificato in percentuale — spazi compresi, che nei nomi di
//! cartella di Windows sono la regola, non l'eccezione.

use std::path::{Path, PathBuf};

use lsp_types::Uri;

/// I byte che in un percorso restano sé stessi: `unreserved` più `sub-delims`
/// più `:` e `@` (RFC 3986 §3.3), cioè `pchar`. La barra la aggiunge chi
/// costruisce. Tutto il resto — spazio, `%`, `#`, `?`, i non-ASCII — si
/// codifica: `%` compreso, o un file che ha una percentuale nel nome non
/// tornerebbe indietro uguale.
const fn is_pchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'@'
        )
}

fn push_encoded(out: &mut String, segment: &str) {
    for byte in segment.bytes() {
        if is_pchar(byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
}

/// Da percorso a URI. `None` se il percorso non è assoluto: una URI relativa
/// non esiste, e inventarne una farebbe aprire al server un file a caso.
pub fn path_to_uri(path: &Path) -> Option<Uri> {
    if !path.is_absolute() {
        return None;
    }
    let testo = path.to_str()?;
    let mut uri = String::with_capacity(testo.len() + 16);
    uri.push_str("file://");
    #[cfg(windows)]
    {
        // UNC: `\\server\condivisione\x` → `file://server/condivisione/x`.
        // L'autorità è il server, e **non** si codifica come un segmento.
        if let Some(unc) = testo
            .strip_prefix(r"\\?\UNC\")
            .or_else(|| testo.strip_prefix(r"\\"))
        {
            for (indice, segmento) in unc.split(['\\', '/']).enumerate() {
                if indice > 0 {
                    uri.push('/');
                }
                push_encoded(&mut uri, segmento);
            }
            return uri.parse().ok();
        }
    }
    #[cfg(windows)]
    let segmenti = {
        // La lettera di unità non ha una barra iniziale: la si aggiunge qui.
        uri.push('/');
        testo
            .strip_prefix(r"\\?\")
            .unwrap_or(testo)
            .split(['\\', '/'])
    };
    #[cfg(not(windows))]
    // La barra iniziale del percorso POSIX è già il primo separatore.
    // Il backslash su Unix è un carattere del nome, non un separatore.
    let segmenti = testo.split('/');
    for (indice, segmento) in segmenti.enumerate() {
        if indice > 0 {
            uri.push('/');
        }
        push_encoded(&mut uri, segmento);
    }
    uri.parse().ok()
}

/// Da URI a percorso assoluto. `None` per schema non file, componenti non
/// filesystem, percent encoding invalido o autorità non supportata. Non è
/// canonicalizzazione filesystem, confinamento o permesso di apertura.
pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    path_from_str(uri.as_str())
}

/// Come [`uri_to_path`], ma da testo grezzo. **Privata di proposito**: una
/// sola porta pubblica vuol dire una sola forma canonica, ed è tutto il
/// senso di questo modulo. Le URI che arrivano dal server sono già `Uri`.
fn path_from_str(uri: &str) -> Option<PathBuf> {
    let parsed: Uri = uri.parse().ok()?;
    if !parsed.scheme()?.as_str().eq_ignore_ascii_case("file")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let authority = parsed.authority()?;
    if authority.userinfo().is_some() || authority.port().is_some() {
        return None;
    }
    // Separare i componenti PRIMA del decoding. Un host non è un segmento
    // del pathname, e %23/%3F nel path non diventano fragment/query.
    let host = authority.as_str();
    let local = host.is_empty() || host.eq_ignore_ascii_case("localhost");
    let decoded = percent_decode(parsed.path().as_str())?;
    if decoded.contains('\0') {
        return None;
    }
    #[cfg(not(windows))]
    if !local {
        return None;
    } // Niente UNC remoto trasformato in relativo.
    #[cfg(windows)]
    if !local {
        let path = PathBuf::from(format!(r"\\{}{}", host, decoded.replace('/', r"\")));
        return path.is_absolute().then_some(path);
    }
    #[cfg(windows)]
    let decoded = if decoded.starts_with('/') && decoded.as_bytes().get(2) == Some(&b':') {
        decoded.get(1..)?.replace('/', r"\")
    } else {
        decoded.replace('/', r"\")
    };
    let path = PathBuf::from(decoded);
    path.is_absolute().then_some(path)
}

#[allow(clippy::indexing_slicing)] // index < bytes.len() è la guardia del ciclo
fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex(*bytes.get(index + 1)?)?;
            let low = hex(*bytes.get(index + 2)?)?;
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
#[path = "lsp_uri_authority_tests.rs"]
mod authority_tests;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::{path_to_uri, uri_to_path};
    use std::path::{Path, PathBuf};

    fn andata_e_ritorno(percorso: &str) -> (String, PathBuf) {
        let uri = path_to_uri(Path::new(percorso)).expect("percorso assoluto");
        let indietro = uri_to_path(&uri).expect("URI file:");
        (uri.as_str().to_owned(), indietro)
    }

    /// ATTENZIONE: Il test che vale: se andata e ritorno non coincidono, il server
    /// vede due file dove ce n'è uno, e i problemi si duplicano senza che
    /// nulla lo dica.
    #[test]
    #[cfg(windows)]
    fn una_lettera_di_unita_fa_andata_e_ritorno() {
        let (uri, indietro) = andata_e_ritorno(r"C:\Progetti\Lume\src\app.rs");
        assert_eq!(uri, "file:///C:/Progetti/Lume/src/app.rs");
        assert_eq!(indietro, PathBuf::from(r"C:\Progetti\Lume\src\app.rs"));
    }

    /// Un percorso UNC ha una barra **in meno**: il server è l'autorità.
    #[test]
    #[cfg(windows)]
    fn un_percorso_unc_fa_andata_e_ritorno() {
        let (uri, indietro) = andata_e_ritorno(r"\\build01\condivisa\src\main.rs");
        assert_eq!(uri, "file://build01/condivisa/src/main.rs");
        assert_eq!(indietro, PathBuf::from(r"\\build01\condivisa\src\main.rs"));
    }

    /// Gli spazi nei nomi di cartella su Windows sono la regola («Program
    /// Files», «I miei documenti»), e una URI con uno spazio dentro non è
    /// nemmeno una URI.
    #[test]
    #[cfg(windows)]
    fn spazi_accenti_e_percentuali_si_codificano_e_tornano_indietro() {
        let (uri, indietro) = andata_e_ritorno(r"C:\I miei progetti\città 100%\main.rs");
        assert_eq!(
            uri,
            "file:///C:/I%20miei%20progetti/citt%C3%A0%20100%25/main.rs"
        );
        assert_eq!(
            indietro,
            PathBuf::from(r"C:\I miei progetti\città 100%\main.rs")
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn un_percorso_posix_fa_andata_e_ritorno() {
        let (uri, indietro) = andata_e_ritorno("/home/utente/lume/src/app.rs");
        assert_eq!(uri, "file:///home/utente/lume/src/app.rs");
        assert_eq!(indietro, PathBuf::from("/home/utente/lume/src/app.rs"));
    }

    #[test]
    #[cfg(not(windows))]
    fn i_caratteri_speciali_unix_non_diventano_separatori() {
        let path = r"/home/utente/città 100%/nome\file.rs";
        let (uri, indietro) = andata_e_ritorno(path);
        assert_eq!(
            uri,
            "file:///home/utente/citt%C3%A0%20100%25/nome%5Cfile.rs"
        );
        assert_eq!(indietro, PathBuf::from(path));
    }

    /// Una URI relativa non esiste: inventarne una vorrebbe dire far aprire
    /// al server un file che non è quello.
    #[test]
    fn un_percorso_relativo_non_diventa_una_uri() {
        assert!(path_to_uri(Path::new("src/app.rs")).is_none());
    }

    #[test]
    fn quel_che_non_e_un_file_non_diventa_un_percorso() {
        let http: lsp_types::Uri = "https://esempio.it/x.rs".parse().unwrap();
        assert!(uri_to_path(&http).is_none());
    }

    /// Una codifica rotta si rifiuta invece di consegnare mezzo percorso.
    /// Va provata sulla versione testuale: una URI cosi non si fa nemmeno
    /// parsare, ed e la stringa grezza che arriva dal server.
    #[test]
    fn una_percentuale_monca_non_da_un_percorso_a_meta() {
        assert!(super::path_from_str("file:///bad%ZZpath").is_none());
        assert!("file:///C:/x/%zz.rs".parse::<lsp_types::Uri>().is_err());
    }
}
