// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The workflow module's ports over its host (docs/modules/workflow.md,
//! layers): the tools a scope reaches and their calls (the host's route
//! checks the grant and the visibility again on every call), the host's
//! models (the capability `models`), the agent's turns, the bus, memory's
//! blobs and sessions, and the system clock.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pcx_documents::{HostSessions, Sessions};
use pcx_module_api::agent::{TurnEnd, TurnRequest, TurnStop};
use pcx_module_api::bus::EventFeed;
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::model::{
    Block, ModelFailure, ModelFailureKind, ModelMessage, ModelRequest, ModelTool, Role, ToolChoice,
};
use pcx_module_api::{ModuleError, ModuleErrorKind, ModuleHost, ModuleKind, Scope};
use post_cortex_proto::module::WorkflowTemplate;
use post_cortex_proto::pb::blobs_client::BlobsClient;
use post_cortex_proto::pb::{BlobRequest, PutBlobChunk};
use post_cortex_proto::team::teams_client::TeamsClient;
use post_cortex_proto::team::{ListTeamsRequest, Roster};
use prost_reflect::DynamicMessage;
use serde_json::Value as Json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::domain::Tool;
use crate::ports::{Answer, Blobs, Clock, Events, Models, Said, Tools, Turns, Whose};

/// The module that runs a team member's turns in its own name.
const TEAM: &str = "team";

/// The tool a model answers through.
const ANSWER_TOOL: &str = "answer";

/// The most tokens a `model` block's answer may take.
const MAX_ANSWER_TOKENS: u32 = 8_192;

/// The deadline of the module's own calls (a blob, a session).
const OWN_DEADLINE: Duration = Duration::from_secs(60);

/// What the host-backed ports share: the module's handle, its channel and
/// its workspace.
#[derive(Clone)]
pub struct Host {
    host: Arc<dyn ModuleHost>,
    channel: GrpcChannel,
    workspace: String,
}

impl Host {
    /// The ports over `host` and its `channel`, in `workspace`.
    pub fn new(host: Arc<dyn ModuleHost>, channel: GrpcChannel, workspace: String) -> Self {
        Self {
            host,
            channel,
            workspace,
        }
    }

    /// The scope of the module's own calls in its workspace.
    fn own(&self) -> Scope {
        let wire = post_cortex_proto::module::Scope {
            workspace_id: self.workspace.clone(),
            author: crate::domain::MODULE_ID.to_string(),
            ..Default::default()
        };
        Scope::from_wire(wire, CancellationToken::new(), OWN_DEADLINE)
    }

    fn request<T>(&self, message: T) -> Result<tonic::Request<T>, ModuleError> {
        let mut request = tonic::Request::new(message);
        self.own().to_metadata(request.metadata_mut())?;
        Ok(request)
    }
}

#[async_trait]
impl Tools for Host {
    async fn reachable(&self, scope: &Scope) -> Result<Vec<(String, Tool)>, ModuleError> {
        Ok(self
            .host
            .tools(scope)
            .await?
            .into_iter()
            .map(|used| {
                (
                    used.tool.path().to_string(),
                    Tool {
                        name: used.tool.spec.name.clone(),
                        description: used.tool.spec.description.clone(),
                        request_schema: used.tool.spec.input_schema.clone(),
                        response_schema: used.tool.response_schema(),
                        module: used.module,
                        effect: used.tool.spec.effect,
                        request: used.tool.request().clone(),
                        response: used.tool.response().clone(),
                    },
                )
            })
            .collect())
    }

    async fn call(
        &self,
        scope: &Scope,
        path: &str,
        request: &DynamicMessage,
        idempotency_key: Option<&str>,
    ) -> Result<DynamicMessage, ModuleError> {
        let used = self
            .host
            .tools(scope)
            .await?
            .into_iter()
            .find(|u| u.tool.path() == path)
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::Denied,
                    format!("{path} is no tool this run may call"),
                )
            })?;
        used.tool
            .call_message(self.channel.clone(), scope, request, idempotency_key)
            .await
    }

    async fn templates(
        &self,
        scope: &Scope,
    ) -> Result<Vec<(String, WorkflowTemplate)>, ModuleError> {
        let reached: BTreeSet<String> = self
            .host
            .tools(scope)
            .await?
            .into_iter()
            .map(|used| used.module)
            .collect();
        Ok(self
            .host
            .manifests()
            .await?
            .into_iter()
            .filter(|manifest| reached.contains(&manifest.id))
            .flat_map(|manifest| {
                let module = manifest.id;
                manifest
                    .workflow_templates
                    .into_iter()
                    .map(move |template| (module.clone(), template))
            })
            .collect())
    }
}

/// What a turn that ended as `end` said: its answer when the model
/// answered; otherwise why it did not, as a failure of its own kind (a
/// provider's refused quota is `ResourceExhausted`, not an empty answer).
fn said(end: TurnEnd) -> Result<Said, ModuleError> {
    let refused = |kind, why: String| {
        Err(ModuleError::new(
            kind,
            format!("the turn ended without an answer: {why}"),
        ))
    };
    match end.stop {
        TurnStop::Answered(_) => Ok(Said {
            answer: end.answer,
            tokens: end.usage.input
                + end.usage.output
                + end.usage.cache_read
                + end.usage.cache_write,
        }),
        TurnStop::Failed(failure) => {
            let failed = failed(failure);
            refused(failed.kind, failed.message)
        }
        TurnStop::Blocked(why) => refused(
            ModuleErrorKind::Denied,
            format!("a module blocked it: {why}"),
        ),
        TurnStop::StepLimit => refused(
            ModuleErrorKind::ResourceExhausted,
            "it took the most steps it may".to_string(),
        ),
        TurnStop::Repeating(what) => refused(
            ModuleErrorKind::Unavailable,
            format!("its tools kept failing: {what}"),
        ),
        TurnStop::Cancelled => refused(
            ModuleErrorKind::DeadlineExceeded,
            "it was cancelled".to_string(),
        ),
    }
}

/// A model's failure as a module's.
fn failed(failure: ModelFailure) -> ModuleError {
    let kind = match failure.kind {
        ModelFailureKind::RateLimited
        | ModelFailureKind::Overloaded
        | ModelFailureKind::Unavailable => ModuleErrorKind::Unavailable,
        ModelFailureKind::QuotaExhausted => ModuleErrorKind::ResourceExhausted,
        ModelFailureKind::Unauthorized => ModuleErrorKind::Denied,
        ModelFailureKind::ContextTooLong | ModelFailureKind::Invalid => {
            ModuleErrorKind::InvalidArgument
        }
    };
    ModuleError::new(kind, failure.message)
}

#[async_trait]
impl Models for Host {
    async fn answer(
        &self,
        scope: &Scope,
        model: &str,
        system: &str,
        prompt: &str,
        schema: &Json,
    ) -> Result<Answer, ModuleError> {
        let provider = self.host.model().await?;
        let model = if model.is_empty() {
            provider
                .models()
                .into_iter()
                .next()
                .map(|m| m.id)
                .ok_or_else(|| {
                    ModuleError::new(ModuleErrorKind::FailedPrecondition, "no provider runs")
                })?
        } else {
            model.to_string()
        };
        let Json::Object(input_schema) = schema.clone() else {
            return Err(ModuleError::new(
                ModuleErrorKind::Internal,
                "an answer's schema is an object",
            ));
        };
        let request = ModelRequest {
            model,
            system: system.to_string(),
            messages: vec![ModelMessage {
                role: Role::User,
                blocks: vec![Block::Text {
                    text: prompt.to_string(),
                }],
            }],
            tools: vec![ModelTool {
                name: ANSWER_TOOL.to_string(),
                description: "Give the answer: every field the schema asks for.".to_string(),
                input_schema,
            }],
            max_tokens: MAX_ANSWER_TOKENS,
            tool_choice: ToolChoice::Tool(ANSWER_TOOL.to_string()),
        };
        let (blocks, usage, _) = provider
            .generate(scope, request)
            .await
            .map_err(failed)?
            .collect()
            .await
            .map_err(failed)?;
        let answer = blocks
            .into_iter()
            .find_map(|b| match b {
                Block::ToolUse { name, input, .. } if name == ANSWER_TOOL => Some(input),
                _ => None,
            })
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::InvalidArgument,
                    "the model gave no answer through its tool",
                )
            })?;
        Ok(Answer {
            answer,
            tokens: usage.input + usage.output + usage.cache_read + usage.cache_write,
        })
    }
}

/// Whether `member` is one the team runs turns of: an agent of the harness
/// in one of `teams`.
///
/// # Errors
/// What is missing, and where a person adds it.
fn in_a_team(teams: &[Roster], member: &str) -> Result<(), String> {
    let found = teams
        .iter()
        .flat_map(|roster| &roster.agents)
        .filter_map(|record| record.agent.as_ref())
        .find(|agent| agent.name == member);
    match found {
        Some(agent) if agent.harness => Ok(()),
        Some(_) => Err(format!(
            "member {member} is no agent of the harness, so the team runs no turn of it: an agent block names one that is"
        )),
        None => Err(format!(
            "no member {member} in the workspace's teams: add it to a team in Teams"
        )),
    }
}

#[async_trait]
impl Turns for Host {
    async fn turn(
        &self,
        scope: &Scope,
        session: &str,
        whose: Whose<'_>,
        prompt: &str,
    ) -> Result<Said, ModuleError> {
        // A member's turn is the team's: it applies the member's role, skills,
        // model, modules and tools, and runs under the team's reach (the
        // agent's own never reads `member`). The agent's own runs under this
        // module's reach, every module's, so it is given its modules here.
        let (turns, request) = match whose {
            Whose::Member(member) => (
                self.host.turns_of(TEAM).await?,
                TurnRequest {
                    member: Some(member.to_string()),
                    ..TurnRequest::new(session, prompt)
                },
            ),
            Whose::Agent(modules) => (
                self.host.turns().await?,
                TurnRequest {
                    modules: modules.to_vec(),
                    ..TurnRequest::new(session, prompt)
                },
            ),
        };
        // A run reads the turn's end, not its events as they come.
        let (events, _) = mpsc::channel(1);
        said(turns.run(scope, &request, &events).await?)
    }

    async fn ready(&self, whose: Whose<'_>) -> Result<(), ModuleError> {
        let missing =
            |what: String| Err(ModuleError::new(ModuleErrorKind::FailedPrecondition, what));
        let agent = i32::from(ModuleKind::Agent);
        if !self.host.manifests().await?.iter().any(|m| m.kind == agent) {
            return missing(
                "an agent block asks the agent, and no module fills the agent slot: enable the Agent module"
                    .to_string(),
            );
        }
        if self.host.model().await?.models().is_empty() {
            return missing(
                "an agent block asks a model, and no provider lists one: set a provider's key in Models"
                    .to_string(),
            );
        }
        let Whose::Member(member) = whose else {
            return Ok(());
        };
        let teams = TeamsClient::new(self.channel.clone())
            .list_teams(self.request(ListTeamsRequest {
                workspace_id: self.workspace.clone(),
                team: String::new(),
            })?)
            .await?
            .into_inner()
            .teams;
        in_a_team(&teams, member).or_else(missing)
    }

    async fn session(&self, name: &str) -> Result<String, ModuleError> {
        let scope = self.own();
        HostSessions::new(self.channel.clone(), &scope, &self.workspace)
            .create(name)
            .await
    }
}

#[async_trait]
impl Events for Host {
    async fn publish(
        &self,
        scope: &Scope,
        name: &str,
        payload: pbjson_types::Any,
    ) -> Result<(), ModuleError> {
        self.host
            .bus()
            .await?
            .publish(scope, name, payload)
            .await
            .map(|_| ())
    }

    async fn follow(&self) -> Result<EventFeed, ModuleError> {
        self.host.bus().await?.follow().await
    }

    async fn ack(&self, name: &str, position: u64) -> Result<(), ModuleError> {
        self.host.bus().await?.ack(name, position).await
    }
}

/// The media type of a message blob.
const PROTOBUF: &str = "application/x-protobuf";

/// Bytes of a blob in one chunk at most.
const CHUNK_BYTES: usize = 256 * 1024;

#[async_trait]
impl Blobs for Host {
    async fn put(&self, bytes: Vec<u8>) -> Result<String, ModuleError> {
        let chunks: Vec<PutBlobChunk> = bytes
            .chunks(CHUNK_BYTES)
            .enumerate()
            .map(|(i, data)| PutBlobChunk {
                workspace_id: if i == 0 {
                    self.workspace.clone()
                } else {
                    String::new()
                },
                data: data.to_vec(),
                media_type: if i == 0 {
                    PROTOBUF.to_string()
                } else {
                    String::new()
                },
            })
            .collect();
        let mut client = BlobsClient::new(self.channel.clone());
        let info = client
            .put_blob(self.request(tokio_stream::iter(chunks))?)
            .await?
            .into_inner();
        Ok(info.sha256)
    }

    async fn get(&self, sha256: &str) -> Result<Vec<u8>, ModuleError> {
        let mut client = BlobsClient::new(self.channel.clone());
        let mut chunks = client
            .get_blob(self.request(BlobRequest {
                workspace_id: self.workspace.clone(),
                sha256: sha256.to_string(),
            })?)
            .await?
            .into_inner();
        let mut bytes = Vec::new();
        while let Some(chunk) = chunks.message().await? {
            bytes.extend_from_slice(&chunk.data);
        }
        Ok(bytes)
    }
}

/// The system's clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(test)]
mod tests;
