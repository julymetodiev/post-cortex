// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Projects: the workspace and session of a project key, found or created
//! idempotently (ARCHITECTURE section 4, under Workspaces).
//!
//! The key lives on the workspace (`WorkspaceMetadata::projects`, persisted
//! with the workspace row) and is looked up through the registry's key
//! index. Resolution is serialised per key, so clients resolving the same
//! new key at once end with one workspace.

use std::sync::Arc;

use post_cortex_core::services::{ResolveProjectRequest, ResolveProjectResponse};
use post_cortex_core::workspace::{SessionRole, WorkspaceProject};
use tracing::info;
use uuid::Uuid;

use crate::capabilities::sessions::Sessions;
use crate::capabilities::workspaces::{CreateWorkspace, Workspaces};
use crate::memory_system::MemoryRuntime;

/// Longest project key, in characters.
const MAX_PROJECT_KEY_CHARS: usize = 512;
/// Longest project or workspace name, in characters.
const MAX_PROJECT_NAME_CHARS: usize = 200;

/// Why a project resolution failed.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    /// The key, name or workspace name is malformed.
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
    /// Creating or joining the project's workspace failed.
    #[error(transparent)]
    Workspace(#[from] crate::capabilities::workspaces::WorkspaceError),
    /// Creating the project's session failed.
    #[error(transparent)]
    Session(#[from] crate::capabilities::sessions::SessionError),
    /// Storing the binding failed.
    #[error("Internal error: {0}")]
    Storage(#[from] crate::memory_system::RuntimeError),
}

/// The one mapping of a storage port error to this capability's error.
fn storage(e: post_cortex_ports::PortError) -> ProjectError {
    ProjectError::Storage(e.into())
}

/// Where a project key resolves ([`Projects::find`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectPlacement {
    /// The key is bound to this workspace; `session_id` is its session
    /// (which may have been deleted since: a resolution then adds one).
    Bound {
        /// The key's workspace.
        workspace_id: Uuid,
        /// The key's session, as bound.
        session_id: Option<Uuid>,
    },
    /// The key is new and would join this existing workspace (the one the
    /// request names).
    Joins {
        /// The named workspace.
        workspace_id: Uuid,
    },
    /// The key is new and would get a new workspace.
    NewWorkspace,
}

/// The Projects capability service.
#[derive(Clone)]
pub struct Projects {
    rt: Arc<MemoryRuntime>,
    workspaces: Workspaces,
    sessions: Sessions,
}

impl Projects {
    pub(crate) fn new(rt: Arc<MemoryRuntime>, workspaces: Workspaces, sessions: Sessions) -> Self {
        Self {
            rt,
            workspaces,
            sessions,
        }
    }

    /// The workspace and session of project `key`, created on first use.
    ///
    /// A bound key returns its workspace and session (`created: false`); if
    /// that session was deleted, a new one named `name` takes its place. A
    /// new key gets a new session named `name` in the workspace named
    /// `workspace` (created if missing), else in a new workspace named
    /// `name` (`created: true`).
    ///
    /// # Errors
    /// `InvalidArgument`: the key must be 1 to 512 printable characters
    /// without whitespace, the trimmed name 1 to 200 characters, the
    /// trimmed workspace name at most 200. `Storage` otherwise.
    #[tracing::instrument(
        skip(self, req),
        fields(key = %req.key, client = req.client.as_deref().unwrap_or("")),
        name = "post_cortex.resolve_project",
    )]
    pub async fn resolve(
        &self,
        req: ResolveProjectRequest,
    ) -> Result<ResolveProjectResponse, ProjectError> {
        let (key, name, workspace) = validate(&req)?;
        let locks = &self.rt.project_locks;
        // Lock order: key, workspace name, workspace id.
        let _key = locks.lock(format!("key:{key}")).await;

        if let Some(ws) = self.rt.workspace_manager.get_workspace_by_project(key) {
            if let Some(project) = ws.project(key)
                && self
                    .rt
                    .session_manager
                    .get_session(project.session_id)
                    .await
                    .map_err(ProjectError::Storage)?
                    .is_some()
            {
                return Ok(ResolveProjectResponse {
                    workspace_id: ws.id,
                    session_id: project.session_id,
                    created: false,
                });
            }
            let session_id = self.add_session(ws.id, key, name).await?;
            info!(
                "Project {} lost its session; new session {} in workspace {}",
                key, session_id, ws.id
            );
            return Ok(ResolveProjectResponse {
                workspace_id: ws.id,
                session_id,
                created: false,
            });
        }

        let workspace_id = match workspace {
            Some(ws_name) => {
                let _name = locks.lock(format!("name:{ws_name}")).await;
                match self.workspaces.find_by_name(ws_name) {
                    Some(ws) => ws.id,
                    None => self.create_workspace(ws_name, key).await?,
                }
            }
            None => self.create_workspace(name, key).await?,
        };
        let session_id = self.add_session(workspace_id, key, name).await?;
        info!(
            "Project {} bound to workspace {} with session {}",
            key, workspace_id, session_id
        );
        Ok(ResolveProjectResponse {
            workspace_id,
            session_id,
            created: true,
        })
    }

    /// Where [`Self::resolve`] would put project `key`, found without
    /// creating or changing anything: the workspace the key is bound to,
    /// else the existing workspace named `workspace` (trimmed, blank counts
    /// as absent), else none (a new workspace would be made). Gateways use
    /// it to authorise a resolution before it runs.
    #[must_use]
    pub fn find(&self, key: &str, workspace: Option<&str>) -> ProjectPlacement {
        if let Some(ws) = self.rt.workspace_manager.get_workspace_by_project(key) {
            return ProjectPlacement::Bound {
                workspace_id: ws.id,
                session_id: ws.project(key).map(|p| p.session_id),
            };
        }
        match workspace
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .and_then(|name| self.workspaces.find_by_name(name))
        {
            Some(ws) => ProjectPlacement::Joins {
                workspace_id: ws.id,
            },
            None => ProjectPlacement::NewWorkspace,
        }
    }

    /// Create and persist the workspace of a new project.
    async fn create_workspace(&self, name: &str, key: &str) -> Result<Uuid, ProjectError> {
        self.workspaces
            .create(CreateWorkspace {
                name: name.to_string(),
                description: format!("Project {key}"),
            })
            .await
            .map(|ws| ws.id)
            .map_err(ProjectError::from)
    }

    /// Create the session of project `key` in a workspace and bind the key
    /// to it.
    async fn add_session(
        &self,
        workspace_id: Uuid,
        key: &str,
        name: &str,
    ) -> Result<Uuid, ProjectError> {
        let session_id = self
            .sessions
            .create(Some(name.to_string()), Some(format!("Project {key}")))
            .await?;
        self.workspaces
            .add_session(workspace_id, session_id, SessionRole::Primary)
            .await?;

        // Other keys may be bound to this workspace meanwhile: store the list
        // as it stands with ours in it, then bind it in memory.
        let _workspace = self.workspaces.lock_workspace(workspace_id).await;
        let project = WorkspaceProject {
            key: key.to_string(),
            session_id,
        };
        let mut bound = self.workspaces.get(workspace_id)?.as_ref().clone();
        bound.set_project(project.clone());
        self.rt
            .stores
            .workspaces
            .save_workspace_projects(workspace_id, &bound.projects())
            .await
            .map_err(storage)?;
        self.rt
            .workspace_manager
            .set_workspace_project(&workspace_id, project)
            .map_err(crate::capabilities::workspaces::WorkspaceError::from)?;
        Ok(session_id)
    }
}

/// The key (1 to 512 printable characters, no whitespace), the trimmed name
/// and the trimmed workspace name (absent when empty) of `req`.
fn validate(req: &ResolveProjectRequest) -> Result<(&str, &str, Option<&str>), ProjectError> {
    let invalid = |msg: String| ProjectError::InvalidArgument(msg);
    let key = req.key.as_str();
    if key.is_empty() || key.chars().count() > MAX_PROJECT_KEY_CHARS {
        return Err(invalid(format!(
            "key must be 1 to {MAX_PROJECT_KEY_CHARS} characters"
        )));
    }
    if key.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid("key must be printable, without whitespace".into()));
    }
    let bounded = |field: &str, value: &str| -> Result<(), ProjectError> {
        if value.chars().count() > MAX_PROJECT_NAME_CHARS {
            return Err(invalid(format!(
                "{field} must be at most {MAX_PROJECT_NAME_CHARS} characters"
            )));
        }
        Ok(())
    };
    let display = req.name.trim();
    if display.is_empty() {
        return Err(invalid("name must not be empty".into()));
    }
    bounded("name", display)?;
    let workspace = req
        .workspace
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty());
    if let Some(w) = workspace {
        bounded("workspace", w)?;
    }
    Ok((key, display, workspace))
}

#[cfg(test)]
mod tests;
