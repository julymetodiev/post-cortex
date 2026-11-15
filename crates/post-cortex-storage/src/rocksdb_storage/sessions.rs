// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The [`SessionStore`] port on RocksDB (row layouts and key families in
//! [`crate::rocksdb_storage::rows`] and
//! [`crate::rocksdb_storage::session_rows`]). A save writes the rows its
//! `SessionChanges` name, in one `WriteBatch` under the session's lock, so
//! its cost is what it changes, not the session's size.

use crate::port_error::PortResult;
use crate::rocksdb_storage::rows::decode_meta;
use crate::rocksdb_storage::session_rows::{
    SessionData, queue_fresh, queue_save, queue_session_data_delete, read_session, session_key,
};
use async_trait::async_trait;
use rocksdb::WriteBatch;
use std::collections::HashMap;
use tracing::{debug, info};
use uuid::Uuid;

use post_cortex_core::session::active_session::{ActiveSession, StoredSession};
use post_cortex_ports::error::{corrupt, session_not_found};
use post_cortex_ports::storage::{SessionChanges, SessionStore, SessionSummary};

use crate::rocksdb_storage::RealRocksDBStorage;
use crate::rocksdb_storage::core::{blocking, scan_prefix};
use crate::rocksdb_storage::session_listing::list_session_ids;
use crate::rocksdb_storage::session_rows::META_PREFIX;

/// Key prefix of the memberships of every workspace.
const MEMBERSHIP_PREFIX: &[u8] = b"ws_session:";

#[async_trait]
impl SessionStore for RealRocksDBStorage {
    /// Save `session` with `changes`: the head, the rows `changes` name and
    /// the listing record, in one batch.
    async fn save_session(
        &self,
        session: &ActiveSession,
        changes: SessionChanges,
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                // Cheap: a session is a few `Arc`s.
                let session = session.clone();
                blocking(move || {
                    let _guard = locks.lock(session.id());
                    let mut batch = WriteBatch::default();
                    let rows = queue_save(&db, &mut batch, &session, &changes)?;
                    db.write(batch)?;
                    debug!(
                        "RealRocksDBStorage: Saved session {} ({} record rows, {} removed, whole graph: {})",
                        session.id(),
                        rows,
                        changes.removed.len(),
                        changes.whole_graph
                    );
                    Ok(())
                })
                .await
            })
            .await
    }

    /// Load a session: head, records, graph and vectorized ids from one
    /// snapshot. Reads never write.
    async fn load_session(&self, session_id: Uuid) -> PortResult<StoredSession> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                blocking(move || {
                    let stored = read_session(&db, session_id)?.ok_or_else(|| {
                        debug!("RealRocksDBStorage: Session {} not found", session_id);
                        session_not_found(session_id)
                    })?;
                    debug!(
                        "RealRocksDBStorage: Loaded session {} ({} updates)",
                        session_id,
                        stored.session.records().len()
                    );
                    Ok(stored)
                })
                .await
            })
            .await
    }

    /// Delete a session and everything stored under its id: head, records,
    /// graph, vectorized keys, meta, messages, embeddings, checkpoints and
    /// workspace memberships, in one batch.
    async fn delete_session(&self, session_id: Uuid) -> PortResult<bool> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let locks = self.session_locks.clone();

                blocking(move || {
                    let _guard = locks.lock(session_id);
                    let existed = db.get(session_key(session_id).as_bytes())?.is_some();
                    let mut batch = WriteBatch::default();
                    let deleted =
                        queue_session_data_delete(&db, &mut batch, session_id, SessionData::All)?;
                    // `ws_session:{workspace}:{session}`: the session is the key's tail.
                    let tail = format!(":{session_id}");
                    for item in scan_prefix(&db, MEMBERSHIP_PREFIX) {
                        let (key, _) = item?;
                        if key.ends_with(tail.as_bytes()) {
                            batch.delete(&key);
                        }
                    }
                    db.write(batch)?;
                    info!(
                        "RealRocksDBStorage: Deleted session {} and {} related records",
                        session_id, deleted
                    );
                    Ok(existed)
                })
                .await
            })
            .await
    }

    /// List all session IDs
    async fn list_sessions(&self) -> PortResult<Vec<Uuid>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let sessions = blocking(move || list_session_ids(&db)).await?;
                debug!("RealRocksDBStorage: Found {} sessions", sessions.len());
                Ok(sessions)
            })
            .await
    }

    /// Metadata of every stored session from the `session_meta:` records.
    /// A session without a readable record is `Corrupt`.
    async fn list_session_summaries(&self) -> PortResult<Vec<SessionSummary>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                blocking(move || {
                    let mut metas: HashMap<Uuid, SessionSummary> = HashMap::new();
                    for item in scan_prefix(&db, META_PREFIX.as_bytes()) {
                        let (key, value) = item?;
                        let summary = decode_meta(&String::from_utf8_lossy(&key), &value)?;
                        metas.insert(summary.id, summary);
                    }
                    list_session_ids(&db)?
                        .into_iter()
                        .map(|id| {
                            metas
                                .remove(&id)
                                .ok_or_else(|| corrupt("session", id, "no listing record").into())
                        })
                        .collect()
                })
                .await
            })
            .await
    }

    /// Check if session exists
    async fn session_exists(&self, session_id: Uuid) -> PortResult<bool> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let key = session_key(session_id);
                blocking(move || Ok(db.get(key.as_bytes())?.is_some())).await
            })
            .await
    }

    /// Replace a stored session with `session`: the target's records,
    /// graph, vectors and messages are deleted and `session` is written,
    /// in one batch (what `import --overwrite` means). Checkpoints and
    /// workspace memberships stay.
    async fn replace_session(
        &self,
        session: &ActiveSession,
        vectorized: &[Uuid],
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                let session = session.clone();
                let vectorized = vectorized.to_vec();
                blocking(move || {
                    let _guard = locks.lock(session.id());
                    let mut batch = WriteBatch::default();
                    queue_session_data_delete(&db, &mut batch, session.id(), SessionData::Content)?;
                    queue_fresh(&mut batch, &session, &vectorized)?;
                    db.write(batch)?;
                    Ok(())
                })
                .await
            })
            .await
    }
}
