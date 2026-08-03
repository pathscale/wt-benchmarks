use std::collections::BTreeMap;
use std::hint::black_box;
use std::str::FromStr;
use std::time::Instant;

use serde::Serialize;
use worktable::prelude::*;
use worktable::worktable;
use wt_benchmarks::kv::text_value;
use wt_benchmarks::result::LatencySummary;
use wt_benchmarks::rng::Rng;

worktable!(
    name: LinkBenchLink,
    columns: {
        id1: u64 primary_key,
        link_type: u64 primary_key,
        id2: u64 primary_key,
        source_type: u128,
        time: u64,
        version: u64,
        data: String,
    },
    indexes: {
        source_type_idx: source_type,
    }
);

worktable!(
    name: LinkBenchNode,
    columns: {
        id: u64 primary_key,
        version: u64,
        time: u64,
        data: String,
    }
);

#[derive(Clone, Debug)]
struct Config {
    nodes: u64,
    links_per_node: u64,
    link_types: u64,
    operations: u64,
    repetitions: usize,
    payload_bytes: usize,
    list_limit: usize,
    sample_every: u64,
    seed: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            nodes: 100_000,
            links_per_node: 20,
            link_types: 2,
            operations: 1_000_000,
            repetitions: 5,
            payload_bytes: 64,
            list_limit: 100,
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
                    "linkbench-worktable options:\n\
                     --nodes N              initial graph nodes (default 100000)\n\
                     --links-per-node N     initial outgoing links (default 20)\n\
                     --link-types N         link-type cardinality (default 2)\n\
                     --operations N         request operations (default 1000000)\n\
                     --repetitions N        fresh repetitions (default 5)\n\
                     --payload-bytes N      node/link payload bytes (default 64)\n\
                     --list-limit N         maximum links returned (default 100)\n\
                     --sample-every N       latency sampling interval (default 1000)\n\
                     --seed N               deterministic seed (default 42)"
                );
                std::process::exit(0);
            }
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--nodes" => config.nodes = parse(&flag, &value)?,
                "--links-per-node" => config.links_per_node = parse(&flag, &value)?,
                "--link-types" => config.link_types = parse(&flag, &value)?,
                "--operations" => config.operations = parse(&flag, &value)?,
                "--repetitions" => config.repetitions = parse(&flag, &value)?,
                "--payload-bytes" => config.payload_bytes = parse(&flag, &value)?,
                "--list-limit" => config.list_limit = parse(&flag, &value)?,
                "--sample-every" => config.sample_every = parse(&flag, &value)?,
                "--seed" => config.seed = parse(&flag, &value)?,
                _ => return Err(format!("unknown option: {flag}")),
            }
        }
        if config.nodes == 0
            || config.links_per_node == 0
            || config.link_types == 0
            || config.operations == 0
            || config.repetitions == 0
            || config.payload_bytes == 0
            || config.list_limit == 0
            || config.sample_every == 0
        {
            return Err(
                "counts, repetitions, sizes, and sampling interval must be non-zero".into(),
            );
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
    AddLink,
    DeleteLink,
    UpdateLink,
    CountLinks,
    GetLink,
    GetLinkList,
    AddNode,
    UpdateNode,
    DeleteNode,
    GetNode,
}

impl Kind {
    const ALL: [Self; 10] = [
        Self::AddLink,
        Self::DeleteLink,
        Self::UpdateLink,
        Self::CountLinks,
        Self::GetLink,
        Self::GetLinkList,
        Self::AddNode,
        Self::UpdateNode,
        Self::DeleteNode,
        Self::GetNode,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::AddLink => "add_link",
            Self::DeleteLink => "delete_link",
            Self::UpdateLink => "update_link",
            Self::CountLinks => "count_links",
            Self::GetLink => "get_link",
            Self::GetLinkList => "get_link_list",
            Self::AddNode => "add_node",
            Self::UpdateNode => "update_node",
            Self::DeleteNode => "delete_node",
            Self::GetNode => "get_node",
        }
    }
}

#[derive(Debug)]
enum Operation {
    LinkUpsert {
        kind: Kind,
        id1: u64,
        link_type: u64,
        id2: u64,
        time: u64,
        version: u64,
        data: String,
    },
    DeleteLink {
        id1: u64,
        link_type: u64,
        id2: u64,
    },
    CountLinks {
        id1: u64,
        link_type: u64,
    },
    GetLink {
        id1: u64,
        link_type: u64,
        id2: u64,
    },
    GetLinkList {
        id1: u64,
        link_type: u64,
        minimum_time: u64,
    },
    AddNode {
        id: u64,
        time: u64,
        data: String,
    },
    UpdateNode {
        id: u64,
        time: u64,
        data: String,
    },
    DeleteNode {
        id: u64,
    },
    GetNode {
        id: u64,
    },
}

impl Operation {
    fn kind(&self) -> Kind {
        match self {
            Self::LinkUpsert { kind, .. } => *kind,
            Self::DeleteLink { .. } => Kind::DeleteLink,
            Self::CountLinks { .. } => Kind::CountLinks,
            Self::GetLink { .. } => Kind::GetLink,
            Self::GetLinkList { .. } => Kind::GetLinkList,
            Self::AddNode { .. } => Kind::AddNode,
            Self::UpdateNode { .. } => Kind::UpdateNode,
            Self::DeleteNode { .. } => Kind::DeleteNode,
            Self::GetNode { .. } => Kind::GetNode,
        }
    }
}

#[derive(Serialize)]
struct ResultRow {
    schema_version: u32,
    suite: &'static str,
    profile: &'static str,
    port_status: &'static str,
    engine: &'static str,
    repetition: usize,
    nodes_initial: u64,
    links_initial: u64,
    operations: u64,
    errors: u64,
    payload_bytes: usize,
    elapsed_ns: u128,
    ops_per_second: f64,
    checksum: u64,
    operation_counts: BTreeMap<&'static str, u64>,
    latency: BTreeMap<&'static str, LatencySummary>,
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
    let operations = generate_operations(&config);
    for repetition in 1..=config.repetitions {
        let result = run_repetition(&config, repetition, &operations).await;
        println!(
            "{}",
            serde_json::to_string(&result).expect("result must serialize")
        );
    }
}

async fn run_repetition(config: &Config, repetition: usize, operations: &[Operation]) -> ResultRow {
    let links = LinkBenchLinkWorkTable::default();
    let nodes = LinkBenchNodeWorkTable::default();
    load(config, &links, &nodes);

    let mut counts = BTreeMap::new();
    let mut samples: BTreeMap<Kind, Vec<u64>> = Kind::ALL
        .into_iter()
        .map(|kind| (kind, Vec::new()))
        .collect();
    let mut errors = 0_u64;
    let mut checksum = 0_u64;
    let started = Instant::now();
    for (index, operation) in operations.iter().enumerate() {
        let kind = operation.kind();
        let sampled = (index as u64).is_multiple_of(config.sample_every);
        let operation_started = sampled.then(Instant::now);
        match execute(config, &links, &nodes, operation).await {
            Ok(value) => checksum = checksum.wrapping_add(value),
            Err(()) => errors += 1,
        }
        if let Some(operation_started) = operation_started {
            samples
                .get_mut(&kind)
                .expect("all kinds initialized")
                .push(operation_started.elapsed().as_nanos().min(u64::MAX as u128) as u64);
        }
        *counts.entry(kind.as_str()).or_insert(0) += 1;
    }
    let elapsed_ns = started.elapsed().as_nanos();
    let latency = samples
        .into_iter()
        .map(|(kind, values)| (kind.as_str(), LatencySummary::from_samples(values)))
        .collect();

    ResultRow {
        schema_version: 1,
        suite: "linkbench",
        profile: "fb-operation-mix-synthetic-zipf",
        port_status: "operation-compatible; empirical degree distribution not yet imported",
        engine: "worktable",
        repetition,
        nodes_initial: config.nodes,
        links_initial: config.nodes * config.links_per_node,
        operations: config.operations,
        errors,
        payload_bytes: config.payload_bytes,
        elapsed_ns,
        ops_per_second: config.operations as f64 / (elapsed_ns as f64 / 1_000_000_000.0),
        checksum: black_box(checksum),
        operation_counts: counts,
        latency,
        feature_versioned_row_publication: cfg!(feature = "versioned-row-publication"),
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
    }
}

fn load(config: &Config, links: &LinkBenchLinkWorkTable, nodes: &LinkBenchNodeWorkTable) {
    for id in 0..config.nodes {
        nodes
            .insert(LinkBenchNodeRow {
                id,
                version: 0,
                time: 0,
                data: text_value(id, config.payload_bytes),
            })
            .expect("fresh node key");
        for slot in 0..config.links_per_node {
            let link_type = slot % config.link_types;
            let type_slot = slot / config.link_types;
            let id2 = existing_target(config.nodes, id, link_type, type_slot);
            links
                .insert(link_row(
                    id,
                    link_type,
                    id2,
                    0,
                    0,
                    text_value(id ^ id2, config.payload_bytes),
                ))
                .expect("fresh link key");
        }
    }
}

async fn execute(
    config: &Config,
    links: &LinkBenchLinkWorkTable,
    nodes: &LinkBenchNodeWorkTable,
    operation: &Operation,
) -> Result<u64, ()> {
    match operation {
        Operation::LinkUpsert {
            id1,
            link_type,
            id2,
            time,
            version,
            data,
            ..
        } => links
            .upsert(link_row(
                *id1,
                *link_type,
                *id2,
                *time,
                *version,
                data.clone(),
            ))
            .await
            .map(|()| 1)
            .map_err(|_| ()),
        Operation::DeleteLink {
            id1,
            link_type,
            id2,
        } => match links.delete((*id1, *link_type, *id2)).await {
            Ok(()) => Ok(1),
            Err(WorkTableError::NotFound) => Ok(0),
            Err(_) => Err(()),
        },
        Operation::CountLinks { id1, link_type } => links
            .select_by_source_type(source_type(*id1, *link_type))
            .execute()
            .map(|rows| rows.len() as u64)
            .map_err(|_| ()),
        Operation::GetLink {
            id1,
            link_type,
            id2,
        } => Ok(u64::from(
            black_box(links.select((*id1, *link_type, *id2))).is_some(),
        )),
        Operation::GetLinkList {
            id1,
            link_type,
            minimum_time,
        } => {
            let mut rows = links
                .select_by_source_type(source_type(*id1, *link_type))
                .execute()
                .map_err(|_| ())?;
            rows.retain(|row| row.time >= *minimum_time);
            rows.sort_unstable_by_key(|row| std::cmp::Reverse(row.time));
            Ok(rows.len().min(config.list_limit) as u64)
        }
        Operation::AddNode { id, time, data } => nodes
            .upsert(LinkBenchNodeRow {
                id: *id,
                version: 0,
                time: *time,
                data: data.clone(),
            })
            .await
            .map(|()| 1)
            .map_err(|_| ()),
        Operation::UpdateNode { id, time, data } => match nodes
            .update(LinkBenchNodeRow {
                id: *id,
                version: *time,
                time: *time,
                data: data.clone(),
            })
            .await
        {
            Ok(_) => Ok(1),
            Err(WorkTableError::NotFound) => Ok(0),
            Err(_) => Err(()),
        },
        Operation::DeleteNode { id } => match nodes.delete(*id).await {
            Ok(()) => Ok(1),
            Err(WorkTableError::NotFound) => Ok(0),
            Err(_) => Err(()),
        },
        Operation::GetNode { id } => Ok(u64::from(black_box(nodes.select(*id)).is_some())),
    }
}

fn generate_operations(config: &Config) -> Vec<Operation> {
    let zipf = ZipfCdf::new(config.nodes as usize, 0.8);
    let mut rng = Rng::new(config.seed);
    let mut operations = Vec::with_capacity(config.operations as usize);
    for index in 0..config.operations {
        let choice = rng.unit_f64() * 100.0;
        let id1 = zipf.sample(&mut rng) as u64;
        let link_type = rng.below(config.link_types);
        let slot_count = config.links_per_node.div_ceil(config.link_types);
        let slot = rng.below(slot_count);
        let id2 = existing_target(config.nodes, id1, link_type, slot);
        let operation = if choice < 8.988_660_1 {
            Operation::LinkUpsert {
                kind: Kind::AddLink,
                id1,
                link_type,
                id2: config.nodes + index,
                time: index,
                version: 0,
                data: text_value(index, config.payload_bytes),
            }
        } else if choice < 11.979_426_5 {
            Operation::DeleteLink {
                id1,
                link_type,
                id2,
            }
        } else if choice < 19.991_639 {
            Operation::LinkUpsert {
                kind: Kind::UpdateLink,
                id1,
                link_type,
                id2,
                time: index,
                version: index,
                data: text_value(index, config.payload_bytes),
            }
        } else if choice < 24.877_995_7 {
            Operation::CountLinks { id1, link_type }
        } else if choice < 25.404_109_9 {
            Operation::GetLink {
                id1,
                link_type,
                id2,
            }
        } else if choice < 76.116_024_4 {
            Operation::GetLinkList {
                id1,
                link_type,
                minimum_time: index.saturating_sub(10_000),
            }
        } else if choice < 78.689_303_3 {
            Operation::AddNode {
                id: config.nodes + index,
                time: index,
                data: text_value(index, config.payload_bytes),
            }
        } else if choice < 86.055_740_3 {
            Operation::UpdateNode {
                id: rng.below(config.nodes),
                time: index,
                data: text_value(index, config.payload_bytes),
            }
        } else if choice < 87.067_331_7 {
            Operation::DeleteNode {
                id: rng.below(config.nodes),
            }
        } else {
            Operation::GetNode {
                id: zipf.sample(&mut rng) as u64,
            }
        };
        operations.push(operation);
    }
    operations
}

fn link_row(
    id1: u64,
    link_type: u64,
    id2: u64,
    time: u64,
    version: u64,
    data: String,
) -> LinkBenchLinkRow {
    LinkBenchLinkRow {
        id1,
        link_type,
        id2,
        source_type: source_type(id1, link_type),
        time,
        version,
        data,
    }
}

fn source_type(id1: u64, link_type: u64) -> u128 {
    ((id1 as u128) << 64) | link_type as u128
}

fn existing_target(nodes: u64, id1: u64, link_type: u64, slot: u64) -> u64 {
    id1.wrapping_mul(1_000_003)
        .wrapping_add(link_type.wrapping_mul(65_537))
        .wrapping_add(slot)
        % nodes
}

struct ZipfCdf {
    cumulative: Vec<f64>,
}

impl ZipfCdf {
    fn new(items: usize, theta: f64) -> Self {
        let mut cumulative = Vec::with_capacity(items);
        let mut sum = 0.0;
        for rank in 1..=items {
            sum += 1.0 / (rank as f64).powf(theta);
            cumulative.push(sum);
        }
        Self { cumulative }
    }

    fn sample(&self, rng: &mut Rng) -> usize {
        let target = rng.unit_f64() * self.cumulative.last().copied().expect("nonempty Zipf CDF");
        self.cumulative.partition_point(|value| *value < target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_mix_matches_fb_profile() {
        let config = Config {
            nodes: 1_000,
            operations: 1_000_000,
            ..Config::default()
        };
        let operations = generate_operations(&config);
        let get_lists = operations
            .iter()
            .filter(|operation| operation.kind() == Kind::GetLinkList)
            .count();
        let fraction = get_lists as f64 / operations.len() as f64;
        assert!((fraction - 0.507_119_145).abs() < 0.002);
    }

    #[test]
    fn source_type_encoding_is_injective_for_pair() {
        assert_ne!(source_type(1, 2), source_type(2, 1));
        assert_eq!(source_type(1, 2), (1_u128 << 64) | 2);
    }
}
