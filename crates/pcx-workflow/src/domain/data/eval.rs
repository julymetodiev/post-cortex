// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A data block run over its variables (docs/modules/workflow.md, data
//! blocks, speed): the variables converted to CEL when first read, each
//! item binding only `item` and `index` over them, each evaluation within
//! its own budget and the block's meter; at most a million items. Lists,
//! records, groups and violations are shared values (maps of their
//! fields): keeping an item or making a record copies no item.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use pcx_expr::constraints::Violation;
use pcx_expr::schema::zero;
use pcx_expr::{
    Bound, Entries, ExprError, ExprErrorKind, Items, List, Local, Meter, Value as Expr, Values,
};
use post_cortex_proto::workflow::Failure;

use crate::domain::data::order::{Key, Sorted};
use crate::domain::data::waves::each;
use crate::domain::data::{Compiled, Data, Filter, Input, Transform, Validate};
use crate::domain::graph::{INVALID, NEXT, VALID};
use crate::domain::retry::{failure, kinds};

/// Items a data block reads at most.
const MAX_ITEMS: usize = 1_000_000;

/// Steps a data block may spend over all its evaluations.
const MAX_STEPS: u64 = 50_000_000;

/// Violations a failed `validate` names in its message at most.
const NAMED: usize = 20;

/// What a data block gave.
#[derive(Debug)]
pub struct Gave {
    /// Its output, by its fields.
    pub output: BTreeMap<String, Expr>,
    /// The port it leaves by.
    pub port: &'static str,
    /// The items it read; 0 for one value.
    pub items: usize,
}

impl Data {
    /// The block run over `values`; `fail_invalid` when its `invalid` port
    /// has no edge, so a violation fails it.
    ///
    /// # Errors
    /// An expression failed or spent its steps, a value does not fit its
    /// field, too many items; a violation when `fail_invalid`.
    pub fn run(&self, values: &Values, fail_invalid: bool) -> Result<Gave, Failure> {
        let bound = Bound::new(values);
        let mut meter = Meter::new(MAX_STEPS);
        match &self.kind {
            Compiled::Filter(f) => f.run(&bound, &mut meter),
            Compiled::Transform(t) => t.run(&bound, &mut meter),
            Compiled::Validate(v) => v.run(&bound, &mut meter, fail_invalid),
        }
    }
}

impl Filter {
    fn run(&self, bound: &Bound<'_>, meter: &mut Meter) -> Result<Gave, Failure> {
        let items = listed(&self.items, bound, meter)?;
        let mut kept = Vec::new();
        let mut keys = BTreeSet::new();
        if self.limit != Some(0) {
            each(
                items.len(),
                meter,
                |i, meter| {
                    let local = item(bound, &items, i)?;
                    if !self.kept.holds_in(&local, meter).map_err(expression)? {
                        return Ok(None);
                    }
                    let key = match &self.distinct {
                        Some(distinct) => Some(Key::of(
                            distinct.value_in(&local, meter).map_err(expression)?,
                        )?),
                        None => None,
                    };
                    Ok(Some((key, items.value(i).map_err(internal)?)))
                },
                |_, passed| {
                    let Some((key, value)) = passed else {
                        return Ok(true);
                    };
                    if let Some(key) = key
                        && !keys.insert(key)
                    {
                        return Ok(true);
                    }
                    kept.push(value);
                    Ok(self.limit.is_none_or(|limit| kept.len() < limit))
                },
            )?;
        }
        let dropped = items.len() - kept.len();
        Ok(Gave {
            output: fields([
                ("items", Expr::list(kept)),
                ("dropped", Expr::Int(count(dropped))),
            ]),
            port: NEXT,
            items: items.len(),
        })
    }
}

impl Transform {
    fn run(&self, bound: &Bound<'_>, meter: &mut Meter) -> Result<Gave, Failure> {
        let Input::Items(source) = &self.input else {
            return self.one(bound, meter);
        };
        let items = listed(source, bound, meter)?;
        let mut rows = Vec::with_capacity(items.len());
        each(
            items.len(),
            meter,
            |i, meter| self.record(&item(bound, &items, i)?, meter, i),
            |_, row| {
                rows.push(row);
                Ok(true)
            },
        )?;
        if let Some(key) = &self.sort_by {
            let mut keyed = Vec::with_capacity(rows.len());
            each(
                rows.len(),
                meter,
                |i, meter| {
                    key.value_in(&bound.row(&rows[i], i), meter)
                        .map_err(expression)
                },
                |_, k| {
                    keyed.push(k);
                    Ok(true)
                },
            )?;
            let keys = Sorted::all(keyed.iter())?;
            let mut order: Vec<(Sorted, Expr)> = keys.into_iter().zip(rows).collect();
            if self.descending {
                order.sort_by(|a, b| b.0.cmp(&a.0));
            } else {
                order.sort_by(|a, b| a.0.cmp(&b.0));
            }
            rows = order.into_iter().map(|(_, row)| row).collect();
        }
        let n = items.len();
        let Some(group_by) = &self.group_by else {
            return Ok(Gave {
                output: fields([("items", Expr::list(rows))]),
                port: NEXT,
                items: n,
            });
        };
        let mut keys = Vec::with_capacity(rows.len());
        each(
            rows.len(),
            meter,
            |i, meter| match group_by
                .value_in(&bound.row(&rows[i], i), meter)
                .map_err(expression)?
            {
                Expr::String(key) => Ok(key),
                other => Err(gave(&other, "string")),
            },
            |_, key| {
                keys.push(key);
                Ok(true)
            },
        )?;
        let mut groups: Vec<(Arc<str>, Vec<Expr>)> = Vec::new();
        let mut at: HashMap<Arc<str>, usize> = HashMap::new();
        for (key, row) in keys.into_iter().zip(rows) {
            match at.get(&key) {
                Some(&g) => groups[g].1.push(row),
                None => {
                    at.insert(Arc::clone(&key), groups.len());
                    groups.push((key, vec![row]));
                }
            }
        }
        let (key, items) = (Arc::<str>::from("key"), Arc::<str>::from("items"));
        let groups = groups.into_iter().map(|(k, rows)| {
            Expr::Map(Entries::keyed([
                (Arc::clone(&key), Expr::String(k)),
                (Arc::clone(&items), Expr::list(rows)),
            ]))
        });
        Ok(Gave {
            output: fields([("groups", Expr::list(groups))]),
            port: NEXT,
            items: n,
        })
    }

    /// The fields of one value's record, as named values.
    fn one(&self, bound: &Bound<'_>, meter: &mut Meter) -> Result<Gave, Failure> {
        let Input::Value(source) = &self.input else {
            return Err(failure("internal", "a transform of items reads no value"));
        };
        let value = source.value_in(&bound.alone(), meter).map_err(expression)?;
        let local = bound.value(&value);
        let mut out = BTreeMap::new();
        for (field, _, expr) in &self.fields {
            let value = match expr {
                Some(expr) => typed(
                    field,
                    expr,
                    expr.value_in(&local, meter).map_err(expression)?,
                    None,
                )?,
                None => zero(&field.ty),
            };
            out.insert(field.name.clone(), value);
        }
        Ok(Gave {
            output: out,
            port: NEXT,
            items: 0,
        })
    }

    /// Item `index`'s record: each field evaluated over `local` (a field
    /// without an expression, or null, its zero value).
    fn record(
        &self,
        local: &Local<'_, '_>,
        meter: &mut Meter,
        index: usize,
    ) -> Result<Expr, Failure> {
        let mut built = Vec::with_capacity(self.fields.len());
        for (field, key, expr) in &self.fields {
            let value = match expr {
                Some(expr) => typed(
                    field,
                    expr,
                    expr.value_in(local, meter).map_err(expression)?,
                    Some(index),
                )?,
                None => zero(&field.ty),
            };
            built.push((Arc::clone(key), value));
        }
        Ok(Expr::Map(Entries::keyed(built)))
    }
}

/// `value`, which `expr` gave for `field` (of item `index`), when it is of
/// the field's type; null as the field's zero value. Only an expression
/// whose type has a `dyn` in it is checked now: the check of the block
/// accepted the type of any other.
fn typed(
    field: &pcx_expr::schema::Field,
    expr: &pcx_expr::Expression,
    value: Expr,
    index: Option<usize>,
) -> Result<Expr, Failure> {
    if matches!(value, Expr::Null) {
        return Ok(zero(&field.ty));
    }
    if expr.ty().known() || field.ty.accepts(&value.ty()) {
        return Ok(value);
    }
    let path = match index {
        None => field.name.clone(),
        Some(i) => format!("items[{i}].{}", field.name),
    };
    Err(failure(
        kinds::TEMPLATE,
        &format!("{path}: a {} is not a {}", value.ty(), field.ty),
    ))
}

impl Validate {
    fn run(
        &self,
        bound: &Bound<'_>,
        meter: &mut Meter,
        fail_invalid: bool,
    ) -> Result<Gave, Failure> {
        let mut found = Vec::new();
        let (output, items) = match &self.input {
            Input::Value(source) => {
                let value = source.value_in(&bound.alone(), meter).map_err(expression)?;
                self.check(&value, &bound.value(&value), None, meter, &mut found)?;
                (fields([("violations", listed_violations(&found))]), 0)
            }
            Input::Items(source) => {
                let items = listed(source, bound, meter)?;
                let mut valid = Vec::new();
                let mut invalid = Vec::new();
                let index = Arc::<str>::from("index");
                let broke = Arc::<str>::from("violations");
                each(
                    items.len(),
                    meter,
                    |i, meter| {
                        let value = items.value(i).map_err(internal)?;
                        let mut broken = Vec::new();
                        self.check(
                            &value,
                            &item(bound, &items, i)?,
                            Some(i),
                            meter,
                            &mut broken,
                        )?;
                        Ok((value, broken))
                    },
                    |i, (value, broken)| {
                        if broken.is_empty() {
                            valid.push(value);
                        } else {
                            invalid.push(Expr::Map(Entries::keyed([
                                (Arc::clone(&index), Expr::Int(count(i))),
                                (Arc::clone(&broke), listed_violations(&broken)),
                            ])));
                            found.extend(broken);
                        }
                        Ok(true)
                    },
                )?;
                let output = fields([
                    ("violations", listed_violations(&found)),
                    ("valid", Expr::list(valid)),
                    ("invalid", Expr::list(invalid)),
                ]);
                (output, items.len())
            }
        };
        if found.is_empty() {
            return Ok(Gave {
                output,
                port: VALID,
                items,
            });
        }
        if fail_invalid {
            return Err(failure(kinds::INVALID, &summary(&found)));
        }
        Ok(Gave {
            output,
            port: INVALID,
            items,
        })
    }

    /// What `value` (item `index`, or the one value) breaks of the schema
    /// and the rules (evaluated over `local`), into `out`; a path is written
    /// only for what broke.
    fn check(
        &self,
        value: &Expr,
        local: &Local<'_, '_>,
        index: Option<usize>,
        meter: &mut Meter,
        out: &mut Vec<Violation>,
    ) -> Result<(), Failure> {
        let before = out.len();
        if !self.schema.is_empty() {
            self.schema.violations(value, "", out);
        }
        for (n, rule) in self.rules.iter().enumerate() {
            if rule.holds.holds_in(local, meter).map_err(expression)? {
                continue;
            }
            out.push(Violation {
                path: rule.path.clone(),
                rule: format!("rules[{n}]"),
                message: rule.message.text_in(local).map_err(expression)?,
            });
        }
        if let Some(i) = index {
            for v in &mut out[before..] {
                v.path = if v.path.is_empty() {
                    format!("items[{i}]")
                } else {
                    format!("items[{i}].{}", v.path)
                };
            }
        }
        Ok(())
    }
}

/// `found` as the list of violations: each a map of `path`, `rule` and
/// `message`, as the check reads a `pcx.workflow.v1.Violation`.
fn listed_violations(found: &[Violation]) -> Expr {
    let (path, rule, message) = (
        Arc::<str>::from("path"),
        Arc::<str>::from("rule"),
        Arc::<str>::from("message"),
    );
    Expr::List(List::new(found.iter().map(|v| {
        Expr::Map(Entries::keyed([
            (Arc::clone(&path), Expr::string(v.path.as_str())),
            (Arc::clone(&rule), Expr::string(v.rule.as_str())),
            (Arc::clone(&message), Expr::string(v.message.as_str())),
        ]))
    })))
}

/// `found`, the violations of a failed `validate`, for a person: the
/// first few, and how many more.
fn summary(found: &[Violation]) -> String {
    let named: Vec<String> = found
        .iter()
        .take(NAMED)
        .map(|v| {
            if v.path.is_empty() {
                v.message.clone()
            } else {
                format!("{}: {}", v.path, v.message)
            }
        })
        .collect();
    let more = found.len().saturating_sub(NAMED);
    if more == 0 {
        named.join("; ")
    } else {
        format!("{}; and {more} more", named.join("; "))
    }
}

/// The list `source` gives, at most `MAX_ITEMS`.
fn listed(
    source: &pcx_expr::Expression,
    bound: &Bound<'_>,
    meter: &mut Meter,
) -> Result<Items, Failure> {
    let items = source.items_in(&bound.alone(), meter).map_err(expression)?;
    if items.len() > MAX_ITEMS {
        return Err(failure(
            kinds::BUDGET,
            &format!(
                "{} items: a data block reads at most {MAX_ITEMS}",
                items.len()
            ),
        ));
    }
    Ok(items)
}

/// The variables of item `i`.
fn item<'a, 'v>(bound: &'a Bound<'v>, items: &Items, i: usize) -> Result<Local<'a, 'v>, Failure> {
    bound
        .item(items, i)
        .ok_or_else(|| failure("internal", &format!("no item {i}")))
}

/// Named values: a block's output by its fields.
fn fields<const N: usize>(named: [(&str, Expr); N]) -> BTreeMap<String, Expr> {
    named.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// `n` as CEL's int: a block reads at most a million items.
fn count(n: usize) -> i64 {
    n as i64
}

/// An expression's failure: `budget` when it spent its steps.
fn expression(e: ExprError) -> Failure {
    let kind = if e.kind == ExprErrorKind::Budget {
        kinds::BUDGET
    } else {
        kinds::EXPRESSION
    };
    failure(kind, &e.to_string())
}

/// A value that is none here, out of what was read as one.
fn internal(why: String) -> Failure {
    failure("internal", &why)
}

/// A value that is not the `wanted` one.
pub fn gave(value: &Expr, wanted: &str) -> Failure {
    failure(
        kinds::EXPRESSION,
        &format!("gave a {}, not a {wanted}", value.ty()),
    )
}
