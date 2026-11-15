// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Arguments of the `session` tool. The schemars attributes are the tool's
//! published input schema, pinned by the daemon's `tools_list.json`.

use schemars::JsonSchema;
use serde::Deserialize;

/// Request payload for the `session` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct SessionRequest {
    /// Session management action (`create`, `list`, `load`, `search`, `update_metadata`, `delete`, `delete_message`, `delete_messages`, `resolve_project`).
    #[schemars(description = r#"Action:
- create (name, description): a new session's UUID
- list
- load (session_id): one session's metadata
- search (query): sessions by name or description
- update_metadata (session_id; name and/or description)
- delete (session_id): the session and its updates, for good
- delete_message (session_id, seq): one captured turn; records derived from it keep its id
- delete_messages (session_id): every captured turn; records stay
- resolve_project (key, name; workspace): find or create the project's workspace and session; the same key always gives the same ones"#)]
    pub action: String,
    /// Session name (used by `create`, `update_metadata` and `resolve_project`).
    #[schemars(
        description = r#"Session name, for create and update_metadata; for resolve_project, the name of a new workspace and session (1 to 200 characters)."#
    )]
    pub name: Option<String>,
    /// Session description (used by `create` and `update_metadata`).
    #[schemars(description = r#"Session description, for create and update_metadata."#)]
    pub description: Option<String>,
    /// Session UUID (required for `load`, `update_metadata`, `delete`).
    #[schemars(
        description = r#"Session UUID, for load, update_metadata, delete, delete_message and delete_messages."#
    )]
    pub session_id: Option<String>,
    /// Search query for the `search` action.
    #[schemars(
        description = r#"For search: a case-insensitive substring of a session's name or description."#
    )]
    pub query: Option<String>,
    /// Project key for `resolve_project`.
    #[schemars(
        description = r#"For resolve_project: the git remote origin URL without scheme, user and .git, host lowercased (git@host:a/b is host/a/b), or local:<first 16 hex of the sha256 of the absolute git root or working directory>. 1 to 512 printable characters, no whitespace. Example: "github.com/julymetodiev/post-cortex"."#
    )]
    pub key: Option<String>,
    /// Workspace name for `resolve_project`.
    #[schemars(
        description = r#"For resolve_project: put a new key in the workspace of this name (created if missing) instead of a new one named after the project. Ignored when the key is bound already."#
    )]
    pub workspace: Option<String>,
    /// Message position for `delete_message`.
    #[schemars(
        description = r#"For delete_message: the turn's position, the number in its content id message:<seq>."#
    )]
    pub seq: Option<u64>,
}
