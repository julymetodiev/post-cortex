// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An archived workspace is read-only (`docs/memory.md`, Workspaces): a call
//! that changes memory (the operation table's write and delete effects) and
//! names an archived workspace, a session of one, or a project placed in
//! one, is refused. Restoring the workspace and deleting it are the two
//! changes it takes. The memory instance, the data's owner, holds every
//! call to it, as it holds a call to its reach (`crate::reach`).

use uuid::Uuid;

use crate::Memory;
use crate::capabilities::ProjectPlacement;
use crate::tools::operations::classify::Classified;

/// A call refused for an archived workspace.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{op}: workspace {workspace} is archived and read-only; restore it to change it")]
pub struct Archived {
    /// The operation.
    pub op: &'static str,
    /// The archived workspace.
    pub workspace: Uuid,
}

/// The changes an archived workspace takes: restoring it and deleting it.
const TAKEN: [&str; 2] = ["workspace.archive", "workspace.delete"];

impl Memory {
    /// Whether the workspaces the call `call` changes are open to it.
    ///
    /// # Errors
    /// [`Archived`] for the first archived workspace it would change.
    pub fn open_to(&self, call: &Classified) -> Result<(), Archived> {
        let op = call.operation.name;
        if !call.changes() || TAKEN.contains(&op) {
            return Ok(());
        }
        let archived = |workspace: &Uuid| {
            self.workspaces
                .get(*workspace)
                .is_ok_and(|w| w.is_archived())
        };
        let placed = call.project.as_ref().and_then(|project| {
            match self
                .projects
                .find(&project.key, project.workspace.as_deref())
            {
                ProjectPlacement::Bound { workspace_id, .. }
                | ProjectPlacement::Joins { workspace_id } => Some(workspace_id),
                ProjectPlacement::NewWorkspace => None,
            }
        });
        if let Some(workspace) = call.workspaces.iter().chain(&placed).find(|w| archived(w)) {
            return Err(Archived {
                op,
                workspace: *workspace,
            });
        }
        if call.sessions.is_empty() {
            return Ok(());
        }
        let of_session = self
            .workspaces
            .list(true)
            .into_iter()
            .filter(|w| w.is_archived())
            .find(|w| {
                call.sessions
                    .iter()
                    .any(|s| w.get_session_role(s).is_some())
            });
        match of_session {
            Some(workspace) => Err(Archived {
                op,
                workspace: workspace.id,
            }),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests;
