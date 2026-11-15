// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Workspaces: groups of related sessions (e.g. the services of one
//! product) as plain values. The registry that holds them is runtime state
//! of the memory crate.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;
use uuid::Uuid;

#[cfg(test)]
mod tests;

/// Role of a session within a workspace
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionRole {
    /// Primary/main service in the workspace
    Primary,
    /// Related peer service (e.g., other microservices)
    Related,
    /// External dependency documentation
    Dependency,
    /// Shared library used across services
    Shared,
}

crate::core::wire::wire_names!(SessionRole, "session role", {
    Primary => "primary",
    Related => "related",
    Dependency => "dependency",
    Shared => "shared",
});

/// A project resolved to a workspace: its key and the session made for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceProject {
    /// Project key computed by the client: a normalised git remote such as
    /// `github.com/owner/repo`, or `local:<hash>` for a checkout without one.
    pub key: String,
    /// Session of the project in the workspace.
    pub session_id: Uuid,
}

/// Metadata for a workspace
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct WorkspaceMetadata {
    /// Type of project (e.g., "microservices", "monorepo")
    pub project_type: Option<String>,
    /// Tags for categorization
    pub tags: Vec<String>,
    /// Physical paths to project roots
    pub root_paths: Vec<PathBuf>,
    /// Additional user-defined metadata
    pub custom_data: std::collections::HashMap<String, String>,
    /// Projects resolved to this workspace. Unlike the fields above it is
    /// persisted with the workspace row.
    #[serde(default)]
    pub projects: Vec<WorkspaceProject>,
    /// When the workspace was archived; `None` while it is active. Archived
    /// workspaces keep their sessions and data but are left out of default
    /// listings. Persisted with the workspace row.
    #[serde(default)]
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Whether its accurate searches may use the remote reranker (Voyage),
    /// which receives the query and the candidates' text. Off until a
    /// person turns it on (docs/memory.md, search). Persisted with the
    /// workspace row.
    #[serde(default)]
    pub remote_rerank: bool,
}

/// A workspace: its identity, its member sessions with their roles and its
/// metadata. A value: changing a copy changes only that copy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// Unique workspace identifier
    pub id: Uuid,
    /// Human-readable workspace name
    pub name: String,
    /// The segment of its addresses ([`crate::slug::workspace_slug`]):
    /// made when it is created, unique, kept after a rename.
    pub slug: String,
    /// Workspace description
    pub description: String,
    /// When the workspace was created
    pub created_at: SystemTime,
    #[serde(default)]
    sessions: BTreeMap<Uuid, SessionRole>,
    #[serde(default)]
    metadata: WorkspaceMetadata,
}

impl Workspace {
    /// A workspace without sessions, created at `created_at`.
    pub fn new(
        id: Uuid,
        name: String,
        slug: String,
        description: String,
        created_at: SystemTime,
    ) -> Self {
        Self {
            id,
            name,
            slug,
            description,
            created_at,
            sessions: BTreeMap::new(),
            metadata: WorkspaceMetadata::default(),
        }
    }

    /// Add `session_id` with `role`, or change its role; returns the role
    /// it had.
    pub fn add_session(&mut self, session_id: Uuid, role: SessionRole) -> Option<SessionRole> {
        self.sessions.insert(session_id, role)
    }

    /// Remove `session_id`; returns the role it had.
    pub fn remove_session(&mut self, session_id: &Uuid) -> Option<SessionRole> {
        self.sessions.remove(session_id)
    }

    /// Role of a member session.
    pub fn get_session_role(&self, session_id: &Uuid) -> Option<SessionRole> {
        self.sessions.get(session_id).copied()
    }

    /// Every member session with its role, by session id.
    pub fn get_all_sessions(&self) -> Vec<(Uuid, SessionRole)> {
        self.sessions
            .iter()
            .map(|(id, role)| (*id, *role))
            .collect()
    }

    /// How many sessions belong to the workspace.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// The workspace's metadata.
    pub fn metadata(&self) -> &WorkspaceMetadata {
        &self.metadata
    }

    /// The project bound to `key`, if any.
    pub fn project(&self, key: &str) -> Option<WorkspaceProject> {
        self.metadata
            .projects
            .iter()
            .find(|p| p.key == key)
            .cloned()
    }

    /// All projects bound to this workspace.
    pub fn projects(&self) -> Vec<WorkspaceProject> {
        self.metadata.projects.clone()
    }

    /// Bind `project.key` to this workspace, replacing the session the key
    /// had here before.
    pub fn set_project(&mut self, project: WorkspaceProject) {
        self.metadata.projects.retain(|p| p.key != project.key);
        self.metadata.projects.push(project);
    }

    /// When the workspace was archived, if it is.
    pub fn archived_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.archived_at
    }

    /// Whether the workspace is archived.
    pub fn is_archived(&self) -> bool {
        self.archived_at().is_some()
    }

    /// Archive (`Some(time)`) or unarchive (`None`); returns when it was
    /// archived before.
    pub fn set_archived(
        &mut self,
        archived_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        std::mem::replace(&mut self.metadata.archived_at, archived_at)
    }

    /// Whether its accurate searches may use the remote reranker.
    pub fn remote_rerank(&self) -> bool {
        self.metadata.remote_rerank
    }

    /// Allow or forbid the remote reranker; returns what it was.
    pub fn set_remote_rerank(&mut self, allowed: bool) -> bool {
        std::mem::replace(&mut self.metadata.remote_rerank, allowed)
    }
}
