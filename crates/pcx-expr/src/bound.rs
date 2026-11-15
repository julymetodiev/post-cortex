// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Variables converted to CEL once, when an expression first reads each
//! (a block's variables it never reads are never converted), for every
//! expression a block evaluates over them, and a list's items evaluated
//! one by one over them: each
//! item binds only `item`, `index` (or `row`) over the variables, within a
//! budget of steps for the item and a meter of steps for the block
//! (docs/modules/workflow.md, data blocks).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use cel_core::{Activation, Value as Cel};

use crate::value::{Value, Values};

/// The variables, each converted to CEL when it is first read.
pub struct Bound<'v> {
    values: &'v Values,
    read: HashMap<&'v str, OnceLock<Cel>>,
}

impl<'v> Bound<'v> {
    /// `values`, converted as they are read.
    #[must_use]
    pub fn new(values: &'v Values) -> Self {
        Self {
            values,
            read: values
                .keys()
                .map(|k| (k.as_str(), OnceLock::new()))
                .collect(),
        }
    }

    /// Variable `name` as CEL reads it, converted the first time.
    fn get(&self, name: &str) -> Option<Cel> {
        let (key, once) = self.read.get_key_value(name)?;
        let value = self.values.get(*key)?;
        Some(once.get_or_init(|| value.cel()).clone())
    }

    /// The variables with item `index` of `items` bound as `item`, and its
    /// place as `index`; none past the items.
    #[must_use]
    pub fn item<'a>(&'a self, items: &Items, index: usize) -> Option<Local<'a, 'v>> {
        let item = items.0.get(index)?.clone();
        Some(Local {
            vars: self,
            local: [Some(("item", item)), Some(("index", place(index)))],
        })
    }

    /// The variables with `row` (a record a block made) and its place
    /// bound.
    #[must_use]
    pub fn row<'a>(&'a self, row: &Value, index: usize) -> Local<'a, 'v> {
        Local {
            vars: self,
            local: [Some(("row", row.cel())), Some(("index", place(index)))],
        }
    }

    /// The variables with `value` (the one value a block reads) bound.
    #[must_use]
    pub fn value<'a>(&'a self, value: &Value) -> Local<'a, 'v> {
        Local {
            vars: self,
            local: [Some(("value", value.cel())), None],
        }
    }

    /// The variables alone.
    #[must_use]
    pub fn alone(&self) -> Local<'_, 'v> {
        Local {
            vars: self,
            local: [None, None],
        }
    }
}

/// `index` as CEL's `int`.
fn place(index: usize) -> Cel {
    // A list in memory holds fewer items than an i64 counts.
    Cel::Int(index as i64)
}

/// The variables of one evaluation: the block's and a few of its own,
/// which hide the block's of their names.
pub struct Local<'a, 'v> {
    vars: &'a Bound<'v>,
    /// At most two of its own (an item and its place): no allocation for
    /// each item.
    local: [Option<(&'static str, Cel)>; 2],
}

impl Activation for Local<'_, '_> {
    fn resolve(&self, name: &str) -> Option<Cel> {
        self.local
            .iter()
            .flatten()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.clone())
            .or_else(|| self.vars.get(name))
    }
}

/// A list's items as CEL holds them, converted back only when kept.
pub struct Items(Arc<[Cel]>);

impl Items {
    /// `items`, a list an expression gave.
    #[must_use]
    pub fn new(items: Arc<[Cel]>) -> Self {
        Self(items)
    }

    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Item `index`.
    ///
    /// # Errors
    /// There is no such item, or it is no value here; the reason.
    pub fn value(&self, index: usize) -> Result<Value, String> {
        let item = self
            .0
            .get(index)
            .ok_or_else(|| format!("no item {index} of {}", self.0.len()))?;
        Value::of_cel(item)
    }
}

/// Steps a block may still spend over all its evaluations.
#[derive(Debug, Clone, Copy)]
pub struct Meter {
    left: u64,
    all: u64,
}

impl Meter {
    /// A block's meter of `steps`.
    #[must_use]
    pub fn new(steps: u64) -> Self {
        Self {
            left: steps,
            all: steps,
        }
    }

    /// The steps left.
    #[must_use]
    pub fn left(&self) -> u64 {
        self.left
    }

    /// The steps it started with.
    #[must_use]
    pub fn all(&self) -> u64 {
        self.all
    }

    /// `steps` spent.
    pub fn spend(&mut self, steps: u64) {
        self.left = self.left.saturating_sub(steps);
    }

    /// `steps` spent when it has them: false, and none left, when it has
    /// fewer (an evaluation on a copy of it spent more than it now has).
    pub fn charge(&mut self, steps: u64) -> bool {
        let had = steps <= self.left;
        self.spend(steps);
        had
    }
}
