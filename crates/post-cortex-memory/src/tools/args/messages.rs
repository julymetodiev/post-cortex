// Copyright (c) 2026 Julius ML
// MIT License

//! Arguments of the `manage_messages` tool. The schemars attributes are the
//! tool's published input schema.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

/// Request payload for the `manage_messages` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct ManageMessagesRequest {
    /// `append` or `page`.
    #[schemars(
        description = r#"Action on a session's message stream (its conversation turns).

Valid values:
- append: Store turns at the end of the stream, in order (messages); a turn already stored (its turn_id) is skipped
- page: Read the stream oldest first from a position (from, limit); next_from is where the next page starts

An agent keeps its turns whole: each with the turn it follows (parent) and its blocks (text,
thinking, tool_use, tool_result); text is what search reads."#
    )]
    pub action: String,
    /// The session whose stream it is.
    #[schemars(description = "Session ID (36-char UUID) whose stream it is. Required.")]
    pub session_id: String,
    /// The turns of an append.
    #[schemars(description = "For append: the turns, in order.")]
    pub messages: Option<Vec<MessageInput>>,
    /// Where a page starts.
    #[schemars(description = "For page: the first position read (default 0).")]
    pub from: Option<u64>,
    /// At most this many.
    #[schemars(description = "For page: at most this many turns (default 100, max 1000).")]
    pub limit: Option<u32>,
}

/// One turn of an append.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct MessageInput {
    /// The client's id for the turn; appends are idempotent on it.
    #[schemars(
        description = "The turn's id, unique in the session; an append skips one stored already."
    )]
    pub turn_id: String,
    /// `user` or `assistant`.
    #[schemars(description = "user (a person, or tool results) or assistant (the model).")]
    pub role: String,
    /// What search reads.
    #[schemars(
        description = "The turn's text (what search reads); may be empty when it has blocks."
    )]
    pub text: Option<String>,
    /// The turn it follows.
    #[schemars(description = "The turn_id it follows (a fork); none: the one before it.")]
    pub parent: Option<String>,
    /// Its blocks.
    #[schemars(
        description = "The turn whole: [{\"type\":\"text\",\"text\":..}, {\"type\":\"thinking\",\"text\":..,\"signature\":null}, {\"type\":\"tool_use\",\"id\":..,\"name\":..,\"input\":{..}}, {\"type\":\"tool_result\",\"tool_use_id\":..,\"content\":..,\"is_error\":false}]"
    )]
    pub blocks: Option<Vec<Value>>,
    /// The tools it used.
    #[schemars(description = "Names of the tools the turn used.")]
    pub tools: Option<Vec<String>>,
    /// Who wrote it, when not the caller.
    #[schemars(description = "Who wrote the turn (optional; defaults to the caller).")]
    pub author: Option<String>,
}
