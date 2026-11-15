// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Listing session ids without loading the sessions: by seeking past each
//! session's rows.

use crate::port_error::Result;
use crate::rocksdb_storage::core::prefix_read_opts;
use crate::rocksdb_storage::session_rows::{SESSION_KEY_LEN, SESSION_PREFIX};
use rocksdb::DB;
use uuid::Uuid;

/// Ids of every `session:{id}` blob, skipping over each session's rows
/// with a seek instead of walking them.
pub fn list_session_ids(db: &DB) -> Result<Vec<Uuid>> {
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
