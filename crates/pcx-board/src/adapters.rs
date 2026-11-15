// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The board's ports over its host channel (W23): sessions and decision
//! records through memory's `pcx.v1` (`CreateSession`,
//! `AddSessionToWorkspace`, `UpdateContext`), a branch's commits through
//! `tools-git`'s `BranchCommits`; its documents are
//! `pcx_documents::HostDocuments`; its events through the host's bus. Each
//! call runs in the caller's scope, so memory names the board as the writer
//! and the caller as the author, and an event its workspace and author.

use std::sync::Arc;

use async_trait::async_trait;
use pbjson_types::Any;
use pcx_documents::{HostSessions, Sessions};
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, ModuleErrorKind, ModuleHost, Scope};
use post_cortex_proto::board::{
    DecisionApproved, DecisionDeclined, DecisionMade, TaskCreated, TaskMoved,
};
use post_cortex_proto::pb::post_cortex_client::PostCortexClient;
use post_cortex_proto::pb::{ContextContent, EntityInfo, RelationInfo, UpdateContextRequest};
use post_cortex_proto::tools_git::git_client::GitClient;
use post_cortex_proto::tools_git::{BranchCommitsRequest, Repo};
use prost::Message;
use tonic::Request;
use tracing::{debug, warn};

use crate::module::Running;
use crate::ports::{BoardEvent, Commits, DecisionRecord, Events, Records};

/// The ports of one call, in workspace `workspace`.
pub struct HostPorts<'a> {
    channel: GrpcChannel,
    host: Arc<dyn ModuleHost>,
    /// The call's scope, naming its workspace ([`Scope::working_in`]).
    scope: Scope,
    workspace: &'a str,
}

impl<'a> HostPorts<'a> {
    /// The ports over the `running` module's host channel and handle, in
    /// `scope`, in `workspace`.
    pub fn new(running: &Running, scope: &Scope, workspace: &'a str) -> Self {
        Self {
            channel: running.channel.clone(),
            host: Arc::clone(&running.host),
            scope: scope.working_in(workspace),
            workspace,
        }
    }

    /// `message` as a request in the call's scope.
    fn request<T>(&self, message: T) -> Result<Request<T>, ModuleError> {
        let mut request = Request::new(message);
        self.scope.to_metadata(request.metadata_mut())?;
        Ok(request)
    }
}

fn concept(name: &str) -> EntityInfo {
    EntityInfo {
        name: name.to_string(),
        entity_type: "concept".to_string(),
    }
}

#[async_trait]
impl Records for HostPorts<'_> {
    async fn create_session(&self, name: &str) -> Result<String, ModuleError> {
        HostSessions::new(self.channel.clone(), &self.scope, self.workspace)
            .create(name)
            .await
    }

    async fn record_decision(
        &self,
        session: &str,
        decision: &DecisionRecord,
    ) -> Result<(), ModuleError> {
        let (task, board) = (
            format!("task: {}", decision.task),
            format!("board: {}", decision.board),
        );
        let record = UpdateContextRequest {
            session_id: session.to_string(),
            interaction_type: "decision_made".to_string(),
            content: Some(ContextContent {
                title: decision.what.clone(),
                description: decision.why.clone(),
                ..ContextContent::default()
            }),
            entities: vec![concept(&task), concept(&board)],
            relations: vec![RelationInfo {
                from_entity: task,
                to_entity: board,
                relation_type: "related_to".to_string(),
                context: "a decision on a task of the board".to_string(),
            }],
            author: Some(decision.decided_by.clone()),
            ..UpdateContextRequest::default()
        };
        PostCortexClient::new(self.channel.clone())
            .update_context(self.request(record)?)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Commits for HostPorts<'_> {
    async fn branch_commits(&self, repo: &str, branch: &str) -> Result<Vec<String>, ModuleError> {
        let listed = GitClient::new(self.channel.clone())
            .branch_commits(self.request(BranchCommitsRequest {
                at: Some(Repo {
                    repo: repo.to_string(),
                    root: String::new(),
                }),
                branch: branch.to_string(),
                base: String::new(),
            })?)
            .await?
            .into_inner();
        Ok(listed.commits.into_iter().map(|c| c.sha).collect())
    }
}

/// An event's name and message, as the manifest declares it.
fn on_the_bus(event: BoardEvent) -> (&'static str, Any) {
    let any = |name: &str, value: Vec<u8>| Any {
        type_url: format!("type.googleapis.com/pcx.board.v1.{name}"),
        value: value.into(),
    };
    match event {
        BoardEvent::TaskCreated {
            task,
            board,
            title,
            column,
        } => (
            TASK_CREATED,
            any(
                "TaskCreated",
                TaskCreated {
                    task,
                    board,
                    title,
                    column,
                }
                .encode_to_vec(),
            ),
        ),
        BoardEvent::TaskMoved {
            task,
            board,
            from,
            to,
        } => (
            TASK_MOVED,
            any(
                "TaskMoved",
                TaskMoved {
                    task,
                    board,
                    from,
                    to,
                }
                .encode_to_vec(),
            ),
        ),
        BoardEvent::DecisionMade {
            decision,
            task,
            pending,
        } => (
            DECISION_MADE,
            any(
                "DecisionMade",
                DecisionMade {
                    decision,
                    task,
                    pending,
                }
                .encode_to_vec(),
            ),
        ),
        BoardEvent::DecisionApproved {
            decision,
            task,
            approved_by,
        } => (
            DECISION_APPROVED,
            any(
                "DecisionApproved",
                DecisionApproved {
                    decision,
                    task,
                    approved_by,
                }
                .encode_to_vec(),
            ),
        ),
        BoardEvent::DecisionDeclined {
            decision,
            task,
            declined_by,
        } => (
            DECISION_DECLINED,
            any(
                "DecisionDeclined",
                DecisionDeclined {
                    decision,
                    task,
                    declined_by,
                }
                .encode_to_vec(),
            ),
        ),
    }
}

/// `board.task_created`.
pub const TASK_CREATED: &str = "board.task_created";
/// `board.task_moved`.
pub const TASK_MOVED: &str = "board.task_moved";
/// `board.decision_made`.
pub const DECISION_MADE: &str = "board.decision_made";
/// `board.decision_approved`.
pub const DECISION_APPROVED: &str = "board.decision_approved";
/// `board.decision_declined`.
pub const DECISION_DECLINED: &str = "board.decision_declined";

#[async_trait]
impl Events for HostPorts<'_> {
    async fn emitted(&self, event: BoardEvent) {
        let (name, payload) = on_the_bus(event);
        let published = match self.host.bus().await {
            Ok(bus) => bus.publish(&self.scope, name, payload).await,
            Err(error) => Err(error),
        };
        match published {
            Ok(_) => {}
            // A host that keeps no events (a test's, a bare one).
            Err(e) if e.kind == ModuleErrorKind::FailedPrecondition => {
                debug!(event = name, "the host keeps no events: {e}");
            }
            Err(e) => warn!(event = name, "the event was not kept: {e}"),
        }
    }
}
