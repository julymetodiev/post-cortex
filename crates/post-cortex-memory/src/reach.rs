// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The reach of a call (`docs/connect.md` section 4a): the workspaces a
//! person limited to some may reach. The memory instance, the data's owner,
//! holds every call that carries one to it, whoever forwarded the call: a
//! call (classified by the operation table) that names a workspace outside
//! it, a session of no workspace in it, a project placed outside it, or
//! every workspace (but making a new session, which reaches nothing yet, the
//! instance's health, and a person's own user card), is refused.

use post_cortex_core::core::message::{MessageCursor, MessageOrder};
use uuid::Uuid;

use crate::Memory;
use crate::capabilities::ProjectPlacement;
use crate::tools::operations::classify::Classified;

/// Why a call is outside its reach.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReachRefusal {
    /// The reach names something that is no workspace id.
    #[error("the call's reach names {0}, no workspace id")]
    NotAWorkspace(String),
    /// The call reaches every workspace.
    #[error("{op} reaches every workspace; this call may reach only some")]
    Everything {
        /// The operation.
        op: &'static str,
    },
    /// The call names a workspace outside the reach.
    #[error("{op} reaches workspace {workspace}, outside this call's reach")]
    Workspace {
        /// The operation.
        op: &'static str,
        /// The workspace.
        workspace: Uuid,
    },
    /// The call names a session of no workspace in the reach.
    #[error("{op} reaches session {session}, of no workspace in this call's reach")]
    Session {
        /// The operation.
        op: &'static str,
        /// The session.
        session: Uuid,
    },
    /// The call's project is placed outside the reach, or would make a new
    /// workspace (`None`).
    #[error("{op}: project {key} is placed outside this call's reach")]
    Project {
        /// The operation.
        op: &'static str,
        /// The project key.
        key: String,
        /// Where it is placed; `None`: a new workspace.
        workspace: Option<Uuid>,
    },
}

/// The workspace ids of a reach as a scope carries it.
///
/// # Errors
/// [`ReachRefusal::NotAWorkspace`] for an id that is none.
pub fn workspaces(reach: &[String]) -> Result<Vec<Uuid>, ReachRefusal> {
    reach
        .iter()
        .map(|id| Uuid::parse_str(id.trim()).map_err(|_| ReachRefusal::NotAWorkspace(id.clone())))
        .collect()
}

impl Memory {
    /// Whether the call `call` stays within `reach`; `for_person`: the call
    /// is for a person (`Scope.person`), whose own user card is within any
    /// reach.
    ///
    /// # Errors
    /// The [`ReachRefusal`] of what it reaches outside it.
    pub async fn within_reach(
        &self,
        call: &Classified,
        reach: &[Uuid],
        for_person: bool,
    ) -> Result<(), ReachRefusal> {
        let op = call.operation.name;
        if for_person && call.user_card() {
            return Ok(());
        }
        if let Some(project) = &call.project {
            let placed = match self
                .projects
                .find(&project.key, project.workspace.as_deref())
            {
                ProjectPlacement::Bound { workspace_id, .. }
                | ProjectPlacement::Joins { workspace_id } => Some(workspace_id),
                ProjectPlacement::NewWorkspace => None,
            };
            return match placed {
                Some(w) if reach.contains(&w) => Ok(()),
                workspace => Err(ReachRefusal::Project {
                    op,
                    key: project.key.clone(),
                    workspace,
                }),
            };
        }
        // A new session reaches nothing yet (it joins a workspace only by a
        // call that names it), nor does the instance's health.
        if call.reaches_nothing() {
            return Ok(());
        }
        if call.instance_wide() {
            return Err(ReachRefusal::Everything { op });
        }
        if let Some(workspace) = call.workspaces.iter().find(|w| !reach.contains(w)) {
            return Err(ReachRefusal::Workspace {
                op,
                workspace: *workspace,
            });
        }
        if call.sessions.is_empty() {
            return Ok(());
        }
        let sessions = self.sessions_in_reach(reach);
        for session in call.sessions.iter().filter(|s| !sessions.contains(s)) {
            // Joining a workspace in the reach, a new session (in no
            // workspace, with nothing in it) is reached; another session
            // would bring what it holds into the reach.
            if call.joins_workspace() && self.is_new_session(*session).await {
                continue;
            }
            return Err(ReachRefusal::Session {
                op,
                session: *session,
            });
        }
        Ok(())
    }

    /// Whether `session` is in no workspace and holds no record and no
    /// message.
    async fn is_new_session(&self, session: Uuid) -> bool {
        let in_workspace = self
            .workspaces
            .list(true)
            .iter()
            .any(|w| w.get_session_role(&session).is_some());
        if in_workspace {
            return false;
        }
        let no_records = self
            .sessions
            .get(session)
            .await
            .is_ok_and(|s| s.records().is_empty());
        let no_messages = self
            .messages
            .page(session, MessageCursor::From(0), 1, MessageOrder::Asc)
            .await
            .is_ok_and(|page| page.is_empty());
        no_records && no_messages
    }

    /// The sessions of the workspaces of `reach`.
    #[must_use]
    pub fn sessions_in_reach(&self, reach: &[Uuid]) -> Vec<Uuid> {
        reach
            .iter()
            .filter_map(|w| self.workspaces.get(*w).ok())
            .flat_map(|w| w.get_all_sessions().into_iter().map(|(s, _)| s))
            .collect()
    }
}
