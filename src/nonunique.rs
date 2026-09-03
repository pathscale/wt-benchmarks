//! Non-unique secondary index: the same workload on every backend that can run it.
//!
//! WorkTable's own `nonunique_arctic_vs_wti` bench compares WorkTablesIndex
//! against Arctic, but it gives them **different key types** - WTI indexes a
//! 32-character hex `String` while Arctic indexes the `u128` that string was
//! rendered from. Both therefore measure String allocation and 32-byte
//! comparison as much as they measure the index, and the two arms are not
//! comparable as backends.
//!
//! This module separates the two effects by running three arms over one shared
//! key derivation:
//!
//! - `wti_u128` vs `arctic_u128` - the backend comparison, equal footing.
//! - `wti_string` vs `wti_u128` - the cost of the key type, one backend.
//!
//! **Congee is absent because it cannot do this at all.** `worktable_codegen`
//! rejects it outright: *"non-unique indexes currently require
//! `worktables_index` or `arctic`"*. That is a capability gap, not an omission
//! here, and it is why this is a two-backend comparison where the KV benches
//! are three.

/// Spread the seed so keys are not sequential in the index.
///
/// Identical to the derivation in WorkTable's own bench, so numbers from the
/// two remain comparable.
pub fn hash_of(seed: u64) -> u128 {
    let spread = (seed as u128).wrapping_mul(0x9E37_79B9_7F4A_7C15_F39C_C060_5CED_C834);
    spread ^ (spread >> 64)
}

/// One index key, derived from a seed.
///
/// Exists so the three arms share a key derivation and differ only in how the
/// key is *represented* - which is the whole point of separating the backend
/// question from the key-type question.
pub trait NonUniqueKey: Clone {
    fn from_seed(seed: u64) -> Self;
}

impl NonUniqueKey for u128 {
    fn from_seed(seed: u64) -> u128 {
        hash_of(seed)
    }
}

impl NonUniqueKey for String {
    fn from_seed(seed: u64) -> String {
        format!("{:032x}", hash_of(seed))
    }
}

/// (fan-out, distinct keys). Totals stay in the same few-thousand-row band, so
/// the shapes differ in index structure rather than in table size. Matches the
/// shapes in WorkTable's bench.
pub const SHAPES: [(u64, u64); 3] = [(1, 4096), (10, 512), (1000, 8)];

/// Generates a table with one non-unique secondary index on `source`, plus a
/// driver over it, for one index backend and key type.
///
/// Each arm lives in its own module: the generated `AdjacencyRow` /
/// `AdjacencyWorkTable` idents are not table-name-prefixed and would collide.
macro_rules! nonunique_backend_table {
    ($module:ident, $driver:ident, $key:ident, $using:ident) => {
        pub mod $module {
            use worktable::prelude::*;
            use worktable::worktable;

            use crate::nonunique::NonUniqueKey;

            worktable!(
                name: Adjacency,
                // Arctic requires an explicit persist choice; WorkTablesIndex
                // accepts one, so both arms share a single declaration.
                persist: false,
                columns: {
                    id: u64 primary_key autoincrement,
                    source: $key,
                    payload: u64,
                },
                indexes: {
                    source_idx: source using $using,
                }
            );

            pub struct $driver {
                table: AdjacencyWorkTable,
                keys: u64,
            }

            impl $driver {
                /// Steady state: `fan_out` rows already present per key, so a
                /// measured insert lands on an existing key rather than
                /// creating one.
                pub fn populated(fan_out: u64, keys: u64) -> Self {
                    let table = AdjacencyWorkTable::default();
                    for seed in 0..keys {
                        for copy in 0..fan_out {
                            futures::executor::block_on(table
                                .insert(AdjacencyRow {
                                    id: table.get_next_pk().into(),
                                    source: <$key as NonUniqueKey>::from_seed(seed),
                                    payload: copy,
                                }))
                                .expect("insert");
                        }
                    }
                    Self { table, keys }
                }

                pub fn next_row(&self, seed: u64) -> AdjacencyRow {
                    AdjacencyRow {
                        id: self.table.get_next_pk().into(),
                        source: <$key as NonUniqueKey>::from_seed(seed % self.keys),
                        payload: u64::MAX,
                    }
                }

                pub fn insert_row(&self, row: AdjacencyRow) {
                    futures::executor::block_on(self.table.insert(row)).expect("insert");
                }

                /// Every row under one key. The fan-out is what the index has
                /// to walk, so this is the operation the shapes vary.
                pub fn select_by_key(&self, seed: u64) -> usize {
                    self.table
                        .select_by_source(<$key as NonUniqueKey>::from_seed(seed % self.keys))
                        .execute()
                        .expect("select")
                        .len()
                }
            }
        }
    };
}

nonunique_backend_table!(wti_u128, WtiU128Adjacency, u128, worktables_index);
nonunique_backend_table!(arctic_u128, ArcticU128Adjacency, u128, arctic);
// The key-type arm: same backend as `wti_u128`, String key. Arctic cannot take
// this - codegen rejects String keys for a non-unique arctic index - so the
// key-type question is only askable on WorkTablesIndex.
nonunique_backend_table!(wti_string, WtiStringAdjacency, String, worktables_index);
