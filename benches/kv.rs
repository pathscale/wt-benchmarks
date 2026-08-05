//! KV throughput bench (Criterion, Throughput::Elements) — the adopted-twin KV
//! workload across engines, so Criterion reports elements/sec with confidence
//! intervals directly. This is the THROUGHPUT port; a separate latency port
//! records p50/p95/p99 for the concurrent workloads. Single-threaded point KV
//! only needs the throughput view.
//!
//! Engines beyond WorkTable are behind their adapter features:
//!   cargo bench --bench kv --features external-adapters

use std::time::Duration;

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, Throughput, criterion_group, criterion_main};

// The WorkTable engines checksum/serialize inside their drivers; the external
// KV engines (sqlite/redb/lmdb/duckdb) still call these directly, so the import
// is gated to those adapters to avoid an unused-import warning otherwise.
#[cfg(any(
    feature = "sqlite-adapter",
    feature = "redb-adapter",
    feature = "lmdb-adapter",
    feature = "duckdb-adapter"
))]
use wt_benchmarks::kv::{text_checksum, text_value};

const ROWS: u64 = 10_000;
const OPS: u64 = 10_000;
const SCAN_OPS: u64 = 200;
const SCAN_LEN: u64 = 100;
const PAYLOAD: usize = 64;

fn seeded_keys(n: u64, bound: u64, seed: u64) -> Vec<u64> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s % bound
        })
        .collect()
}

fn point_keys() -> Vec<u64> {
    seeded_keys(OPS, ROWS, 42)
}
fn scan_starts() -> Vec<u64> {
    seeded_keys(SCAN_OPS, ROWS - SCAN_LEN + 1, 0xa5a5)
}

fn grp<'a>(c: &'a mut Criterion, name: &str, elems: u64) -> BenchmarkGroup<'a, WallTime> {
    let mut g = c.benchmark_group(name);
    g.throughput(Throughput::Elements(elems));
    g.measurement_time(Duration::from_secs(5));
    g
}

// ------------------------------------------------------------- WorkTable
// One engine module per WorkTable primary-index backend. The bench-function
// label distinguishes them within each group (worktable = WorkTablesIndex, the
// default; worktable-congee; worktable-arctic), so `kv/overwrite` etc. compare
// all three side by side.
#[cfg(feature = "worktable-adapter")]
macro_rules! worktable_backend_engine {
    ($module:ident, $driver:ident, $label:literal) => {
        mod $module {
            use super::*;
            use wt_benchmarks::kv_table::$driver;

            pub fn insert(g: &mut BenchmarkGroup<'_, WallTime>) {
                g.bench_function($label, |b| {
                    b.iter_batched(
                        || $driver::new(PAYLOAD),
                        |kv| {
                            for k in 0..ROWS {
                                kv.insert(k);
                            }
                            kv
                        },
                        criterion::BatchSize::SmallInput,
                    )
                });
            }
            pub fn point_read(g: &mut BenchmarkGroup<'_, WallTime>, keys: &[u64]) {
                let kv = $driver::load(PAYLOAD, ROWS);
                g.bench_function($label, |b| b.iter(|| kv.point_read_checksum(keys)));
            }
            pub fn overwrite(g: &mut BenchmarkGroup<'_, WallTime>, keys: &[u64]) {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let kv = $driver::load(PAYLOAD, ROWS);
                g.bench_function($label, |b| b.iter(|| rt.block_on(kv.overwrite(keys))));
            }
            pub fn range_scan(g: &mut BenchmarkGroup<'_, WallTime>, starts: &[u64]) {
                let kv = $driver::load(PAYLOAD, ROWS);
                g.bench_function($label, |b| {
                    b.iter(|| kv.range_scan_checksum(starts, SCAN_LEN))
                });
            }
        }
    };
}

#[cfg(feature = "worktable-adapter")]
worktable_backend_engine!(worktable_engine, WorktableKv, "worktable");
#[cfg(feature = "worktable-adapter")]
worktable_backend_engine!(worktable_congee_engine, CongeeKv, "worktable-congee");
#[cfg(feature = "worktable-adapter")]
worktable_backend_engine!(worktable_arctic_engine, ArcticKv, "worktable-arctic");

// --------------------------------------------------------------- SQLite
#[cfg(feature = "sqlite-adapter")]
mod sqlite_engine {
    use super::*;
    use rusqlite::{Connection, params};

    fn load() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF;
             CREATE TABLE kv (id INTEGER PRIMARY KEY, payload TEXT NOT NULL);",
        )
        .unwrap();
        {
            let mut s = c.prepare("INSERT INTO kv VALUES (?,?)").unwrap();
            for k in 0..ROWS {
                s.execute(params![k as i64, text_value(k, PAYLOAD)])
                    .unwrap();
            }
        }
        c
    }
    pub fn insert(g: &mut BenchmarkGroup<'_, WallTime>) {
        g.bench_function("sqlite", |b| {
            b.iter_batched(
                || {
                    let c = Connection::open_in_memory().unwrap();
                    c.execute_batch(
                        "PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF;
                         CREATE TABLE kv (id INTEGER PRIMARY KEY, payload TEXT NOT NULL);",
                    )
                    .unwrap();
                    c
                },
                |c| {
                    let mut s = c.prepare("INSERT INTO kv VALUES (?,?)").unwrap();
                    for k in 0..ROWS {
                        s.execute(params![k as i64, text_value(k, PAYLOAD)])
                            .unwrap();
                    }
                    drop(s);
                    c
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }
    pub fn point_read(g: &mut BenchmarkGroup<'_, WallTime>, keys: &[u64]) {
        let c = load();
        g.bench_function("sqlite", |b| {
            b.iter(|| {
                let mut s = c.prepare("SELECT payload FROM kv WHERE id=?").unwrap();
                let mut sum = 0u64;
                for k in keys {
                    let p: String = s.query_row(params![*k as i64], |r| r.get(0)).unwrap();
                    sum = sum.wrapping_add(text_checksum(*k, &p));
                }
                sum
            })
        });
    }
    pub fn overwrite(g: &mut BenchmarkGroup<'_, WallTime>, keys: &[u64]) {
        let c = load();
        g.bench_function("sqlite", |b| {
            b.iter(|| {
                let mut s = c.prepare("UPDATE kv SET payload=? WHERE id=?").unwrap();
                for k in keys {
                    s.execute(params![text_value(k.wrapping_mul(17), PAYLOAD), *k as i64])
                        .unwrap();
                }
            })
        });
    }
    pub fn range_scan(g: &mut BenchmarkGroup<'_, WallTime>, starts: &[u64]) {
        let c = load();
        g.bench_function("sqlite", |b| {
            b.iter(|| {
                let mut s = c
                    .prepare("SELECT id,payload FROM kv WHERE id>=? ORDER BY id LIMIT ?")
                    .unwrap();
                let mut sum = 0u64;
                for start in starts {
                    let mut rows = s.query(params![*start as i64, SCAN_LEN as i64]).unwrap();
                    while let Some(row) = rows.next().unwrap() {
                        let id: i64 = row.get(0).unwrap();
                        let p: String = row.get(1).unwrap();
                        sum = sum.wrapping_add(text_checksum(id as u64, &p));
                    }
                }
                sum
            })
        });
    }
}

// ----------------------------------------------------------------- redb
#[cfg(feature = "redb-adapter")]
mod redb_engine {
    use super::*;
    use redb::{Database, ReadableDatabase, TableDefinition};

    const T: TableDefinition<u64, &[u8]> = TableDefinition::new("kv");

    fn fresh() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("b.redb")).unwrap();
        {
            let wt = db.begin_write().unwrap();
            wt.open_table(T).unwrap();
            wt.commit().unwrap();
        }
        (dir, db)
    }
    fn load() -> (tempfile::TempDir, Database) {
        let (dir, db) = fresh();
        let wt = db.begin_write().unwrap();
        {
            let mut t = wt.open_table(T).unwrap();
            for k in 0..ROWS {
                t.insert(k, text_value(k, PAYLOAD).as_bytes()).unwrap();
            }
        }
        wt.commit().unwrap();
        (dir, db)
    }
    pub fn insert(g: &mut BenchmarkGroup<'_, WallTime>) {
        // One database; delete the table's rows before each batch instead of
        // creating a new file per iteration.
        let (_dir, db) = fresh();
        g.bench_function("redb", |b| {
            b.iter_batched(
                || {
                    let wt = db.begin_write().unwrap();
                    {
                        let mut t = wt.open_table(T).unwrap();
                        t.retain(|_, _| false).unwrap();
                    }
                    wt.commit().unwrap();
                },
                |()| {
                    let wt = db.begin_write().unwrap();
                    {
                        let mut t = wt.open_table(T).unwrap();
                        for k in 0..ROWS {
                            t.insert(k, text_value(k, PAYLOAD).as_bytes()).unwrap();
                        }
                    }
                    wt.commit().unwrap();
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }
    pub fn point_read(g: &mut BenchmarkGroup<'_, WallTime>, keys: &[u64]) {
        let (_dir, db) = load();
        g.bench_function("redb", |b| {
            b.iter(|| {
                let rt = db.begin_read().unwrap();
                let t = rt.open_table(T).unwrap();
                let mut sum = 0u64;
                for k in keys {
                    let v = t.get(*k).unwrap().unwrap();
                    sum = sum
                        .wrapping_add(text_checksum(*k, std::str::from_utf8(v.value()).unwrap()));
                }
                sum
            })
        });
    }
    pub fn overwrite(g: &mut BenchmarkGroup<'_, WallTime>, keys: &[u64]) {
        let (_dir, db) = load();
        g.bench_function("redb", |b| {
            b.iter(|| {
                let wt = db.begin_write().unwrap();
                {
                    let mut t = wt.open_table(T).unwrap();
                    for k in keys {
                        t.insert(*k, text_value(k.wrapping_mul(17), PAYLOAD).as_bytes())
                            .unwrap();
                    }
                }
                wt.commit().unwrap();
            })
        });
    }
    pub fn range_scan(g: &mut BenchmarkGroup<'_, WallTime>, starts: &[u64]) {
        let (_dir, db) = load();
        g.bench_function("redb", |b| {
            b.iter(|| {
                let rt = db.begin_read().unwrap();
                let t = rt.open_table(T).unwrap();
                let mut sum = 0u64;
                for s in starts {
                    for row in t.range(*s..).unwrap().take(SCAN_LEN as usize) {
                        let (k, v) = row.unwrap();
                        sum = sum.wrapping_add(text_checksum(
                            k.value(),
                            std::str::from_utf8(v.value()).unwrap(),
                        ));
                    }
                }
                sum
            })
        });
    }
}

// ----------------------------------------------------------------- lmdb
#[cfg(feature = "lmdb-adapter")]
mod lmdb_engine {
    use super::*;
    use heed::types::{Bytes, U64};
    use heed::{Database, Env, EnvOpenOptions, byteorder::BigEndian};

    type K = U64<BigEndian>;
    // 256 MiB is ample for 10k small rows and avoids exhausting mmap/fd limits
    // when Criterion opens many environments across batch iterations.
    const MAP: usize = 256 * 1024 * 1024;

    fn open() -> (tempfile::TempDir, Env, Database<K, Bytes>) {
        let dir = tempfile::tempdir().unwrap();
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(MAP)
                .max_dbs(1)
                .open(dir.path())
                .unwrap()
        };
        let db = {
            let mut w = env.write_txn().unwrap();
            let db = env.create_database(&mut w, None).unwrap();
            w.commit().unwrap();
            db
        };
        (dir, env, db)
    }
    fn load() -> (tempfile::TempDir, Env, Database<K, Bytes>) {
        let (dir, env, db) = open();
        let mut w = env.write_txn().unwrap();
        for k in 0..ROWS {
            db.put(&mut w, &k, text_value(k, PAYLOAD).as_bytes())
                .unwrap();
        }
        w.commit().unwrap();
        (dir, env, db)
    }
    pub fn insert(g: &mut BenchmarkGroup<'_, WallTime>) {
        // Reuse ONE environment; clear the db before each measured insert batch
        // rather than opening a fresh mmap per iteration (which exhausts OS
        // resources under Criterion's repeated sampling).
        let (_dir, env, db) = open();
        g.bench_function("lmdb", |b| {
            b.iter_batched(
                || {
                    let mut w = env.write_txn().unwrap();
                    db.clear(&mut w).unwrap();
                    w.commit().unwrap();
                },
                |()| {
                    let mut w = env.write_txn().unwrap();
                    for k in 0..ROWS {
                        db.put(&mut w, &k, text_value(k, PAYLOAD).as_bytes())
                            .unwrap();
                    }
                    w.commit().unwrap();
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }
    pub fn point_read(g: &mut BenchmarkGroup<'_, WallTime>, keys: &[u64]) {
        let (_dir, env, db) = load();
        g.bench_function("lmdb", |b| {
            b.iter(|| {
                let r = env.read_txn().unwrap();
                let mut sum = 0u64;
                for k in keys {
                    let v = db.get(&r, k).unwrap().unwrap();
                    sum = sum.wrapping_add(text_checksum(*k, std::str::from_utf8(v).unwrap()));
                }
                sum
            })
        });
    }
    pub fn range_scan(g: &mut BenchmarkGroup<'_, WallTime>, starts: &[u64]) {
        let (_dir, env, db) = load();
        g.bench_function("lmdb", |b| {
            b.iter(|| {
                let r = env.read_txn().unwrap();
                let mut sum = 0u64;
                for s in starts {
                    for row in db.range(&r, &(*s..)).unwrap().take(SCAN_LEN as usize) {
                        let (k, v) = row.unwrap();
                        sum = sum.wrapping_add(text_checksum(k, std::str::from_utf8(v).unwrap()));
                    }
                }
                sum
            })
        });
    }
    pub fn overwrite(g: &mut BenchmarkGroup<'_, WallTime>, keys: &[u64]) {
        let (_dir, env, db) = load();
        g.bench_function("lmdb", |b| {
            b.iter(|| {
                let mut w = env.write_txn().unwrap();
                for k in keys {
                    db.put(
                        &mut w,
                        k,
                        text_value(k.wrapping_mul(17), PAYLOAD).as_bytes(),
                    )
                    .unwrap();
                }
                w.commit().unwrap();
            })
        });
    }
}

// ---------------------------------------------------------------- duckdb
#[cfg(feature = "duckdb-adapter")]
mod duckdb_engine {
    use super::*;
    use duckdb::{Connection, params};

    fn load() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE kv (id UBIGINT PRIMARY KEY, payload VARCHAR NOT NULL);")
            .unwrap();
        {
            let mut s = c.prepare("INSERT INTO kv VALUES (?,?)").unwrap();
            for k in 0..ROWS {
                s.execute(params![k, text_value(k, PAYLOAD)]).unwrap();
            }
        }
        c
    }
    pub fn insert(g: &mut BenchmarkGroup<'_, WallTime>) {
        g.bench_function("duckdb", |b| {
            b.iter_batched(
                || {
                    let c = Connection::open_in_memory().unwrap();
                    c.execute_batch(
                        "CREATE TABLE kv (id UBIGINT PRIMARY KEY, payload VARCHAR NOT NULL);",
                    )
                    .unwrap();
                    c
                },
                |c| {
                    let mut s = c.prepare("INSERT INTO kv VALUES (?,?)").unwrap();
                    for k in 0..ROWS {
                        s.execute(params![k, text_value(k, PAYLOAD)]).unwrap();
                    }
                    drop(s);
                    c
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }
    pub fn point_read(g: &mut BenchmarkGroup<'_, WallTime>, keys: &[u64]) {
        let c = load();
        g.bench_function("duckdb", |b| {
            b.iter(|| {
                let mut s = c.prepare("SELECT payload FROM kv WHERE id=?").unwrap();
                let mut sum = 0u64;
                for k in keys {
                    let p: String = s.query_row(params![*k], |r| r.get(0)).unwrap();
                    sum = sum.wrapping_add(text_checksum(*k, &p));
                }
                sum
            })
        });
    }
    pub fn overwrite(g: &mut BenchmarkGroup<'_, WallTime>, keys: &[u64]) {
        let c = load();
        g.bench_function("duckdb", |b| {
            b.iter(|| {
                let mut s = c.prepare("UPDATE kv SET payload=? WHERE id=?").unwrap();
                for k in keys {
                    s.execute(params![text_value(k.wrapping_mul(17), PAYLOAD), *k])
                        .unwrap();
                }
            })
        });
    }
    pub fn range_scan(g: &mut BenchmarkGroup<'_, WallTime>, starts: &[u64]) {
        let c = load();
        g.bench_function("duckdb", |b| {
            b.iter(|| {
                let mut s = c
                    .prepare("SELECT id,payload FROM kv WHERE id>=? ORDER BY id LIMIT ?")
                    .unwrap();
                let mut sum = 0u64;
                for start in starts {
                    let mut rows = s.query(params![*start, SCAN_LEN]).unwrap();
                    while let Some(row) = rows.next().unwrap() {
                        let id: u64 = row.get(0).unwrap();
                        let p: String = row.get(1).unwrap();
                        sum = sum.wrapping_add(text_checksum(id, &p));
                    }
                }
                sum
            })
        });
    }
}

fn bench_insert(c: &mut Criterion) {
    let mut g = grp(c, "kv/insert", ROWS);
    #[cfg(feature = "worktable-adapter")]
    worktable_engine::insert(&mut g);
    #[cfg(feature = "worktable-adapter")]
    worktable_congee_engine::insert(&mut g);
    #[cfg(feature = "worktable-adapter")]
    worktable_arctic_engine::insert(&mut g);
    #[cfg(feature = "sqlite-adapter")]
    sqlite_engine::insert(&mut g);
    #[cfg(feature = "redb-adapter")]
    redb_engine::insert(&mut g);
    #[cfg(feature = "lmdb-adapter")]
    lmdb_engine::insert(&mut g);
    #[cfg(feature = "duckdb-adapter")]
    duckdb_engine::insert(&mut g);
    g.finish();
}

fn bench_point_read(c: &mut Criterion) {
    let keys = point_keys();
    let mut g = grp(c, "kv/point_read", OPS);
    #[cfg(feature = "worktable-adapter")]
    worktable_engine::point_read(&mut g, &keys);
    #[cfg(feature = "worktable-adapter")]
    worktable_congee_engine::point_read(&mut g, &keys);
    #[cfg(feature = "worktable-adapter")]
    worktable_arctic_engine::point_read(&mut g, &keys);
    #[cfg(feature = "sqlite-adapter")]
    sqlite_engine::point_read(&mut g, &keys);
    #[cfg(feature = "redb-adapter")]
    redb_engine::point_read(&mut g, &keys);
    #[cfg(feature = "lmdb-adapter")]
    lmdb_engine::point_read(&mut g, &keys);
    #[cfg(feature = "duckdb-adapter")]
    duckdb_engine::point_read(&mut g, &keys);
    g.finish();
}

fn bench_overwrite(c: &mut Criterion) {
    let keys = point_keys();
    let mut g = grp(c, "kv/overwrite", OPS);
    #[cfg(feature = "worktable-adapter")]
    worktable_engine::overwrite(&mut g, &keys);
    #[cfg(feature = "worktable-adapter")]
    worktable_congee_engine::overwrite(&mut g, &keys);
    #[cfg(feature = "worktable-adapter")]
    worktable_arctic_engine::overwrite(&mut g, &keys);
    #[cfg(feature = "sqlite-adapter")]
    sqlite_engine::overwrite(&mut g, &keys);
    #[cfg(feature = "redb-adapter")]
    redb_engine::overwrite(&mut g, &keys);
    #[cfg(feature = "lmdb-adapter")]
    lmdb_engine::overwrite(&mut g, &keys);
    #[cfg(feature = "duckdb-adapter")]
    duckdb_engine::overwrite(&mut g, &keys);
    g.finish();
}

fn bench_range_scan(c: &mut Criterion) {
    let starts = scan_starts();
    let mut g = grp(c, "kv/range_scan", SCAN_OPS * SCAN_LEN);
    #[cfg(feature = "worktable-adapter")]
    worktable_engine::range_scan(&mut g, &starts);
    #[cfg(feature = "worktable-adapter")]
    worktable_congee_engine::range_scan(&mut g, &starts);
    #[cfg(feature = "worktable-adapter")]
    worktable_arctic_engine::range_scan(&mut g, &starts);
    #[cfg(feature = "redb-adapter")]
    redb_engine::range_scan(&mut g, &starts);
    #[cfg(feature = "lmdb-adapter")]
    lmdb_engine::range_scan(&mut g, &starts);
    #[cfg(feature = "sqlite-adapter")]
    sqlite_engine::range_scan(&mut g, &starts);
    #[cfg(feature = "duckdb-adapter")]
    duckdb_engine::range_scan(&mut g, &starts);
    g.finish();
}

criterion_group!(
    benches,
    bench_insert,
    bench_point_read,
    bench_overwrite,
    bench_range_scan
);
criterion_main!(benches);
