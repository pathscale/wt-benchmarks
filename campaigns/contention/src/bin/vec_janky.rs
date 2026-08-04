//! vec_janky — the baseline modeled on ACTUAL production code, not a tidy
//! textbook `Vec`. This is the janky tabular-over-`Vec` access pattern found
//! in the wild in web3.trading-backend and AgencyZero:
//!
//!   * lookup a row by a non-primary key via a LINEAR SCAN
//!       `slippage_rows.iter().find(|s| s.event_id == close_id)`
//!       (web3.trading-backend/src/handlers/s3/sub_s3_execution.rs:140,204)
//!     — no secondary index; every "find by field" is O(n).
//!
//!   * records carry an embedded JSON payload that is (de)serialized on the
//!     value path, mirroring the exchange adapters that do
//!       `serde_json::from_value(result_value.clone())`  (deribit rest/order.rs)
//!       `serde_json::from_str(raw)...`                   (AgencyZero wt-tools)
//!     — typed access costs a parse every time.
//!
//! This is what a real Rust app does BEFORE adopting an engine: a `Vec`, no
//! index, and JSON at the typing boundary. WorkTable replaces exactly this —
//! a generated B-tree index (no scan) and a typed archive (no per-access
//! parse). If WorkTable is competitive or ahead of THIS, that is the honest,
//! compelling comparison; the tidy `Vec`+`HashMap` baseline (vec_realistic)
//! flatters the hand-rolled side by assuming discipline real code rarely has.
//!
//! CSV to stdout: bench,engine,op,ops_per_sec

use serde_json::json;
use wt_contention_campaign::util::*;

/// A record as janky code stores it: typed primary key, but the "columns" live
/// in an embedded JSON blob that must be parsed to read a field. This mirrors
/// rows that arrive as `serde_json::Value` and are stashed without a schema.
#[derive(Clone, Debug)]
struct JankyRecord {
    id: u64,
    event_id: u64, // the field looked up by linear scan (no index)
    payload: String, // embedded JSON: {"a":..,"b":..,"c":..,"d":".."}
}

fn mk_janky(id: u64) -> JankyRecord {
    // build the payload the way janky code does: serialize a JSON object
    let payload = json!({
        "a": id,
        "b": id,
        "c": id as f64,
        "d": "payloadpayload",
    })
    .to_string();
    JankyRecord { id, event_id: id, payload }
}

/// Read field `b` out of a record the janky way: parse the embedded JSON and
/// pull the field. This is `serde_json::from_str` + `.get(...)` per access.
fn read_b(rec: &JankyRecord) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(&rec.payload).ok()?;
    v.get("b").and_then(serde_json::Value::as_u64)
}

fn main() {
    let rows = env_u64("ROWS", 1_000_000);
    let reps = env_u64("REPS", 5) as usize;
    // Linear-scan lookups are O(n); at 1M rows a full sweep is 1e12 ops. Cap
    // the number of scan lookups so the run finishes, and report the per-lookup
    // rate honestly (each lookup still scans the whole Vec).
    let scan_lookups = env_u64("SCAN_LOOKUPS", 2_000);
    println!("bench,engine,op,ops_per_sec");

    // ---- insert (build the Vec, serialize JSON payloads) ----
    let ins = median_ops_per_sec(reps, || {
        let mut v: Vec<JankyRecord> = Vec::with_capacity(rows as usize);
        for i in 0..rows {
            v.push(mk_janky(i));
        }
        std::hint::black_box(&v);
        rows
    });
    println!("vec_janky,vec_scan_json,insert,{ins:.0}");

    let store: Vec<JankyRecord> = (0..rows).map(mk_janky).collect();

    // ---- point read by pk == slot, then PARSE the payload to get a field ----
    // Even the "fast" path pays a JSON parse per typed access.
    let rd = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(42);
        for _ in 0..rows {
            let i = rng.below(rows) as usize;
            std::hint::black_box(read_b(&store[i]));
        }
        rows
    });
    println!("vec_janky,vec_scan_json,point_read_typed,{rd:.0}");

    // ---- secondary lookup by event_id via LINEAR SCAN (the real jank) ----
    // `store.iter().find(|s| s.event_id == key)` — O(n) every time.
    let sec = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(43);
        for _ in 0..scan_lookups {
            let key = rng.below(rows);
            std::hint::black_box(store.iter().find(|s| s.event_id == key));
        }
        scan_lookups
    });
    println!("vec_janky,vec_scan_json,select_by_secondary_scan,{sec:.0}");

    // ---- field update: find by scan, parse, mutate, re-serialize ----
    // The full janky write path: locate the record (scan), decode JSON, change
    // a field, re-encode. This is what an app without an engine actually does.
    let mut mutable = store.clone();
    let upd = median_ops_per_sec(reps, || {
        let mut rng = Rng::new(7);
        let n = scan_lookups; // scan-bounded, same reason as above
        for _ in 0..n {
            let key = rng.below(rows);
            if let Some(rec) = mutable.iter_mut().find(|s| s.event_id == key) {
                let mut v: serde_json::Value = serde_json::from_str(&rec.payload).unwrap();
                v["b"] = json!(key);
                rec.payload = v.to_string();
            }
        }
        n
    });
    println!("vec_janky,vec_scan_json,update_field_scan_json,{upd:.0}");
}
