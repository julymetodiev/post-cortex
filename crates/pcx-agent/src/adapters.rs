// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The agent's ports over its host handle, in the turn's scope: the
//! transcript through memory's `pcx.v1` messages (W23) on the module's
//! host channel, the tools of the modules it uses (their RPCs, called on
//! that channel), its events through the host's dispatch.

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::model::{Block, ModelFailure, ModelTool, Role};
use pcx_module_api::{
    Event, EventKind, ModuleError, ModuleErrorKind, ModuleHost, Outcome, Scope, UsedTool,
};
use post_cortex_proto::pb::post_cortex_client::PostCortexClient;
use post_cortex_proto::pb::{AppendMessagesRequest, CapturedMessage, ListMessagesRequest};
use serde_json::Value;
use tonic::Request;

use crate::domain::Turn;
use crate::ports::{Hooks, ToolAnswer, Tools, Transcript};

/// Turns read per page.
const PAGE: u32 = 500;

/// The ports of one turn.
pub struct HostPorts<'a> {
    host: &'a dyn ModuleHost,
    channel: GrpcChannel,
    scope: &'a Scope,
    /// The workspace of a call whose scope has none and whose input names none.
    workspace: Option<String>,
    /// The tools the turn listed, which its calls name.
    listed: Mutex<Vec<UsedTool>>,
}

impl<'a> HostPorts<'a> {
    /// The ports over `host` and its `channel`, in `scope`; a call that has
    /// no workspace otherwise runs in `workspace`.
    pub fn new(
        host: &'a dyn ModuleHost,
        channel: GrpcChannel,
        scope: &'a Scope,
        workspace: Option<String>,
    ) -> Self {
        Self {
            host,
            channel,
            scope,
            workspace,
            listed: Mutex::new(Vec::new()),
        }
    }

    /// `message` as a request in the turn's scope.
    fn request<T>(&self, message: T) -> Result<Request<T>, ModuleError> {
        let mut request = Request::new(message);
        self.scope.to_metadata(request.metadata_mut())?;
        Ok(request)
    }
}

fn role(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// A stored turn's role.
fn role_of(turn: &CapturedMessage) -> Result<Role, ModuleError> {
    match turn.role.as_str() {
        "user" => Ok(Role::User),
        "assistant" => Ok(Role::Assistant),
        other => Err(ModuleError::new(
            ModuleErrorKind::Internal,
            format!("turn {} is stored with the role {other}", turn.turn_id),
        )),
    }
}

/// A block that does not cross the wire, as the transcript's failure.
fn unreadable(turn: &str) -> impl Fn(ModelFailure) -> ModuleError + '_ {
    move |e| {
        ModuleError::new(
            ModuleErrorKind::Internal,
            format!("turn {turn}: {}", e.message),
        )
    }
}

#[async_trait]
impl Transcript for HostPorts<'_> {
    async fn load(&self, session: &str) -> Result<Vec<Turn>, ModuleError> {
        let mut client = PostCortexClient::new(self.channel.clone());
        let mut turns = Vec::new();
        let mut from = 0;
        loop {
            let page = client
                .list_messages(self.request(ListMessagesRequest {
                    session_id: session.to_string(),
                    from,
                    limit: PAGE,
                })?)
                .await?
                .into_inner();
            for stored in page.messages {
                let Some(turn) = stored.message else {
                    continue;
                };
                let role = role_of(&turn)?;
                let blocks = turn
                    .blocks
                    .into_iter()
                    .map(Block::from_wire)
                    .collect::<Result<_, _>>()
                    .map_err(unreadable(&turn.turn_id))?;
                turns.push(Turn {
                    turn_id: turn.turn_id,
                    role,
                    text: turn.text,
                    parent: turn.parent,
                    blocks,
                    author: turn.author,
                });
            }
            match page.next_from {
                Some(next) => from = next,
                None => return Ok(turns),
            }
        }
    }

    async fn append(&self, session: &str, turns: &[Turn]) -> Result<(), ModuleError> {
        let messages = turns
            .iter()
            .map(|t| {
                Ok(CapturedMessage {
                    turn_id: t.turn_id.clone(),
                    role: role(t.role).to_string(),
                    text: t.text.clone(),
                    parent: t.parent.clone(),
                    blocks: t
                        .blocks
                        .iter()
                        .cloned()
                        .map(Block::to_wire)
                        .collect::<Result<_, _>>()
                        .map_err(unreadable(&t.turn_id))?,
                    tools: t.tools(),
                    author: t.author.clone(),
                    ..CapturedMessage::default()
                })
            })
            .collect::<Result<Vec<_>, ModuleError>>()?;
        PostCortexClient::new(self.channel.clone())
            .append_messages(self.request(AppendMessagesRequest {
                session_id: session.to_string(),
                messages,
            })?)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Tools for HostPorts<'_> {
    async fn list(&self) -> Result<Vec<(String, ModelTool)>, ModuleError> {
        let used = self.host.tools(self.scope).await?;
        let tools = used
            .iter()
            .map(|t| {
                let tool = ModelTool {
                    name: t.tool.spec.name.clone(),
                    description: t.tool.spec.description.clone(),
                    input_schema: t.tool.spec.input_schema.clone(),
                };
                (t.module.clone(), tool)
            })
            .collect();
        *self.listed.lock() = used;
        Ok(tools)
    }

    async fn call(&self, name: &str, input: Value) -> ToolAnswer {
        let found = self
            .listed
            .lock()
            .iter()
            .find(|t| t.tool.spec.name == name)
            .map(|t| t.tool.clone());
        let Some(tool) = found else {
            return ToolAnswer {
                content: format!("{name} is not one of the tools of this turn"),
                is_error: true,
                offline: false,
            };
        };
        let named = input["workspace_id"]
            .as_str()
            .is_some_and(|w| !w.is_empty());
        let scope = Scope {
            workspace_id: match &self.scope.workspace_id {
                None if !named => self.workspace.clone(),
                ours => ours.clone(),
            },
            ..self.scope.clone()
        };
        match tool.call(self.channel.clone(), &scope, input).await {
            // An RPC's answer is its data.
            Ok(output) => ToolAnswer {
                content: output.data.map_or(output.text, |data| data.to_string()),
                is_error: output.failed,
                offline: false,
            },
            Err(error) => ToolAnswer {
                content: match &error.details {
                    Some(details) => format!("{}\n\n{details}", error.message),
                    None => error.message.clone(),
                },
                is_error: true,
                offline: error.kind == ModuleErrorKind::Offline,
            },
        }
    }
}

#[async_trait]
impl Hooks for HostPorts<'_> {
    async fn send(&self, kind: EventKind, event: Event) -> Result<Outcome, ModuleError> {
        match self.host.dispatch(self.scope, kind, &event).await {
            Err(e) if e.kind == ModuleErrorKind::Denied => Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!("the agent is not in the agent slot: {}", e.message),
            )),
            other => other,
        }
    }
}
