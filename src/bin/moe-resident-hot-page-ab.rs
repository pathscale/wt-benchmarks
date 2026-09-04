//! One-page read/write contention probe for WorkTable's row read path.
//!
//! Readers select rows that the writer never changes. All rows occupy one
//! physical page, so a difference isolates false page-level contention rather
//! than logical row contention or an index-backend comparison.

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use worktable_vec::ArcticTable;
use wt_benchmarks::moe_resident::{MoeResidentOriginRow, MoeResidentOriginWorkTable, logical_row};

const ROWS: usize = 256;
const READERS: usize = 4;
const WRITES: usize = 160_000;
const SAMPLES: usize = 9;
const READ_ONLY_OPS: usize = 2_000_000;

#[derive(Clone, Copy)]
struct Sample {
    writer_elapsed: Duration,
    reader_ops: usize,
    checksum: u64,
}

fn populated() -> Arc<MoeResidentOriginWorkTable> {
    let table = MoeResidentOriginWorkTable::default();
    for index in 0..ROWS {
        let (origin_key, value) = logical_row(index, ROWS);
        futures::executor::block_on(table.insert(MoeResidentOriginRow {
            origin_key,
            source: value.source,
            ordinal: value.ordinal,
            example: value.example,
            path_count: value.path_count,
            origin_count: value.origin_count,
        }))
        .expect("unique fixture row");
    }
    assert_eq!(
        table.system_info().page_count,
        1,
        "fixture must use one page"
    );
    Arc::new(table)
}

fn sample() -> Sample {
    let table = populated();
    let reader_ready = Arc::new(AtomicUsize::new(0));
    let start = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let launch = Arc::new(Barrier::new(READERS + 1));

    let (writer_elapsed, reader_ops, checksum) = std::thread::scope(|scope| {
        let mut reader_handles = Vec::with_capacity(READERS);
        for reader in 0..READERS {
            let table = Arc::clone(&table);
            let reader_ready = Arc::clone(&reader_ready);
            let start = Arc::clone(&start);
            let done = Arc::clone(&done);
            let launch = Arc::clone(&launch);
            reader_handles.push(scope.spawn(move || {
                launch.wait();
                reader_ready.fetch_add(1, Ordering::Release);
                while !start.load(Ordering::Acquire) {
                    std::hint::spin_loop();
                }

                let mut operations = 0usize;
                let mut checksum = 0u64;
                let mut index = reader + 1;
                while !done.load(Ordering::Acquire) {
                    let (key, _) = logical_row(index, ROWS);
                    let row = table.select(key).expect("reader row must remain present");
                    checksum = checksum.wrapping_add(
                        u64::from(row.source)
                            ^ u64::from(row.ordinal).rotate_left(17)
                            ^ u64::from(row.example).rotate_left(31),
                    );
                    operations += 1;
                    index += READERS;
                    if index >= ROWS {
                        index = reader + 1;
                    }
                }
                (operations, checksum)
            }));
        }

        let writer_table = Arc::clone(&table);
        let writer_ready = Arc::clone(&reader_ready);
        let writer_start = Arc::clone(&start);
        let writer_done = Arc::clone(&done);
        let writer_launch = Arc::clone(&launch);
        let writer = scope.spawn(move || {
            writer_launch.wait();
            while writer_ready.load(Ordering::Acquire) != READERS {
                std::hint::spin_loop();
            }
            writer_start.store(true, Ordering::Release);

            let (origin_key, value) = logical_row(0, ROWS);
            let started = Instant::now();
            for revision in 0..WRITES {
                futures::executor::block_on(writer_table.upsert(MoeResidentOriginRow {
                    origin_key,
                    source: value.source,
                    ordinal: value.ordinal,
                    example: revision as u32,
                    path_count: value.path_count,
                    origin_count: value.origin_count,
                }))
                .expect("writer upsert");
            }
            let elapsed = started.elapsed();
            writer_done.store(true, Ordering::Release);
            elapsed
        });

        let writer_elapsed = writer.join().expect("writer thread");
        let mut reader_ops = 0usize;
        let mut checksum = 0u64;
        for handle in reader_handles {
            let (operations, reader_checksum) = handle.join().expect("reader thread");
            reader_ops += operations;
            checksum = checksum.wrapping_add(reader_checksum);
        }
        (writer_elapsed, reader_ops, checksum)
    });

    black_box(checksum);
    Sample {
        writer_elapsed,
        reader_ops,
        checksum,
    }
}

fn read_only_sample(readers: usize) -> f64 {
    let table = populated();
    let launch = Arc::new(Barrier::new(readers + 1));
    let started = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(readers);
        for reader in 0..readers {
            let table = Arc::clone(&table);
            let launch = Arc::clone(&launch);
            handles.push(scope.spawn(move || {
                launch.wait();
                let mut checksum = 0u64;
                let mut index = reader + 1;
                for _ in 0..READ_ONLY_OPS {
                    let (key, _) = logical_row(index, ROWS);
                    let row = table.select(key).expect("reader row must remain present");
                    checksum = checksum.wrapping_add(u64::from(row.example));
                    index += readers;
                    if index >= ROWS {
                        index = reader + 1;
                    }
                }
                checksum
            }));
        }
        launch.wait();
        let started = Instant::now();
        let checksum = handles.into_iter().fold(0u64, |sum, handle| {
            sum.wrapping_add(handle.join().expect("reader thread"))
        });
        black_box(checksum);
        started.elapsed()
    });
    readers as f64 * READ_ONLY_OPS as f64 / started.as_secs_f64() / 1_000_000.0
}

fn arctic_read_only_sample(readers: usize) -> f64 {
    let mut table = ArcticTable::with_capacity(ROWS);
    for index in 0..ROWS {
        let (key, value) = logical_row(index, ROWS);
        table.insert(key, value).expect("unique fixture row");
    }
    let table = Arc::new(table);
    let launch = Arc::new(Barrier::new(readers + 1));
    let elapsed = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(readers);
        for reader in 0..readers {
            let table = Arc::clone(&table);
            let launch = Arc::clone(&launch);
            handles.push(scope.spawn(move || {
                launch.wait();
                let mut checksum = 0u64;
                let mut index = reader + 1;
                for _ in 0..READ_ONLY_OPS {
                    let (key, _) = logical_row(index, ROWS);
                    let row = table.select(&key).expect("reader row must remain present");
                    checksum = checksum.wrapping_add(u64::from(row.example));
                    index += readers;
                    if index >= ROWS {
                        index = reader + 1;
                    }
                }
                checksum
            }));
        }
        launch.wait();
        let started = Instant::now();
        let checksum = handles.into_iter().fold(0u64, |sum, handle| {
            sum.wrapping_add(handle.join().expect("reader thread"))
        });
        black_box(checksum);
        started.elapsed()
    });
    readers as f64 * READ_ONLY_OPS as f64 / elapsed.as_secs_f64() / 1_000_000.0
}

fn main() {
    let mode = "exact-cell";
    println!(
        "mode={mode}\trows={ROWS}\tpages=1\treaders={READERS}\twrites={WRITES}\tsamples={SAMPLES}"
    );

    for readers in [1, 2, 4, 8] {
        black_box(arctic_read_only_sample(readers));
        let mut arctic_values = (0..5)
            .map(|_| arctic_read_only_sample(readers))
            .collect::<Vec<_>>();
        arctic_values.sort_by(f64::total_cmp);
        println!(
            "read_only_arctic\treaders={readers}\tmedian_mops={:.3}\traw_mops={arctic_values:?}",
            arctic_values[arctic_values.len() / 2]
        );

        black_box(read_only_sample(readers));
        let mut values = (0..5)
            .map(|_| read_only_sample(readers))
            .collect::<Vec<_>>();
        values.sort_by(f64::total_cmp);
        println!(
            "read_only\treaders={readers}\tmedian_mops={:.3}\traw_mops={values:?}",
            values[values.len() / 2]
        );
    }

    if std::env::args().nth(1).as_deref() == Some("read-only") {
        return;
    }

    // One discarded run resolves one-time allocator, thread, and SMR setup.
    black_box(sample());

    let mut samples = Vec::with_capacity(SAMPLES);
    for index in 0..SAMPLES {
        let result = sample();
        let seconds = result.writer_elapsed.as_secs_f64();
        let writer_ns = result.writer_elapsed.as_nanos() as f64 / WRITES as f64;
        let reader_mops = result.reader_ops as f64 / seconds / 1_000_000.0;
        println!(
            "sample={}\twriter_ns_per_upsert={writer_ns:.2}\treader_mops={reader_mops:.3}\treader_ops={}\tchecksum={}",
            index + 1,
            result.reader_ops,
            result.checksum,
        );
        samples.push((writer_ns, reader_mops));
    }

    samples.sort_by(|left, right| left.0.total_cmp(&right.0));
    let writer_median = samples[SAMPLES / 2].0;
    samples.sort_by(|left, right| left.1.total_cmp(&right.1));
    let reader_median = samples[SAMPLES / 2].1;
    println!("median\twriter_ns_per_upsert={writer_median:.2}\treader_mops={reader_median:.3}");
}
