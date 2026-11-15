// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The workflow's domain (docs/modules/workflow.md, layers), pure: a
//! workflow's graph and the check it passes when it is written and before
//! every run. The check reads the tools the workflow may reach (a
//! [`Catalog`], their messages by their descriptors) and the workflows it
//! calls ([`Signature`]s); expressions go through `pcx-expr`.

mod blocks;
mod data;
mod fill;
mod flow;
pub mod grants;
mod graph;
mod keep;
mod preview;
mod records;
mod retry;
mod run;
mod schema;
mod slots;
pub mod template;
mod value;
mod work;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use pcx_expr::schema::Field;
use pcx_module_api::Effect;
use pcx_module_api::name::{NAME_CHARS, is_slug};
use post_cortex_proto::workflow::{Run, RunStatus, Workflow};
use prost_reflect::{DescriptorPool, MessageDescriptor};

pub use data::Data;
pub use flow::Found;
pub use preview::Previewing;
pub use retry::{failure, kinds};
pub use run::{Act, Journal, Next, Plan};
pub use schema::read_input;
pub use value::{
    as_heard, as_message, as_named, read as read_value, time, timestamp, write as write_value,
    write_map,
};
pub use work::{Awaited, Fires, Work};

/// The module's id, and the prefix of its document kinds.
pub const MODULE_ID: &str = "workflow";

/// The kind of a workflow's documents in the module.
pub const WORKFLOW_KIND: &str = "workflow";

/// The kind of a run's documents in the module (not searched).
pub const RUN_KIND: &str = "run";

/// The kind of a run's journal's documents in the module (not searched).
pub const STEP_KIND: &str = "step";

/// The kind of a workflow's store's documents in the module (not
/// searched).
pub const STATE_KIND: &str = "state";

/// A document kind of the module as memory keeps it, `workflow/<kind>`.
#[must_use]
pub fn kind(of: &str) -> String {
    format!("{MODULE_ID}/{of}")
}

/// The statuses of a run that has not ended.
pub const OPEN: [RunStatus; 2] = [RunStatus::Running, RunStatus::Waiting];

/// Whether `run` has not ended.
#[must_use]
pub fn open(run: &Run) -> bool {
    RunStatus::try_from(run.status).is_ok_and(|status| OPEN.contains(&status))
}

/// Characters of a workflow's description at most.
const MAX_DESCRIPTION: usize = 4_000;

/// Why a workflow does not pass the check: the block (none: the workflow
/// itself) and the field at fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The block's id; none for a field of the workflow.
    pub block: Option<String>,
    /// The field (`request.task.title`, `condition`, `graph.edges[2]`).
    pub field: String,
    /// What is wrong, for a person.
    pub message: String,
}

impl Refusal {
    /// A refusal of the workflow's own `field`.
    pub fn workflow(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            block: None,
            field: field.into(),
            message: message.into(),
        }
    }

    /// A refusal of `field` of block `block`.
    pub fn block(block: &str, field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            block: Some(block.to_string()),
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.block {
            Some(block) => write!(f, "block {block}, {}: {}", self.field, self.message),
            None => write!(f, "{}: {}", self.field, self.message),
        }
    }
}

impl std::error::Error for Refusal {}

/// A tool a workflow may call: an RPC of a running module.
#[derive(Debug, Clone)]
pub struct Tool {
    /// Its tool's name (`task_add`).
    pub name: String,
    /// What it does, for a model.
    pub description: String,
    /// The JSON Schema of its request, as the host's tools read it.
    pub request_schema: serde_json::Map<String, serde_json::Value>,
    /// The JSON Schema of its response.
    pub response_schema: serde_json::Map<String, serde_json::Value>,
    /// The module that offers it.
    pub module: String,
    /// What calling it does.
    pub effect: Effect,
    /// Its request message.
    pub request: MessageDescriptor,
    /// Its response message.
    pub response: MessageDescriptor,
}

/// The tools a workflow may reach, by their gRPC paths (its author's when
/// it is written, its own scope's before a run), and the messages of them
/// all and of the workflow's own package in one pool.
#[derive(Debug, Clone)]
pub struct Catalog {
    tools: BTreeMap<String, Tool>,
    messages: DescriptorPool,
}

impl Catalog {
    /// The catalog of `tools`, by path: their messages read again from one
    /// pool of the workflow's descriptors and every tool's module's (a file
    /// two modules both carry is kept once).
    ///
    /// # Errors
    /// The descriptors do not read together; the reason.
    pub fn new(tools: impl IntoIterator<Item = (String, Tool)>) -> Result<Self, String> {
        let mut messages = DescriptorPool::decode(post_cortex_proto::WORKFLOW_DESCRIPTOR_SET)
            .map_err(|e| format!("the workflow's descriptors do not read: {e}"))?;
        let tools: Vec<_> = tools.into_iter().collect();
        for (path, tool) in &tools {
            messages
                .add_file_descriptor_protos(
                    tool.request.parent_pool().file_descriptor_protos().cloned(),
                )
                .map_err(|e| {
                    format!("{path}: its module's descriptors do not read with the others: {e}")
                })?;
        }
        let find = |pool: &DescriptorPool, m: &MessageDescriptor| {
            pool.get_message_by_name(m.full_name())
                .ok_or_else(|| format!("{} is not in the pool", m.full_name()))
        };
        let tools = tools
            .into_iter()
            .map(|(path, tool)| {
                Ok((
                    path,
                    Tool {
                        request: find(&messages, &tool.request)?,
                        response: find(&messages, &tool.response)?,
                        ..tool
                    },
                ))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self { tools, messages })
    }

    /// The tool at gRPC path `path`.
    pub fn tool(&self, path: &str) -> Option<&Tool> {
        self.tools.get(path)
    }

    /// The modules of its tools.
    pub fn modules(&self) -> BTreeSet<&str> {
        self.tools.values().map(|t| t.module.as_str()).collect()
    }

    /// The messages of the tools and the workflow's package.
    pub fn messages(&self) -> &DescriptorPool {
        &self.messages
    }
}

/// What a called workflow takes and gives: its input's fields and its
/// output's type.
#[derive(Debug, Clone)]
pub struct Signature {
    /// Its input's fields.
    pub input: Vec<Field>,
    /// Its output's type.
    pub output: pcx_expr::Type,
}

/// What the check found: the workflow's signature, and what a run reads
/// by block (each `tool` block's module and effect, which the dry run, the
/// approvals and the retries read; each block's variables; each graph's
/// end's, the workflow's by the key `""`).
#[derive(Debug, Clone)]
pub struct Checked {
    /// What it takes and gives.
    pub signature: Signature,
    /// What its blocks need at run time.
    pub found: Found,
}

/// `workflow` checked against what it may reach (`catalog`) and the
/// workflows it calls (`callees`, by name): its name and description,
/// its input, its grants (a subset of the catalog's modules), its graph
/// (one start per trigger, no cycles, every block reached, every edge's
/// ends in its graph), and every block's fields and expressions against
/// what precedes it.
///
/// # Errors
/// The first rule it breaks, naming the block and the field.
pub fn check(
    workflow: &Workflow,
    catalog: &Catalog,
    callees: &BTreeMap<String, Signature>,
) -> Result<Checked, Refusal> {
    if !is_slug(&workflow.name) {
        return Err(Refusal::workflow(
            "name",
            format!(
                "\"{}\" is not 1 to {NAME_CHARS} of a-z, 0-9 and -",
                workflow.name
            ),
        ));
    }
    if workflow.description.chars().count() > MAX_DESCRIPTION {
        return Err(Refusal::workflow(
            "description",
            format!("is over {MAX_DESCRIPTION} characters"),
        ));
    }
    let input = schema::fields(workflow.input.as_ref(), catalog.messages())
        .map_err(|(field, message)| Refusal::workflow(format!("input.{field}"), message))?;
    let modules = catalog.modules();
    let mut grants = BTreeSet::new();
    for (i, module) in workflow.grants.iter().flat_map(|g| &g.modules).enumerate() {
        if !modules.contains(module.as_str()) {
            return Err(Refusal::workflow(
                format!("grants.modules[{i}]"),
                format!("{module} is not a module whose tools its author may call"),
            ));
        }
        if !grants.insert(module.as_str()) {
            return Err(Refusal::workflow(
                format!("grants.modules[{i}]"),
                format!("names {module} twice"),
            ));
        }
    }
    let graph = workflow
        .graph
        .as_ref()
        .ok_or_else(|| Refusal::workflow("graph", "a workflow has a graph"))?;
    let shape = graph::shape(graph, true)?;
    graph::placed(graph, &workflow.layout)?;
    let context = blocks::Context {
        catalog,
        callees,
        grants: &grants,
        workflow: &workflow.name,
    };
    let checked = flow::check(&context, &shape, flow::Vars::input(&input))?;
    let mut found = checked.found;
    let output = if workflow.output.trim().is_empty() {
        pcx_expr::Type::Null
    } else {
        let expr = flow::output(catalog, &workflow.output, &checked.ends)
            .map_err(|message| Refusal::workflow("output", message))?;
        let ty = expr.ty().clone();
        found.outputs.insert(String::new(), expr);
        ty
    };
    found.ends.insert(String::new(), checked.ends);
    Ok(Checked {
        signature: Signature { input, output },
        found,
    })
}

/// The text search finds a workflow by: its name and description.
pub fn text(workflow: &Workflow) -> String {
    format!("{}\n{}", workflow.name, workflow.description)
}

#[cfg(test)]
mod tests;
