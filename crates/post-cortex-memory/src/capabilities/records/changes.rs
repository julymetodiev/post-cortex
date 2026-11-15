// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Changing stored records: delete, forget (retire), purge (erase), and the
//! change feed.

use uuid::Uuid;

use crate::capabilities::records::{RecordError, Records};
use crate::memory_system::MemoryRuntime;

impl Records {
    /// Remove a record (a ghost or bad row) and its embedding (memory,
    /// storage, query cache); returns whether it existed.
    ///
    /// # Errors
    /// `SessionNotFound`, contention or storage failed.
    pub async fn delete(&self, session_id: Uuid, entry_id: Uuid) -> Result<bool, RecordError> {
        let handle = self.handle(session_id).await?;
        let updated =
            MemoryRuntime::cas_update_session(&handle, session_id, "delete_update", |session| {
                session.remove_records(&[entry_id]).len() == 1
            })
            .await
            .map_err(RecordError::Storage)?;
        if updated.is_none() {
            return Ok(false);
        }
        // Shared set: drop the id before the snapshot is persisted.
        handle.vectorized().unmark(&entry_id);
        // Named: storage never infers a removal from the snapshot.
        self.rt
            .persister
            .persist_removals_nowait(&handle, vec![entry_id]);
        self.rt
            .drop_content_vectors(session_id, &[entry_id.to_string()])
            .await;
        Ok(true)
    }

    /// Retire a record: its validity ends now, it stays in the history.
    /// Returns false if it is unknown or already retired.
    ///
    /// # Errors
    /// `SessionNotFound`, contention or storage failed.
    pub async fn forget(&self, session_id: Uuid, entry_id: Uuid) -> Result<bool, RecordError> {
        let handle = self.handle(session_id).await?;
        let now = chrono::Utc::now();
        let mut changed = None;
        let updated =
            MemoryRuntime::cas_update_session(&handle, session_id, "forget_update", |session| {
                changed = session.retire_update(&entry_id, now);
                changed.is_some()
            })
            .await
            .map_err(RecordError::Storage)?;
        match (updated, changed) {
            (Some(_), Some(update)) => {
                // Named explicitly, so the stored row is rewritten.
                self.rt.persister.persist_nowait(&handle, vec![update]);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Erase a record and everything derived from it (GDPR erasure): the
    /// record row, its embedding and full-text entry, and the graph, which
    /// is rebuilt from the remaining records so entities, descriptions and
    /// edges only it introduced are gone from memory and storage. Returns
    /// false if it is unknown.
    ///
    /// # Errors
    /// `SessionNotFound`, contention or storage failed.
    pub async fn purge(&self, session_id: Uuid, entry_id: Uuid) -> Result<bool, RecordError> {
        let handle = self.handle(session_id).await?;
        let updated =
            MemoryRuntime::cas_update_session(&handle, session_id, "purge_update", |session| {
                if session.remove_records(&[entry_id]).is_empty() {
                    return false;
                }
                session.rebuild_entity_graph();
                true
            })
            .await
            .map_err(RecordError::Storage)?;
        if updated.is_none() {
            return Ok(false);
        }
        handle.vectorized().unmark(&entry_id);
        let rt = &self.rt;
        // The stored graph is rewritten whole: rows of entities and edges
        // only the record introduced must not come back on the next load.
        rt.persister.persist_rebuild_nowait(&handle, vec![entry_id]);
        rt.drop_content_vectors(session_id, &[entry_id.to_string()])
            .await;
        rt.session_manager.invalidate_derived(&session_id);
        Ok(true)
    }
}
