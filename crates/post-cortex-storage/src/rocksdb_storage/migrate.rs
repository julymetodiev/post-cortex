// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Versioned data migrations, run once when the database is opened.
//!
//! The data version is stored at [`DATA_VERSION_KEY`] (ASCII decimal; a
//! database without it is version 0, i.e. written by 0.5.0 or earlier, or
//! empty). Opening runs every step above the stored version, then records
//! the new version; a database already at [`DATA_VERSION`] costs one point
//! read, and one from a newer build is refused. Migrations run at open
//! because RocksDB allows one process per database: nothing else can read
//! or write while they run, no operator step can be forgotten, and the read
//! path never has to know a legacy layout.
//!
//! Step 1 (0.5.0 layouts to the marked rows of [`crate::rocksdb_storage::rows`]):
//! - each session in one batch: blob, update rows, update index and
//!   listing record. A blob with embedded updates (written before update
//!   rows) is authoritative: its updates get rows and the index, index
//!   entries it lacks lose their rows (as on the old read path). Listing
//!   records are made for sessions without one.
//! - checkpoints, workspaces, memberships and source references are
//!   re-encoded; the checkpoint-by-session index and the source-by-file
//!   index (now holding the entry id) are (re)built.
//! - the `entity:` and `relationship:` rows are deleted: the session blob
//!   holds the graph and is its only source.
//!
//! Step 2 (data version 1 to 2): embedding, message, card and radar
//! dismissal rows, stored as unmarked bincode until then, get their
//! markers ([`crate::rocksdb_storage::rows`]).
//!
//! Step 3 (data version 2 to 3): each session in one batch from the
//! layout of [`crate::rocksdb_storage::v2_layout`] (a blob with the graph
//! and the vectorized ids, an update index, rows keyed by update id) to
//! the rows of [`crate::rocksdb_storage::session_rows`]: a head, records
//! under ordinals in index order, the graph as entity and edge rows and a
//! key per vectorized record. The old rows and the index are deleted.
//!
//! Step 4 (data version 3 to 4; docs/memory.md, document keys and pages):
//! every document's latest version gets its place in its kind's order and
//! is unkeyed, for memory to key at start; the `doc_kind` rows go.
//!
//! Step 5 (data version 4 to 5): every workspace row gets its slug (the
//! oldest first; [`post_cortex_core::slug::workspace_slug`]) and the
//! layout [`WORKSPACE_V5`].
//!
//! Step 6 (data version 5 to 6): every workspace row gets the layout
//! [`WORKSPACE_V6`], the remote reranker forbidden (a person allows it per
//! workspace).
//!
//! A row the migration cannot read is left as it is (and logged); reading
//! it later fails with `Corrupt` for its record only. Every step is
//! idempotent, so a migration interrupted half way is finished by the next
//! open.

use crate::port_error::Result;
use rocksdb::{DB, WriteBatch};
use std::collections::HashSet;
use tracing::{info, warn};
use uuid::Uuid;

use crate::rocksdb_storage::message_rows::MessageRowV1;
use post_cortex_core::core::card::Card;
use post_cortex_core::core::dismissal::RadarDismissal;
use post_cortex_core::session::active_session::StoredSession;
use serde::{Serialize, de::DeserializeOwned};

use crate::rocksdb_storage::checkpoints::{CHECKPOINT_PREFIX, checkpoint_index_key};
use crate::rocksdb_storage::core::{keys_with_prefix, scan_prefix};
use crate::rocksdb_storage::freshness::{SOURCE_REF_PREFIX, put_file_index};
use crate::rocksdb_storage::legacy;
use crate::rocksdb_storage::legacy::{WORKSPACE_V4, WORKSPACE_V5};
use crate::rocksdb_storage::rows::{
    self, CHECKPOINT_V4, MEMBERSHIP_V1, UPDATE_V4, WORKSPACE_V6, encode_checkpoint,
    encode_membership, encode_meta, encode_update,
};
use crate::rocksdb_storage::session_listing::list_session_ids;
use crate::rocksdb_storage::session_rows::{meta_key, queue_fresh, session_key};
use crate::rocksdb_storage::source_rows::{SOURCE_V2, SourceRow};
use crate::rocksdb_storage::types::StoredEmbedding;
use crate::rocksdb_storage::v2_layout::{
    SESSION_V1, decode_ids, decode_session, encode_ids, encode_session, update_index_key,
    update_key,
};
use post_cortex_ports::storage::SessionSummary;
use workspace_steps::{to_version_5, to_version_6};

mod workspace_steps;

/// Data version this build writes.
pub const DATA_VERSION: u32 = 6;

/// Where the data version is stored.
pub const DATA_VERSION_KEY: &[u8] = b"pcx_meta:data_version";

/// Markers of the indexes data version 0 built at open.
const OLD_INDEX_MARKERS: [&[u8]; 2] = [
    b"index_version:checkpoint_by_session",
    b"index_version:source_by_file",
];

/// Puts per batch outside sessions.
const BATCH_ROWS: usize = 1024;

/// The stored data version.
pub fn data_version(db: &DB) -> Result<u32> {
    let Some(raw) = db.get(DATA_VERSION_KEY)? else {
        return Ok(0);
    };
    std::str::from_utf8(&raw)
        .ok()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| {
            post_cortex_ports::error::corrupt("data version", "pcx_meta", format!("{raw:?}"))
        })
        .map_err(Into::into)
}

/// Bring the database to [`DATA_VERSION`].
pub fn migrate(db: &DB) -> Result<()> {
    let version = data_version(db)?;
    if version > DATA_VERSION {
        return Err(post_cortex_ports::PortError::unavailable(format!(
            "RocksDB data version {version} was written by a newer post-cortex; this build \
             reads up to {DATA_VERSION}"
        ))
        .into());
    }
    if version < 1 {
        let report = to_version_1(db)?;
        info!("RealRocksDBStorage: migrated data to version 1 ({report})");
        db.put(DATA_VERSION_KEY, b"1")?;
    }
    if version < 2 {
        let report = to_version_2(db)?;
        info!("RealRocksDBStorage: migrated data to version 2 ({report})");
        db.put(DATA_VERSION_KEY, b"2")?;
    }
    if version < 3 {
        let mut report = Report::default();
        for session_id in list_session_ids(db)? {
            to_version_3(db, session_id, &mut report)?;
        }
        info!("RealRocksDBStorage: migrated data to version 3 ({report})");
        db.put(DATA_VERSION_KEY, b"3")?;
    }
    if version < 4 {
        let placed = crate::rocksdb_storage::documents::order_existing(db)?;
        info!("RealRocksDBStorage: migrated data to version 4 ({placed} documents ordered)");
        db.put(DATA_VERSION_KEY, b"4")?;
    }
    if version < 5 {
        let slugged = to_version_5(db)?;
        info!("RealRocksDBStorage: migrated data to version 5 ({slugged} workspaces got a slug)");
        db.put(DATA_VERSION_KEY, b"5")?;
    }
    if version < 6 {
        let rewritten = to_version_6(db)?;
        info!(
            "RealRocksDBStorage: migrated data to version 6 ({rewritten} workspaces, remote \
             reranker forbidden)"
        );
        db.put(DATA_VERSION_KEY, b"6")?;
    }
    Ok(())
}

/// Step 3 for one session, in one batch; a session already in the version
/// 3 layout (its head is not a version 2 blob) is left as it is, and so is
/// one with a record row that is missing or unreadable: it stays in the old
/// layout, which the reader does not know, so loading it is `Corrupt`.
fn to_version_3(db: &DB, session_id: Uuid, report: &mut Report) -> Result<()> {
    let key = session_key(session_id);
    let Some(blob) = db.get(key.as_bytes())? else {
        return Ok(());
    };
    if !blob.starts_with(SESSION_V1) {
        return Ok(());
    }
    let StoredSession {
        mut session,
        vectorized,
    } = match decode_session(&key, &blob) {
        Ok(stored) => stored,
        Err(e) => {
            warn!("RealRocksDBStorage: migration left unreadable session {session_id}: {e}");
            report.unreadable += 1;
            return Ok(());
        }
    };
    let index_key = update_index_key(session_id);
    let ids: Vec<Uuid> = match db.get(index_key.as_bytes())? {
        Some(index) => decode_ids(&index_key, &index)?,
        None => Vec::new(),
    };
    let keys: Vec<String> = ids.iter().map(|id| update_key(session_id, *id)).collect();
    let mut updates = Vec::with_capacity(keys.len());
    let mut batch = WriteBatch::default();
    for (row_key, value) in keys.iter().zip(db.multi_get(&keys)) {
        let update = match value? {
            Some(row) => rows::decode_update(row_key, &row).map_err(|e| e.to_string()),
            None => Err("indexed but missing".to_string()),
        };
        match update {
            Ok(update) => updates.push(update),
            Err(e) => {
                warn!(
                    "RealRocksDBStorage: migration left session {session_id} in the old \
                     layout: update row {row_key}: {e}"
                );
                report.unreadable += 1;
                return Ok(());
            }
        }
        batch.delete(row_key.as_bytes());
    }
    report.rows += updates.len();
    // The blob's graph is the stored graph; `replace_records` only
    // relinks the records to it.
    session.replace_records(updates);
    batch.delete(index_key.as_bytes());
    queue_fresh(&mut batch, &session, &vectorized)?;
    db.write(batch)?;
    report.sessions += 1;
    Ok(())
}

/// Step 2: mark the rows of the families stored unmarked until version 1.
fn to_version_2(db: &DB) -> Result<Report> {
    let mut report = Report::default();
    mark_family::<StoredEmbedding>(db, "embedding:", rows::EMBEDDING_V1, &mut report)?;
    mark_family::<MessageRowV1>(db, "message:", rows::MESSAGE_V1, &mut report)?;
    mark_family::<Card>(db, "card:", rows::CARD_V1, &mut report)?;
    mark_family::<RadarDismissal>(db, "radar_dismissal:", rows::DISMISSAL_V1, &mut report)?;
    Ok(report)
}

/// Rewrite the unmarked `T` rows under `prefix` behind `marker`.
fn mark_family<T: Serialize + DeserializeOwned>(
    db: &DB,
    prefix: &str,
    marker: &[u8],
    report: &mut Report,
) -> Result<()> {
    rewrite_family(db, prefix, report, |key, row, batch| {
        if row.starts_with(marker) {
            return Ok(true);
        }
        let Some(value) = legacy::decode_unmarked::<T>(row) else {
            return Ok(false);
        };
        batch.put(key.as_bytes(), rows::encode_marked(marker, &value)?);
        Ok(true)
    })
}

/// What step 1 did.
#[derive(Default)]
struct Report {
    sessions: usize,
    rows: usize,
    other: usize,
    graph_rows: usize,
    unreadable: usize,
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} sessions, {} update rows, {} other rows rewritten, {} graph rows dropped, \
             {} unreadable rows left",
            self.sessions, self.rows, self.other, self.graph_rows, self.unreadable
        )
    }
}

fn to_version_1(db: &DB) -> Result<Report> {
    let mut report = Report::default();
    for session_id in list_session_ids(db)? {
        migrate_session(db, session_id, &mut report)?;
    }
    rewrite_family(db, CHECKPOINT_PREFIX, &mut report, |key, row, batch| {
        let checkpoint = match row.strip_prefix(CHECKPOINT_V4) {
            Some(_) => rows::decode_checkpoint(key, row).ok(),
            None => legacy::decode_checkpoint(row),
        };
        let Some(checkpoint) = checkpoint else {
            return Ok(false);
        };
        let index = checkpoint_index_key(checkpoint.session_id, checkpoint.id);
        batch.put(index.as_bytes(), []);
        if !row.starts_with(CHECKPOINT_V4) {
            batch.put(key.as_bytes(), encode_checkpoint(&checkpoint)?);
        }
        Ok(true)
    })?;
    rewrite_family(db, "workspace:", &mut report, |key, row, batch| {
        if [WORKSPACE_V4, WORKSPACE_V5, WORKSPACE_V6]
            .iter()
            .any(|marker| row.starts_with(marker))
        {
            return Ok(true);
        }
        let Some(workspace) = legacy::decode_workspace(row) else {
            return Ok(false);
        };
        batch.put(
            key.as_bytes(),
            rows::encode_marked(WORKSPACE_V4, &workspace)?,
        );
        Ok(true)
    })?;
    rewrite_family(db, "ws_session:", &mut report, |key, row, batch| {
        if row.starts_with(MEMBERSHIP_V1) {
            return Ok(true);
        }
        let Some(membership) = legacy::decode_membership(row) else {
            return Ok(false);
        };
        batch.put(key.as_bytes(), encode_membership(&membership)?);
        Ok(true)
    })?;
    // The file index is rebuilt from the references (its values changed).
    let mut batch = WriteBatch::default();
    for key in keys_with_prefix(db, b"source_by_file:")? {
        batch.delete(&key);
    }
    db.write(batch)?;
    rewrite_family(db, SOURCE_REF_PREFIX, &mut report, |key, row, batch| {
        let source = match row.strip_prefix(SOURCE_V2) {
            Some(_) => SourceRow::decode(key.as_bytes(), row),
            None => legacy::source::decode_source(key, row),
        };
        let source = match source {
            Ok(source) => source,
            Err(e) => {
                warn!("RealRocksDBStorage: migration: {e:#}");
                return Ok(false);
            }
        };
        put_file_index(batch, &source.file_path, &source.entry_id);
        if !row.starts_with(SOURCE_V2) {
            batch.put(key.as_bytes(), source.encode()?);
        }
        Ok(true)
    })?;

    let mut batch = WriteBatch::default();
    for prefix in ["entity:", "relationship:"] {
        for key in keys_with_prefix(db, prefix.as_bytes())? {
            batch.delete(&key);
            report.graph_rows += 1;
        }
    }
    for marker in OLD_INDEX_MARKERS {
        batch.delete(marker);
    }
    db.write(batch)?;
    Ok(report)
}

/// Rewrite every row under `prefix` with `convert`, which queues the new
/// row (and any index entry) and returns false for a row it cannot read.
fn rewrite_family(
    db: &DB,
    prefix: &str,
    report: &mut Report,
    mut convert: impl FnMut(&str, &[u8], &mut WriteBatch) -> Result<bool>,
) -> Result<()> {
    let mut batch = WriteBatch::default();
    for item in scan_prefix(db, prefix.as_bytes()) {
        let (key, row) = item?;
        let key = String::from_utf8_lossy(&key).into_owned();
        if convert(&key, &row, &mut batch)? {
            report.other += 1;
        } else {
            warn!("RealRocksDBStorage: migration left unreadable row {key}");
            report.unreadable += 1;
        }
        if batch.len() >= BATCH_ROWS {
            db.write(std::mem::take(&mut batch))?;
        }
    }
    db.write(batch)?;
    Ok(())
}

/// One session in one batch; see the module docs.
fn migrate_session(db: &DB, session_id: Uuid, report: &mut Report) -> Result<()> {
    let key = session_key(session_id);
    let Some(blob) = db.get(key.as_bytes())? else {
        return Ok(());
    };
    if blob.starts_with(SESSION_V1) {
        return Ok(());
    }
    let StoredSession {
        mut session,
        vectorized,
    } = match serde_json::from_slice::<StoredSession>(&blob) {
        Ok(stored) => stored,
        Err(e) => {
            warn!("RealRocksDBStorage: migration left unreadable session {session_id}: {e}");
            report.unreadable += 1;
            return Ok(());
        }
    };
    let index_key = update_index_key(session_id);
    let stored_ids: Vec<Uuid> = match db.get(index_key.as_bytes())? {
        Some(index) => decode_ids(&index_key, &index)?,
        None => Vec::new(),
    };
    let mut batch = WriteBatch::default();

    let ids: Vec<Uuid> = if !session.records().is_empty() || stored_ids.is_empty() {
        // Embedded list (or nothing): authoritative.
        let embedded: HashSet<Uuid> = session.records().iter().map(|u| u.id).collect();
        for id in stored_ids.iter().filter(|id| !embedded.contains(id)) {
            batch.delete(update_key(session_id, *id).as_bytes());
        }
        for update in session.records() {
            batch.put(
                update_key(session_id, update.id).as_bytes(),
                encode_update(update)?,
            );
            report.rows += 1;
        }
        session.records().iter().map(|u| u.id).collect()
    } else {
        let keys: Vec<String> = stored_ids
            .iter()
            .map(|id| update_key(session_id, *id))
            .collect();
        let mut updates = Vec::with_capacity(keys.len());
        for (row_key, value) in keys.iter().zip(db.multi_get(&keys)) {
            let Some(row) = value? else {
                warn!("RealRocksDBStorage: migration: update row {row_key} is missing");
                report.unreadable += 1;
                continue;
            };
            let update = match row.strip_prefix(UPDATE_V4) {
                Some(_) => rows::decode_update(row_key, &row).ok(),
                None => legacy::decode_update(&row),
            };
            match update {
                Some(update) => {
                    if !row.starts_with(UPDATE_V4) {
                        batch.put(row_key.as_bytes(), encode_update(&update)?);
                        report.rows += 1;
                    }
                    updates.push(update);
                }
                None => {
                    warn!("RealRocksDBStorage: migration left unreadable row {row_key}");
                    report.unreadable += 1;
                }
            }
        }
        session.replace_records(updates);
        stored_ids
    };

    let mut summary = SessionSummary::from_session(&session);
    summary.update_count = ids.len();
    batch.put(index_key.as_bytes(), encode_ids(ids.iter()));
    batch.put(meta_key(session_id).as_bytes(), encode_meta(&summary)?);
    batch.put(key.as_bytes(), encode_session(&session, &vectorized)?);
    db.write(batch)?;
    report.sessions += 1;
    Ok(())
}

#[cfg(test)]
pub(super) mod tests;
