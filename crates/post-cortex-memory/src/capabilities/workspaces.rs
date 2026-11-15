// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Workspaces: create, get, list, update, archive, delete and membership
//! (ARCHITECTURE section 4). Every change is made in storage and in the
//! workspace registry in the same order under the workspace's lock: storage
//! first, so a failed write leaves nothing behind and is returned.

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use post_cortex_core::slug::workspace_slug;
use post_cortex_core::workspace::{SessionRole, Workspace};
use post_cortex_ports::StoredWorkspace;
use post_cortex_ports::{BlobStore, WorkspaceStore, WorkspaceUpdate};
use uuid::Uuid;

use crate::memory_system::{
    Change, ChangeLog, KeyedGuard, KeyedLocks, RegistryError, RestoredWorkspace, WorkspaceChange,
    WorkspaceChangeKind, WorkspaceGraphCache, WorkspaceManager,
};

/// Longest workspace name, in characters (after trimming).
pub const MAX_WORKSPACE_NAME_CHARS: usize = 200;
/// Longest workspace description, in characters.
pub const MAX_WORKSPACE_DESCRIPTION_CHARS: usize = 4000;

/// Why a workspace operation failed.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// No such workspace, or the session is not in it.
    #[error("{0}")]
    NotFound(String),
    /// The request is malformed (empty or too long name, nothing to change,
    /// ...).
    #[error("{0}")]
    InvalidArgument(String),
    /// Another workspace has the name.
    #[error("{0}")]
    Conflict(String),
    /// Storage failed; nothing changed in memory.
    #[error("Failed to persist workspace: {0}")]
    Storage(#[from] crate::memory_system::RuntimeError),
    /// Storing a membership change failed; nothing changed in memory.
    #[error("Failed to persist workspace membership: {0}")]
    MembershipStorage(crate::memory_system::RuntimeError),
}

/// The one mapping of a storage port error to this capability's error.
fn storage(e: post_cortex_ports::PortError) -> WorkspaceError {
    WorkspaceError::Storage(e.into())
}

impl From<RegistryError> for WorkspaceError {
    fn from(e: RegistryError) -> Self {
        match e {
            RegistryError::WorkspaceNotFound(_) | RegistryError::NotAMember { .. } => {
                WorkspaceError::NotFound(e.to_string())
            }
            RegistryError::NameTaken(_) => WorkspaceError::Conflict(e.to_string()),
        }
    }
}

/// The mapping of a storage port error of a membership change.
fn membership_storage(e: post_cortex_ports::PortError) -> WorkspaceError {
    WorkspaceError::MembershipStorage(e.into())
}

/// A new workspace.
#[derive(Debug, Clone)]
pub struct CreateWorkspace {
    /// Its name: trimmed, 1 to [`MAX_WORKSPACE_NAME_CHARS`] characters.
    pub name: String,
    /// Its description: at most [`MAX_WORKSPACE_DESCRIPTION_CHARS`].
    pub description: String,
}

/// A change to a workspace ([`Workspaces::update`]); fields left `None`
/// stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateWorkspace {
    /// Its new name: trimmed, 1 to [`MAX_WORKSPACE_NAME_CHARS`] characters,
    /// not the name of another workspace.
    pub name: Option<String>,
    /// Its new description: at most [`MAX_WORKSPACE_DESCRIPTION_CHARS`].
    pub description: Option<String>,
    /// Allow or forbid its accurate searches the remote reranker, which
    /// receives the query and the candidates' text.
    pub remote_rerank: Option<bool>,
}

/// The Workspaces capability service.
#[derive(Clone)]
pub struct Workspaces {
    store: Arc<dyn WorkspaceStore>,
    blobs: Option<Arc<dyn BlobStore>>,
    registry: Arc<WorkspaceManager>,
    locks: Arc<KeyedLocks>,
    graphs: Arc<WorkspaceGraphCache>,
    changes: ChangeLog,
}

impl Workspaces {
    pub(crate) fn new(
        store: Arc<dyn WorkspaceStore>,
        blobs: Option<Arc<dyn BlobStore>>,
        registry: Arc<WorkspaceManager>,
        locks: Arc<KeyedLocks>,
        graphs: Arc<WorkspaceGraphCache>,
        changes: ChangeLog,
    ) -> Self {
        Self {
            store,
            blobs,
            registry,
            locks,
            graphs,
            changes,
        }
    }

    /// Publish that `kind` happened to `workspace_id` (once storage holds it).
    fn publish(&self, workspace_id: Uuid, kind: WorkspaceChangeKind) {
        self.changes.publish(Change::Workspace(WorkspaceChange {
            workspace_id,
            kind,
            at: Utc::now(),
        }));
    }

    /// Create a workspace: the name and description are validated, the
    /// workspace is written to storage, then registered. This is the one
    /// create path: every transport, `pcx` and project resolution use it.
    /// Names are not unique (two projects may share a directory name); a
    /// rename to a taken name is refused.
    ///
    /// # Errors
    /// `InvalidArgument` for a bad name or description, `Storage` when the
    /// write failed (nothing is left behind).
    pub async fn create(&self, req: CreateWorkspace) -> Result<Arc<Workspace>, WorkspaceError> {
        let name = valid_name(&req.name)?;
        valid_description(&req.description)?;
        // One create at a time takes a slug: two of one name cannot both
        // find it free.
        let _slugs = self.locks.lock("workspace-slugs".to_string()).await;
        let id = Uuid::new_v4();
        let slug = workspace_slug(&name, id, |base| {
            self.registry
                .list_workspaces()
                .iter()
                .any(|ws| ws.slug == base)
        });
        // One creation time, written and then cached as written.
        let stored = StoredWorkspace {
            id,
            name,
            slug,
            description: req.description,
            sessions: Vec::new(),
            created_at: Utc::now().timestamp().max(0) as u64,
            projects: Vec::new(),
            archived_at: None,
            remote_rerank: false,
        };
        self.store
            .restore_workspace(&stored)
            .await
            .map_err(storage)?;
        hydrate(&self.registry, stored).map_err(storage)?;
        self.publish(id, WorkspaceChangeKind::Created);
        self.get(id)
    }

    /// The workspace `workspace_id`.
    ///
    /// # Errors
    /// `NotFound`.
    pub fn get(&self, workspace_id: Uuid) -> Result<Arc<Workspace>, WorkspaceError> {
        self.registry
            .get_workspace(&workspace_id)
            .ok_or_else(|| WorkspaceError::NotFound(format!("Workspace {workspace_id} not found")))
    }

    /// The workspace named `name`, if any.
    #[must_use]
    pub fn find_by_name(&self, name: &str) -> Option<Arc<Workspace>> {
        self.registry.get_workspace_by_name(name)
    }

    /// The workspaces, leaving archived ones out unless `include_archived`.
    #[must_use]
    pub fn list(&self, include_archived: bool) -> Vec<Arc<Workspace>> {
        self.registry.list_workspaces_filtered(include_archived)
    }

    /// How many workspaces there are, archived ones included.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.registry.total_workspaces()
    }

    /// Change the name, description and/or remote reranking of a
    /// workspace (see [`UpdateWorkspace`]). Returns the workspace as it is
    /// now.
    ///
    /// # Errors
    /// `InvalidArgument` (a bad name or description, or nothing to change),
    /// `NotFound`, `Conflict` or `Storage`.
    pub async fn update(
        &self,
        workspace_id: Uuid,
        change: UpdateWorkspace,
    ) -> Result<Arc<Workspace>, WorkspaceError> {
        let UpdateWorkspace {
            name,
            description,
            remote_rerank,
        } = change;
        let name = name.as_deref().map(valid_name).transpose()?;
        if let Some(d) = &description {
            valid_description(d)?;
        }
        if name.is_none() && description.is_none() && remote_rerank.is_none() {
            return Err(WorkspaceError::InvalidArgument(
                "nothing to update: give a name, a description or remote reranking".to_string(),
            ));
        }

        let _ws = self.lock_workspace(workspace_id).await;
        let before = self.get(workspace_id)?;
        // The registry takes the new name first: two renames to one name
        // cannot both pass, whichever workspaces they lock.
        let workspace = self.registry.update_workspace(
            &workspace_id,
            name.clone(),
            description.clone(),
            remote_rerank,
        )?;
        let update = WorkspaceUpdate {
            name,
            description,
            archived_at: None,
            remote_rerank,
        };
        if let Err(e) = self.store.update_workspace(workspace_id, &update).await {
            // Put the old values back. Under the workspace lock the
            // workspace exists and its old name was ours, so this succeeds.
            self.registry.update_workspace(
                &workspace_id,
                Some(before.name.clone()),
                Some(before.description.clone()),
                Some(before.remote_rerank()),
            )?;
            return Err(storage(e));
        }
        self.publish(workspace_id, WorkspaceChangeKind::Updated);
        Ok(workspace)
    }

    /// Archive (`true`) or unarchive (`false`) a workspace. Archived
    /// workspaces keep their sessions and data and are left out of default
    /// listings; archiving an archived workspace keeps its archive time.
    /// Returns the workspace and whether its state changed.
    ///
    /// # Errors
    /// `NotFound` or `Storage`.
    pub async fn set_archived(
        &self,
        workspace_id: Uuid,
        archived: bool,
    ) -> Result<(Arc<Workspace>, bool), WorkspaceError> {
        let _ws = self.lock_workspace(workspace_id).await;
        let workspace = self.get(workspace_id)?;
        let before = workspace.archived_at();
        if before.is_some() == archived {
            return Ok((workspace, false));
        }
        // Whole seconds: what storage keeps, so a restart reads the same.
        let archived_at = archived
            .then(|| DateTime::<Utc>::from_timestamp(Utc::now().timestamp(), 0))
            .flatten();
        let update = WorkspaceUpdate {
            archived_at: Some(archived_at.map(|t| t.timestamp().max(0) as u64)),
            ..Default::default()
        };
        self.store
            .update_workspace(workspace_id, &update)
            .await
            .map_err(storage)?;
        let (workspace, _) = self
            .registry
            .set_workspace_archived(&workspace_id, archived_at)?;
        let kind = if archived {
            WorkspaceChangeKind::Archived
        } else {
            WorkspaceChangeKind::Unarchived
        };
        self.publish(workspace_id, kind);
        Ok((workspace, true))
    }

    /// Delete a workspace's blobs, then the workspace from storage
    /// (memberships, card and radar dismissals with it), then from memory;
    /// its sessions stay. Returns whether it existed. Every transport
    /// deletes through here.
    ///
    /// # Errors
    /// `Storage`: the workspace stays.
    pub async fn delete(&self, workspace_id: Uuid) -> Result<bool, WorkspaceError> {
        let _ws = self.lock_workspace(workspace_id).await;
        if self.registry.get_workspace(&workspace_id).is_none() {
            return Ok(false);
        }
        // Its blobs first: if the blob store fails, the workspace stays and
        // a retry deletes the rest.
        if let Some(blobs) = &self.blobs {
            for blob in blobs.list_blobs(workspace_id).await.map_err(storage)? {
                blobs
                    .delete_blob(workspace_id, &blob.id)
                    .await
                    .map_err(storage)?;
            }
        }
        self.store
            .delete_workspace(workspace_id)
            .await
            .map_err(storage)?;
        self.graphs.invalidate(workspace_id);
        let existed = self.registry.delete_workspace(&workspace_id).is_some();
        if existed {
            self.publish(workspace_id, WorkspaceChangeKind::Deleted);
        }
        Ok(existed)
    }

    /// Add a session to a workspace with `role`, or change its role.
    ///
    /// # Errors
    /// `NotFound` or `MembershipStorage`.
    pub async fn add_session(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: SessionRole,
    ) -> Result<(), WorkspaceError> {
        let _member = self.lock_membership(workspace_id, session_id).await;
        self.get(workspace_id)?;
        // Storage first: a failed write leaves memory as it was.
        self.store
            .add_session_to_workspace(workspace_id, session_id, role)
            .await
            .map_err(membership_storage)?;
        self.registry
            .add_session_to_workspace(&workspace_id, session_id, role)?;
        self.graphs.invalidate(workspace_id);
        self.publish(workspace_id, WorkspaceChangeKind::SessionAdded(session_id));
        Ok(())
    }

    /// Remove a session from a workspace; returns the role it had.
    ///
    /// # Errors
    /// `NotFound` (no such workspace, or the session is not in it) or
    /// `MembershipStorage`.
    pub async fn remove_session(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<SessionRole, WorkspaceError> {
        let _member = self.lock_membership(workspace_id, session_id).await;
        let workspace = self.get(workspace_id)?;
        if workspace.get_session_role(&session_id).is_none() {
            return Err(RegistryError::NotAMember {
                session_id,
                workspace_id,
            }
            .into());
        }
        // Storage first: a failed write leaves memory as it was.
        self.store
            .remove_session_from_workspace(workspace_id, session_id)
            .await
            .map_err(membership_storage)?;
        let role = self
            .registry
            .remove_session_from_workspace(&workspace_id, &session_id)?
            .ok_or(RegistryError::NotAMember {
                session_id,
                workspace_id,
            })?;
        self.graphs.invalidate(workspace_id);
        self.publish(
            workspace_id,
            WorkspaceChangeKind::SessionRemoved(session_id),
        );
        Ok(role)
    }

    /// Keeps memory and storage of one workspace's fields changing in the
    /// same order (shared with project binding).
    pub(crate) async fn lock_workspace(&self, workspace_id: Uuid) -> KeyedGuard<'_> {
        self.locks.lock(format!("workspace:{workspace_id}")).await
    }

    /// Keeps memory and storage of one membership changing in the same order.
    async fn lock_membership(&self, workspace_id: Uuid, session_id: Uuid) -> KeyedGuard<'_> {
        self.locks
            .lock(format!("member:{workspace_id}:{session_id}"))
            .await
    }
}

fn valid_name(name: &str) -> Result<String, WorkspaceError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_WORKSPACE_NAME_CHARS {
        return Err(WorkspaceError::InvalidArgument(format!(
            "name must be 1 to {MAX_WORKSPACE_NAME_CHARS} characters"
        )));
    }
    Ok(name.to_string())
}

fn valid_description(description: &str) -> Result<(), WorkspaceError> {
    if description.chars().count() > MAX_WORKSPACE_DESCRIPTION_CHARS {
        return Err(WorkspaceError::InvalidArgument(format!(
            "description must be at most {MAX_WORKSPACE_DESCRIPTION_CHARS} characters"
        )));
    }
    Ok(())
}

/// Put a stored workspace in the registry, replacing the copy it has:
/// members, creation time, projects, archive state and remote reranking. Startup and import
/// hydrate through here.
pub fn hydrate(
    registry: &WorkspaceManager,
    stored: StoredWorkspace,
) -> Result<(), post_cortex_ports::PortError> {
    let archived_at = stored
        .archived_at
        .map(|secs| {
            i64::try_from(secs)
                .ok()
                .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
                .ok_or_else(|| {
                    post_cortex_ports::error::corrupt("workspace", stored.id, "archived_at")
                })
        })
        .transpose()?;
    registry.restore_workspace(RestoredWorkspace {
        id: stored.id,
        name: stored.name,
        slug: stored.slug,
        description: stored.description,
        sessions: stored.sessions,
        created_at: UNIX_EPOCH + Duration::from_secs(stored.created_at),
        projects: stored.projects,
        archived_at,
        remote_rerank: stored.remote_rerank,
    });
    Ok(())
}

#[cfg(test)]
mod tests;
