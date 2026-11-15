// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The workflow module's `pcx.workflow.v1.Workflows` service: each RPC
//! maps its request to the service or the engine and its answer or failure
//! back.

use std::pin::Pin;

use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::workflow::workflows_server::Workflows;
use post_cortex_proto::workflow::{
    CancelRunRequest, CreateWorkflowRequest, DeleteWorkflowRequest, DeleteWorkflowResponse, DryRun,
    DryRunRequest, GetRunRequest, InstallTemplateRequest, ListRunsRequest, ListTemplatesRequest,
    ListToolsRequest, ListWorkflowsRequest, Preview, PreviewRequest, Refused, Run, RunChange,
    RunDetail, RunList, RunRecord, RunWorkflowRequest, TemplateList, ToolList, ToolUse,
    UpdateWorkflowRequest, ValidateWorkflowRequest, Validation, WatchRunRequest, Workflow,
    WorkflowList, WorkflowRecord, run_change,
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status};

use crate::module::{Shared, running};
use crate::ports::Kept;
use crate::service::engine::Watched;

/// A watched run's changes on the wire.
pub type RunChanges = Pin<Box<dyn Stream<Item = Result<RunChange, Status>> + Send>>;

/// The service's handlers.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

fn workflow_record(kept: Kept<Workflow>) -> WorkflowRecord {
    WorkflowRecord {
        id: kept.id,
        version: kept.version,
        workflow: Some(kept.data),
        last_run: None,
    }
}

fn run_record(kept: Kept<Run>) -> RunRecord {
    RunRecord {
        id: kept.id,
        version: kept.version,
        run: Some(kept.data),
    }
}

/// The workflow a request carries.
fn given(workflow: Option<Workflow>) -> Result<Workflow, ModuleError> {
    workflow.ok_or_else(|| {
        ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            "the request has no workflow",
        )
        .with_field("workflow")
    })
}

#[tonic::async_trait]
impl Workflows for Handlers {
    async fn create_workflow(
        &self,
        request: Request<CreateWorkflowRequest>,
    ) -> Result<Response<WorkflowRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let workflow = given(request.into_inner().workflow)?;
        let workflows = running.workflows(&scope);
        let kept = running.engine.create(&scope, &workflows, &workflow).await?;
        Ok(Response::new(workflow_record(kept)))
    }

    async fn update_workflow(
        &self,
        request: Request<UpdateWorkflowRequest>,
    ) -> Result<Response<WorkflowRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let req = request.into_inner();
        let workflow = given(req.workflow)?;
        let workflows = running.workflows(&scope);
        let kept = running
            .engine
            .update(&scope, &workflows, &req.id, req.version, &workflow)
            .await?;
        Ok(Response::new(workflow_record(kept)))
    }

    async fn delete_workflow(
        &self,
        request: Request<DeleteWorkflowRequest>,
    ) -> Result<Response<DeleteWorkflowResponse>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let workflows = running.workflows(&scope);
        running
            .engine
            .delete(&workflows, &request.get_ref().id)
            .await?;
        Ok(Response::new(DeleteWorkflowResponse {}))
    }

    async fn validate_workflow(
        &self,
        request: Request<ValidateWorkflowRequest>,
    ) -> Result<Response<Validation>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let workflow = given(request.into_inner().workflow)?;
        let workflows = running.workflows(&scope);
        let verdict = running
            .service(&workflows)
            .verdict(&scope, &workflow)
            .await?;
        Ok(Response::new(match verdict {
            Ok(checked) => Validation {
                tools: checked
                    .found
                    .tools
                    .into_iter()
                    .map(|(block, tool)| ToolUse {
                        block,
                        module: tool.module,
                        effect: tool.effect.into(),
                    })
                    .collect(),
                refused: None,
            },
            Err(refusal) => Validation {
                tools: Vec::new(),
                refused: Some(Refused {
                    block: refusal.block.unwrap_or_default(),
                    field: refusal.field,
                    message: refusal.message,
                }),
            },
        }))
    }

    async fn list_workflows(
        &self,
        request: Request<ListWorkflowsRequest>,
    ) -> Result<Response<WorkflowList>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let workflows = running.workflows(&scope);
        let all = running
            .service(&workflows)
            .listed(running.engine.runs_port())
            .await?;
        Ok(Response::new(WorkflowList {
            workflows: all
                .into_iter()
                .map(|(workflow, last)| WorkflowRecord {
                    last_run: last.map(run_record),
                    ..workflow_record(workflow)
                })
                .collect(),
        }))
    }

    async fn list_tools(
        &self,
        request: Request<ListToolsRequest>,
    ) -> Result<Response<ToolList>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let workflows = running.workflows(&scope);
        let module = request.into_inner().module;
        let tools = running.service(&workflows).tools(&scope, &module).await?;
        Ok(Response::new(ToolList { tools }))
    }

    async fn list_templates(
        &self,
        request: Request<ListTemplatesRequest>,
    ) -> Result<Response<TemplateList>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let workflows = running.workflows(&scope);
        let module = request.into_inner().module;
        let templates = running
            .service(&workflows)
            .templates(&scope, &module)
            .await?;
        Ok(Response::new(TemplateList { templates }))
    }

    async fn install_template(
        &self,
        request: Request<InstallTemplateRequest>,
    ) -> Result<Response<WorkflowRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let req = request.into_inner();
        let workflows = running.workflows(&scope);
        let kept = running
            .engine
            .install(&scope, &workflows, &req.module, &req.template, &req.values)
            .await?;
        Ok(Response::new(workflow_record(kept)))
    }

    async fn run_workflow(
        &self,
        request: Request<RunWorkflowRequest>,
    ) -> Result<Response<RunRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let run = running.engine.run(&scope, request.into_inner()).await?;
        Ok(Response::new(run_record(run)))
    }

    async fn dry_run_workflow(
        &self,
        request: Request<DryRunRequest>,
    ) -> Result<Response<DryRun>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let req = request.into_inner();
        let workflow = given(req.workflow)?;
        let (run, steps) = running
            .engine
            .dry_run(
                &scope,
                &workflow,
                &req.input.unwrap_or_default(),
                req.models,
            )
            .await?;
        Ok(Response::new(DryRun {
            run: run.data.into(),
            steps,
        }))
    }

    async fn list_runs(
        &self,
        request: Request<ListRunsRequest>,
    ) -> Result<Response<RunList>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let req = request.into_inner();
        let runs = running.engine.runs(&req.workflow, req.limit).await?;
        Ok(Response::new(RunList {
            runs: runs.into_iter().map(run_record).collect(),
        }))
    }

    async fn get_run(
        &self,
        request: Request<GetRunRequest>,
    ) -> Result<Response<RunDetail>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let (run, steps) = running.engine.detail(&request.into_inner().id).await?;
        Ok(Response::new(RunDetail {
            run: Some(run_record(run)),
            steps,
        }))
    }

    async fn cancel_run(
        &self,
        request: Request<CancelRunRequest>,
    ) -> Result<Response<RunRecord>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let run = running.engine.cancel(&request.into_inner().id).await?;
        Ok(Response::new(run_record(run)))
    }

    async fn preview_block(
        &self,
        request: Request<PreviewRequest>,
    ) -> Result<Response<Preview>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let mut req = request.into_inner();
        let workflow = given(req.workflow.take())?;
        let preview = running.engine.preview(&scope, workflow, req).await?;
        Ok(Response::new(preview))
    }

    type WatchRunStream = RunChanges;

    async fn watch_run(
        &self,
        request: Request<WatchRunRequest>,
    ) -> Result<Response<RunChanges>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared, &scope)?;
        let watch = running.engine.watch(&request.into_inner().id).await?;
        let changes =
            ReceiverStream::new(watch).map(|watched| watched.map(change).map_err(Status::from));
        Ok(Response::new(Box::pin(changes)))
    }
}

fn change(watched: Watched) -> RunChange {
    let change = match watched {
        Watched::Whole(run, steps) => run_change::Change::Whole(RunDetail {
            run: Some(run_record(*run)),
            steps,
        }),
        Watched::Run(run) => run_change::Change::Run(run_record(*run)),
        Watched::Step(step) => run_change::Change::Step(*step),
    };
    RunChange {
        change: Some(change),
    }
}
