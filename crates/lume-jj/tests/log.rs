#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use lume_jj::{JjError, JjLogEntry, LOG_TEMPLATE, parse_log};

const CORRENTE: &str = r#"{"change":"kkmpptxz","commit":"3f2a91c0d4e5","description":"aggiunge \"citazioni\", tab\te à ü","author":"Rossi","when":"2026-09-29 10:15","bookmarks":"main dev","working_copy":true,"empty":false,"conflict":false,"immutable":false}"#;
const RADICE: &str = r#"{"change":"zzzzzzzz","commit":"000000000000","description":"","author":"","when":"1970-01-01 00:00","bookmarks":"","working_copy":false,"empty":true,"conflict":false,"immutable":true}"#;

#[test]
fn parse_log_legge_una_riga_per_cambiamento() {
    let testo = format!("{CORRENTE}\n{RADICE}\n");

    let entries = parse_log(&testo).unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries[0],
        JjLogEntry {
            change: "kkmpptxz".into(),
            commit: "3f2a91c0d4e5".into(),
            description: "aggiunge \"citazioni\", tab\te à ü".into(),
            author: "Rossi".into(),
            when: "2026-09-29 10:15".into(),
            bookmarks: "main dev".into(),
            working_copy: true,
            empty: false,
            conflict: false,
            immutable: false,
        }
    );
    assert!(entries[1].immutable && entries[1].empty && !entries[1].working_copy);
}

#[test]
fn parse_log_ignora_righe_vuote_e_crlf() {
    let testo = format!("\r\n{CORRENTE}\r\n\r\n{RADICE}\r\n");

    let entries = parse_log(&testo).unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].change, "kkmpptxz");
}

#[test]
fn parse_log_di_un_testo_vuoto_e_una_lista_vuota() {
    assert_eq!(parse_log("").unwrap(), Vec::new());
    assert_eq!(parse_log("\n\n").unwrap(), Vec::new());
}

#[test]
fn una_riga_non_json_da_un_errore_chiaro() {
    let errore = parse_log("Error: something went wrong\n").unwrap_err();

    match errore {
        JjError::Parse(messaggio) => {
            assert!(messaggio.contains("riga 1"), "{messaggio}");
            assert!(
                messaggio.contains("Error: something went wrong"),
                "{messaggio}"
            );
        }
        altro => panic!("atteso Parse, trovato {altro:?}"),
    }
}

#[test]
fn una_riga_valida_seguita_da_spazzatura_indica_la_riga_giusta() {
    let testo = format!("{CORRENTE}\nnon-json\n");

    let errore = parse_log(&testo).unwrap_err();

    assert!(errore.to_string().contains("riga 2"), "{errore}");
}

#[test]
fn un_campo_mancante_e_un_errore_di_parse() {
    let errore = parse_log("{\"change\":\"a\"}\n").unwrap_err();

    assert!(matches!(errore, JjError::Parse(_)), "{errore:?}");
}

#[test]
fn il_template_ha_tutti_i_campi_e_il_newline_finale_come_letterale() {
    for campo in [
        "change",
        "commit",
        "description",
        "author",
        "when",
        "bookmarks",
        "working_copy",
        "empty",
        "conflict",
        "immutable",
    ] {
        assert!(
            LOG_TEMPLATE.contains(&format!("\"{campo}\":")),
            "manca il campo {campo}: {LOG_TEMPLATE}"
        );
    }
    // Nei letterali a singolo apice di jj non c'è escape: il newline che chiude
    // la riga è il letterale a doppio apice "\n", e il testo del template non
    // contiene mai un a-capo vero.
    assert!(LOG_TEMPLATE.ends_with(r#""\n""#), "{LOG_TEMPLATE}");
    assert!(!LOG_TEMPLATE.contains('\n'));
}
