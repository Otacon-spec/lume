//! Isolated opt-in tree-sitter request histogram; not production allocation policy.
#![cfg(all(target_os = "linux", target_env = "gnu"))]
#![allow(unsafe_code)]
use std::{
    ffi::c_void,
    sync::atomic::{AtomicU64, Ordering},
};
static BUCKETS: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];
static REQUESTED: AtomicU64 = AtomicU64::new(0);
static MALLOC: AtomicU64 = AtomicU64::new(0);
static CALLOC: AtomicU64 = AtomicU64::new(0);
static REALLOC: AtomicU64 = AtomicU64::new(0);
static LIVE_232: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<usize>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));
static PEAK_232: AtomicU64 = AtomicU64::new(0);
static FREE: AtomicU64 = AtomicU64::new(0);
type Sample = (usize, u64, Vec<usize>);
static SAMPLES: std::sync::Mutex<Vec<Sample>> = std::sync::Mutex::new(Vec::new());
unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn calloc(n: usize, size: usize) -> *mut c_void;
    fn realloc(pointer: *mut c_void, size: usize) -> *mut c_void;
    fn free(pointer: *mut c_void);
    fn backtrace(buffer: *mut *mut c_void, size: std::ffi::c_int) -> std::ffi::c_int;
}
fn count(size: usize) {
    let bucket = (usize::BITS - size.max(1).leading_zeros() - 1) as usize;
    if let Some(counter) = BUCKETS.get(bucket) {
        let index = counter.fetch_add(1, Ordering::Relaxed);
        if (bucket == 6 || bucket == 7) && index % 65536 == 0 {
            let mut frames = [std::ptr::null_mut(); 16];
            // SAFETY: writable array of sixteen pointers, passed with its exact capacity.
            let count = unsafe { backtrace(frames.as_mut_ptr(), 16) };
            if let Ok(mut samples) = SAMPLES.lock() {
                samples.push((
                    size,
                    index,
                    frames
                        .iter()
                        .take(count.max(0) as usize)
                        .map(|p| *p as usize)
                        .collect(),
                ));
            }
        }
    }
    REQUESTED.fetch_add(size as u64, Ordering::Relaxed);
}
unsafe extern "C" fn tracked_malloc(size: usize) -> *mut c_void {
    MALLOC.fetch_add(1, Ordering::Relaxed);
    count(size);
    // SAFETY: forwards the unchanged C allocation request to glibc.
    {
        // SAFETY: forwards the unchanged C allocation request to glibc.
        let pointer = unsafe { malloc(size) };
        if size == 232
            && !pointer.is_null()
            && let Ok(mut live) = LIVE_232.lock()
        {
            live.insert(pointer as usize);
            PEAK_232.fetch_max(live.len() as u64, Ordering::Relaxed);
        }
        pointer
    }
}
unsafe extern "C" fn tracked_calloc(n: usize, size: usize) -> *mut c_void {
    CALLOC.fetch_add(1, Ordering::Relaxed);
    count(n.saturating_mul(size));
    // SAFETY: glibc checks multiplication overflow; original arguments are retained.
    unsafe { calloc(n, size) }
}
unsafe extern "C" fn tracked_realloc(pointer: *mut c_void, size: usize) -> *mut c_void {
    REALLOC.fetch_add(1, Ordering::Relaxed);
    count(size);
    // SAFETY: tree-sitter supplies a pointer from these same forwarded allocators.
    unsafe { realloc(pointer, size) }
}
unsafe extern "C" fn tracked_free(pointer: *mut c_void) {
    FREE.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut live) = LIVE_232.lock() {
        live.remove(&(pointer as usize));
    }
    // SAFETY: glibc accepts null or a live pointer from these same allocators.
    unsafe { free(pointer) }
}
#[test]
#[ignore = "global allocator hook: run only this test in an isolated process"]
fn rust_parse_allocation_request_histogram() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("LD_PRELOAD").is_some() {
        return Err("requires native glibc".into());
    }
    let prefix = "// città 漢字 😀\nfn main() { println!(\"é\"); }\n";
    let padding = "// padding ascii per corpus\n";
    let remaining = 32_505_856 - prefix.len() - 1;
    let raw = format!(
        "{prefix}\n{}{}",
        padding.repeat(remaining / padding.len()),
        " ".repeat(remaining % padding.len())
    );
    // SAFETY: this opt-in integration binary has one test; no parser exists yet.
    // Hooks forward to the same glibc allocator and remain installed through all drops.
    unsafe {
        tree_sitter::set_allocator(
            Some(tracked_malloc),
            Some(tracked_calloc),
            Some(tracked_realloc),
            Some(tracked_free),
        );
    }
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into())?;
    let tree = parser.parse(&raw, None).ok_or("parse failed")?;
    assert_eq!(tree.root_node().end_byte(), raw.len());
    println!(
        "ALLOC_REQUESTS,malloc={},calloc={},realloc={},free={},requested_bytes={}",
        MALLOC.load(Ordering::Relaxed),
        CALLOC.load(Ordering::Relaxed),
        REALLOC.load(Ordering::Relaxed),
        FREE.load(Ordering::Relaxed),
        REQUESTED.load(Ordering::Relaxed)
    );
    for (bucket, counter) in BUCKETS.iter().enumerate() {
        let count = counter.load(Ordering::Relaxed);
        if count != 0 {
            println!("ALLOC_BUCKET,log2_floor={bucket},requests={count}");
        }
    }
    println!(
        "ALLOC_LIVE_232,peak={},post_parse={}",
        PEAK_232.load(Ordering::Relaxed),
        LIVE_232.lock().map(|live| live.len()).unwrap_or(0)
    );
    drop(parser);
    drop(tree);
    if let Ok(samples) = SAMPLES.lock() {
        for (size, index, frames) in samples.iter() {
            println!("ALLOC_STACK,size={size},index={index},frames={frames:x?}");
        }
    }
    println!(
        "ALLOC_MAPS_BEGIN\n{}\nALLOC_MAPS_END",
        std::fs::read_to_string("/proc/self/maps")?
    );
    println!("ALLOC_FREE_AFTER_DROP,{}", FREE.load(Ordering::Relaxed));
    // SAFETY: all tree-sitter owners are gone, no other test or parser can run here.
    unsafe {
        tree_sitter::set_allocator(None, None, None, None);
    }
    Ok(())
}
