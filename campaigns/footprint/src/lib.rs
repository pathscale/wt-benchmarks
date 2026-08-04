use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const FIXED_LOGICAL_BYTES_PER_ROW: u64 = 8 + 8 + 8 + 8;

#[derive(Clone, Debug)]
pub struct StorageConfig {
    pub path: PathBuf,
    pub rows: u64,
    pub payload_bytes: usize,
    pub drain_every: u64,
}

impl StorageConfig {
    pub fn parse() -> Result<Self, String> {
        let mut path = None;
        let mut rows = 10_000_u64;
        let mut payload_bytes = 64_usize;
        let mut drain_every = 5_000_u64;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--path" => path = Some(PathBuf::from(next_value(&mut args, "--path")?)),
                "--rows" => rows = parse_value(&mut args, "--rows")?,
                "--payload-bytes" => payload_bytes = parse_value(&mut args, "--payload-bytes")?,
                "--drain-every" => drain_every = parse_value(&mut args, "--drain-every")?,
                "--help" | "-h" => return Err(usage().to_string()),
                _ => return Err(format!("unknown argument {arg}\n{}", usage())),
            }
        }
        let path = path.ok_or_else(|| format!("--path is required\n{}", usage()))?;
        if rows == 0 {
            return Err("--rows must be greater than zero".to_string());
        }
        if drain_every == 0 {
            return Err("--drain-every must be greater than zero".to_string());
        }
        if path.exists() {
            return Err(format!(
                "refusing to overwrite existing benchmark path {}",
                path.display()
            ));
        }
        Ok(Self {
            path,
            rows,
            payload_bytes,
            drain_every,
        })
    }

    pub fn logical_row_bytes(&self) -> u64 {
        FIXED_LOGICAL_BYTES_PER_ROW + self.payload_bytes as u64
    }

    pub fn logical_dataset_bytes(&self, live_rows: u64) -> u64 {
        self.logical_row_bytes() * live_rows
    }
}

fn usage() -> &'static str {
    "usage: <binary> --path PATH [--rows N] [--payload-bytes N] [--drain-every N]"
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} needs a value"))
}

fn parse_value<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<T, String> {
    let raw = next_value(args, flag)?;
    raw.parse()
        .map_err(|_| format!("invalid value for {flag}: {raw}"))
}

pub fn payload(id: u64, bytes: usize) -> String {
    let marker = b'a' + (id % 26) as u8;
    String::from_utf8(vec![marker; bytes]).expect("ASCII payload")
}

pub fn encoded_row(id: u64, payload_bytes: usize) -> Vec<u8> {
    encoded_row_with_payload_seed(id, id, payload_bytes)
}

pub fn encoded_row_with_payload_seed(id: u64, payload_seed: u64, payload_bytes: usize) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(FIXED_LOGICAL_BYTES_PER_ROW as usize + payload_bytes);
    encoded.extend_from_slice(&id.to_le_bytes());
    encoded.extend_from_slice(&(id % 10_000).to_le_bytes());
    encoded.extend_from_slice(&id.wrapping_mul(17).to_le_bytes());
    encoded.extend_from_slice(&(id as f64 / 100.0).to_bits().to_le_bytes());
    encoded.extend(std::iter::repeat_n(
        b'a' + (payload_seed % 26) as u8,
        payload_bytes,
    ));
    encoded
}

pub fn encoded_checksum(encoded: &[u8]) -> Option<u64> {
    if encoded.len() < FIXED_LOGICAL_BYTES_PER_ROW as usize {
        return None;
    }
    let id = u64::from_le_bytes(encoded[0..8].try_into().ok()?);
    let account_id = u64::from_le_bytes(encoded[8..16].try_into().ok()?);
    let sequence = u64::from_le_bytes(encoded[16..24].try_into().ok()?);
    let score_bits = u64::from_le_bytes(encoded[24..32].try_into().ok()?);
    Some(
        id ^ account_id
            ^ sequence
            ^ score_bits
            ^ (encoded.len() - FIXED_LOGICAL_BYTES_PER_ROW as usize) as u64,
    )
}

pub fn live_rows_after_churn(rows: u64) -> u64 {
    rows - rows.div_ceil(4)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FileStats {
    pub files: u64,
    pub logical_bytes: u64,
    pub allocated_bytes: Option<u64>,
}

pub fn file_stats(path: &Path) -> io::Result<FileStats> {
    let mut stats = FileStats {
        allocated_bytes: allocated_bytes_supported().then_some(0),
        ..FileStats::default()
    };
    visit(path, &mut stats)?;
    Ok(stats)
}

fn visit(path: &Path, stats: &mut FileStats) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_file() {
        stats.files += 1;
        stats.logical_bytes += metadata.len();
        if let Some(total) = &mut stats.allocated_bytes {
            *total += file_allocated_bytes(&metadata);
        }
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            visit(&entry?.path(), stats)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn allocated_bytes_supported() -> bool {
    true
}

#[cfg(not(unix))]
fn allocated_bytes_supported() -> bool {
    false
}

#[cfg(unix)]
fn file_allocated_bytes(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.blocks() * 512
}

#[cfg(not(unix))]
fn file_allocated_bytes(_metadata: &fs::Metadata) -> u64 {
    0
}

pub fn emit_storage(
    engine: &str,
    phase: &str,
    config: &StorageConfig,
    live_rows: u64,
) -> io::Result<()> {
    let stats = file_stats(&config.path)?;
    let allocated = stats
        .allocated_bytes
        .map(|bytes| bytes.to_string())
        .unwrap_or_else(|| "null".to_string());
    println!(
        "{{\"benchmark\":\"storage-footprint\",\"engine\":\"{engine}\",\"phase\":\"{phase}\",\"rows\":{live_rows},\"payload_bytes\":{},\"logical_dataset_bytes\":{},\"file_count\":{},\"file_logical_bytes\":{},\"file_allocated_bytes\":{allocated}}}",
        config.payload_bytes,
        config.logical_dataset_bytes(live_rows),
        stats.files,
        stats.logical_bytes,
    );
    Ok(())
}
