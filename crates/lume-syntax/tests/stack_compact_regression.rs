#![allow(clippy::unwrap_used)]
use lume_syntax::{InputEditRaw, LanguageId::*, SyntaxState};
use ropey::Rope;
fn end(text: &str) -> (usize, usize) {
    let row = text.bytes().filter(|b| *b == b'\n').count();
    (
        row,
        text.rfind('\n').map_or(text.len(), |n| text.len() - n - 1),
    )
}
#[test]
fn all_language_trees_and_queries() {
    let mut actual = String::new();
    let cases = [
        (Rust, "fn f(){let x=1;}"),
        (Toml, "[a]\nx=1\n"),
        (Markdown, "# Heading\n`code`\n"),
        (Java, "class A { int x=1; }"),
        (C, "int f(){return 1;}"),
        (Cpp, "template<class T> T f(T x){return x;}"),
        (CSharp, "class A { int X=1; }"),
        (Python, "def f():\n    return 1\n"),
        (JavaScript, "const x = a => a+1;"),
        (TypeScript, "const x: number=1;"),
        (Tsx, "const x=<A value={1}/>;"),
        (Json, "{\"x\":1}"),
        (Yaml, "x: 1\n"),
        (Html, "<p class=\"x\">text</p>"),
        (Css, "p { color: red; }"),
        (Sql, "select 1 as x;"),
        (Kotlin, "fun f(): Int = 1"),
        (Xml, "<a x=\"1\"><b/></a>"),
    ];
    for (lang, seed) in cases {
        let mut old = seed.to_string();
        let initial = Rope::from_str(&old);
        let mut state = SyntaxState::new(lang, initial.slice(..)).unwrap();
        let variants = [
            seed.to_string(),
            format!("\n{seed}"),
            format!("{seed}\n"),
            seed.chars().skip(1).collect::<String>(),
            format!("{}😀", seed.replace('1', "2")),
        ];
        for (step, text) in variants.iter().enumerate() {
            let rope = Rope::from_str(text);
            state.apply_edit(
                &InputEditRaw {
                    start_byte: 0,
                    old_end_byte: old.len(),
                    new_end_byte: text.len(),
                    start_point: (0, 0),
                    old_end_point: end(&old),
                    new_end_point: end(text),
                },
                rope.slice(..),
            );
            let fresh = SyntaxState::new(lang, rope.slice(..)).unwrap();
            let snapshot = state
                .highlight_snapshot(rope.slice(..), step as u64, 0..rope.len_bytes())
                .unwrap();
            let oracle = fresh
                .highlight_snapshot(rope.slice(..), step as u64, 0..rope.len_bytes())
                .unwrap();
            assert_eq!(
                state.tree().root_node().to_sexp(),
                fresh.tree().root_node().to_sexp()
            );
            assert_eq!(snapshot.tokens(), oracle.tokens());
            actual.push_str(&format!(
                "LANG_ORACLE,{lang:?},{step},{:?},{:?}\n",
                state.tree().root_node().to_sexp(),
                snapshot.tokens()
            ));
            old = text.clone();
        }
    }
    assert_eq!(
        actual,
        include_str!("fixtures/stack-compact-languages.golden")
    );
}
