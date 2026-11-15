// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The export archive (ARCHITECTURE section 5): plain data of what an
//! export holds (sessions with their records and message streams,
//! checkpoints, workspaces with their dismissals, cards, index entries and
//! the embedding model), and the options and outcome of an export and an
//! import. An archive is written and read as a sequence of parts
//! ([`ArchivePart`]), never whole. The `ArchiveCodec` adapter owns the
//! format versions, the JSON schema and compression; the Transfer
//! capability owns what goes into an archive and the import merge rules.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::context_update::ContextUpdate;
use crate::core::dismissal::RadarDismissal;
use crate::core::message::Message;
use crate::session::active_session::StoredSession;
use crate::workspace::SessionRole;

mod part;
#[cfg(any(test, feature = "test-support"))]
mod test_support;

pub use part::{
    ArchiveHeader, ArchivePart, ArchiveSection, ArchiveSize, card_histories, card_scopes,
};
#[cfg(any(test, feature = "test-support"))]
pub use test_support::ExportData;

/// The model an archive's vectors were embedded with (and the model an
/// embedding engine runs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingModelInfo {
    /// The model's configuration name (e.g. `PotionMultilingual`).
    pub model: String,
    /// The length of its vectors.
    pub dimension: usize,
}

/// Compression type for export
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompressionType {
    /// No compression — raw JSON.
    #[default]
    None,
    /// Gzip compression (`.json.gz`).
    Gzip,
    /// Zstandard compression (`.json.zst`).
    Zstd,
}

impl CompressionType {
    /// Its name (`none`, `gzip`, `zstd`), as [`std::str::FromStr`] reads it.
    pub fn name(&self) -> &'static str {
        match self {
            CompressionType::None => "none",
            CompressionType::Gzip => "gzip",
            CompressionType::Zstd => "zstd",
        }
    }

    /// Get file extension for this compression type
    pub fn extension(&self) -> &'static str {
        match self {
            CompressionType::None => "json",
            CompressionType::Gzip => "json.gz",
            CompressionType::Zstd => "json.zst",
        }
    }

    /// Detect compression type from file path
    pub fn from_path(path: &Path) -> Self {
        let path_str = path.to_string_lossy().to_lowercase();
        if path_str.ends_with(".json.gz") || path_str.ends_with(".gz") {
            CompressionType::Gzip
        } else if path_str.ends_with(".json.zst") || path_str.ends_with(".zst") {
            CompressionType::Zstd
        } else {
            CompressionType::None
        }
    }
}

/// A compression name that is none of `none`, `gzip`, `zstd` (or their
/// short forms).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown compression {0:?} (expected none, gzip or zstd)")]
pub struct UnknownCompression(pub String);

impl std::str::FromStr for CompressionType {
    type Err = UnknownCompression;

    /// The exact names; `json`, `gz` and `zst` are accepted as short forms.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" | "json" => Ok(CompressionType::None),
            "gzip" | "gz" => Ok(CompressionType::Gzip),
            "zstd" | "zst" => Ok(CompressionType::Zstd),
            _ => Err(UnknownCompression(s.to_string())),
        }
    }
}

/// Export type indicator
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportType {
    /// Full database export — all sessions, workspaces, checkpoints.
    Full,
    /// Selective export of a list of sessions.
    SelectiveSessions {
        /// IDs of the sessions included in the export.
        session_ids: Vec<Uuid>,
    },
    /// Selective export of a single workspace (and its member sessions).
    SelectiveWorkspace {
        /// ID of the exported workspace.
        workspace_id: Uuid,
    },
}

/// Metadata about the export
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportMetadata {
    /// When the export was created
    pub exported_at: DateTime<Utc>,
    /// Version of post-cortex that created this export
    pub post_cortex_version: String,
    /// Type of export (full or selective)
    pub export_type: ExportType,
    /// Compression used (for reference, actual compression is in the file)
    pub compression: CompressionType,
    /// Total number of sessions exported
    pub session_count: usize,
    /// Total number of workspaces exported
    pub workspace_count: usize,
    /// Total number of updates exported
    pub update_count: usize,
    /// Total number of checkpoints exported
    pub checkpoint_count: usize,
    /// Total number of embeddings exported (v1.2.0+)
    #[serde(default)]
    pub embedding_count: usize,
    /// Total number of captured messages exported (v1.3.0+)
    #[serde(default)]
    pub message_count: usize,
    /// Total number of card versions exported (v1.3.0+)
    #[serde(default)]
    pub card_count: usize,
    /// Number of document versions (format 1.5.0 and later).
    #[serde(default)]
    pub document_count: usize,
    /// The model the embeddings were made with (v1.4.1+; absent when the
    /// exporter did not know it)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<EmbeddingModelInfo>,
}

/// Exported session data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedSession {
    /// The session with its records and its embedded record ids; written
    /// without the views derived from the records (v1.4.0+), which a reader
    /// rebuilds.
    #[serde(serialize_with = "session_records_only")]
    pub session: StoredSession,
    /// The session's records again, as files before v1.4.0 carry them;
    /// empty (and left out) from v1.4.0, where `session` holds them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub updates: Vec<ContextUpdate>,
    /// The session's captured message stream, by `seq` (v1.3.0+; empty in
    /// older exports and with [`ExportOptions::include_messages`] off).
    #[serde(default)]
    pub messages: Vec<Message>,
}

impl ExportedSession {
    /// The session's records: `updates` in files before v1.4.0, else the
    /// session's own.
    pub fn records(&self) -> Vec<ContextUpdate> {
        if self.updates.is_empty() {
            self.session
                .session
                .records()
                .iter()
                .map(|u| ContextUpdate::clone(u))
                .collect()
        } else {
            self.updates.clone()
        }
    }

    /// How many records the session has.
    pub fn record_count(&self) -> usize {
        if self.updates.is_empty() {
            self.session.session.records().len()
        } else {
            self.updates.len()
        }
    }
}

fn session_records_only<S: serde::Serializer>(
    session: &StoredSession,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    session.view().without_derived().serialize(serializer)
}

/// Exported workspace data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedWorkspace {
    /// Workspace ID.
    pub id: Uuid,
    /// Workspace display name.
    pub name: String,
    /// The segment of its addresses (v1.8.0+); an import gives one to an
    /// archive's workspace without it, or whose slug another workspace has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// Workspace description.
    pub description: String,
    /// Member sessions and their roles.
    pub sessions: Vec<(Uuid, SessionRole)>,
    /// Creation timestamp (Unix seconds).
    pub created_at: u64,
    /// Projects resolved to the workspace (absent in older exports).
    #[serde(default)]
    pub projects: Vec<crate::workspace::WorkspaceProject>,
    /// When the workspace was archived (Unix seconds); absent while it is
    /// active and in older exports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<u64>,
    /// Whether its accurate searches may use the remote reranker (v1.9.0+;
    /// absent: off, as in older exports).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remote_rerank: bool,
    /// The workspace's radar dismissals (v1.4.0+).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dismissals: Vec<RadarDismissal>,
}

/// Exported embedding data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedEmbedding {
    /// Stable content ID this embedding is keyed by.
    pub content_id: String,
    /// Session ID that produced the embedding.
    pub session_id: String,
    /// Embedding vector.
    pub vector: Vec<f32>,
    /// Source text that was embedded.
    pub text: String,
    /// Content type tag (e.g. `"message"`, `"summary"`).
    pub content_type: String,
    /// Timestamp of the embedded content (ISO-8601 string).
    pub timestamp: String,
    /// Free-form key/value metadata attached to the embedding.
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
}

/// What an export covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportScope {
    /// Everything: all sessions and workspaces.
    All,
    /// These sessions.
    Sessions(Vec<Uuid>),
    /// One workspace and its member sessions.
    Workspace(Uuid),
}

/// Options for export operation
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Compression type
    pub compression: CompressionType,
    /// Include checkpoints in export
    pub include_checkpoints: bool,
    /// Pretty print JSON (only for uncompressed)
    pub pretty: bool,
    /// Include the sessions' captured message streams and their vectors
    /// (default on; streams can be large).
    pub include_messages: bool,
    /// The model the store's vectors are embedded with, recorded in the
    /// archive's metadata.
    pub embedding_model: Option<EmbeddingModelInfo>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            compression: CompressionType::default(),
            include_checkpoints: false,
            pretty: false,
            include_messages: true,
            embedding_model: None,
        }
    }
}

/// Options for import operation
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// The model the target store embeds with: an archive whose vectors
    /// were made by another one is refused (a preview warns)
    pub embedding_model: Option<EmbeddingModelInfo>,
    /// Only import specific session IDs (None = import all)
    pub session_filter: Option<Vec<Uuid>>,
    /// Only import specific workspace IDs (None = import all)
    pub workspace_filter: Option<Vec<Uuid>>,
    /// Skip existing sessions instead of erroring
    pub skip_existing: bool,
    /// Overwrite existing sessions, and replace the history of a card
    /// that differs from the archive's
    pub overwrite: bool,
    /// On a card whose history diverged from the archive's, take the
    /// archive's latest version when it was written later (by `updated_at`):
    /// it is added as the target's next version. Without it (and without
    /// `overwrite`) the target's latest version stays. Version numbers of
    /// two independent histories are not compared.
    pub cards_newer: bool,
}

/// Result of an import operation
#[derive(Debug, Clone, Default)]
pub struct ImportResult {
    /// Number of sessions successfully imported.
    pub sessions_imported: usize,
    /// Number of sessions skipped (already existed and `skip_existing` was set).
    pub sessions_skipped: usize,
    /// Number of workspaces successfully imported.
    pub workspaces_imported: usize,
    /// Number of workspaces skipped.
    pub workspaces_skipped: usize,
    /// Number of context updates imported.
    pub updates_imported: usize,
    /// Number of session checkpoints imported.
    pub checkpoints_imported: usize,
    /// Number of embeddings imported.
    pub embeddings_imported: usize,
    /// Number of captured messages written.
    pub messages_imported: usize,
    /// Number of messages left out because their turn was stored already.
    pub messages_skipped: usize,
    /// Number of cards (one per scope) written or brought up to date.
    pub cards_imported: usize,
    /// Number of cards left as the target had them.
    pub cards_skipped: usize,
    /// Number of documents written or brought up to date.
    pub documents_imported: usize,
    /// Number of documents left as the target had them.
    pub documents_skipped: usize,
    /// The documents written (workspace, document), so their latest
    /// versions can be embedded.
    pub documents: Vec<(Uuid, Uuid)>,
    /// Sessions that received messages, so their vectors can be loaded or
    /// made by a running system.
    pub message_sessions: Vec<Uuid>,
    /// Human-readable errors encountered during import.
    pub errors: Vec<String>,
}
