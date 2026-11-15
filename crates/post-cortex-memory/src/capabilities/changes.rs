// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! ChangeLog: the changes of a memory instance as storage holds them
//! (ARCHITECTURE section 4); see [`crate::memory_system::change_log`].

use std::sync::Arc;

use pcx_module_api::MemoryWrite;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tracing::warn;
use uuid::Uuid;

use crate::RecordEventKind;
use crate::memory_system::{Change, ChangeLog, WorkspaceManager};

/// The ChangeLog capability service.
#[derive(Clone)]
pub struct Changes {
    log: ChangeLog,
    workspaces: Arc<WorkspaceManager>,
}

impl Changes {
    /// The capability over the instance's change `log` and its workspaces.
    pub fn new(log: ChangeLog, workspaces: Arc<WorkspaceManager>) -> Self {
        Self { log, workspaces }
    }

    /// The changes modules follow, from now on, as memory-write events
    /// (ARCHITECTURE section 7, modules together).
    #[must_use]
    pub fn memory_writes(&self) -> MemoryWrites {
        MemoryWrites {
            changes: self.log.subscribe(),
            workspaces: Arc::clone(&self.workspaces),
        }
    }

    /// A receiver of every change stored from now on: records written,
    /// forgotten or removed (once the storage write carrying them has
    /// finished; writes that do not wait for durability persist in the
    /// background, so a change can follow the call that made it), turns
    /// appended or deleted, card versions, workspace changes. Changes of one
    /// session arrive in write order; changes made before the call are not
    /// replayed.
    ///
    /// The feed holds a fixed number of changes per receiver and never
    /// blocks writers: a receiver that falls further behind gets
    /// `RecvError::Lagged(n)`, its `n` oldest changes are gone, and it should
    /// re-read what it watches before receiving again.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.log.subscribe()
    }
}

#[cfg(test)]
mod tests;

/// A memory write and the reach of the call that made it
/// (docs/connect.md 4a): the modules that follow it act within that reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The write.
    pub write: MemoryWrite,
    /// The reach of the call that made it; none: every workspace.
    pub reach: Option<Vec<Uuid>>,
}

/// The memory writes of the changes stored from now on.
pub struct MemoryWrites {
    changes: broadcast::Receiver<Change>,
    workspaces: Arc<WorkspaceManager>,
}

impl MemoryWrites {
    /// The memory writes of the next change modules follow: a document
    /// version or deletion; a record change once per workspace its session
    /// belongs to (once without a workspace when it belongs to none);
    /// [`MemoryWrite::FellBehind`] once when changes were lost. `None` once
    /// the memory instance is gone.
    pub async fn next(&mut self) -> Option<Vec<Written>> {
        loop {
            let change = match self.changes.recv().await {
                Ok(change) => change,
                Err(RecvError::Lagged(lost)) => {
                    warn!(lost, "memory-write feed fell behind");
                    return Some(vec![Written {
                        write: MemoryWrite::FellBehind,
                        reach: None,
                    }]);
                }
                Err(RecvError::Closed) => return None,
            };
            let writes = self.writes_of(&change);
            if !writes.is_empty() {
                return Some(writes);
            }
        }
    }

    fn writes_of(&self, change: &Change) -> Vec<Written> {
        match change {
            Change::Document(d) => vec![Written {
                write: MemoryWrite::Document {
                    workspace_id: d.workspace_id.to_string(),
                    id: d.id.to_string(),
                    kind: d.kind.to_string(),
                    version: d.version.unwrap_or(0),
                    deleted: d.version.is_none(),
                },
                reach: d.reach.clone(),
            }],
            Change::Record(r) => {
                let mut workspaces: Vec<String> = self
                    .workspaces
                    .list_workspaces()
                    .iter()
                    .filter(|w| w.get_session_role(&r.session_id).is_some())
                    .map(|w| w.id.to_string())
                    .collect();
                if workspaces.is_empty() {
                    workspaces.push(String::new());
                }
                workspaces
                    .into_iter()
                    // A record carries no reach: no module wakes a member
                    // on one.
                    .map(|workspace_id| Written {
                        write: MemoryWrite::Record {
                            workspace_id,
                            id: r.entry_id.to_string(),
                            record_type: r.update_type.map(|t| t.as_str().to_string()),
                            deleted: r.kind == RecordEventKind::Removed,
                        },
                        reach: None,
                    })
                    .collect()
            }
            Change::Messages(_) | Change::Card(_) | Change::Workspace(_) => Vec::new(),
        }
    }
}
