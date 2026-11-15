// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool `session`: create, list, load, search, update_metadata, delete,
//! delete_message / delete_messages (the captured message stream),
//! resolve_project.

use crate::Memory;
use post_cortex_core::services::ResolveProjectRequest;
use serde_json::Value;
use uuid::Uuid;

use crate::tools::Failure;
use crate::tools::args::SessionRequest;
use crate::tools::check;
use crate::tools::handlers::Outcome;
use crate::tools::parse::{FieldHint, arguments};
use crate::tools::reply;
use crate::tools::responses::{
    MessagesDeleted, SessionInfo, SessionList, SessionListing, SessionSearch,
};

const TOOL: &str = "session";

const ACTION_HINT: &str = "Valid actions: create (name, description), list, load (session_id), search (query), update_metadata (session_id + name/description), delete (session_id), delete_message (session_id, seq), delete_messages (session_id), resolve_project (key, name, workspace)";

const HINTS: &[FieldHint] = &[FieldHint {
    field: "action",
    expected: "one of: create, list, load, search, update_metadata, delete, delete_message, delete_messages, resolve_project",
    hint: ACTION_HINT,
}];

/// `client` is the caller's name, logged by `resolve_project`.
pub async fn handle(memory: &Memory, params: Value, client: Option<String>) -> Outcome {
    let req: SessionRequest = arguments(params, HINTS)?;
    let action = crate::tools::handlers::action(TOOL, &req.action, ACTION_HINT)?;
    match action.as_str() {
        "create" => {
            let session_id = memory
                .sessions
                .create(req.name.clone(), req.description.clone())
                .await
                .map_err(Failure::from)?;
            reply::with(
                format!(
                    "Created session: {}{}{}",
                    session_id,
                    req.name
                        .as_ref()
                        .map(|n| format!(" (name: {n})"))
                        .unwrap_or_default(),
                    req.description
                        .as_ref()
                        .map(|d| format!(" - {d}"))
                        .unwrap_or_default(),
                ),
                &crate::tools::responses::SessionCreated {
                    session_id: session_id.to_string(),
                    name: req.name.clone(),
                    description: req.description.clone(),
                },
            )
        }
        "list" => {
            // Metadata records only: listing neither loads sessions nor
            // fills the session cache; entity counts are known for cached
            // sessions only.
            let summaries = memory
                .sessions
                .list_summaries()
                .await
                .map_err(Failure::from)?;
            let sessions: Vec<SessionListing> = summaries
                .iter()
                .map(|s| {
                    let entities = memory
                        .sessions
                        .cached(s.id)
                        .map(|session| session.entity_graph().entities.len());
                    SessionListing::new(s, entities)
                })
                .collect();
            reply::with(
                format!("Found {} sessions", sessions.len()),
                &SessionList { sessions },
            )
        }
        "load" => {
            let session_id = required_session(&req, "load")?;
            let session = memory.sessions.get(session_id).await.map_err(|e| {
                let message = format!("Session not found: {e}");
                Failure::from(e).with_message(message)
            })?;
            let info = SessionInfo {
                session_id: session_id.to_string(),
                name: session.name().unwrap_or_default(),
                description: session.description().unwrap_or_default(),
                created_at_unix: session.created_at().timestamp(),
                update_count: session.records().len(),
            };
            reply::with(format!("Loaded session {session_id}"), &info)
        }
        "search" => {
            let query = req.query.as_ref().ok_or_else(|| {
                check::missing(
                    "query",
                    "search query string",
                    "Action 'search' requires a non-empty 'query' for name/description substring match",
                )
            })?;
            // Metadata records only: searching never loads sessions.
            let sessions: Vec<SessionInfo> = memory
                .sessions
                .find(query)
                .await
                .map_err(Failure::from)?
                .iter()
                .map(SessionInfo::from)
                .collect();
            reply::with(
                format!("Found {} matching sessions", sessions.len()),
                &SessionSearch {
                    query: query.clone(),
                    sessions,
                },
            )
        }
        "update_metadata" => {
            let session_id = required_session(&req, "update_metadata")?;
            memory
                .sessions
                .update_metadata(session_id, req.name.clone(), req.description.clone())
                .await
                .map_err(Failure::from)?;
            reply::with(
                format!("Updated metadata for session {session_id}"),
                &crate::tools::responses::SessionUpdated {
                    session_id: session_id.to_string(),
                },
            )
        }
        "delete" => {
            let session_id = required_session(&req, "delete")?;
            let deleted = memory
                .sessions
                .delete(session_id)
                .await
                .map_err(Failure::from)?;
            reply::with(
                if deleted {
                    format!("Deleted session {session_id}")
                } else {
                    format!("Session {session_id} was not present (no-op)")
                },
                &crate::tools::responses::SessionDeleted {
                    session_id: session_id.to_string(),
                    deleted,
                },
            )
        }
        "delete_message" | "delete_messages" => delete_messages(memory, &req, &action).await,
        "resolve_project" => {
            let (Some(key), Some(name)) = (req.key.clone(), req.name.clone()) else {
                return Err(Failure::invalid(
                    "resolve_project needs key (the project key) and name (its display name)",
                )
                .parameter("key,name"));
            };
            let response = memory
                .projects
                .resolve(ResolveProjectRequest {
                    key: key.clone(),
                    name,
                    workspace: req.workspace.clone(),
                    client,
                })
                .await
                .map_err(Failure::from)?;
            reply::with(
                format!(
                    "Project {}: workspace {}, session {}{}",
                    key,
                    response.workspace_id,
                    response.session_id,
                    if response.created { " (created)" } else { "" }
                ),
                &response,
            )
        }
        other => Err(crate::tools::handlers::unhandled(TOOL, other)),
    }
}

/// The session id `action` needs.
fn required_session(req: &SessionRequest, action: &str) -> Result<Uuid, Failure> {
    let raw = req.session_id.as_ref().ok_or_else(|| {
        check::missing(
            "session_id",
            "UUID string (36 chars with hyphens)",
            &format!("Action '{action}' requires session_id (the session UUID)"),
        )
    })?;
    check::session_id(raw)
}

/// `delete_message` (one turn, by `seq`) or `delete_messages` (all).
async fn delete_messages(memory: &Memory, req: &SessionRequest, action: &str) -> Outcome {
    let session_id = required_session(req, action)?;
    let failed = |e: crate::capabilities::MessageError| {
        let failure = Failure::from(e);
        match failure.kind {
            crate::tools::Kind::NotFound => failure.parameter("session_id"),
            _ => failure,
        }
    };
    let (deleted, message) = if action == "delete_message" {
        let seq = req.seq.ok_or_else(|| {
            Failure::invalid(
                "delete_message needs seq: the turn's position (the <seq> of its content id message:<seq>)",
            )
            .parameter("seq")
        })?;
        let deleted = memory
            .messages
            .delete(session_id, seq)
            .await
            .map_err(failed)?;
        let message = if deleted {
            format!("Deleted message:{seq} of session {session_id}")
        } else {
            format!("No message:{seq} in session {session_id} (no-op)")
        };
        (u64::from(deleted), message)
    } else {
        let deleted = memory
            .messages
            .delete_all(session_id)
            .await
            .map_err(failed)?;
        (
            deleted as u64,
            format!("Deleted {deleted} messages of session {session_id}"),
        )
    };
    let remaining = memory
        .messages
        .stats(session_id)
        .await
        .map_err(Failure::from)?
        .count;
    reply::with(
        message,
        &MessagesDeleted {
            session_id: session_id.to_string(),
            deleted,
            remaining,
        },
    )
}

#[cfg(test)]
mod tests;
