//! Fixture and statistics shared by the case binary and its tests.
//!
//! The key set is **synthetic**. It reproduces the shape the EKOPathRS storage
//! review measured - `fn:<unit>/loop:<i>` - and its density - 354 regions over
//! 163 units - but the unit names are generated, not the corpus's real ones.
//! Every unit name is the same width, so every key is the same width and no
//! contender gets a length advantage.

use std::collections::BTreeMap;

use arctic::key::BoxedStr;
use arctic::key::NonNull;
use arctic::key::Str;
use arctic::sequential;

/// Regions per unit, cycled. 163 units under this rule emit exactly 354 keys,
/// which is the density `ekoctl mem` reported.
fn regions_for_unit(unit: usize) -> usize {
    if unit.is_multiple_of(6) { 3 } else { 2 }
}

/// Generate `count` keys of shape `fn:<unit>/loop:<i>`.
pub fn generate_keys(count: usize) -> Vec<String> {
    let mut keys = Vec::with_capacity(count);
    let mut unit = 0usize;
    while keys.len() < count {
        for region in 0..regions_for_unit(unit) {
            if keys.len() == count {
                break;
            }
            keys.push(format!("fn:unit_{unit:06}/loop:{region}"));
        }
        unit += 1;
    }
    keys
}

/// The distinct parent prefixes of `keys`, i.e. `fn:<unit>/`.
pub fn parent_prefixes(keys: &[String]) -> Vec<String> {
    let mut parents: Vec<String> = keys
        .iter()
        .map(|key| {
            let end = key.find('/').expect("Generated keys contain a slash");
            key[..=end].to_owned()
        })
        .collect();
    parents.dedup();
    parents
}

/// Deterministic xorshift64. A fixed seed keeps the probe order identical
/// across arms, profiles and machines.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn shuffle<T>(&mut self, slice: &mut [T]) {
        for index in (1..slice.len()).rev() {
            let swap = (self.next_u64() % (index as u64 + 1)) as usize;
            slice.swap(index, swap);
        }
    }
}

/// Both maps loaded with the same keys, plus a fixed probe order.
pub struct Fixture {
    pub keys: Vec<String>,
    pub parents: Vec<String>,
    pub arctic: sequential::Map<BoxedStr<NonNull>, u64>,
    pub btree: BTreeMap<String, u64>,
    /// Indices into `keys`, shuffled once.
    pub probes: Vec<u32>,
    /// Indices into `parents`, shuffled once.
    pub parent_probes: Vec<u32>,
}

impl Fixture {
    pub fn build(count: usize, seed: u64) -> Self {
        let keys = generate_keys(count);
        let parents = parent_prefixes(&keys);

        let mut arctic = sequential::Map::<BoxedStr<NonNull>, u64>::new();
        let mut btree = BTreeMap::new();
        for (index, key) in keys.iter().enumerate() {
            let value = index as u64;
            let validated = Str::<NonNull>::new(key.as_str()).expect("No null byte");
            let _ = arctic.upsert(validated, value);
            btree.insert(key.clone(), value);
        }

        let mut rng = Rng::new(seed);
        let mut probes: Vec<u32> = (0..keys.len() as u32).collect();
        rng.shuffle(&mut probes);
        let mut parent_probes: Vec<u32> = (0..parents.len() as u32).collect();
        rng.shuffle(&mut parent_probes);

        Self {
            keys,
            parents,
            arctic,
            btree,
            probes,
            parent_probes,
        }
    }

    /// Probe keys, pre-validated, in probe order. Construction is hoisted out
    /// of every timed region that says it is measuring the map.
    pub fn arctic_probe_keys(&self) -> Vec<&Str<NonNull>> {
        self.probes
            .iter()
            .map(|index| {
                Str::<NonNull>::new(self.keys[*index as usize].as_str()).expect("No null byte")
            })
            .collect()
    }

    /// The same probe keys as plain `&str`, in the same order.
    pub fn btree_probe_keys(&self) -> Vec<&str> {
        self.probes
            .iter()
            .map(|index| self.keys[*index as usize].as_str())
            .collect()
    }

    pub fn prefix_probe_keys(&self) -> Vec<&str> {
        self.parent_probes
            .iter()
            .map(|index| self.parents[*index as usize].as_str())
            .collect()
    }
}

/// Median of a sample set. Does not mutate the caller's vector order meaning.
pub fn median(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("No NaN"));
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

pub fn quantile(samples: &[f64], q: f64) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("No NaN"));
    let index = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[index]
}

impl Fixture {
    /// Probe keys in insertion order, i.e. lexicographic. A contender scanned
    /// in this order gets perfect locality; it is what a loop over the key
    /// list, rather than over a shuffled probe sequence, actually measures.
    pub fn arctic_keys_in_order(&self) -> Vec<&Str<NonNull>> {
        self.keys
            .iter()
            .map(|key| Str::<NonNull>::new(key.as_str()).expect("No null byte"))
            .collect()
    }

    pub fn btree_keys_in_order(&self) -> Vec<&str> {
        self.keys.iter().map(String::as_str).collect()
    }
}
