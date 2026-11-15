// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What one attempt of a block does, worked out from the values on its
//! path (docs/modules/workflow.md, blocks): a request filled, a prompt
//! written, a condition's port, a time to wait until. A flow block's work
//! is done here (`Done`); an action's is the engine's, through its ports.

mod awaited;

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use pcx_expr::{Escape, Value as Expr, Values};
use pcx_module_api::Effect;
use post_cortex_proto::workflow::{Block, Failure, block::Kind, wait};
use prost_reflect::DynamicMessage;

use crate::domain::data::Data;
use crate::domain::fill;
use crate::domain::graph::{self, INVALID, NEXT};
use crate::domain::retry::{failure, kinds};
use crate::domain::run::Plan;
use crate::domain::schema;
use pcx_expr::schema::Field;

pub use awaited::{Awaited, Fires, awaited};

/// What an attempt does.
#[derive(Debug)]
pub enum Work {
    /// Call a module's RPC.
    Tool {
        /// Its gRPC path.
        path: String,
        /// What calling it does.
        effect: Effect,
        /// Its request.
        request: DynamicMessage,
        /// What a dry run answers for it: its fixture, or its response's
        /// zero values.
        fixture: DynamicMessage,
    },
    /// Ask a model, its answer through one forced tool.
    Model {
        /// The model; empty: the host's default.
        model: String,
        /// The system prompt.
        system: String,
        /// The prompt.
        prompt: String,
        /// What it answers.
        output: Vec<Field>,
    },
    /// Run a turn of a team member (none: the host's agent).
    Agent {
        /// The member; empty: the host's agent.
        member: String,
        /// The prompt.
        prompt: String,
        /// What it answers.
        output: Vec<Field>,
    },
    /// Run another workflow and wait for it.
    Call {
        /// Its name.
        workflow: String,
        /// Its input.
        input: BTreeMap<String, Expr>,
    },
    /// Wait until a time.
    Sleep {
        /// The time.
        until: DateTime<Utc>,
    },
    /// Wait for an event of the bus that matches (its name and filter are
    /// its block's, read when it waits).
    Event {
        /// The time it fails at, unheard; none: the run's wall time.
        until: Option<DateTime<Utc>>,
    },
    /// Ask a decision on the board and wait until it is approved.
    Approval {
        /// What is decided.
        question: String,
        /// What it risks (`pcx.board.v1.Risk`).
        risk: i32,
        /// The task; none: the run's.
        task: Option<String>,
        /// Whether only a person approves it, whatever the team's trust.
        person: bool,
    },
    /// Read a value of the workflow's store.
    StateGet {
        /// Its key.
        key: String,
    },
    /// Write a value of the workflow's store.
    StatePut {
        /// Its key.
        key: String,
        /// Its value.
        value: Expr,
    },
    /// Remove a value of the workflow's store.
    StateDelete {
        /// Its key.
        key: String,
    },
    /// A data block to run (off the engine's threads): `data` over
    /// `values`, failing on a violation when `fail_invalid`.
    Data {
        /// The block, compiled.
        data: Arc<Data>,
        /// The values on its path.
        values: Values,
        /// Whether its `invalid` port has no edge.
        fail_invalid: bool,
    },
    /// Nothing to do outside: it ends with `output` by `port`.
    Done {
        /// Its output.
        output: Expr,
        /// Its port.
        port: String,
    },
    /// A `for_each`'s or a `parallel`'s step opens, `input` its items.
    Open {
        /// The items (a `for_each`'s); null for a `parallel`.
        input: Expr,
    },
    /// A `for_each`'s or a `parallel`'s bodies ended: its step succeeds.
    Close {
        /// The bodies' outputs.
        output: Expr,
    },
    /// It fails before it does anything (a template that does not fill).
    Fail {
        /// Why.
        failure: Failure,
    },
    /// Its step waits; it goes on when this comes.
    Await(Awaited),
}

/// What attempting `block` does with `values` on its path.
pub fn of(plan: &Plan<'_>, block: &Block, values: &Values) -> Work {
    worked(plan, block, values).unwrap_or_else(|failure| Work::Fail { failure })
}

fn worked(plan: &Plan<'_>, block: &Block, values: &Values) -> Result<Work, Failure> {
    let id = block.id.as_str();
    let found = &plan.checked.found;
    let text = |field: &str| {
        found
            .text(id, field)?
            .fill(values, Escape::None)
            .map_err(expression)
    };
    let value = |field: &str| found.expr(id, field)?.value(values).map_err(expression);
    let fields = |output| {
        schema::fields(output, plan.catalog.messages())
            .map_err(|(field, why)| failure("internal", &format!("{field}: {why}")))
    };
    let done = |output: Expr, port: &str| Work::Done {
        output,
        port: port.to_string(),
    };
    let data = || -> Result<Work, Failure> {
        let data = found
            .data
            .get(id)
            .ok_or_else(|| failure("internal", &format!("block {id} was not checked")))?;
        let graph = plan
            .workflow
            .graph
            .as_ref()
            .ok_or_else(|| failure("internal", "the workflow has no graph"))?;
        Ok(Work::Data {
            data: Arc::clone(data),
            values: values.clone(),
            fail_invalid: !graph::has_edge(graph, id, INVALID),
        })
    };
    Ok(match &block.kind {
        None => return Err(failure("internal", "a block has a kind")),
        Some(Kind::Trigger(_)) => done(Expr::Null, NEXT),
        Some(Kind::Tool(tool)) => {
            let found = plan.checked.found.tools.get(id).ok_or_else(|| {
                failure("internal", &format!("block {id}'s tool was not checked"))
            })?;
            let with = |field: &str| -> Result<fill::With<'_>, Failure> {
                Ok(fill::With {
                    texts: plan.checked.found.template(id, field).ok_or_else(|| {
                        failure("internal", &format!("block {id}'s {field} was not checked"))
                    })?,
                    messages: plan.catalog.messages(),
                    values,
                })
            };
            let request = match &tool.request {
                Some(template) => {
                    fill::message(template, &found.request, &with("tool.request")?, "").map_err(
                        |(field, why)| failure(kinds::TEMPLATE, &format!("{field}: {why}")),
                    )?
                }
                None => DynamicMessage::new(found.request.clone()),
            };
            let fixture = match &tool.fixture {
                Some(template) => {
                    fill::message(template, &found.response, &with("tool.fixture")?, "").map_err(
                        |(field, why)| failure(kinds::TEMPLATE, &format!("fixture.{field}: {why}")),
                    )?
                }
                None => DynamicMessage::new(found.response.clone()),
            };
            Work::Tool {
                path: tool.method.clone(),
                effect: found.effect,
                request,
                fixture,
            }
        }
        Some(Kind::Model(model)) => Work::Model {
            model: model.model.clone(),
            system: text("model.system")?,
            prompt: text("model.prompt")?,
            output: fields(model.output.as_ref())?,
        },
        Some(Kind::Agent(agent)) => Work::Agent {
            member: agent.member.clone(),
            prompt: text("agent.prompt")?,
            output: fields(agent.output.as_ref())?,
        },
        Some(Kind::Call(call)) => {
            let callee = plan.checked.found.calls.get(id).ok_or_else(|| {
                failure("internal", &format!("block {id}'s call was not checked"))
            })?;
            let template = call.input.clone().unwrap_or_default();
            let with = fill::With {
                texts: plan
                    .checked
                    .found
                    .template(id, "call.input")
                    .ok_or_else(|| {
                        failure(
                            "internal",
                            &format!("block {id}'s call.input was not checked"),
                        )
                    })?,
                messages: plan.catalog.messages(),
                values,
            };
            Work::Call {
                workflow: call.workflow.clone(),
                input: fill::input(&template, callee, &with)
                    .map_err(|(field, why)| failure(kinds::TEMPLATE, &format!("{field}: {why}")))?,
            }
        }
        Some(Kind::Branch(_)) => match value("branch.condition")? {
            Expr::Bool(holds) => done(Expr::Bool(holds), if holds { "then" } else { "else" }),
            other => return Err(not(&other, "bool")),
        },
        Some(Kind::Switch(switch)) => {
            for n in 0..switch.cases.len() {
                match value(&format!("switch.cases[{n}]"))? {
                    Expr::Bool(true) => {
                        // A switch has fewer cases than an i64 counts.
                        return Ok(done(Expr::Int(n as i64), &format!("case:{n}")));
                    }
                    Expr::Bool(false) => {}
                    other => return Err(not(&other, "bool")),
                }
            }
            done(Expr::Int(-1), "default")
        }
        Some(Kind::ForEach(_)) => match value("for_each.items")? {
            Expr::List(items) => Work::Open {
                input: Expr::List(items),
            },
            other => return Err(not(&other, "list")),
        },
        Some(Kind::Parallel(_)) => Work::Open { input: Expr::Null },
        Some(Kind::Wait(w)) => match &w.until {
            None => {
                return Err(failure(
                    "internal",
                    "a wait has a duration, a time or an event",
                ));
            }
            Some(wait::Until::Duration(d)) => Work::Sleep {
                until: plan.now + duration(d)?,
            },
            Some(wait::Until::At(_)) => match value("wait.at")? {
                Expr::Timestamp(until) => Work::Sleep { until },
                other => return Err(not(&other, "timestamp")),
            },
            Some(wait::Until::Event(_)) => Work::Event {
                until: w
                    .timeout
                    .as_ref()
                    .map(duration)
                    .transpose()?
                    .map(|d| plan.now + d),
            },
        },
        Some(Kind::Approval(approval)) => Work::Approval {
            question: text("approval.question")?,
            risk: approval.risk,
            task: if approval.task.trim().is_empty() {
                None
            } else {
                match value("approval.task")? {
                    Expr::String(task) => Some(task.to_string()),
                    other => return Err(not(&other, "string")),
                }
            },
            person: approval.person,
        },
        Some(Kind::Set(set)) => {
            let mut out = BTreeMap::new();
            for name in set.values.keys() {
                out.insert(name.clone(), value(&format!("set.values.{name}"))?);
            }
            done(Expr::map(out), NEXT)
        }
        Some(Kind::StateGet(_)) => Work::StateGet {
            key: text("state_get.key")?,
        },
        Some(Kind::StatePut(_)) => Work::StatePut {
            key: text("state_put.key")?,
            value: value("state_put.value")?,
        },
        Some(Kind::StateDelete(_)) => Work::StateDelete {
            key: text("state_delete.key")?,
        },
        Some(Kind::Filter(_) | Kind::Transform(_) | Kind::Validate(_)) => data()?,
    })
}

/// An expression's failure.
fn expression(e: pcx_expr::ExprError) -> Failure {
    failure(kinds::EXPRESSION, &e.to_string())
}

/// A value that is not the type `wanted`.
fn not(value: &Expr, wanted: &str) -> Failure {
    failure(
        kinds::EXPRESSION,
        &format!("gave a {}, not a {wanted}", value.ty()),
    )
}

/// A proto duration as chrono's.
fn duration(d: &prost_types::Duration) -> Result<TimeDelta, Failure> {
    TimeDelta::try_seconds(d.seconds)
        .and_then(|s| s.checked_add(&TimeDelta::nanoseconds(d.nanos.into())))
        .ok_or_else(|| failure("internal", "a duration out of range"))
}
