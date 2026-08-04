//! WorkTable KV table + driver, shared by the `kv-worktable` binary and the
//! Criterion `kv` bench so both exercise identical operations. Feature-gated on
//! `worktable-adapter`.

use worktable::prelude::*;
use worktable::worktable;

use crate::kv::{text_checksum, text_value};

worktable!(
    name: KvBench,
    columns: {
        id: u64 primary_key,
        payload: String,
    },
    queries: {
        update: {
            Payload(payload) by id,
        }
    }
);

/// In-memory WorkTable KV driver over the workload's fixed operations.
pub struct WorktableKv {
    table: KvBenchWorkTable,
    payload_bytes: usize,
}

impl WorktableKv {
    pub fn new(payload_bytes: usize) -> Self {
        Self {
            table: KvBenchWorkTable::default(),
            payload_bytes,
        }
    }

    /// Load `rows` sequential keys. Returns self populated.
    pub fn load(payload_bytes: usize, rows: u64) -> Self {
        let kv = Self::new(payload_bytes);
        for key in 0..rows {
            kv.insert(key);
        }
        kv
    }

    pub fn insert(&self, key: u64) {
        self.table
            .insert(KvBenchRow {
                id: key,
                payload: text_value(key, self.payload_bytes),
            })
            .expect("insert");
    }

    /// Sum of per-row checksums over `keys` — matches the other adapters.
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
