// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Arguments of the `admin` tool. The schemars attributes are the tool's
//! published input schema, pinned by the daemon's `tools_list.json`.

use schemars::JsonSchema;
use serde::Deserialize;

/// Request payload for the `admin` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct AdminRequest {
    /// Administrative action (`health`, `vectorize_session`, `vectorize_stats`, `create_checkpoint`).
    #[schemars(description = r#"Administrative action on the Post-Cortex daemon.

Valid values:
- health: Report daemon health (active sessions, embeddings status, version)
- vectorize_session: Backfill embeddings for an existing session (requires session_id)
- vectorize_stats: Return embedding-pipeline statistics as JSON
- create_checkpoint: Persist a snapshot of a session for later recall (requires session_id)"#)]
    pub action: String,
    /// Session UUID for actions that target a specific session.
    #[schemars(description = r#"Session UUID, for vectorize_session and create_checkpoint."#)]
    pub session_id: Option<String>,
}
