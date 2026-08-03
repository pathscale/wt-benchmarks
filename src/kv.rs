use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::Serialize;

use crate::rng::Rng;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurabilityMode {
    Memory,
    Relaxed,
    Durable,
}

impl Display for DurabilityMode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Memory => "memory",
            Self::Relaxed => "relaxed",
            Self::Durable => "durable",
        })
    }
}

impl FromStr for DurabilityMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "memory" => Ok(Self::Memory),
            "relaxed" => Ok(Self::Relaxed),
            "durable" => Ok(Self::Durable),
            _ => Err(format!("unknown durability mode: {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionScope {
    PerOperation,
    Batch,
}

impl Display for TransactionScope {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PerOperation => "per-operation",
            Self::Batch => "batch",
        })
    }
}

impl FromStr for TransactionScope {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "per-operation" | "operation" => Ok(Self::PerOperation),
            "batch" | "reused" => Ok(Self::Batch),
            _ => Err(format!("unknown transaction scope: {value}")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct KvConfig {
    pub rows: u64,
    pub operations: u64,
    pub scan_operations: u64,
    pub scan_length: u64,
    pub repetitions: usize,
    pub payload_bytes: usize,
    pub seed: u64,
    pub durability: DurabilityMode,
    pub transaction_scope: TransactionScope,
}

impl Default for KvConfig {
    fn default() -> Self {
        Self {
            rows: 100_000,
            operations: 100_000,
            scan_operations: 1_000,
            scan_length: 100,
            repetitions: 5,
            payload_bytes: 64,
            seed: 42,
            durability: DurabilityMode::Relaxed,
            transaction_scope: TransactionScope::PerOperation,
        }
    }
}

impl KvConfig {
    pub fn from_args(engine: &str) -> Result<Self, String> {
        let mut config = Self::default();
        if engine == "worktable" || engine == "sqlite" {
            config.durability = DurabilityMode::Memory;
        }
        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            if flag == "--help" || flag == "-h" {
                println!(
                    "{engine} KV adapter options:\n\
                     --rows N                       loaded rows (default 100000)\n\
                     --operations N                 point reads/updates (default 100000)\n\
                     --scan-operations N            range queries (default 1000)\n\
                     --scan-length N                records per range (default 100)\n\
                     --repetitions N                fresh repetitions (default 5)\n\
                     --payload-bytes N              bytes per value (default 64)\n\
                     --seed N                       deterministic seed (default 42)\n\
                     --durability memory|relaxed|durable\n\
                     --transaction-scope per-operation|batch"
                );
                std::process::exit(0);
            }
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--rows" => config.rows = parse(&flag, &value)?,
                "--operations" => config.operations = parse(&flag, &value)?,
                "--scan-operations" => config.scan_operations = parse(&flag, &value)?,
                "--scan-length" => config.scan_length = parse(&flag, &value)?,
                "--repetitions" => config.repetitions = parse(&flag, &value)?,
                "--payload-bytes" => config.payload_bytes = parse(&flag, &value)?,
                "--seed" => config.seed = parse(&flag, &value)?,
                "--durability" => config.durability = value.parse()?,
                "--transaction-scope" => config.transaction_scope = value.parse()?,
                _ => return Err(format!("unknown option: {flag}")),
            }
        }
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        if self.rows == 0
            || self.operations == 0
            || self.scan_operations == 0
            || self.scan_length == 0
            || self.repetitions == 0
            || self.payload_bytes == 0
        {
            return Err("counts, repetitions, and payload size must be non-zero".into());
        }
        if self.scan_length > self.rows {
            return Err("--scan-length cannot exceed --rows".into());
        }
        Ok(())
    }

    pub fn point_keys(&self) -> Vec<u64> {
        keys(self.operations, self.rows, self.seed)
    }

    pub fn scan_starts(&self) -> Vec<u64> {
        keys(
            self.scan_operations,
            self.rows - self.scan_length + 1,
            self.seed ^ 0xa5a5_a5a5,
        )
    }
}

fn parse<T>(flag: &str, value: &str) -> Result<T, String>
where
    T: FromStr,
    T::Err: Display,
{
    value
        .parse()
        .map_err(|error| format!("invalid value for {flag}: {error}"))
}

#[derive(Serialize)]
pub struct KvResult<'a> {
    pub schema_version: u32,
    pub suite: &'static str,
    pub engine: &'a str,
    pub layer: &'a str,
    pub operation: &'a str,
    pub repetition: usize,
    pub rows: u64,
    pub operations: u64,
    pub payload_bytes: usize,
    pub durability: String,
    pub transaction_scope: String,
    pub read_ownership: &'a str,
    pub elapsed_ns: u128,
    pub ops_per_second: f64,
    pub checksum: u64,
    pub target_arch: &'static str,
    pub target_os: &'static str,
}

#[allow(clippy::too_many_arguments)]
pub fn emit(
    config: &KvConfig,
    engine: &str,
    operation: &str,
    repetition: usize,
    operations: u64,
    read_ownership: &str,
    elapsed_ns: u128,
    checksum: u64,
) {
    let result = KvResult {
        schema_version: 1,
        suite: "embedded-kv-layers",
        engine,
        layer: match config.durability {
            DurabilityMode::Memory => "L2",
            DurabilityMode::Relaxed => "L3",
            DurabilityMode::Durable => "L4",
        },
        operation,
        repetition,
        rows: config.rows,
        operations,
        payload_bytes: config.payload_bytes,
        durability: config.durability.to_string(),
        transaction_scope: config.transaction_scope.to_string(),
        read_ownership,
        elapsed_ns,
        ops_per_second: operations as f64 / (elapsed_ns as f64 / 1_000_000_000.0),
        checksum,
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
    };
    println!(
        "{}",
        serde_json::to_string(&result).expect("result must serialize")
    );
}

pub fn value(key: u64, payload_bytes: usize) -> Vec<u8> {
    let mut rng = Rng::new(key ^ 0x517c_c1b7_2722_0a95);
    let mut value = Vec::with_capacity(8 + payload_bytes);
    value.extend_from_slice(&key.to_le_bytes());
    value.extend((0..payload_bytes).map(|_| rng.next_u64() as u8));
    value
}

pub fn value_checksum(value: &[u8]) -> u64 {
    let prefix: [u8; 8] = value[..8].try_into().expect("encoded value prefix");
    u64::from_le_bytes(prefix).wrapping_add(value.len() as u64)
}

pub fn text_value(key: u64, payload_bytes: usize) -> String {
    let mut rng = Rng::new(key ^ 0x517c_c1b7_2722_0a95);
    let bytes: Vec<u8> = (0..payload_bytes)
        .map(|_| b'!' + rng.below((b'~' - b'!') as u64 + 1) as u8)
        .collect();
    String::from_utf8(bytes).expect("generated payload is printable ASCII")
}

pub fn text_checksum(key: u64, value: &str) -> u64 {
    key.wrapping_add(value.len() as u64)
}

fn keys(count: u64, upper: u64, seed: u64) -> Vec<u64> {
    let mut rng = Rng::new(seed);
    (0..count).map(|_| rng.below(upper)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_shape_and_checksum() {
        let encoded = value(42, 64);
        assert_eq!(encoded.len(), 72);
        assert_eq!(value_checksum(&encoded), 114);
    }

    #[test]
    fn text_value_is_ascii_and_sized() {
        let encoded = text_value(42, 64);
        assert_eq!(encoded.len(), 64);
        assert!(encoded.is_ascii());
        assert_eq!(text_checksum(42, &encoded), 106);
    }
}
