// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The export format's versions: the one written, and the one gate of
//! what this build reads. The archive's types are plain data in
//! `post_cortex_core::archive`.

/// Current export format version
/// v1.0.0 - Initial format (RelationType only in graph edges)
/// v1.1.0 - Added EdgeData with context field in graph edges
/// v1.2.0 - Added embeddings export/import
/// v1.3.0 - Added cards (every version) and the sessions' message streams
/// v1.4.0 - Every scope carries the same sections: the stored vectors of
///          the exported sessions (records and messages), the radar
///          dismissals of the exported workspaces; each record is written
///          once (in the session, without the derived views; `updates`
///          left out)
/// v1.4.1 - The embedding model of the vectors
/// v1.5.0 - The documents of modules (every version) of the exported
///          workspaces
/// v1.6.0 - A message's parent and blocks (an agent's turn whole), written
///          when it has them
/// v1.7.0 - A document's data is a message of its kind's type (`@type` and
///          base64 `bytes`); JSON data of earlier files is converted on
///          import (W23)
/// v1.8.0 - A workspace's slug; an older file's workspaces get theirs on
///          import
/// v1.9.0 - Whether a workspace lets its accurate searches use the remote
///          reranker (`remote_rerank`; absent: off)
/// v1.10.0 - A person's user card (scope `{"person": <user id>}`), which a
///          full export carries with the host's
///
/// Binaries that read at most 1.3.0 refuse a 1.4.0 file before writing
/// anything ("Incompatible export format version"); the new fields are
/// optional, so a 1.4.0 binary reads every 1.x file (see
/// [`check_format_version`]).
pub const EXPORT_FORMAT_VERSION: &str = "1.10.0";

/// The newest minor version of format 1 this build reads.
const READABLE_MINOR: u32 = 10;

/// Whether this build reads an archive of format `version`: `1.<minor>.<patch>`
/// with `minor` at most the current one. The one version gate of import
/// and preview, for every backend.
///
/// # Errors
/// [`ArchiveError::Version`](crate::export_import::codec::ArchiveError::Version)
/// when it is not readable.
pub fn check_format_version(version: &str) -> crate::export_import::codec::Result<()> {
    let parts: Option<Vec<u32>> = version.split('.').map(|p| p.parse().ok()).collect();
    match parts.as_deref() {
        Some([1, minor, _patch]) if *minor <= READABLE_MINOR => Ok(()),
        _ => Err(crate::export_import::codec::ArchiveError::Version {
            found: version.to_string(),
        }),
    }
}

/// Schema URL for validation (GitHub raw URL)
pub const EXPORT_SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/JuliusDeGroot/post-cortex/main/schemas/export/v1.10.0.json";
