// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The MCP transport: every tool of the host's modules over rmcp
//! (ARCHITECTURE section 7).
//!
//! `tools/list` is the host's tool list; a call runs through the host's
//! policy within [`TOOL_DEADLINE`], cancelled with the MCP request, with the
//! caller as the default author. [`call_result`] maps a tool's output or
//! error onto MCP in one place. `prompts/list` and `prompts/get` are the
//! host's prompts, in the same scope as a tool call.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_host::Host;
use pcx_module_api::{Effect, ModuleError, Scope, ToolOutput, ToolSpec, metadata};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, CustomNotification,
    ErrorData as McpError, Extensions, GetPromptRequestParams, GetPromptResponse, Implementation,
    InitializeRequestParams, InitializeResult, ListPromptsResult, ListToolsResult, MetaObject,
    PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig, ServerNotification,
    Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use tracing::{debug, warn};

mod author;
mod prompts;

/// How long a call without a session runs before its answer's head is sent:
/// well within a proxy's limit on silence (Cloudflare's 120 s).
const HEAD_AFTER: Duration = Duration::from_secs(10);

/// What opens that head: a notification of ours, which a client may ignore.
const RUNNING_NOTIFICATION: &str = "notifications/pcx/tool_running";

pub use author::{author_value, header_author};

/// Longest a tool call may run.
pub const TOOL_DEADLINE: Duration = Duration::from_secs(300);

/// The MCP service over one host.
#[derive(Clone)]
pub struct McpService {
    host: Arc<Host>,
    instructions: &'static str,
}

impl McpService {
    /// The MCP service over `host`, telling clients `instructions`.
    #[must_use]
    pub fn new(host: Arc<Host>, instructions: &'static str) -> Self {
        Self { host, instructions }
    }
}

impl ServerHandler for McpService {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2024_11_05)
        .with_server_info(Implementation::from_build_env())
        .with_instructions(self.instructions)
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        context.peer.set_peer_info(request);
        if let Some(parts) = context.extensions.get::<axum::http::request::Parts>() {
            debug!("Client initialized from {}", parts.uri);
        }
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(
            self.host.tools().iter().map(tool).collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        // Only a call to a tool that does not exist is a protocol error;
        // every failure of a known tool is a tool error the caller can read.
        if !self.host.has_tool(&request.name) {
            return Err(McpError::invalid_params("tool not found", None));
        }
        let scope = client_scope(&context);
        let arguments = serde_json::Value::Object(request.arguments.unwrap_or_default());
        // An MCP client is an agent: a tool only a person calls is refused.
        let call = self.host.call_for_agent(&scope, &request.name, arguments);
        let outcome = if context.meta.protocol_version().is_some() {
            head_while_it_runs(call, &context.peer).await
        } else {
            call.await
        };
        Ok(call_result(outcome).into())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let listed = self.host.prompts(&client_scope(&context)).await;
        Ok(ListPromptsResult::with_all_items(
            listed.into_iter().map(prompts::prompt).collect(),
        ))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        let arguments = prompts::arguments(request.arguments)?;
        let filled = self
            .host
            .prompt(&client_scope(&context), &request.name, &arguments)
            .await
            .map_err(prompts::error)?;
        Ok(prompts::content(filled)?.into())
    }
}

/// `call`'s outcome, its answer's head sent while it runs. A request that names
/// its protocol version has no session, and the transport sends the head of its
/// answer only with its first message, then keep-alives; a proxy with a limit on
/// silence would cut a call that takes minutes. A call still running after
/// [`HEAD_AFTER`] sends a message, so a short call stays a plain answer.
async fn head_while_it_runs<T>(call: impl Future<Output = T>, peer: &rmcp::Peer<RoleServer>) -> T {
    tokio::pin!(call);
    tokio::select! {
        outcome = &mut call => return outcome,
        () = tokio::time::sleep(HEAD_AFTER) => {}
    }
    let running = CustomNotification::new(RUNNING_NOTIFICATION, None);
    if let Err(e) = peer
        .send_notification(ServerNotification::CustomNotification(running))
        .await
    {
        warn!("the head of a running call was not sent: {e}");
    }
    call.await
}

/// The scope of an MCP request: the caller as the default author, within
/// [`TOOL_DEADLINE`], cancelled with the request.
fn client_scope(context: &RequestContext<RoleServer>) -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: caller_author(&context.peer, &context.extensions),
        caller_module: None,
        call_depth: 0,
        turn: None,
        // A trusted forwarder's (a gateway, a connected daemon's tunnel);
        // one that is not text reaches no workspace.
        reach: context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.headers.get(metadata::REACH))
            .map(|value| {
                value
                    .to_str()
                    .map_or_else(|_| Vec::new(), metadata::reach_of)
            }),
        // An MCP client is an agent: it never acts with a person's role
        // (docs/host.md).
        role: None,
        // A trusted forwarder's mark that the caller may only read
        // (docs/host.md, effects); present with any value, it holds.
        read_only: context
            .extensions
            .get::<axum::http::request::Parts>()
            .is_some_and(|parts| parts.headers.contains_key(metadata::READ_ONLY)),
        // The gateway's: the person the agent acts for with the person's key.
        person: context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.headers.get(metadata::PERSON))
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        deadline: Instant::now() + TOOL_DEADLINE,
        cancel: context.ct.clone(),
    }
}

/// The MCP tool of a module's tool, its effect as MCP's hints
/// (docs/host.md, effects): `read` is read-only, `write` and `external`
/// destructive, `external` open-world.
fn tool(spec: &ToolSpec) -> Tool {
    let read = spec.effect == Effect::Read;
    Tool::new(
        spec.name.clone(),
        spec.description.clone(),
        Arc::new(spec.input_schema.clone()),
    )
    .annotate(
        ToolAnnotations::default()
            .read_only(read)
            .destructive(!read)
            .open_world(spec.effect == Effect::External),
    )
}

/// Where a tool's data starts in the second text block of its result.
pub const RESULTS: &str = "\n\nResults:\n";

/// The MCP result of a tool's outcome: its text (none for an RPC's, which
/// has only data), then its data as `Results:`, its meta as `_meta`; a failure is a tool error (`isError: true`) with its
/// message, then its details as `Details:`. Clients render a tool call's
/// JSON-RPC error opaquely, so a known tool never answers with one.
pub fn call_result(outcome: Result<ToolOutput, ModuleError>) -> CallToolResult {
    let (contents, failed, meta) = match outcome {
        Ok(output) => {
            let mut contents = Vec::new();
            if !output.text.is_empty() {
                contents.push(ContentBlock::text(output.text));
            }
            if let Some(data) = output.data {
                contents.push(ContentBlock::text(format!("{RESULTS}{data}")));
            }
            (contents, output.failed, output.meta)
        }
        Err(error) => {
            let mut contents = vec![ContentBlock::text(error.message)];
            if let Some(details) = error.details {
                contents.push(ContentBlock::text(format!("\n\nDetails:\n{details}")));
            }
            (contents, true, None)
        }
    };
    let mut result = if failed {
        CallToolResult::error(contents)
    } else {
        CallToolResult::success(contents)
    };
    result.meta = meta.map(MetaObject);
    result
}

/// The default author of a tool call: the `X-PCX-Author` header of the
/// HTTP request, else the MCP client's name unless that only names its SDK.
fn caller_author(peer: &rmcp::Peer<RoleServer>, extensions: &Extensions) -> Option<String> {
    let info = peer.peer_info();
    author::default_author(
        extensions.get::<axum::http::request::Parts>(),
        info.as_ref().map(|info| info.client_info.name.as_str()),
    )
}

#[cfg(test)]
mod tests;
