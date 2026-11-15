// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A run's next blocks from its journal (docs/modules/workflow.md, the
//! engine), pure. A graph runs one block at a time: each block leaves by
//! one port and each port has at most one edge, so a graph's state is the
//! block it is at, found by walking its journal from its start. A
//! `for_each` runs its bodies and a `parallel` its branches as graphs of
//! their own (`<block>#<n>` in the journal), and its step ends when they
//! do. Every attempt is a step; a failed one is tried again after its
//! backoff while the block's `retry` and the failure allow, then leaves by
//! `error` or fails the graph.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use pcx_expr::{Value as Expr, Values};
use post_cortex_proto::workflow::{
    Block, Failure, Graph, Join, Step, StepStatus, Workflow, block::Kind,
};

use crate::domain::graph::{ERROR, NEXT, shape};
use crate::domain::retry::{backoff, failure, kinds, retryable};
use crate::domain::value::{as_message, read};
use crate::domain::work::{self, Work};
use crate::domain::{Catalog, Checked};
use prost_reflect::DescriptorPool;

/// A step of the journal: the step without its input and output, which
/// are read once, when it is recorded (docs/modules/workflow.md, data
/// blocks, speed: a large output is not read again for each later block).
#[derive(Debug)]
pub struct Recorded {
    /// The step, its input and output taken out.
    pub step: Step,
    /// Its input, read.
    input: Option<Result<Expr, String>>,
    /// Its output, read.
    output: Option<Result<Expr, String>>,
}

/// A run's steps, by graph and block, each block's attempts in order;
/// their values read by the messages of the run's catalog.
#[derive(Debug)]
pub struct Journal {
    steps: BTreeMap<(String, String), Vec<Recorded>>,
    messages: DescriptorPool,
}

impl Journal {
    /// The journal of `steps`, read by `messages`.
    pub fn new(steps: impl IntoIterator<Item = Step>, messages: &DescriptorPool) -> Self {
        let mut journal = Self {
            steps: BTreeMap::new(),
            messages: messages.clone(),
        };
        for step in steps {
            journal.record(step);
        }
        journal
    }

    /// `step` written: a new attempt, or one that changed; its input and
    /// output read now.
    pub fn record(&mut self, mut step: Step) {
        let input = step.input.take().map(|v| read(&v, &self.messages));
        let output = step.output.take().map(|v| read(&v, &self.messages));
        let recorded = Recorded {
            step,
            input,
            output,
        };
        let attempts = self
            .steps
            .entry((recorded.step.at.clone(), recorded.step.block.clone()))
            .or_default();
        match attempts
            .iter_mut()
            .find(|s| s.step.attempt == recorded.step.attempt)
        {
            Some(known) => *known = recorded,
            None => {
                attempts.push(recorded);
                attempts.sort_by_key(|s| s.step.attempt);
            }
        }
    }

    /// The latest attempt of `block` in graph `at`.
    pub fn last(&self, at: &str, block: &str) -> Option<&Recorded> {
        self.steps
            .get(&(at.to_string(), block.to_string()))
            .and_then(|a| a.last())
    }

    /// Whether graph `at` (or one inside it) has a step.
    fn started(&self, at: &str) -> bool {
        let inside = format!("{at}/");
        self.steps
            .keys()
            .any(|(a, _)| a == at || a.starts_with(&inside))
    }
}

impl Recorded {
    /// Its output; null when it has none.
    ///
    /// # Errors
    /// It did not read; why.
    pub fn output(&self) -> Result<Expr, String> {
        self.output.clone().unwrap_or(Ok(Expr::Null))
    }

    /// Its input, when it has one.
    ///
    /// # Errors
    /// It did not read; why.
    pub fn input(&self) -> Option<Result<Expr, String>> {
        self.input.clone()
    }
}

/// What a run does next.
#[derive(Debug)]
pub enum Next {
    /// Start these (or, for a step that runs, start it again if it is not
    /// in flight: after a restart).
    Act(Vec<Act>),
    /// It succeeded with its output.
    Succeeded(Expr),
    /// It failed.
    Failed(Failure),
}

/// An attempt of a block to start, or a step to close.
#[derive(Debug)]
pub struct Act {
    /// Its graph.
    pub at: String,
    /// Its block.
    pub block: String,
    /// Its attempt.
    pub attempt: u32,
    /// Not before (a retry's backoff); none: now.
    pub not_before: Option<DateTime<Utc>>,
    /// What to do.
    pub work: Work,
}

/// A run being planned: its workflow checked, its journal, its values.
pub struct Plan<'a> {
    /// The workflow at the run's version.
    pub workflow: &'a Workflow,
    /// What its check found.
    pub checked: &'a Checked,
    /// The tools and messages it reaches.
    pub catalog: &'a Catalog,
    /// The run's input (`input.<field>`).
    pub input: &'a Values,
    /// What started it: its trigger block and the `Fired` message.
    pub trigger: (&'a str, &'a Expr),
    /// Its journal.
    pub journal: &'a Journal,
    /// Now.
    pub now: DateTime<Utc>,
}

/// How a graph's walk stands.
enum Walk {
    /// A block of it runs or waits.
    Busy,
    /// It ended: its values at the end, and when its last step ended.
    Ended(Values, Option<prost_types::Timestamp>),
    /// It failed.
    Failed(Failure),
}

impl Plan<'_> {
    /// What the run does next.
    pub fn next(&self) -> Next {
        let Some(graph) = &self.workflow.graph else {
            return Next::Failed(failure("internal", "the workflow has no graph"));
        };
        let mut acts = Vec::new();
        match self.walk(graph, "", self.trigger.0, self.input.clone(), &mut acts) {
            Walk::Busy => Next::Act(acts),
            Walk::Failed(f) => Next::Failed(f),
            Walk::Ended(values, _) => match self.output(&self.workflow.output, "", &values) {
                Ok(output) => Next::Succeeded(output),
                Err(f) => Next::Failed(f),
            },
        }
    }

    /// Graph `graph` at `at` walked from block `start` with `values`; the
    /// blocks to start into `acts`.
    fn walk(
        &self,
        graph: &Graph,
        at: &str,
        start: &str,
        mut values: Values,
        acts: &mut Vec<Act>,
    ) -> Walk {
        let edge = |from: &str, port: &str| {
            graph
                .edges
                .iter()
                .find(|e| e.from == from && (e.port == port || (e.port.is_empty() && port == NEXT)))
                .map(|e| e.to.as_str())
        };
        let mut id = start;
        loop {
            let Some(block) = graph.blocks.iter().find(|b| b.id == id) else {
                return Walk::Failed(failure("internal", &format!("no block {id}")));
            };
            let Some(recorded) = self.journal.last(at, id) else {
                acts.push(self.start(block, at, 1, &values, None));
                return Walk::Busy;
            };
            let step = &recorded.step;
            match StepStatus::try_from(step.status) {
                Ok(StepStatus::Running | StepStatus::Waiting) => {
                    match &block.kind {
                        Some(Kind::ForEach(_) | Kind::Parallel(_)) => {
                            if let Some(act) = self.inside(block, at, recorded, &values, acts) {
                                acts.push(act);
                            }
                        }
                        // A step that waits (a time, an event, a decision, a
                        // called run) goes on when the engine hears of it.
                        _ if step.status == StepStatus::Waiting as i32 => acts.push(Act {
                            at: at.to_string(),
                            block: block.id.clone(),
                            attempt: step.attempt,
                            not_before: None,
                            work: match work::awaited(self, block, step, &values) {
                                Ok(awaited) => Work::Await(awaited),
                                Err(failure) => Work::Fail { failure },
                            },
                        }),
                        _ => acts.push(self.start(block, at, step.attempt, &values, None)),
                    }
                    return Walk::Busy;
                }
                Ok(StepStatus::Succeeded) => {
                    let output = match recorded.output() {
                        Ok(output) => output,
                        Err(why) => return Walk::Failed(failure("internal", &why)),
                    };
                    if let Err(why) = put(&mut values, block, output, self.trigger.1) {
                        return Walk::Failed(failure("internal", &why));
                    }
                    let port = if step.port.is_empty() {
                        NEXT
                    } else {
                        step.port.as_str()
                    };
                    match edge(id, port) {
                        Some(next) => id = next,
                        None => return Walk::Ended(values, step.ended_at),
                    }
                }
                Ok(StepStatus::Failed | StepStatus::Unspecified) | Err(_) => {
                    let failed = step
                        .failure
                        .clone()
                        .unwrap_or_else(|| failure("internal", "a step failed with no failure"));
                    let attempts = block.retry.as_ref().map_or(1, |r| r.attempts.max(1));
                    let effect = self.checked.found.tools.get(id).map(|t| t.effect);
                    if step.attempt < attempts && retryable(effect, &failed.kind) {
                        let not_before = step
                            .ended_at
                            .as_ref()
                            .and_then(crate::domain::value::time)
                            .map(|t| t + backoff(step.attempt));
                        acts.push(self.start(block, at, step.attempt + 1, &values, not_before));
                        return Walk::Busy;
                    }
                    match edge(id, ERROR) {
                        Some(next) => {
                            match as_message(&failed, self.catalog.messages()) {
                                Ok(m) => {
                                    values.insert(format!("steps.{id}"), Expr::Message(m));
                                }
                                Err(why) => return Walk::Failed(failure("internal", &why)),
                            }
                            id = next;
                        }
                        None => return Walk::Failed(failed),
                    }
                }
            }
        }
    }

    /// A `for_each`'s bodies or a `parallel`'s branches walked; the act
    /// that closes its step once they end as it joins them.
    fn inside(
        &self,
        block: &Block,
        at: &str,
        recorded: &Recorded,
        values: &Values,
        acts: &mut Vec<Act>,
    ) -> Option<Act> {
        let step = &recorded.step;
        let close = |result: Result<Expr, Failure>| Act {
            at: at.to_string(),
            block: block.id.clone(),
            attempt: step.attempt,
            not_before: None,
            work: match result {
                Ok(output) => Work::Close { output },
                Err(failure) => Work::Fail { failure },
            },
        };
        let path = |n: usize| {
            if at.is_empty() {
                format!("{}#{n}", block.id)
            } else {
                format!("{at}/{}#{n}", block.id)
            }
        };
        match &block.kind {
            Some(Kind::ForEach(each)) => {
                let body = each.body.as_ref()?;
                let start = start_of(body)?;
                let items = match recorded.input() {
                    Some(Ok(Expr::List(items))) => items,
                    _ => {
                        return Some(close(Err(failure(
                            "internal",
                            "a for_each's step has no items",
                        ))));
                    }
                };
                // At most 16: a usize holds it.
                let concurrency = each.concurrency.max(1) as usize;
                let mut outputs = Vec::with_capacity(items.len());
                let mut busy = 0;
                for (n, item) in items.iter().enumerate() {
                    let item = match item {
                        Ok(item) => item,
                        Err(why) => return Some(close(Err(failure("internal", &why)))),
                    };
                    let at = path(n);
                    if busy >= concurrency && !self.journal.started(&at) {
                        busy += 1;
                        continue;
                    }
                    let mut values = values.clone();
                    values.insert("item".into(), item);
                    // A list has fewer items than an i64 counts.
                    values.insert("index".into(), Expr::Int(n as i64));
                    match self.walk(body, &at, start, values, acts) {
                        Walk::Busy => busy += 1,
                        Walk::Failed(f) => return Some(close(Err(f))),
                        Walk::Ended(end, _) => match self.output(&body.output, &block.id, &end) {
                            Ok(output) => outputs.push(output),
                            Err(f) => return Some(close(Err(f))),
                        },
                    }
                }
                (busy == 0).then(|| close(Ok(Expr::list(outputs))))
            }
            Some(Kind::Parallel(parallel)) => {
                let wanted = match Join::try_from(parallel.join) {
                    Ok(Join::First) => 1,
                    // At most its branches (the check): a usize holds it.
                    Ok(Join::N) => parallel.n as usize,
                    _ => parallel.branches.len(),
                };
                let mut ended: Vec<(Option<prost_types::Timestamp>, usize, Expr)> = Vec::new();
                let mut busy = 0;
                for (n, branch) in parallel.branches.iter().enumerate() {
                    let start = start_of(branch)?;
                    match self.walk(branch, &path(n), start, values.clone(), acts) {
                        Walk::Busy => busy += 1,
                        Walk::Failed(f) => return Some(close(Err(f))),
                        Walk::Ended(end, at) => {
                            match self.output(&branch.output, &format!("{}#{n}", block.id), &end) {
                                Ok(output) => ended.push((at, n, output)),
                                Err(f) => return Some(close(Err(f))),
                            }
                        }
                    }
                }
                if ended.len() < wanted && busy > 0 {
                    return None;
                }
                ended.sort_by_key(|(t, n, _)| (t.map(|t| (t.seconds, t.nanos)), *n));
                let mut outputs: Vec<Expr> =
                    ended.into_iter().take(wanted).map(|(_, _, o)| o).collect();
                Some(close(Ok(match Join::try_from(parallel.join) {
                    Ok(Join::First) => outputs.pop().unwrap_or(Expr::Null),
                    _ => Expr::list(outputs),
                })))
            }
            _ => None,
        }
    }

    /// Attempt `attempt` of `block` in graph `at`, with `values`.
    fn start(
        &self,
        block: &Block,
        at: &str,
        attempt: u32,
        values: &Values,
        not_before: Option<DateTime<Utc>>,
    ) -> Act {
        Act {
            at: at.to_string(),
            block: block.id.clone(),
            attempt,
            not_before,
            work: work::of(self, block, values),
        }
    }

    /// A graph's `output` expression over `values` where it ended, typed by
    /// the variables of its end `key`; null when it has none.
    fn output(&self, expression: &str, key: &str, values: &Values) -> Result<Expr, Failure> {
        if expression.trim().is_empty() {
            return Ok(Expr::Null);
        }
        let expr = self.checked.found.outputs.get(key).ok_or_else(|| {
            failure(
                "internal",
                &format!("the output of graph \"{key}\" was not checked"),
            )
        })?;
        expr.value(values)
            .map_err(|e| failure(kinds::EXPRESSION, &e.to_string()))
    }
}

/// The one block of a body or a branch no edge reaches.
fn start_of(graph: &Graph) -> Option<&str> {
    shape(graph, false)
        .ok()
        .and_then(|s| s.order.first().map(|b| b.id.as_str()))
}

/// `block`'s `output` put into `values`: `trigger` for a trigger, a
/// schema's or a `set`'s fields as `steps.<id>.<field>`, else
/// `steps.<id>`.
fn put(values: &mut Values, block: &Block, output: Expr, fired: &Expr) -> Result<(), String> {
    match (&block.kind, output) {
        (Some(Kind::Trigger(_)), _) => {
            values.insert("trigger".into(), fired.clone());
        }
        (
            Some(
                Kind::Model(_)
                | Kind::Agent(_)
                | Kind::Set(_)
                | Kind::Filter(_)
                | Kind::Transform(_)
                | Kind::Validate(_),
            ),
            Expr::Map(fields),
        ) => {
            for entry in fields.iter() {
                let (name, value) = entry?;
                values.insert(format!("steps.{}.{name}", block.id), value);
            }
        }
        (_, output) => {
            values.insert(format!("steps.{}", block.id), output);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
