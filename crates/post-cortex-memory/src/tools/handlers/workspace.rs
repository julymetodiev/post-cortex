// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool `manage_workspace`: workspaces and their members, update and
//! archive, the project and user cards, radar dismissals.

use crate::Memory;
use crate::capabilities::{CreateWorkspace, UpdateWorkspace};
use post_cortex_core::core::dismissal::RadarItemKind;
use post_cortex_core::workspace::SessionRole;
use serde_json::Value;
use uuid::Uuid;

use crate::tools::args::ManageWorkspaceRequest;
use crate::tools::check;
use crate::tools::handlers::Outcome;
use crate::tools::parse::{FieldHint, arguments};
use crate::tools::reply;
use crate::tools::responses::{
    MemberAdded, MemberRemoved, RadarDismissals, RadarRestored, WorkspaceCreated, WorkspaceDeleted,
    WorkspaceDetail, WorkspaceList, WorkspaceListing, WorkspaceSummary,
};
use crate::tools::{Failure, wire};

mod cards;

const TOOL: &str = "manage_workspace";

const ACTION_HINT: &str = "Valid actions: create (with name/description), list (include_archived), get (workspace_id), delete (workspace_id), add_session (workspace_id, session_id, role), remove_session (workspace_id, session_id), get_card, update_card, update (workspace_id + name/description), archive / unarchive (workspace_id), dismiss_radar_item / restore_radar_item (workspace_id, kind, key), list_radar_dismissals (workspace_id)";

const HINTS: &[FieldHint] = &[FieldHint {
    field: "action",
    expected: "one of: create, list, get, delete, add_session, remove_session, get_card, update_card, update, archive, unarchive, dismiss_radar_item, restore_radar_item, list_radar_dismissals",
    hint: "Valid actions: create (workspace), list (all), get (workspace details), delete (workspace), add_session (to workspace), remove_session (from workspace), update (name/description), archive, unarchive, dismiss_radar_item / restore_radar_item (kind, key), list_radar_dismissals",
}];

pub async fn handle(
    memory: &Memory,
    params: Value,
    author: Option<String>,
    person: Option<uuid::Uuid>,
) -> Outcome {
    let req: ManageWorkspaceRequest = arguments(params, HINTS)?;
    let action = crate::tools::handlers::action(TOOL, &req.action, ACTION_HINT)?;
    match action.as_str() {
        "create" => {
            let hint = "For create action, provide 'name' and 'description' for the new workspace";
            let name = req
                .name
                .clone()
                .ok_or_else(|| check::missing("name", "workspace name string", hint))?;
            let description = req.description.clone().ok_or_else(|| {
                check::missing("description", "workspace description string", hint)
            })?;
            // Persisted first: a failed write is an error and leaves nothing.
            let ws = memory
                .workspaces
                .create(CreateWorkspace {
                    name,
                    description: description.clone(),
                })
                .await
                .map_err(Failure::from)?;
            reply::with(
                format!("Created workspace '{}' with ID: {}", ws.name, ws.id),
                &WorkspaceCreated {
                    workspace_id: ws.id.to_string(),
                    name: ws.name.clone(),
                    slug: ws.slug.clone(),
                    description,
                    session_count: 0,
                },
            )
        }
        "list" => {
            let workspaces: Vec<WorkspaceListing> = memory
                .workspaces
                .list(req.include_archived.unwrap_or(false))
                .iter()
                .map(|ws| WorkspaceListing::from(ws.as_ref()))
                .collect();
            reply::with(
                format!("Found {} workspaces", workspaces.len()),
                &WorkspaceList { workspaces },
            )
        }
        "get" => {
            let ws = workspace(
                &req,
                "For get action, provide 'workspace_id' of the workspace to retrieve",
            )?;
            let ws = memory.workspaces.get(ws).map_err(Failure::from)?;
            reply::with(
                format!("Workspace '{}' retrieved", ws.name),
                &WorkspaceDetail::from(ws.as_ref()),
            )
        }
        "delete" => {
            let id = workspace(
                &req,
                "For delete action, provide 'workspace_id' of the workspace to delete",
            )?;
            // Through the locked delete: storage first, then memory.
            let name = memory
                .workspaces
                .get(id)
                .map_err(Failure::from)?
                .name
                .clone();
            if !memory.workspaces.delete(id).await.map_err(|e| {
                let message = format!("Failed to delete workspace from storage: {e}");
                Failure::from(e).with_message(message)
            })? {
                return Err(Failure::new(
                    crate::tools::Kind::NotFound,
                    format!("Workspace {id} not found"),
                ));
            }
            reply::with(
                format!("Deleted workspace '{name}'"),
                &WorkspaceDeleted {
                    workspace_id: id.to_string(),
                    name,
                },
            )
        }
        "add_session" | "remove_session" => membership(memory, &req, &action).await,
        "get_card" | "update_card" => cards::handle(memory, &req, &action, author, person).await,
        "update" => {
            let ws = required_workspace(&req, "update")?;
            let ws = memory
                .workspaces
                .update(
                    ws,
                    UpdateWorkspace {
                        name: req.name.clone(),
                        description: req.description.clone(),
                        ..UpdateWorkspace::default()
                    },
                )
                .await
                .map_err(Failure::from)?;
            reply::with(
                format!("Updated workspace '{}'", ws.name),
                &WorkspaceSummary::new(&ws, None),
            )
        }
        "archive" | "unarchive" => {
            let archive = action == "archive";
            let ws = required_workspace(&req, &action)?;
            let (ws, changed) = memory
                .workspaces
                .set_archived(ws, archive)
                .await
                .map_err(Failure::from)?;
            let state = if archive { "archived" } else { "active" };
            let message = if changed {
                format!("Workspace '{}' is now {state}", ws.name)
            } else {
                format!("Workspace '{}' was already {state}", ws.name)
            };
            reply::with(message, &WorkspaceSummary::new(&ws, Some(changed)))
        }
        "dismiss_radar_item" | "restore_radar_item" => radar(memory, &req, &action, author).await,
        "list_radar_dismissals" => {
            let ws = required_workspace(&req, "list_radar_dismissals")?;
            let dismissals = memory
                .views
                .radar_dismissals(ws)
                .await
                .map_err(Failure::from)?;
            reply::with(
                format!("{} dismissed radar items", dismissals.len()),
                &RadarDismissals { dismissals },
            )
        }
        other => Err(crate::tools::handlers::unhandled(TOOL, other)),
    }
}

/// `add_session` or `remove_session`.
async fn membership(memory: &Memory, req: &ManageWorkspaceRequest, action: &str) -> Outcome {
    let hint = format!("For {action} action, provide 'workspace_id' and 'session_id'");
    let ws = workspace(req, &hint)?;
    let raw = req
        .session_id
        .as_deref()
        .ok_or_else(|| check::missing("session_id", "session UUID string", &hint))?;
    let session = check::session_id(raw)?;
    if action == "add_session" {
        let role = req.role.as_deref().unwrap_or("related");
        check::role(role)?;
        let role: SessionRole = wire::parse("role", role)?;
        memory
            .workspaces
            .add_session(ws, session, role)
            .await
            .map_err(|e| match e {
                crate::capabilities::WorkspaceError::NotFound(ref m) => {
                    let message = format!("Failed to add session to workspace: {m}");
                    Failure::from(e).with_message(message)
                }
                e => Failure::from(e),
            })?;
        return reply::with(
            "Added session to workspace successfully",
            &MemberAdded::new(ws, session, role),
        );
    }
    let role = memory
        .workspaces
        .remove_session(ws, session)
        .await
        .map_err(Failure::from)?;
    reply::with(
        "Removed session from workspace successfully",
        &MemberRemoved {
            workspace_id: ws.to_string(),
            session_id: session.to_string(),
            previous_role: role.stored_name(),
        },
    )
}

/// `dismiss_radar_item` or `restore_radar_item`.
async fn radar(
    memory: &Memory,
    req: &ManageWorkspaceRequest,
    action: &str,
    author: Option<String>,
) -> Outcome {
    let ws = required_workspace(req, action)?;
    let raw = req.kind.as_deref().ok_or_else(|| {
        Failure::invalid(format!(
            "{action} needs kind: one of {}",
            crate::tools::wire::names::<RadarItemKind>()
        ))
        .parameter("kind")
    })?;
    let kind: RadarItemKind = crate::tools::wire::parse("kind", raw)?;
    let key = req.key.clone().ok_or_else(|| {
        Failure::invalid(format!(
            "{action} needs key: the item as the radar lists it"
        ))
        .parameter("key")
    })?;
    if action == "dismiss_radar_item" {
        let author = req
            .author
            .clone()
            .filter(|a| !a.trim().is_empty())
            .or(author);
        let dismissal = memory
            .views
            .dismiss_radar_item(ws, kind, &key, author)
            .await
            .map_err(Failure::from)?;
        return reply::with(
            format!("Dismissed {} '{}'", kind.as_str(), dismissal.key),
            &dismissal,
        );
    }
    let existed = memory
        .views
        .restore_radar_item(ws, kind, &key)
        .await
        .map_err(Failure::from)?;
    let message = if existed {
        format!("Restored {} '{}'", kind.as_str(), key.trim())
    } else {
        format!(
            "{} '{}' was not dismissed (no-op)",
            kind.as_str(),
            key.trim()
        )
    };
    reply::with(message, &RadarRestored { existed })
}

/// The `workspace_id` of `action`, missing with `hint`.
fn workspace(req: &ManageWorkspaceRequest, hint: &str) -> Result<Uuid, Failure> {
    let raw = req
        .workspace_id
        .as_deref()
        .ok_or_else(|| check::missing("workspace_id", "workspace UUID string", hint))?;
    check::workspace_id(raw)
}

/// The `workspace_id` that `action` needs.
fn required_workspace(req: &ManageWorkspaceRequest, action: &str) -> Result<Uuid, Failure> {
    let raw = req.workspace_id.as_deref().ok_or_else(|| {
        Failure::invalid(format!("{action} needs workspace_id")).parameter("workspace_id")
    })?;
    check::workspace_id(raw)
}

#[cfg(test)]
mod tests;
