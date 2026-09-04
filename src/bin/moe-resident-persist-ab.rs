use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use worktable::prelude::*;
use wt_benchmarks::moe_resident::{
    MoeResidentOriginPersistedPersistenceEngine, MoeResidentOriginPersistedRow,
    MoeResidentOriginPersistedWorkTable, build, logical_row, query_keys, query_worktable,
    query_worktable_persisted,
};

const ROWS: usize = 1_528;
const QUERIES: usize = 1_000_000;
const SAMPLES: usize = 9;

struct CleanDir(PathBuf);

impl Drop for CleanDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn artifact_bytes(path: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![path.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = std::fs::metadata(&path).unwrap();
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                pending.push(entry.unwrap().path());
            }
        } else {
            total += metadata.len();
        }
    }
    total
}

fn median(mut values: Vec<Duration>) -> Duration {
    values.sort_unstable();
    values[values.len() / 2]
}

fn ns_per_query(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1e9 / QUERIES as f64
}

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let root =
            PathBuf::from("target/moe-resident-persist-ab").join(std::process::id().to_string());
        assert!(root.starts_with("target/moe-resident-persist-ab"));
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
        let _cleanup = CleanDir(root.clone());

        let config = DiskConfig::new_with_table_name(
            root.to_string_lossy().into_owned(),
            MoeResidentOriginPersistedWorkTable::name_snake_case(),
            MoeResidentOriginPersistedWorkTable::version(),
        );
        let engine = MoeResidentOriginPersistedPersistenceEngine::new(config)
            .await
            .unwrap();
        let started = Instant::now();
        let table = MoeResidentOriginPersistedWorkTable::load(engine)
            .await
            .unwrap();
        let empty_load = started.elapsed();

        let rows = (0..ROWS)
            .map(|index| {
                let (key, value) = logical_row(index, ROWS);
                MoeResidentOriginPersistedRow {
                    origin_key: key,
                    source: value.source,
                    ordinal: value.ordinal,
                    example: value.example,
                    path_count: value.path_count,
                    origin_count: value.origin_count,
                }
            })
            .collect::<Vec<_>>();
        let started = Instant::now();
        table.insert_many(rows).await.unwrap();
        let visible = started.elapsed();
        table.wait_for_ops().await.unwrap();
        let durable = started.elapsed();

        let keys = query_keys(ROWS, QUERIES);
        let live_before_close = query_worktable_persisted(&table, &keys[..10_000]);
        table.close().await.unwrap();
        let bytes = artifact_bytes(&root);

        let config = DiskConfig::new_with_table_name(
            root.to_string_lossy().into_owned(),
            MoeResidentOriginPersistedWorkTable::name_snake_case(),
            MoeResidentOriginPersistedWorkTable::version(),
        );
        let engine = MoeResidentOriginPersistedPersistenceEngine::new(config)
            .await
            .unwrap();
        let started = Instant::now();
        let reopened = MoeResidentOriginPersistedWorkTable::load(engine)
            .await
            .unwrap();
        let reopen = started.elapsed();
        assert_eq!(reopened.count(), ROWS);

        let first = query_worktable_persisted(&reopened, &keys[..1]);
        let check = query_worktable_persisted(&reopened, &keys[..10_000]);
        assert_eq!(check.checksum, live_before_close.checksum);

        let (memory, _) = build(ROWS);
        assert_eq!(
            query_worktable(&memory.worktable_arctic, &keys[..10_000]).checksum,
            check.checksum
        );

        let mut persisted_samples = Vec::with_capacity(SAMPLES);
        let mut memory_samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            persisted_samples.push(query_worktable_persisted(&reopened, &keys).elapsed);
            memory_samples.push(query_worktable(&memory.worktable_arctic, &keys).elapsed);
        }
        let persisted = median(persisted_samples);
        let memory = median(memory_samples);

        println!("published WorkTable 1.0.0-beta.17, Arctic primary index");
        println!("rows={ROWS} queries={QUERIES} samples={SAMPLES}");
        println!("artifact_bytes={bytes}");
        println!("empty_create_load_ms={:.3}", empty_load.as_secs_f64() * 1e3);
        println!("insert_visible_ms={:.3}", visible.as_secs_f64() * 1e3);
        println!("insert_durable_ms={:.3}", durable.as_secs_f64() * 1e3);
        println!("reopen_ms={:.3}", reopen.as_secs_f64() * 1e3);
        println!("first_reopened_query_ns={}", first.elapsed.as_nanos());
        println!("warm_memory_ns_per_query={:.2}", ns_per_query(memory));
        println!("warm_persisted_ns_per_query={:.2}", ns_per_query(persisted));
        println!(
            "persisted_over_memory={:.3}x",
            persisted.as_secs_f64() / memory.as_secs_f64()
        );
        println!("checksum={}", check.checksum);

        reopened.close().await.unwrap();
    });
}
