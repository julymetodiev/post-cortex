// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The stored form of a session (data version 3; row layouts in
//! [`crate::rocksdb_storage::rows`]): a head row (metadata), the records
//! under ordinals in session order (with an id -> ordinal key per record
//! and the next ordinal), the entity graph as rows
//! ([`crate::rocksdb_storage::graph_rows`]), one key per vectorized record
//! and the listing record. A save writes what its [`SessionChanges`] name,
//! in one `WriteBatch`; a load reads one snapshot.

use std::collections::{HashMap, HashSet};

use crate::port_error::Result;
use crate::rocksdb_storage::checkpoints::{
    checkpoint_index_prefix, checkpoint_key, indexed_checkpoint,
};
use crate::rocksdb_storage::core::{keys_with_prefix, scan_snapshot_prefix};
use crate::rocksdb_storage::graph_rows;
use crate::rocksdb_storage::messages::{message_authors_key, message_count_key, message_next_key};
use crate::rocksdb_storage::rows::{
    decode_head, decode_update, encode_head, encode_meta, encode_update,
};
use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::session::active_session::{ActiveSession, StoredSession};
use post_cortex_ports::error::corrupt;
use post_cortex_ports::storage::{SessionChanges, SessionSummary};
use rocksdb::{DB, WriteBatch};
use uuid::Uuid;

pub const SESSION_PREFIX: &str = "session:";

pub const META_PREFIX: &str = "session_meta:";

/// `session:` + hyphenated UUID.
pub const SESSION_KEY_LEN: usize = SESSION_PREFIX.len() + 36;

pub fn session_key(session_id: Uuid) -> String {
    format!("session:{}", session_id)
}

pub fn meta_key(session_id: Uuid) -> String {
    format!("session_meta:{}", session_id)
}

fn record_prefix(session_id: Uuid) -> String {
    format!("session:{session_id}:rec:")
}

/// Ordinals in hex of fixed width, so key order is session order.
fn record_key(session_id: Uuid, ordinal: u64) -> String {
    format!("session:{session_id}:rec:{ordinal:016x}")
}

fn ordinal_key(session_id: Uuid, update_id: Uuid) -> String {
    format!("session:{session_id}:recid:{update_id}")
}

fn next_key(session_id: Uuid) -> String {
    format!("session:{session_id}:next")
}

fn vectorized_prefix(session_id: Uuid) -> String {
    format!("session:{session_id}:vec:")
}

fn vectorized_key(session_id: Uuid, update_id: Uuid) -> String {
    format!("{}{update_id}", vectorized_prefix(session_id))
}

fn decode_ordinal(key: &str, row: &[u8]) -> Result<u64> {
    let bytes: [u8; 8] = row
        .try_into()
        .map_err(|_| corrupt("record ordinal", key, "not 8 bytes"))?;
    Ok(u64::from_be_bytes(bytes))
}

/// What of a session [`queue_session_data_delete`] deletes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SessionData {
    /// Records, graph, vectors and messages: what a replacing import
    /// rewrites. Checkpoints and workspace memberships stay.
    Content,
    /// Everything stored under the session, checkpoints included.
    All,
}

/// Queue deleting `what` of `session_id` into `batch`; returns the number
/// of rows found under the session's prefixes. Workspace memberships are
/// the caller's (they are keyed by workspace).
pub fn queue_session_data_delete(
    db: &DB,
    batch: &mut WriteBatch,
    session_id: Uuid,
    what: SessionData,
) -> Result<usize> {
    batch.delete(session_key(session_id).as_bytes());
    batch.delete(meta_key(session_id).as_bytes());
    batch.delete(message_next_key(session_id).as_bytes());
    batch.delete(message_count_key(session_id).as_bytes());
    batch.delete(message_authors_key(session_id).as_bytes());

    let mut deleted = 0usize;
    for prefix in [
        format!("session:{}:", session_id),
        format!("message:{}:", session_id),
        format!("message_turn:{}:", session_id),
        format!("embedding:{}:", session_id),
    ] {
        for key in keys_with_prefix(db, prefix.as_bytes())? {
            batch.delete(&key);
            deleted += 1;
        }
    }
    if what == SessionData::All {
        let checkpoints = checkpoint_index_prefix(session_id);
        for key in keys_with_prefix(db, checkpoints.as_bytes())? {
            let id = indexed_checkpoint(&checkpoints, &key)?;
            batch.delete(checkpoint_key(id).as_bytes());
            batch.delete(&key);
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// The positions in `session` of the records `ids`, searched from the end
/// (new records are the newest). Ids not in the session are left out.
fn positions(session: &ActiveSession, ids: &HashSet<Uuid>) -> HashMap<Uuid, usize> {
    let mut found = HashMap::with_capacity(ids.len());
    for (i, update) in session.records().iter().enumerate().rev() {
        if found.len() == ids.len() {
            break;
        }
        if ids.contains(&update.id) {
            found.insert(update.id, i);
        }
    }
    found
}

/// Queue the save of `session` with `changes` into `batch`: the head, the
/// rows of `changes.updates` (a record already stored keeps its ordinal,
/// new ones get the next ordinals in session order), the deletes of the
/// removed records, the graph rows the records changed (or the whole
/// graph), the vectorized keys and the listing record. The caller holds
/// the session's lock and writes the batch. Returns the record rows
/// written.
pub fn queue_save(
    db: &DB,
    batch: &mut WriteBatch,
    session: &ActiveSession,
    changes: &SessionChanges,
) -> Result<usize> {
    let session_id = session.id();
    let removed: HashSet<Uuid> = changes.removed.iter().copied().collect();
    let writes: Vec<&ContextUpdate> = changes
        .updates
        .iter()
        .filter(|u| !removed.contains(&u.id))
        .collect();

    let ordinal_keys: Vec<String> = writes
        .iter()
        .map(|u| ordinal_key(session_id, u.id))
        .collect();
    let mut fresh = Vec::new();
    for ((update, key), stored) in writes
        .iter()
        .zip(&ordinal_keys)
        .zip(db.multi_get(&ordinal_keys))
    {
        match stored? {
            Some(row) => {
                let ordinal = decode_ordinal(key, &row)?;
                batch.put(
                    record_key(session_id, ordinal).as_bytes(),
                    encode_update(update)?,
                );
            }
            None => fresh.push(*update),
        }
    }
    if !fresh.is_empty() {
        let ids: HashSet<Uuid> = fresh.iter().map(|u| u.id).collect();
        let at = positions(session, &ids);
        fresh.sort_by_key(|u| at.get(&u.id).copied().unwrap_or(usize::MAX));
        let next = next_key(session_id);
        let mut ordinal = match db.get(next.as_bytes())? {
            Some(row) => decode_ordinal(&next, &row)?,
            None => 0,
        };
        for update in fresh {
            batch.put(
                record_key(session_id, ordinal).as_bytes(),
                encode_update(update)?,
            );
            batch.put(
                ordinal_key(session_id, update.id).as_bytes(),
                ordinal.to_be_bytes(),
            );
            ordinal += 1;
        }
        batch.put(next.as_bytes(), ordinal.to_be_bytes());
    }

    let removed_keys: Vec<String> = changes
        .removed
        .iter()
        .map(|id| ordinal_key(session_id, *id))
        .collect();
    for ((id, key), stored) in changes
        .removed
        .iter()
        .zip(&removed_keys)
        .zip(db.multi_get(&removed_keys))
    {
        if let Some(row) = stored? {
            batch.delete(record_key(session_id, decode_ordinal(key, &row)?).as_bytes());
            batch.delete(key.as_bytes());
        }
        batch.delete(vectorized_key(session_id, *id).as_bytes());
    }

    if changes.whole_graph {
        graph_rows::queue_whole(db, batch, session_id, session.entity_graph())?;
    } else {
        graph_rows::queue_changed(
            batch,
            session_id,
            session.entity_graph(),
            writes.iter().copied(),
        )?;
    }
    for id in &changes.vectorized {
        batch.put(vectorized_key(session_id, *id).as_bytes(), []);
    }
    for id in &changes.unvectorized {
        batch.delete(vectorized_key(session_id, *id).as_bytes());
    }
    queue_head(batch, session)?;
    Ok(writes.len())
}

/// Queue all of `session` for a session with nothing stored under it (new,
/// replaced or migrated): its records under ordinals 0.., its whole graph
/// and the embedded records `vectorized`.
pub fn queue_fresh(
    batch: &mut WriteBatch,
    session: &ActiveSession,
    vectorized: &[Uuid],
) -> Result<()> {
    let session_id = session.id();
    let mut ordinal = 0u64;
    for update in session.records() {
        batch.put(
            record_key(session_id, ordinal).as_bytes(),
            encode_update(update)?,
        );
        batch.put(
            ordinal_key(session_id, update.id).as_bytes(),
            ordinal.to_be_bytes(),
        );
        ordinal += 1;
    }
    batch.put(next_key(session_id).as_bytes(), ordinal.to_be_bytes());
    graph_rows::queue_all(batch, session_id, session.entity_graph())?;
    for id in vectorized {
        batch.put(vectorized_key(session_id, *id).as_bytes(), []);
    }
    queue_head(batch, session)
}

/// The head row and the listing record of `session`.
fn queue_head(batch: &mut WriteBatch, session: &ActiveSession) -> Result<()> {
    let session_id = session.id();
    batch.put(session_key(session_id).as_bytes(), encode_head(session)?);
    batch.put(
        meta_key(session_id).as_bytes(),
        encode_meta(&SessionSummary::from_session(session))?,
    );
    Ok(())
}

/// Read head, records, graph and vectorized ids from one snapshot so a
/// concurrent save cannot be observed half-applied. An unreadable row is
/// `Corrupt`.
pub fn read_session(db: &DB, session_id: Uuid) -> Result<Option<StoredSession>> {
    let snapshot = db.snapshot();
    let key = session_key(session_id);
    let Some(row) = snapshot.get(key.as_bytes())? else {
        return Ok(None);
    };
    let head = decode_head(&key, &row)?;

    let mut records = Vec::new();
    for item in scan_snapshot_prefix(&snapshot, record_prefix(session_id).as_bytes()) {
        let (key, row) = item?;
        records.push(decode_update(&String::from_utf8_lossy(&key), &row)?);
    }
    let graph = graph_rows::read_graph(&snapshot, session_id, head.last_updated)?;
    let prefix = vectorized_prefix(session_id);
    let mut vectorized = Vec::new();
    for item in scan_snapshot_prefix(&snapshot, prefix.as_bytes()) {
        let (key, _) = item?;
        let key = String::from_utf8_lossy(&key);
        let id = key
            .strip_prefix(prefix.as_str())
            .and_then(|id| Uuid::parse_str(id).ok())
            .ok_or_else(|| corrupt("vectorized record", key.as_ref(), "no record id"))?;
        vectorized.push(id);
    }
    let session = ActiveSession::from_records(head.metadata, head.last_updated, records, graph);
    Ok(Some(StoredSession::new(session, vectorized)))
}
