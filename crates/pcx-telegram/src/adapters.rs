// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The channel's ports over its host channel (W23): the board's documents
//! and `GetTask`, `ApproveDecision` and `DeclineDecision`, and the chief's
//! `Ask`. A person's call carries that person as its author, which the
//! host admits for a tool only a person calls when the module's settings
//! name them (`PersonGrant`).

use std::time::{Duration, Instant};

use async_trait::async_trait;
use pcx_documents::{HostDocuments, Typed};
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, Scope};
use post_cortex_proto::board::boards_client::BoardsClient;
use post_cortex_proto::board::{Decision, DecisionRequest, GetTaskRequest};
use post_cortex_proto::chief::AskRequest;
use post_cortex_proto::chief::chief_client::ChiefClient;
use tokio_util::sync::CancellationToken;
use tonic::Request;

use crate::domain::MODULE_ID;
use crate::ports::{Board, Chief, Waiting};

/// The kind of the board's decisions.
const DECISION: &str = "board/decision";

/// The deadline of a call of the channel; a question to the chief runs a
/// turn, so it gets longer.
const CALL: Duration = Duration::from_secs(30);

/// The deadline of a question to the chief.
const QUESTION: Duration = Duration::from_secs(600);

/// The ports over the module's host channel.
pub struct HostPorts {
    channel: GrpcChannel,
    stop: CancellationToken,
}

impl HostPorts {
    /// The ports over `channel`, whose calls `stop` cancels.
    pub fn new(channel: GrpcChannel, stop: CancellationToken) -> Self {
        Self { channel, stop }
    }

    /// The scope of a call in `workspace` (none: no workspace) by
    /// `author`, within `within`.
    fn scope(&self, workspace: Option<&str>, author: &str, within: Duration) -> Scope {
        Scope {
            tenant: String::new(),
            profile: String::new(),
            session_id: None,
            workspace_id: workspace.map(str::to_string),
            author: Some(author.to_string()),
            caller_module: None,
            call_depth: 0,
            turn: None,
            reach: None,
            role: None,
            read_only: false,
            person: None,
            deadline: Instant::now() + within,
            cancel: self.stop.child_token(),
        }
    }

    fn request<T>(scope: &Scope, message: T) -> Result<Request<T>, ModuleError> {
        let mut request = Request::new(message);
        scope.to_metadata(request.metadata_mut())?;
        Ok(request)
    }

    fn boards(&self) -> BoardsClient<GrpcChannel> {
        BoardsClient::new(self.channel.clone())
    }
}

#[async_trait]
impl Board for HostPorts {
    async fn decision(&self, workspace: &str, id: &str) -> Result<Option<Waiting>, ModuleError> {
        let scope = self.scope(Some(workspace), MODULE_ID, CALL);
        let docs = HostDocuments::new(self.channel.clone(), &scope, workspace);
        let Some(decision) = Typed::new(&docs, DECISION).find::<Decision>(id).await? else {
            return Ok(None);
        };
        let thread = self
            .boards()
            .get_task(Self::request(
                &scope,
                GetTaskRequest {
                    workspace_id: workspace.to_string(),
                    task: decision.data.task.clone(),
                },
            )?)
            .await?
            .into_inner();
        let task = thread
            .task
            .and_then(|t| t.task)
            .map(|t| t.title)
            .unwrap_or_default();
        Ok(thread
            .decisions
            .into_iter()
            .find(|d| d.id == id)
            .map(|decision| Waiting { decision, task }))
    }

    async fn approve(
        &self,
        workspace: &str,
        person: &str,
        id: &str,
        version: u64,
    ) -> Result<(), ModuleError> {
        let scope = self.scope(Some(workspace), person, CALL);
        self.boards()
            .approve_decision(Self::request(
                &scope,
                DecisionRequest {
                    workspace_id: workspace.to_string(),
                    decision: id.to_string(),
                    version,
                    note: Some("approved in Telegram".to_string()),
                },
            )?)
            .await?;
        Ok(())
    }

    async fn decline(
        &self,
        workspace: &str,
        person: &str,
        id: &str,
        version: u64,
    ) -> Result<(), ModuleError> {
        let scope = self.scope(Some(workspace), person, CALL);
        self.boards()
            .decline_decision(Self::request(
                &scope,
                DecisionRequest {
                    workspace_id: workspace.to_string(),
                    decision: id.to_string(),
                    version,
                    note: Some("declined in Telegram".to_string()),
                },
            )?)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Chief for HostPorts {
    async fn ask(
        &self,
        person: &str,
        question: &str,
        conversation: &str,
    ) -> Result<(String, String), ModuleError> {
        let scope = self.scope(None, person, QUESTION);
        let answer = ChiefClient::new(self.channel.clone())
            .ask(Self::request(
                &scope,
                AskRequest {
                    question: question.to_string(),
                    conversation_id: conversation.to_string(),
                },
            )?)
            .await?
            .into_inner();
        let text = answer.end.map(|e| e.answer).unwrap_or_default();
        Ok((text, answer.conversation_id))
    }
}
