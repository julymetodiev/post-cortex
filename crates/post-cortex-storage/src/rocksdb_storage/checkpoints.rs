// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Checkpoints: one row each (layout in [`crate::rocksdb_storage::rows`]), indexed by
//! session so they are deleted with it.

use crate::port_error::PortResult;
use crate::rocksdb_storage::RealRocksDBStorage;
use crate::rocksdb_storage::core::{blocking, scan_prefix};
use crate::rocksdb_storage::rows::{decode_checkpoint, encode_checkpoint};
use async_trait::async_trait;
use post_cortex_ports::error::corrupt;
use post_cortex_ports::storage::CheckpointStore;
use post_cortex_ports::storage::SessionCheckpoint;
use rocksdb::WriteBatch;
use tracing::info;
use uuid::Uuid;

pub const CHECKPOINT_PREFIX: &str = "checkpoint:";

pub fn checkpoint_key(checkpoint_id: Uuid) -> String {
    format!("{CHECKPOINT_PREFIX}{checkpoint_id}")
}

pub fn checkpoint_index_prefix(session_id: Uuid) -> String {
    format!("checkpoint_by_session:{session_id}:")
}

/// Per-session index entry of a checkpoint (empty value), so deleting a
/// session finds its checkpoints without scanning them all.
pub fn checkpoint_index_key(session_id: Uuid, checkpoint_id: Uuid) -> String {
    format!("{}{checkpoint_id}", checkpoint_index_prefix(session_id))
}

/// The checkpoint id of an index entry under `prefix`; an unreadable one
/// is `Corrupt`.
pub fn indexed_checkpoint(
    prefix: &str,
    index_key: &[u8],
) -> post_cortex_ports::error::Result<Uuid> {
    std::str::from_utf8(&index_key[prefix.len()..])
        .ok()
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(|| {
            corrupt(
                "checkpoint index",
                String::from_utf8_lossy(index_key),
                "bad id",
            )
        })
}

#[async_trait]
impl CheckpointStore for RealRocksDBStorage {
    /// Save checkpoint to RocksDB
    async fn save_checkpoint(&self, checkpoint: &SessionCheckpoint) -> PortResult<()> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let checkpoint = checkpoint.clone();
                blocking(move || {
                    let mut batch = WriteBatch::default();
                    batch.put(
                        checkpoint_key(checkpoint.id).as_bytes(),
                        encode_checkpoint(&checkpoint)?,
                    );
                    batch.put(
                        checkpoint_index_key(checkpoint.session_id, checkpoint.id).as_bytes(),
                        [],
                    );
                    db.write(batch)?;
                    info!(
                        "RealRocksDBStorage: Checkpoint saved with ID: {}",
                        checkpoint.id
                    );
                    Ok(())
                })
                .await
            })
            .await
    }

    /// The checkpoints the session's index lists, by id; a listed one that
    /// is missing is `Corrupt`.
    async fn list_checkpoints(&self, session_id: Uuid) -> PortResult<Vec<SessionCheckpoint>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                blocking(move || {
                    let prefix = checkpoint_index_prefix(session_id);
                    let mut checkpoints = Vec::new();
                    for item in scan_prefix(&db, prefix.as_bytes()) {
                        let (index_key, _) = item?;
                        let id = indexed_checkpoint(&prefix, &index_key)?;
                        let key = checkpoint_key(id);
                        let row = db
                            .get(key.as_bytes())?
                            .ok_or_else(|| corrupt("checkpoint", id, "indexed but missing"))?;
                        checkpoints.push(decode_checkpoint(&key, &row)?);
                    }
                    Ok(checkpoints)
                })
                .await
            })
            .await
    }
}
