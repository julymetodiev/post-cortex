// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Each kind of block checked against the variables on every path to it
//! (docs/modules/workflow.md, blocks and the check): its fields, its
//! expressions and templates, and what it gives.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use pcx_expr::{Inputs, Type};
use post_cortex_proto::board::Risk;
use post_cortex_proto::workflow::{Block, Graph, Join, block::Kind, trigger, wait};

use crate::domain::data;
use crate::domain::flow::{self, Found, Out, Vars};
use crate::domain::graph;
use crate::domain::keep::Keep;
use crate::domain::slots::{self, Target};
use crate::domain::{Refusal, Signature, schema};

/// Attempts of a block at most.
const MAX_ATTEMPTS: u32 = 10;

/// Bodies of a `for_each` at once at most.
const MAX_CONCURRENCY: u32 = 16;

/// The longest one attempt of a block may take: a run's default wall time.
const MAX_TIMEOUT: Duration = Duration::from_secs(24 * 3_600);

/// The bus's event message: an `event` trigger's and a `wait`'s.
const BUS_EVENT: &str = "pcx.module.v1.BusEvent";

/// A board decision: an `approval`'s output.
const DECISION: &str = "pcx.board.v1.Decision";

/// The module an approval decides on.
pub const BOARD: &str = "board";

/// What the blocks are checked with.
pub struct Context<'a> {
    /// The tools the workflow may reach.
    pub catalog: &'a crate::domain::Catalog,
    /// The workflows it calls, by name.
    pub callees: &'a BTreeMap<String, Signature>,
    /// The modules it asks for.
    pub grants: &'a BTreeSet<&'a str>,
    /// The workflow's name (its records' package).
    pub workflow: &'a str,
}

/// What a block's check found.
pub struct Checked {
    /// What it gives.
    pub out: Out,
    /// What it (its bodies, its branches) needs at run time.
    pub found: Found,
}

/// `block`, of `kind`, checked with `vars` its variables.
///
/// # Errors
/// The first field at fault.
pub fn check(
    context: &Context<'_>,
    block: &Block,
    kind: &Kind,
    vars: &Vars,
) -> Result<Checked, Refusal> {
    let id = block.id.as_str();
    let refuse = |field: &str, message: String| Refusal::block(id, field, message);
    if let Some(retry) = &block.retry
        && (retry.attempts == 0 || retry.attempts > MAX_ATTEMPTS)
    {
        return Err(refuse(
            "retry.attempts",
            format!("is 1 to {MAX_ATTEMPTS}, the first attempt included"),
        ));
    }
    if let Some(timeout) = &block.timeout {
        positive(timeout, MAX_TIMEOUT).map_err(|why| refuse("timeout", why))?;
    }
    let inputs = vars
        .inputs(context.catalog)
        .map_err(|why| refuse("kind", why))?;
    let mut keep = Keep {
        id,
        inputs,
        found: Found::default(),
    };
    let with_event = |vars: &Vars, field: &str| {
        vars.with("event", Type::Message(BUS_EVENT.into()))
            .inputs(context.catalog)
            .map_err(|why| refuse(field, why))
    };
    Ok(match kind {
        Kind::Trigger(t) => {
            match &t.kind {
                None => return Err(refuse("trigger.kind", "a trigger has a kind".to_string())),
                Some(trigger::Kind::Webhook(w)) if w.source.trim().is_empty() => {
                    return Err(refuse("trigger.webhook.source", "is empty".to_string()));
                }
                Some(trigger::Kind::Event(e)) => {
                    if e.name.trim().is_empty() {
                        return Err(refuse("trigger.event.name", "is empty".to_string()));
                    }
                    if !e.filter.trim().is_empty() {
                        let inputs = with_event(&Vars::default(), "trigger.event.filter")?;
                        keep.condition_in("trigger.event.filter", &e.filter, &inputs)?;
                    }
                }
                Some(_) => {}
            }
            keep.gives(Out::One(Type::Null))
        }
        Kind::Tool(tool) => {
            let found = context.catalog.tool(&tool.method).ok_or_else(|| {
                refuse(
                    "tool.method",
                    format!(
                        "{} is no tool of a running module its author may call",
                        tool.method
                    ),
                )
            })?;
            if !context.grants.contains(found.module.as_str()) {
                return Err(refuse(
                    "tool.method",
                    format!(
                        "its module {} is not in the workflow's grants",
                        found.module
                    ),
                ));
            }
            if let Some(request) = &tool.request {
                let texts = slots::check(
                    request,
                    &Target::Message(found.request.clone()),
                    &keep.inputs,
                )
                .map_err(|(field, why)| refuse(&format!("tool.request.{field}"), why))?;
                keep.found
                    .templates
                    .insert((id.to_string(), "tool.request".to_string()), texts);
            }
            if let Some(fixture) = &tool.fixture {
                let texts = slots::check(
                    fixture,
                    &Target::Message(found.response.clone()),
                    &keep.inputs,
                )
                .map_err(|(field, why)| refuse(&format!("tool.fixture.{field}"), why))?;
                keep.found
                    .templates
                    .insert((id.to_string(), "tool.fixture".to_string()), texts);
            }
            keep.found.tools.insert(id.to_string(), found.clone());
            keep.gives(Out::One(Type::Message(
                found.response.full_name().to_string(),
            )))
        }
        Kind::Model(model) => {
            keep.text("model.system", &model.system, false)?;
            keep.text("model.prompt", &model.prompt, true)?;
            let out = answer(context, model.output.as_ref(), "model.output", id)?;
            keep.gives(out)
        }
        Kind::Agent(agent) => {
            // The agent's own turn is given the tools of the grants' modules:
            // with none it would be given every tool the agent has.
            if agent.member.is_empty() && context.grants.is_empty() {
                return Err(refuse(
                    "agent.member",
                    "a turn of the host's agent uses the tools of the modules in the workflow's grants, which name none: name a member, a module in grants, or use a model block".to_string(),
                ));
            }
            keep.text("agent.prompt", &agent.prompt, true)?;
            let out = answer(context, agent.output.as_ref(), "agent.output", id)?;
            keep.gives(out)
        }
        Kind::Call(call) => {
            let callee = context.callees.get(&call.workflow).ok_or_else(|| {
                refuse(
                    "call.workflow",
                    format!("\"{}\" is no workflow", call.workflow),
                )
            })?;
            let input = call.input.clone().unwrap_or_default();
            let texts = slots::check(&input, &Target::Fields(&callee.input), &keep.inputs)
                .map_err(|(field, why)| refuse(&format!("call.input.{field}"), why))?;
            keep.found
                .templates
                .insert((id.to_string(), "call.input".to_string()), texts);
            keep.found
                .calls
                .insert(id.to_string(), callee.input.clone());
            keep.gives(Out::One(callee.output.clone()))
        }
        Kind::Branch(branch) => {
            keep.condition("branch.condition", &branch.condition)?;
            keep.gives(Out::One(Type::Bool))
        }
        Kind::Switch(switch) => {
            if switch.cases.is_empty() {
                return Err(refuse("switch.cases", "a switch has a case".to_string()));
            }
            for (n, case) in switch.cases.iter().enumerate() {
                keep.condition(&format!("switch.cases[{n}]"), case)?;
            }
            keep.gives(Out::One(Type::Int))
        }
        Kind::ForEach(each) => {
            if each.concurrency > MAX_CONCURRENCY {
                return Err(refuse(
                    "for_each.concurrency",
                    format!("is at most {MAX_CONCURRENCY}"),
                ));
            }
            let item = match keep.expr("for_each.items", &each.items)? {
                Type::List(item) => *item,
                Type::Dyn => Type::Dyn,
                other => {
                    return Err(refuse(
                        "for_each.items",
                        format!("gives a {other}, not a list"),
                    ));
                }
            };
            let body = each
                .body
                .as_ref()
                .ok_or_else(|| refuse("for_each.body", "a for_each has a body".to_string()))?;
            let entry = vars.with("item", item).with("index", Type::Int);
            let (output, found) = nested(context, body, entry, id, id, "for_each.body")?;
            keep.found.extend(found);
            keep.gives(Out::One(Type::List(Box::new(output))))
        }
        Kind::Parallel(parallel) => {
            if parallel.branches.is_empty() {
                return Err(refuse(
                    "parallel.branches",
                    "a parallel has a branch".to_string(),
                ));
            }
            let mut outputs = Vec::with_capacity(parallel.branches.len());
            for (n, branch) in parallel.branches.iter().enumerate() {
                let (output, branch_found) = nested(
                    context,
                    branch,
                    vars.clone(),
                    id,
                    &format!("{id}#{n}"),
                    &format!("parallel.branches[{n}]"),
                )?;
                outputs.push(output);
                keep.found.extend(branch_found);
            }
            let one = match outputs.as_slice() {
                [first, rest @ ..] if rest.iter().all(|o| o == first) => first.clone(),
                _ => Type::Dyn,
            };
            let out = match Join::try_from(parallel.join) {
                Ok(Join::All) => Type::List(Box::new(one)),
                Ok(Join::First) => one,
                Ok(Join::N) => {
                    let n = usize::try_from(parallel.n).unwrap_or(usize::MAX);
                    if n == 0 || n > parallel.branches.len() {
                        return Err(refuse(
                            "parallel.n",
                            format!("is 1 to its {} branches", parallel.branches.len()),
                        ));
                    }
                    Type::List(Box::new(one))
                }
                Ok(Join::Unspecified) | Err(_) => {
                    return Err(refuse("parallel.join", "is all, first or n".to_string()));
                }
            };
            keep.gives(Out::One(out))
        }
        Kind::Wait(w) => {
            let out = match &w.until {
                None => {
                    return Err(refuse(
                        "wait.until",
                        "a wait has a duration, a time or an event".to_string(),
                    ));
                }
                Some(wait::Until::Duration(d)) => {
                    positive(d, MAX_TIMEOUT).map_err(|why| refuse("wait.duration", why))?;
                    Type::Null
                }
                Some(wait::Until::At(at)) => {
                    let ty = keep.expr("wait.at", at)?;
                    if !Type::Timestamp.accepts(&ty) {
                        return Err(refuse("wait.at", format!("gives a {ty}, not a timestamp")));
                    }
                    Type::Null
                }
                Some(wait::Until::Event(e)) => {
                    if e.name.trim().is_empty() {
                        return Err(refuse("wait.event.name", "is empty".to_string()));
                    }
                    if !e.filter.trim().is_empty() {
                        let inputs = with_event(vars, "wait.event.filter")?;
                        keep.condition_in("wait.event.filter", &e.filter, &inputs)?;
                    }
                    Type::Message(BUS_EVENT.into())
                }
            };
            if let Some(timeout) = &w.timeout {
                positive(timeout, MAX_TIMEOUT).map_err(|why| refuse("wait.timeout", why))?;
            }
            keep.gives(Out::One(out))
        }
        Kind::Approval(approval) => {
            if !context.grants.contains(BOARD) {
                return Err(refuse(
                    "approval",
                    "an approval is a decision on the board: the workflow's grants have the board"
                        .to_string(),
                ));
            }
            keep.text("approval.question", &approval.question, true)?;
            if !approval.task.trim().is_empty() {
                let ty = keep.expr("approval.task", &approval.task)?;
                if !Type::String.accepts(&ty) {
                    return Err(refuse(
                        "approval.task",
                        format!("gives a {ty}, not a task's id"),
                    ));
                }
            }
            match Risk::try_from(approval.risk) {
                Ok(Risk::Unspecified | Risk::None) | Err(_) => {
                    return Err(refuse(
                        "approval.risk",
                        "is code, money, data or operations: what needs a person's approval"
                            .to_string(),
                    ));
                }
                Ok(_) => {}
            }
            keep.gives(Out::One(Type::Message(DECISION.into())))
        }
        Kind::Set(set) => {
            if set.values.is_empty() {
                return Err(refuse("set.values", "a set has a value".to_string()));
            }
            let mut fields = Vec::with_capacity(set.values.len());
            for (name, source) in &set.values {
                let field = format!("set.values.{name}");
                Inputs::new([(name.clone(), Type::Dyn)]).map_err(|e| refuse(&field, e.message))?;
                let ty = keep.expr(&field, source)?;
                fields.push((name.clone(), ty));
            }
            keep.gives(Out::Fields(fields))
        }
        Kind::StateGet(get) => {
            keep.text("state_get.key", &get.key, true)?;
            keep.gives(Out::One(Type::Dyn))
        }
        Kind::StatePut(put) => {
            keep.text("state_put.key", &put.key, true)?;
            keep.expr("state_put.value", &put.value)?;
            keep.gives(Out::One(Type::Null))
        }
        Kind::StateDelete(delete) => {
            keep.text("state_delete.key", &delete.key, true)?;
            keep.gives(Out::One(Type::Null))
        }
        Kind::Filter(f) => {
            let out = data::filter(context, &mut keep, vars, f)?;
            keep.gives(out)
        }
        Kind::Transform(t) => {
            let out = data::transform(context, &mut keep, vars, t)?;
            keep.gives(out)
        }
        Kind::Validate(v) => {
            let out = data::validate(context, &mut keep, vars, v)?;
            keep.gives(out)
        }
    })
}

/// A `model`'s or an `agent`'s answer: the fields of its output schema.
fn answer(
    context: &Context<'_>,
    output: Option<&post_cortex_proto::workflow::Schema>,
    at: &str,
    id: &str,
) -> Result<Out, Refusal> {
    let fields = schema::fields(output, context.catalog.messages())
        .map_err(|(field, why)| Refusal::block(id, format!("{at}.{field}"), why))?;
    if fields.is_empty() {
        return Err(Refusal::block(id, at, "an answer has a field"));
    }
    Ok(Out::Fields(
        fields.into_iter().map(|f| (f.name, f.ty)).collect(),
    ))
}

/// A body's or a branch's `graph` checked from `entry`: the type of its
/// output, and what its blocks need at run time, its end's variables by
/// `key`.
fn nested(
    context: &Context<'_>,
    graph: &Graph,
    entry: Vars,
    id: &str,
    key: &str,
    at: &str,
) -> Result<(Type, Found), Refusal> {
    let shape = graph::shape(graph, false).map_err(|r| match r.block {
        Some(_) => r,
        None => Refusal::block(id, format!("{at}.{}", r.field), r.message),
    })?;
    let checked = flow::check(context, &shape, entry)?;
    let mut found = checked.found;
    let output = if graph.output.trim().is_empty() {
        Type::Null
    } else {
        let expr = flow::output(context.catalog, &graph.output, &checked.ends)
            .map_err(|why| Refusal::block(id, format!("{at}.output"), why))?;
        let ty = expr.ty().clone();
        found.outputs.insert(key.to_string(), expr);
        ty
    };
    found.ends.insert(key.to_string(), checked.ends);
    Ok((output, found))
}

/// That `duration` is above zero and at most `max`.
fn positive(duration: &prost_types::Duration, max: Duration) -> Result<(), String> {
    let fits = Duration::try_from(*duration)
        .ok()
        .filter(|d| !d.is_zero() && *d <= max);
    match fits {
        Some(_) => Ok(()),
        None => Err(format!("is above 0 and at most {} s", max.as_secs())),
    }
}
