//! KV + embedded-JSON workload: the "durable jank" tier of the realistic
//! comparison. Modeled on the widespread real-world pattern (oxidecomputer/
//! omicron, pict-rs) of shoving a structured record into an embedded KV store
//! as a serialized JSON blob, keyed by id — so every field touch pays a
//! parse -> mutate -> reserialize -> put tax on the WHOLE document, versus
//! WorkTable's typed columns where a single-field update writes one column.
//!
//! Every engine stores the SAME record so the comparison is honest:
//!   WorkTable  -> typed columns
//!   KV engines -> the same fields as one `serde_json` object per key
//!
//! Ops: insert, point_get, update_one_field (the money op), query_by_field.

use serde::{Deserialize, Serialize};

/// The shared record. Flat, typed, realistic — an account/user-ish row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: u64,
    pub name: String,
    pub email: String,
    pub age: u32,
    pub balance: f64,
    pub active: bool,
}

impl Account {
    /// Deterministic record for key `k`.
    pub fn make(k: u64) -> Account {
        Account {
            id: k,
            name: format!("user-{k:08}"),
            email: format!("user{k}@example.test"),
            age: 18 + (k % 60) as u32,
            balance: (k as f64) * 1.5,
            active: k % 2 == 0,
        }
    }

    /// Folds the fields we read into a checksum, so point/query ops can't be
    /// optimized away and every engine is forced to materialize the same data.
    pub fn checksum(&self) -> u64 {
        let mut s = self.id;
        s = s.wrapping_add(self.name.len() as u64);
        s = s.wrapping_add(self.email.len() as u64);
        s = s.wrapping_add(self.age as u64);
        s = s.wrapping_add(self.balance as u64);
        s = s.wrapping_add(self.active as u64);
        s
    }
}

// ------------------------------------------------------------- WorkTable
// Typed columns. A single-field update touches ONE column via the generated
// query; no JSON parse/reserialize. Generated once per primary-index backend
// selectable via the `using` keyword (WorkTablesIndex / Congee / Arctic), each
// in its own module so the generated idents don't collide.
#[cfg(feature = "worktable-adapter")]
macro_rules! wt_doc_backend {
    ($module:ident, $driver:ident, $using:ident) => {
        pub mod $module {
            use crate::kv_json::Account;
            use worktable::prelude::*;
            use worktable::worktable;

            worktable!(
                name: AccountDoc,
                // congee/arctic require an explicit persist choice; WTI accepts
                // it too, so all three share one declaration.
                persist: false,
                columns: {
                    id: u64 primary_key using $using,
                    name: String,
                    email: String,
                    age: u32,
                    balance: f64,
                    active: bool,
                },
                queries: {
                    update: {
                        Balance(balance) by id,
                    }
                }
            );

            pub struct $driver {
                table: AccountDocWorkTable,
            }

            impl $driver {
                pub fn new() -> Self {
                    Self { table: AccountDocWorkTable::default() }
                }
                pub fn load(rows: u64) -> Self {
                    let e = Self::new();
                    for k in 0..rows {
                        e.insert(k);
                    }
                    e
                }
                pub fn insert(&self, k: u64) {
                    let a = Account::make(k);
                    self.table
                        .insert(AccountDocRow {
                            id: a.id,
                            name: a.name,
                            email: a.email,
                            age: a.age,
                            balance: a.balance,
                            active: a.active,
                        })
                        .expect("insert");
                }
                pub fn point_get_checksum(&self, keys: &[u64]) -> u64 {
                    keys.iter().fold(0u64, |acc, k| {
                        let r = self.table.select(*k).expect("row");
                        acc.wrapping_add(
                            Account {
                                id: r.id,
                                name: r.name,
                                email: r.email,
                                age: r.age,
                                balance: r.balance,
                                active: r.active,
                            }
                            .checksum(),
                        )
                    })
                }
                /// The money op: update ONE typed column in place.
                pub async fn update_balance(&self, keys: &[u64]) {
                    for k in keys {
                        self.table
                            .update_balance(BalanceQuery { balance: (*k as f64) * 2.25 }, *k)
                            .await
                            .expect("update");
                    }
                }
                /// Query by a non-key field: scan, materialize, filter.
                pub fn query_active_over_age_checksum(&self, min_age: u32) -> u64 {
                    self.table
                        .select_all()
                        .execute()
                        .expect("scan")
                        .into_iter()
                        .filter(|r| r.active && r.age >= min_age)
                        .fold(0u64, |acc, r| acc.wrapping_add(r.id))
                }
            }
        }
    };
}

#[cfg(feature = "worktable-adapter")]
wt_doc_backend!(worktable_engine, WtDoc, worktables_index);
#[cfg(feature = "worktable-adapter")]
wt_doc_backend!(worktable_congee_engine, WtDocCongee, congee);
#[cfg(feature = "worktable-adapter")]
wt_doc_backend!(worktable_arctic_engine, WtDocArctic, arctic);

// ---------------------------------------------------- KV + JSON (redb / lmdb)
// The durable-jank tier. Value = one serde_json blob per key. A single-field
// update must get -> from_slice -> mutate -> to_vec -> put the whole document.
//
// TRANSACTION GRANULARITY (load-bearing for a fair comparison):
// Every CRUD operation commits its OWN transaction — one begin_write/commit per
// insert, per update_field, per delete. This mirrors WorkTable, which publishes
// one mutation per `update`/`insert`/`delete` call, and mirrors how these stores
// are used in the wild (omicron / pict-rs commit per logical write, not
// thousands-at-once). Batching all N ops into a single transaction would let the
// KV amortize its commit/durability cost across the batch and misrepresent
// per-operation CRUD latency — do NOT wrap the op loops in one transaction.
// (`load()` is bulk setup, not a measured op, so it may batch.)

#[cfg(feature = "redb-adapter")]
pub mod redb_engine {
    use super::Account;
    use redb::{Database, Durability, ReadableDatabase, ReadableTable, TableDefinition};

    const T: TableDefinition<u64, &[u8]> = TableDefinition::new("accounts");

    pub struct RedbJson {
        _dir: tempfile::TempDir,
        db: Database,
    }

    impl RedbJson {
        pub fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = Database::create(dir.path().join("accounts.redb")).unwrap();
            {
                let mut w = db.begin_write().unwrap();
                w.set_durability(Durability::None);
                {
                    let _ = w.open_table(T).unwrap();
                }
                w.commit().unwrap();
            }
            Self { _dir: dir, db }
        }
        pub fn load(rows: u64) -> Self {
            let e = Self::new();
            let mut w = e.db.begin_write().unwrap();
            w.set_durability(Durability::None);
            {
                let mut t = w.open_table(T).unwrap();
                for k in 0..rows {
                    let bytes = serde_json::to_vec(&Account::make(k)).unwrap();
                    t.insert(k, bytes.as_slice()).unwrap();
                }
            }
            w.commit().unwrap();
            e
        }
        pub fn insert(&self, k: u64) {
            let bytes = serde_json::to_vec(&Account::make(k)).unwrap();
            let mut w = self.db.begin_write().unwrap();
            w.set_durability(Durability::None);
            {
                let mut t = w.open_table(T).unwrap();
                t.insert(k, bytes.as_slice()).unwrap();
            }
            w.commit().unwrap();
        }
        pub fn point_get_checksum(&self, keys: &[u64]) -> u64 {
            let r = self.db.begin_read().unwrap();
            let t = r.open_table(T).unwrap();
            keys.iter().fold(0u64, |acc, k| {
                let v = t.get(*k).unwrap().expect("row");
                let a: Account = serde_json::from_slice(v.value()).unwrap();
                acc.wrapping_add(a.checksum())
            })
        }
        /// The money op: for EACH key, in its own transaction, parse the whole
        /// doc, mutate one field, reserialize, and commit. One begin_write/commit
        /// per update — matches WorkTable's per-op publish (see module note).
        pub fn update_balance(&self, keys: &[u64]) {
            for k in keys {
                let mut w = self.db.begin_write().unwrap();
                w.set_durability(Durability::None);
                {
                    let mut t = w.open_table(T).unwrap();
                    let mut a: Account = {
                        let v = t.get(*k).unwrap().expect("row");
                        serde_json::from_slice(v.value()).unwrap()
                    };
                    a.balance = (*k as f64) * 2.25;
                    let bytes = serde_json::to_vec(&a).unwrap();
                    t.insert(*k, bytes.as_slice()).unwrap();
                }
                w.commit().unwrap();
            }
        }
        pub fn query_active_over_age_checksum(&self, min_age: u32) -> u64 {
            let r = self.db.begin_read().unwrap();
            let t = r.open_table(T).unwrap();
            let mut acc = 0u64;
            for row in t.range::<u64>(..).unwrap() {
                let (_, v) = row.unwrap();
                let a: Account = serde_json::from_slice(v.value()).unwrap();
                if a.active && a.age >= min_age {
                    acc = acc.wrapping_add(a.id);
                }
            }
            acc
        }
    }
}

#[cfg(feature = "lmdb-adapter")]
pub mod lmdb_engine {
    use super::Account;
    use heed::types::{Bytes, U64};
    use heed::{Database, Env, EnvOpenOptions};

    type AccountDb = Database<U64<heed::byteorder::NativeEndian>, Bytes>;

    pub struct LmdbJson {
        _dir: tempfile::TempDir,
        env: Env,
        db: AccountDb,
    }

    impl LmdbJson {
        pub fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let env = unsafe {
                let mut opts = EnvOpenOptions::new();
                opts.map_size(1024 * 1024 * 1024)
                    .flags(heed::EnvFlags::NO_SYNC | heed::EnvFlags::NO_META_SYNC);
                opts.open(dir.path()).unwrap()
            };
            let mut wtxn = env.write_txn().unwrap();
            let db: AccountDb = env.create_database(&mut wtxn, None).unwrap();
            wtxn.commit().unwrap();
            Self { _dir: dir, env, db }
        }
        pub fn load(rows: u64) -> Self {
            let e = Self::new();
            let mut w = e.env.write_txn().unwrap();
            for k in 0..rows {
                let bytes = serde_json::to_vec(&Account::make(k)).unwrap();
                e.db.put(&mut w, &k, &bytes).unwrap();
            }
            w.commit().unwrap();
            e
        }
        pub fn insert(&self, k: u64) {
            let bytes = serde_json::to_vec(&Account::make(k)).unwrap();
            let mut w = self.env.write_txn().unwrap();
            self.db.put(&mut w, &k, &bytes).unwrap();
            w.commit().unwrap();
        }
        pub fn point_get_checksum(&self, keys: &[u64]) -> u64 {
            let r = self.env.read_txn().unwrap();
            keys.iter().fold(0u64, |acc, k| {
                let v = self.db.get(&r, k).unwrap().expect("row");
                let a: Account = serde_json::from_slice(v).unwrap();
                acc.wrapping_add(a.checksum())
            })
        }
        /// One write txn per update — matches WorkTable's per-op publish and the
        /// redb engine (see module note); do NOT batch the loop in one txn.
        pub fn update_balance(&self, keys: &[u64]) {
            for k in keys {
                let mut w = self.env.write_txn().unwrap();
                let mut a: Account = {
                    let v = self.db.get(&w, k).unwrap().expect("row");
                    serde_json::from_slice(v).unwrap()
                };
                a.balance = (*k as f64) * 2.25;
                let bytes = serde_json::to_vec(&a).unwrap();
                self.db.put(&mut w, k, &bytes).unwrap();
                w.commit().unwrap();
            }
        }
        pub fn query_active_over_age_checksum(&self, min_age: u32) -> u64 {
            let r = self.env.read_txn().unwrap();
            let mut acc = 0u64;
            for row in self.db.iter(&r).unwrap() {
                let (_, v) = row.unwrap();
                let a: Account = serde_json::from_slice(v).unwrap();
                if a.active && a.age >= min_age {
                    acc = acc.wrapping_add(a.id);
                }
            }
            acc
        }
    }
}
