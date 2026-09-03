//! Summarize repeated Criterion text runs without treating one noisy pass as
//! the result.
//!
//! Arguments are `LABEL:PATH`; repeat a label for every rotated pass.

use std::collections::BTreeMap;

fn duration_ns(value: &str, unit: &str) -> Result<f64, String> {
    let value: f64 = value
        .parse()
        .map_err(|_| format!("bad duration {value:?}"))?;
    let scale = match unit {
        "ps" => 0.001,
        "ns" => 1.0,
        "µs" | "us" => 1_000.0,
        "ms" => 1_000_000.0,
        "s" => 1_000_000_000.0,
        _ => return Err(format!("unknown duration unit {unit:?}")),
    };
    Ok(value * scale)
}

fn time_median_ns(line: &str) -> Result<Option<f64>, String> {
    let Some((_, bracketed)) = line.split_once("time:") else {
        return Ok(None);
    };
    if bracketed.contains('%') {
        return Ok(None);
    }
    let inner = bracketed
        .trim()
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| format!("malformed Criterion time line: {line}"))?;
    // Criterion prints a second `time:` line for percentage change when an
    // old baseline exists in the target directory. It is comparison metadata,
    // not a duration measurement for this pass.
    let fields: Vec<_> = inner.split_whitespace().collect();
    if fields.len() != 6 {
        return Err(format!("expected three duration bounds: {line}"));
    }
    duration_ns(fields[2], fields[3]).map(Some)
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
        let mut benchmark = None;
        let mut run = BTreeMap::new();
        for line in contents.lines() {
            if let Some(name) = line
                .strip_prefix("Benchmarking ")
                .and_then(|line| line.strip_suffix(": Analyzing"))
            {
                benchmark = Some(name.to_owned());
                continue;
            }
            let Some(value) = time_median_ns(line)? else {
                continue;
            };
            let name = benchmark
                .take()
                .ok_or_else(|| format!("time line without benchmark in {path}: {line}"))?;
            run.insert(name.clone(), value);
            results
                .entry(name)
                .or_default()
                .entry(label.to_owned())
                .or_default()
                .push(value);
        }
        for backend in ["arctic", "congee", "worktables_index"] {
            let publish = format!("moe_pgo/publish/{backend}/12288");
            let retire = format!("moe_pgo/retire/{backend}/12288");
            if let Some(total) = run
                .get(&publish)
                .zip(run.get(&retire))
                .map(|(publish, retire)| publish + retire)
            {
                results
                    .entry(format!("moe_pgo/publish_plus_retire/{backend}/12288"))
                    .or_default()
                    .entry(label.to_owned())
                    .or_default()
                    .push(total);
            }
        }
    }

    println!("| benchmark | beta13 ns | beta15 ns | beta17 ns | b17 vs b13 | b17 vs b15 |");
    println!("|---|---:|---:|---:|---:|---:|");
    for (benchmark, versions) in results {
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
            "| {benchmark} | {:.3} [{:.3}–{:.3}] | {:.3} [{:.3}–{:.3}] | {:.3} [{:.3}–{:.3}] | {:+.1}% | {:+.1}% |",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_middle_bound_and_normalizes_units() {
        assert_eq!(
            time_median_ns("thing time:   [900.0 ns 1.2500 µs 2.0 µs]").unwrap(),
            Some(1_250.0)
        );
    }

    #[test]
    fn median_rejects_one_noisy_extreme() {
        assert_eq!(median(&mut [10.0, 100.0, 11.0]), 11.0);
    }

    #[test]
    fn ignores_criterion_baseline_percentage_lines() {
        assert_eq!(
            time_median_ns("thing time:   [+1.0% +2.0% +3.0%]").unwrap(),
            None
        );
    }
}
