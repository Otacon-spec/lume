//! Diagnostic release opt-in, corpus identical to the GUI31MiB probe.
#![cfg(all(target_os = "linux", target_env = "gnu"))]
#![allow(unsafe_code)]
use lume_syntax::{LanguageId, SyntaxState};
use ropey::Rope;
use std::time::Instant;

// ABI checked against the host glibc malloc.h; diagnostic only.
#[repr(C)]
struct MallInfo {
    arena: usize,
    ordblks: usize,
    smblks: usize,
    hblks: usize,
    hblkhd: usize,
    usmblks: usize,
    fsmblks: usize,
    uordblks: usize,
    fordblks: usize,
    keepcost: usize,
}
unsafe extern "C" {
    fn mallinfo2() -> MallInfo;
    fn malloc_trim(pad: usize) -> std::ffi::c_int;
}

fn trim(label: &str) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    // SAFETY: glibc malloc_trim takes a size_t and no pointers, and is thread-safe.
    let released = unsafe { malloc_trim(0) };
    println!(
        "SYNTAX_TRIM,{label},{released},{}us,{}",
        start.elapsed().as_micros(),
        memory()?
    );
    Ok(())
}

fn memory() -> std::io::Result<String> {
    // SAFETY: mallinfo2 has no arguments; repr(C) matches ten size_t fields.
    let info = unsafe { mallinfo2() };
    Ok(format!(
        "{};arena={};used={};free={};mmap={}",
        std::fs::read_to_string("/proc/self/smaps_rollup")?
            .lines()
            .filter(|line| line.starts_with("Rss:") || line.starts_with("Pss:"))
            .collect::<Vec<_>>()
            .join(";"),
        info.arena,
        info.uordblks,
        info.fordblks,
        info.hblkhd
    ))
}

#[test]
#[ignore = "diagnostic parser and tree ownership, direct contiguous source"]
fn parser_tree_ownership_memory() -> Result<(), Box<dyn std::error::Error>> {
    let raw = corpus();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into())?;
    println!("PARSER_OWNER,baseline,{}", memory()?);
    let tree = parser.parse(&raw, None).ok_or("parse failed")?;
    assert_eq!(tree.root_node().end_byte(), raw.len());
    println!("PARSER_OWNER,parsed,{}", memory()?);
    parser.reset();
    println!("PARSER_OWNER,reset,{}", memory()?);
    drop(parser);
    assert_eq!(tree.root_node().end_byte(), raw.len());
    println!("PARSER_OWNER,drop_parser,{}", memory()?);
    drop(tree);
    println!("PARSER_OWNER,drop_tree,{}", memory()?);
    drop(raw);
    println!("PARSER_OWNER,drop_source,{}", memory()?);
    Ok(())
}

fn corpus() -> String {
    let prefix = "// città 漢字 😀\nfn main() { println!(\"é\"); }\n";
    let padding = "// padding ascii per corpus\n";
    let remaining = 32_505_856 - prefix.len() - 1;
    format!(
        "{prefix}\n{}{}",
        padding.repeat(remaining / padding.len()),
        " ".repeat(remaining % padding.len())
    )
}

#[test]
#[ignore = "isolated release memory diagnostic, not a GUI budget"]
fn rust_gui_corpus_parse_query_drop_memory() -> Result<(), Box<dyn std::error::Error>> {
    let reclaim = std::env::var_os("LUME_DIAGNOSTIC_TRIM").is_some();
    if reclaim && std::env::var_os("LD_PRELOAD").is_some() {
        return Err("trim diagnostic requires native glibc allocator".into());
    }
    let raw = corpus();
    assert_eq!(raw.len(), 32_505_856);
    let rope = Rope::from_str(&raw);
    println!("SYNTAX_MEMORY,baseline,{}", memory()?);
    let start = Instant::now();
    let state = SyntaxState::new(LanguageId::Rust, rope.slice(..)).ok_or("parse failed")?;
    println!(
        "SYNTAX_MEMORY,parse,{}us,{}",
        start.elapsed().as_micros(),
        memory()?
    );
    let start = Instant::now();
    let snapshot = state
        .highlight_snapshot(rope.slice(..), 0, 0..1024)
        .ok_or("query failed")?;
    println!(
        "SYNTAX_MEMORY,query,{}us,{}",
        start.elapsed().as_micros(),
        memory()?
    );
    assert!(!snapshot.tokens().is_empty());
    assert!(snapshot.covers(LanguageId::Rust, 0, 0..1024));
    if reclaim {
        trim("live_state")?;
        let repeated = state
            .highlight_snapshot(rope.slice(..), 0, 0..1024)
            .ok_or("query after trim failed")?;
        assert_eq!(snapshot.tokens(), repeated.tokens());
    }
    drop(snapshot);
    println!("SYNTAX_MEMORY,drop_snapshot,{}", memory()?);
    drop(state);
    println!("SYNTAX_MEMORY,drop_state,{}", memory()?);
    drop(rope);
    drop(raw);
    println!("SYNTAX_MEMORY,drop_source,{}", memory()?);
    if reclaim {
        trim("after_drop")?;
    }
    Ok(())
}
