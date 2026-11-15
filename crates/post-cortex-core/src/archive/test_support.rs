// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A whole archive in memory, for tests: built or inspected at once, and
//! turned into the parts the codec writes and back. Its serde form is the
//! archive's JSON, the oracle the streaming codec is checked against.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::archive::{
    ArchiveHeader, ArchivePart, CompressionType, ExportMetadata, ExportType, ExportedEmbedding,
    ExportedSession, ExportedWorkspace,
};
use crate::core::card::Card;
use crate::core::document::Document;
use crate::session::checkpoint::SessionCheckpoint;

/// A whole archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportData {
    /// Format version for compatibility checking
    pub format_version: String,
    /// Schema URL for validation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Export metadata
    pub metadata: ExportMetadata,
    /// Exported sessions with their updates
    pub sessions: Vec<ExportedSession>,
    /// Exported workspaces
    pub workspaces: Vec<ExportedWorkspace>,
    /// Exported checkpoints
    pub checkpoints: Vec<SessionCheckpoint>,
    /// Exported embeddings (v1.2.0+)
    #[serde(default)]
    pub embeddings: Vec<ExportedEmbedding>,
    /// Every stored version of the exported cards, oldest first per card
    /// (v1.3.0+).
    #[serde(default)]
    pub cards: Vec<Card>,
    /// Every stored version of the exported documents, oldest first per
    /// document (v1.5.0+).
    #[serde(default)]
    pub documents: Vec<Document>,
}

impl ExportData {
    /// An empty archive of `format_version` (with its `schema` URL), made
    /// at `exported_at`.
    pub fn new(
        format_version: &str,
        schema: &str,
        export_type: ExportType,
        compression: CompressionType,
        exported_at: DateTime<Utc>,
    ) -> Self {
        Self::with_header(ArchiveHeader::new(
            format_version,
            schema,
            export_type,
            compression,
            exported_at,
        ))
    }

    /// An empty archive with `header`.
    pub fn with_header(header: ArchiveHeader) -> Self {
        Self {
            format_version: header.format_version,
            schema: header.schema,
            metadata: header.metadata,
            sessions: Vec::new(),
            workspaces: Vec::new(),
            checkpoints: Vec::new(),
            embeddings: Vec::new(),
            cards: Vec::new(),
            documents: Vec::new(),
        }
    }

    /// The archive of `parts` (header first).
    ///
    /// # Panics
    /// When the first part is not the header, or another one is.
    pub fn from_parts(parts: impl IntoIterator<Item = ArchivePart>) -> Self {
        let mut parts = parts.into_iter();
        let Some(ArchivePart::Header(header)) = parts.next() else {
            panic!("an archive starts with its header");
        };
        let mut data = Self::with_header(*header);
        for part in parts {
            match part {
                ArchivePart::Header(_) => panic!("an archive has one header"),
                ArchivePart::Session(s) => data.sessions.push(*s),
                ArchivePart::Workspace(w) => data.workspaces.push(*w),
                ArchivePart::Checkpoint(c) => data.checkpoints.push(*c),
                ArchivePart::Embedding(e) => data.embeddings.push(*e),
                ArchivePart::Card(c) => data.cards.push(*c),
                ArchivePart::Document(d) => data.documents.push(*d),
            }
        }
        data
    }

    /// The parts of the archive, in archive order.
    pub fn parts(&self) -> Vec<ArchivePart> {
        let header = ArchiveHeader {
            format_version: self.format_version.clone(),
            schema: self.schema.clone(),
            metadata: self.metadata.clone(),
        };
        std::iter::once(ArchivePart::Header(Box::new(header)))
            .chain(
                self.sessions
                    .iter()
                    .map(|s| ArchivePart::Session(Box::new(s.clone()))),
            )
            .chain(
                self.workspaces
                    .iter()
                    .map(|w| ArchivePart::Workspace(Box::new(w.clone()))),
            )
            .chain(
                self.checkpoints
                    .iter()
                    .map(|c| ArchivePart::Checkpoint(Box::new(c.clone()))),
            )
            .chain(
                self.embeddings
                    .iter()
                    .map(|e| ArchivePart::Embedding(Box::new(e.clone()))),
            )
            .chain(
                self.cards
                    .iter()
                    .map(|c| ArchivePart::Card(Box::new(c.clone()))),
            )
            .chain(
                self.documents
                    .iter()
                    .map(|d| ArchivePart::Document(Box::new(d.clone()))),
            )
            .collect()
    }

    /// Update metadata counts based on current data
    pub fn update_counts(&mut self) {
        self.metadata.session_count = self.sessions.len();
        self.metadata.workspace_count = self.workspaces.len();
        self.metadata.update_count = self
            .sessions
            .iter()
            .map(ExportedSession::record_count)
            .sum();
        self.metadata.checkpoint_count = self.checkpoints.len();
        self.metadata.embedding_count = self.embeddings.len();
        self.metadata.message_count = self.sessions.iter().map(|s| s.messages.len()).sum();
        self.metadata.card_count = self.cards.len();
        self.metadata.document_count = self.documents.len();
    }

    /// Get list of session IDs in this export
    pub fn session_ids(&self) -> Vec<Uuid> {
        self.sessions
            .iter()
            .map(|s| s.session.session.id())
            .collect()
    }

    /// Get list of workspace IDs in this export
    pub fn workspace_ids(&self) -> Vec<Uuid> {
        self.workspaces.iter().map(|w| w.id).collect()
    }
}
