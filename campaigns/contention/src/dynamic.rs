//! The naive dynamic baseline for the specialization comparison (paper Table 1).
//!
//! This is deliberately the "dirty dynamic Vec" engine — what you get when you
//! reach for a slot vector + BTreeMap + tagged values instead of a real table:
//!   * rows are vectors of tagged values (boxed representation)
//!   * a runtime catalog maps column name -> position (hash lookup per access)
//!   * rows are encoded/decoded through a tag-dispatched serializer per access
//!   * row locking is a coarse per-row mutex from a dynamic lock table
//!
//! WHAT THIS BASELINE IS FOR (do not "fix" it to be faster or fairer):
//! WorkTable is NOT expected to beat this on raw point ops, and by design it may
//! not. That is the point. A bare Vec + BTreeMap skips a real page allocator,
//! B-tree, secondary indexes, and durable structure, so it *should* look fast on
//! a trivial point workload. The comparison shows the cost of that shortcut:
//! DynTable buys point-op speed by throwing away everything a table gives you.
//! It sits alongside the real dynamic engines (SQLite, DuckDB) so the paper can
//! contrast WorkTable against three different kinds of alternative — a naive
//! hand-rolled dynamic store, an embedded SQL engine, and a columnar SQL engine
//! — rather than a single strawman. A "win" here would mean the baseline is
//! secretly doing WorkTable's work; keep it naive on purpose.

use parking_lot::{Mutex, RwLock};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    U64(u64),
    F64(f64),
    Str(String),
}

pub struct DynSchema {
    pub cols: Vec<&'static str>,
    pub catalog: HashMap<&'static str, usize>,
}

impl DynSchema {
    pub fn bench() -> Self {
        let cols = vec!["id", "a", "b", "e", "c", "d"];
        let catalog = cols.iter().enumerate().map(|(i, c)| (*c, i)).collect();
        DynSchema { cols, catalog }
    }
}

fn encode(row: &[Value], out: &mut Vec<u8>) {
    out.clear();
    for v in row {
        match v {
            Value::U64(x) => {
                out.push(0);
                out.extend_from_slice(&x.to_le_bytes());
            }
            Value::F64(x) => {
                out.push(1);
                out.extend_from_slice(&x.to_le_bytes());
            }
            Value::Str(s) => {
                out.push(2);
                out.extend_from_slice(&(s.len() as u32).to_le_bytes());
                out.extend_from_slice(s.as_bytes());
            }
        }
    }
}

fn decode(mut b: &[u8]) -> Vec<Value> {
    let mut row = Vec::with_capacity(6);
    while !b.is_empty() {
        match b[0] {
            0 => {
                row.push(Value::U64(u64::from_le_bytes(b[1..9].try_into().unwrap())));
                b = &b[9..];
            }
            1 => {
                row.push(Value::F64(f64::from_le_bytes(b[1..9].try_into().unwrap())));
                b = &b[9..];
            }
            2 => {
                let n = u32::from_le_bytes(b[1..5].try_into().unwrap()) as usize;
                row.push(Value::Str(String::from_utf8(b[5..5 + n].to_vec()).unwrap()));
                b = &b[5 + n..];
            }
            _ => unreachable!(),
        }
    }
    row
}

pub struct DynTable {
    pub schema: DynSchema,
    slots: RwLock<Vec<Vec<u8>>>,
    pk_index: RwLock<BTreeMap<u64, usize>>,
    locks: RwLock<HashMap<u64, Arc<Mutex<()>>>>,
    next_pk: AtomicU64,
}

impl DynTable {
    pub fn new() -> Self {
        DynTable {
            schema: DynSchema::bench(),
            slots: RwLock::new(Vec::new()),
            pk_index: RwLock::new(BTreeMap::new()),
            locks: RwLock::new(HashMap::new()),
            next_pk: AtomicU64::new(0),
        }
    }

    pub fn get_next_pk(&self) -> u64 {
        self.next_pk.fetch_add(1, Ordering::AcqRel)
    }

    pub fn insert(&self, row: Vec<Value>) -> u64 {
        let pk = match &row[0] {
            Value::U64(x) => *x,
            _ => panic!("pk must be u64"),
        };
        let mut buf = Vec::with_capacity(64);
        encode(&row, &mut buf);
        let mut slots = self.slots.write();
        let idx = slots.len();
        slots.push(buf);
        drop(slots);
        self.pk_index.write().insert(pk, idx);
        pk
    }

    pub fn select(&self, pk: u64) -> Option<Vec<Value>> {
        let idx = *self.pk_index.read().get(&pk)?;
        let slots = self.slots.read();
        Some(decode(&slots[idx]))
    }

    /// Field update through the catalog — the dynamic path a specialized
    /// `update_upd_b` avoids: lock table, hash lookup, decode, dispatch,
    /// re-encode, write back.
    pub fn update_field(&self, pk: u64, col: &str, v: Value) -> Option<()> {
        let lock = {
            let mut locks = self.locks.write();
            locks.entry(pk).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
        };
        let _g = lock.lock();
        let idx = *self.pk_index.read().get(&pk)?;
        let pos = *self.schema.catalog.get(col)?;
        let mut slots = self.slots.write();
        let mut row = decode(&slots[idx]);
        row[pos] = v;
        let mut buf = Vec::with_capacity(64);
        encode(&row, &mut buf);
        slots[idx] = buf;
        Some(())
    }
}

pub fn mk_dyn_row(pk: u64, v: u64) -> Vec<Value> {
    vec![
        Value::U64(pk),
        Value::U64(v),
        Value::U64(v),
        Value::U64(v),
        Value::F64(v as f64),
        Value::Str("payloadpayload".to_string()),
    ]
}
