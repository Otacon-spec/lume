//! Diagnostica opt-in del costo parse/query durante codice temporaneamente invalido.
use lume_syntax::{InputEditRaw, LanguageId, SyntaxState};
use ropey::Rope;

#[test]
#[ignore = "diagnostica release percorso cancellabile worker, non budget prodotto"]
fn cancellable_worker_parse_and_snapshot_cost() -> Result<(), Box<dyn std::error::Error>> {
    use std::{sync::atomic::AtomicBool, time::Instant};
    let cancel = AtomicBool::new(false);
    let prefix = "// città 漢字 😀\nfn main() { println!(\"é\"); }\n";
    for lines in [40_000, 320_000] {
        let source = format!(
            "{prefix}\n{}",
            "// padding ascii per corpus\n".repeat(lines)
        );
        let mut rope = Rope::from_str(&source);
        let start = Instant::now();
        let mut syntax = SyntaxState::new_cancellable(LanguageId::Rust, rope.slice(..), &cancel)
            .ok_or("initial parse")?;
        let initial_us = start.elapsed().as_micros();
        let offset = prefix.len();
        rope.insert(rope.byte_to_char(offset), "a");
        let edit = InputEditRaw {
            start_byte: offset,
            old_end_byte: offset,
            new_end_byte: offset + 1,
            start_point: (2, 0),
            old_end_point: (2, 0),
            new_end_point: (2, 1),
        };
        let start = Instant::now();
        assert!(syntax.apply_edit_cancellable(&edit, rope.slice(..), &cancel));
        let edit_us = start.elapsed().as_micros();
        let start = Instant::now();
        let snapshot = syntax
            .highlight_snapshot_cancellable(rope.slice(..), 1, 0..2048, &cancel)
            .ok_or("snapshot")?;
        let query_us = start.elapsed().as_micros();
        let oracle: Vec<_> = syntax
            .highlights(rope.slice(..), 0..2048)
            .into_iter()
            .filter_map(|(r, c)| {
                let clipped = r.start..r.end.min(2048);
                (!clipped.is_empty()).then_some((clipped, c))
            })
            .collect();
        assert_eq!(snapshot.tokens(), oracle);
        assert_eq!(syntax.tree().root_node().end_byte(), rope.len_bytes());
        assert!(syntax.tree().root_node().has_error());
        eprintln!(
            "WORKER_PARSE_QUERY bytes={} initial_us={initial_us} edit_us={edit_us} query_us={query_us} tokens={} release={}",
            rope.len_bytes(),
            snapshot.tokens().len(),
            !cfg!(debug_assertions)
        );
    }
    Ok(())
}

#[test]
#[ignore = "diagnostica release esplicita, non budget prodotto"]
fn edit_invalido_inizio_corpus_grande_parse_e_query() -> Result<(), Box<dyn std::error::Error>> {
    let prefix = "// città 漢字 😀\nfn main() { println!(\"é\"); }\n";
    for lines in [40_000, 320_000] {
        let source = format!(
            "{prefix}\n{}",
            "// padding ascii per corpus\n".repeat(lines)
        );
        let mut rope = Rope::from_str(&source);
        let mut syntax =
            SyntaxState::new(LanguageId::Rust, rope.slice(..)).ok_or("Rust non disponibile")?;
        let offset = prefix.len();
        rope.insert(rope.byte_to_char(offset), "a");
        let start = std::time::Instant::now();
        syntax.apply_edit(
            &InputEditRaw {
                start_byte: offset,
                old_end_byte: offset,
                new_end_byte: offset + 1,
                start_point: (2, 0),
                old_end_point: (2, 0),
                new_end_point: (2, 1),
            },
            rope.slice(..),
        );
        let parse_us = start.elapsed().as_micros();
        let start = std::time::Instant::now();
        let highlights = syntax.highlights(rope.slice(..), 0..2048);
        let query_us = start.elapsed().as_micros();
        assert_eq!(syntax.tree().root_node().end_byte(), rope.len_bytes());
        assert!(syntax.tree().root_node().has_error());
        eprintln!(
            "SYNTAX_LARGE_ERROR:bytes={},parse_us={parse_us},query_us={query_us},captures={},release={}",
            rope.len_bytes(),
            highlights.len(),
            !cfg!(debug_assertions)
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "diagnostica Linux release esplicita, non budget prodotto"]
fn memoria_corpus_grande_per_fase() -> Result<(), Box<dyn std::error::Error>> {
    fn measure(phase: &str) -> Result<(), Box<dyn std::error::Error>> {
        let rollup = std::fs::read_to_string("/proc/self/smaps_rollup")?;
        let pss = rollup
            .lines()
            .find_map(|line| {
                line.strip_prefix("Pss:")
                    .and_then(|rest| rest.split_whitespace().next())
            })
            .ok_or("PSS assente")?
            .parse::<u64>()?;
        eprintln!(
            "SYNTAX_MEMORY:phase={phase},pss_kib={pss},release={}",
            !cfg!(debug_assertions)
        );
        #[cfg(target_env = "gnu")]
        allocator_probe::measure(phase);
        Ok(())
    }
    measure("baseline")?;
    let prefix = "// città 漢字 😀\nfn main() { println!(\"é\"); }\n";
    let source = format!(
        "{prefix}\n{}",
        "// padding ascii per corpus\n".repeat(320_000)
    );
    measure("source_string")?;
    let mut rope = Rope::from_str(&source);
    measure("source_plus_rope")?;
    drop(source);
    measure("rope_only")?;
    let mut syntax = SyntaxState::new(LanguageId::Rust, rope.slice(..)).ok_or("parse")?;
    assert_eq!(syntax.tree().root_node().end_byte(), rope.len_bytes());
    measure("initial_tree")?;
    #[cfg(target_env = "gnu")]
    {
        allocator_probe::trim();
        measure("initial_tree_after_trim")?;
    }
    rope.insert(rope.byte_to_char(prefix.len()), "a");
    syntax.apply_edit(
        &InputEditRaw {
            start_byte: prefix.len(),
            old_end_byte: prefix.len(),
            new_end_byte: prefix.len() + 1,
            start_point: (2, 0),
            old_end_point: (2, 0),
            new_end_point: (2, 1),
        },
        rope.slice(..),
    );
    assert!(syntax.tree().root_node().has_error());
    measure("error_tree")?;
    #[cfg(target_env = "gnu")]
    {
        allocator_probe::trim();
        measure("error_tree_after_trim")?;
    }
    let snapshot = syntax
        .highlight_snapshot(rope.slice(..), 2, 0..2048)
        .ok_or("snapshot")?;
    assert!(!snapshot.tokens().is_empty());
    measure("viewport_snapshot")?;
    drop(syntax);
    measure("after_tree_drop")?;
    drop(snapshot);
    drop(rope);
    measure("after_all_drop")?;
    #[cfg(target_env = "gnu")]
    {
        allocator_probe::trim();
        measure("after_allocator_trim")?;
    }
    Ok(())
}

// ABI glibc verificata contro /usr/include/malloc.h; solo diagnostica opt-in.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(unsafe_code)]
mod allocator_probe {
    #[repr(C)]
    struct Mallinfo2 {
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
        fn mallinfo2() -> Mallinfo2;
        fn malloc_trim(pad: usize) -> std::ffi::c_int;
    }
    pub fn measure(phase: &str) {
        // SAFETY: glibc restituisce per valore dieci size_t, nessun puntatore.
        let info = unsafe { mallinfo2() };
        eprintln!(
            "SYNTAX_ALLOCATOR:phase={phase},arena_bytes={},in_use_bytes={},free_bytes={},mmap_bytes={}",
            info.arena, info.uordblks, info.fordblks, info.hblkhd
        );
    }
    pub fn trim() {
        // SAFETY: API glibc thread-safe, pad=0; non invalida allocazioni vive.
        let returned = unsafe { malloc_trim(0) };
        eprintln!("SYNTAX_ALLOCATOR:trim_returned={returned}");
    }
}
