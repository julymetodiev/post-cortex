// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Radar dismissals and the [`DismissalStore`] trait implementation.
//!
//! Key family: `radar_dismissal:{workspace_id}:{kind}:{key}` -> bincode
//! [`RadarDismissal`]. The key is the item's own key, so a dismissal of the
//! same item replaces the earlier one.

use crate::port_error::PortResult;
use crate::port_error::Result;
use async_trait::async_trait;
use rocksdb::{DB, WriteBatch};
use uuid::Uuid;

use post_cortex_core::core::dismissal::{RadarDismissal, RadarItemKind};
use post_cortex_ports::storage::DismissalStore;

use crate::rocksdb_storage::RealRocksDBStorage;
use crate::rocksdb_storage::core::{blocking, keys_with_prefix, scan_prefix};
use crate::rocksdb_storage::rows::{DISMISSAL_V1, decode_marked, encode_marked};

fn workspace_prefix(workspace_id: Uuid) -> String {
    format!("radar_dismissal:{workspace_id}:")
}

fn dismissal_key(workspace_id: Uuid, kind: RadarItemKind, key: &str) -> String {
    format!("radar_dismissal:{workspace_id}:{}:{key}", kind.as_str())
}

fn encode(dismissal: &RadarDismissal) -> Result<Vec<u8>> {
    encode_marked(DISMISSAL_V1, dismissal)
}

fn decode(key: &[u8], row: &[u8]) -> Result<RadarDismissal> {
    decode_marked(
        "radar dismissal",
        &String::from_utf8_lossy(key),
        DISMISSAL_V1,
        row,
    )
}

/// Delete the dismissals of `workspace_id` in `batch`.
pub fn delete_dismissals_in(db: &DB, batch: &mut WriteBatch, workspace_id: Uuid) -> Result<()> {
    for key in keys_with_prefix(db, workspace_prefix(workspace_id).as_bytes())? {
        batch.delete(&key);
    }
    Ok(())
}

#[async_trait]
impl DismissalStore for RealRocksDBStorage {
    async fn put_dismissal(
        &self,
        workspace_id: Uuid,
        dismissal: &RadarDismissal,
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let key = dismissal_key(workspace_id, dismissal.kind, &dismissal.key);
                let value = encode(dismissal)?;
                blocking(move || Ok(db.put(key.as_bytes(), value)?)).await
            })
            .await
    }

    async fn delete_dismissal(
        &self,
        workspace_id: Uuid,
        kind: RadarItemKind,
        key: &str,
    ) -> PortResult<bool> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                let key = dismissal_key(workspace_id, kind, key);
                blocking(move || {
                    // Read and delete together, so two restores report one deletion.
                    let _guard = locks.lock(workspace_id);
                    let existed = db.get(key.as_bytes())?.is_some();
                    if existed {
                        db.delete(key.as_bytes())?;
                    }
                    Ok(existed)
                })
                .await
            })
            .await
    }

    async fn list_dismissals(&self, workspace_id: Uuid) -> PortResult<Vec<RadarDismissal>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let prefix = workspace_prefix(workspace_id);
                blocking(move || {
                    let mut dismissals = Vec::new();
                    for item in scan_prefix(&db, prefix.as_bytes()) {
                        let (key, value) = item?;
                        dismissals.push(decode(&key, &value)?);
                    }
                    Ok(dismissals)
                })
                .await
            })
            .await
    }
}
