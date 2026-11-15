// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Workspace, membership and radar dismissal responses.

use post_cortex_core::core::dismissal::RadarDismissal;
use post_cortex_core::workspace::{SessionRole, Workspace};
use serde::Serialize;

/// The creation time of a workspace.
fn created_at(ws: &Workspace) -> String {
    chrono::DateTime::<chrono::Utc>::from(ws.created_at).to_rfc3339()
}

fn archived_at(ws: &Workspace) -> Option<String> {
    ws.archived_at().map(|t| t.to_rfc3339())
}

/// A workspace created.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceCreated {
    /// Its id.
    pub workspace_id: String,
    /// Its name (trimmed).
    pub name: String,
    /// The segment of its addresses (kept after a rename).
    pub slug: String,
    /// Its description.
    pub description: String,
    /// Always 0.
    pub session_count: usize,
}

/// The workspaces.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceList {
    /// One per workspace.
    pub workspaces: Vec<WorkspaceListing>,
}

/// A workspace of a [`WorkspaceList`].
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceListing {
    /// Its id.
    pub workspace_id: String,
    /// Its name.
    pub name: String,
    /// The segment of its addresses (kept after a rename).
    pub slug: String,
    /// Its description.
    pub description: String,
    /// Its member sessions.
    pub session_count: usize,
    /// Creation time.
    pub created_at: String,
    /// When it was archived, if it is.
    pub archived_at: Option<String>,
}

impl From<&Workspace> for WorkspaceListing {
    fn from(ws: &Workspace) -> Self {
        Self {
            workspace_id: ws.id.to_string(),
            name: ws.name.clone(),
            slug: ws.slug.clone(),
            description: ws.description.clone(),
            session_count: ws.session_count(),
            created_at: created_at(ws),
            archived_at: archived_at(ws),
        }
    }
}

/// A workspace with its members.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceDetail {
    /// Its id.
    pub workspace_id: String,
    /// Its name.
    pub name: String,
    /// The segment of its addresses (kept after a rename).
    pub slug: String,
    /// Its description.
    pub description: String,
    /// Creation time.
    pub created_at: String,
    /// Its member sessions with their roles.
    pub sessions: Vec<WorkspaceMember>,
    /// How many.
    pub session_count: usize,
    /// When it was archived, if it is.
    pub archived_at: Option<String>,
}

impl From<&Workspace> for WorkspaceDetail {
    fn from(ws: &Workspace) -> Self {
        Self {
            workspace_id: ws.id.to_string(),
            name: ws.name.clone(),
            slug: ws.slug.clone(),
            description: ws.description.clone(),
            created_at: created_at(ws),
            sessions: ws
                .get_all_sessions()
                .into_iter()
                .map(|(id, role)| WorkspaceMember {
                    session_id: id.to_string(),
                    role: role.stored_name(),
                })
                .collect(),
            session_count: ws.session_count(),
            archived_at: archived_at(ws),
        }
    }
}

/// A member session.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceMember {
    /// The session.
    pub session_id: String,
    /// Its role (stored name: `Primary`, `Related`, ...).
    pub role: &'static str,
}

/// A workspace deleted.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceDeleted {
    /// Its id.
    pub workspace_id: String,
    /// Its name.
    pub name: String,
}

/// A session added to a workspace.
#[derive(Debug, Clone, Serialize)]
pub struct MemberAdded {
    /// The workspace.
    pub workspace_id: String,
    /// The session.
    pub session_id: String,
    /// Its role (stored name).
    pub role: &'static str,
}

impl MemberAdded {
    /// `session_id` added to `workspace_id` as `role`.
    #[must_use]
    pub fn new(workspace_id: uuid::Uuid, session_id: uuid::Uuid, role: SessionRole) -> Self {
        Self {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
            role: role.stored_name(),
        }
    }
}

/// A session removed from a workspace.
#[derive(Debug, Clone, Serialize)]
pub struct MemberRemoved {
    /// The workspace.
    pub workspace_id: String,
    /// The session.
    pub session_id: String,
    /// The role it had (stored name).
    pub previous_role: &'static str,
}

/// A workspace after an update or an archive change.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceSummary {
    /// Its id.
    pub workspace_id: String,
    /// Its name.
    pub name: String,
    /// Its description.
    pub description: String,
    /// Its member sessions.
    pub session_count: usize,
    /// When it was archived, if it is.
    pub archived_at: Option<String>,
    /// The archive state changed (archive and unarchive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed: Option<bool>,
}

impl WorkspaceSummary {
    /// The summary of `ws`; `changed` for an archive change.
    #[must_use]
    pub fn new(ws: &Workspace, changed: Option<bool>) -> Self {
        Self {
            workspace_id: ws.id.to_string(),
            name: ws.name.clone(),
            description: ws.description.clone(),
            session_count: ws.session_count(),
            archived_at: archived_at(ws),
            changed,
        }
    }
}

/// A radar item restored.
#[derive(Debug, Clone, Serialize)]
pub struct RadarRestored {
    /// It was dismissed.
    pub existed: bool,
}

/// The radar dismissals of a workspace.
#[derive(Debug, Clone, Serialize)]
pub struct RadarDismissals {
    /// By kind and key.
    pub dismissals: Vec<RadarDismissal>,
}
