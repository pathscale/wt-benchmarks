//! Summarize repeated AgentCode JSONL runs. Arguments are `LABEL:PATH`.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Deserialize)]
struct PhaseResult {
    backend: String,
    mode: String,
    phase: String,
    nanos_per_row: f64,
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let midpoint = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[midpoint - 1] + values[midpoint]) / 2.0
    } else {
        values[midpoint]
    }
}

fn main() -> Result<(), String> {
    let mut results: BTreeMap<String, BTreeMap<String, Vec<f64>>> = BTreeMap::new();
    for argument in std::env::args().skip(1) {
        let (label, path) = argument
            .split_once(':')
            .ok_or_else(|| format!("expected LABEL:PATH, got {argument:?}"))?;
        let contents = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
        let mut run = BTreeMap::new();
        for line in contents.lines().filter(|line| line.starts_with('{')) {
            let row: PhaseResult = serde_json::from_str(line)
                .map_err(|error| format!("{path}: malformed JSONL row: {error}"))?;
            run.insert(
                format!("{}/{}/{}", row.backend, row.mode, row.phase),
                row.nanos_per_row,
            );
            results
                .entry(format!("{}/{}/{}", row.backend, row.mode, row.phase))
                .or_default()
                .entry(label.to_owned())
                .or_default()
                .push(row.nanos_per_row);
        }
        for backend in ["wti", "arctic", "congee"] {
            for (name, first, second) in [
                (
                    "one_at_a_time_total",
                    "put_symbols_one_at_a_time",
                    "wait_for_ops_one_at_a_time",
                ),
                (
                    "insert_many_total",
                    "put_symbols_insert_many",
                    "wait_for_ops_after_batch",
                ),
            ] {
                let first = format!("{backend}/disk/{first}");
                let second = format!("{backend}/disk/{second}");
                let total = run
                    .get(&first)
                    .zip(run.get(&second))
                    .map(|(first, second)| first + second)
                    .ok_or_else(|| format!("{path}: missing phase for {backend}/{name}"))?;
                results
                    .entry(format!("{backend}/disk/{name}"))
                    .or_default()
                    .entry(label.to_owned())
                    .or_default()
                    .push(total);
            }
        }
    }

    println!(
        "| AgentCode phase | beta13 ns/row | beta15 ns/row | beta18 ns/row | b18 vs b13 | b18 vs b15 |"
    );
    println!("|---|---:|---:|---:|---:|---:|");
    for (phase, versions) in results {
        let mut summarized = BTreeMap::new();
        for (version, mut values) in versions {
            let low = values.iter().copied().min_by(f64::total_cmp).unwrap();
            let high = values.iter().copied().max_by(f64::total_cmp).unwrap();
            let middle = median(&mut values);
            summarized.insert(version, (low, middle, high));
        }
        let beta13 = summarized.get("beta13").ok_or("missing beta13")?;
        let beta15 = summarized.get("beta15").ok_or("missing beta15")?;
        let beta18 = summarized.get("beta18").ok_or("missing beta18")?;
        println!(
            "| {phase} | {:.2} [{:.2}–{:.2}] | {:.2} [{:.2}–{:.2}] | {:.2} [{:.2}–{:.2}] | {:+.1}% | {:+.1}% |",
            beta13.1,
            beta13.0,
            beta13.2,
            beta15.1,
            beta15.0,
            beta15.2,
            beta18.1,
            beta18.0,
            beta18.2,
            (beta18.1 / beta13.1 - 1.0) * 100.0,
            (beta18.1 / beta15.1 - 1.0) * 100.0,
        );
    }
    Ok(())
}
