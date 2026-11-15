// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool `manage_entity`: delete and merge entities; delete, forget and
//! purge records.

use crate::Memory;
use serde_json::Value;
use uuid::Uuid;

use crate::tools::Failure;
use crate::tools::args::ManageEntityRequest;
use crate::tools::check;
use crate::tools::handlers::Outcome;
use crate::tools::parse::{FieldHint, arguments};
use crate::tools::reply;
use crate::tools::responses::{EntitiesMerged, EntityDeleted, RecordChanged};

const TOOL: &str = "manage_entity";

const HINTS: &[FieldHint] = &[
    FieldHint {
        field: "action",
        expected: "one of: delete, merge, delete_update, forget, purge",
        hint: "Use 'delete' with entity_name for entities, or 'delete_update' with entry_id for context rows",
    },
    FieldHint {
        field: "session_id",
        expected: "UUID string (36 chars with hyphens)",
        hint: "",
    },
];

pub async fn handle(memory: &Memory, params: Value) -> Outcome {
    let req: ManageEntityRequest = arguments(params, HINTS)?;
    let session_id = check::session_id(&req.session_id)?;
    let action = req.action.to_lowercase();
    match action.as_str() {
        "delete" => {
            let entity_name = req.entity_name.as_deref().ok_or_else(|| {
                Failure::invalid("action 'delete' requires entity_name").parameter("entity_name")
            })?;
            let existed = memory
                .entities
                .delete(session_id, entity_name)
                .await
                .map_err(Failure::from)?;
            reply::with(
                if existed {
                    format!("Deleted entity '{entity_name}' from session {session_id}")
                } else {
                    format!("Entity '{entity_name}' not present in session {session_id} (no-op)")
                },
                &EntityDeleted {
                    existed,
                    entity_name: entity_name.to_string(),
                    session_id: session_id.to_string(),
                },
            )
        }
        "merge" => {
            let name = |v: &Option<String>, field: &str| {
                v.clone().ok_or_else(|| {
                    Failure::invalid(format!("action 'merge' requires {field}")).parameter(field)
                })
            };
            let alias = name(&req.entity_name, "entity_name")?;
            let into = name(&req.into, "into")?;
            let merged = memory
                .entities
                .merge(session_id, &alias, &into)
                .await
                .map_err(Failure::from)?;
            reply::with(
                if merged {
                    format!("Merged entity '{alias}' into '{into}' in session {session_id}")
                } else {
                    format!(
                        "Nothing merged: '{alias}' or '{into}' is not in session {session_id}, or they are one entity"
                    )
                },
                &EntitiesMerged {
                    merged,
                    entity_name: alias,
                    into,
                    session_id: session_id.to_string(),
                },
            )
        }
        "delete_update" | "forget" | "purge" => {
            let entry_id = entry_id(&req, &action)?;
            let done = match action.as_str() {
                "delete_update" => memory.records.delete(session_id, entry_id).await,
                "forget" => memory.records.forget(session_id, entry_id).await,
                _ => memory.records.purge(session_id, entry_id).await,
            }
            .map_err(Failure::from)?;
            let verb = match action.as_str() {
                "delete_update" => "Deleted",
                "forget" => "Retired",
                _ => "Purged",
            };
            reply::with(
                if done {
                    format!("{verb} update {entry_id} in session {session_id}")
                } else if action == "forget" {
                    format!(
                        "Update {entry_id} not in session {session_id} or already retired (no-op)"
                    )
                } else {
                    format!("Update {entry_id} not present in session {session_id} (no-op)")
                },
                &RecordChanged {
                    existed: done,
                    entry_id: entry_id.to_string(),
                    session_id: session_id.to_string(),
                },
            )
        }
        other => Err(crate::tools::handlers::unhandled(TOOL, other)),
    }
}

/// The `entry_id` an action on a record needs.
fn entry_id(req: &ManageEntityRequest, action: &str) -> Result<Uuid, Failure> {
    let raw = req.entry_id.as_deref().ok_or_else(|| {
        Failure::invalid(format!("action '{action}' requires entry_id")).parameter("entry_id")
    })?;
    check::uuid(
        "entry_id",
        raw,
        "Invalid UUID: Invalid UUID",
        "UUID string (36 chars with hyphens)",
        "entry_id is the ContextUpdate id (e.g. the entry_id returned by assemble_context items)",
    )
}
