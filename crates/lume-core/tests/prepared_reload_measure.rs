//! Diagnostica opt-in: costo del reload identico, senza GUI o I/O file.
#![cfg(target_os = "linux")]

use lume_core::{DocId, Document, PreparedText};
use std::time::Instant;

fn memory() -> std::io::Result<String> {
    Ok(std::fs::read_to_string("/proc/self/status")?
        .lines()
        .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
        .collect::<Vec<_>>()
        .join(";"))
}

#[test]
#[ignore = "diagnostica release isolata, nessun budget GUI dedotto"]
fn identical_reload_preparation_cost() -> Result<(), Box<dyn std::error::Error>> {
    let raw = "// padding\n".repeat(31 * 1024 * 1024 / 11);
    let mut document = Document::from_str(&raw, None, DocId(1));
    println!("PREPARE_MEMORY,baseline,{},{}", raw.len(), memory()?);
    let start = Instant::now();
    let text = PreparedText::prepare(&raw);
    println!(
        "PREPARE_MEMORY,rope,{}us,{}",
        start.elapsed().as_micros(),
        memory()?
    );
    let start = Instant::now();
    let reload = document.read_snapshot().prepare_reload(text);
    println!(
        "PREPARE_MEMORY,inverse,{}us,{}",
        start.elapsed().as_micros(),
        memory()?
    );
    assert!(document.reload_prepared(reload).is_some());
    assert_eq!(document.rope().len_bytes(), raw.len());
    assert!(!document.is_dirty());
    assert!(document.undo().is_some());
    assert_eq!(document.rope().len_bytes(), raw.len());
    Ok(())
}
