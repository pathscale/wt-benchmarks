//! The agentcode storage profile: a semantic code index that republishes a
//! whole generation of facts about a repository every time the source changes.
//!
//! Every other bench here is a shape WorkTable might meet. This one is a shape
//! it has already met, taken from `pathscale/agentcode`, whose store is the
//! reason several WorkTable releases exist. The point is to answer two
//! questions without needing agentcode in the loop: will WorkTable carry this
//! workload, and did a change to WorkTable just make it worse.
//!
//! ## What the profile is
//!
//! A generation is one indexed state of a repository. Publishing one writes,
//! across three tables, roughly 18 symbol postings and 18 dependency edges per
//! source file, each row tagged with the generation's key. Generations are kept
//! side by side, so the tables accumulate and every row carries the generation
//! it belongs to.
//!
//! Four properties of that shape drive everything measured here.
//!
//! **The rows are persisted.** agentcode's dominant cost is the durable insert
//! path: 10.24 us marginal per persisted row against 0.46 us for the same
//! insert into a memory table, a factor of 22 that no index choice touches. The
//! `persisted` and `memory` arms of `publish` are that ratio, and it is the
//! single most important number in this file.
//!
//! **The generation key is hot.** Every row written in one generation shares
//! one `snapshot_key`, so the non-unique index over it has a fan-out equal to
//! the whole generation. This is exactly the distribution that made
//! `WorkTablesIndex` 0.0.8 scan on insert and cost agentcode a 21x regression,
//! and it is why `generation_scan` sweeps generation size rather than fixing it.
//!
//! **Adjacency is walked on every call.** `dependencies.query` looks an edge up
//! by source and by target. Both keys were 129 character strings until the
//! graph moved to a `u128` hash on Arctic, verified against the edge the row
//! already deserializes.
//!
//! **Width is measured at fleet scale.** Blob digests were 64 character hex
//! strings on the two highest-row tables and are now `u128` pairs. Row width is
//! the term that decides whether a 5 MB source tree becomes a 60 MB state
//! directory or a 60 GB one, so the tables here carry the real widths rather
//! than convenient ones.
//!
//! Feature-gated on `worktable-adapter`.

use std::path::Path;

/// Symbol postings per source file, from 14,400 symbols over 800 files.
pub const SYMBOLS_PER_FILE: u64 = 18;
/// Dependency edges per source file, from 14,400 edges over 800 files.
pub const EDGES_PER_FILE: u64 = 18;

/// Files in a generation. 800 is agentcode's measured production fixture, at
/// 14,400 symbols and 14,400 edges. The sweep varies this, so the generation key's
/// fan-out varies with it, which is the property that matters.
pub const FILES: [u64; 3] = [50, 200, 800];

/// Files touched by an incremental update. agentcode's measured case is one.
pub const CHANGED_FILES: u64 = 1;

/// Rows one generation of `files` files writes across the three tables.
pub fn rows_per_generation(files: u64) -> u64 {
    files * (SYMBOLS_PER_FILE * 2 + EDGES_PER_FILE)
}

/// A cheap deterministic spread, so keys look like digests rather than a
/// sequence and the index cannot exploit ordering that real data does not have.
pub fn spread(seed: u64) -> u128 {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    s ^= s >> 30;
    s = s.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    s ^= s >> 27;
    s = s.wrapping_mul(0x94D0_49BB_1331_11EB);
    s ^= s >> 31;
    ((s as u128) << 64) | (s.rotate_left(17) as u128)
}

/// A normalised symbol name of the length agentcode actually stores. Names are
/// the one genuinely variable-width column left on these tables, so they are
/// not shortened to make the bench look better.
pub fn symbol_name(file: u64, ordinal: u64) -> String {
    format!("crate::module_{file}::function_{ordinal}_normalized")
}

/// A serialised edge record, the width agentcode writes per dependency fact.
pub fn edge_blob(file: u64, ordinal: u64) -> String {
    format!(
        "{{\"kind\":\"calls\",\"source\":\"module_{file}::sym_{ordinal}\",\
         \"target\":\"module_{}::sym_{}\",\"range\":[{},{}]}}",
        file.wrapping_add(1),
        ordinal.wrapping_add(3),
        ordinal * 40,
        ordinal * 40 + 32
    )
}

/// Generates the three tables of the profile for one durability mode, plus a
/// driver exposing the operations agentcode's latency actually depends on.
///
/// The persisted and memory variants differ only in `persist`, so the ratio
/// between their `publish` arms isolates the durable write path and nothing
/// else.
macro_rules! codegraph_tables {
    ($module:ident, persist: $persist:tt) => {
        pub mod $module {
            use worktable::prelude::*;
            use worktable::worktable;

            // One row per distinct normalised symbol name in a generation.
            // `records_blob` addresses the posting's rows on disk and is a pair
            // of machine words rather than a 64 character hex digest: a
            // truncation would name no file, and this is the table where width
            // pays at fleet scale.
            worktable! {
                name: SymbolPosting,
                version: 3,
                persist: $persist,
                columns: {
                    id: u64 primary_key autoincrement using arctic,
                    posting_hash: u128,
                    snapshot_key: u128,
                    normalized_name: String,
                    records_blob_high: u128,
                    records_blob_low: u128
                },
                indexes: {
                    posting_idx: posting_hash unique using arctic,
                    snapshot_idx: snapshot_key using arctic
                }
            }

            // The lexicon that makes a generation enumerable. `snapshot_idx` is
            // the hot non-unique index: one key per generation, fan-out equal to
            // the generation.
            worktable! {
                name: SymbolLexeme,
                version: 2,
                persist: $persist,
                columns: {
                    id: u64 primary_key autoincrement using arctic,
                    lexeme_key: u128,
                    snapshot_key: u128,
                    normalized_name: String,
                    posting_hash: u128
                },
                indexes: {
                    lexeme_idx: lexeme_key unique using arctic,
                    snapshot_idx: snapshot_key using arctic
                }
            }

            // The graph adjacency `dependencies.query` walks on every call.
            // `source_key` and `target_key` are hashes of the node key, and a
            // query verifies every row it returns against the edge the row
            // already deserializes, because a hash can collide.
            worktable! {
                name: DependencyFact,
                version: 3,
                persist: $persist,
                columns: {
                    id: u64 primary_key autoincrement using arctic,
                    edge_hash: u128,
                    snapshot_key: u128,
                    source_key: u128,
                    target_key: u128,
                    edge_blob: String
                },
                indexes: {
                    edge_idx: edge_hash unique using arctic,
                    snapshot_idx: snapshot_key using arctic,
                    source_idx: source_key using arctic,
                    target_idx: target_key using arctic
                }
            }
        }
    };
}

codegraph_tables!(persisted, persist: true);
codegraph_tables!(memory, persist: false);

/// One generation's rows, built before the timed region so a publish benchmark
/// measures the write and not the row construction.
pub struct Generation {
    pub key: u128,
    pub files: u64,
}

impl Generation {
    pub fn new(ordinal: u64, files: u64) -> Self {
        Self {
            key: spread(0xC0DE_0000 ^ ordinal),
            files,
        }
    }

    /// The node key a dependency walk asks for. Taken from the middle of the
    /// generation so it is neither the first nor the last row inserted.
    pub fn probe_node(&self) -> u128 {
        spread(0x4E0D_u64 ^ (self.files / 2))
    }
}

macro_rules! codegraph_driver {
    ($module:ident) => {
        pub mod $module {
            use super::super::$module::*;
            use super::super::{
                EDGES_PER_FILE, Generation, SYMBOLS_PER_FILE, edge_blob, spread, symbol_name,
            };
            use worktable::prelude::*;

            pub struct Store {
                pub postings: SymbolPostingWorkTable,
                pub lexemes: SymbolLexemeWorkTable,
                pub edges: DependencyFactWorkTable,
            }

            impl Store {
                /// Writes one generation the way agentcode writes one: batched
                /// per table, then flushed, because a publish that has not
                /// reached disk has not happened.
                pub fn publish(&self, generation: &Generation) -> u64 {
                    let mut postings = Vec::new();
                    let mut lexemes = Vec::new();
                    let mut edges = Vec::new();
                    for file in 0..generation.files {
                        for ordinal in 0..SYMBOLS_PER_FILE {
                            let seed = generation.key as u64 ^ (file << 8) ^ ordinal;
                            let hash = spread(seed);
                            postings.push(SymbolPostingRow {
                                id: self.postings.get_next_pk().into(),
                                posting_hash: hash,
                                snapshot_key: generation.key,
                                normalized_name: symbol_name(file, ordinal),
                                records_blob_high: spread(seed ^ 0xB10B),
                                records_blob_low: spread(seed ^ 0xB10C),
                            });
                            lexemes.push(SymbolLexemeRow {
                                id: self.lexemes.get_next_pk().into(),
                                lexeme_key: spread(seed ^ 0x1EFE),
                                snapshot_key: generation.key,
                                normalized_name: symbol_name(file, ordinal),
                                posting_hash: hash,
                            });
                        }
                        for ordinal in 0..EDGES_PER_FILE {
                            let seed = generation.key as u64 ^ (file << 8) ^ ordinal;
                            edges.push(DependencyFactRow {
                                id: self.edges.get_next_pk().into(),
                                edge_hash: spread(seed ^ 0xED6E),
                                snapshot_key: generation.key,
                                source_key: spread(0x4E0D_u64 ^ file),
                                target_key: spread(0x4E0D_u64 ^ file.wrapping_add(1)),
                                edge_blob: edge_blob(file, ordinal),
                            });
                        }
                    }
                    let written = (postings.len() + lexemes.len() + edges.len()) as u64;
                    futures::executor::block_on(self.postings.insert_many(postings)).expect("postings");
                    futures::executor::block_on(self.lexemes.insert_many(lexemes)).expect("lexemes");
                    futures::executor::block_on(self.edges.insert_many(edges)).expect("edges");
                    written
                }

                /// Enumerates a generation through the hot non-unique index.
                /// Fan-out is the whole generation, which is the distribution
                /// that broke `WorkTablesIndex` 0.0.8.
                pub fn generation_scan(&self, generation: &Generation) -> usize {
                    self.lexemes
                        .select_by_snapshot_key(generation.key)
                        .execute()
                        .map(|rows| rows.len())
                        .unwrap_or(0)
                }

                /// The adjacency walk `dependencies.query` performs on every
                /// call: incoming and outgoing edges for one node.
                pub fn dependency_walk(&self, node: u128) -> usize {
                    let out = self
                        .edges
                        .select_by_source_key(node)
                        .execute()
                        .map(|rows| rows.len())
                        .unwrap_or(0);
                    let incoming = self
                        .edges
                        .select_by_target_key(node)
                        .execute()
                        .map(|rows| rows.len())
                        .unwrap_or(0);
                    out + incoming
                }
            }
        }
    };
}

pub mod driver {
    codegraph_driver!(persisted);
    codegraph_driver!(memory);
}

impl driver::persisted::Store {
    /// Opens the three tables under `root`, the way agentcode opens a state
    /// directory: one persistence engine per table, then a load that replays
    /// what is already there. Reopening an existing `root` is the cold-open
    /// path, so the same call serves both.
    pub async fn open(root: &Path) -> Self {
        use worktable::prelude::*;
        let root = root.to_string_lossy().to_string();
        std::fs::create_dir_all(&root).expect("state directory");

        let postings =
            persisted::SymbolPostingPersistenceEngine::new(DiskConfig::new_with_table_name(
                &root,
                persisted::SymbolPostingWorkTable::name_snake_case(),
                persisted::SymbolPostingWorkTable::version(),
            ))
            .await
            .expect("posting engine");
        let lexemes =
            persisted::SymbolLexemePersistenceEngine::new(DiskConfig::new_with_table_name(
                &root,
                persisted::SymbolLexemeWorkTable::name_snake_case(),
                persisted::SymbolLexemeWorkTable::version(),
            ))
            .await
            .expect("lexeme engine");
        let edges =
            persisted::DependencyFactPersistenceEngine::new(DiskConfig::new_with_table_name(
                &root,
                persisted::DependencyFactWorkTable::name_snake_case(),
                persisted::DependencyFactWorkTable::version(),
            ))
            .await
            .expect("edge engine");

        Self {
            postings: persisted::SymbolPostingWorkTable::load(postings)
                .await
                .expect("load postings"),
            lexemes: persisted::SymbolLexemeWorkTable::load(lexemes)
                .await
                .expect("load lexemes"),
            edges: persisted::DependencyFactWorkTable::load(edges)
                .await
                .expect("load edges"),
        }
    }

    /// Waits for every queued persistence operation. agentcode does this on
    /// publish and on shutdown, because a deploy that tears the process before
    /// the drain has lost the generation. A publish benchmark that skips it is
    /// measuring an enqueue, not a write.
    pub async fn flush(&self) {
        self.postings.wait_for_ops().await.expect("drain postings");
        self.lexemes.wait_for_ops().await.expect("drain lexemes");
        self.edges.wait_for_ops().await.expect("drain edges");
    }
}

// Written out rather than derived, and not moved into the macro. Both Stores
// are generated from the same macro and their fields all implement `Default`,
// so deriving it would also give the persisted Store a `Default` that builds
// tables with no persistence engine behind them: a store that looks durable,
// accepts writes, and silently keeps nothing.
#[allow(clippy::derivable_impls)]
impl Default for driver::memory::Store {
    fn default() -> Self {
        Self {
            postings: memory::SymbolPostingWorkTable::default(),
            lexemes: memory::SymbolLexemeWorkTable::default(),
            edges: memory::DependencyFactWorkTable::default(),
        }
    }
}
