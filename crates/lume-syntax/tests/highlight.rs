#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Test di integrazione: highlight Rust, reparse incrementale,
//! rilevamento lingua e smoke test TOML/Markdown.

use std::ops::Range;
use std::path::Path;

use ropey::Rope;

use lume_syntax::{InputEditRaw, LanguageId, SyntaxState, TokenClass, detect};

/// Fixture Rust: fn, struct, &str, numero, commento //, #[derive], lifetime.
const RUST_SRC: &str = "#[derive(Debug)]\nstruct Foo<'a> {\n    name: &'a str,\n}\n\n// commento importante\nfn main() {\n    let x = 42;\n    let s = \"ciao\";\n}\n";

/// Classe alla posizione `byte` (None se nessuno span la copre).
fn class_at(hl: &[(Range<usize>, TokenClass)], byte: usize) -> Option<TokenClass> {
    hl.iter().find(|(r, _)| r.contains(&byte)).map(|(_, c)| *c)
}

/// Offset in byte della prima occorrenza di `needle`.
fn off(haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .expect("needle non trovato nella fixture")
}

/// (riga, colonna-in-byte) 0-based di un offset in byte.
fn point_at(text: &str, byte: usize) -> (usize, usize) {
    let before = &text[..byte];
    let row = before.matches('\n').count();
    let col = byte - before.rfind('\n').map_or(0, |i| i + 1);
    (row, col)
}

#[test]
fn rust_highlights_classi_attese() {
    let rope = Rope::from_str(RUST_SRC);
    let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).expect("parse fallito");
    let hl = state.highlights(rope.slice(..), 0..RUST_SRC.len());

    assert!(!hl.is_empty());

    // Span ordinati e non sovrapposti.
    for w in hl.windows(2) {
        assert!(
            w[0].0.end <= w[1].0.start,
            "span sovrapposti o fuori ordine: {:?} poi {:?}",
            w[0],
            w[1]
        );
    }

    // Parole chiave.
    assert_eq!(
        class_at(&hl, off(RUST_SRC, "fn ")),
        Some(TokenClass::Keyword)
    );
    assert_eq!(
        class_at(&hl, off(RUST_SRC, "struct ")),
        Some(TokenClass::Keyword)
    );
    assert_eq!(
        class_at(&hl, off(RUST_SRC, "let x")),
        Some(TokenClass::Keyword)
    );
    // Tipi: nominale e builtin (&'a str).
    assert_eq!(class_at(&hl, off(RUST_SRC, "Foo")), Some(TokenClass::Type));
    assert_eq!(
        class_at(&hl, off(RUST_SRC, " str,") + 1),
        Some(TokenClass::Type)
    );
    // Nome di funzione.
    assert_eq!(
        class_at(&hl, off(RUST_SRC, "main")),
        Some(TokenClass::Function)
    );
    // Letterale stringa (dentro le virgolette).
    assert_eq!(
        class_at(&hl, off(RUST_SRC, "ciao")),
        Some(TokenClass::String)
    );
    // Numero: tree-sitter-rust cattura i letterali interi come
    // `constant.builtin`, quindi la classe attesa è Constant.
    assert_eq!(
        class_at(&hl, off(RUST_SRC, "42")),
        Some(TokenClass::Constant)
    );
    // Commento di riga.
    assert_eq!(
        class_at(&hl, off(RUST_SRC, "// commento")),
        Some(TokenClass::Comment)
    );
    // Attributo #[derive(Debug)]: l'identificatore "derive" ricade
    // nella capture esterna (attribute_item) @attribute.
    assert_eq!(
        class_at(&hl, off(RUST_SRC, "derive")),
        Some(TokenClass::Attribute)
    );
    // Lifetime: (lifetime (identifier) @label) → Keyword sulla "a" di 'a.
    assert_eq!(
        class_at(&hl, off(RUST_SRC, "'a>") + 1),
        Some(TokenClass::Keyword)
    );
    // Campo di struct.
    assert_eq!(
        class_at(&hl, off(RUST_SRC, "name")),
        Some(TokenClass::Property)
    );
    // Punteggiatura annidata dentro l'attributo: vince la capture interna.
    assert_eq!(
        class_at(&hl, off(RUST_SRC, "(Debug")),
        Some(TokenClass::Punctuation)
    );
}

#[test]
fn rust_highlights_sottorange() {
    let rope = Rope::from_str(RUST_SRC);
    let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).expect("parse fallito");
    // Solo la riga con il numero.
    let start = off(RUST_SRC, "let x");
    let end = off(RUST_SRC, "42;") + 3;
    let hl = state.highlights(rope.slice(..), start..end);
    assert_eq!(
        class_at(&hl, off(RUST_SRC, "42")),
        Some(TokenClass::Constant)
    );
}

#[test]
fn java_c_cpp_and_csharp_are_detected_and_highlighted() {
    let fixtures = [
        (
            "Main.java",
            LanguageId::Java,
            "public class Main { static void run() { int value = 42; } }",
        ),
        (
            "main.c",
            LanguageId::C,
            "int main(void) { const char *message = \"hello\"; return 0; }",
        ),
        (
            "main.hpp",
            LanguageId::Cpp,
            "class Main { public: int run() const { return 42; } };",
        ),
        (
            "Program.cs",
            LanguageId::CSharp,
            "public class Program { static void Main() { var value = 42; } }",
        ),
    ];

    for (path, expected_language, source) in fixtures {
        assert_eq!(detect(Path::new(path)), Some(expected_language));
        let rope = Rope::from_str(source);
        let syntax = SyntaxState::new(expected_language, rope.slice(..)).expect("parse fallito");
        let highlights = syntax.highlights(rope.slice(..), 0..source.len());
        assert!(
            highlights
                .iter()
                .any(|(_, class)| *class != TokenClass::Text),
            "nessun token colorato per {path}"
        );
    }
}

#[test]
fn additional_languages_are_detected_and_highlighted() {
    let fixtures = [
        (
            "script.py",
            LanguageId::Python,
            "def greet(name):\n    return f\"Hi {name}\"\n",
        ),
        (
            "component.jsx",
            LanguageId::JavaScript,
            "export const Button = () => <button>OK</button>;\n",
        ),
        (
            "model.ts",
            LanguageId::TypeScript,
            "interface User { name: string }\nconst user: User = { name: \"Ada\" };\n",
        ),
        (
            "card.tsx",
            LanguageId::Tsx,
            "interface Props { name: string }\nexport const Card = ({ name }: Props) => <div>{name}</div>;\n",
        ),
        (
            "settings.json",
            LanguageId::Json,
            "{\n  \"name\": \"Lume\",\n  \"enabled\": true\n}\n",
        ),
        (
            "pipeline.yaml",
            LanguageId::Yaml,
            "name: Lume\nactive: true\n",
        ),
        (
            "index.html",
            LanguageId::Html,
            "<!doctype html><main class=\"app\"><h1>Lume</h1></main>\n",
        ),
        (
            "site.css",
            LanguageId::Css,
            ".app { color: #c70; margin: 1rem; }\n",
        ),
        (
            "schema.sql",
            LanguageId::Sql,
            "SELECT id, name FROM users WHERE active = true;\n",
        ),
        (
            "App.kt",
            LanguageId::Kotlin,
            "class App { fun greet(name: String): String = \"Hi $name\" }\n",
        ),
        (
            "layout.xml",
            LanguageId::Xml,
            "<?xml version=\"1.0\"?><root id=\"1\"><child>ok</child></root>\n",
        ),
    ];

    for (path, expected_language, source) in fixtures {
        assert_eq!(detect(Path::new(path)), Some(expected_language));
        let rope = Rope::from_str(source);
        let syntax = SyntaxState::new(expected_language, rope.slice(..)).expect("parse fallito");
        let highlights = syntax.highlights(rope.slice(..), 0..source.len());
        assert!(
            highlights
                .iter()
                .any(|(_, class)| *class != TokenClass::Text),
            "nessun token colorato per {path}"
        );
    }
}

#[test]
fn detects_additional_language_extension_aliases() {
    let aliases = [
        ("script.pyi", LanguageId::Python),
        ("script.pyw", LanguageId::Python),
        ("module.js", LanguageId::JavaScript),
        ("module.mjs", LanguageId::JavaScript),
        ("module.cjs", LanguageId::JavaScript),
        ("model.mts", LanguageId::TypeScript),
        ("model.cts", LanguageId::TypeScript),
        ("config.yml", LanguageId::Yaml),
        ("page.htm", LanguageId::Html),
        ("build.kts", LanguageId::Kotlin),
        ("schema.xsd", LanguageId::Xml),
        ("sheet.xsl", LanguageId::Xml),
        ("sheet.xslt", LanguageId::Xml),
        ("icon.svg", LanguageId::Xml),
    ];

    for (path, language) in aliases {
        assert_eq!(detect(Path::new(path)), Some(language), "{path}");
    }

    assert_eq!(detect(Path::new("config.jsonc")), None);
}

#[test]
fn reparse_incrementale_equivale_a_parse_completo() {
    let original = "fn main() {\n    let a = 1;\n}\n";
    let insert = "    let b = 2;\n";
    // Inserimento all'inizio della riga di chiusura "}".
    let pos = original.find('}').expect("fixture senza '}'");

    let mut rope = Rope::from_str(original);
    let mut state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).expect("parse fallito");

    rope.insert(rope.byte_to_char(pos), insert);
    let edited = rope.to_string();

    let edit = InputEditRaw {
        start_byte: pos,
        old_end_byte: pos,
        new_end_byte: pos + insert.len(),
        start_point: point_at(original, pos),
        old_end_point: point_at(original, pos),
        new_end_point: point_at(&edited, pos + insert.len()),
    };
    state.apply_edit(&edit, rope.slice(..));

    let fresh = SyntaxState::new(LanguageId::Rust, rope.slice(..)).expect("parse fallito");
    assert_eq!(
        state.tree().root_node().to_sexp(),
        fresh.tree().root_node().to_sexp(),
        "l'albero incrementale differisce dal parse da zero"
    );
}

#[test]
fn detect_estensioni() {
    assert_eq!(detect(Path::new("src/main.rs")), Some(LanguageId::Rust));
    assert_eq!(detect(Path::new("Cargo.toml")), Some(LanguageId::Toml));
    assert_eq!(detect(Path::new("README.md")), Some(LanguageId::Markdown));
    assert_eq!(detect(Path::new("NOTE.MD")), Some(LanguageId::Markdown));
    assert_eq!(detect(Path::new("foo.txt")), None);
    assert_eq!(detect(Path::new("senza_estensione")), None);
}

#[test]
fn toml_smoke() {
    let src = "# commento\ntitle = \"lume\"\n\n[sezione]\nnumero = 42\nflag = true\n";
    let rope = Rope::from_str(src);
    let state = SyntaxState::new(LanguageId::Toml, rope.slice(..)).expect("parse TOML fallito");
    let hl = state.highlights(rope.slice(..), 0..src.len());
    assert!(!hl.is_empty(), "nessuna capture TOML");
    assert_eq!(
        class_at(&hl, src.find("42").unwrap()),
        Some(TokenClass::Number)
    );
    assert_eq!(
        class_at(&hl, src.find("# commento").unwrap()),
        Some(TokenClass::Comment)
    );
}

#[test]
fn markdown_smoke() {
    let src = "# Titolo\n\nparagrafo di testo\n\n- voce di lista\n";
    let rope = Rope::from_str(src);
    let state =
        SyntaxState::new(LanguageId::Markdown, rope.slice(..)).expect("parse Markdown fallito");
    let hl = state.highlights(rope.slice(..), 0..src.len());
    assert!(!hl.is_empty(), "nessuna capture Markdown");
    // Titolo e marcatore devono avere classi colorate visibili nel tema.
    assert_eq!(class_at(&hl, 0), Some(TokenClass::Attribute));
    assert_eq!(
        class_at(&hl, src.find("Titolo").unwrap()),
        Some(TokenClass::Keyword)
    );
}
