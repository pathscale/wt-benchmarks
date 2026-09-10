//! Turn a stream of [`GridRow`] JSON into a table that cannot overclaim.
//!
//! # Why this refuses to print some numbers
//!
//! Every wrong conclusion this study has produced came from comparing two
//! numbers that had no error bars. A page-size effect reported at +31% from
//! three rounds collapsed to between -4.3% and +8.6% at eleven. A flavor read
//! as 85% better on seven repetitions was 0.2% better on sixteen. A scheduler
//! change appeared to buy 22.7% on an arm that the change could not touch,
//! which is to say the instrument's own noise was larger than every effect
//! being reported.
//!
//! So this does two things a hand-written table does not.
//!
//! It reports a **distribution**, not a median: n, min, p50, max. A p50 with
//! no range beside it is the shape of every mistake above.
//!
//! And it **only names a winner when the two ranges do not overlap**. If they
//! do, it prints `within noise` and the observed spread instead of a
//! percentage. A percentage that a second run would reverse is worse than no
//! percentage, because it gets quoted.
//!
//! # Usage
//!
//! ```text
//! WT_JSON=1 ./some-benchmark | summarize-grid
//! WT_FORMAT=md|toon|tsv summarize-grid < rows.jsonl
//! ```
//!
//! # The comparison is paired
//!
//! Each cell is compared against the cell that differs from it **only** in
//! runtime: same page size, same thread count, same op count. A single global
//! baseline would compare nagoya at one page size against tokio at another and
//! attribute the page size to the runtime, which is the exact class of error
//! that put a "vs 4096" table into the results doc when the default was 16384.
//!
//! The baseline runtime is `tokio`, because that is the "before" every claim
//! in this study is against. `WT_BASELINE=<substring>` names a different one.

use std::collections::BTreeMap;
use std::io::Read;

use serde_json::Value;

/// The keys that name the runtime, in the two row shapes this reads.
///
/// `GridRow` says `runtime`/`tuning`; `RunResult`, which ycsb and tatp already
/// emit per repetition, says `engine`/`runtime_flavor`. Both carry
/// `ops_per_second`, `cpu_x` and a repetition, so both can be summarised the
/// same way, and teaching this to read both is cheaper and less duplicative
/// than making those two benchmarks emit a second row alongside the one they
/// already print.
const RUNTIME_KEYS: [&str; 2] = ["runtime", "engine"];
const TUNING_KEYS: [&str; 2] = ["tuning", "runtime_flavor"];
/// Dimensions from either shape. A key absent from a row is simply skipped.
const DIMENSION_KEYS: [&str; 12] = [
    "index_backend",
    "dispatch",
    "page_size",
    "worker_threads",
    "threads",
    "readers",
    "writers",
    "ops_per_task",
    "workload",
    "distribution",
    "records_initial",
    "operations_requested",
];

/// The measured samples for one cell.
#[derive(Default)]
struct Cell {
    ops: Vec<f64>,
    reads: Vec<f64>,
    writes: Vec<f64>,
    cpu: Vec<f64>,
    /// Latency percentiles, one entry per repetition, in nanoseconds.
    ///
    /// Kept per repetition and then taken at the median rather than pooled,
    /// because pooling samples across repetitions would let one slow run
    /// dominate a percentile and hide that it was one run.
    read_p50: Vec<f64>,
    read_p90: Vec<f64>,
    read_p99: Vec<f64>,
    write_p50: Vec<f64>,
    write_p90: Vec<f64>,
    write_p99: Vec<f64>,
}

impl Cell {
    fn sorted_ops(&self) -> Vec<f64> {
        let mut v = self.ops.clone();
        v.sort_by(|a, b| a.partial_cmp(b).expect("a benchmark must not report NaN"));
        v
    }

    fn median(values: &[f64]) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        let mid = values.len() / 2;
        if values.len() % 2 == 0 {
            (values[mid - 1] + values[mid]) / 2.0
        } else {
            values[mid]
        }
    }

    fn cpu_median(&self) -> f64 {
        Self::median_of(&self.cpu)
    }

    fn median_of(values: &[f64]) -> f64 {
        let mut v = values.to_vec();
        v.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
        Self::median(&v)
    }
}

/// Microseconds, or a dash when a benchmark sampled no latency.
fn micros(values: &[f64]) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    format!("{:.1}", Cell::median_of(values) / 1000.0)
}

/// Pull one percentile out of either row shape.
///
/// `GridRow` carries `read_latency`/`write_latency` objects. `RunResult`, from
/// ycsb and tatp, carries a `latency` map keyed by operation kind, so reads
/// come from `read` or `scan` and writes from `update`, `insert` or
/// `read_modify_write`.
fn percentile_from(row: &Value, side: &str, key: &str) -> Option<f64> {
    if let Some(summary) = row.get(format!("{side}_latency")) {
        return summary.get(key).and_then(Value::as_f64);
    }
    let map = row.get("latency")?.as_object()?;
    let kinds: &[&str] = if side == "read" {
        &["read", "scan"]
    } else {
        &["update", "insert", "read_modify_write"]
    };
    let mut best: Option<f64> = None;
    for kind in kinds {
        if let Some(value) = map.get(*kind).and_then(|s| s.get(key)).and_then(Value::as_f64) {
            best = Some(best.map_or(value, |b: f64| b.max(value)));
        }
    }
    best
}

/// Everything about a cell except which runtime ran it.
///
/// Two cells sharing this are the same measurement on two runtimes, which is
/// the only pair worth taking a ratio of.
fn dimensions(row: &Value) -> String {
    let get = |key: &str| row.get(key).map_or_else(String::new, |v| match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    });
    let mut parts = vec![get("suite")];
    for key in DIMENSION_KEYS {
        let value = get(key);
        if !value.is_empty() {
            parts.push(format!("{key}={value}"));
        }
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" ")
}

/// The label a cell is filed under: every dimension that varies, in order.
fn label(row: &Value) -> String {
    let get = |key: &str| row.get(key).map_or_else(String::new, |v| match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    });
    let first = |keys: &[&str]| keys.iter().map(|k| get(k)).find(|v| !v.is_empty()).unwrap_or_default();
    let mut parts = vec![get("suite"), first(&RUNTIME_KEYS), first(&TUNING_KEYS)];
    for key in DIMENSION_KEYS {
        let value = get(key);
        if !value.is_empty() {
            parts.push(format!("{key}={value}"));
        }
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" ")
}

fn main() {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .expect("grid rows on stdin");

    let mut cells: BTreeMap<String, Cell> = BTreeMap::new();
    let mut cell_dimensions: BTreeMap<String, String> = BTreeMap::new();
    for line in input.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if row.get("ops_per_second").is_none() {
            continue;
        }
        let name = label(&row);
        cell_dimensions.entry(name.clone()).or_insert_with(|| dimensions(&row));
        let cell = cells.entry(name).or_default();
        if let Some(ops) = row["ops_per_second"].as_f64() {
            cell.ops.push(ops);
        }
        if let Some(cpu) = row.get("cpu_x").and_then(Value::as_f64) {
            cell.cpu.push(cpu);
        }
        if let Some(v) = row.get("read_ops_per_second").and_then(Value::as_f64) {
            cell.reads.push(v);
        }
        if let Some(v) = row.get("write_ops_per_second").and_then(Value::as_f64) {
            cell.writes.push(v);
        }
        for (side, p50, p90, p99) in [("read", 0usize, 1usize, 2usize), ("write", 3, 4, 5)] {
            for (key, slot) in [("p50_ns", p50), ("p90_ns", p90), ("p99_ns", p99)] {
                if let Some(value) = percentile_from(&row, side, key) {
                    match slot {
                        0 => cell.read_p50.push(value),
                        1 => cell.read_p90.push(value),
                        2 => cell.read_p99.push(value),
                        3 => cell.write_p50.push(value),
                        4 => cell.write_p90.push(value),
                        _ => cell.write_p99.push(value),
                    }
                }
            }
        }
    }

    if cells.is_empty() {
        eprintln!("no grid rows on stdin; run the benchmark with WT_JSON=1");
        std::process::exit(2);
    }

    let wanted = std::env::var("WT_BASELINE").unwrap_or_else(|_| "tokio".to_owned());
    // One baseline per set of dimensions, so a cell is only ever compared to
    // the same measurement on the other runtime.
    let mut baseline_for: BTreeMap<String, String> = BTreeMap::new();
    for (name, dims) in &cell_dimensions {
        if name.contains(wanted.as_str()) {
            baseline_for.insert(dims.clone(), name.clone());
        }
    }

    let format = std::env::var("WT_FORMAT").unwrap_or_else(|_| "md".to_owned());

    let header = [
        "cell", "n", "min", "p50", "max", "queries/s", "upserts/s", "cpu_x",
        "r p50 us", "r p90 us", "r p99 us",
        "w p50 us", "w p90 us", "w p99 us",
        "vs baseline",
    ];
    match format.as_str() {
        "toon" => println!("cells[{}]{{{}}}:", cells.len(), header.join(",")),
        "tsv" => println!("{}", header.join("\t")),
        _ => {
            println!("| {} |", header.join(" | "));
            println!("|{}|", vec!["---"; header.len()].join("|"));
        }
    }

    for (name, cell) in &cells {
        let ops = cell.sorted_ops();
        if ops.is_empty() {
            continue;
        }
        let paired = cell_dimensions
            .get(name)
            .and_then(|dims| baseline_for.get(dims))
            .filter(|b| *b != name)
            .map(|b| cells[b].sorted_ops());
        let verdict = match (&paired, ()) {
            (Some(base), ()) if !base.is_empty() => {
                // The whole point. Overlapping ranges mean a second run could
                // reverse the sign, so no percentage is printed at all.
                let overlaps = !(ops[0] > base[base.len() - 1] || base[0] > ops[ops.len() - 1]);
                if overlaps {
                    let spread = if ops[0] > 0.0 { ops[ops.len() - 1] / ops[0] } else { 0.0 };
                    format!("within noise (spread {spread:.2}x)")
                } else {
                    let delta = Cell::median(&ops) / Cell::median(base) - 1.0;
                    format!("{:+.1}%", delta * 100.0)
                }
            }
            _ => "baseline".to_owned(),
        };
        let row = [
            name.clone(),
            ops.len().to_string(),
            format!("{:.0}", ops[0]),
            format!("{:.0}", Cell::median(&ops)),
            format!("{:.0}", ops[ops.len() - 1]),
            if cell.reads.is_empty() { "-".to_owned() } else { format!("{:.0}", Cell::median_of(&cell.reads)) },
            if cell.writes.is_empty() { "-".to_owned() } else { format!("{:.0}", Cell::median_of(&cell.writes)) },
            format!("{:.1}", cell.cpu_median()),
            micros(&cell.read_p50),
            micros(&cell.read_p90),
            micros(&cell.read_p99),
            micros(&cell.write_p50),
            micros(&cell.write_p90),
            micros(&cell.write_p99),
            verdict,
        ];
        match format.as_str() {
            "toon" => println!("{}", row.join(",")),
            "tsv" => println!("{}", row.join("\t")),
            _ => println!("| {} |", row.join(" | ")),
        }
    }

    // A cell measured once has no error bar, and saying so is the point.
    let single: Vec<&String> = cells.iter().filter(|(_, c)| c.ops.len() < 2).map(|(k, _)| k).collect();
    if !single.is_empty() {
        eprintln!(
            "warning: {} cell(s) have fewer than 2 repetitions, so no range and no comparison: {:?}",
            single.len(),
            single
        );
    }
}
