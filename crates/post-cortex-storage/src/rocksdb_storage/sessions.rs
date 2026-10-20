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
//!
//! Key families for a session `{id}`:
//! - `session:{id}` -> JSON session blob. Written with an empty
//!   `incremental_updates` array; blobs from older versions embed them all.
//! - `session:{id}:update:{update_id}` -> bincode [`ContextUpdate`], one row
//!   per update.
//! - `session_updates:{id}` -> ids of the session's updates in session
//!   order, 16 raw bytes each. Its presence means the updates live in the
//!   rows; invariant: every listed id has a row.
//! - `session_meta:{id}` -> JSON [`SessionSummary`] for listings.
//!
//! All of a save is one `WriteBatch`, so blob, rows, index and meta never
//! disagree on disk.

use anyhow::Result;
use async_trait::async_trait;
use rocksdb::{DB, WriteBatch};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::session_not_found;
use crate::traits::{SessionSummary, Storage};
use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_core::workspace::SessionRole;

use super::RealRocksDBStorage;
use super::core::{SessionLocks, blocking, keys_with_prefix, prefix_read_opts, scan_prefix};
use super::legacy::{CHECKPOINT_V3, decode_checkpoint, decode_update};
use super::messages::message_next_key;
use super::types::{SessionCheckpoint, StoredWorkspace, StoredWorkspaceSession};

const SESSION_PREFIX: &str = "session:";
const META_PREFIX: &str = "session_meta:";
/// `session:` + hyphenated UUID.
const SESSION_KEY_LEN: usize = SESSION_PREFIX.len() + 36;

fn session_key(session_id: Uuid) -> String {
    format!("session:{}", session_id)
}

fn update_key(session_id: Uuid, update_id: Uuid) -> String {
    format!("session:{}:update:{}", session_id, update_id)
}

fn update_index_key(session_id: Uuid) -> String {
    format!("session_updates:{}", session_id)
}

fn meta_key(session_id: Uuid) -> String {
    format!("session_meta:{}", session_id)
}

fn encode_ids<'a>(ids: impl Iterator<Item = &'a Uuid>) -> Vec<u8> {
    ids.flat_map(|id| *id.as_bytes()).collect()
}

fn decode_ids(bytes: &[u8]) -> Vec<Uuid> {
    let (chunks, _) = bytes.as_chunks::<16>();
    chunks.iter().map(|c| Uuid::from_bytes(*c)).collect()
}

fn encode_update(update: &ContextUpdate) -> Result<Vec<u8>> {
    bincode::serde::encode_to_vec(update, bincode::config::standard())
        .map_err(|e| anyhow::anyhow!("Failed to serialize update: {}", e))
}

/// Session blob as written by this version: everything but the updates.
fn session_blob(session: &ActiveSession) -> Result<Vec<u8>> {
    serde_json::to_vec(&session.serializable_view().without_incremental_updates())
        .map_err(|e| anyhow::anyhow!("Failed to serialize session: {}", e))
}

/// Write `session` with its rows, update index and meta in one
/// `WriteBatch`. The caller holds the session's lock.
///
/// Rows are written for `explicit` updates and for every session update
/// the stored index does not list yet (all of them for a session saved by
/// an older version). Only the `removed` updates lose their rows and index
/// entries: an update the snapshot merely lacks stays, because the
/// snapshot may predate it (an evicted or concurrent copy of the session).
/// The index keeps its stored order and appends new ids in session order.
/// Returns the number of rows written.
fn persist_session(
    db: &DB,
    session: &ActiveSession,
    blob: &[u8],
    explicit: &[ContextUpdate],
    removed: &[Uuid],
) -> Result<usize> {
    let session_id = session.id();
    let index_key = update_index_key(session_id);
    let old_index = db.get(index_key.as_bytes())?;
    let stored_order: Vec<Uuid> = old_index.as_deref().map(decode_ids).unwrap_or_default();
    let stored: HashSet<Uuid> = stored_order.iter().copied().collect();
    let removed: HashSet<Uuid> = removed.iter().copied().collect();
    let explicit_ids: HashSet<Uuid> = explicit.iter().map(|u| u.id).collect();

    let mut batch = WriteBatch::default();
    let mut rows = 0;
    for update in explicit.iter().filter(|u| !removed.contains(&u.id)) {
        batch.put(
            update_key(session_id, update.id).as_bytes(),
            encode_update(update)?,
        );
        rows += 1;
    }
    for update in session.incremental_updates.iter() {
        if !stored.contains(&update.id)
            && !explicit_ids.contains(&update.id)
            && !removed.contains(&update.id)
        {
            batch.put(
                update_key(session_id, update.id).as_bytes(),
                encode_update(update)?,
            );
            rows += 1;
        }
    }
    for id in &removed {
        batch.delete(update_key(session_id, *id).as_bytes());
    }

    let mut indexed: HashSet<Uuid> = HashSet::with_capacity(stored.len());
    let index_ids: Vec<Uuid> = stored_order
        .iter()
        .copied()
        .chain(session.incremental_updates.iter().map(|u| u.id))
        .chain(explicit.iter().map(|u| u.id))
        .filter(|id| !removed.contains(id) && indexed.insert(*id))
        .collect();
    let index = encode_ids(index_ids.iter());
    if old_index.as_deref() != Some(index.as_slice()) {
        batch.put(index_key.as_bytes(), &index);
    }
    let meta = serde_json::to_vec(&SessionSummary::from_session(session))?;
    batch.put(meta_key(session_id).as_bytes(), &meta);
    batch.put(session_key(session_id).as_bytes(), blob);

    db.write(batch)?;
    Ok(rows)
}

/// A session read from disk.
struct LoadedSession {
    session: ActiveSession,
    /// Raw blob when it came from an older writer (updates embedded, or no
    /// update index yet) and still needs its rows/index/meta backfilled.
    legacy_blob: Option<Vec<u8>>,
}

/// Read blob, index and rows from one snapshot so a concurrent save cannot
/// be observed half-applied.
fn read_session(db: &DB, session_id: Uuid) -> Result<Option<LoadedSession>> {
    let snapshot = db.snapshot();
    let Some(blob) = snapshot.get(session_key(session_id).as_bytes())? else {
        return Ok(None);
    };
    let mut session: ActiveSession = serde_json::from_slice(&blob)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize session: {}", e))?;
    let index = snapshot.get(update_index_key(session_id).as_bytes())?;

    // An embedded list wins: it was written by a version that keeps the
    // blob authoritative (possibly after this one, on a downgrade).
    let Some(index) = index.filter(|_| session.incremental_updates.is_empty()) else {
        return Ok(Some(LoadedSession {
            session,
            legacy_blob: Some(blob),
        }));
    };

    let ids = decode_ids(&index);
    let keys: Vec<String> = ids.iter().map(|id| update_key(session_id, *id)).collect();
    let mut updates = Vec::with_capacity(ids.len());
    for (id, value) in ids.iter().zip(snapshot.multi_get(&keys)) {
        match value?.as_deref().map(decode_update) {
            Some(Some(update)) => updates.push(update),
            Some(None) => warn!("Session {}: undecodable update row {}", session_id, id),
            None => warn!("Session {}: update row {} is missing", session_id, id),
        }
    }
    session.set_incremental_updates(updates);
    Ok(Some(LoadedSession {
        session,
        legacy_blob: None,
    }))
}

/// Rewrite a legacy-format session in the current format, unless another
/// writer replaced the blob since it was read.
fn backfill_legacy(
    db: &DB,
    locks: &SessionLocks,
    session: &ActiveSession,
    legacy_blob: &[u8],
) -> Result<()> {
    let session_id = session.id();
    let blob = session_blob(session)?;
    let _guard = locks.lock(session_id);
    if db.get(session_key(session_id).as_bytes())?.as_deref() != Some(legacy_blob) {
        return Ok(());
    }
    // The embedded list is authoritative here (see `read_session`): index
    // entries it lacks are updates its writer removed.
    let embedded: HashSet<Uuid> = session.incremental_updates.iter().map(|u| u.id).collect();
    let removed: Vec<Uuid> = db
        .get(update_index_key(session_id).as_bytes())?
        .as_deref()
        .map(decode_ids)
        .unwrap_or_default()
        .into_iter()
        .filter(|id| !embedded.contains(id))
        .collect();
    let rows = persist_session(db, session, &blob, &[], &removed)?;
    info!(
        "RealRocksDBStorage: Migrated session {} to row-backed updates ({} rows backfilled, blob {} -> {} bytes)",
        session_id,
        rows,
        legacy_blob.len(),
        blob.len()
    );
    Ok(())
}

/// Ids of every `session:{id}` blob, skipping over each session's rows
/// with a seek instead of walking them.
fn list_session_ids(db: &DB) -> Result<Vec<Uuid>> {
    let mut sessions = Vec::new();
    let mut it = db.raw_iterator_opt(prefix_read_opts(SESSION_PREFIX.as_bytes()));
    it.seek(SESSION_PREFIX.as_bytes());
    while let Some(key) = it.key() {
        if key.len() < SESSION_KEY_LEN {
            it.next();
            continue;
        }
        if key.len() == SESSION_KEY_LEN
            && let Ok(id) = std::str::from_utf8(&key[SESSION_PREFIX.len()..])
                .map_err(|_| ())
                .and_then(|s| Uuid::parse_str(s).map_err(|_| ()))
        {
            sessions.push(id);
        }
        // `session:{id};` sorts right after every `session:{id}:...` row.
        let mut next = key[..SESSION_KEY_LEN].to_vec();
        next.push(b';');
        it.seek(&next);
    }
    it.status()?;
    Ok(sessions)
}

/// Element count of a JSON array without materialising it.
struct SeqLen(usize);

impl<'de> Deserialize<'de> for SeqLen {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Counter;
        impl<'de> serde::de::Visitor<'de> for Counter {
            type Value = SeqLen;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a sequence")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<SeqLen, A::Error> {
                let mut n = 0;
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    n += 1;
                }
                Ok(SeqLen(n))
            }
        }
        deserializer.deserialize_seq(Counter)
    }
}

/// The listing fields of a session blob.
#[derive(Deserialize)]
struct BlobHead {
    id: Uuid,
    name: Option<String>,
    description: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    last_updated: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    incremental_updates: Option<SeqLen>,
}

/// Summary for a session without a meta record (saved by an older
/// version), stored as its meta record for next time.
fn summarize_without_meta(
    db: &DB,
    locks: &SessionLocks,
    session_id: Uuid,
) -> Result<Option<SessionSummary>> {
    let _guard = locks.lock(session_id);
    if let Some(meta) = db.get(meta_key(session_id).as_bytes())?
        && let Ok(summary) = serde_json::from_slice(&meta)
    {
        return Ok(Some(summary));
    }
    let Some(blob) = db.get(session_key(session_id).as_bytes())? else {
        return Ok(None);
    };
    let head: BlobHead = serde_json::from_slice(&blob)
        .map_err(|e| anyhow::anyhow!("Failed to read session {} header: {}", session_id, e))?;
    let embedded = head.incremental_updates.map_or(0, |n| n.0);
    let update_count = if embedded > 0 {
        embedded
    } else {
        db.get(update_index_key(session_id).as_bytes())?
            .map_or(0, |index| index.len() / 16)
    };
    let summary = SessionSummary {
        id: head.id,
        name: head.name,
        description: head.description,
        created_at: head.created_at,
        last_updated: head.last_updated,
        update_count,
    };
    db.put(
        meta_key(session_id).as_bytes(),
        serde_json::to_vec(&summary)?,
    )?;
    Ok(Some(summary))
}

impl RealRocksDBStorage {
    /// Save session to RocksDB.
    ///
    /// The blob is written without `incremental_updates`; every update the
    /// stored index does not list yet gets its row in the same batch, and
    /// rows of updates the session no longer contains are deleted.
    pub async fn save_session(&self, session: &ActiveSession) -> Result<()> {
        self.persist(session, Vec::new(), Vec::new()).await
    }

    /// Save session blob and context updates in a single atomic WriteBatch.
    ///
    /// `updates` are written (or rewritten) as rows; any other session
    /// update without a row is written as well, so the session never
    /// references a missing row.
    pub async fn save_session_with_updates(
        &self,
        session: &ActiveSession,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        if session.id() != session_id {
            return Err(anyhow::anyhow!(
                "Session id mismatch: blob {} vs updates for {}",
                session.id(),
                session_id
            ));
        }
        self.save_session_with_changes(session, session_id, updates, Vec::new())
            .await
    }

    /// [`Self::save_session_with_updates`] that also deletes the rows (and
    /// index entries) of the `removed` updates. Saves never delete rows
    /// otherwise; see `persist_session`.
    pub async fn save_session_with_changes(
        &self,
        session: &ActiveSession,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
        removed: Vec<Uuid>,
    ) -> Result<()> {
        if session.id() != session_id {
            return Err(anyhow::anyhow!(
                "Session id mismatch: blob {} vs updates for {}",
                session.id(),
                session_id
            ));
        }
        self.persist(session, updates, removed).await
    }

    async fn persist(
        &self,
        session: &ActiveSession,
        updates: Vec<ContextUpdate>,
        removed: Vec<Uuid>,
    ) -> Result<()> {
        let db = self.db.clone();
        let locks = self.session_locks.clone();
        let session = session.clone();

        blocking(move || {
            let blob = session_blob(&session)?;
            let _guard = locks.lock(session.id());
            let rows = persist_session(&db, &session, &blob, &updates, &removed)?;
            debug!(
                "RealRocksDBStorage: Saved session {} ({} bytes, {} update rows written)",
                session.id(),
                blob.len(),
                rows
            );
            Ok(())
        })
        .await
    }

    /// Load session from RocksDB, rebuilding `incremental_updates` from the
    /// update rows. Sessions written by older versions (updates embedded in
    /// the blob) load as-is and are migrated to the row format on the way.
    pub async fn load_session(&self, session_id: Uuid) -> Result<ActiveSession> {
        let db = self.db.clone();
        let locks = self.session_locks.clone();

        blocking(move || {
            let Some(loaded) = read_session(&db, session_id)? else {
                debug!("RealRocksDBStorage: Session {} not found", session_id);
                return Err(session_not_found(session_id));
            };
            if let Some(legacy_blob) = &loaded.legacy_blob
                && let Err(e) = backfill_legacy(&db, &locks, &loaded.session, legacy_blob)
            {
                warn!(
                    "RealRocksDBStorage: Could not migrate session {} to row-backed updates: {:#}",
                    session_id, e
                );
            }
            debug!(
                "RealRocksDBStorage: Loaded session {} ({} updates)",
                session_id,
                loaded.session.incremental_updates.len()
            );
            Ok(loaded.session)
        })
        .await
    }

    /// Save checkpoint to RocksDB
    pub async fn save_checkpoint(&self, checkpoint: &SessionCheckpoint) -> Result<()> {
        let db = self.db.clone();
        let checkpoint = checkpoint.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut checkpoint_data = CHECKPOINT_V3.to_vec();
            checkpoint_data.extend(
                bincode::serde::encode_to_vec(&checkpoint, bincode::config::standard())
                    .map_err(|e| anyhow::anyhow!("Failed to serialize checkpoint: {}", e))?,
            );

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
                    let checkpoint = decode_checkpoint(&data)
                        .ok_or_else(|| anyhow::anyhow!("Failed to deserialize checkpoint"))?;

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
            for item in scan_prefix(&db, b"checkpoint:") {
                let (_, value) = item?;
                if let Some(checkpoint) = decode_checkpoint(&value) {
                    checkpoints.push(checkpoint);
                }
            }

            info!(
                "RealRocksDBStorage: Listed {} checkpoints",
                checkpoints.len()
            );
            Ok(checkpoints)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Write update rows for a session.
    ///
    /// If the session is stored in the row-backed format, updates it does
    /// not contain yet are appended to it (they show up on the next load).
    /// For a session saved by an older version only the rows are written,
    /// as before.
    pub async fn batch_save_updates(
        &self,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let db = self.db.clone();
        let locks = self.session_locks.clone();

        blocking(move || {
            let mut batch = WriteBatch::default();
            for update in &updates {
                batch.put(
                    update_key(session_id, update.id).as_bytes(),
                    encode_update(update)?,
                );
            }

            let _guard = locks.lock(session_id);
            let index_key = update_index_key(session_id);
            if let Some(index) = db.get(index_key.as_bytes())? {
                let mut ids = decode_ids(&index);
                let mut known: HashSet<Uuid> = ids.iter().copied().collect();
                let before = ids.len();
                ids.extend(updates.iter().map(|u| u.id).filter(|id| known.insert(*id)));
                if ids.len() != before {
                    batch.put(index_key.as_bytes(), encode_ids(ids.iter()));
                    // Keep the listing count in step with the index.
                    if let Some(meta) = db.get(meta_key(session_id).as_bytes())?
                        && let Ok(mut summary) = serde_json::from_slice::<SessionSummary>(&meta)
                    {
                        summary.update_count = ids.len();
                        batch.put(
                            meta_key(session_id).as_bytes(),
                            serde_json::to_vec(&summary)?,
                        );
                    }
                }
            }
            db.write(batch)?;

            debug!(
                "RealRocksDBStorage: Batch saved {} updates for session {}",
                updates.len(),
                session_id
            );
            Ok(())
        })
        .await
    }

    /// List all session IDs
    pub async fn list_sessions(&self) -> Result<Vec<Uuid>> {
        let db = self.db.clone();
        let sessions = blocking(move || list_session_ids(&db)).await?;
        debug!("RealRocksDBStorage: Found {} sessions", sessions.len());
        Ok(sessions)
    }

    /// Metadata of every stored session from the `session_meta:` records;
    /// sessions saved before those existed are summarised from their blob
    /// header once and get a meta record.
    pub async fn list_session_summaries(&self) -> Result<Vec<SessionSummary>> {
        let db = self.db.clone();
        let locks = self.session_locks.clone();

        blocking(move || {
            let mut metas: HashMap<Uuid, SessionSummary> = HashMap::new();
            for item in scan_prefix(&db, META_PREFIX.as_bytes()) {
                let (_, value) = item?;
                if let Ok(summary) = serde_json::from_slice::<SessionSummary>(&value) {
                    metas.insert(summary.id, summary);
                }
            }

            let ids = list_session_ids(&db)?;
            let mut summaries = Vec::with_capacity(ids.len());
            for session_id in ids {
                if let Some(summary) = metas.remove(&session_id) {
                    summaries.push(summary);
                    continue;
                }
                match summarize_without_meta(&db, &locks, session_id) {
                    Ok(Some(summary)) => summaries.push(summary),
                    Ok(None) => {}
                    Err(e) => warn!("Skipping session {} in listing: {:#}", session_id, e),
                }
            }
            Ok(summaries)
        })
        .await
    }

    /// Delete a session and everything stored under its id: blob, update
    /// rows and index, meta, messages, embeddings, entities, relationships
    /// and workspace memberships.
    pub async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        let db = self.db.clone();
        let locks = self.session_locks.clone();

        blocking(move || {
            let _guard = locks.lock(session_id);
            let mut batch = WriteBatch::default();

            batch.delete(session_key(session_id).as_bytes());
            batch.delete(update_index_key(session_id).as_bytes());
            batch.delete(meta_key(session_id).as_bytes());
            batch.delete(message_next_key(session_id).as_bytes());

            let mut deleted = 0usize;
            for prefix in [
                format!("session:{}:", session_id),
                format!("message:{}:", session_id),
                format!("message_turn:{}:", session_id),
                format!("embedding:{}:", session_id),
                format!("entity:{}:", session_id),
                format!("relationship:{}:", session_id),
            ] {
                for key in keys_with_prefix(&db, prefix.as_bytes())? {
                    batch.delete(&key);
                    deleted += 1;
                }
            }

            for item in scan_prefix(&db, b"ws_session:") {
                let (key, value) = item?;
                if let Ok((membership, _)) = bincode::serde::decode_from_slice::<
                    StoredWorkspaceSession,
                    _,
                >(&value, bincode::config::standard())
                    && membership.session_id == session_id
                {
                    batch.delete(&key);
                }
            }

            db.write(batch)?;
            info!(
                "RealRocksDBStorage: Deleted session {} and {} related records",
                session_id, deleted
            );
            Ok(())
        })
        .await
    }

    /// Delete all entity and relationship records of a session.
    pub async fn clear_session_entities(&self, session_id: Uuid) -> Result<()> {
        let db = self.db.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            for prefix in [
                format!("entity:{}:", session_id),
                format!("relationship:{}:", session_id),
            ] {
                for key in keys_with_prefix(&db, prefix.as_bytes())? {
                    batch.delete(&key);
                }
            }
            db.write(batch)?;
            Ok(())
        })
        .await
    }

    /// Check if session exists
    pub async fn session_exists(&self, session_id: Uuid) -> Result<bool> {
        let db = self.db.clone();
        let key = session_key(session_id);
        blocking(move || Ok(db.get(key.as_bytes())?.is_some())).await
    }

    /// Load the updates of a session in session order. For a session saved
    /// by an older version (no update index), every stored row is returned.
    pub async fn load_session_updates(&self, session_id: Uuid) -> Result<Vec<ContextUpdate>> {
        let db = self.db.clone();

        let updates = blocking(move || {
            let snapshot = db.snapshot();
            if let Some(index) = snapshot.get(update_index_key(session_id).as_bytes())? {
                let keys: Vec<String> = decode_ids(&index)
                    .into_iter()
                    .map(|id| update_key(session_id, id))
                    .collect();
                let mut updates = Vec::with_capacity(keys.len());
                for value in snapshot.multi_get(&keys) {
                    if let Some(update) = value?.as_deref().and_then(decode_update) {
                        updates.push(update);
                    }
                }
                return Ok(updates);
            }

            let prefix = format!("session:{}:update:", session_id);
            let mut updates = Vec::new();
            for item in scan_prefix(&db, prefix.as_bytes()) {
                let (_, value) = item?;
                if let Some(update) = decode_update(&value) {
                    updates.push(update);
                }
            }
            Ok(updates)
        })
        .await?;

        debug!(
            "RealRocksDBStorage: Loaded {} updates for session {}",
            updates.len(),
            session_id
        );
        Ok(updates)
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

    async fn list_session_summaries(&self) -> Result<Vec<SessionSummary>> {
        RealRocksDBStorage::list_session_summaries(self).await
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

    async fn save_session_with_changes(
        &self,
        session: &ActiveSession,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
        removed: Vec<Uuid>,
    ) -> Result<()> {
        RealRocksDBStorage::save_session_with_changes(self, session, session_id, updates, removed)
            .await
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
        RealRocksDBStorage::clear_session_entities(self, session_id).await
    }
}
