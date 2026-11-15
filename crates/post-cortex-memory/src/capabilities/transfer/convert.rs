// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Between the archive's sections and what the ports store: index entries
//! and workspaces.

use post_cortex_core::archive::{ExportedEmbedding, ExportedWorkspace};
use post_cortex_ports::error::parse_timestamp;
use post_cortex_ports::storage::StoredWorkspace;
use post_cortex_ports::{IndexEntry, PortError, VectorMetadata};

/// An index entry as the archive carries it.
pub fn exported_embedding(entry: IndexEntry) -> ExportedEmbedding {
    let meta = entry.meta;
    ExportedEmbedding {
        content_id: meta.id,
        session_id: meta.source,
        vector: entry.vector,
        text: meta.text.to_string(),
        content_type: meta.content_type,
        timestamp: meta.timestamp.to_rfc3339(),
        metadata: meta.metadata,
    }
}

/// The index entry of an archived embedding.
///
/// # Errors
/// `Corrupt` on a timestamp that is not RFC 3339.
pub fn index_entry(e: ExportedEmbedding) -> Result<IndexEntry, PortError> {
    Ok(IndexEntry {
        meta: VectorMetadata {
            timestamp: parse_timestamp("embedding", &e.content_id, &e.timestamp)?,
            id: e.content_id,
            text: e.text.into(),
            source: e.session_id,
            content_type: e.content_type,
            metadata: e.metadata,
        },
        vector: e.vector,
    })
}

/// A stored workspace as the archive carries it (without its dismissals).
pub fn exported_workspace(ws: StoredWorkspace) -> ExportedWorkspace {
    ExportedWorkspace {
        id: ws.id,
        name: ws.name,
        slug: Some(ws.slug),
        description: ws.description,
        sessions: ws.sessions,
        created_at: ws.created_at,
        projects: ws.projects,
        archived_at: ws.archived_at,
        remote_rerank: ws.remote_rerank,
        dismissals: Vec::new(),
    }
}

/// The workspace row of an archived workspace, with the slug the import
/// gave it.
pub fn stored_workspace(ws: ExportedWorkspace, slug: String) -> StoredWorkspace {
    StoredWorkspace {
        id: ws.id,
        name: ws.name,
        slug,
        description: ws.description,
        sessions: ws.sessions,
        created_at: ws.created_at,
        projects: ws.projects,
        archived_at: ws.archived_at,
        remote_rerank: ws.remote_rerank,
    }
}
