// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The session layout of data versions 1 and 2, read (and, by step 1,
//! written) only by the migration: one JSON blob per session holding its
//! metadata, entity graph and vectorized ids, update rows keyed by the
//! update id, and an index row listing them in session order.

use crate::port_error::Result;
use post_cortex_core::session::active_session::{ActiveSession, StoredSession};
use post_cortex_ports::error::corrupt;
use uuid::Uuid;

use crate::rocksdb_storage::rows::{body, json_marked};

/// Session blob, JSON, written without `incremental_updates`.
pub const SESSION_V1: &[u8] = b"pcx-ses-v1\0";

pub fn update_key(session_id: Uuid, update_id: Uuid) -> String {
    format!("session:{}:update:{}", session_id, update_id)
}

pub fn update_index_key(session_id: Uuid) -> String {
    format!("session_updates:{}", session_id)
}

pub fn encode_ids<'a>(ids: impl Iterator<Item = &'a Uuid>) -> Vec<u8> {
    ids.flat_map(|id| *id.as_bytes()).collect()
}

/// The ids of the update index stored under `key`; a length that is not a
/// whole number of ids is `Corrupt`.
pub fn decode_ids(key: &str, bytes: &[u8]) -> Result<Vec<Uuid>> {
    let (chunks, rest) = bytes.as_chunks::<16>();
    if !rest.is_empty() {
        return Err(corrupt("update index", key, "length is not a multiple of 16").into());
    }
    Ok(chunks.iter().map(|c| Uuid::from_bytes(*c)).collect())
}

pub fn encode_session(session: &ActiveSession, vectorized: &[Uuid]) -> Result<Vec<u8>> {
    json_marked(
        SESSION_V1,
        &session
            .serializable_view()
            .with_vectorized(vectorized)
            .without_incremental_updates(),
    )
}

pub fn decode_session(key: &str, row: &[u8]) -> Result<StoredSession> {
    let json = body("session", key, SESSION_V1, row)?;
    serde_json::from_slice(json).map_err(|e| corrupt("session", key, e).into())
}
