//! Non-unique index insert against key fan-out, the shape that hides a linear
//! scan inside an insert.
//!
//! **Consumer profile: AgentCode.** See `docs/BENCHMARK_CATALOG.md`.
//!
//! Every other index bench here spreads its keys, so each key holds one or a
//! few values and an insert that scans the values already under the key costs
//! the same as one that seeks. Skew is what separates them: hold the row count
//! fixed, raise how many rows share a key, and a sound index stays flat while a
//! scanning one grows with the fan-out.
//!
//! This is not hypothetical. `WorkTablesIndex` 0.0.8 ordered multimap entries by
//! `(key, random discriminator)`, which makes an entry unfindable by value, so
//! insert had to scan. At 16,000 values under one key that was 356.31 us per
//! insert against 416 ns on 0.0.7, a factor of 856, while the distinct-key arm
//! stayed flat at 0.44 us throughout. It shipped, and it reached a consumer as a
//! 21x regression on a workload where one generation id was shared by every row
//! written in that generation. 0.0.9 identifies entries by `(key, value)` and is
//! binary searchable.
//!
//! Feature-gated on `worktable-adapter`.

use crate::kv_table::IndexBackend;

/// Rows inserted per iteration, held constant across the sweep so the arms
/// differ only in how those rows are distributed over keys.
pub const ROWS: u64 = 20_000;

/// Values sharing one key. `ROWS / FAN_OUT` distinct keys are used, so every
/// arm inserts `ROWS` rows and only the skew changes.
pub const FAN_OUT: [u64; 4] = [1, 16, 256, 4_096];

/// Generates a `worktable!` with one non-unique index for a given backend, plus
/// a driver that inserts `ROWS` rows at a chosen fan-out. Each backend lives in
/// its own module so the generated idents do not collide.
macro_rules! fanout_backend_table {
    ($module:ident, $backend:ident) => {
        pub mod $module {
            use worktable::prelude::*;
            use worktable::worktable;

            worktable!(
                name: Fanout,
                persist: false,
                columns: {
                    id: u64 primary_key autoincrement,
                    group_key: u64,
                    payload: u64
                },
                indexes: {
                    group_idx: group_key using $backend
                }
            );

            pub struct Driver {
                table: FanoutWorkTable,
            }

            impl Driver {
                pub fn new() -> Self {
                    Self {
                        table: FanoutWorkTable::default(),
                    }
                }

                /// Inserts `rows` rows spread over `rows / fan_out` keys, so
                /// `fan_out` rows share each key. Returns the row count so the
                /// caller cannot have the work optimised away.
                pub fn insert_at_fan_out(&self, rows: u64, fan_out: u64) -> u64 {
                    let groups = (rows / fan_out).max(1);
                    for i in 0..rows {
                        self.table
                            .insert(FanoutRow {
                                id: self.table.get_next_pk().into(),
                                group_key: i % groups,
                                payload: i,
                            })
                            .expect("insert");
                    }
                    rows
                }

            }

            impl Default for Driver {
                fn default() -> Self {
                    Self::new()
                }
            }
        }
    };
}

fanout_backend_table!(wti, worktables_index);
fanout_backend_table!(arctic, arctic);

/// The backends that have a non-unique index. Congee is absent because it does
/// not provide one, so including it would compare a congee non-unique index
/// against WorkTablesIndex standing in for it.
pub const BACKENDS: [IndexBackend; 2] = [IndexBackend::WorktablesIndex, IndexBackend::Arctic];

/// Inserts `ROWS` rows at `fan_out` into a fresh table on `backend`.
pub fn insert_at_fan_out(backend: IndexBackend, fan_out: u64) -> u64 {
    match backend {
        IndexBackend::WorktablesIndex => wti::Driver::new().insert_at_fan_out(ROWS, fan_out),
        IndexBackend::Arctic => arctic::Driver::new().insert_at_fan_out(ROWS, fan_out),
        IndexBackend::Congee => unreachable!("congee has no non-unique index backend"),
    }
}

// There is deliberately no read sweep here. A lookup at fan-out F returns F
// rows, so the cost of materialising the result set scales with the axis being
// swept, and holding the row total constant only trades that for a lookup count
// that falls just as fast. A first draft measured 8.18 ms at fan-out 1 against
// 733 us at 256 on an index with no defect at all, purely from amortising 20,000
// lookups down to 78. A guard whose own slope is that large hides the slope it
// exists to catch. The defect this file guards is on the insert path.
