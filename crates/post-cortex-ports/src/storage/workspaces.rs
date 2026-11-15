// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The workspace port: workspaces, their members and projects.

use crate::error::Result;
use async_trait::async_trait;
use post_cortex_core::workspace::{SessionRole, WorkspaceProject};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A stored workspace with its members, as listings return it .
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredWorkspace {
    /// Workspace ID.
    pub id: Uuid,
    /// Workspace display name.
    pub name: String,
    /// The segment of its addresses: unique, kept after a rename.
    pub slug: String,
    /// Workspace description.
    pub description: String,
    /// Member sessions and their roles.
    pub sessions: Vec<(Uuid, SessionRole)>,
    /// Creation timestamp (Unix seconds).
    pub created_at: u64,
    /// Projects resolved to the workspace.
    pub projects: Vec<WorkspaceProject>,
    /// When the workspace was archived (Unix seconds); `None` while active.
    pub archived_at: Option<u64>,
    /// Whether its accurate searches may use the remote reranker.
    pub remote_rerank: bool,
}

/// A change to a stored workspace (see [`WorkspaceStore::update_workspace`]);
/// fields left `None` stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceUpdate {
    /// New name.
    pub name: Option<String>,
    /// New description.
    pub description: Option<String>,
    /// `Some(Some(t))` archives the workspace at `t` (Unix seconds),
    /// `Some(None)` unarchives it.
    pub archived_at: Option<Option<u64>>,
    /// Allow (`Some(true)`) or forbid (`Some(false)`) the remote reranker.
    pub remote_rerank: Option<bool>,
}

/// Stored workspaces with their members (sessions and roles) and projects.
#[async_trait]
pub trait WorkspaceStore: Send + Sync {
    /// Create a workspace (with `slug`, the remote reranker forbidden) or
    /// rename it. An existing workspace keeps its slug, creation time,
    /// projects, archive state, remote reranking and members;
    /// each of `session_ids` that is not a member yet becomes one as
    /// `Primary`.
    async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        slug: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> Result<()>;

    /// Replace the projects of a stored workspace; fails if there is no such
    /// workspace. `save_workspace_metadata` keeps them.
    async fn save_workspace_projects(
        &self,
        workspace_id: Uuid,
        projects: &[WorkspaceProject],
    ) -> Result<()>;

    /// Change the name, description, archive state or remote reranking of
    /// a stored workspace; the other fields of the row (members, projects, creation
    /// time) stay. Fails with a not-found error (see
    /// [`crate::PortError::NotFound`]) if there is no such workspace.
    async fn update_workspace(&self, workspace_id: Uuid, update: &WorkspaceUpdate) -> Result<()>;

    /// Write `workspace` as it is, its creation time, projects, archive
    /// state, remote reranking and members (with their roles) included, in place of whatever
    /// is stored under its id; members it lacks are removed. Its card and
    /// radar dismissals stay. What import (and `--overwrite`) does to a
    /// workspace.
    async fn restore_workspace(&self, workspace: &StoredWorkspace) -> Result<()>;

    /// Delete a workspace, its memberships, its card and its radar
    /// dismissals.
    async fn delete_workspace(&self, workspace_id: Uuid) -> Result<()>;

    /// Every stored workspace.
    async fn list_workspaces(&self) -> Result<Vec<StoredWorkspace>>;

    /// Make a session a member of a workspace with `role` (a member gets
    /// the new role); fails with a not-found error if there is no such
    /// workspace.
    async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: SessionRole,
    ) -> Result<()>;

    /// Remove a session from a workspace.
    async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<()>;
}
