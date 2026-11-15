// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The workspace steps of the migration ([`crate::rocksdb_storage::migrate`]):
//! step 5 gives every workspace row its slug, step 6 the layout with remote
//! reranking.

use std::collections::HashSet;

use rocksdb::{DB, WriteBatch};
use tracing::warn;

use crate::port_error::Result;
use crate::rocksdb_storage::core::scan_prefix;
use crate::rocksdb_storage::legacy::{self, V4WorkspaceRow, V5WorkspaceRow, WORKSPACE_V5};
use crate::rocksdb_storage::rows::{self, WORKSPACE_V6, WorkspaceRow, encode_workspace};
use post_cortex_core::slug::workspace_slug;

/// Step 6: every workspace row of the version 5 layout in the version 6
/// one, the remote reranker forbidden, in one batch. Rows of version 6
/// stay as they are.
pub fn to_version_6(db: &DB) -> Result<usize> {
    let mut batch = WriteBatch::default();
    let mut rewritten = 0;
    for item in scan_prefix(db, b"workspace:") {
        let (key, row) = item?;
        let key = String::from_utf8_lossy(&key).into_owned();
        if row.starts_with(WORKSPACE_V6) {
            continue;
        }
        let Ok(w) = rows::decode_marked::<V5WorkspaceRow>("workspace", &key, WORKSPACE_V5, &row)
        else {
            warn!("RealRocksDBStorage: migration left unreadable row {key}");
            continue;
        };
        let row = WorkspaceRow {
            id: w.id,
            name: w.name,
            slug: w.slug,
            description: w.description,
            created_at: w.created_at,
            projects: w.projects,
            archived_at: w.archived_at,
            remote_rerank: false,
        };
        batch.put(key.as_bytes(), encode_workspace(&row)?);
        rewritten += 1;
    }
    db.write(batch)?;
    Ok(rewritten)
}

/// Step 5: every workspace row of the version 4 layout gets its slug, the
/// oldest first ([`workspace_slug`]: its name's, or with its id when an
/// older workspace has that), in one batch. Rows with a slug keep it.
pub fn to_version_5(db: &DB) -> Result<usize> {
    let mut taken = HashSet::new();
    let mut unslugged: Vec<(String, V4WorkspaceRow)> = Vec::new();
    for item in scan_prefix(db, b"workspace:") {
        let (key, row) = item?;
        let key = String::from_utf8_lossy(&key).into_owned();
        if row.starts_with(WORKSPACE_V5) {
            let w: V5WorkspaceRow = rows::decode_marked("workspace", &key, WORKSPACE_V5, &row)?;
            taken.insert(w.slug);
        } else if let Some(workspace) = legacy::decode_workspace(&row) {
            unslugged.push((key, workspace));
        } else {
            warn!("RealRocksDBStorage: migration left unreadable row {key}");
        }
    }
    unslugged.sort_by_key(|(_, w)| (w.created_at, w.id));
    let mut batch = WriteBatch::default();
    let slugged = unslugged.len();
    for (key, w) in unslugged {
        let slug = workspace_slug(&w.name, w.id, |base| taken.contains(base));
        taken.insert(slug.clone());
        let row = V5WorkspaceRow {
            id: w.id,
            name: w.name,
            slug,
            description: w.description,
            created_at: w.created_at,
            projects: w.projects,
            archived_at: w.archived_at,
        };
        batch.put(key.as_bytes(), rows::encode_marked(WORKSPACE_V5, &row)?);
    }
    db.write(batch)?;
    Ok(slugged)
}
