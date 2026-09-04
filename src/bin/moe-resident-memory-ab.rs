use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use arctic::concurrent::smr::PsReclaim;
use arctic::{ConcurrentMap, Key};
use parking_lot::RwLock;
use worktable::IndexMap;
use worktable::data_bucket::Link;
use worktable::prelude::OffsetEqLink;
use worktable_vec::{ArcticTable, IndexedTable, LinearTable};
use wt_benchmarks::moe_resident::{
    MoeResidentOriginCongeeRow, MoeResidentOriginCongeeWorkTable, MoeResidentOriginRow,
    MoeResidentOriginWorkTable, MoeResidentOriginWtiRow, MoeResidentOriginWtiWorkTable,
    logical_row, query_arctic_vec, query_btree, query_keys, query_linear, query_worktable,
    query_worktable_congee, query_worktable_wti,
};

const ROW_COUNTS: [usize; 4] = [0, 1, 64, 1_528];
const CHECK_QUERIES: usize = 10_000;
const PUBLICATION_SHARDS: usize = 64;

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

fn measure<T>(
    name: &str,
    rows: usize,
    paired: bool,
    mut build: impl FnMut() -> T,
    check: impl Fn(&T) -> u64,
) {
    // Keep the first table alive while measuring the second. This removes
    // process-global/TLS first-use cost without letting its drop trigger
    // deferred cleanup during the measured build.
    let first = paired.then(|| build());
    if let Some(table) = &first {
        std::hint::black_box(check(table));
    }

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
        "{name}\trows={rows}\tcycle={}\tretained={retained}\tpeak={peak}\tallocated={allocated}\tallocations={allocations}\tpost_drop={post_drop}\tchecksum={checksum}",
        if paired { "paired" } else { "cold" }
    );
    drop(first);
}

fn linear(rows: usize, paired: bool, keys: &[u64]) {
    measure(
        "vec-linear",
        rows,
        paired,
        || {
            let mut table = LinearTable::with_capacity(rows);
            for index in 0..rows {
                let (key, value) = logical_row(index, rows);
                table.insert(key, value).unwrap();
            }
            table
        },
        |table| query_linear(table, keys).checksum,
    );
}

fn btree(rows: usize, paired: bool, keys: &[u64]) {
    measure(
        "vec-btree",
        rows,
        paired,
        || {
            let mut table = IndexedTable::with_capacity(rows);
            for index in 0..rows {
                let (key, value) = logical_row(index, rows);
                table.insert(key, value).unwrap();
            }
            table
        },
        |table| query_btree(table, keys).checksum,
    );
}

fn arctic_vec(rows: usize, paired: bool, keys: &[u64]) {
    measure(
        "vec-arctic",
        rows,
        paired,
        || {
            let mut table = ArcticTable::with_capacity(rows);
            for index in 0..rows {
                let (key, value) = logical_row(index, rows);
                table.insert(key, value).unwrap();
            }
            table
        },
        |table| query_arctic_vec(table, keys).checksum,
    );
}

fn worktable_arctic(rows: usize, paired: bool, keys: &[u64]) {
    measure(
        "worktable-arctic",
        rows,
        paired,
        || {
            let table = MoeResidentOriginWorkTable::default();
            for index in 0..rows {
                let (key, value) = logical_row(index, rows);
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
        |table| {
            let info = table.system_info();
            eprintln!(
                "worktable-pages={}\tworktable-row-bytes={}\tworktable-secondary-index-bytes={}",
                info.page_count, info.memory_usage_bytes, info.idx_size
            );
            query_worktable(table, keys).checksum
        },
    );
}

fn worktable_wti(rows: usize, paired: bool, keys: &[u64]) {
    measure(
        "worktable-wti",
        rows,
        paired,
        || {
            let table = MoeResidentOriginWtiWorkTable::default();
            for index in 0..rows {
                let (key, value) = logical_row(index, rows);
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

fn worktable_congee(rows: usize, paired: bool, keys: &[u64]) {
    measure(
        "worktable-congee",
        rows,
        paired,
        || {
            let table = MoeResidentOriginCongeeWorkTable::default();
            for index in 0..rows {
                let (key, value) = logical_row(index, rows);
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

fn boxed_link_index(rows: usize, paired: bool, keys: &[u64]) {
    measure(
        "component-boxed-link-index",
        rows,
        paired,
        || {
            let map = ConcurrentMap::<u64, Box<Link>, PsReclaim>::new();
            for index in 0..rows {
                let (key, _) = logical_row(index, rows);
                map.insert(
                    key.as_insert(),
                    Box::new(Link {
                        page_id: (index as u32 / 512 + 1).into(),
                        offset: (index as u32 % 512) * 32,
                        length: 32,
                    }),
                )
                .unwrap();
            }
            map
        },
        |map| {
            keys.iter().fold(0, |checksum, key| {
                checksum ^ u64::from(map.get(key).unwrap().offset)
            })
        },
    );
}

fn reverse_index(rows: usize, paired: bool, keys: &[u64]) {
    measure(
        "component-reverse-index",
        rows,
        paired,
        || {
            let map = IndexMap::<OffsetEqLink<16_384>, u64>::default();
            for index in 0..rows {
                let (key, _) = logical_row(index, rows);
                let link = Link {
                    page_id: (index as u32 / 512 + 1).into(),
                    offset: (index as u32 % 512) * 32,
                    length: 32,
                };
                map.insert(OffsetEqLink(link), key);
            }
            map
        },
        |map| {
            keys.iter().fold(0, |checksum, key| {
                let index = *key as usize % rows.max(1);
                let link = Link {
                    page_id: (index as u32 / 512 + 1).into(),
                    offset: (index as u32 % 512) * 32,
                    length: 32,
                };
                map.get(&OffsetEqLink(link))
                    .map_or(checksum, |entry| checksum ^ entry.get().value)
            })
        },
    );
}

struct PublicationRow {
    origin_key: u64,
    value: wt_benchmarks::moe_resident::OriginValue,
}

struct PublicationVersion {
    row: Arc<PublicationRow>,
    flags: u8,
}

struct Publication {
    version: RwLock<PublicationVersion>,
}

fn publication_map(rows: usize, paired: bool, keys: &[u64]) {
    measure(
        "component-publication-map",
        rows,
        paired,
        || {
            let mut shards: [RwLock<HashMap<Link, Arc<Publication>>>; PUBLICATION_SHARDS] =
                std::array::from_fn(|_| RwLock::new(HashMap::new()));
            for index in 0..rows {
                let (origin_key, value) = logical_row(index, rows);
                let link = Link {
                    page_id: (index as u32 / 512 + 1).into(),
                    offset: (index as u32 % 512) * 32,
                    length: 32,
                };
                shards[index & (PUBLICATION_SHARDS - 1)].get_mut().insert(
                    link,
                    Arc::new(Publication {
                        version: RwLock::new(PublicationVersion {
                            row: Arc::new(PublicationRow { origin_key, value }),
                            flags: 0,
                        }),
                    }),
                );
            }
            shards
        },
        |shards| {
            keys.iter().fold(0, |checksum, key| {
                let index = *key as usize % rows.max(1);
                let link = Link {
                    page_id: (index as u32 / 512 + 1).into(),
                    offset: (index as u32 % 512) * 32,
                    length: 32,
                };
                let publication = shards[index & (PUBLICATION_SHARDS - 1)]
                    .read()
                    .get(&link)
                    .cloned();
                publication.map_or(checksum, |publication| {
                    let version = publication.version.read();
                    checksum
                        ^ version.row.origin_key
                        ^ u64::from(version.row.value.ordinal)
                        ^ u64::from(version.flags)
                })
            })
        },
    );
}

fn child(arm: &str, rows: usize, paired: bool) {
    let keys = if rows == 0 {
        Vec::new()
    } else {
        query_keys(rows, CHECK_QUERIES)
    };
    match arm {
        "vec-linear" => linear(rows, paired, &keys),
        "vec-btree" => btree(rows, paired, &keys),
        "vec-arctic" => arctic_vec(rows, paired, &keys),
        "worktable-arctic" => worktable_arctic(rows, paired, &keys),
        "worktable-wti" => worktable_wti(rows, paired, &keys),
        "worktable-congee" => worktable_congee(rows, paired, &keys),
        "component-boxed-link-index" => boxed_link_index(rows, paired, &keys),
        "component-reverse-index" => reverse_index(rows, paired, &keys),
        "component-publication-map" => publication_map(rows, paired, &keys),
        _ => panic!("unknown memory arm {arm}"),
    }
}

fn main() {
    let mut arguments = std::env::args();
    let executable = arguments.next().unwrap();
    if let Some(arm) = arguments.next() {
        let rows = arguments.next().unwrap().parse().unwrap();
        let paired = match arguments.next().as_deref() {
            Some("cold") => false,
            Some("paired") => true,
            cycle => panic!("unknown measurement cycle {cycle:?}"),
        };
        child(&arm, rows, paired);
        return;
    }

    println!("heap bytes from the process-global counting allocator");
    for rows in ROW_COUNTS {
        for arm in ["vec-arctic", "worktable-arctic"] {
            for cycle in ["cold", "paired"] {
                let status = Command::new(&executable)
                    .args([arm, &rows.to_string(), cycle])
                    .status()
                    .unwrap();
                assert!(status.success(), "memory arm {arm}/{rows}/{cycle} failed");
            }
        }
    }
}
