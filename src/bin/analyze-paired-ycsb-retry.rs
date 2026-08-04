use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

use eyre::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use wt_benchmarks::rng::{Rng, mix64};

const BOOTSTRAP_SAMPLES: usize = 20_000;

#[derive(Deserialize)]
struct Latency {
    p50_ns: Option<u64>,
    p99_ns: Option<u64>,
}

#[derive(Deserialize)]
struct Record {
    backend: String,
    campaign_pair: Option<usize>,
    operations: u64,
    first_read_misses: u64,
    retry_recovered: u64,
    final_read_errors: u64,
    insert_errors: u64,
    ops_per_second: f64,
    read_latency: Latency,
    stable_index_read_retry: bool,
}

#[derive(Default)]
struct Pair {
    off: Option<Record>,
    stable: Option<Record>,
}

#[derive(Serialize)]
struct Interval {
    lower: f64,
    upper: f64,
}

#[derive(Serialize)]
struct BackendSummary {
    backend: String,
    pairs: usize,
    operations: u64,
    feature_off_first_read_misses: u64,
    feature_off_retry_recovered: u64,
    feature_on_first_read_misses: u64,
    final_read_errors: u64,
    insert_errors: u64,
    feature_off_median_ops_per_second: f64,
    feature_on_median_ops_per_second: f64,
    median_paired_delta_percent: f64,
    paired_delta_bootstrap_95_percent: Interval,
    minimum_paired_delta_percent: f64,
    maximum_paired_delta_percent: f64,
    feature_off_median_read_p50_ns: f64,
    feature_on_median_read_p50_ns: f64,
    feature_off_median_read_p99_ns: f64,
    feature_on_median_read_p99_ns: f64,
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_unstable_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

fn bootstrap_median_interval(values: &[f64], seed: u64) -> Interval {
    let mut rng = Rng::new(seed);
    let mut sample = vec![0.0; values.len()];
    let mut medians = Vec::with_capacity(BOOTSTRAP_SAMPLES);
    for _ in 0..BOOTSTRAP_SAMPLES {
        for value in &mut sample {
            *value = values[rng.below(values.len() as u64) as usize];
        }
        medians.push(median(&mut sample));
    }
    medians.sort_unstable_by(f64::total_cmp);
    let last = medians.len() - 1;
    Interval {
        lower: medians[((last as f64) * 0.025).round() as usize],
        upper: medians[((last as f64) * 0.975).round() as usize],
    }
}

fn latency_values<'a>(records: impl Iterator<Item = &'a Record>, p99: bool) -> Vec<f64> {
    records
        .filter_map(|record| {
            if p99 {
                record.read_latency.p99_ns
            } else {
                record.read_latency.p50_ns
            }
        })
        .map(|value| value as f64)
        .collect()
}

fn summarize(backend: String, pairs: BTreeMap<usize, Pair>) -> Result<BackendSummary> {
    if pairs.is_empty() {
        bail!("backend {backend} has no pairs");
    }

    let mut off = Vec::with_capacity(pairs.len());
    let mut stable = Vec::with_capacity(pairs.len());
    for (pair_number, pair) in pairs {
        off.push(pair.off.ok_or_else(|| {
            eyre::eyre!("backend {backend} pair {pair_number} is missing feature-off result")
        })?);
        stable.push(pair.stable.ok_or_else(|| {
            eyre::eyre!("backend {backend} pair {pair_number} is missing feature-on result")
        })?);
    }

    let mut off_throughput = off
        .iter()
        .map(|record| record.ops_per_second)
        .collect::<Vec<_>>();
    let mut stable_throughput = stable
        .iter()
        .map(|record| record.ops_per_second)
        .collect::<Vec<_>>();
    let paired_deltas = off
        .iter()
        .zip(&stable)
        .map(|(off, stable)| (stable.ops_per_second / off.ops_per_second - 1.0) * 100.0)
        .collect::<Vec<_>>();
    let mut sorted_deltas = paired_deltas.clone();
    let median_paired_delta_percent = median(&mut sorted_deltas);

    let mut off_p50 = latency_values(off.iter(), false);
    let mut stable_p50 = latency_values(stable.iter(), false);
    let mut off_p99 = latency_values(off.iter(), true);
    let mut stable_p99 = latency_values(stable.iter(), true);
    if [
        off_p50.len(),
        stable_p50.len(),
        off_p99.len(),
        stable_p99.len(),
    ]
    .iter()
    .any(|length| *length != off.len())
    {
        bail!("backend {backend} has a missing p50 or p99 latency summary");
    }

    let seed = backend.bytes().fold(0x05ee_d187_u64, |state, byte| {
        mix64(state ^ u64::from(byte))
    });
    Ok(BackendSummary {
        backend,
        pairs: off.len(),
        operations: off
            .iter()
            .chain(&stable)
            .map(|record| record.operations)
            .sum(),
        feature_off_first_read_misses: off.iter().map(|record| record.first_read_misses).sum(),
        feature_off_retry_recovered: off.iter().map(|record| record.retry_recovered).sum(),
        feature_on_first_read_misses: stable.iter().map(|record| record.first_read_misses).sum(),
        final_read_errors: off
            .iter()
            .chain(&stable)
            .map(|record| record.final_read_errors)
            .sum(),
        insert_errors: off
            .iter()
            .chain(&stable)
            .map(|record| record.insert_errors)
            .sum(),
        feature_off_median_ops_per_second: median(&mut off_throughput),
        feature_on_median_ops_per_second: median(&mut stable_throughput),
        median_paired_delta_percent,
        paired_delta_bootstrap_95_percent: bootstrap_median_interval(&paired_deltas, seed),
        minimum_paired_delta_percent: sorted_deltas[0],
        maximum_paired_delta_percent: sorted_deltas[sorted_deltas.len() - 1],
        feature_off_median_read_p50_ns: median(&mut off_p50),
        feature_on_median_read_p50_ns: median(&mut stable_p50),
        feature_off_median_read_p99_ns: median(&mut off_p99),
        feature_on_median_read_p99_ns: median(&mut stable_p99),
    })
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| eyre::eyre!("usage: analyze-paired-ycsb-retry RESULTS.jsonl"))?;
    let reader = BufReader::new(File::open(&path).wrap_err_with(|| format!("open {path}"))?);
    let mut grouped = BTreeMap::<String, BTreeMap<usize, Pair>>::new();

    for (line_number, line) in reader.lines().enumerate() {
        let line = line.wrap_err_with(|| format!("read {} line {}", path, line_number + 1))?;
        let record: Record = serde_json::from_str(&line)
            .wrap_err_with(|| format!("parse {} line {}", path, line_number + 1))?;
        let pair_number = record
            .campaign_pair
            .ok_or_else(|| eyre::eyre!("{} line {} has no campaign_pair", path, line_number + 1))?;
        let pair = grouped
            .entry(record.backend.clone())
            .or_default()
            .entry(pair_number)
            .or_default();
        let slot = if record.stable_index_read_retry {
            &mut pair.stable
        } else {
            &mut pair.off
        };
        if slot.replace(record).is_some() {
            bail!("{} contains a duplicate result in pair {pair_number}", path);
        }
    }

    let summaries = grouped
        .into_iter()
        .map(|(backend, pairs)| summarize(backend, pairs))
        .collect::<Result<Vec<_>>>()?;
    println!("{}", serde_json::to_string_pretty(&summaries)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_handles_odd_and_even_samples() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), 2.5);
    }

    #[test]
    fn constant_bootstrap_interval_is_exact() {
        let interval = bootstrap_median_interval(&[1.25; 15], 42);
        assert_eq!(interval.lower, 1.25);
        assert_eq!(interval.upper, 1.25);
    }
}
