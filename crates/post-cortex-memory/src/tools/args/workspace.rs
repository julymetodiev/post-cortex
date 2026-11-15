// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Arguments of the `manage_workspace` tool. The schemars attributes are the tool's
//! published input schema, pinned by the daemon's `tools_list.json`.

use schemars::JsonSchema;
use serde::Deserialize;

/// Request payload for the `manage_workspace` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct ManageWorkspaceRequest {
    /// Workspace action (`create`, `list`, `get`, `delete`, `add_session`, `remove_session`,
    /// `get_card`, `update_card`, `update`, `archive`, `unarchive`, `dismiss_radar_item`,
    /// `restore_radar_item`, `list_radar_dismissals`).
    #[schemars(description = r#"Action:
- create (name, description)
- list (include_archived)
- get, delete, archive, unarchive (workspace_id)
- update: rename and/or redescribe (workspace_id; name, description)
- add_session (workspace_id, session_id; role), remove_session (workspace_id, session_id)
- get_card: the project card (workspace_id) or the user card (card "user"); history
- update_card: the next version of a card (content, expected_version; workspace_id or card "user")
- dismiss_radar_item (workspace_id, kind, key; author), restore_radar_item (workspace_id, kind, key), list_radar_dismissals (workspace_id)

Cards are markdown notes of at most 2 KB put in front of every session: the project card holds the stack, conventions and goals, the user card preferences. update_card needs the version you read (0 for a new card); if it changed since, nothing is written and the latest comes back to merge."#)]
    pub action: String,
    /// Workspace UUID for actions that target a specific workspace.
    #[schemars(
        description = r#"Workspace UUID, for every action but create, list and the user card's (find it with action list)."#
    )]
    pub workspace_id: Option<String>,
    /// Session UUID for `add_session`/`remove_session`.
    #[schemars(description = r#"Session UUID, for add_session and remove_session."#)]
    pub session_id: Option<String>,
    /// Workspace name (used by `create`).
    #[schemars(description = r#"Workspace name, for create and update."#)]
    pub name: Option<String>,
    /// Workspace description (used by `create`).
    #[schemars(description = r#"Workspace description, for create and update."#)]
    pub description: Option<String>,
    /// Session role within the workspace (used by `add_session`).
    #[schemars(
        description = r#"Role of the session, for add_session: primary, related (default), dependency or shared."#
    )]
    pub role: Option<String>,
    /// Which card `get_card`/`update_card` act on: `workspace` or `user`.
    #[schemars(
        description = r#"Which card get_card and update_card act on: workspace (default; the project card of workspace_id) or user (the user's card)."#
    )]
    pub card: Option<String>,
    /// New card text (used by `update_card`).
    #[schemars(
        description = r#"New card text for update_card: markdown, at most 2 KB, replacing the whole card (sections: Stack, Conventions, Preferences, Current goals)."#
    )]
    pub content: Option<String>,
    /// Version the new text was edited from (used by `update_card`).
    #[schemars(
        description = r#"For update_card: the version your content was edited from (get_card shows it; 0 when there is no card)."#
    )]
    pub expected_version: Option<u64>,
    /// Older versions to return (used by `get_card`).
    #[schemars(description = r#"For get_card: this many older versions as well (default 0)."#)]
    pub history: Option<u32>,
    /// Who writes the card (used by `update_card`).
    #[schemars(
        description = r#"Who writes the card, for update_card (optional): your agent name, the user, a teammate. Default: the connection's X-PCX-Author header, else the MCP client's name."#
    )]
    pub author: Option<String>,
    /// Include archived workspaces (used by `list`).
    #[schemars(description = r#"For list: include archived workspaces (default false)."#)]
    pub include_archived: Option<bool>,
    /// Radar item kind (used by the radar dismissal actions).
    #[schemars(
        description = r#"For the radar actions: the item's list: unsolved_problem, decision_without_rationale, open_question, stale_entity or alias_candidate."#
    )]
    pub kind: Option<String>,
    /// Radar item key (used by the radar dismissal actions).
    #[schemars(
        description = r#"For the radar actions: which item: the entity name (unsolved_problem, stale_entity), the record id (decision_without_rationale), the question text (open_question) or "<name>|<of>" (alias_candidate), as the radar lists them."#
    )]
    pub key: Option<String>,
}
