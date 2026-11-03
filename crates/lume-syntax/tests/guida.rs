#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

const GUIDA: &str = include_str!("../../../docs/estendere/aggiungere-un-linguaggio.md");
const LANGUAGE_RS: &str = include_str!("../src/language.rs");
const TOKEN_RS: &str = include_str!("../src/token.rs");
const HIGHLIGHT_TEST: &str = include_str!("highlight.rs");
const CARGO_RADICE: &str = include_str!("../../../Cargo.toml");
const CARGO_SYNTAX: &str = include_str!("../Cargo.toml");

/// I prefissi di capture di `class_for_capture`, nell'ordine in cui la
/// funzione li prova (il primo che combacia vince).
const PREFISSI: [&str; 32] = [
    "text.title",
    "text.literal",
    "text.uri",
    "text.reference",
    "punctuation.special",
    "keyword",
    "label",
    "storage",
    "conditional",
    "repeat",
    "include",
    "type",
    "tag",
    "namespace",
    "function",
    "method",
    "string",
    "escape",
    "character",
    "number",
    "float",
    "integer",
    "comment",
    "attribute",
    "constant",
    "boolean",
    "operator",
    "punctuation",
    "property",
    "field",
    "variable",
    "parameter",
];

fn citato(voce: &str) -> bool {
    GUIDA.contains(&format!("`{voce}`"))
}

#[test]
fn la_guida_cita_i_simboli_e_i_simboli_esistono_ancora() {
    let simboli = [
        ("LanguageId", LANGUAGE_RS, "pub enum LanguageId"),
        ("detect", LANGUAGE_RS, "pub fn detect("),
        ("LanguageSpec", LANGUAGE_RS, "pub struct LanguageSpec"),
        ("class_for_capture", LANGUAGE_RS, "fn class_for_capture("),
        ("build_spec", LANGUAGE_RS, "fn build_spec("),
        ("build_spec_parts", LANGUAGE_RS, "fn build_spec_parts("),
        ("spec", LANGUAGE_RS, "pub fn spec("),
        ("TokenClass", TOKEN_RS, "pub enum TokenClass"),
    ];
    for (simbolo, sorgente, firma) in simboli {
        assert!(citato(simbolo), "la guida non cita `{simbolo}`");
        assert!(
            sorgente.contains(firma),
            "`{simbolo}` non esiste più: {firma}"
        );
    }
}

#[test]
fn la_guida_elenca_ogni_classe_di_token() {
    let inizio = TOKEN_RS.find("pub enum TokenClass").unwrap();
    let corpo = &TOKEN_RS[inizio..];
    let corpo = &corpo[corpo.find('{').unwrap() + 1..];
    let corpo = &corpo[..corpo.find('}').unwrap()];
    let varianti: Vec<&str> = corpo
        .lines()
        .map(str::trim)
        .filter(|riga| !riga.is_empty() && !riga.starts_with("//") && !riga.starts_with('#'))
        .map(|riga| riga.trim_end_matches(','))
        .collect();
    assert!(!varianti.is_empty(), "nessuna variante letta da token.rs");
    for variante in varianti {
        assert!(
            citato(variante),
            "la guida non elenca TokenClass::{variante}"
        );
    }
}

#[test]
fn la_tabella_dei_prefissi_coincide_con_class_for_capture() {
    let inizio = LANGUAGE_RS.find("fn class_for_capture(").unwrap();
    let fine = LANGUAGE_RS.find("fn build_spec(").unwrap();
    let corpo = &LANGUAGE_RS[inizio..fine];
    assert_eq!(
        corpo.matches("starts_with(\"").count(),
        PREFISSI.len(),
        "class_for_capture ha prefissi che il test e la guida non elencano (o viceversa)"
    );
    for prefisso in PREFISSI {
        assert!(
            corpo.contains(&format!("starts_with(\"{prefisso}\")")),
            "class_for_capture non usa più il prefisso {prefisso}"
        );
        assert!(
            citato(prefisso),
            "la guida non elenca il prefisso `{prefisso}`"
        );
    }
}

#[test]
fn l_esempio_kotlin_della_guida_e_il_codice_vero() {
    let chiamata = "Some(include_str!(\"../queries/kotlin_folds.scm\"))";
    let braccio = "LanguageId::Kotlin => KOTLIN.as_ref()";
    let estensioni = "\"kt\" | \"kts\" => Some(LanguageId::Kotlin)";
    for riga in [chiamata, braccio, estensioni] {
        assert!(
            LANGUAGE_RS.contains(riga),
            "language.rs non contiene più: {riga}"
        );
        assert!(GUIDA.contains(riga), "la guida non cita più: {riga}");
    }
    for voce in [
        "KOTLIN_HIGHLIGHTS",
        "tree-sitter-kotlin-ng",
        "additional_languages_are_detected_and_highlighted",
        "detects_additional_language_extension_aliases",
    ] {
        assert!(GUIDA.contains(voce), "la guida non nomina {voce}");
    }
    assert!(HIGHLIGHT_TEST.contains("fn additional_languages_are_detected_and_highlighted"));
    assert!(HIGHLIGHT_TEST.contains("fn detects_additional_language_extension_aliases"));
    assert!(CARGO_RADICE.contains("tree-sitter-kotlin-ng"));
    assert!(CARGO_SYNTAX.contains("tree-sitter-kotlin-ng = { workspace = true }"));
    assert!(GUIDA.contains("tree-sitter-kotlin-ng = { workspace = true }"));
}
