//! WorkTable KV table + driver, shared by the `kv-worktable` binary and the
//! Criterion `kv` bench so both exercise identical operations. Feature-gated on
//! `worktable-adapter`.
//!
//! The workload is generated once per primary-index backend via
//! `kv_backend_table!`, so the same KV operations can be benchmarked across
//! WorkTable's `using` backends — WorkTablesIndex (the default), Congee, and
//! Arctic — without duplicating the driver.

/// Selects which primary-index backend a KV driver uses. Chosen at runtime by
/// the binary (`--index-backend`) or per-benchmark by the Criterion harness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexBackend {
    /// WorkTablesIndex — the default `worktable!` primary index.
    WorktablesIndex,
    Congee,
    Arctic,
}

impl IndexBackend {
    pub const ALL: [IndexBackend; 3] = [
        IndexBackend::WorktablesIndex,
        IndexBackend::Congee,
        IndexBackend::Arctic,
    ];

    pub fn label(self) -> &'static str {
        match self {
            IndexBackend::WorktablesIndex => "worktables_index",
            IndexBackend::Congee => "congee",
            IndexBackend::Arctic => "arctic",
        }
    }

    pub fn benchmark_label(self) -> &'static str {
        match self {
            IndexBackend::WorktablesIndex => "worktable",
            IndexBackend::Congee => "worktable-congee",
            IndexBackend::Arctic => "worktable-arctic",
        }
    }

    /// Parse the `--index-backend` value / a bench parameter.
    pub fn parse(value: &str) -> Option<IndexBackend> {
        match value {
            "worktables_index" | "wti" | "default" => Some(IndexBackend::WorktablesIndex),
            "congee" => Some(IndexBackend::Congee),
            "arctic" => Some(IndexBackend::Arctic),
            _ => None,
        }
    }
}

/// Generates a `worktable!` KV table for one primary-index backend plus a driver
/// exposing the fixed KV operations. Each variant lives in its own module so the
/// generated `KvBenchRow` / `PayloadQuery` / `KvBenchWorkTable` idents do not
/// collide across backends; the driver type is re-exported at module scope.
/// Every backend has the identical method surface, so callers dispatch on
/// `IndexBackend` uniformly.
macro_rules! kv_backend_table {
    ($module:ident, $driver:ident, $using:ident) => {
        mod $module {
            use worktable::prelude::*;
            use worktable::worktable;

            use crate::kv::{text_checksum, text_value};

            worktable!(
                name: KvBench,
                // In-memory KV bench; congee/arctic require an explicit
                // persist choice, and WorkTablesIndex accepts it too, so all
                // three backends share one declaration.
                persist: false,
                columns: {
                    id: u64 primary_key using $using,
                    payload: String,
                },
                queries: {
                    update: {
                        Payload(payload) by id,
                    }
                }
            );

            /// In-memory WorkTable KV driver for one primary-index backend.
            pub struct $driver {
                table: KvBenchWorkTable,
                payload_bytes: usize,
            }

            impl $driver {
                pub fn new(payload_bytes: usize) -> Self {
                    Self {
                        table: KvBenchWorkTable::default(),
                        payload_bytes,
                    }
                }

                pub fn load(payload_bytes: usize, rows: u64) -> Self {
                    let kv = Self::new(payload_bytes);
                    for key in 0..rows {
                        kv.insert(key);
                    }
                    kv
                }

                pub fn insert(&self, key: u64) {
                    futures::executor::block_on(self.table
                        .insert(KvBenchRow {
                            id: key,
                            payload: text_value(key, self.payload_bytes),
                        }))
                        .expect("insert");
                }

                pub fn point_read_checksum(&self, keys: &[u64]) -> u64 {
                    keys.iter().fold(0u64, |sum, key| {
                        let row = self.table.select(*key).expect("loaded key");
                        sum.wrapping_add(text_checksum(row.id, &row.payload))
                    })
                }

                pub async fn overwrite(&self, keys: &[u64]) {
                    for key in keys {
                        self.table
                            .update_payload(
                                PayloadQuery {
                                    payload: text_value(key.wrapping_mul(17), self.payload_bytes),
                                },
                                *key,
                            )
                            .await
                            .expect("update");
                    }
                }

                pub fn range_scan_checksum(&self, starts: &[u64], scan_length: u64) -> u64 {
                    let mut checksum = 0u64;
                    for start in starts {
                        let end = start + scan_length;
                        let rows = self
                            .table
                            .select_by_pk_range(*start..end)
                            .execute()
                            .expect("range scan");
                        for row in rows {
                            checksum = checksum.wrapping_add(text_checksum(row.id, &row.payload));
                        }
                    }
                    checksum
                }

                pub fn count(&self) -> u64 {
                    self.table.count() as u64
                }

                pub async fn delete(&self, keys: &[u64]) -> u64 {
                    let mut deleted = 0u64;
                    for key in keys {
                        if self.table.delete(*key).await.is_ok() {
                            deleted += 1;
                        }
                    }
                    deleted
                }
            }
        }

        pub use $module::$driver;
    };
}

kv_backend_table!(wti_backend, WorktableKv, worktables_index);
kv_backend_table!(congee_backend, CongeeKv, congee);
kv_backend_table!(arctic_backend, ArcticKv, arctic);
