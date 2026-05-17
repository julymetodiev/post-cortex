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

//! Session, checkpoint, and context-update persistence plus the [`Storage`]
//! trait implementation (which dispatches to inherent helpers across this
//! module).

use anyhow::Result;
use async_trait::async_trait;
use rocksdb::WriteBatch;
use tracing::{debug, info};
use uuid::Uuid;

use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::session::active_session::ActiveSession;
use crate::traits::Storage;
use post_cortex_core::workspace::SessionRole;

use super::RealRocksDBStorage;
use super::types::{SessionCheckpoint, StoredWorkspace};

impl RealRocksDBStorage {
    /// Save session to RocksDB
    pub async fn save_session(&self, session: &ActiveSession) -> Result<()> {
        info!(
            "RealRocksDBStorage: Saving session with ID: {}",
            session.id()
        );

        let db = self.db.clone();
        let session = session.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            // Serialize session data with serde_json (bincode doesn't support deserialize_any)
            let session_data = serde_json::to_vec(&session)
                .map_err(|e| anyhow::anyhow!("Failed to serialize session: {}", e))?;

            info!(
                "RealRocksDBStorage: Session data serialized, size: {} bytes",
                session_data.len()
            );

            // Use binary key for better performance
            let key = format!("session:{}", session.id());

            // Write to RocksDB
            db.put(key.as_bytes(), &session_data)?;

            info!("RealRocksDBStorage: Session saved to RocksDB");
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Load session from RocksDB
    pub async fn load_session(&self, session_id: Uuid) -> Result<ActiveSession> {
        info!(
            "RealRocksDBStorage: Loading session with ID: {}",
            session_id
        );

        let db = self.db.clone();
        let key = format!("session:{}", session_id);

        tokio::task::spawn_blocking(move || -> Result<ActiveSession> {
            match db.get(key.as_bytes())? {
                Some(data) => {
                    info!(
                        "RealRocksDBStorage: Session data found, size: {} bytes",
                        data.len()
                    );

                    let session: ActiveSession = serde_json::from_slice(&data)
                        .map_err(|e| anyhow::anyhow!("Failed to deserialize session: {}", e))?;

                    info!("RealRocksDBStorage: Session deserialized successfully");
                    Ok(session)
                }
                None => {
                    info!("RealRocksDBStorage: Session not found");
                    Err(anyhow::anyhow!("Session not found"))
                }
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Save checkpoint to RocksDB
    pub async fn save_checkpoint(&self, checkpoint: &SessionCheckpoint) -> Result<()> {
        let db = self.db.clone();
        let checkpoint = checkpoint.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let checkpoint_data =
                bincode::serde::encode_to_vec(&checkpoint, bincode::config::standard())
                    .map_err(|e| anyhow::anyhow!("Failed to serialize checkpoint: {}", e))?;

            let key = format!("checkpoint:{}", checkpoint.id);
            db.put(key.as_bytes(), &checkpoint_data)?;

            info!(
                "RealRocksDBStorage: Checkpoint saved with ID: {}",
                checkpoint.id
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Load checkpoint from RocksDB
    pub async fn load_checkpoint(&self, checkpoint_id: Uuid) -> Result<SessionCheckpoint> {
        let db = self.db.clone();
        let key = format!("checkpoint:{}", checkpoint_id);

        tokio::task::spawn_blocking(move || -> Result<SessionCheckpoint> {
            match db.get(key.as_bytes())? {
                Some(data) => {
                    let (checkpoint, _): (SessionCheckpoint, usize) =
                        bincode::serde::decode_from_slice(&data, bincode::config::standard())
                            .map_err(|e| {
                                anyhow::anyhow!("Failed to deserialize checkpoint: {}", e)
                            })?;

                    info!(
                        "RealRocksDBStorage: Checkpoint loaded with ID: {}",
                        checkpoint_id
                    );
                    Ok(checkpoint)
                }
                None => Err(anyhow::anyhow!("Checkpoint not found")),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// List all checkpoints
    pub async fn list_checkpoints(&self) -> Result<Vec<SessionCheckpoint>> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<Vec<SessionCheckpoint>> {
            let mut checkpoints = Vec::new();
            let iter = db.iterator(rocksdb::IteratorMode::From(
                b"checkpoint:",
                rocksdb::Direction::Forward,
            ));

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with("checkpoint:") {
                    break;
                }

                if let Ok((checkpoint, _)) =
                    bincode::serde::decode_from_slice::<SessionCheckpoint, _>(
                        &value,
                        bincode::config::standard(),
                    )
                {
                    checkpoints.push(checkpoint);
                }
            }

            info!("RealRocksDBStorage: Listed {} checkpoints", checkpoints.len());
            Ok(checkpoints)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Batch save multiple updates efficiently
    pub async fn batch_save_updates(
        &self,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        let db = self.db.clone();
        let updates_len = updates.len();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut batch = WriteBatch::default();

            for update in &updates {
                let key = format!("session:{}:update:{}", session_id, update.id);
                let update_data =
                    bincode::serde::encode_to_vec(update, bincode::config::standard())
                        .map_err(|e| anyhow::anyhow!("Failed to serialize update: {}", e))?;

                batch.put(key.as_bytes(), &update_data);
            }

            db.write(batch)?;

            info!(
                "RealRocksDBStorage: Batch saved {} updates for session {}",
                updates_len, session_id
            );

            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Save session blob and context updates in a single atomic WriteBatch.
    /// Combines save_session + batch_save_updates into one spawn_blocking call.
    pub async fn save_session_with_updates(
        &self,
        session: &ActiveSession,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        let db = self.db.clone();
        let session = session.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut batch = rocksdb::WriteBatch::default();

            // Session blob (serde_json — required due to serde_json::Value fields)
            let session_key = format!("session:{}", session.id());
            let session_data = serde_json::to_vec(&session)
                .map_err(|e| anyhow::anyhow!("Failed to serialize session: {}", e))?;
            batch.put(session_key.as_bytes(), &session_data);

            // Context update records (bincode)
            for update in &updates {
                let update_key = format!("session:{}:update:{}", session_id, update.id);
                let update_data =
                    bincode::serde::encode_to_vec(update, bincode::config::standard())
                        .map_err(|e| anyhow::anyhow!("Failed to serialize update: {}", e))?;
                batch.put(update_key.as_bytes(), &update_data);
            }

            db.write(batch)?;

            debug!(
                "RealRocksDBStorage: Combined save - session {} + {} updates ({} bytes)",
                session_id,
                updates.len(),
                session_data.len()
            );

            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// List all session IDs
    pub async fn list_sessions(&self) -> Result<Vec<Uuid>> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<Vec<Uuid>> {
            let mut sessions = Vec::new();

            // Use iterator with seek - prefix_iterator doesn't work correctly
            // when prefix_extractor size (16 bytes) doesn't match our prefix (8 bytes "session:")
            let iter = db.iterator(rocksdb::IteratorMode::From(
                b"session:",
                rocksdb::Direction::Forward,
            ));
            for item in iter {
                let (key, _) = item?;
                let key_str = String::from_utf8_lossy(&key);

                // Stop when we've passed the "session:" prefix
                if !key_str.starts_with("session:") {
                    break;
                }

                // Skip update keys (format: "session:{id}:update:{update_id}")
                if key_str.contains(":update:") {
                    continue;
                }

                // Extract UUID from "session:{uuid}" format
                if let Some(uuid_str) = key_str.strip_prefix("session:")
                    && let Ok(uuid) = Uuid::parse_str(uuid_str)
                {
                    sessions.push(uuid);
                }
            }

            info!("RealRocksDBStorage: Found {} sessions", sessions.len());
            Ok(sessions)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Delete session and all related data
    pub async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut batch = WriteBatch::default();

            // Delete main session
            let session_key = format!("session:{}", session_id);
            batch.delete(session_key.as_bytes());

            // Delete all updates for this session
            // Key format: "session:{session_id}:update:{update_id}"
            let update_prefix = format!("session:{}:update:", session_id);
            let iter = db.iterator(rocksdb::IteratorMode::From(
                update_prefix.as_bytes(),
                rocksdb::Direction::Forward,
            ));
            let mut keys_to_delete = Vec::new();

            for item in iter {
                let (key, _) = item?;
                let key_str = String::from_utf8_lossy(&key);

                // Stop when we've passed our prefix (prefix_iterator may continue)
                if !key_str.starts_with(&update_prefix) {
                    break;
                }
                keys_to_delete.push(key.to_vec());
            }

            // Batch delete all related keys
            for key in keys_to_delete {
                batch.delete(&key);
            }
            db.write(batch)?;

            info!(
                "RealRocksDBStorage: Deleted session {} and related data",
                session_id
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Check if session exists
    pub async fn session_exists(&self, session_id: Uuid) -> Result<bool> {
        let db = self.db.clone();
        let key = format!("session:{}", session_id);

        tokio::task::spawn_blocking(move || -> Result<bool> {
            Ok(db.get(key.as_bytes())?.is_some())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Load all updates for a session
    pub async fn load_session_updates(&self, session_id: Uuid) -> Result<Vec<ContextUpdate>> {
        let db = self.db.clone();
        let update_prefix = format!("session:{}:update:", session_id);

        tokio::task::spawn_blocking(move || -> Result<Vec<ContextUpdate>> {
            let mut updates = Vec::new();
            let iter = db.iterator(rocksdb::IteratorMode::From(
                update_prefix.as_bytes(),
                rocksdb::Direction::Forward,
            ));

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with(&update_prefix) {
                    break;
                }

                if let Ok((update, _)) = bincode::serde::decode_from_slice::<ContextUpdate, _>(
                    &value,
                    bincode::config::standard(),
                ) {
                    updates.push(update);
                }
            }

            info!(
                "RealRocksDBStorage: Loaded {} updates for session {}",
                updates.len(),
                session_id
            );
            Ok(updates)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }
}

#[async_trait]
impl Storage for RealRocksDBStorage {
    async fn save_session(&self, session: &ActiveSession) -> Result<()> {
        RealRocksDBStorage::save_session(self, session).await
    }

    async fn load_session(&self, session_id: Uuid) -> Result<ActiveSession> {
        RealRocksDBStorage::load_session(self, session_id).await
    }

    async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        RealRocksDBStorage::delete_session(self, session_id).await
    }

    async fn list_sessions(&self) -> Result<Vec<Uuid>> {
        RealRocksDBStorage::list_sessions(self).await
    }

    async fn session_exists(&self, session_id: Uuid) -> Result<bool> {
        RealRocksDBStorage::session_exists(self, session_id).await
    }

    async fn batch_save_updates(
        &self,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        RealRocksDBStorage::batch_save_updates(self, session_id, updates).await
    }

    async fn save_session_with_updates(
        &self,
        session: &ActiveSession,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        RealRocksDBStorage::save_session_with_updates(self, session, session_id, updates).await
    }

    async fn load_session_updates(&self, session_id: Uuid) -> Result<Vec<ContextUpdate>> {
        RealRocksDBStorage::load_session_updates(self, session_id).await
    }

    async fn save_checkpoint(&self, checkpoint: &SessionCheckpoint) -> Result<()> {
        RealRocksDBStorage::save_checkpoint(self, checkpoint).await
    }

    async fn load_checkpoint(&self, checkpoint_id: Uuid) -> Result<SessionCheckpoint> {
        RealRocksDBStorage::load_checkpoint(self, checkpoint_id).await
    }

    async fn list_checkpoints(&self) -> Result<Vec<SessionCheckpoint>> {
        RealRocksDBStorage::list_checkpoints(self).await
    }

    async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> Result<()> {
        RealRocksDBStorage::save_workspace_metadata(
            self,
            workspace_id,
            name,
            description,
            session_ids,
        )
        .await
    }

    async fn delete_workspace(&self, workspace_id: Uuid) -> Result<()> {
        RealRocksDBStorage::delete_workspace(self, workspace_id).await
    }

    async fn list_workspaces(&self) -> Result<Vec<StoredWorkspace>> {
        RealRocksDBStorage::list_workspaces(self).await
    }

    async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: SessionRole,
    ) -> Result<()> {
        RealRocksDBStorage::add_session_to_workspace(self, workspace_id, session_id, role).await
    }

    async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<()> {
        RealRocksDBStorage::remove_session_from_workspace(self, workspace_id, session_id).await
    }

    async fn compact(&self) -> Result<()> {
        RealRocksDBStorage::compact(self).await
    }

    async fn get_key_count(&self) -> Result<usize> {
        RealRocksDBStorage::get_key_count(self).await
    }

    async fn get_stats(&self) -> Result<String> {
        RealRocksDBStorage::get_stats(self).await
    }

    async fn clear_session_entities(&self, session_id: Uuid) -> Result<()> {
        let entities = self.load_session_entities(session_id).await?;
        for entity in &entities {
            self.delete_stored_entity(session_id, &entity.name).await?;
        }
        Ok(())
    }
}
