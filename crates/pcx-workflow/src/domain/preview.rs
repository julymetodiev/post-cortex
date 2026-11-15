// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A data block previewed over a run's steps (docs/modules/workflow.md,
//! data blocks, the editor): the run's journal without the block's step,
//! the steps of the `for_each` and `parallel` blocks around it open again,
//! walked as a run is, so the block gets the values a run gives it; then
//! the block runs over them, a violation shown rather than failed.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use pcx_expr::{Value as Expr, Values};
use post_cortex_proto::workflow::{Failure, Step, StepStatus, Workflow, block::Kind};

use crate::domain::data::Gave;
use crate::domain::retry::failure;
use crate::domain::run::{Journal, Next, Plan};
use crate::domain::work::Work;
use crate::domain::{Catalog, Checked};

/// What a preview reads: the run's workflow checked, its input and
/// trigger, its steps.
pub struct Previewing<'a> {
    /// The workflow.
    pub workflow: &'a Workflow,
    /// What its check found.
    pub checked: &'a Checked,
    /// What it reaches.
    pub catalog: &'a Catalog,
    /// The run's input (`input.<field>`).
    pub input: &'a Values,
    /// What started it (`trigger`).
    pub fired: &'a Expr,
    /// Now.
    pub now: DateTime<Utc>,
}

impl Previewing<'_> {
    /// Block `block` of graph `at` run over the values `steps` give it:
    /// what it gave, or its own failure.
    ///
    /// # Errors
    /// It is no data block, or the steps do not reach it; why.
    pub fn preview(
        &self,
        block: &str,
        at: &str,
        steps: Vec<Step>,
    ) -> Result<Result<Gave, Failure>, Failure> {
        let around = enclosing(at);
        let journal = Journal::new(
            steps
                .into_iter()
                .filter(|s| !(s.at == at && s.block == block))
                .map(|mut s| {
                    if around.contains(&(s.at.clone(), s.block.clone())) {
                        s.status = StepStatus::Running.into();
                        s.output = None;
                    }
                    s
                }),
            self.catalog.messages(),
        );
        let trigger = self
            .workflow
            .graph
            .iter()
            .flat_map(|g| &g.blocks)
            .find(|b| matches!(b.kind, Some(Kind::Trigger(_))) && journal.last("", &b.id).is_some())
            .map(|b| b.id.clone())
            .ok_or_else(|| unreached(block, "no trigger has a step"))?;
        let plan = Plan {
            workflow: self.workflow,
            checked: self.checked,
            catalog: self.catalog,
            input: self.input,
            trigger: (&trigger, self.fired),
            journal: &journal,
            now: self.now,
        };
        let Next::Act(acts) = plan.next() else {
            return Err(unreached(block, "the run ends before it"));
        };
        let act = acts
            .into_iter()
            .find(|a| a.at == at && a.block == block)
            .ok_or_else(|| unreached(block, "the steps do not lead to it"))?;
        match act.work {
            Work::Data { data, values, .. } => Ok(data.run(&values, false)),
            Work::Fail { failure } => Ok(Err(failure)),
            _ => Err(failure(
                "invalid_argument",
                &format!("block {block} is no filter, transform or validate"),
            )),
        }
    }
}

/// The steps that hold graph `at` open: for `a#0/b#2`, `a` in the
/// workflow's graph and `b` in `a#0`.
fn enclosing(at: &str) -> BTreeSet<(String, String)> {
    let mut around = BTreeSet::new();
    let mut outer = String::new();
    for part in at.split('/').filter(|p| !p.is_empty()) {
        let block = part.split_once('#').map_or(part, |(b, _)| b);
        around.insert((outer.clone(), block.to_string()));
        if !outer.is_empty() {
            outer.push('/');
        }
        outer.push_str(part);
    }
    around
}

/// The steps do not reach `block`: why.
fn unreached(block: &str, why: &str) -> Failure {
    failure(
        "invalid_argument",
        &format!("block {block} cannot be previewed over these steps: {why}"),
    )
}
