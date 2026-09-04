//! Explicit, non-concurrent `Vec`-backed table baselines.
//!
//! This crate isolates the storage pattern applications otherwise tend to
//! grow ad hoc: ordered rows, linear point lookup, and an optional side index.

use std::collections::BTreeMap;

use arctic::{ConcurrentMap, Key};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InsertError<K> {
    DuplicateKey(K),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinearTable<K, V> {
    rows: Vec<(K, V)>,
}

impl<K, V> LinearTable<K, V>
where
    K: Eq,
{
    pub fn new() -> Self {
        Self { rows: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            rows: Vec::with_capacity(capacity),
        }
    }

    pub fn insert(&mut self, key: K, value: V) -> Result<usize, InsertError<K>> {
        if self.rows.iter().any(|(present, _)| present == &key) {
            return Err(InsertError::DuplicateKey(key));
        }
        let row = self.rows.len();
        self.rows.push((key, value));
        Ok(row)
    }

    #[inline]
    pub fn select(&self, key: &K) -> Option<&V> {
        self.rows
            .iter()
            .find(|(present, _)| present == key)
            .map(|(_, value)| value)
    }

    pub fn rows(&self) -> &[(K, V)] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexedTable<K, V> {
    rows: Vec<(K, V)>,
    primary: BTreeMap<K, usize>,
}

impl<K, V> IndexedTable<K, V>
where
    K: Clone + Ord,
{
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            primary: BTreeMap::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            rows: Vec::with_capacity(capacity),
            primary: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, key: K, value: V) -> Result<usize, InsertError<K>> {
        if self.primary.contains_key(&key) {
            return Err(InsertError::DuplicateKey(key));
        }
        let row = self.rows.len();
        self.rows.push((key.clone(), value));
        self.primary.insert(key, row);
        Ok(row)
    }

    #[inline]
    pub fn select(&self, key: &K) -> Option<&V> {
        self.primary
            .get(key)
            .and_then(|row| self.rows.get(*row))
            .map(|(_, value)| value)
    }

    pub fn rows(&self) -> &[(K, V)] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

pub struct ArcticTable<K: Key, V> {
    rows: Vec<(K, V)>,
    primary: ConcurrentMap<K, u64>,
}

impl<K: Key, V> std::fmt::Debug for ArcticTable<K, V> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ArcticTable")
            .field("rows", &self.rows.len())
            .finish_non_exhaustive()
    }
}

impl<K: Key, V> Default for ArcticTable<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Key, V> ArcticTable<K, V> {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            primary: ConcurrentMap::default(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            rows: Vec::with_capacity(capacity),
            primary: ConcurrentMap::default(),
        }
    }

    pub fn insert(&mut self, key: K, value: V) -> Result<usize, InsertError<K>> {
        let row = self.rows.len();
        if self.primary.insert(key.as_insert(), row as u64).is_err() {
            return Err(InsertError::DuplicateKey(key));
        }
        self.rows.push((key, value));
        Ok(row)
    }

    #[inline]
    pub fn select(&self, key: &K::Borrowed) -> Option<&V> {
        let row = *self.primary.get(key)? as usize;
        self.rows.get(row).map(|(_, value)| value)
    }

    pub fn rows(&self) -> &[(K, V)] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_backend_has_the_same_row_and_lookup_contract() {
        let mut linear = LinearTable::new();
        let mut indexed = IndexedTable::new();
        let mut arctic = ArcticTable::new();
        for (key, value) in [(7_u64, "a"), (2, "b"), (11, "c")] {
            linear.insert(key, value).unwrap();
            indexed.insert(key, value).unwrap();
            arctic.insert(key, value).unwrap();
        }

        assert_eq!(linear.rows(), indexed.rows());
        assert_eq!(linear.rows(), arctic.rows());
        for key in [2_u64, 7, 11, 99] {
            assert_eq!(linear.select(&key), indexed.select(&key));
            assert_eq!(linear.select(&key), arctic.select(&key));
        }
        assert_eq!(
            linear.insert(7, "duplicate"),
            Err(InsertError::DuplicateKey(7))
        );
        assert_eq!(
            indexed.insert(7, "duplicate"),
            Err(InsertError::DuplicateKey(7))
        );
        assert_eq!(
            arctic.insert(7, "duplicate"),
            Err(InsertError::DuplicateKey(7))
        );
    }
}
