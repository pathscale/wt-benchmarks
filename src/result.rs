use std::collections::BTreeMap;

use serde::Serialize;

use crate::config::Config;

#[derive(Debug, Serialize)]
pub struct LatencySummary {
    pub samples: usize,
    pub p50_ns: Option<u64>,
    pub p95_ns: Option<u64>,
    pub p99_ns: Option<u64>,
    pub p999_ns: Option<u64>,
    pub max_ns: Option<u64>,
}

impl LatencySummary {
    pub fn from_samples(mut samples: Vec<u64>) -> Self {
        samples.sort_unstable();
        Self {
            samples: samples.len(),
            p50_ns: percentile(&samples, 0.50),
            p95_ns: percentile(&samples, 0.95),
            p99_ns: percentile(&samples, 0.99),
            p999_ns: percentile(&samples, 0.999),
            max_ns: samples.last().copied(),
        }
    }
}

fn percentile(samples: &[u64], fraction: f64) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let rank = ((samples.len() - 1) as f64 * fraction).ceil() as usize;
    samples.get(rank).copied()
}

#[derive(Debug, Serialize)]
pub struct RunResult {
    pub schema_version: u32,
    pub suite: &'static str,
    pub engine: &'static str,
    pub repetition: usize,
    pub workload: String,
    pub distribution: String,
    pub records_initial: u64,
    pub operations_requested: u64,
    pub operations_completed: u64,
    pub errors: u64,
    pub retryable_errors: u64,
    pub threads: usize,
    pub field_bytes: usize,
    pub fields: usize,
    pub seed: u64,
    pub sample_every: u64,
    pub load_elapsed_ns: u128,
    pub elapsed_ns: u128,
    pub ops_per_second: f64,
    pub feature_versioned_row_publication: bool,
    pub transaction_semantics: &'static str,
    pub read_ownership: &'static str,
    pub engine_version: Option<&'static str>,
    pub target_arch: &'static str,
    pub target_os: &'static str,
    pub operation_counts: BTreeMap<String, u64>,
    pub operation_errors: BTreeMap<String, u64>,
    pub latency: BTreeMap<String, LatencySummary>,
}

impl RunResult {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: &Config,
        repetition: usize,
        distribution: &str,
        completed: u64,
        errors: u64,
        load_elapsed_ns: u128,
        elapsed_ns: u128,
        operation_counts: BTreeMap<String, u64>,
        operation_errors: BTreeMap<String, u64>,
        latency: BTreeMap<String, LatencySummary>,
    ) -> Self {
        let seconds = elapsed_ns as f64 / 1_000_000_000.0;
        Self {
            schema_version: 1,
            suite: "ycsb",
            engine: "worktable",
            repetition,
            workload: config.workload.to_string(),
            distribution: distribution.to_owned(),
            records_initial: config.records,
            operations_requested: config.operations,
            operations_completed: completed,
            errors,
            retryable_errors: 0,
            threads: config.threads,
            field_bytes: config.field_bytes,
            fields: 10,
            seed: config.seed,
            sample_every: config.sample_every,
            load_elapsed_ns,
            elapsed_ns,
            ops_per_second: completed as f64 / seconds,
            feature_versioned_row_publication: cfg!(feature = "versioned-row-publication"),
            transaction_semantics: "WorkTable operations; read-modify-write is two application calls",
            read_ownership: "materialized-owned-row",
            engine_version: None,
            target_arch: std::env::consts::ARCH,
            target_os: std::env::consts::OS,
            operation_counts,
            operation_errors,
            latency,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn for_engine(
        config: &Config,
        repetition: usize,
        distribution: &str,
        engine: &'static str,
        completed: u64,
        errors: u64,
        retryable_errors: u64,
        load_elapsed_ns: u128,
        elapsed_ns: u128,
        operation_counts: BTreeMap<String, u64>,
        operation_errors: BTreeMap<String, u64>,
        latency: BTreeMap<String, LatencySummary>,
        transaction_semantics: &'static str,
        read_ownership: &'static str,
        engine_version: Option<&'static str>,
    ) -> Self {
        let mut result = Self::new(
            config,
            repetition,
            distribution,
            completed,
            errors,
            load_elapsed_ns,
            elapsed_ns,
            operation_counts,
            operation_errors,
            latency,
        );
        result.engine = engine;
        result.retryable_errors = retryable_errors;
        result.transaction_semantics = transaction_semantics;
        result.read_ownership = read_ownership;
        result.engine_version = engine_version;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_percentiles() {
        let summary = LatencySummary::from_samples((1..=1_000).collect());
        assert_eq!(summary.p50_ns, Some(501));
        assert_eq!(summary.p99_ns, Some(991));
        assert_eq!(summary.max_ns, Some(1_000));
    }
}
