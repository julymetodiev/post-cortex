// Copyright (c) 2025, 2026 Julius ML
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

//! Workspace metadata persistence and workspace<->session association records.

use anyhow::Result;
use rocksdb::WriteBatch;
use std::collections::HashMap;
use tracing::info;
use uuid::Uuid;

use post_cortex_core::workspace::SessionRole;

use super::RealRocksDBStorage;
use super::types::{StoredWorkspace, StoredWorkspaceSession};

impl RealRocksDBStorage {
    /// List all workspaces with their sessions (hydrated from ws_session records)
    pub async fn list_workspaces(&self) -> Result<Vec<StoredWorkspace>> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<Vec<StoredWorkspace>> {
            let mut workspaces: HashMap<Uuid, StoredWorkspace> = HashMap::new();

            // First pass: Load all workspaces using iterator with seek
            // (prefix_iterator doesn't work with our 16-byte prefix extractor)
            let workspace_iter = db.iterator(rocksdb::IteratorMode::From(
                b"workspace:",
                rocksdb::Direction::Forward,
            ));
            for item in workspace_iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                // Stop when we've passed the "workspace:" prefix
                if !key_str.starts_with("workspace:") {
                    break;
                }

                if let Ok((mut workspace, _)) =
                    bincode::serde::decode_from_slice::<StoredWorkspace, _>(
                        &value,
                        bincode::config::standard(),
                    )
                {
                    // Clear stored sessions as they might be stale (source of truth is ws_session records)
                    workspace.sessions.clear();
                    workspaces.insert(workspace.id, workspace);
                }
            }

            // Second pass: Load all workspace-session associations using iterator with seek
            let ws_session_iter = db.iterator(rocksdb::IteratorMode::From(
                b"ws_session:",
                rocksdb::Direction::Forward,
            ));
            for item in ws_session_iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                // Stop when we've passed the "ws_session:" prefix
                if !key_str.starts_with("ws_session:") {
                    break;
                }

                if let Ok((ws_session, _)) = bincode::serde::decode_from_slice::<
                    StoredWorkspaceSession,
                    _,
                >(&value, bincode::config::standard())
                    && let Some(ws) = workspaces.get_mut(&ws_session.workspace_id)
                {
                    ws.sessions.push((ws_session.session_id, ws_session.role));
                }
            }

            Ok(workspaces.into_values().collect())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Save workspace metadata to RocksDB
    pub async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> Result<()> {
        info!(
            "RealRocksDBStorage: Saving workspace {} ({})",
            name, workspace_id
        );

        let db = self.db.clone();
        let name = name.to_string();
        let description = description.to_string();
        let session_ids = session_ids.to_vec();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let workspace_data = StoredWorkspace {
                id: workspace_id,
                name,
                description,
                sessions: session_ids
                    .iter()
                    .map(|id| (*id, SessionRole::Primary))
                    .collect(),
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            };

            let data = bincode::serde::encode_to_vec(&workspace_data, bincode::config::standard())
                .map_err(|e| anyhow::anyhow!("Failed to serialize workspace: {}", e))?;

            let key = format!("workspace:{}", workspace_id);
            db.put(key.as_bytes(), data)?;

            info!(
                "RealRocksDBStorage: Workspace {} saved successfully",
                workspace_id
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Delete workspace from RocksDB
    pub async fn delete_workspace(&self, workspace_id: Uuid) -> Result<()> {
        info!("RealRocksDBStorage: Deleting workspace {}", workspace_id);

        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let key = format!("workspace:{}", workspace_id);
            db.delete(key.as_bytes())?;

            // Also delete all workspace-session associations
            let ws_session_prefix = format!("ws_session:{}:", workspace_id);
            let iter = db.iterator(rocksdb::IteratorMode::From(
                ws_session_prefix.as_bytes(),
                rocksdb::Direction::Forward,
            ));
            let mut keys_to_delete = Vec::new();

            for item in iter {
                let (key, _) = item?;
                let key_str = String::from_utf8_lossy(&key);

                // Stop when we've passed our prefix
                if !key_str.starts_with(&ws_session_prefix) {
                    break;
                }
                keys_to_delete.push(key.to_vec());
            }

            let mut batch = WriteBatch::default();
            for key in keys_to_delete {
                batch.delete(&key);
            }
            db.write(batch)?;

            info!(
                "RealRocksDBStorage: Workspace {} deleted successfully",
                workspace_id
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Add session to workspace association
    pub async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: SessionRole,
    ) -> Result<()> {
        info!(
            "RealRocksDBStorage: Adding session {} to workspace {} with role {:?}",
            session_id, workspace_id, role
        );

        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let ws_session = StoredWorkspaceSession {
                workspace_id,
                session_id,
                role,
                added_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            };

            let data = bincode::serde::encode_to_vec(&ws_session, bincode::config::standard())
                .map_err(|e| anyhow::anyhow!("Failed to serialize workspace-session: {}", e))?;

            let key = format!("ws_session:{}:{}", workspace_id, session_id);
            db.put(key.as_bytes(), data)?;

            info!(
                "RealRocksDBStorage: Session {} added to workspace {} successfully",
                session_id, workspace_id
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Remove session from workspace association
    pub async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<()> {
        info!(
            "RealRocksDBStorage: Removing session {} from workspace {}",
            session_id, workspace_id
        );

        let db = self.db.clone();
        let key = format!("ws_session:{}:{}", workspace_id, session_id);

        tokio::task::spawn_blocking(move || -> Result<()> {
            db.delete(key.as_bytes())?;

            info!(
                "RealRocksDBStorage: Session {} removed from workspace {} successfully",
                session_id, workspace_id
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }
}
