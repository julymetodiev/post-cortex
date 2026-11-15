// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The keys a data block orders and tells items apart by
//! (docs/modules/workflow.md, data blocks): a `distinct_by` key and a
//! `sort_by` key.

use std::cmp::Ordering;
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use pcx_expr::Value as Expr;
use post_cortex_proto::workflow::Failure;

use crate::domain::data::eval::gave;
use crate::domain::retry::{failure, kinds};

/// A `distinct_by` key.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub enum Key {
    Bool(bool),
    Int(i64),
    Uint(u64),
    Text(Arc<str>),
}

impl Key {
    pub fn of(value: Expr) -> Result<Self, Failure> {
        Ok(match value {
            Expr::Bool(b) => Self::Bool(b),
            Expr::Int(i) => Self::Int(i),
            Expr::Uint(u) => Self::Uint(u),
            Expr::String(s) => Self::Text(s),
            other => return Err(gave(&other, "string, int, uint or bool key")),
        })
    }
}

/// A `sort_by` key, ordered.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub enum Sorted {
    Int(i64),
    Uint(u64),
    Double(Total),
    Text(Arc<str>),
    Time(DateTime<Utc>),
    Span(TimeDelta),
}

/// A double ordered totally (NaN after every number).
#[derive(PartialEq)]
pub struct Total(f64);

impl Eq for Total {}

impl PartialOrd for Total {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Total {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Sorted {
    /// `keys` as keys of one ordered kind.
    pub fn all<'a>(keys: impl Iterator<Item = &'a Expr>) -> Result<Vec<Self>, Failure> {
        let mut out: Vec<Self> = Vec::new();
        for key in keys {
            let sorted = match key {
                Expr::Int(i) => Self::Int(*i),
                Expr::Uint(u) => Self::Uint(*u),
                Expr::Double(d) => Self::Double(Total(*d)),
                Expr::String(s) => Self::Text(Arc::clone(s)),
                Expr::Timestamp(t) => Self::Time(*t),
                Expr::Duration(d) => Self::Span(*d),
                other => return Err(gave(other, "key with an order")),
            };
            if let Some(first) = out.first()
                && std::mem::discriminant(first) != std::mem::discriminant(&sorted)
            {
                return Err(failure(
                    kinds::EXPRESSION,
                    "the sort keys are of more than one type",
                ));
            }
            out.push(sorted);
        }
        Ok(out)
    }
}
