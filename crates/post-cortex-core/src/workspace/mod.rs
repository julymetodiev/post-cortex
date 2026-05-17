// Copyright (c) 2025 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Workspace module for organizing related sessions (e.g., microservices)
//!
//! Provides lock-free workspace management with DashMap-based concurrent access.
//! Enables logical grouping of sessions that belong to the same project ecosystem.

use arc_swap::ArcSwap;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use uuid::Uuid;

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

/// Metadata for a workspace
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceMetadata {
    /// Type of project (e.g., "microservices", "monorepo")
    pub project_type: Option<String>,
    /// Tags for categorization
    pub tags: Vec<String>,
    /// Physical paths to project roots
    pub root_paths: Vec<PathBuf>,
    /// Additional user-defined metadata
    pub custom_data: std::collections::HashMap<String, String>,
}

/// A workspace groups related sessions (e.g., microservices in same ecosystem)
///
/// All operations are lock-free using DashMap and ArcSwap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    /// Unique workspace identifier
    pub id: Uuid,
    /// Human-readable workspace name
    pub name: String,
    /// Workspace description
    pub description: String,
    /// When the workspace was created
    pub created_at: SystemTime,

    /// Lock-free session tracking with roles
    #[serde(skip)]
    pub session_ids: Arc<DashMap<Uuid, SessionRole>>,

    /// Atomic metadata updates
    #[serde(skip)]
    pub metadata: Arc<ArcSwap<WorkspaceMetadata>>,
}

impl Workspace {
    /// Creates a new workspace with the given ID, name, and description
    pub fn new(id: Uuid, name: String, description: String) -> Self {
        Self {
            id,
            name,
            description,
            created_at: SystemTime::now(),
            session_ids: Arc::new(DashMap::new()),
            metadata: Arc::new(ArcSwap::from_pointee(WorkspaceMetadata::default())),
        }
    }

    /// Add a session to this workspace (lock-free)
    pub fn add_session(&self, session_id: Uuid, role: SessionRole) {
        self.session_ids.insert(session_id, role);
    }

    /// Remove a session from this workspace (lock-free)
    pub fn remove_session(&self, session_id: &Uuid) -> Option<SessionRole> {
        self.session_ids.remove(session_id).map(|(_, role)| role)
    }

    /// Get role of a session (lock-free)
    pub fn get_session_role(&self, session_id: &Uuid) -> Option<SessionRole> {
        self.session_ids.get(session_id).map(|entry| *entry.value())
    }

    /// Get all session IDs (lock-free snapshot)
    pub fn get_all_sessions(&self) -> Vec<(Uuid, SessionRole)> {
        self.session_ids
            .iter()
            .map(|entry| (*entry.key(), *entry.value()))
            .collect()
    }

    /// Update metadata atomically
    pub fn update_metadata<F>(&self, f: F)
    where
        F: FnOnce(&mut WorkspaceMetadata),
    {
        let current = self.metadata.load();
        let mut new_metadata = (**current).clone();
        f(&mut new_metadata);
        self.metadata.store(Arc::new(new_metadata));
    }
}

/// Lock-free workspace manager
///
/// Manages all workspaces with zero blocking operations using DashMap.
pub struct WorkspaceManager {
    /// Lock-free workspace cache
    workspaces: Arc<DashMap<Uuid, Arc<ArcSwap<Workspace>>>>,

    /// Lock-free name index for fast lookup
    name_index: Arc<DashMap<String, Uuid>>,

    /// Atomic counters
    total_workspaces: Arc<AtomicU64>,
}

impl WorkspaceManager {
    /// Creates a new empty workspace manager
    pub fn new() -> Self {
        Self {
            workspaces: Arc::new(DashMap::new()),
            name_index: Arc::new(DashMap::new()),
            total_workspaces: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Create a new workspace (lock-free)
    pub fn create_workspace(&self, name: String, description: String) -> Uuid {
        let id = Uuid::new_v4();
        let workspace = Workspace::new(id, name.clone(), description);

        self.workspaces
            .insert(id, Arc::new(ArcSwap::from_pointee(workspace)));
        self.name_index.insert(name, id);
        self.total_workspaces.fetch_add(1, Ordering::Relaxed);

        id
    }

    /// Restore a workspace from storage (lock-free)
    pub fn restore_workspace(
        &self,
        id: Uuid,
        name: String,
        description: String,
        sessions: Vec<(Uuid, SessionRole)>,
    ) {
        let workspace = Workspace::new(id, name.clone(), description);

        // Restore sessions
        for (session_id, role) in sessions {
            workspace.add_session(session_id, role);
        }

        self.workspaces
            .insert(id, Arc::new(ArcSwap::from_pointee(workspace)));
        self.name_index.insert(name, id);
        self.total_workspaces.fetch_add(1, Ordering::Relaxed);
    }

    /// Get workspace by ID (lock-free)
    pub fn get_workspace(&self, id: &Uuid) -> Option<Arc<Workspace>> {
        self.workspaces.get(id).map(|entry| {
            let arc_swap = entry.value();
            arc_swap.load_full()
        })
    }

    /// Get workspace by name (lock-free)
    pub fn get_workspace_by_name(&self, name: &str) -> Option<Arc<Workspace>> {
        self.name_index.get(name).and_then(|entry| {
            let id = *entry.value();
            self.get_workspace(&id)
        })
    }

    /// List all workspaces (lock-free snapshot)
    pub fn list_workspaces(&self) -> Vec<Arc<Workspace>> {
        self.workspaces
            .iter()
            .map(|entry| entry.value().load_full())
            .collect()
    }

    /// Delete workspace (lock-free)
    pub fn delete_workspace(&self, id: &Uuid) -> Option<Arc<Workspace>> {
        if let Some((_, arc_swap)) = self.workspaces.remove(id) {
            let workspace = arc_swap.load_full();
            self.name_index.remove(&workspace.name);
            self.total_workspaces.fetch_sub(1, Ordering::Relaxed);
            Some(workspace)
        } else {
            None
        }
    }

    /// Add session to workspace (lock-free)
    pub fn add_session_to_workspace(
        &self,
        workspace_id: &Uuid,
        session_id: Uuid,
        role: SessionRole,
    ) -> Result<(), String> {
        self.get_workspace(workspace_id)
            .ok_or_else(|| format!("Workspace {} not found", workspace_id))?
            .add_session(session_id, role);
        Ok(())
    }

    /// Remove session from workspace (lock-free)
    pub fn remove_session_from_workspace(
        &self,
        workspace_id: &Uuid,
        session_id: &Uuid,
    ) -> Result<Option<SessionRole>, String> {
        self.get_workspace(workspace_id)
            .ok_or_else(|| format!("Workspace {} not found", workspace_id))?
            .remove_session(session_id)
            .ok_or_else(|| format!("Session {} not in workspace", session_id))
            .map(Some)
    }

    /// Get total workspace count (atomic read)
    pub fn total_workspaces(&self) -> u64 {
        self.total_workspaces.load(Ordering::Relaxed)
    }
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_creation() {
        let manager = WorkspaceManager::new();

        let ws_id = manager.create_workspace(
            "Test Workspace".to_string(),
            "Testing workspace".to_string(),
        );

        let workspace = manager.get_workspace(&ws_id).unwrap();
        assert_eq!(workspace.name, "Test Workspace");
        assert_eq!(workspace.description, "Testing workspace");
        assert_eq!(manager.total_workspaces(), 1);
    }

    #[test]
    fn test_session_management() {
        let manager = WorkspaceManager::new();
        let ws_id = manager.create_workspace("Test".to_string(), "".to_string());

        let session1 = Uuid::new_v4();
        let session2 = Uuid::new_v4();

        manager
            .add_session_to_workspace(&ws_id, session1, SessionRole::Primary)
            .unwrap();
        manager
            .add_session_to_workspace(&ws_id, session2, SessionRole::Related)
            .unwrap();

        let workspace = manager.get_workspace(&ws_id).unwrap();
        assert_eq!(workspace.session_ids.len(), 2);
        assert_eq!(
            workspace.get_session_role(&session1),
            Some(SessionRole::Primary)
        );
        assert_eq!(
            workspace.get_session_role(&session2),
            Some(SessionRole::Related)
        );
    }

    #[test]
    fn test_workspace_lookup_by_name() {
        let manager = WorkspaceManager::new();
        let ws_id = manager.create_workspace("MyWorkspace".to_string(), "".to_string());

        let workspace = manager.get_workspace_by_name("MyWorkspace").unwrap();
        assert_eq!(workspace.id, ws_id);
    }

    #[tokio::test]
    async fn test_concurrent_workspace_operations() {
        let manager = Arc::new(WorkspaceManager::new());

        // Spawn 50 concurrent tasks creating workspaces
        let tasks: Vec<_> = (0..50)
            .map(|i| {
                let mgr = manager.clone();
                tokio::spawn(async move {
                    let ws_id =
                        mgr.create_workspace(format!("Workspace {}", i), format!("Test {}", i));

                    // Add 5 sessions to each workspace
                    for _ in 0..5 {
                        let session_id = Uuid::new_v4();
                        mgr.add_session_to_workspace(&ws_id, session_id, SessionRole::Related)
                            .unwrap();
                    }

                    ws_id
                })
            })
            .collect();

        // Wait for all tasks
        let workspace_ids: Vec<_> = futures::future::join_all(tasks)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        // Verify all workspaces created
        assert_eq!(manager.total_workspaces(), 50);

        // Verify each workspace has 5 sessions
        for ws_id in workspace_ids {
            let workspace = manager.get_workspace(&ws_id).unwrap();
            assert_eq!(workspace.session_ids.len(), 5);
        }
    }
}
