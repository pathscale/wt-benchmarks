use std::env;

use crate::ycsb::{Distribution, Workload};

#[derive(Clone, Debug)]
pub struct Config {
    pub workload: Workload,
    pub records: u64,
    pub operations: u64,
    pub threads: usize,
    pub repetitions: usize,
    pub sample_every: u64,
    pub seed: u64,
    pub field_bytes: usize,
    pub scan_max: u64,
    pub zipf_theta: f64,
    pub distribution_override: Option<Distribution>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workload: Workload::A,
            records: 100_000,
            operations: 1_000_000,
            threads: 1,
            repetitions: 5,
            sample_every: 1_024,
            seed: 42,
            field_bytes: 100,
            scan_max: 100,
            zipf_theta: 0.99,
            distribution_override: None,
        }
    }
}

impl Config {
    pub fn from_args() -> Result<Self, String> {
        let mut config = Self::default();
        let mut args = env::args().skip(1);

        while let Some(flag) = args.next() {
            if flag == "--help" || flag == "-h" {
                print_help();
                std::process::exit(0);
            }

            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--workload" => config.workload = value.parse()?,
                "--records" => config.records = parse(&flag, &value)?,
                "--operations" => config.operations = parse(&flag, &value)?,
                "--threads" => config.threads = parse(&flag, &value)?,
                "--repetitions" => config.repetitions = parse(&flag, &value)?,
                "--sample-every" => config.sample_every = parse(&flag, &value)?,
                "--seed" => config.seed = parse(&flag, &value)?,
                "--field-bytes" => config.field_bytes = parse(&flag, &value)?,
                "--scan-max" => config.scan_max = parse(&flag, &value)?,
                "--zipf-theta" => config.zipf_theta = parse(&flag, &value)?,
                "--distribution" => {
                    config.distribution_override = Some(value.parse()?);
                }
                _ => return Err(format!("unknown option: {flag}")),
            }
        }

        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        if self.records == 0 {
            return Err("--records must be greater than zero".into());
        }
        if self.operations == 0 {
            return Err("--operations must be greater than zero".into());
        }
        if self.threads == 0 {
            return Err("--threads must be greater than zero".into());
        }
        if self.repetitions == 0 {
            return Err("--repetitions must be greater than zero".into());
        }
        if self.sample_every == 0 {
            return Err("--sample-every must be greater than zero".into());
        }
        if self.field_bytes == 0 {
            return Err("--field-bytes must be greater than zero".into());
        }
        if self.scan_max == 0 {
            return Err("--scan-max must be greater than zero".into());
        }
        if !(0.0..1.0).contains(&self.zipf_theta) {
            return Err("--zipf-theta must be in [0, 1)".into());
        }
        Ok(())
    }
}

fn parse<T>(flag: &str, value: &str) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|error| format!("invalid value for {flag}: {error}"))
}

fn print_help() {
    println!(
        "YCSB A-F WorkTable runner\n\n\
         Options:\n\
           --workload A|B|C|D|E|F       Workload mix (default A)\n\
           --records N                   Initial record count (default 100000)\n\
           --operations N                Measured operations (default 1000000)\n\
           --threads N                   Concurrent workers (default 1)\n\
           --repetitions N               Fresh-table repetitions (default 5)\n\
           --sample-every N              Time one in N operations (default 1024)\n\
           --seed N                      Deterministic stream seed (default 42)\n\
           --field-bytes N               Bytes per each of 10 fields (default 100)\n\
           --scan-max N                  Maximum Workload E scan length (default 100)\n\
           --zipf-theta F                Zipf exponent in [0,1) (default 0.99)\n\
           --distribution NAME           uniform|zipfian|latest override\n"
    );
}
