//! Where the time in a single-row delete goes.
//!
//! A delete ghosts the row in place and leaves the storage to be reclaimed
//! later, so the mutation itself is one store into a page. The call has always
//! cost far more than that, and aggregate delete throughput cannot say why:
//! it reports the total without saying which part of the total moved.
//!
//! This rebuilds `delete` out of its public parts and times each part added on
//! top of the last, so a change shows up as a change to one rung rather than a
//! change to a number. The ladder is what identified reclamation as roughly
//! 1250 ns of a 2074 ns delete, against a ghost flip that measures 1 ns.
//!
//! Each rung runs on its own freshly built table, and the table is moved into
//! the timed closure and returned so its drop lands outside the clock. Dropping
//! a populated table inside the measurement was worth a 25x error the first
//! time this was written by hand.
//!
//! Feature-gated on `worktable-adapter`.

use std::time::Instant;

use serde::Serialize;
use worktable::prelude::*;
use worktable::worktable;

worktable!(
    name: DelCost,
    persist: false,
    columns: { id: u64 primary_key, payload: u64, bucket: u32 },
    indexes: { payload_idx: payload unique, bucket_idx: bucket },
);

#[derive(Serialize)]
pub struct RungResult {
    pub schema_version: u32,
    pub suite: &'static str,
    pub engine: &'static str,
    /// The rung's name. Cumulative rungs are each the one before plus one step.
    pub rung: &'static str,
    /// True when this rung includes every step below it.
    pub cumulative: bool,
    pub rows: u64,
    pub elapsed_ns: u128,
    pub nanos_per_row: f64,
    pub target_arch: &'static str,
    pub target_os: &'static str,
}

pub struct Config {
    pub rows: u64,
}

impl Config {
    /// `--rows N`, defaulting to 20k. Large enough that a run is not dominated
    /// by fixture construction, small enough that the whole ladder is seconds.
    pub fn from_args() -> Result<Self, String> {
        let mut rows = 20_000u64;
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--rows" => {
                    let value = args.get(i + 1).ok_or("--rows needs a value")?;
                    rows = value
                        .parse::<u64>()
                        .map_err(|_| format!("--rows: {value} is not a number"))?;
                    if rows == 0 {
                        return Err("--rows must be at least 1".to_string());
                    }
                    i += 2;
                }
                "--help" => return Err("usage: delete-cost-worktable [--rows N]".to_string()),
                other => return Err(format!("unrecognised argument {other}")),
            }
        }
        Ok(Self { rows })
    }
}

pub fn table(rows: u64) -> DelCostWorkTable {
    let table = DelCostWorkTable::default();
    let batch: Vec<_> = (0..rows)
        .map(|id| DelCostRow {
            id,
            payload: 1_000_000 + id,
            bucket: (id % 16) as u32,
        })
        .collect();
    futures::executor::block_on(table.insert_many(batch)).expect("fixture inserts");
    table
}

pub fn keys(rows: u64) -> Vec<DelCostPrimaryKey> {
    (0..rows).map(Into::into).collect()
}

/// Deletion order. Sequential coalesces on every free and keeps the empty-link
/// registry at about one entry; shuffled coalesces on almost none and lets it
/// grow to one entry per row, which is why the two differ by more than noise.
pub fn shuffled_keys(rows: u64) -> Vec<DelCostPrimaryKey> {
    let mut keys = keys(rows);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    for i in (1..keys.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        keys.swap(i, (state % (i as u64 + 1)) as usize);
    }
    keys
}

pub fn emit(rung: &'static str, cumulative: bool, rows: u64, elapsed_ns: u128) {
    let result = RungResult {
        schema_version: 1,
        suite: "delete-cost",
        engine: "worktable",
        rung,
        cumulative,
        rows,
        elapsed_ns,
        nanos_per_row: elapsed_ns as f64 / rows as f64,
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
    };
    println!(
        "{}",
        serde_json::to_string(&result).expect("result must serialize")
    );
}

/// Times `op` over the whole fixture, keeping the fixture's drop off the clock.
pub fn rung<T>(name: &'static str, cumulative: bool, rows: u64, fixture: T, op: impl FnOnce(&T)) {
    let started = Instant::now();
    op(&fixture);
    let elapsed = started.elapsed();
    drop(fixture);
    emit(name, cumulative, rows, elapsed.as_nanos());
}

/// Runs the whole ladder and emits one JSON object per rung.
///
/// Lives here rather than in the binary because the `worktable!` macro expands
/// its table into *this* crate, and the inner `WorkTable` the rungs reach
/// through is private outside it. Splitting the ladder from the type it
/// measures would mean widening that visibility for a benchmark.
pub fn run(config: &Config) {
    let rows = config.rows;

    // Warm the allocator and every code path once. Without this the first rung
    // carries the cost of faulting in pages the rest reuse.
    {
        let t = table(rows);
        for pk in keys(rows) {
            futures::executor::block_on(t.delete(pk)).expect("warm");
        }
    }

    // ---- the ladder: each rung is the one above plus one step ----

    // The page write path with a no-op mutation: page lookup, page write lock,
    // and the re-read and republication `with_mut_ref` always does. No ghost
    // bit is set, so this is everything the flip costs except the flip.
    rung(
        "page_write_path",
        true,
        rows,
        (table(rows), keys(rows)),
        |(t, ks)| {
            for pk in ks {
                let link: Link =
                    t.0.primary_index
                        .pk_map
                        .get_value(pk)
                        .map(Into::into)
                        .expect("link");
                unsafe { t.0.data.with_mut_ref(link, |_| ()).expect("with_mut_ref") };
            }
        },
    );

    // The same, now flipping the ghost bit. The difference is the flip itself.
    rung(
        "ghost_flip",
        true,
        rows,
        (table(rows), keys(rows)),
        |(t, ks)| {
            for pk in ks {
                let link: Link =
                    t.0.primary_index
                        .pk_map
                        .get_value(pk)
                        .map(Into::into)
                        .expect("link");
                unsafe { t.0.data.with_mut_ref(link, |r| r.delete()).expect("ghost") };
            }
        },
    );

    // Plus the row counter, the retirement, and whatever reclamation the
    // delete path still absorbs.
    rung(
        "data_delete",
        true,
        rows,
        (table(rows), keys(rows)),
        |(t, ks)| {
            for pk in ks {
                let link: Link =
                    t.0.primary_index
                        .pk_map
                        .get_value(pk)
                        .map(Into::into)
                        .expect("link");
                t.0.data.delete(link).expect("delete");
            }
        },
    );

    // Plus primary index removal, which is two structures: the pk map and the
    // reverse link-to-pk map.
    rung(
        "primary_index_removal",
        true,
        rows,
        (table(rows), keys(rows)),
        |(t, ks)| {
            for pk in ks {
                let link: Link =
                    t.0.primary_index
                        .pk_map
                        .get_value(pk)
                        .map(Into::into)
                        .expect("link");
                t.0.primary_index.remove(pk, link);
                t.0.data.delete(link).expect("delete");
            }
        },
    );

    // Plus secondary index removal. Rows are fetched before the clock starts,
    // so this rung is the removals and not the read that feeds them.
    rung(
        "secondary_index_removal",
        true,
        rows,
        {
            let t = table(rows);
            let ks = keys(rows);
            let fetched: Vec<DelCostRow> = ks
                .iter()
                .map(|pk| t.0.select(pk.clone()).expect("row"))
                .collect();
            (t, ks, fetched)
        },
        |(t, ks, fetched)| {
            for (pk, row) in ks.iter().zip(fetched.iter()) {
                let link: Link =
                    t.0.primary_index
                        .pk_map
                        .get_value(pk)
                        .map(Into::into)
                        .expect("link");
                t.0.indexes
                    .delete_row(row.clone(), link)
                    .expect("secondary");
                t.0.primary_index.remove(pk, link);
                t.0.data.delete(link).expect("delete");
            }
        },
    );

    // Plus the `select` the generated delete runs to obtain that row: an epoch
    // pin, a second primary lookup, and a full rkyv deserialize.
    rung(
        "select_the_row",
        true,
        rows,
        (table(rows), keys(rows)),
        |(t, ks)| {
            for pk in ks {
                let link: Link =
                    t.0.primary_index
                        .pk_map
                        .get_value(pk)
                        .map(Into::into)
                        .expect("link");
                let row = t.0.select(pk.clone()).expect("row");
                t.0.indexes.delete_row(row, link).expect("secondary");
                t.0.primary_index.remove(pk, link);
                t.0.data.delete(link).expect("delete");
            }
        },
    );

    // ---- whole calls, for comparison ----

    rung(
        "delete_without_lock",
        false,
        rows,
        (table(rows), keys(rows)),
        |(t, ks)| {
            for pk in ks {
                futures::executor::block_on(t.delete_without_lock(pk.clone())).expect("delete");
            }
        },
    );

    rung(
        "delete",
        false,
        rows,
        (table(rows), keys(rows)),
        |(t, ks)| {
            for pk in ks {
                futures::executor::block_on(t.delete(pk.clone())).expect("delete");
            }
        },
    );

    // Shuffled: nothing coalesces, so the empty-link registry grows to one
    // entry per row instead of staying at about one.
    rung(
        "delete_shuffled",
        false,
        rows,
        (table(rows), shuffled_keys(rows)),
        |(t, ks)| {
            for pk in ks {
                futures::executor::block_on(t.delete(pk.clone())).expect("delete");
            }
        },
    );

    rung(
        "delete_many",
        false,
        rows,
        (table(rows), keys(rows)),
        |(t, ks)| {
            std::hint::black_box(
                futures::executor::block_on(t.delete_many(ks.clone())).expect("delete_many"),
            );
        },
    );

    // Insert, so the delete figures have a same-run denominator rather than
    // being compared against a number from another session.
    rung("insert", false, rows, table(0), |t| {
        for id in 0..rows {
            futures::executor::block_on(t.insert(DelCostRow {
                id,
                payload: 1_000_000 + id,
                bucket: (id % 16) as u32,
            }))
            .expect("insert");
        }
    });

    // Delete then refill. Reclamation is deferred to whoever needs the space,
    // so this is the shape that makes the consumer pay for it, and the pair is
    // what a churn workload actually costs. Reported per pair.
    rung(
        "delete_then_insert_pair",
        false,
        rows,
        (table(rows), keys(rows)),
        |(t, ks)| {
            for pk in ks {
                futures::executor::block_on(t.delete(pk.clone())).expect("delete");
            }
            for id in 0..rows {
                futures::executor::block_on(t.insert(DelCostRow {
                    id: rows + id,
                    payload: 2_000_000 + id,
                    bucket: (id % 16) as u32,
                }))
                .expect("insert");
            }
        },
    );
}
