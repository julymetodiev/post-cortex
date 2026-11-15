// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a waiting step waits for, and the test of an `event` trigger
//! (docs/modules/workflow.md, blocks: wait, approval, call; triggers).

use chrono::{DateTime, Utc};
use pcx_expr::{Expression, Value as Expr, Values};
use post_cortex_proto::workflow::{Block, EventTrigger, Failure, block::Kind, wait};

use crate::domain::Catalog;
use crate::domain::flow::Vars;
use crate::domain::retry::failure;
use crate::domain::run::Plan;
use crate::domain::work::{expression, not};

/// The bus's event message.
const BUS_EVENT: &str = "pcx.module.v1.BusEvent";

/// What a waiting step waits for.
#[derive(Debug)]
pub enum Awaited {
    /// A time: a `wait`'s, or an event's timeout (`event` none).
    Time {
        /// The time.
        until: DateTime<Utc>,
    },
    /// An event that matches, or its timeout.
    Event {
        /// Its name.
        name: String,
        /// Its filter, compiled: true for the event (`event`) with the
        /// step's values; none: any.
        filter: Option<Filter>,
        /// Its timeout; none: the run's wall time.
        until: Option<DateTime<Utc>>,
    },
    /// A decision of the board, approved.
    Decision {
        /// Its id.
        id: String,
    },
    /// A called run, ended.
    Run {
        /// Its id.
        id: String,
    },
}

/// An event filter with the values it reads.
#[derive(Debug)]
pub struct Filter {
    expr: Expression,
    values: Values,
}

impl Filter {
    /// Whether `event` (a `pcx.module.v1.BusEvent`) passes.
    ///
    /// # Errors
    /// The filter failed or gave no bool.
    pub fn passes(&self, event: Expr) -> Result<bool, Failure> {
        let mut values = self.values.clone();
        values.insert("event".into(), event);
        match self.expr.value(&values).map_err(expression)? {
            Expr::Bool(holds) => Ok(holds),
            other => Err(not(&other, "bool")),
        }
    }
}

/// An `event` trigger's test, its filter compiled once (when the engine
/// reads the workflow's triggers).
#[derive(Debug)]
pub struct Fires {
    trigger: EventTrigger,
    filter: Option<Expression>,
}

impl Fires {
    /// The test of `trigger`, its filter compiled against the bus's event
    /// message of `catalog`.
    ///
    /// # Errors
    /// The filter does not compile.
    pub fn new(trigger: EventTrigger, catalog: &Catalog) -> Result<Self, Failure> {
        let filter = if trigger.filter.trim().is_empty() {
            None
        } else {
            let inputs = Vars::default()
                .with("event", pcx_expr::Type::Message(BUS_EVENT.into()))
                .inputs(catalog)
                .map_err(|why| failure("internal", &why))?;
            Some(Expression::check(&trigger.filter, &inputs).map_err(expression)?)
        };
        Ok(Self { trigger, filter })
    }

    /// Whether `event` (named `name`, a `pcx.module.v1.BusEvent` as CEL
    /// reads it) starts a run: its name, and its filter holds.
    ///
    /// # Errors
    /// The filter failed or gave no bool.
    pub fn fires(&self, name: &str, event: Expr) -> Result<bool, Failure> {
        if self.trigger.name != name {
            return Ok(false);
        }
        let Some(filter) = &self.filter else {
            return Ok(true);
        };
        Filter {
            expr: filter.clone(),
            values: Values::new(),
        }
        .passes(event)
    }
}

/// What waiting `step` of `block` waits for, with `values` on its path.
pub fn awaited(
    plan: &Plan<'_>,
    block: &Block,
    step: &post_cortex_proto::workflow::Step,
    values: &Values,
) -> Result<Awaited, Failure> {
    let until = step
        .waits_until
        .as_ref()
        .and_then(crate::domain::value::time);
    Ok(match &block.kind {
        Some(Kind::Wait(w)) => match &w.until {
            Some(wait::Until::Event(e)) => {
                let filter = if e.filter.trim().is_empty() {
                    None
                } else {
                    Some(Filter {
                        expr: plan
                            .checked
                            .found
                            .expr(&block.id, "wait.event.filter")?
                            .clone(),
                        values: values.clone(),
                    })
                };
                Awaited::Event {
                    name: e.name.clone(),
                    filter,
                    until,
                }
            }
            _ => Awaited::Time {
                until: until.ok_or_else(|| failure("internal", "a waiting wait has no time"))?,
            },
        },
        Some(Kind::Approval(_)) => Awaited::Decision {
            id: step.waits_for.clone(),
        },
        Some(Kind::Call(_)) => Awaited::Run {
            id: step.waits_for.clone(),
        },
        _ => {
            return Err(failure(
                "internal",
                &format!("block {} does not wait", block.id),
            ));
        }
    })
}
