use std::collections::BTreeMap;
use std::hint::black_box;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use tokio::sync::Barrier;
use worktable::prelude::*;
use worktable::worktable;
use wt_benchmarks::result::LatencySummary;
use wt_benchmarks::rng::Rng;

type BitRest = [u8; 9];
type HexValues = [u8; 10];
type Byte2Values = [u16; 10];

worktable!(
    name: TatpSubscriber,
    columns: {
        s_id: u64 primary_key,
        sub_nbr: String,
        bit_1: u8,
        bit_rest: BitRest,
        hex_values: HexValues,
        byte2_values: Byte2Values,
        msc_location: u32,
        vlr_location: u32,
    },
    indexes: {
        sub_nbr_idx: sub_nbr unique,
    },
    queries: {
        update: {
            Bit1(bit_1) by s_id,
            VlrLocation(vlr_location) by s_id,
        }
    }
);

worktable!(
    name: TatpAccessInfo,
    columns: {
        id: u128 primary_key,
        s_id: u64,
        ai_type: u8,
        data1: u16,
        data2: u16,
        data3: String,
        data4: String,
    }
);

worktable!(
    name: TatpSpecialFacility,
    columns: {
        id: u128 primary_key,
        s_id: u64,
        sf_type: u8,
        is_active: u8,
        error_control: u16,
        data_a: u16,
        data_b: String,
    }
);

worktable!(
    name: TatpCallForwarding,
    columns: {
        id: u128 primary_key,
        s_id: u64,
        sf_type: u8,
        start_time: u8,
        facility: u128,
        end_time: u8,
        numberx: String,
    },
    indexes: {
        facility_idx: facility,
    }
);

#[derive(Clone, Debug)]
struct Config {
    subscribers: u64,
    operations: u64,
    threads: usize,
    repetitions: usize,
    sample_every: u64,
    seed: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            subscribers: 100_000,
            operations: 1_000_000,
            threads: 1,
            repetitions: 5,
            sample_every: 1_000,
            seed: 42,
        }
    }
}

impl Config {
    fn from_args() -> Result<Self, String> {
        let mut config = Self::default();
        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            if flag == "--help" || flag == "-h" {
                println!(
                    "tatp-worktable options:\n\
                     --subscribers N       subscriber rows (default 100000)\n\
                     --operations N        transaction attempts (default 1000000)\n\
                     --threads N           concurrent workers (default 1)\n\
                     --repetitions N       fresh-table repetitions (default 5)\n\
                     --sample-every N      sample one in N transaction latencies (default 1000)\n\
                     --seed N              deterministic seed (default 42)"
                );
                std::process::exit(0);
            }
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--subscribers" => config.subscribers = parse(&flag, &value)?,
                "--operations" => config.operations = parse(&flag, &value)?,
                "--threads" => config.threads = parse(&flag, &value)?,
                "--repetitions" => config.repetitions = parse(&flag, &value)?,
                "--sample-every" => config.sample_every = parse(&flag, &value)?,
                "--seed" => config.seed = parse(&flag, &value)?,
                _ => return Err(format!("unknown option: {flag}")),
            }
        }
        if config.subscribers == 0
            || config.operations == 0
            || config.threads == 0
            || config.repetitions == 0
            || config.sample_every == 0
        {
            return Err(
                "counts, threads, repetitions, and sampling interval must be non-zero".into(),
            );
        }
        if config.subscribers > 999_999_999_999_999 {
            return Err("--subscribers must fit TATP's 15-digit subscriber number".into());
        }
        Ok(config)
    }
}

fn parse<T>(flag: &str, value: &str) -> Result<T, String>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|error| format!("invalid value for {flag}: {error}"))
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Kind {
    DeleteCallForwarding,
    GetAccessData,
    GetNewDestination,
    GetSubscriberData,
    InsertCallForwarding,
    UpdateLocation,
    UpdateSubscriberData,
}

impl Kind {
    const ALL: [Self; 7] = [
        Self::DeleteCallForwarding,
        Self::GetAccessData,
        Self::GetNewDestination,
        Self::GetSubscriberData,
        Self::InsertCallForwarding,
        Self::UpdateLocation,
        Self::UpdateSubscriberData,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::DeleteCallForwarding => "delete_call_forwarding",
            Self::GetAccessData => "get_access_data",
            Self::GetNewDestination => "get_new_destination",
            Self::GetSubscriberData => "get_subscriber_data",
            Self::InsertCallForwarding => "insert_call_forwarding",
            Self::UpdateLocation => "update_location",
            Self::UpdateSubscriberData => "update_subscriber_data",
        }
    }
}

#[derive(Debug)]
enum Operation {
    DeleteCallForwarding {
        sub_nbr: String,
        sf_type: u8,
        start_time: u8,
    },
    GetAccessData {
        s_id: u64,
        ai_type: u8,
    },
    GetNewDestination {
        s_id: u64,
        sf_type: u8,
        start_time: u8,
        end_time: u8,
    },
    GetSubscriberData {
        s_id: u64,
    },
    InsertCallForwarding {
        sub_nbr: String,
        sf_type: u8,
        start_time: u8,
        end_time: u8,
        numberx: String,
    },
    UpdateLocation {
        sub_nbr: String,
        vlr_location: u32,
    },
    UpdateSubscriberData {
        s_id: u64,
        bit_1: u8,
        data_a: u16,
        sf_type: u8,
    },
}

impl Operation {
    fn kind(&self) -> Kind {
        match self {
            Self::DeleteCallForwarding { .. } => Kind::DeleteCallForwarding,
            Self::GetAccessData { .. } => Kind::GetAccessData,
            Self::GetNewDestination { .. } => Kind::GetNewDestination,
            Self::GetSubscriberData { .. } => Kind::GetSubscriberData,
            Self::InsertCallForwarding { .. } => Kind::InsertCallForwarding,
            Self::UpdateLocation { .. } => Kind::UpdateLocation,
            Self::UpdateSubscriberData { .. } => Kind::UpdateSubscriberData,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Completed,
    ExpectedAbort,
    UnexpectedError,
}

#[derive(Default)]
struct Tables {
    subscribers: TatpSubscriberWorkTable,
    access_info: TatpAccessInfoWorkTable,
    special_facilities: TatpSpecialFacilityWorkTable,
    call_forwarding: TatpCallForwardingWorkTable,
}

#[derive(Default)]
struct LoadCounts {
    subscribers: u64,
    access_info: u64,
    special_facilities: u64,
    call_forwarding: u64,
}

#[derive(Default)]
struct WorkerResult {
    completed: u64,
    expected_aborts: u64,
    unexpected_errors: u64,
    counts: [u64; 7],
    latency: [Vec<u64>; 7],
}

#[derive(Serialize)]
struct ResultRow {
    schema_version: u32,
    suite: &'static str,
    profile: &'static str,
    engine: &'static str,
    repetition: usize,
    subscribers: u64,
    operations_requested: u64,
    transactions_completed: u64,
    expected_aborts: u64,
    unexpected_errors: u64,
    threads: usize,
    sample_every: u64,
    seed: u64,
    subscriber_rows: u64,
    access_info_rows: u64,
    special_facility_rows: u64,
    call_forwarding_rows_initial: u64,
    load_elapsed_ns: u128,
    elapsed_ns: u128,
    attempts_per_second: f64,
    completed_per_second: f64,
    transaction_counts: BTreeMap<&'static str, u64>,
    latency: BTreeMap<&'static str, LatencySummary>,
    transaction_semantics: &'static str,
    expected_miss_semantics: &'static str,
    read_ownership: &'static str,
    feature_versioned_row_publication: bool,
    target_arch: &'static str,
    target_os: &'static str,
}

#[tokio::main]
async fn main() {
    let config = Config::from_args().unwrap_or_else(|error| {
        eprintln!("error: {error}\nrun with --help for usage");
        std::process::exit(2);
    });
    if config.threads > 1 && !cfg!(feature = "versioned-row-publication") {
        eprintln!(
            "error: concurrent TATP mixes reads with page mutation; rerun with \
             --features versioned-row-publication or use --threads 1"
        );
        std::process::exit(2);
    }

    for repetition in 1..=config.repetitions {
        let result = run_repetition(&config, repetition).await;
        println!(
            "{}",
            serde_json::to_string(&result).expect("result must serialize")
        );
    }
}

async fn run_repetition(config: &Config, repetition: usize) -> ResultRow {
    let tables = Arc::new(Tables::default());
    let load_started = Instant::now();
    let load_counts = load(&tables, config);
    let load_elapsed_ns = load_started.elapsed().as_nanos();
    let streams = generate_streams(config);

    let ready = Arc::new(Barrier::new(config.threads + 1));
    let start = Arc::new(Barrier::new(config.threads + 1));
    let mut handles = Vec::with_capacity(config.threads);
    for stream in streams {
        let tables = Arc::clone(&tables);
        let ready = Arc::clone(&ready);
        let start = Arc::clone(&start);
        let sample_every = config.sample_every;
        handles.push(tokio::spawn(async move {
            ready.wait().await;
            start.wait().await;
            run_worker(tables, stream, sample_every).await
        }));
    }

    ready.wait().await;
    let measured_started = Instant::now();
    start.wait().await;
    let mut combined = WorkerResult::default();
    for handle in handles {
        let mut worker = handle.await.expect("TATP worker panicked");
        combined.completed += worker.completed;
        combined.expected_aborts += worker.expected_aborts;
        combined.unexpected_errors += worker.unexpected_errors;
        for kind in Kind::ALL {
            combined.counts[kind as usize] += worker.counts[kind as usize];
            combined.latency[kind as usize].append(&mut worker.latency[kind as usize]);
        }
    }
    let elapsed_ns = measured_started.elapsed().as_nanos();

    let mut transaction_counts = BTreeMap::new();
    let mut latency = BTreeMap::new();
    for kind in Kind::ALL {
        transaction_counts.insert(kind.as_str(), combined.counts[kind as usize]);
        latency.insert(
            kind.as_str(),
            LatencySummary::from_samples(std::mem::take(&mut combined.latency[kind as usize])),
        );
    }
    let seconds = elapsed_ns as f64 / 1_000_000_000.0;
    ResultRow {
        schema_version: 1,
        suite: "tatp",
        profile: "benchbase-canonical-mix",
        engine: "worktable",
        repetition,
        subscribers: config.subscribers,
        operations_requested: config.operations,
        transactions_completed: combined.completed,
        expected_aborts: combined.expected_aborts,
        unexpected_errors: combined.unexpected_errors,
        threads: config.threads,
        sample_every: config.sample_every,
        seed: config.seed,
        subscriber_rows: load_counts.subscribers,
        access_info_rows: load_counts.access_info,
        special_facility_rows: load_counts.special_facilities,
        call_forwarding_rows_initial: load_counts.call_forwarding,
        load_elapsed_ns,
        elapsed_ns,
        attempts_per_second: config.operations as f64 / seconds,
        completed_per_second: combined.completed as f64 / seconds,
        transaction_counts,
        latency,
        transaction_semantics: "application procedures; no automatic cross-table atomicity",
        expected_miss_semantics: "duplicate insert, missing facility, and missing delete count as expected aborts",
        read_ownership: "materialized-owned-row",
        feature_versioned_row_publication: cfg!(feature = "versioned-row-publication"),
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
    }
}

fn load(tables: &Tables, config: &Config) -> LoadCounts {
    let mut rng = Rng::new(config.seed ^ 0x7461_7470);
    let mut counts = LoadCounts::default();
    for s_id in 1..=config.subscribers {
        tables
            .subscribers
            .insert(TatpSubscriberRow {
                s_id,
                sub_nbr: subscriber_number(s_id),
                bit_1: rng.below(2) as u8,
                bit_rest: std::array::from_fn(|_| rng.below(2) as u8),
                hex_values: std::array::from_fn(|_| rng.below(16) as u8),
                byte2_values: std::array::from_fn(|_| rng.below(256) as u16),
                msc_location: rng.next_u64() as u32,
                vlr_location: rng.next_u64() as u32,
            })
            .expect("subscriber keys and numbers must be unique");
        counts.subscribers += 1;

        for ai_type in random_subset(&mut rng, &[1, 2, 3, 4], 1, 4) {
            tables
                .access_info
                .insert(TatpAccessInfoRow {
                    id: facility_key(s_id, ai_type),
                    s_id,
                    ai_type,
                    data1: rng.below(256) as u16,
                    data2: rng.below(256) as u16,
                    data3: alpha_string(&mut rng, 3),
                    data4: alpha_string(&mut rng, 5),
                })
                .expect("access-info primary keys must be unique");
            counts.access_info += 1;
        }

        for sf_type in random_subset(&mut rng, &[1, 2, 3, 4], 1, 4) {
            tables
                .special_facilities
                .insert(TatpSpecialFacilityRow {
                    id: facility_key(s_id, sf_type),
                    s_id,
                    sf_type,
                    is_active: u8::from(rng.below(100) < 85),
                    error_control: rng.below(256) as u16,
                    data_a: rng.below(256) as u16,
                    data_b: alpha_string(&mut rng, 5),
                })
                .expect("special-facility primary keys must be unique");
            counts.special_facilities += 1;

            for start_time in random_subset(&mut rng, &[0, 8, 16], 0, 3) {
                tables
                    .call_forwarding
                    .insert(TatpCallForwardingRow {
                        id: call_forwarding_key(s_id, sf_type, start_time),
                        s_id,
                        sf_type,
                        start_time,
                        facility: facility_key(s_id, sf_type),
                        end_time: start_time + 1 + rng.below(8) as u8,
                        numberx: numeric_string(&mut rng, 15),
                    })
                    .expect("call-forwarding primary keys must be unique");
                counts.call_forwarding += 1;
            }
        }
    }
    counts
}

fn generate_streams(config: &Config) -> Vec<Vec<Operation>> {
    let mut streams = (0..config.threads)
        .map(|_| Vec::with_capacity(config.operations as usize / config.threads + 1))
        .collect::<Vec<_>>();
    let mut rng = Rng::new(config.seed ^ 0x6d69_7873);
    for operation_index in 0..config.operations {
        let choice = rng.below(100);
        let kind = kind_for_choice(choice);
        let s_id = 1 + rng.below(config.subscribers);
        let sf_type = (1 + rng.below(4)) as u8;
        let start_time = (8 * rng.below(3)) as u8;
        let operation = match kind {
            Kind::DeleteCallForwarding => Operation::DeleteCallForwarding {
                sub_nbr: subscriber_number(s_id),
                sf_type,
                start_time,
            },
            Kind::GetAccessData => Operation::GetAccessData {
                s_id,
                ai_type: (1 + rng.below(4)) as u8,
            },
            Kind::GetNewDestination => Operation::GetNewDestination {
                s_id,
                sf_type,
                start_time,
                end_time: (1 + rng.below(24)) as u8,
            },
            Kind::GetSubscriberData => Operation::GetSubscriberData { s_id },
            Kind::InsertCallForwarding => Operation::InsertCallForwarding {
                sub_nbr: subscriber_number(s_id),
                sf_type,
                start_time,
                end_time: (1 + rng.below(24)) as u8,
                numberx: subscriber_number(s_id),
            },
            Kind::UpdateLocation => Operation::UpdateLocation {
                sub_nbr: subscriber_number(s_id),
                vlr_location: rng.next_u64() as u32,
            },
            Kind::UpdateSubscriberData => Operation::UpdateSubscriberData {
                s_id,
                bit_1: rng.below(2) as u8,
                data_a: rng.below(256) as u16,
                sf_type,
            },
        };
        streams[operation_index as usize % config.threads].push(operation);
    }
    streams
}

fn kind_for_choice(choice: u64) -> Kind {
    match choice {
        0..=1 => Kind::DeleteCallForwarding,
        2..=36 => Kind::GetAccessData,
        37..=46 => Kind::GetNewDestination,
        47..=81 => Kind::GetSubscriberData,
        82..=83 => Kind::InsertCallForwarding,
        84..=97 => Kind::UpdateLocation,
        _ => Kind::UpdateSubscriberData,
    }
}

async fn run_worker(
    tables: Arc<Tables>,
    stream: Vec<Operation>,
    sample_every: u64,
) -> WorkerResult {
    let mut result = WorkerResult::default();
    let sample_capacity = stream.len() / sample_every as usize + 1;
    for samples in &mut result.latency {
        samples.reserve(sample_capacity);
    }
    for (index, operation) in stream.into_iter().enumerate() {
        let kind = operation.kind();
        let sampled = (index as u64).is_multiple_of(sample_every);
        let started = sampled.then(Instant::now);
        let outcome = execute(&tables, operation).await;
        if let Some(started) = started {
            result.latency[kind as usize]
                .push(started.elapsed().as_nanos().min(u64::MAX as u128) as u64);
        }
        result.counts[kind as usize] += 1;
        match outcome {
            Outcome::Completed => result.completed += 1,
            Outcome::ExpectedAbort => result.expected_aborts += 1,
            Outcome::UnexpectedError => result.unexpected_errors += 1,
        }
    }
    result
}

async fn execute(tables: &Tables, operation: Operation) -> Outcome {
    match operation {
        Operation::GetSubscriberData { s_id } => {
            if black_box(tables.subscribers.select(s_id)).is_some() {
                Outcome::Completed
            } else {
                Outcome::UnexpectedError
            }
        }
        Operation::GetAccessData { s_id, ai_type } => {
            black_box(tables.access_info.select(facility_key(s_id, ai_type)));
            Outcome::Completed
        }
        Operation::GetNewDestination {
            s_id,
            sf_type,
            start_time,
            end_time,
        } => {
            let Some(facility) = tables
                .special_facilities
                .select(facility_key(s_id, sf_type))
            else {
                return Outcome::Completed;
            };
            if facility.is_active == 1 {
                match tables
                    .call_forwarding
                    .select_by_facility(facility_key(s_id, sf_type))
                    .execute()
                {
                    Ok(rows) => {
                        black_box(
                            rows.into_iter()
                                .filter(|row| {
                                    row.start_time <= start_time && row.end_time > end_time
                                })
                                .count(),
                        );
                    }
                    Err(_) => return Outcome::UnexpectedError,
                }
            }
            Outcome::Completed
        }
        Operation::UpdateLocation {
            sub_nbr,
            vlr_location,
        } => {
            let Some(subscriber) = tables.subscribers.select_by_sub_nbr(sub_nbr) else {
                return Outcome::UnexpectedError;
            };
            match tables
                .subscribers
                .update_vlr_location(VlrLocationQuery { vlr_location }, subscriber.s_id)
                .await
            {
                Ok(_) => Outcome::Completed,
                Err(_) => Outcome::UnexpectedError,
            }
        }
        Operation::UpdateSubscriberData {
            s_id,
            bit_1,
            data_a,
            sf_type,
        } => {
            let Some(mut facility) = tables
                .special_facilities
                .select(facility_key(s_id, sf_type))
            else {
                return Outcome::ExpectedAbort;
            };
            if tables
                .subscribers
                .update_bit_1(Bit1Query { bit_1 }, s_id)
                .await
                .is_err()
            {
                return Outcome::UnexpectedError;
            }
            facility.data_a = data_a;
            match tables.special_facilities.update(facility).await {
                Ok(_) => Outcome::Completed,
                Err(_) => Outcome::UnexpectedError,
            }
        }
        Operation::InsertCallForwarding {
            sub_nbr,
            sf_type,
            start_time,
            end_time,
            numberx,
        } => {
            let Some(subscriber) = tables.subscribers.select_by_sub_nbr(sub_nbr) else {
                return Outcome::UnexpectedError;
            };
            let s_id = subscriber.s_id;
            if tables
                .special_facilities
                .select(facility_key(s_id, sf_type))
                .is_none()
            {
                return Outcome::ExpectedAbort;
            }
            match tables.call_forwarding.insert(TatpCallForwardingRow {
                id: call_forwarding_key(s_id, sf_type, start_time),
                s_id,
                sf_type,
                start_time,
                facility: facility_key(s_id, sf_type),
                end_time,
                numberx,
            }) {
                Ok(_) => Outcome::Completed,
                Err(WorkTableError::AlreadyExists(_) | WorkTableError::PrimaryAlreadyExists) => {
                    Outcome::ExpectedAbort
                }
                Err(_) => Outcome::UnexpectedError,
            }
        }
        Operation::DeleteCallForwarding {
            sub_nbr,
            sf_type,
            start_time,
        } => {
            let Some(subscriber) = tables.subscribers.select_by_sub_nbr(sub_nbr) else {
                return Outcome::UnexpectedError;
            };
            match tables
                .call_forwarding
                .delete(call_forwarding_key(subscriber.s_id, sf_type, start_time))
                .await
            {
                Ok(_) => Outcome::Completed,
                Err(WorkTableError::NotFound) => Outcome::ExpectedAbort,
                Err(_) => Outcome::UnexpectedError,
            }
        }
    }
}

fn facility_key(s_id: u64, sf_type: u8) -> u128 {
    ((s_id as u128) << 8) | sf_type as u128
}

fn call_forwarding_key(s_id: u64, sf_type: u8, start_time: u8) -> u128 {
    (facility_key(s_id, sf_type) << 8) | start_time as u128
}

fn subscriber_number(s_id: u64) -> String {
    format!("{s_id:015}")
}

fn alpha_string(rng: &mut Rng, length: usize) -> String {
    (0..length)
        .map(|_| (b'A' + rng.below(26) as u8) as char)
        .collect()
}

fn numeric_string(rng: &mut Rng, length: usize) -> String {
    (0..length)
        .map(|_| (b'0' + rng.below(10) as u8) as char)
        .collect()
}

fn random_subset(rng: &mut Rng, source: &[u8], minimum: usize, maximum: usize) -> Vec<u8> {
    let mut values = source.to_vec();
    let length = minimum + rng.below((maximum - minimum + 1) as u64) as usize;
    for index in 0..length {
        let swap = index + rng.below((values.len() - index) as u64) as usize;
        values.swap(index, swap);
    }
    values.truncate(length);
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_mix_weights_sum_to_one_hundred() {
        let mut counts = [0_u64; 7];
        for choice in 0..100 {
            counts[kind_for_choice(choice) as usize] += 1;
        }
        assert_eq!(counts, [2, 35, 10, 35, 2, 14, 2]);
    }

    #[test]
    fn subscriber_numbers_have_spec_width() {
        assert_eq!(subscriber_number(1), "000000000000001");
        assert_eq!(subscriber_number(123_456), "000000000123456");
    }

    #[tokio::test]
    async fn small_workload_has_no_unexpected_errors() {
        let config = Config {
            subscribers: 100,
            operations: 10_000,
            threads: 1,
            repetitions: 1,
            sample_every: 100,
            seed: 42,
        };
        let result = run_repetition(&config, 1).await;
        assert_eq!(result.unexpected_errors, 0);
        assert_eq!(
            result.transactions_completed + result.expected_aborts,
            config.operations
        );
    }
}
