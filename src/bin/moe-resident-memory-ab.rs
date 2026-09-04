use std::alloc::{GlobalAlloc, Layout, System};
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use worktable_vec::{ArcticTable, IndexedTable, LinearTable};
use wt_benchmarks::moe_resident::{
    MoeResidentOriginCongeeRow, MoeResidentOriginCongeeWorkTable, MoeResidentOriginRow,
    MoeResidentOriginWorkTable, MoeResidentOriginWtiRow, MoeResidentOriginWtiWorkTable,
    logical_row, query_arctic_vec, query_btree, query_keys, query_linear, query_worktable,
    query_worktable_congee, query_worktable_wti,
};

const ROWS: usize = 1_528;
const CHECK_QUERIES: usize = 10_000;

struct CountingAllocator;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED: AtomicU64 = AtomicU64::new(0);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarding the exact layout to the system allocator.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_alloc(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarding the exact layout to the system allocator.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_alloc(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: forwarding the pointer with its original layout.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarding the pointer and its original layout.
        let replacement = unsafe { System.realloc(pointer, old, new_size) };
        if !replacement.is_null() {
            if new_size >= old.size() {
                record_alloc(new_size - old.size());
            } else {
                LIVE.fetch_sub(old.size() - new_size, Ordering::Relaxed);
            }
        }
        replacement
    }
}

fn record_alloc(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    ALLOCATED.fetch_add(bytes as u64, Ordering::Relaxed);
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    let mut peak = PEAK.load(Ordering::Relaxed);
    while live > peak {
        match PEAK.compare_exchange_weak(peak, live, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => peak = observed,
        }
    }
}

fn measure<T>(name: &str, build: impl FnOnce() -> T, check: impl FnOnce(&T) -> u64) {
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    ALLOCATED.store(0, Ordering::Relaxed);
    ALLOCATIONS.store(0, Ordering::Relaxed);

    let table = build();
    let retained = LIVE.load(Ordering::Relaxed).saturating_sub(baseline);
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    let allocated = ALLOCATED.load(Ordering::Relaxed);
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let checksum = check(&table);
    std::hint::black_box(&table);
    drop(table);
    let post_drop = LIVE.load(Ordering::Relaxed).saturating_sub(baseline);

    println!(
        "{name}\tretained={retained}\tpeak={peak}\tallocated={allocated}\tallocations={allocations}\tpost_drop={post_drop}\tchecksum={checksum}"
    );
}

fn linear(keys: &[u64]) {
    measure(
        "vec-linear",
        || {
            let mut table = LinearTable::with_capacity(ROWS);
            for index in 0..ROWS {
                let (key, value) = logical_row(index, ROWS);
                table.insert(key, value).unwrap();
            }
            table
        },
        |table| query_linear(table, keys).checksum,
    );
}

fn btree(keys: &[u64]) {
    measure(
        "vec-btree",
        || {
            let mut table = IndexedTable::with_capacity(ROWS);
            for index in 0..ROWS {
                let (key, value) = logical_row(index, ROWS);
                table.insert(key, value).unwrap();
            }
            table
        },
        |table| query_btree(table, keys).checksum,
    );
}

fn arctic_vec(keys: &[u64]) {
    measure(
        "vec-arctic",
        || {
            let mut table = ArcticTable::with_capacity(ROWS);
            for index in 0..ROWS {
                let (key, value) = logical_row(index, ROWS);
                table.insert(key, value).unwrap();
            }
            table
        },
        |table| query_arctic_vec(table, keys).checksum,
    );
}

fn worktable_arctic(keys: &[u64]) {
    measure(
        "worktable-arctic",
        || {
            let table = MoeResidentOriginWorkTable::default();
            for index in 0..ROWS {
                let (key, value) = logical_row(index, ROWS);
                futures::executor::block_on(table.insert(MoeResidentOriginRow {
                    origin_key: key,
                    source: value.source,
                    ordinal: value.ordinal,
                    example: value.example,
                    path_count: value.path_count,
                    origin_count: value.origin_count,
                }))
                .unwrap();
            }
            table
        },
        |table| query_worktable(table, keys).checksum,
    );
}

fn worktable_wti(keys: &[u64]) {
    measure(
        "worktable-wti",
        || {
            let table = MoeResidentOriginWtiWorkTable::default();
            for index in 0..ROWS {
                let (key, value) = logical_row(index, ROWS);
                futures::executor::block_on(table.insert(MoeResidentOriginWtiRow {
                    origin_key: key,
                    source: value.source,
                    ordinal: value.ordinal,
                    example: value.example,
                    path_count: value.path_count,
                    origin_count: value.origin_count,
                }))
                .unwrap();
            }
            table
        },
        |table| query_worktable_wti(table, keys).checksum,
    );
}

fn worktable_congee(keys: &[u64]) {
    measure(
        "worktable-congee",
        || {
            let table = MoeResidentOriginCongeeWorkTable::default();
            for index in 0..ROWS {
                let (key, value) = logical_row(index, ROWS);
                futures::executor::block_on(table.insert(MoeResidentOriginCongeeRow {
                    origin_key: key,
                    source: value.source,
                    ordinal: value.ordinal,
                    example: value.example,
                    path_count: value.path_count,
                    origin_count: value.origin_count,
                }))
                .unwrap();
            }
            table
        },
        |table| query_worktable_congee(table, keys).checksum,
    );
}

fn child(arm: &str) {
    let keys = query_keys(ROWS, CHECK_QUERIES);
    match arm {
        "vec-linear" => linear(&keys),
        "vec-btree" => btree(&keys),
        "vec-arctic" => arctic_vec(&keys),
        "worktable-arctic" => worktable_arctic(&keys),
        "worktable-wti" => worktable_wti(&keys),
        "worktable-congee" => worktable_congee(&keys),
        _ => panic!("unknown memory arm {arm}"),
    }
}

fn main() {
    let mut arguments = std::env::args();
    let executable = arguments.next().unwrap();
    if let Some(arm) = arguments.next() {
        child(&arm);
        return;
    }

    println!("rows={ROWS}; heap bytes from the process-global counting allocator");
    for arm in [
        "vec-linear",
        "vec-btree",
        "vec-arctic",
        "worktable-arctic",
        "worktable-wti",
        "worktable-congee",
    ] {
        let status = Command::new(&executable).arg(arm).status().unwrap();
        assert!(status.success(), "memory arm {arm} failed");
    }
}
