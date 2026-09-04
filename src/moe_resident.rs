//! MoE-PGO resident provenance lookup: extracted Vec indexes versus WorkTable.
//!
//! The application currently resolves a Decision question by scanning
//! `DecisionProgram.examples` and their origins for `(source, ordinal, rule)`.
//! This benchmark flattens that same logical origin relation once, then holds
//! the row payload constant while changing only its lookup machinery.

use std::time::{Duration, Instant};

use worktable::prelude::*;
use worktable::worktable;
use worktable_vec::{ArcticTable, IndexedTable, LinearTable};

use crate::rng::Rng;

worktable! {
    name: MoeResidentOrigin,
    persist: false,
    columns: {
        origin_key: u64 primary_key using arctic,
        source: u32,
        ordinal: u32,
        example: u32,
        path_count: u16,
        origin_count: u16,
    }
}

worktable! {
    name: MoeResidentOriginPersisted,
    persist: true,
    columns: {
        origin_key: u64 primary_key using arctic,
        source: u32,
        ordinal: u32,
        example: u32,
        path_count: u16,
        origin_count: u16,
    }
}

worktable! {
    name: MoeResidentOriginWti,
    persist: false,
    columns: {
        origin_key: u64 primary_key using worktables_index,
        source: u32,
        ordinal: u32,
        example: u32,
        path_count: u16,
        origin_count: u16,
    }
}

worktable! {
    name: MoeResidentOriginCongee,
    persist: false,
    columns: {
        origin_key: u64 primary_key using congee,
        source: u32,
        ordinal: u32,
        example: u32,
        path_count: u16,
        origin_count: u16,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OriginValue {
    pub source: u32,
    pub ordinal: u32,
    pub example: u32,
    pub path_count: u16,
    pub origin_count: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct TimedChecksum {
    pub elapsed: Duration,
    pub checksum: u64,
}

pub struct Tables {
    pub linear: LinearTable<u64, OriginValue>,
    pub btree: IndexedTable<u64, OriginValue>,
    pub arctic_vec: ArcticTable<u64, OriginValue>,
    pub worktable_arctic: MoeResidentOriginWorkTable,
    pub worktable_wti: MoeResidentOriginWtiWorkTable,
    pub worktable_congee: MoeResidentOriginCongeeWorkTable,
}

#[derive(Clone, Copy, Debug)]
pub struct BuildTimes {
    pub linear: Duration,
    pub btree: Duration,
    pub arctic_vec: Duration,
    pub worktable_arctic: Duration,
    pub worktable_wti: Duration,
    pub worktable_congee: Duration,
}

#[inline]
pub fn origin_key(source: u32, ordinal: u32) -> u64 {
    (u64::from(source) << 32) | u64::from(ordinal)
}

pub fn logical_rows(row_count: usize) -> Vec<(u64, OriginValue)> {
    (0..row_count)
        .map(|index| logical_row(index, row_count))
        .collect()
}

pub fn logical_row(index: usize, row_count: usize) -> (u64, OriginValue) {
    // WT currently has 122 source documents and 1,528 open questions. Varying
    // ordinals retains that source/question key shape while the payload
    // mirrors the fields needed after the origin join.
    let source = (index % 122) as u32;
    let ordinal = (index / 122) as u32;
    let value = OriginValue {
        source,
        ordinal,
        example: ((index * 1_103) % row_count.max(1)) as u32,
        path_count: (1 + index % 17) as u16,
        origin_count: (1 + usize::from(index.is_multiple_of(97))) as u16,
    };
    (origin_key(source, ordinal), value)
}

pub fn build(row_count: usize) -> (Tables, BuildTimes) {
    let rows = logical_rows(row_count);

    let start = Instant::now();
    let mut linear = LinearTable::with_capacity(rows.len());
    for (key, value) in rows.iter().copied() {
        linear.insert(key, value).expect("unique origin");
    }
    let linear_time = start.elapsed();

    let start = Instant::now();
    let mut btree = IndexedTable::with_capacity(rows.len());
    for (key, value) in rows.iter().copied() {
        btree.insert(key, value).expect("unique origin");
    }
    let btree_time = start.elapsed();

    let start = Instant::now();
    let mut arctic_vec = ArcticTable::with_capacity(rows.len());
    for (key, value) in rows.iter().copied() {
        arctic_vec.insert(key, value).expect("unique origin");
    }
    let arctic_vec_time = start.elapsed();

    let start = Instant::now();
    let worktable_arctic = MoeResidentOriginWorkTable::default();
    for (key, value) in rows.iter().copied() {
        futures::executor::block_on(worktable_arctic.insert(MoeResidentOriginRow {
            origin_key: key,
            source: value.source,
            ordinal: value.ordinal,
            example: value.example,
            path_count: value.path_count,
            origin_count: value.origin_count,
        }))
        .expect("unique origin");
    }
    let worktable_time = start.elapsed();

    let start = Instant::now();
    let worktable_wti = MoeResidentOriginWtiWorkTable::default();
    for (key, value) in rows.iter().copied() {
        futures::executor::block_on(worktable_wti.insert(MoeResidentOriginWtiRow {
            origin_key: key,
            source: value.source,
            ordinal: value.ordinal,
            example: value.example,
            path_count: value.path_count,
            origin_count: value.origin_count,
        }))
        .expect("unique origin");
    }
    let worktable_wti_time = start.elapsed();

    let start = Instant::now();
    let worktable_congee = MoeResidentOriginCongeeWorkTable::default();
    for (key, value) in rows.iter().copied() {
        futures::executor::block_on(worktable_congee.insert(MoeResidentOriginCongeeRow {
            origin_key: key,
            source: value.source,
            ordinal: value.ordinal,
            example: value.example,
            path_count: value.path_count,
            origin_count: value.origin_count,
        }))
        .expect("unique origin");
    }
    let worktable_congee_time = start.elapsed();

    (
        Tables {
            linear,
            btree,
            arctic_vec,
            worktable_arctic,
            worktable_wti,
            worktable_congee,
        },
        BuildTimes {
            linear: linear_time,
            btree: btree_time,
            arctic_vec: arctic_vec_time,
            worktable_arctic: worktable_time,
            worktable_wti: worktable_wti_time,
            worktable_congee: worktable_congee_time,
        },
    )
}

pub fn query_keys(row_count: usize, queries: usize) -> Vec<u64> {
    let rows = logical_rows(row_count);
    let mut rng = Rng::new(0x0A11_C71C);
    (0..queries)
        .map(|_| rows[rng.below(rows.len() as u64) as usize].0)
        .collect()
}

#[inline]
fn checksum(value: OriginValue) -> u64 {
    u64::from(value.source)
        ^ u64::from(value.ordinal).rotate_left(7)
        ^ u64::from(value.example).rotate_left(17)
        ^ u64::from(value.path_count).rotate_left(31)
        ^ u64::from(value.origin_count).rotate_left(43)
}

pub fn query_linear(table: &LinearTable<u64, OriginValue>, keys: &[u64]) -> TimedChecksum {
    let start = Instant::now();
    let mut total = 0_u64;
    for key in keys {
        total = total.wrapping_add(checksum(*table.select(key).expect("known origin")));
    }
    TimedChecksum {
        elapsed: start.elapsed(),
        checksum: total,
    }
}

pub fn query_btree(table: &IndexedTable<u64, OriginValue>, keys: &[u64]) -> TimedChecksum {
    let start = Instant::now();
    let mut total = 0_u64;
    for key in keys {
        total = total.wrapping_add(checksum(*table.select(key).expect("known origin")));
    }
    TimedChecksum {
        elapsed: start.elapsed(),
        checksum: total,
    }
}

pub fn query_arctic_vec(table: &ArcticTable<u64, OriginValue>, keys: &[u64]) -> TimedChecksum {
    let start = Instant::now();
    let mut total = 0_u64;
    for key in keys {
        total = total.wrapping_add(checksum(*table.select(key).expect("known origin")));
    }
    TimedChecksum {
        elapsed: start.elapsed(),
        checksum: total,
    }
}

pub fn query_worktable(table: &MoeResidentOriginWorkTable, keys: &[u64]) -> TimedChecksum {
    let start = Instant::now();
    let mut total = 0_u64;
    for key in keys {
        let row = table.select(*key).expect("known origin");
        total = total.wrapping_add(checksum(OriginValue {
            source: row.source,
            ordinal: row.ordinal,
            example: row.example,
            path_count: row.path_count,
            origin_count: row.origin_count,
        }));
    }
    TimedChecksum {
        elapsed: start.elapsed(),
        checksum: total,
    }
}

pub fn worktable_arctic_links(table: &MoeResidentOriginWorkTable, keys: &[u64]) -> Vec<Link> {
    keys.iter()
        .map(|key| {
            table
                .0
                .primary_index
                .pk_map
                .get_value(&MoeResidentOriginPrimaryKey::from(*key))
                .expect("known origin")
                .0
        })
        .collect()
}

/// WorkTable's Arctic primary-index lookup without page access.
pub fn query_worktable_arctic_index(
    table: &MoeResidentOriginWorkTable,
    keys: &[u64],
) -> TimedChecksum {
    let start = Instant::now();
    let mut total = 0_u64;
    for key in keys {
        let link = table
            .0
            .primary_index
            .pk_map
            .get_value(&MoeResidentOriginPrimaryKey::from(*key))
            .expect("known origin")
            .0;
        total = total.wrapping_add(
            (u32::from(link.page_id) as u64).rotate_left(11)
                ^ u64::from(link.offset).rotate_left(23)
                ^ u64::from(link.length),
        );
    }
    TimedChecksum {
        elapsed: start.elapsed(),
        checksum: total,
    }
}

/// Page resolution, exact-cell read lock, flags, and row deserialization with
/// a pre-resolved link. The fixed fixture has no mutations, so omitting the
/// outer link-reuse pin is safe and isolates its cost.
pub fn query_worktable_data(table: &MoeResidentOriginWorkTable, links: &[Link]) -> TimedChecksum {
    let start = Instant::now();
    let mut total = 0_u64;
    for link in links {
        let row = table
            .0
            .data
            .select_non_ghosted(*link)
            .expect("known origin");
        total = total.wrapping_add(checksum(OriginValue {
            source: row.source,
            ordinal: row.ordinal,
            example: row.example,
            path_count: row.path_count,
            origin_count: row.origin_count,
        }));
    }
    TimedChecksum {
        elapsed: start.elapsed(),
        checksum: total,
    }
}

/// Same pre-resolved page access with the production outer grace-period pin
/// acquired once per lookup.
pub fn query_worktable_data_pinned(
    table: &MoeResidentOriginWorkTable,
    links: &[Link],
) -> TimedChecksum {
    let start = Instant::now();
    let mut total = 0_u64;
    for link in links {
        let _guard = table.0.data.read_guard();
        let row = table
            .0
            .data
            .select_non_ghosted(*link)
            .expect("known origin");
        total = total.wrapping_add(checksum(OriginValue {
            source: row.source,
            ordinal: row.ordinal,
            example: row.example,
            path_count: row.path_count,
            origin_count: row.origin_count,
        }));
    }
    TimedChecksum {
        elapsed: start.elapsed(),
        checksum: total,
    }
}

pub fn query_worktable_persisted(
    table: &MoeResidentOriginPersistedWorkTable,
    keys: &[u64],
) -> TimedChecksum {
    let start = Instant::now();
    let mut total = 0_u64;
    for key in keys {
        let row = table.select(*key).expect("known origin");
        total = total.wrapping_add(checksum(OriginValue {
            source: row.source,
            ordinal: row.ordinal,
            example: row.example,
            path_count: row.path_count,
            origin_count: row.origin_count,
        }));
    }
    TimedChecksum {
        elapsed: start.elapsed(),
        checksum: total,
    }
}

pub fn query_worktable_wti(table: &MoeResidentOriginWtiWorkTable, keys: &[u64]) -> TimedChecksum {
    let start = Instant::now();
    let mut total = 0_u64;
    for key in keys {
        let row = table.select(*key).expect("known origin");
        total = total.wrapping_add(checksum(OriginValue {
            source: row.source,
            ordinal: row.ordinal,
            example: row.example,
            path_count: row.path_count,
            origin_count: row.origin_count,
        }));
    }
    TimedChecksum {
        elapsed: start.elapsed(),
        checksum: total,
    }
}

pub fn query_worktable_congee(
    table: &MoeResidentOriginCongeeWorkTable,
    keys: &[u64],
) -> TimedChecksum {
    let start = Instant::now();
    let mut total = 0_u64;
    for key in keys {
        let row = table.select(*key).expect("known origin");
        total = total.wrapping_add(checksum(OriginValue {
            source: row.source,
            ordinal: row.ordinal,
            example: row.example,
            path_count: row.path_count,
            origin_count: row.origin_count,
        }));
    }
    TimedChecksum {
        elapsed: start.elapsed(),
        checksum: total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_index_returns_the_same_logical_rows_and_checksum() {
        let (tables, _) = build(1_528);
        let keys = query_keys(1_528, 10_000);
        let linear = query_linear(&tables.linear, &keys);
        let btree = query_btree(&tables.btree, &keys);
        let arctic_vec = query_arctic_vec(&tables.arctic_vec, &keys);
        let worktable = query_worktable(&tables.worktable_arctic, &keys);
        let worktable_wti = query_worktable_wti(&tables.worktable_wti, &keys);
        let worktable_congee = query_worktable_congee(&tables.worktable_congee, &keys);
        assert_eq!(linear.checksum, btree.checksum);
        assert_eq!(linear.checksum, arctic_vec.checksum);
        assert_eq!(linear.checksum, worktable.checksum);
        assert_eq!(linear.checksum, worktable_wti.checksum);
        assert_eq!(linear.checksum, worktable_congee.checksum);
    }
}
