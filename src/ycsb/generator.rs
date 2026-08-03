use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::rng::{Rng, mix64};
use crate::ycsb::{Distribution, Workload};

pub const FIELD_COUNT: usize = 10;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum OperationKind {
    Read = 0,
    Update = 1,
    Insert = 2,
    Scan = 3,
    ReadModifyWrite = 4,
}

impl OperationKind {
    pub const ALL: [Self; 5] = [
        Self::Read,
        Self::Update,
        Self::Insert,
        Self::Scan,
        Self::ReadModifyWrite,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Update => "update",
            Self::Insert => "insert",
            Self::Scan => "scan",
            Self::ReadModifyWrite => "read_modify_write",
        }
    }
}

#[derive(Debug)]
pub enum Operation {
    Read {
        key: u64,
    },
    ReadAcknowledged {
        sample: u64,
        distribution: Distribution,
    },
    Update {
        key: u64,
        field: u8,
        value: String,
    },
    Insert {
        key: u64,
        fields: Box<[String; FIELD_COUNT]>,
    },
    Scan {
        start: u64,
        length: u64,
    },
    ReadModifyWrite {
        key: u64,
        field: u8,
        value: String,
    },
}

impl Operation {
    pub fn kind(&self) -> OperationKind {
        match self {
            Self::Read { .. } | Self::ReadAcknowledged { .. } => OperationKind::Read,
            Self::Update { .. } => OperationKind::Update,
            Self::Insert { .. } => OperationKind::Insert,
            Self::Scan { .. } => OperationKind::Scan,
            Self::ReadModifyWrite { .. } => OperationKind::ReadModifyWrite,
        }
    }
}

pub struct GeneratedWorkload {
    pub streams: Vec<Vec<Operation>>,
    pub acknowledged: Option<Arc<AcknowledgedKeyspace>>,
}

pub struct AcknowledgedKeyspace {
    frontier: AtomicU64,
    pending: Mutex<BTreeSet<u64>>,
    zipf: ZipfCdf,
}

impl AcknowledgedKeyspace {
    fn new(initial_records: u64, zipf: ZipfCdf) -> Self {
        assert!(initial_records > 0, "acknowledged keyspace cannot be empty");
        Self {
            frontier: AtomicU64::new(initial_records - 1),
            pending: Mutex::new(BTreeSet::new()),
            zipf,
        }
    }

    pub fn resolve(&self, sample: u64, distribution: Distribution) -> u64 {
        let frontier = self.frontier.load(Ordering::Acquire);
        let key_count = frontier
            .checked_add(1)
            .expect("YCSB acknowledged key count overflowed");
        match distribution {
            Distribution::Uniform => ((sample as u128 * key_count as u128) >> 64) as u64,
            Distribution::Zipfian => {
                let rank = self.zipf.sample_token(sample, key_count as usize) as u64;
                mix64(rank) % key_count
            }
            Distribution::Latest => {
                let rank = self.zipf.sample_token(sample, key_count as usize) as u64;
                frontier - rank
            }
        }
    }

    pub fn acknowledge(&self, key: u64) {
        if key <= self.frontier.load(Ordering::Acquire) {
            return;
        }

        let mut pending = self
            .pending
            .lock()
            .expect("YCSB acknowledged-key mutex poisoned");
        let mut frontier = self.frontier.load(Ordering::Relaxed);
        if key <= frontier {
            return;
        }
        pending.insert(key);
        while frontier < u64::MAX {
            let next = frontier + 1;
            if !pending.remove(&next) {
                break;
            }
            frontier = next;
        }
        self.frontier.store(frontier, Ordering::Release);
    }

    #[cfg(test)]
    fn frontier(&self) -> u64 {
        self.frontier.load(Ordering::Acquire)
    }
}

pub fn generate_streams(config: &Config) -> GeneratedWorkload {
    let mut zipf = ZipfCdf::new(config.records as usize, config.zipf_theta);
    let distribution = config
        .distribution_override
        .unwrap_or_else(|| config.workload.default_distribution());
    let mut rng = Rng::new(config.seed);
    let mut key_count = config.records;
    let mut streams: Vec<Vec<Operation>> = (0..config.threads)
        .map(|_| Vec::with_capacity(config.operations as usize / config.threads + 1))
        .collect();

    for operation_index in 0..config.operations {
        zipf.ensure(key_count as usize);
        let choice = rng.below(10_000);
        let operation = match config.workload {
            Workload::A if choice < 5_000 => read(&mut rng, &zipf, distribution, key_count),
            Workload::A => update(&mut rng, &zipf, distribution, key_count, config.field_bytes),
            Workload::B if choice < 9_500 => read(&mut rng, &zipf, distribution, key_count),
            Workload::B => update(&mut rng, &zipf, distribution, key_count, config.field_bytes),
            Workload::C => read(&mut rng, &zipf, distribution, key_count),
            Workload::D if choice < 9_500 => Operation::ReadAcknowledged {
                sample: rng.next_u64(),
                distribution,
            },
            Workload::D => {
                let key = key_count;
                key_count = key_count
                    .checked_add(1)
                    .expect("YCSB generated key count overflowed");
                Operation::Insert {
                    key,
                    fields: Box::new(make_fields(key ^ config.seed, config.field_bytes)),
                }
            }
            Workload::E if choice < 9_500 => Operation::Scan {
                start: sample_key(&mut rng, &zipf, distribution, key_count),
                length: 1 + rng.below(config.scan_max),
            },
            Workload::E => {
                let key = key_count;
                key_count = key_count
                    .checked_add(1)
                    .expect("YCSB generated key count overflowed");
                Operation::Insert {
                    key,
                    fields: Box::new(make_fields(key ^ config.seed, config.field_bytes)),
                }
            }
            Workload::F if choice < 5_000 => read(&mut rng, &zipf, distribution, key_count),
            Workload::F => Operation::ReadModifyWrite {
                key: sample_key(&mut rng, &zipf, distribution, key_count),
                field: rng.below(FIELD_COUNT as u64) as u8,
                value: make_value(rng.next_u64(), config.field_bytes),
            },
        };

        streams[operation_index as usize % config.threads].push(operation);
    }

    zipf.ensure(key_count as usize);
    let acknowledged = (config.workload == Workload::D)
        .then(|| Arc::new(AcknowledgedKeyspace::new(config.records, zipf)));
    GeneratedWorkload {
        streams,
        acknowledged,
    }
}

fn read(rng: &mut Rng, zipf: &ZipfCdf, distribution: Distribution, key_count: u64) -> Operation {
    Operation::Read {
        key: sample_key(rng, zipf, distribution, key_count),
    }
}

fn update(
    rng: &mut Rng,
    zipf: &ZipfCdf,
    distribution: Distribution,
    key_count: u64,
    field_bytes: usize,
) -> Operation {
    Operation::Update {
        key: sample_key(rng, zipf, distribution, key_count),
        field: rng.below(FIELD_COUNT as u64) as u8,
        value: make_value(rng.next_u64(), field_bytes),
    }
}

fn sample_key(rng: &mut Rng, zipf: &ZipfCdf, distribution: Distribution, key_count: u64) -> u64 {
    match distribution {
        Distribution::Uniform => rng.below(key_count),
        Distribution::Zipfian => mix64(zipf.sample(rng, key_count as usize) as u64) % key_count,
        Distribution::Latest => key_count - 1 - zipf.sample(rng, key_count as usize) as u64,
    }
}

pub fn make_fields(seed: u64, field_bytes: usize) -> [String; FIELD_COUNT] {
    std::array::from_fn(|index| {
        make_value(seed.wrapping_add(index as u64 * 0x9e37_79b9), field_bytes)
    })
}

fn make_value(seed: u64, length: usize) -> String {
    let mut rng = Rng::new(seed);
    let bytes: Vec<u8> = (0..length)
        .map(|_| b'!' + rng.below((b'~' - b'!') as u64 + 1) as u8)
        .collect();
    String::from_utf8(bytes).expect("ASCII payload")
}

struct ZipfCdf {
    cumulative: Vec<f64>,
    theta: f64,
}

impl ZipfCdf {
    fn new(items: usize, theta: f64) -> Self {
        let mut result = Self {
            cumulative: Vec::with_capacity(items),
            theta,
        };
        result.ensure(items);
        result
    }

    fn ensure(&mut self, items: usize) {
        let mut sum = self.cumulative.last().copied().unwrap_or(0.0);
        for rank in self.cumulative.len() + 1..=items {
            sum += 1.0 / (rank as f64).powf(self.theta);
            self.cumulative.push(sum);
        }
    }

    fn sample(&self, rng: &mut Rng, bound: usize) -> usize {
        self.sample_token(rng.next_u64(), bound)
    }

    fn sample_token(&self, sample: u64, bound: usize) -> usize {
        debug_assert!(bound > 0 && bound <= self.cumulative.len());
        const SCALE: f64 = 1.0 / ((1_u64 << 53) as f64);
        let unit = ((sample >> 11) as f64) * SCALE;
        let target = unit * self.cumulative[bound - 1];
        self.cumulative[..bound].partition_point(|value| *value < target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(workload: Workload) -> Config {
        Config {
            workload,
            records: 1_000,
            operations: 100_000,
            threads: 4,
            repetitions: 1,
            ..Config::default()
        }
    }

    fn counts(workload: Workload) -> [u64; 5] {
        let mut result = [0; 5];
        for operation in generate_streams(&config(workload))
            .streams
            .into_iter()
            .flatten()
        {
            result[operation.kind() as usize] += 1;
        }
        result
    }

    #[test]
    fn canonical_operation_mixes() {
        let a = counts(Workload::A);
        assert!((49_000..=51_000).contains(&a[OperationKind::Read as usize]));
        assert!((49_000..=51_000).contains(&a[OperationKind::Update as usize]));

        let b = counts(Workload::B);
        assert!((94_000..=96_000).contains(&b[OperationKind::Read as usize]));
        assert!((4_000..=6_000).contains(&b[OperationKind::Update as usize]));

        let c = counts(Workload::C);
        assert_eq!(c[OperationKind::Read as usize], 100_000);

        let d = counts(Workload::D);
        assert!((4_000..=6_000).contains(&d[OperationKind::Insert as usize]));

        let e = counts(Workload::E);
        assert!((94_000..=96_000).contains(&e[OperationKind::Scan as usize]));

        let f = counts(Workload::F);
        assert!((49_000..=51_000).contains(&f[OperationKind::ReadModifyWrite as usize]));
    }

    #[test]
    fn payload_has_requested_shape() {
        let fields = make_fields(7, 100);
        assert!(fields.iter().all(|field| field.len() == 100));
        assert_ne!(fields[0], fields[1]);
    }

    #[test]
    fn acknowledged_frontier_advances_only_across_contiguous_inserts() {
        let keyspace = AcknowledgedKeyspace::new(10, ZipfCdf::new(20, 0.99));
        keyspace.acknowledge(12);
        assert_eq!(keyspace.frontier(), 9);
        keyspace.acknowledge(10);
        assert_eq!(keyspace.frontier(), 10);
        keyspace.acknowledge(11);
        assert_eq!(keyspace.frontier(), 12);
    }

    #[test]
    fn workload_d_reads_resolve_only_to_acknowledged_keys() {
        let generated = generate_streams(&config(Workload::D));
        let acknowledged = generated.acknowledged.expect("workload D keyspace");
        for operation in generated.streams.into_iter().flatten() {
            match operation {
                Operation::ReadAcknowledged {
                    sample,
                    distribution,
                } => {
                    let key = acknowledged.resolve(sample, distribution);
                    assert!(key <= acknowledged.frontier());
                }
                Operation::Insert { key, .. } => acknowledged.acknowledge(key),
                _ => panic!("unexpected Workload D operation"),
            }
        }
    }
}
