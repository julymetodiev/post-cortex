// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Arguments of the `manage_entity` tool. The schemars attributes are the tool's
//! published input schema, pinned by the daemon's `tools_list.json`.

use schemars::JsonSchema;
use serde::Deserialize;

/// Request payload for the `manage_entity` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct ManageEntityRequest {
    /// Entity management action.
    #[schemars(description = r#"Action to perform on session content.

Valid values:
- delete: Remove an entity from the session (cascades typed edges). Requires entity_name.
- merge: Merge entity_name into `into` when both name one thing ("Surreal" into "SurrealDB"): mentions,
  relations and counts move over and entity_name becomes an alias. Spellings that differ only in case,
  spaces, '-' or '_' merge on their own. Requires entity_name and into.
- delete_update: Remove a single context update (ghost / bad row) by entry_id. Requires entry_id.
- forget: Retire a record that no longer holds: it stops being valid now but stays in the history
  (search shows its end date). Requires entry_id.
- purge: Erase a record and everything derived from it (embedding, full-text entry, entities and edges
  only it introduced), e.g. for GDPR erasure. Cannot be undone. Requires entry_id."#)]
    pub action: String,
    /// Session UUID that contains the target entity or update.
    #[schemars(description = r#"UUID of the session holding the entity or update."#)]
    pub session_id: String,
    /// Entity name for the `delete` action.
    #[schemars(
        description = r#"The entity, for delete and merge (matched case-insensitively), e.g. "RocksDB"."#
    )]
    pub entity_name: Option<String>,
    /// Context-update ID for the `delete_update` action.
    #[schemars(
        description = r#"The record's entry_id (as search and assemble_context return it), for delete_update, forget and purge."#
    )]
    pub entry_id: Option<String>,
    /// Target entity of the `merge` action.
    #[schemars(description = r#"Entity to merge entity_name into (required for action=merge)."#)]
    #[serde(default)]
    pub into: Option<String>,
}
