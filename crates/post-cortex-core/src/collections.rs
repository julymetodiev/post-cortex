// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Collections of a session that two versions share: a commit changes a
//! session while readers still hold the version before, so the new version
//! must not copy what it did not change.
//!
//! - [`SharedMap`]: a persistent hash trie (`rpds`). Lookups by key are
//!   faster than a `BTreeMap`'s; a change copies the path to its entry.
//!   It has no order, so it has no plain `iter`: a caller picks
//!   [`SharedMap::sorted`] (key order, for anything whose result depends on
//!   the order: serialisation, ranking ties, analytics) or
//!   [`SharedMap::iter_unordered`] (sums, counts, filters).
//! - [`SharedVec`]: an append-mostly vector in two levels of shared
//!   blocks; an append copies a few hundred pointers at most, iteration is
//!   as fast as a slice's.

use std::borrow::Borrow;
use std::hash::Hash;
use std::sync::Arc;

use rpds::HashTrieMapSync;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A persistent map (see the module docs).
#[derive(Clone)]
pub struct SharedMap<K: Hash + Eq, V> {
    map: HashTrieMapSync<K, V>,
}

impl<K: Hash + Eq, V> Default for SharedMap<K, V> {
    fn default() -> Self {
        Self {
            map: HashTrieMapSync::new_sync(),
        }
    }
}

impl<K: Hash + Eq + Clone, V: Clone> SharedMap<K, V> {
    /// An empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Entries in the map.
    pub fn len(&self) -> usize {
        self.map.size()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The value of `key`.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.get(key)
    }

    /// The stored key and value of `key`.
    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.get_key_value(key)
    }

    /// The value of `key`, to change in place (the path to it is copied
    /// when another version shares it).
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.get_mut(key)
    }

    /// Whether `key` is in the map.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.contains_key(key)
    }

    /// Put `value` under `key`, replacing what it held.
    pub fn insert(&mut self, key: K, value: V) {
        self.map.insert_mut(key, value);
    }

    /// Take `key` out; returns its value.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let value = self.map.get(key).cloned()?;
        self.map.remove_mut(key);
        Some(value)
    }

    /// Apply `change` to the value of `key`, inserting `make()` first when
    /// it is missing.
    pub fn upsert(&mut self, key: K, make: impl FnOnce() -> V, change: impl FnOnce(&mut V)) {
        match self.map.get_mut(&key) {
            Some(value) => change(value),
            None => {
                let mut value = make();
                change(&mut value);
                self.map.insert_mut(key, value);
            }
        }
    }

    /// Keep the entries `keep` accepts.
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        let gone: Vec<K> = self
            .map
            .iter()
            .filter(|(k, v)| !keep(k, v))
            .map(|(k, _)| k.clone())
            .collect();
        for key in gone {
            self.map.remove_mut(&key);
        }
    }

    /// Remove every entry.
    pub fn clear(&mut self) {
        self.map = HashTrieMapSync::new_sync();
    }

    /// The entries in no particular order (it varies between processes):
    /// for work whose result does not depend on the order.
    pub fn iter_unordered(&self) -> impl Iterator<Item = (&K, &V)> {
        self.map.iter()
    }

    /// The keys in no particular order (see [`Self::iter_unordered`]).
    pub fn keys_unordered(&self) -> impl Iterator<Item = &K> {
        self.map.keys()
    }

    /// The values in no particular order (see [`Self::iter_unordered`]).
    pub fn values_unordered(&self) -> impl Iterator<Item = &V> {
        self.map.values()
    }
}

impl<K: Hash + Eq + Ord + Clone, V: Clone> SharedMap<K, V> {
    /// The entries in key order: for work whose result depends on the
    /// order. O(n log n).
    pub fn sorted(&self) -> Vec<(&K, &V)> {
        let mut entries: Vec<(&K, &V)> = self.map.iter().collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        entries
    }
}

impl<K: Hash + Eq + Clone, V: Clone> FromIterator<(K, V)> for SharedMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = Self::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

impl<K, Q, V> std::ops::Index<&Q> for SharedMap<K, V>
where
    K: Hash + Eq + Borrow<Q>,
    Q: Hash + Eq + ?Sized,
{
    type Output = V;

    fn index(&self, key: &Q) -> &V {
        &self.map[key]
    }
}

/// Equal when both hold the same entries.
impl<K: Hash + Eq + Clone, V: Clone + PartialEq> PartialEq for SharedMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter_unordered()
                .all(|(k, v)| other.get(k).is_some_and(|w| w == v))
    }
}

impl<K: Hash + Eq + Ord + Clone + std::fmt::Debug, V: Clone + std::fmt::Debug> std::fmt::Debug
    for SharedMap<K, V>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.sorted()).finish()
    }
}

/// Serialised as a map in key order, so the bytes are the same as a
/// `BTreeMap`'s.
impl<K, V> Serialize for SharedMap<K, V>
where
    K: Hash + Eq + Ord + Clone + Serialize,
    V: Clone + Serialize,
{
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.sorted())
    }
}

impl<'de, K, V> Deserialize<'de> for SharedMap<K, V>
where
    K: Hash + Eq + Ord + Clone + Deserialize<'de>,
    V: Clone + Deserialize<'de>,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let map = std::collections::BTreeMap::<K, V>::deserialize(deserializer)?;
        Ok(map.into_iter().collect())
    }
}

/// Elements per block of a [`SharedVec`], and blocks per group.
const FAN: usize = 64;
/// Elements per group of blocks.
const GROUP: usize = FAN * FAN;

/// A group of blocks.
type Group<T> = Arc<Vec<Arc<Vec<T>>>>;

/// An append-mostly vector in two levels of shared blocks (see the module
/// docs): groups of `FAN` blocks of `FAN` elements. An append copies the
/// group list (one pointer per `GROUP` elements), the last group and the
/// last block, so at most a few hundred pointers whatever the length.
#[derive(Clone)]
pub struct SharedVec<T> {
    groups: Arc<Vec<Group<T>>>,
    len: usize,
}

impl<T> Default for SharedVec<T> {
    fn default() -> Self {
        Self {
            groups: Arc::new(Vec::new()),
            len: 0,
        }
    }
}

impl<T: Clone> SharedVec<T> {
    /// An empty vector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Elements in the vector.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the vector is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append `value`: copies the group list, the last group and the last
    /// block when another version shares them, nothing else.
    pub fn push(&mut self, value: T) {
        let groups = Arc::make_mut(&mut self.groups);
        if self.len.is_multiple_of(GROUP) {
            groups.push(Arc::new(Vec::with_capacity(FAN)));
        }
        if let Some(group) = groups.last_mut() {
            let group = Arc::make_mut(group);
            if self.len.is_multiple_of(FAN) {
                group.push(Arc::new(Vec::with_capacity(FAN)));
            }
            if let Some(block) = group.last_mut() {
                Arc::make_mut(block).push(value);
                self.len += 1;
            }
        }
    }

    /// The element at `index`.
    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len {
            return None;
        }
        self.groups
            .get(index / GROUP)?
            .get(index % GROUP / FAN)?
            .get(index % FAN)
    }

    /// The element at `index`, to change in place (its block and group are
    /// copied when another version shares them).
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index >= self.len {
            return None;
        }
        let group = Arc::make_mut(&mut self.groups).get_mut(index / GROUP)?;
        let block = Arc::make_mut(group).get_mut(index % GROUP / FAN)?;
        Arc::make_mut(block).get_mut(index % FAN)
    }

    /// The first element.
    pub fn first(&self) -> Option<&T> {
        self.get(0)
    }

    /// The last element.
    pub fn last(&self) -> Option<&T> {
        self.len.checked_sub(1).and_then(|i| self.get(i))
    }

    /// The elements from `start` on, in order (O(1) to start).
    pub fn iter_from(&self, start: usize) -> SharedVecIter<'_, T> {
        let start = start.min(self.len);
        let (g, b, e) = (start / GROUP, start % GROUP / FAN, start % FAN);
        let group: &[Arc<Vec<T>>] = self.groups.get(g).map_or(&[], |group| group.as_slice());
        let block: &[T] = group.get(b).map_or(&[], |block| block.as_slice());
        SharedVecIter {
            groups: self.groups.get(g + 1..).unwrap_or(&[]).iter(),
            blocks_front: group.get(b + 1..).unwrap_or(&[]).iter(),
            front: block.get(e..).unwrap_or(&[]).iter(),
            blocks_back: [].iter(),
            back: [].iter(),
            left: self.len - start,
        }
    }

    /// Whether `self` and `other` are the same version (they share every
    /// group): a cheap identity for caches keyed by version.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.len == other.len && Arc::ptr_eq(&self.groups, &other.groups)
    }

    /// The elements in order.
    pub fn iter(&self) -> SharedVecIter<'_, T> {
        self.iter_from(0)
    }

    /// Keep the elements `keep` accepts, in order (rebuilds the blocks:
    /// removals are rare).
    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        *self = self.iter().filter(|v| keep(v)).cloned().collect();
    }

    /// The elements, owned.
    pub fn to_vec(&self) -> Vec<T> {
        self.iter().cloned().collect()
    }

    /// Whether `value` is an element. O(n).
    pub fn contains(&self, value: &T) -> bool
    where
        T: PartialEq,
    {
        self.iter().any(|v| v == value)
    }
}

/// The elements of a [`SharedVec`] in order, block by block.
#[derive(Clone)]
pub struct SharedVecIter<'a, T> {
    groups: std::slice::Iter<'a, Group<T>>,
    blocks_front: std::slice::Iter<'a, Arc<Vec<T>>>,
    front: std::slice::Iter<'a, T>,
    blocks_back: std::slice::Iter<'a, Arc<Vec<T>>>,
    back: std::slice::Iter<'a, T>,
    left: usize,
}

impl<'a, T> Iterator for SharedVecIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        loop {
            if let Some(item) = self.front.next() {
                self.left -= 1;
                return Some(item);
            }
            if let Some(block) = self.blocks_front.next() {
                self.front = block.iter();
            } else if let Some(group) = self.groups.next() {
                self.blocks_front = group.iter();
            } else if let Some(block) = self.blocks_back.next() {
                self.front = block.iter();
            } else {
                let item = self.back.next()?;
                self.left -= 1;
                return Some(item);
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.left, Some(self.left))
    }
}

impl<T> DoubleEndedIterator for SharedVecIter<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.back.next_back() {
                self.left -= 1;
                return Some(item);
            }
            if let Some(block) = self.blocks_back.next_back() {
                self.back = block.iter();
            } else if let Some(group) = self.groups.next_back() {
                self.blocks_back = group.iter();
            } else if let Some(block) = self.blocks_front.next_back() {
                self.back = block.iter();
            } else {
                let item = self.front.next_back()?;
                self.left -= 1;
                return Some(item);
            }
        }
    }
}

impl<T> ExactSizeIterator for SharedVecIter<'_, T> {}

impl<T: Clone> FromIterator<T> for SharedVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut vec = Self::new();
        for value in iter {
            vec.push(value);
        }
        vec
    }
}

impl<T: Clone> From<Vec<T>> for SharedVec<T> {
    fn from(values: Vec<T>) -> Self {
        values.into_iter().collect()
    }
}

impl<T> std::ops::Index<usize> for SharedVec<T> {
    type Output = T;

    fn index(&self, index: usize) -> &T {
        &self.groups[index / GROUP][index % GROUP / FAN][index % FAN]
    }
}

impl<T: Clone + PartialEq> PartialEq for SharedVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().eq(other.iter())
    }
}

impl<T: Clone + std::fmt::Debug> std::fmt::Debug for SharedVec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<'a, T: Clone> IntoIterator for &'a SharedVec<T> {
    type Item = &'a T;
    type IntoIter = SharedVecIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Serialised as a sequence, like a `Vec`.
impl<T: Clone + Serialize> Serialize for SharedVec<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de, T: Clone + Deserialize<'de>> Deserialize<'de> for SharedVec<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Vec::<T>::deserialize(deserializer)?.into())
    }
}

#[cfg(test)]
mod tests;
