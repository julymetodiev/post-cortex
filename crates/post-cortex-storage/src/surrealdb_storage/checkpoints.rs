// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Checkpoints, stored per session.

use crate::port_error::PortResult;
use crate::port_error::Result;
use crate::surrealdb_storage::SurrealDBStorage;
use crate::surrealdb_storage::records::CheckpointRecord;
use async_trait::async_trait;
use post_cortex_ports::storage::CheckpointStore;
use post_cortex_ports::storage::SessionCheckpoint;
use tracing::debug;
use uuid::Uuid;

impl CheckpointRecord {
    /// The stored checkpoint; any unreadable field is
    /// [`Corrupt`](post_cortex_ports::PortError::Corrupt).
    fn into_checkpoint(self) -> Result<SessionCheckpoint> {
        use post_cortex_ports::error::{corrupt, parse_timestamp, parse_uuid};
        const KIND: &str = "checkpoint";
        let id = parse_uuid(KIND, &self.checkpoint_id, &self.checkpoint_id)?;
        fn json<T: serde::de::DeserializeOwned>(
            id: Uuid,
            field: &str,
            value: serde_json::Value,
        ) -> Result<T> {
            serde_json::from_value(value)
                .map_err(|e| corrupt(KIND, id, format!("{field}: {e}")).into())
        }
        Ok(SessionCheckpoint {
            id,
            session_id: parse_uuid(KIND, id, &self.session_id)?,
            created_at: parse_timestamp(KIND, id, &self.created_at)?,
            structured_context: json(id, "structured_context", self.structured_context)?,
            recent_updates: json(id, "recent_updates", self.recent_updates)?,
            code_references: json(id, "code_references", self.code_references)?,
            change_history: json(id, "change_history", self.change_history)?,
            total_updates: self.total_updates as usize,
            context_quality_score: self.context_quality_score,
            compression_ratio: self.compression_ratio,
        })
    }
}

#[async_trait]
impl CheckpointStore for SurrealDBStorage {
    async fn save_checkpoint(&self, checkpoint: &SessionCheckpoint) -> PortResult<()> {
        self.breaker
            .guard(async {
                debug!(
                    "SurrealDBStorage: Saving checkpoint with ID: {}",
                    checkpoint.id
                );

                let record = CheckpointRecord {
                    checkpoint_id: checkpoint.id.to_string(),
                    session_id: checkpoint.session_id.to_string(),
                    created_at: checkpoint.created_at.to_rfc3339(),
                    structured_context: serde_json::to_value(&checkpoint.structured_context)?,
                    recent_updates: serde_json::to_value(&checkpoint.recent_updates)?,
                    code_references: serde_json::to_value(&checkpoint.code_references)?,
                    change_history: serde_json::to_value(&checkpoint.change_history)?,
                    total_updates: checkpoint.total_updates as u32,
                    context_quality_score: checkpoint.context_quality_score,
                    compression_ratio: checkpoint.compression_ratio,
                };

                let _: Option<CheckpointRecord> = self
                    .db()
                    .upsert(("checkpoint", checkpoint.id.to_string()))
                    .content(record)
                    .await?;

                debug!("SurrealDBStorage: Checkpoint saved successfully");

                Ok(())
            })
            .await
    }

    async fn list_checkpoints(&self, session_id: Uuid) -> PortResult<Vec<SessionCheckpoint>> {
        self.breaker
            .guard(async {
                let mut response = self
                    .db()
                    .query(
                        "SELECT * FROM checkpoint WHERE session_id = $session_id \
                         ORDER BY checkpoint_id;",
                    )
                    .bind(("session_id", session_id.to_string()))
                    .await?
                    .check()?;
                let records: Vec<CheckpointRecord> = response.take(0)?;
                records
                    .into_iter()
                    .map(CheckpointRecord::into_checkpoint)
                    .collect()
            })
            .await
    }
}
