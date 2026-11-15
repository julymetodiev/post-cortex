// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Radar dismissals and the [`DismissalStore`] trait implementation.
//!
//! Rows of the `radar_dismissal` table, unique on `(workspace_id, kind,
//! key)`; a dismissal of the same item replaces the earlier row in one
//! transaction.

use crate::port_error::PortResult;
use crate::port_error::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use post_cortex_core::core::dismissal::{RadarDismissal, RadarItemKind};
use post_cortex_ports::storage::DismissalStore;

use crate::surrealdb_storage::SurrealDBStorage;

/// Row of the `radar_dismissal` table.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct DismissalRecord {
    workspace_id: String,
    kind: String,
    key: String,
    dismissed_at: String,
    author: Option<String>,
}

impl DismissalRecord {
    fn into_dismissal(self) -> Result<RadarDismissal> {
        Ok(RadarDismissal {
            kind: post_cortex_ports::error::parse_name("radar dismissal", &self.key, &self.kind)?,
            dismissed_at: post_cortex_ports::error::parse_timestamp(
                "radar dismissal",
                &self.key,
                &self.dismissed_at,
            )?,
            key: self.key,
            author: self.author,
        })
    }
}

#[async_trait]
impl DismissalStore for SurrealDBStorage {
    async fn put_dismissal(
        &self,
        workspace_id: Uuid,
        dismissal: &RadarDismissal,
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                let record = DismissalRecord {
                    workspace_id: workspace_id.to_string(),
                    kind: dismissal.kind.as_str().to_string(),
                    key: dismissal.key.clone(),
                    dismissed_at: dismissal.dismissed_at.to_rfc3339(),
                    author: dismissal.author.clone(),
                };
                self.db()
                    .query(
                        "BEGIN TRANSACTION; \
                 DELETE radar_dismissal WHERE workspace_id = $record.workspace_id \
                     AND kind = $record.kind AND key = $record.key; \
                 CREATE radar_dismissal CONTENT $record RETURN NONE; \
                 COMMIT TRANSACTION;",
                    )
                    .bind(("record", record))
                    .await?
                    .check()?;
                Ok(())
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
                let mut response = self
                    .db()
                    .query(
                        "BEGIN TRANSACTION; \
                 LET $gone = SELECT VALUE key FROM radar_dismissal \
                     WHERE workspace_id = $ws AND kind = $kind AND key = $key; \
                 DELETE radar_dismissal WHERE workspace_id = $ws AND kind = $kind AND key = $key; \
                 RETURN $gone; \
                 COMMIT TRANSACTION;",
                    )
                    .bind(("ws", workspace_id.to_string()))
                    .bind(("kind", kind.as_str().to_string()))
                    .bind(("key", key.to_string()))
                    .await?
                    .check()?;
                let gone: Vec<String> = response.take(3)?;
                Ok(!gone.is_empty())
            })
            .await
    }

    async fn list_dismissals(&self, workspace_id: Uuid) -> PortResult<Vec<RadarDismissal>> {
        self.breaker
            .guard(async {
                let mut response = self
                    .db()
                    .query(
                        "SELECT * OMIT id FROM radar_dismissal WHERE workspace_id = $ws \
                 ORDER BY kind, key",
                    )
                    .bind(("ws", workspace_id.to_string()))
                    .await?
                    .check()?;
                let rows: Vec<DismissalRecord> = response.take(0)?;
                rows.into_iter()
                    .map(DismissalRecord::into_dismissal)
                    .collect()
            })
            .await
    }
}
