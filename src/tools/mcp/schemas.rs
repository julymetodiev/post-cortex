pub fn get_all_tool_schemas() -> Vec<serde_json::Value> {
    use serde_json::json;

    vec![
        json!({
            "name": "create_session",
            "description": "Create a new conversation session with optional name and description",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Optional name for the session"},
                    "description": {"type": "string", "description": "Optional description for the session"}
                }
            }
        }),
        json!({
            "name": "load_session",
            "description": "Load an existing session into active memory",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session to load"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "list_sessions",
            "description": "List all available conversation sessions",
            "inputSchema": {
                "type": "object"
            }
        }),
        json!({
            "name": "search_sessions",
            "description": "Search for sessions by name or description",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query string"}
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "update_session_metadata",
            "description": "Update session name or description",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "name": {"type": "string", "description": "New name (optional)"},
                    "description": {"type": "string", "description": "New description (optional)"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "update_conversation_context",
            "description": "Add new knowledge to the conversation: QA, decisions, problems solved, code changes, requirements, or concepts. Content fields vary by interaction_type: qa={question,answer}, decision_made={decision,rationale}, problem_solved={problem,solution}, code_change={file_path|description,changes|diff}, requirement_added={requirement,priority}, concept_defined={concept,definition}. Extra fields become details.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "interaction_type": {"type": "string", "description": "Type: qa, decision_made, problem_solved, code_change, requirement_added, concept_defined"},
                    "content": {
                        "type": "object",
                        "description": "Content object. Expected fields by type: qa={question,answer}, decision_made={decision,rationale}, problem_solved={problem,solution}, code_change={file_path,changes}, requirement_added={requirement,priority}, concept_defined={concept,definition}. Accepts variations: question/title/query, answer/response, problem/issue/bug, solution/fix, decision/choice, rationale/reason/why, file_path/file/description, changes/diff/changes_made. Extra fields become details array."
                    },
                    "code_reference": {"type": "object", "description": "Optional code reference with file paths and line numbers"}
                },
                "required": ["session_id", "interaction_type", "content"]
            }
        }),
        json!({
            "name": "query_conversation_context",
            "description": "Search conversation context using entities, keywords, or structured queries",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "query_type": {"type": "string", "description": "Query type: find_related_entities, get_entity_context, search_updates, get_most_important_entities"},
                    "parameters": {"type": "object", "description": "Query-specific parameters"}
                },
                "required": ["session_id", "query_type", "parameters"]
            }
        }),
        json!({
            "name": "bulk_update_conversation_context",
            "description": "Add multiple context updates in a single operation for better performance",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "updates": {"type": "array", "description": "Array of context update objects"}
                },
                "required": ["session_id", "updates"]
            }
        }),
        json!({
            "name": "semantic_search_session",
            "description": "Search within a session using AI semantic understanding (auto-vectorizes on first use)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "query": {"type": "string", "description": "Natural language search query"},
                    "limit": {"type": "number", "description": "Max results (default: 10)"},
                    "date_from": {"type": "string", "description": "Filter from date (ISO 8601)"},
                    "date_to": {"type": "string", "description": "Filter to date (ISO 8601)"},
                    "interaction_type": {"type": "array", "description": "Filter by interaction types"},
                    "recency_bias": {
                        "type": "number",
                        "description": "Temporal decay factor: 0.0=disabled (default), 0.1-0.5=soft bias, 1.0+=aggressive bias toward recent content",
                        "default": 0.0,
                        "minimum": 0.0,
                        "maximum": 10.0
                    }
                },
                "required": ["session_id", "query"]
            }
        }),
        json!({
            "name": "semantic_search_global",
            "description": "Search across all sessions using AI semantic understanding",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Natural language search query"},
                    "limit": {"type": "number", "description": "Max results (default: 10)"},
                    "date_from": {"type": "string", "description": "Filter from date (ISO 8601)"},
                    "date_to": {"type": "string", "description": "Filter to date (ISO 8601)"},
                    "interaction_type": {"type": "array", "description": "Filter by interaction types"},
                    "recency_bias": {
                        "type": "number",
                        "description": "Temporal decay factor: 0.0=disabled (default), 0.1-0.5=soft bias, 1.0+=aggressive bias toward recent content",
                        "default": 0.0,
                        "minimum": 0.0,
                        "maximum": 10.0
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "find_related_content",
            "description": "Find content related to a specific topic within a session",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "topic": {"type": "string", "description": "Topic to find related content for"},
                    "limit": {"type": "number", "description": "Max results (default: 10)"}
                },
                "required": ["session_id", "topic"]
            }
        }),
        json!({
            "name": "vectorize_session",
            "description": "Manually trigger vectorization for a session (usually happens automatically)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session to vectorize"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_vectorization_stats",
            "description": "Get statistics about vectorization status and HNSW index",
            "inputSchema": {
                "type": "object"
            }
        }),
        json!({
            "name": "get_structured_summary",
            "description": "Get comprehensive session overview with decisions, entities, questions, and concepts",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "compact": {"type": "boolean", "description": "Use compact format for large sessions"},
                    "decisions_limit": {"type": "number", "description": "Max decisions to return"},
                    "entities_limit": {"type": "number", "description": "Max entities to return"},
                    "questions_limit": {"type": "number", "description": "Max questions to return"},
                    "concepts_limit": {"type": "number", "description": "Max concepts to return"},
                    "min_confidence": {"type": "number", "description": "Minimum confidence threshold (0.0-1.0)"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_key_decisions",
            "description": "Get timeline of key architectural and technical decisions from a session",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_key_insights",
            "description": "Get most important insights from a session ranked by importance",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "limit": {"type": "number", "description": "Max insights to return (default: 10)"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_entity_importance_analysis",
            "description": "Analyze which entities (people, concepts, files) are most important in the conversation",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "limit": {"type": "number", "description": "Max entities to return"},
                    "min_importance": {"type": "number", "description": "Minimum importance score (0.0-1.0)"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_entity_network_view",
            "description": "View the network of relationships between entities in the knowledge graph",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "center_entity": {"type": "string", "description": "Optional entity to center the network on"},
                    "max_entities": {"type": "number", "description": "Max entities to include"},
                    "max_relationships": {"type": "number", "description": "Max relationships to include"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_session_statistics",
            "description": "Get detailed statistics about session size, update counts, and entity counts",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "create_session_checkpoint",
            "description": "Create a snapshot of current session state for backup or versioning",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_tool_catalog",
            "description": "Get comprehensive catalog of all available tools with usage guidance",
            "inputSchema": {
                "type": "object"
            }
        }),
        json!({
            "name": "create_workspace",
            "description": "Create a workspace for organizing related sessions by project or topic",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Workspace name"},
                    "description": {"type": "string", "description": "Workspace description"}
                },
                "required": ["name", "description"]
            }
        }),
        json!({
            "name": "get_workspace",
            "description": "Get details about a specific workspace including all sessions",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace_id": {"type": "string", "description": "UUID of the workspace"}
                },
                "required": ["workspace_id"]
            }
        }),
        json!({
            "name": "list_workspaces",
            "description": "List all available workspaces",
            "inputSchema": {
                "type": "object"
            }
        }),
        json!({
            "name": "delete_workspace",
            "description": "Delete a workspace (does not delete the sessions within it)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace_id": {"type": "string", "description": "UUID of the workspace"}
                },
                "required": ["workspace_id"]
            }
        }),
        json!({
            "name": "add_session_to_workspace",
            "description": "Add a session to a workspace with a specific role",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace_id": {"type": "string", "description": "UUID of the workspace"},
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "role": {"type": "string", "description": "Role of this session in the workspace"}
                },
                "required": ["workspace_id", "session_id", "role"]
            }
        }),
        json!({
            "name": "remove_session_from_workspace",
            "description": "Remove a session from a workspace",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace_id": {"type": "string", "description": "UUID of the workspace"},
                    "session_id": {"type": "string", "description": "UUID of the session"}
                },
                "required": ["workspace_id", "session_id"]
            }
        }),
    ]
}
