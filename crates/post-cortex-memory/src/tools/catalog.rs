// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The memory module's tools, built once at startup from the tools'
//! argument types and listed by the host to every transport.

use pcx_module_api::{Effect, ToolSpec, Visibility};
use serde_json::{Map, Value};

use crate::tools::args::{
    AdminRequest, AssembleContextRequest, GetStructuredSummaryRequest, ManageDocumentRequest,
    ManageEntityRequest, ManageMessagesRequest, ManageWorkspaceRequest,
    QueryConversationContextRequest, SemanticSearchRequest, SessionRequest,
    UpdateConversationContextRequest,
};

/// A tool's argument schema could not be built.
#[derive(Debug, thiserror::Error)]
#[error("input schema of tool {tool}: {reason}")]
pub struct SchemaError {
    /// The tool.
    pub tool: &'static str,
    /// Why.
    pub reason: String,
}

/// The memory tools, with their schemas.
#[derive(Debug, Clone)]
pub struct ToolCatalog {
    tools: Vec<ToolSpec>,
}

/// One tool: its name, description, effect (a tool of several actions
/// states the worst of them: docs/host.md, effects) and argument schema.
struct Spec {
    name: &'static str,
    description: &'static str,
    effect: Effect,
    schema: fn() -> Result<Map<String, Value>, String>,
}

/// The tools, by name: the order `tools/list` has always listed them in.
const SPECS: &[Spec] = &[
    Spec {
        name: "admin",
        description: "Daemon administration. Actions: health, vectorize_session (session_id), vectorize_stats, create_checkpoint (session_id).",
        effect: Effect::Write,
        schema: input_schema::<AdminRequest>,
    },
    Spec {
        name: "assemble_context",
        description: "Everything for one task in one call, within token_budget: the user and project cards first, then the graph entities named in the query with their one-hop neighbourhood and what depends on them (at most half of what the cards leave), then the records that best match the query, ranked by the same search as semantic_search (only records that still hold, each shown as its passage for the query). With sources=messages or all, also adds captured conversation turns matching the query as excerpts. Returns ranked items, entity neighbourhood, dependency impact, and an LLM-ready formatted block. Scope: session or workspace.",
        effect: Effect::Read,
        schema: input_schema::<AssembleContextRequest>,
    },
    Spec {
        name: "get_structured_summary",
        description: "Get session summary. Use 'include' to specify sections: decisions, insights, entities, or all (default).",
        effect: Effect::Read,
        schema: input_schema::<GetStructuredSummaryRequest>,
    },
    Spec {
        name: "manage_document",
        description: "The documents modules keep in a workspace, versioned. Actions: put (kind, data, text, expected_version; document_id for a change), get, history, list (kind, fields, limit, cursor), delete. Only the module that declares a kind writes it; anyone who reads the workspace reads it.",
        effect: Effect::Write,
        schema: input_schema::<ManageDocumentRequest>,
    },
    Spec {
        name: "manage_messages",
        description: "A session's message stream, its conversation turns. Actions: append (messages: each with turn_id, role, text, and for an agent's turn its parent and blocks), page (from, limit; oldest first, next_from for the next page). Agents keep their turns whole here to resume them.",
        effect: Effect::Write,
        schema: input_schema::<ManageMessagesRequest>,
    },
    Spec {
        name: "manage_entity",
        description: "Manage entities and context entries in a session. Actions: delete (remove entity + cascade typed edges; requires entity_name), delete_update (remove a single context entry by entry_id).",
        effect: Effect::Write,
        schema: input_schema::<ManageEntityRequest>,
    },
    Spec {
        name: "manage_workspace",
        description: "Manage workspaces and their cards. Actions: create, list, get, delete, add_session, remove_session; get_card / update_card for the project card (workspace_id) or the user card (card: \"user\"): short markdown notes put in front of every session",
        effect: Effect::Write,
        schema: input_schema::<ManageWorkspaceRequest>,
    },
    Spec {
        name: "query_conversation_context",
        description: "Query session data. Types include: find_related_entities, get_entity_context, search_updates, entity_importance, entity_network, find_related_content, key_decisions, key_insights, session_statistics, structured_summary, decisions, open_questions, assemble_context, record (one record whole by entry_id, e.g. after a search snippet).",
        effect: Effect::Read,
        schema: input_schema::<QueryConversationContextRequest>,
    },
    Spec {
        name: "semantic_search",
        description: "Universal semantic search. Scope: session (requires scope_id), workspace (requires scope_id), or global (default).",
        effect: Effect::Read,
        schema: input_schema::<SemanticSearchRequest>,
    },
    Spec {
        name: "session",
        description: "Manage sessions. Actions: create, list, load, search, update_metadata, delete; resolve_project (key, name, optional workspace) finds or creates the workspace and session of a project.",
        effect: Effect::Write,
        schema: input_schema::<SessionRequest>,
    },
    Spec {
        name: "update_conversation_context",
        description: "Add context updates to conversation. Supports single update or bulk mode with updates array.",
        effect: Effect::Write,
        schema: input_schema::<UpdateConversationContextRequest>,
    },
];

impl ToolCatalog {
    /// Build every tool's schema once.
    ///
    /// # Errors
    /// A schema could not be built (a startup error, never an empty schema).
    pub fn build() -> Result<Self, SchemaError> {
        let tools = SPECS
            .iter()
            .map(|spec| {
                let schema = (spec.schema)().map_err(|reason| SchemaError {
                    tool: spec.name,
                    reason,
                })?;
                Ok(ToolSpec {
                    name: spec.name.to_string(),
                    description: spec.description.to_string(),
                    input_schema: schema,
                    visibility: Visibility::Both,
                    effect: spec.effect,
                })
            })
            .collect::<Result<Vec<_>, SchemaError>>()?;
        Ok(Self { tools })
    }

    /// The tools, in list order.
    #[must_use]
    pub fn tools(&self) -> &[ToolSpec] {
        &self.tools
    }
}

/// JSON Schema for a tool's input, in the shape the daemon has always
/// published: draft 2020-12 with optional fields as `"nullable": true` and
/// no `$schema` keys (Cursor, Windsurf and Continue.dev do not support
/// draft 2020-12 references).
///
/// rmcp 3's `schema_for_type` dropped schemars' `AddNullable` transform and
/// emits `"type": ["string", "null"]` instead; generating the schema here
/// keeps `tools/list` compatible for existing MCP clients.
fn input_schema<T: schemars::JsonSchema>() -> Result<Map<String, Value>, String> {
    let mut settings = schemars::generate::SchemaSettings::draft2020_12();
    settings.transforms = vec![Box::new(schemars::transform::AddNullable::default())];
    let schema = settings.into_generator().into_root_schema_for::<T>();
    let mut object = match serde_json::to_value(schema) {
        Ok(Value::Object(object)) => object,
        Ok(other) => return Err(format!("not a JSON object: {other}")),
        Err(e) => return Err(e.to_string()),
    };
    object.remove("$schema");
    if let Some(defs) = object.get_mut("$defs").and_then(|d| d.as_object_mut()) {
        for def in defs.values_mut() {
            if let Some(def) = def.as_object_mut() {
                def.remove("$schema");
            }
        }
    }
    Ok(object)
}

#[cfg(test)]
mod tests;
