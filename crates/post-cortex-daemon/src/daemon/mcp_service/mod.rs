// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Post-Cortex MCP Service — consolidated 9-tool API.
//!
//! Tools:
//! 1. session                       — create/list/load/search/update_metadata/delete sessions
//! 2. update_conversation_context   — single or bulk updates
//! 3. semantic_search               — unified search with scope (structured payload)
//! 4. get_structured_summary        — summary with optional sections
//! 5. query_conversation_context    — flexible queries
//! 6. manage_workspace              — workspace operations
//! 7. assemble_context              — graph-aware retrieval (semantic + traversal + impact)
//! 8. manage_entity                 — delete entities / context updates from a session
//! 9. admin                         — daemon health, vectorization, checkpoints
//!
//! The `#[tool_router]`-decorated impl block must stay in a single file so the
//! rmcp macro can see every `#[tool]` method at once. Tool method bodies are
//! delegated to per-tool submodules to keep this file readable.

use post_cortex_memory::ConversationMemorySystem;
use crate::daemon::coerce::CoercionError;
use post_cortex_mcp::MCPToolResult;
use rmcp::{
    RoleServer, ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{
        CallToolRequestParam, CallToolResult, Content, ErrorData as McpError, ListToolsResult,
        PaginatedRequestParam, *,
    },
    service::RequestContext,
    tool, tool_router,
};
use std::sync::Arc;
use tracing::info;

mod admin;
mod assembly;
mod entity;
mod query;
mod requests;
mod search;
mod session;
mod summary;
mod update_context;
mod workspace;

// Request types are public so external code (e.g. integration tests) can
// build them, mirroring the original monolith's surface.
pub use requests::{
    AdminRequest, AssembleContextRequest, ContextUpdateItem, GetStructuredSummaryRequest,
    ManageEntityRequest, ManageWorkspaceRequest, QueryConversationContextRequest,
    SemanticSearchRequest, SessionRequest, UpdateConversationContextRequest,
};

/// Post-Cortex MCP Service
#[derive(Clone)]
pub struct PostCortexService {
    memory_system: Arc<ConversationMemorySystem>,
    tool_router: ToolRouter<PostCortexService>,
}

// =============================================================================
// Helper Functions (shared across tool submodules)
// =============================================================================

/// Emit an MCPToolResult through CallToolResult, preserving structured data.
///
/// When `result.data` is present, the structured JSON payload is appended as a
/// second `Content::text` item so MCP clients keep access to scores, snippets
/// and entity tags instead of the human-readable summary alone.
pub(crate) fn mcp_result_to_call_result(result: MCPToolResult) -> CallToolResult {
    let mut contents = vec![Content::text(result.message)];
    if let Some(data) = result.data {
        contents.push(Content::text(format!(
            "\n\nResults:\n{}",
            serde_json::to_string_pretty(&data).unwrap_or_default()
        )));
    }
    CallToolResult::success(contents)
}

/// Check that a session exists in storage; used by dry_run paths so we don't
/// approve an update for a session that never existed.
pub(crate) async fn check_session_exists_for_dry_run(
    memory_system: &Arc<ConversationMemorySystem>,
    session_id: uuid::Uuid,
) -> Result<(), McpError> {
    match memory_system.storage_actor.load_session(session_id).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(CoercionError::new(
            "Session not found",
            std::io::Error::new(std::io::ErrorKind::NotFound, "session does not exist"),
            Some(serde_json::Value::String(session_id.to_string())),
        )
        .with_parameter_path("session_id".to_string())
        .with_hint("Create a session first using the 'session' tool with action='create'")
        .to_mcp_error()),
        Err(e) => Err(McpError::internal_error(
            format!("Failed to check session existence: {}", e),
            None,
        )),
    }
}

/// Parse `(date_from, date_to)` into a UTC range. Both must be supplied; partial
/// pairs return an MCP error. `(None, None)` returns `Ok(None)`.
pub(crate) fn parse_date_range(
    date_from: Option<String>,
    date_to: Option<String>,
) -> Result<Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>, McpError> {
    match (date_from, date_to) {
        (Some(from_str), Some(to_str)) => {
            let from = chrono::DateTime::parse_from_rfc3339(&from_str)
                .map_err(|_| {
                    McpError::invalid_params(
                        format!(
                            "Invalid date_from format: '{}'. Expected RFC3339 format (e.g., 2024-01-01T00:00:00Z)",
                            from_str
                        ),
                        Some(serde_json::Value::String("date_from".to_string())),
                    )
                })?
                .with_timezone(&chrono::Utc);
            let to = chrono::DateTime::parse_from_rfc3339(&to_str)
                .map_err(|_| {
                    McpError::invalid_params(
                        format!(
                            "Invalid date_to format: '{}'. Expected RFC3339 format (e.g., 2024-12-31T23:59:59Z)",
                            to_str
                        ),
                        Some(serde_json::Value::String("date_to".to_string())),
                    )
                })?
                .with_timezone(&chrono::Utc);
            Ok(Some((from, to)))
        }
        (Some(_), None) | (None, Some(_)) => Err(McpError::invalid_params(
            "Both date_from and date_to must be provided together".to_string(),
            Some(serde_json::Value::String("date_from,date_to".to_string())),
        )),
        (None, None) => Ok(None),
    }
}

// =============================================================================
// Tool dispatch — every method here is a thin call into a per-tool submodule.
// =============================================================================

#[tool_router]
impl PostCortexService {
    pub fn new(memory_system: Arc<ConversationMemorySystem>) -> Self {
        info!("Initializing Post-Cortex MCP Service (9 consolidated tools)");
        Self {
            memory_system,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Manage sessions. Actions: create, list, load, search, update_metadata, delete.",
        input_schema = rmcp::handler::server::common::schema_for_type::<SessionRequest>()
    )]
    async fn session(
        &self,
        params: Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        session::handle(&self.memory_system, params).await
    }

    #[tool(
        description = "Add context updates to conversation. Supports single update or bulk mode with updates array.",
        input_schema = rmcp::handler::server::common::schema_for_type::<UpdateConversationContextRequest>()
    )]
    async fn update_conversation_context(
        &self,
        params: Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        update_context::handle(&self.memory_system, params).await
    }

    #[tool(
        description = "Universal semantic search. Scope: session (requires scope_id), workspace (requires scope_id), or global (default).",
        input_schema = rmcp::handler::server::common::schema_for_type::<SemanticSearchRequest>()
    )]
    async fn semantic_search(
        &self,
        params: Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        search::handle(&self.memory_system, params).await
    }

    #[tool(
        description = "Get session summary. Use 'include' to specify sections: decisions, insights, entities, or all (default).",
        input_schema = rmcp::handler::server::common::schema_for_type::<GetStructuredSummaryRequest>()
    )]
    async fn get_structured_summary(
        &self,
        params: Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        summary::handle(&self.memory_system, params).await
    }

    #[tool(
        description = "Query session data. Types include: find_related_entities, get_entity_context, search_updates, entity_importance, entity_network, find_related_content, key_decisions, key_insights, session_statistics, structured_summary, decisions, open_questions, assemble_context.",
        input_schema = rmcp::handler::server::common::schema_for_type::<QueryConversationContextRequest>()
    )]
    async fn query_conversation_context(
        &self,
        params: Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        query::handle(&self.memory_system, params).await
    }

    #[tool(
        description = "Manage workspaces. Actions: create, list, get, delete, add_session, remove_session",
        input_schema = rmcp::handler::server::common::schema_for_type::<ManageWorkspaceRequest>()
    )]
    async fn manage_workspace(
        &self,
        params: Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        workspace::handle(&self.memory_system, params).await
    }

    #[tool(
        description = "Graph-aware context assembly: blends semantic search with entity-graph traversal and impact analysis. Returns ranked items, entity neighbourhood, dependency impact, and an LLM-ready formatted block. Scope: session or workspace.",
        input_schema = rmcp::handler::server::common::schema_for_type::<AssembleContextRequest>()
    )]
    async fn assemble_context(
        &self,
        params: Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        assembly::handle(&self.memory_system, params).await
    }

    #[tool(
        description = "Manage entities and context entries in a session. Actions: delete (remove entity + cascade typed edges; requires entity_name), delete_update (remove a single context entry by entry_id).",
        input_schema = rmcp::handler::server::common::schema_for_type::<ManageEntityRequest>()
    )]
    async fn manage_entity(
        &self,
        params: Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        entity::handle(&self.memory_system, params).await
    }

    #[tool(
        description = "Daemon administration. Actions: health, vectorize_session (session_id), vectorize_stats, create_checkpoint (session_id).",
        input_schema = rmcp::handler::server::common::schema_for_type::<AdminRequest>()
    )]
    async fn admin(
        &self,
        params: Parameters<serde_json::Value>,
    ) -> Result<CallToolResult, McpError> {
        admin::handle(&self.memory_system, params).await
    }
}

// NOTE: We implement ServerHandler manually instead of using #[tool_handler] macro
// to strip $schema from tool input schemas for broader MCP client compatibility.
// Many clients (Cursor, Windsurf, Continue.dev) don't support JSON Schema draft/2020-12.
impl ServerHandler for PostCortexService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Post-Cortex: Intelligent conversation memory system with 9 consolidated MCP tools. \
                 Tools: session, update_conversation_context, semantic_search, get_structured_summary, \
                 query_conversation_context, manage_workspace, assemble_context, manage_entity, admin. \
                 All use shared RocksDB for centralized knowledge management.".to_string()
            ),
        }
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        if let Some(parts) = context.extensions.get::<axum::http::request::Parts>() {
            info!("Client initialized from {}", parts.uri);
        }
        Ok(self.get_info())
    }

    // Custom list_tools that strips $schema from input schemas for client compatibility
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools = self.tool_router.list_all();

        // Strip $schema from each tool's input_schema for broader client compatibility
        // Clone and modify since input_schema is Arc<JsonObject>
        let tools: Vec<Tool> = tools
            .into_iter()
            .map(|mut tool| {
                let mut schema = (*tool.input_schema).clone();
                schema.remove("$schema");
                // Also strip from $defs if present
                if let Some(defs) = schema.get_mut("$defs") {
                    if let Some(defs_obj) = defs.as_object_mut() {
                        for (_, def) in defs_obj.iter_mut() {
                            if let Some(def_obj) = def.as_object_mut() {
                                def_obj.remove("$schema");
                            }
                        }
                    }
                }
                tool.input_schema = std::sync::Arc::new(schema);
                tool
            })
            .collect();

        Ok(ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    // Route tool calls to the tool router
    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }
}
