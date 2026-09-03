//! Summarize repeated AgentCode JSONL runs. Arguments are `LABEL:PATH`.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Deserialize)]
struct PhaseResult {
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
            run.insert(format!("{}/{}", row.mode, row.phase), row.nanos_per_row);
            results
                .entry(format!("{}/{}", row.mode, row.phase))
                .or_default()
                .entry(label.to_owned())
                .or_default()
                .push(row.nanos_per_row);
        }
        for (name, first, second) in [
            (
                "disk/one_at_a_time_total",
                "disk/put_symbols_one_at_a_time",
                "disk/wait_for_ops_one_at_a_time",
            ),
            (
                "disk/insert_many_total",
                "disk/put_symbols_insert_many",
                "disk/wait_for_ops_after_batch",
            ),
        ] {
            let total = run
                .get(first)
                .zip(run.get(second))
                .map(|(first, second)| first + second)
                .ok_or_else(|| format!("{path}: missing phase for {name}"))?;
            results
                .entry(name.to_owned())
                .or_default()
                .entry(label.to_owned())
                .or_default()
                .push(total);
        }
    }

    println!(
        "| AgentCode phase | beta13 ns/row | beta15 ns/row | beta17 ns/row | b17 vs b13 | b17 vs b15 |"
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
        let beta17 = summarized.get("beta17").ok_or("missing beta17")?;
        println!(
            "| {phase} | {:.2} [{:.2}–{:.2}] | {:.2} [{:.2}–{:.2}] | {:.2} [{:.2}–{:.2}] | {:+.1}% | {:+.1}% |",
            beta13.1,
            beta13.0,
            beta13.2,
            beta15.1,
            beta15.0,
            beta15.2,
            beta17.1,
            beta17.0,
            beta17.2,
            (beta17.1 / beta13.1 - 1.0) * 100.0,
            (beta17.1 / beta15.1 - 1.0) * 100.0,
        );
    }
    Ok(())
}
