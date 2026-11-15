// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The scheduler's ports over its host handle: the system clock, a script
//! through the `Sandbox` port under the scheduler's grants, a task through
//! the board's `AddTask` in the task's workspace, and a workflow's run
//! through `RunWorkflow`, as the scheduler.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::sandbox::{ExecExit, ExecSpec, process_env};
use pcx_module_api::{ModuleError, ModuleHost, Scope};
use post_cortex_proto::board::AddTaskRequest;
use post_cortex_proto::board::boards_client::BoardsClient;
use post_cortex_proto::scheduler::{Script, WorkflowAction};
use post_cortex_proto::workflow::RunWorkflowRequest;
use post_cortex_proto::workflow::workflows_client::WorkflowsClient;
use tokio_util::sync::CancellationToken;
use tonic::Request;

use crate::domain::{DEFAULT_TIMEOUT, Ended, OUTPUT_TAIL};
use crate::ports::{Clock, World};

/// The system's clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// The world over the module's host handle and channel, in `scope`.
pub struct HostWorld {
    /// The module's host handle: its sandbox.
    pub host: Arc<dyn ModuleHost>,
    /// The module's channel: the board.
    pub channel: GrpcChannel,
    /// The scope of the run (the scheduler as its author).
    pub scope: Scope,
    /// The scripts' PATH; none: the sandbox's default.
    pub path: Option<String>,
    /// Ends a running script when the module stops.
    pub stop: CancellationToken,
}

#[async_trait]
impl World for HostWorld {
    async fn script(&self, script: &Script) -> Result<(Ended, String), ModuleError> {
        let timeout = match script.timeout_seconds {
            0 => DEFAULT_TIMEOUT,
            seconds => seconds,
        };
        let spec = ExecSpec {
            program: script.program.clone(),
            args: script.args.clone(),
            cwd: None,
            env: process_env(self.path.as_deref()),
            stdin: None,
            timeout: Duration::from_secs(u64::from(timeout)),
            output_limit: OUTPUT_TAIL,
        };
        let sandbox = self.host.sandbox().await?;
        let out = sandbox
            .exec(spec, self.stop.child_token())
            .await?
            .collect(OUTPUT_TAIL)
            .await?;
        let ended = match out.exit {
            ExecExit::Code(code) => Ended::Code(code),
            ExecExit::TimedOut => Ended::TimedOut,
            ExecExit::Signal | ExecExit::Cancelled => Ended::Killed,
        };
        let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
        output.push_str(&String::from_utf8_lossy(&out.stderr));
        Ok((ended, output))
    }

    async fn add_task(&self, task: AddTaskRequest) -> Result<String, ModuleError> {
        let mut scope = self.scope.clone();
        scope.workspace_id = Some(task.workspace_id.clone());
        let mut request = Request::new(task);
        scope.to_metadata(request.metadata_mut())?;
        let added = BoardsClient::new(self.channel.clone())
            .add_task(request)
            .await?
            .into_inner();
        Ok(added.id)
    }

    async fn workflow(&self, action: &WorkflowAction) -> Result<String, ModuleError> {
        let mut request = Request::new(RunWorkflowRequest {
            workflow: action.name.clone(),
            input: action.input.clone(),
            ..Default::default()
        });
        self.scope.to_metadata(request.metadata_mut())?;
        let run = WorkflowsClient::new(self.channel.clone())
            .run_workflow(request)
            .await?
            .into_inner();
        Ok(run.id)
    }
}
