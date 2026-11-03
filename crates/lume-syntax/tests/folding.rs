#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use lume_syntax::{FoldRegion, LanguageId, SyntaxState};
use ropey::Rope;
use std::sync::atomic::AtomicBool;

#[test]
fn rust_produce_le_regioni_multilinea() {
    let src = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n\nstruct Foo {\n    a: i32,\n    b: i32,\n}\n\nimpl Foo {\n    fn bar(&self) -> i32 { self.a + self.b }\n}\n\nfn one_line() { 42 }\n\n/* multi\nline comment */\n";
    let rope = Rope::from_str(src);
    let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
    let (regions, rejected) = state.folding_regions(rope.slice(..));
    assert!(!rejected);
    assert_eq!(
        regions,
        vec![
            FoldRegion {
                start_line: 0,
                end_line: 3
            },
            FoldRegion {
                start_line: 5,
                end_line: 8
            },
            FoldRegion {
                start_line: 10,
                end_line: 12
            },
            FoldRegion {
                start_line: 16,
                end_line: 17
            },
        ]
    );
    // la funzione su una riga non produce una regione
    assert!(!regions.iter().any(|r| r.start_line == 14));
}

#[test]
fn rust_annidate_sono_ammesse() {
    let src = "fn outer() {\n    if cond {\n        inner();\n    }\n}\n";
    let rope = Rope::from_str(src);
    let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
    let (regions, rejected) = state.folding_regions(rope.slice(..));
    assert!(!rejected);
    // blocco esterno e blocco interno annidato: entrambi
    assert!(regions.contains(&FoldRegion {
        start_line: 0,
        end_line: 4
    }));
    assert!(regions.contains(&FoldRegion {
        start_line: 1,
        end_line: 3
    }));
}

#[test]
fn rust_documento_vuoto_non_produce_nulla() {
    let rope = Rope::from_str("");
    let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
    let (regions, rejected) = state.folding_regions(rope.slice(..));
    assert!(!rejected);
    assert!(regions.is_empty());
}

#[test]
fn rust_la_graffa_finale_resta_nella_regione() {
    // il delimitatore finale, quando il nodo finisce dopo di esso, fa parte
    // della regione: la riga della graffa viene nascosta insieme al corpo.
    let src = "fn main() {\n    let x = 1;\n}\n";
    let rope = Rope::from_str(src);
    let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
    let (regions, _) = state.folding_regions(rope.slice(..));
    assert!(regions.iter().any(|r| r.start_line == 0 && r.end_line == 2));
}

#[test]
fn la_cancellazione_scarta_il_risultato() {
    let src = "fn main() {\n    let x = 1;\n}\n";
    let rope = Rope::from_str(src);
    let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
    let cancel = AtomicBool::new(true);
    let (regions, rejected) = state.folding_regions_cancellable(rope.slice(..), &cancel);
    assert!(regions.is_empty());
    assert!(!rejected);
}

#[test]
fn il_snapshot_porta_le_regioni() {
    let src = "fn main() {\n    let x = 1;\n}\n";
    let rope = Rope::from_str(src);
    let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
    let snapshot = state
        .highlight_snapshot(rope.slice(..), 7, 0..rope.len_bytes())
        .unwrap();
    assert_eq!(snapshot.folding().len(), 1);
    assert!(!snapshot.folding_rejected());
    assert_eq!(
        snapshot.folding()[0],
        FoldRegion {
            start_line: 0,
            end_line: 2
        }
    );
}

#[test]
fn le_altre_diciassette_lingue_producono_regioni_nel_snapshot() {
    use LanguageId::*;
    for (lingua, testo, fine) in [
        (Toml, "[sezione]\na = 1\nb = 2\n", 2),
        (Markdown, "# Titolo\n\nTesto.\n", 2),
        (Java, "class A {\n    int x;\n}\n", 2),
        (C, "int f() {\n    return 1;\n}\n", 2),
        (Cpp, "class A {\n    int x;\n};\n", 2),
        (CSharp, "class A {\n    int x;\n}\n", 2),
        (Python, "def f():\n    x = 1\n    return x\n", 2),
        (JavaScript, "function f() {\n    return 1;\n}\n", 2),
        (TypeScript, "interface A {\n    x: number;\n}\n", 2),
        (Tsx, "const x = <div>\n    <p>Ciao</p>\n</div>;\n", 2),
        (Json, "{\n    \"a\": 1\n}\n", 2),
        (Yaml, "a:\n  b: 1\n  c: 2\n", 2),
        (Html, "<div>\n    <p>Ciao</p>\n</div>\n", 2),
        (Css, "a {\n    color: red;\n}\n", 2),
        (Sql, "CREATE TABLE a (\n    id INT\n);\n", 2),
        (Kotlin, "fun f() {\n    println(1)\n}\n", 2),
        (Xml, "<radice>\n    <figlio/>\n</radice>\n", 2),
    ] {
        let rope = Rope::from_str(testo);
        let state = SyntaxState::new(lingua, rope.slice(..)).unwrap();
        let snapshot = state
            .highlight_snapshot(rope.slice(..), 1, 0..rope.len_bytes())
            .unwrap();
        assert!(!snapshot.folding_rejected(), "{lingua:?}");
        assert!(
            snapshot.folding().contains(&FoldRegion {
                start_line: 0,
                end_line: fine
            }),
            "{lingua:?}: {:?}",
            snapshot.folding()
        );
    }
}

#[test]
fn le_nuove_lingue_non_comprimono_costrutti_su_una_riga() {
    use LanguageId::*;
    for (lingua, testo) in [
        (Toml, "a = [1, 2]"),
        (Markdown, "# Titolo"),
        (Java, "class A {}"),
        (C, "int f() { return 1; }"),
        (Cpp, "class A {};"),
        (CSharp, "class A {}"),
        (Python, "x = [1, 2]"),
        (JavaScript, "function f() {}"),
        (TypeScript, "interface A { x: number; }"),
        (Tsx, "const x = <div/>;"),
        (Json, "{\"a\": [1, 2]}"),
        (Yaml, "a: [1, 2]"),
        (Html, "<div></div>"),
        (Css, "a { color: red; }"),
        (Sql, "CREATE TABLE a (id INT);"),
        (Kotlin, "fun f() {}"),
        (Xml, "<radice><figlio/></radice>"),
    ] {
        for testo in [testo, ""] {
            let rope = Rope::from_str(testo);
            let state = SyntaxState::new(lingua, rope.slice(..)).unwrap();
            let (regioni, rifiutato) = state.folding_regions(rope.slice(..));
            assert!(!rifiutato, "{lingua:?}");
            assert!(regioni.is_empty(), "{lingua:?}: {regioni:?}");
        }
    }
}

#[test]
fn json_conserva_blocchi_annidati_e_fratelli() {
    let rope = Rope::from_str("{\n  \"a\": [\n    1\n  ],\n  \"b\": {\n    \"c\": 2\n  }\n}\n");
    let state = SyntaxState::new(LanguageId::Json, rope.slice(..)).unwrap();
    let (regioni, rifiutato) = state.folding_regions(rope.slice(..));
    assert!(!rifiutato);
    assert_eq!(
        regioni,
        vec![
            FoldRegion {
                start_line: 0,
                end_line: 7
            },
            FoldRegion {
                start_line: 1,
                end_line: 3
            },
            FoldRegion {
                start_line: 4,
                end_line: 6
            },
        ]
    );
}

#[test]
#[ignore = "diagnostica release del costo folding, non budget UI"]
fn diagnostica_folding_con_migliaia_di_blocchi_fratelli() {
    for quantita in [1_000, 4_000, 16_000] {
        let rope = Rope::from_str(&"fn f() {\n    let x = 1;\n}\n".repeat(quantita));
        let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).unwrap();
        for corsa in 0..5 {
            let inizio = std::time::Instant::now();
            let (regioni, rifiutato) = state.folding_regions(rope.slice(..));
            let microsecondi = inizio.elapsed().as_micros();
            assert!(!rifiutato);
            assert_eq!(regioni.len(), quantita);
            println!("folding,{quantita},{corsa},{microsecondi}");
            std::hint::black_box(regioni);
        }
    }
}
