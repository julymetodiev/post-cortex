// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The webhooks' ports over the module's host channel (W23): the board's
//! `pcx.board.v1` RPCs, memory's `UpdateContext` and the workflows'
//! `RunWorkflow`, each in the source's workspace with the source as the
//! author, so the board and memory name it as who added, commented and
//! recorded.

use async_trait::async_trait;
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, Scope, channel};
use post_cortex_proto::board::boards_client::BoardsClient;
use post_cortex_proto::board::{
    AddCommentRequest, AddTaskRequest, BoardRecord, GetTaskRequest, ListBoardsRequest,
    ListTasksRequest, MoveTaskRequest, TaskRecord, Thread,
};
use post_cortex_proto::channel::channel_client::ChannelClient;
use post_cortex_proto::pb::post_cortex_client::PostCortexClient;
use post_cortex_proto::pb::{ContextContent, EntityInfo, RelationInfo, UpdateContextRequest};
use post_cortex_proto::webhooks::Source;
use post_cortex_proto::workflow::RunWorkflowRequest;
use post_cortex_proto::workflow::workflows_client::WorkflowsClient;
use tonic::Request;

use crate::domain::Incident;
use crate::ports::{Board, Incidents, Reached, Runner, Teller, World};

/// The world over the module's host channel, for a request in `scope`
/// (its deadline and cancellation).
pub struct HostWorld {
    channel: GrpcChannel,
    scope: Scope,
}

impl HostWorld {
    /// The world over `channel` for a request in `scope`.
    pub fn new(channel: GrpcChannel, scope: Scope) -> Self {
        Self { channel, scope }
    }
}

impl World for HostWorld {
    fn of<'a>(&'a self, source: &'a Source) -> Box<dyn Reached + 'a> {
        let mut scope = self.scope.clone();
        scope.workspace_id = Some(source.workspace_id.clone());
        scope.author = Some(source.name.clone());
        Box::new(SourcePorts {
            channel: self.channel.clone(),
            scope,
            source: &source.name,
        })
    }
}

/// The board and memory of one source.
struct SourcePorts<'a> {
    channel: GrpcChannel,
    scope: Scope,
    source: &'a str,
}

impl SourcePorts<'_> {
    /// `message` as a request in the source's scope.
    fn request<T>(&self, message: T) -> Result<Request<T>, ModuleError> {
        let mut request = Request::new(message);
        self.scope.to_metadata(request.metadata_mut())?;
        Ok(request)
    }

    fn boards(&self) -> BoardsClient<GrpcChannel> {
        BoardsClient::new(self.channel.clone())
    }
}

#[async_trait]
impl Board for SourcePorts<'_> {
    async fn add(&self, task: AddTaskRequest) -> Result<TaskRecord, ModuleError> {
        Ok(self
            .boards()
            .add_task(self.request(task)?)
            .await?
            .into_inner())
    }

    async fn tasks(&self, board: &str) -> Result<Vec<TaskRecord>, ModuleError> {
        let listed = self
            .boards()
            .list_tasks(self.request(ListTasksRequest {
                board: board.to_string(),
                ..ListTasksRequest::default()
            })?)
            .await?
            .into_inner();
        Ok(listed.tasks)
    }

    async fn board(&self, board: &str) -> Result<BoardRecord, ModuleError> {
        let listed = self
            .boards()
            .list_boards(self.request(ListBoardsRequest {
                board: board.to_string(),
                ..ListBoardsRequest::default()
            })?)
            .await?
            .into_inner();
        listed.boards.into_iter().next().ok_or_else(|| {
            ModuleError::new(
                pcx_module_api::ModuleErrorKind::NotFound,
                format!("no board {board}"),
            )
        })
    }

    async fn thread(&self, task: &str) -> Result<Thread, ModuleError> {
        Ok(self
            .boards()
            .get_task(self.request(GetTaskRequest {
                task: task.to_string(),
                ..GetTaskRequest::default()
            })?)
            .await?
            .into_inner())
    }

    async fn comment(&self, task: &str, text: &str) -> Result<(), ModuleError> {
        self.boards()
            .add_comment(self.request(AddCommentRequest {
                task: task.to_string(),
                text: text.to_string(),
                ..AddCommentRequest::default()
            })?)
            .await?;
        Ok(())
    }

    async fn close(
        &self,
        task: &str,
        version: u64,
        column: &str,
        member: &str,
    ) -> Result<(), ModuleError> {
        let mut request = Request::new(MoveTaskRequest {
            task: task.to_string(),
            version,
            column: column.to_string(),
            ..MoveTaskRequest::default()
        });
        let mut scope = self.scope.clone();
        scope.author = Some(member.to_string());
        scope.to_metadata(request.metadata_mut())?;
        self.boards().move_task(request).await?;
        Ok(())
    }
}

#[async_trait]
impl Teller for SourcePorts<'_> {
    async fn tell(&self, destination: &str, text: &str) -> Result<(), ModuleError> {
        ChannelClient::new(self.channel.clone())
            .send(channel::send(&self.scope, destination, text)?)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Runner for SourcePorts<'_> {
    async fn run(
        &self,
        workflow: &str,
        source: &str,
        body: Vec<u8>,
    ) -> Result<String, ModuleError> {
        let run = WorkflowsClient::new(self.channel.clone())
            .run_workflow(self.request(RunWorkflowRequest {
                workflow: workflow.to_string(),
                source: source.to_string(),
                body,
                ..Default::default()
            })?)
            .await?
            .into_inner();
        Ok(run.id)
    }
}

fn entity(name: &str, kind: &str) -> EntityInfo {
    EntityInfo {
        name: name.to_string(),
        entity_type: kind.to_string(),
    }
}

#[async_trait]
impl Incidents for SourcePorts<'_> {
    async fn record(&self, session: &str, incident: &Incident) -> Result<(), ModuleError> {
        let alert = format!("alert: {}", incident.alert);
        let mut entities = vec![entity(&alert, "problem")];
        let mut relations = Vec::with_capacity(incident.targets.len());
        for target in &incident.targets {
            entities.push(entity(target, "concept"));
            relations.push(RelationInfo {
                from_entity: alert.clone(),
                to_entity: target.clone(),
                relation_type: "related_to".to_string(),
                context: "the alert fired on it".to_string(),
            });
        }
        if relations.is_empty() {
            // Memory needs a relation: the alert and its source.
            entities.push(entity(self.source, "technology"));
            relations.push(RelationInfo {
                from_entity: alert,
                to_entity: self.source.to_string(),
                relation_type: "related_to".to_string(),
                context: "the source that sent it".to_string(),
            });
        }
        let record = UpdateContextRequest {
            session_id: session.to_string(),
            interaction_type: "problem_solved".to_string(),
            content: Some(ContextContent {
                title: incident.title.clone(),
                description: incident.description.clone(),
                ..ContextContent::default()
            }),
            entities,
            relations,
            author: Some(self.source.to_string()),
            ..UpdateContextRequest::default()
        };
        PostCortexClient::new(self.channel.clone())
            .update_context(self.request(record)?)
            .await?;
        Ok(())
    }
}
