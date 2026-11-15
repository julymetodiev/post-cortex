// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The workflow service (docs/modules/workflow.md, capability): a
//! workflow checked against the tools its author's scope reaches and the
//! workflows it calls, kept under a name unique in the workspace, and
//! listed; and its runs (`engine`). Between the handlers and the ports.

use std::collections::BTreeMap;

use pcx_module_api::{MAX_CALL_DEPTH, ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::workflow::{Graph, Run, ToolEntry, Workflow, block::Kind};

use crate::domain::{self, Catalog, Checked, Refusal, Signature, grants};
use crate::ports::{Kept, Runs, Tools, Workflows};

/// A refusal of the check as the caller sees it: the field it names.
fn refused(refusal: &Refusal) -> ModuleError {
    let field = match &refusal.block {
        Some(block) => format!("block {block}: {}", refusal.field),
        None => refusal.field.clone(),
    };
    ModuleError::new(ModuleErrorKind::InvalidArgument, refusal.to_string()).with_field(field)
}

/// The workflows a graph's `call` blocks name, its bodies' and branches'
/// included.
fn called(graph: &Graph, into: &mut Vec<String>) {
    for block in &graph.blocks {
        match &block.kind {
            Some(Kind::Call(call)) => into.push(call.workflow.clone()),
            Some(Kind::ForEach(each)) => {
                if let Some(body) = &each.body {
                    called(body, into);
                }
            }
            Some(Kind::Parallel(parallel)) => {
                for branch in &parallel.branches {
                    called(branch, into);
                }
            }
            _ => {}
        }
    }
}

/// The module's workflows in its workspace.
pub struct Service<'a> {
    workflows: &'a dyn Workflows,
    tools: &'a dyn Tools,
}

impl<'a> Service<'a> {
    /// The service over the workflows and the tools.
    pub fn new(workflows: &'a dyn Workflows, tools: &'a dyn Tools) -> Self {
        Self { workflows, tools }
    }

    /// `workflow` checked for `scope`'s author: against the tools the scope
    /// reaches and the workflows it calls (at their latest versions).
    ///
    /// # Errors
    /// `InvalidArgument` naming the block and the field it breaks; the
    /// host or memory failed.
    pub async fn validate(
        &self,
        scope: &Scope,
        workflow: &Workflow,
    ) -> Result<Checked, ModuleError> {
        self.checked(scope, workflow)
            .await
            .map(|(_, checked)| checked)
    }

    /// `workflow` checked as [`Service::validate`] checks it, with the
    /// catalog it was checked against: what a run of it reaches.
    ///
    /// # Errors
    /// As [`Service::validate`]'s.
    pub async fn checked(
        &self,
        scope: &Scope,
        workflow: &Workflow,
    ) -> Result<(Catalog, Checked), ModuleError> {
        let (catalog, judged) = self.judged(scope, workflow).await?;
        Ok((catalog, judged.map_err(|r| refused(&r))?))
    }

    /// `workflow` checked as [`Service::validate`] checks it, the first rule
    /// it breaks answered rather than failed (the editor marks its block).
    ///
    /// # Errors
    /// The host or memory failed; a workflow it calls is not there.
    pub async fn verdict(
        &self,
        scope: &Scope,
        workflow: &Workflow,
    ) -> Result<Result<Checked, Refusal>, ModuleError> {
        self.judged(scope, workflow).await.map(|(_, judged)| judged)
    }

    async fn judged(
        &self,
        scope: &Scope,
        workflow: &Workflow,
    ) -> Result<(Catalog, Result<Checked, Refusal>), ModuleError> {
        let mut catalog = self.catalog(scope).await?;
        let mut callees = BTreeMap::new();
        self.signatures(
            &mut catalog,
            workflow,
            &mut vec![workflow.name.clone()],
            &mut callees,
        )
        .await?;
        let catalog = match catalog.with_records(workflow) {
            Ok(catalog) => catalog,
            Err(refused) => {
                let catalog = self.catalog(scope).await?;
                return Ok((catalog, Err(refused)));
            }
        };
        let judged = domain::check(workflow, &catalog, &callees);
        Ok((catalog, judged))
    }

    /// `workflow` checked and kept as a new one, its grants naming the
    /// tools of effect `external` it calls.
    ///
    /// # Errors
    /// `validate`'s; `Conflict` for a name another workflow has.
    pub async fn create(
        &self,
        scope: &Scope,
        workflow: &Workflow,
    ) -> Result<Kept<Workflow>, ModuleError> {
        let checked = self.validate(scope, workflow).await?;
        if self.workflows.named(&workflow.name).await?.is_some() {
            return Err(taken(&workflow.name));
        }
        let workflow = grants::calling(workflow.clone(), &checked.found);
        self.workflows.create(&workflow).await
    }

    /// Workflow `id` changed to `workflow` from version `version`, checked,
    /// its grants naming the tools of effect `external` it calls.
    ///
    /// # Errors
    /// `validate`'s; `Conflict` for a name another workflow has;
    /// `NotFound`; `Conflict` when it changed since `version`.
    pub async fn update(
        &self,
        scope: &Scope,
        id: &str,
        version: u64,
        workflow: &Workflow,
    ) -> Result<Kept<Workflow>, ModuleError> {
        let checked = self.validate(scope, workflow).await?;
        if let Some(other) = self.workflows.named(&workflow.name).await?
            && other.id != id
        {
            return Err(taken(&workflow.name));
        }
        let workflow = grants::calling(workflow.clone(), &checked.found);
        self.workflows.change(id, version, &workflow).await
    }

    /// The workflows, by name.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn list(&self) -> Result<Vec<Kept<Workflow>>, ModuleError> {
        let mut all = self.workflows.all().await?;
        all.sort_by(|a, b| a.data.name.cmp(&b.data.name));
        Ok(all)
    }

    /// The workflows by name, each with its run that changed last: one
    /// run read by its workflow's key for each, never every run.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn listed(
        &self,
        runs: &dyn Runs,
    ) -> Result<Vec<(Kept<Workflow>, Option<Kept<Run>>)>, ModuleError> {
        let mut listed = Vec::new();
        for workflow in self.list().await? {
            let last = runs.of(&workflow.data.name, 1).await?.pop();
            listed.push((workflow, last));
        }
        Ok(listed)
    }

    /// The tools a workflow written by `scope`'s author may call as `tool`
    /// blocks, those of `module` only when it names one, by path, each with
    /// the JSON Schemas of its request and response.
    ///
    /// # Errors
    /// The host failed.
    pub async fn tools(&self, scope: &Scope, module: &str) -> Result<Vec<ToolEntry>, ModuleError> {
        let mut tools = self.tools.reachable(scope).await?;
        tools.retain(|(_, tool)| module.is_empty() || tool.module == module);
        tools.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(tools
            .into_iter()
            .map(|(method, tool)| ToolEntry {
                method,
                tool: tool.name,
                module: tool.module,
                effect: tool.effect.into(),
                description: tool.description,
                request_schema: serde_json::Value::Object(tool.request_schema).to_string(),
                response_schema: serde_json::Value::Object(tool.response_schema).to_string(),
            })
            .collect())
    }

    async fn catalog(&self, scope: &Scope) -> Result<Catalog, ModuleError> {
        let tools = self.tools.reachable(scope).await?;
        Catalog::new(tools).map_err(|why| ModuleError::new(ModuleErrorKind::Internal, why))
    }

    /// The signatures of the workflows `workflow` calls, and theirs, into
    /// `found`, their records added to `catalog`; `chain` the workflows
    /// calling, outermost first.
    ///
    /// # Errors
    /// `InvalidArgument` for a workflow that calls itself, one that calls
    /// deeper than the host's limit, or a called one that does not check;
    /// memory failed.
    async fn signatures(
        &self,
        catalog: &mut Catalog,
        workflow: &Workflow,
        chain: &mut Vec<String>,
        found: &mut BTreeMap<String, Signature>,
    ) -> Result<(), ModuleError> {
        let mut names = Vec::new();
        if let Some(graph) = &workflow.graph {
            called(graph, &mut names);
        }
        for name in names {
            if found.contains_key(&name) {
                continue;
            }
            let invalid = |message: String| {
                ModuleError::new(ModuleErrorKind::InvalidArgument, message)
                    .with_field("call.workflow")
            };
            if chain.contains(&name) {
                return Err(invalid(format!(
                    "{} calls itself: {} -> {name}",
                    name,
                    chain.join(" -> ")
                )));
            }
            if chain.len() > MAX_CALL_DEPTH as usize {
                return Err(invalid(format!(
                    "{} calls deeper than {MAX_CALL_DEPTH} workflows",
                    chain.join(" -> ")
                )));
            }
            // A workflow named but not kept is left out: the check refuses
            // the block that calls it.
            let Some(callee) = self.workflows.named(&name).await? else {
                continue;
            };
            chain.push(name.clone());
            Box::pin(self.signatures(catalog, &callee.data, chain, found)).await?;
            chain.pop();
            let does_not_check =
                |r: Refusal| invalid(format!("the workflow {name} it calls does not check: {r}"));
            *catalog = catalog
                .clone()
                .with_records(&callee.data)
                .map_err(does_not_check)?;
            let checked = domain::check(&callee.data, catalog, found).map_err(does_not_check)?;
            found.insert(name, checked.signature);
        }
        Ok(())
    }
}

/// A name another workflow has.
fn taken(name: &str) -> ModuleError {
    ModuleError::new(
        ModuleErrorKind::Conflict,
        format!("a workflow is named {name} already"),
    )
    .with_field("name")
}

pub mod engine;
mod templates;

#[cfg(test)]
mod tests;
