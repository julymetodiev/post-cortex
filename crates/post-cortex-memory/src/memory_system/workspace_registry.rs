// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The workspaces of a memory system, in memory: one map of workspace
//! values with name and project indexes. Readers get a snapshot
//! (`Arc<Workspace>`) that later changes do not touch; a change copies the
//! workspace and replaces it under the map entry's lock, so concurrent
//! changes of one workspace apply one after the other.

use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use uuid::Uuid;

use post_cortex_core::workspace::{SessionRole, Workspace, WorkspaceProject};

/// Why a change of the registry was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// No workspace with this id.
    #[error("workspace {0} not found")]
    WorkspaceNotFound(Uuid),
    /// Another workspace has this name.
    #[error("a workspace named '{0}' exists")]
    NameTaken(String),
    /// The session is not a member of the workspace.
    #[error("session {session_id} is not in workspace {workspace_id}")]
    NotAMember {
        /// The session.
        session_id: Uuid,
        /// The workspace.
        workspace_id: Uuid,
    },
}

/// A workspace as storage holds it, for [`WorkspaceManager::restore_workspace`].
pub struct RestoredWorkspace {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub sessions: Vec<(Uuid, SessionRole)>,
    pub created_at: SystemTime,
    pub projects: Vec<WorkspaceProject>,
    pub archived_at: Option<DateTime<Utc>>,
    pub remote_rerank: bool,
}

/// The workspace registry (see the module docs).
#[derive(Default)]
pub struct WorkspaceManager {
    workspaces: DashMap<Uuid, Arc<Workspace>>,
    /// Name -> workspace id.
    name_index: DashMap<String, Uuid>,
    /// Project key -> workspace id.
    project_index: DashMap<String, Uuid>,
}

impl WorkspaceManager {
    /// Put a workspace in, replacing any copy it has: a new one, or a
    /// stored one with its members, creation time, projects and archive
    /// state. Only the Workspaces capability adds workspaces (after storage
    /// has them).
    pub fn restore_workspace(&self, restored: RestoredWorkspace) {
        let RestoredWorkspace {
            id,
            name,
            slug,
            description,
            sessions,
            created_at,
            projects,
            archived_at,
            remote_rerank,
        } = restored;
        self.delete_workspace(&id);
        let mut workspace = Workspace::new(id, name, slug, description, created_at);
        for (session_id, role) in sessions {
            workspace.add_session(session_id, role);
        }
        for project in projects {
            workspace.set_project(project);
        }
        workspace.set_archived(archived_at);
        workspace.set_remote_rerank(remote_rerank);
        self.insert(workspace);
    }

    fn insert(&self, workspace: Workspace) {
        self.name_index.insert(workspace.name.clone(), workspace.id);
        for project in workspace.projects() {
            self.project_index.insert(project.key, workspace.id);
        }
        self.workspaces.insert(workspace.id, Arc::new(workspace));
    }

    /// Change the workspace `id` with `f` (on a copy that replaces it).
    fn change<T>(
        &self,
        id: &Uuid,
        f: impl FnOnce(&mut Workspace) -> T,
    ) -> Result<(Arc<Workspace>, T), RegistryError> {
        let mut entry = self
            .workspaces
            .get_mut(id)
            .ok_or(RegistryError::WorkspaceNotFound(*id))?;
        let out = f(Arc::make_mut(entry.value_mut()));
        Ok((Arc::clone(entry.value()), out))
    }

    /// The workspace `id`.
    pub(crate) fn get_workspace(&self, id: &Uuid) -> Option<Arc<Workspace>> {
        self.workspaces
            .get(id)
            .map(|entry| Arc::clone(entry.value()))
    }

    /// The workspace named `name`.
    pub(crate) fn get_workspace_by_name(&self, name: &str) -> Option<Arc<Workspace>> {
        let id = *self.name_index.get(name)?.value();
        self.get_workspace(&id)
    }

    /// The workspace the project `key` is bound to.
    pub(crate) fn get_workspace_by_project(&self, key: &str) -> Option<Arc<Workspace>> {
        let id = *self.project_index.get(key)?.value();
        self.get_workspace(&id)
    }

    /// Bind a project key to a workspace, replacing the session the key had
    /// there before.
    pub(crate) fn set_workspace_project(
        &self,
        workspace_id: &Uuid,
        project: WorkspaceProject,
    ) -> Result<(), RegistryError> {
        let key = project.key.clone();
        self.change(workspace_id, |ws| ws.set_project(project))?;
        self.project_index.insert(key, *workspace_id);
        Ok(())
    }

    /// Every workspace, archived ones included.
    pub(crate) fn list_workspaces(&self) -> Vec<Arc<Workspace>> {
        self.workspaces
            .iter()
            .map(|entry| Arc::clone(entry.value()))
            .collect()
    }

    /// The workspaces, leaving archived ones out unless `include_archived`.
    pub(crate) fn list_workspaces_filtered(&self, include_archived: bool) -> Vec<Arc<Workspace>> {
        let mut workspaces = self.list_workspaces();
        if !include_archived {
            workspaces.retain(|ws| !ws.is_archived());
        }
        workspaces
    }

    /// Change the name, description and/or remote reranking of a
    /// workspace. Fails if there is no such workspace or another workspace
    /// has the new name. Returns the workspace as it is now.
    pub(crate) fn update_workspace(
        &self,
        id: &Uuid,
        name: Option<String>,
        description: Option<String>,
        remote_rerank: Option<bool>,
    ) -> Result<Arc<Workspace>, RegistryError> {
        let current = self
            .get_workspace(id)
            .ok_or(RegistryError::WorkspaceNotFound(*id))?;
        if let Some(name) = &name
            && *name != current.name
        {
            match self.name_index.entry(name.clone()) {
                dashmap::mapref::entry::Entry::Occupied(taken) if taken.get() != id => {
                    return Err(RegistryError::NameTaken(name.clone()));
                }
                dashmap::mapref::entry::Entry::Occupied(_) => {}
                dashmap::mapref::entry::Entry::Vacant(free) => {
                    free.insert(*id);
                }
            }
            self.name_index.remove_if(&current.name, |_, ws| ws == id);
        }
        let (workspace, ()) = self.change(id, |ws| {
            if let Some(name) = name {
                ws.name = name;
            }
            if let Some(description) = description {
                ws.description = description;
            }
            if let Some(allowed) = remote_rerank {
                ws.set_remote_rerank(allowed);
            }
        })?;
        Ok(workspace)
    }

    /// Archive (`Some(time)`) or unarchive (`None`) a workspace. Returns
    /// the workspace and when it was archived before.
    pub(crate) fn set_workspace_archived(
        &self,
        id: &Uuid,
        archived_at: Option<DateTime<Utc>>,
    ) -> Result<(Arc<Workspace>, Option<DateTime<Utc>>), RegistryError> {
        self.change(id, |ws| ws.set_archived(archived_at))
    }

    /// Remove a workspace; returns it.
    pub(crate) fn delete_workspace(&self, id: &Uuid) -> Option<Arc<Workspace>> {
        let (_, workspace) = self.workspaces.remove(id)?;
        self.name_index.remove_if(&workspace.name, |_, ws| ws == id);
        for project in workspace.projects() {
            self.project_index.remove_if(&project.key, |_, ws| ws == id);
        }
        Some(workspace)
    }

    /// Add a session to a workspace (or change its role); returns the role
    /// it had.
    pub(crate) fn add_session_to_workspace(
        &self,
        workspace_id: &Uuid,
        session_id: Uuid,
        role: SessionRole,
    ) -> Result<Option<SessionRole>, RegistryError> {
        self.change(workspace_id, |ws| ws.add_session(session_id, role))
            .map(|(_, previous)| previous)
    }

    /// Remove a session from a workspace; returns the role it had. Fails if
    /// it was not a member.
    pub(crate) fn remove_session_from_workspace(
        &self,
        workspace_id: &Uuid,
        session_id: &Uuid,
    ) -> Result<Option<SessionRole>, RegistryError> {
        self.change(workspace_id, |ws| ws.remove_session(session_id))?
            .1
            .ok_or(RegistryError::NotAMember {
                session_id: *session_id,
                workspace_id: *workspace_id,
            })
            .map(Some)
    }

    /// How many workspaces there are.
    pub(crate) fn total_workspaces(&self) -> u64 {
        self.workspaces.len() as u64
    }
}
