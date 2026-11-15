// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A list and a map of values kept as CEL keeps them, shared: a clone, and
//! a value handed to CEL, is a pointer copy, not a copy of the items
//! (docs/modules/workflow.md, data blocks, speed). Every item was read as
//! a value here when the list or the map was made.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use cel_core::{MapKey, Value as Cel, ValueMap};

use crate::value::Value;

/// A list of values, shared.
#[derive(Clone, PartialEq)]
pub struct List(Arc<[Cel]>);

impl List {
    /// `items` as a list.
    pub fn new(items: impl IntoIterator<Item = Value>) -> Self {
        Self(items.into_iter().map(|v| v.cel()).collect())
    }

    /// `items`, CEL's, each a value here.
    ///
    /// # Errors
    /// An item that is none here; why.
    pub fn of_cel(items: &Arc<[Cel]>) -> Result<Self, String> {
        let mut plain = true;
        for item in items.iter() {
            plain &= Value::plain(item)?;
        }
        if plain {
            return Ok(Self(Arc::clone(items)));
        }
        // An enum is a number here: the list is made again.
        let read = items
            .iter()
            .map(Value::of_cel)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(read))
    }

    /// How many items it has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it has none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Item `index`.
    ///
    /// # Errors
    /// There is none, or it is no value here; why.
    pub fn get(&self, index: usize) -> Result<Value, String> {
        let item = self
            .0
            .get(index)
            .ok_or_else(|| format!("no item {index} of {}", self.0.len()))?;
        Value::of_cel(item)
    }

    /// Its items in order.
    pub fn iter(&self) -> impl Iterator<Item = Result<Value, String>> + '_ {
        self.0.iter().map(Value::of_cel)
    }

    /// The list as CEL holds it.
    #[must_use]
    pub fn cel(&self) -> Cel {
        Cel::List(Arc::clone(&self.0))
    }

    /// Its items as CEL holds them (a data block's `items`).
    #[must_use]
    pub fn shared(&self) -> &Arc<[Cel]> {
        &self.0
    }
}

impl FromIterator<Value> for List {
    fn from_iter<I: IntoIterator<Item = Value>>(items: I) -> Self {
        Self::new(items)
    }
}

impl fmt::Debug for List {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.iter()).finish()
    }
}

/// A map from text to values, shared, in the order of its keys.
#[derive(Clone)]
pub struct Entries(Arc<ValueMap>);

impl PartialEq for Entries {
    fn eq(&self, other: &Self) -> bool {
        self.cel() == other.cel()
    }
}

impl Entries {
    /// `entries` as a map.
    pub fn new(entries: impl IntoIterator<Item = (String, Value)>) -> Self {
        Self(Arc::new(ValueMap::from_entries(
            entries
                .into_iter()
                .map(|(k, v)| (MapKey::String(Arc::from(k)), v.cel())),
        )))
    }

    /// `entries` as a map, its keys shared (a record's field names, made
    /// once for every record).
    pub fn keyed(entries: impl IntoIterator<Item = (Arc<str>, Value)>) -> Self {
        Self(Arc::new(ValueMap::from_entries(
            entries
                .into_iter()
                .map(|(k, v)| (MapKey::String(k), v.cel())),
        )))
    }

    /// `map`, CEL's, its keys text and its values values here.
    ///
    /// # Errors
    /// A key that is not text, a value that is none here; why.
    pub fn of_cel(map: &Arc<ValueMap>) -> Result<Self, String> {
        if Value::plain(&Cel::Map(Arc::clone(map)))? {
            return Ok(Self(Arc::clone(map)));
        }
        // An enum is a number here: the map is made again.
        let read = map
            .iter()
            .map(|(k, v)| match k {
                MapKey::String(k) => Ok((k.to_string(), Value::of_cel(v)?)),
                _ => Err("a map whose keys are not text".to_string()),
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self::new(read))
    }

    /// How many entries it has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it has none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The value of `key`; none when it has no such key.
    ///
    /// # Errors
    /// Its value is none here; why.
    pub fn get(&self, key: &str) -> Result<Option<Value>, String> {
        self.0.get_str(key).map(Value::of_cel).transpose()
    }

    /// Its entries in the order of their keys.
    pub fn iter(&self) -> impl Iterator<Item = Result<(&str, Value), String>> + '_ {
        self.0.iter().map(|(k, v)| match k {
            MapKey::String(k) => Ok((&**k, Value::of_cel(v)?)),
            _ => Err("a map whose keys are not text".to_string()),
        })
    }

    /// Its entries, read, by key.
    ///
    /// # Errors
    /// A value that is none here; why.
    pub fn to_map(&self) -> Result<BTreeMap<String, Value>, String> {
        self.iter()
            .map(|e| e.map(|(k, v)| (k.to_string(), v)))
            .collect()
    }

    /// The map as CEL holds it.
    #[must_use]
    pub fn cel(&self) -> Cel {
        Cel::Map(Arc::clone(&self.0))
    }
}

impl FromIterator<(String, Value)> for Entries {
    fn from_iter<I: IntoIterator<Item = (String, Value)>>(entries: I) -> Self {
        Self::new(entries)
    }
}

impl fmt::Debug for Entries {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.0.iter()).finish()
    }
}
