// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An archive as a sequence of parts, in the order an archive holds them:
//! the header, then the sessions, workspaces, checkpoints, vectors, cards
//! and documents. The codec writes and reads one part at a time, so neither an
//! export nor an import holds a whole archive.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::archive::{
    CompressionType, ExportMetadata, ExportType, ExportedEmbedding, ExportedSession,
    ExportedWorkspace,
};
use crate::core::card::{Card, CardScope};
use crate::core::document::Document;
use crate::session::checkpoint::SessionCheckpoint;

/// What precedes an archive's sessions: its format and its metadata.
#[derive(Debug, Clone)]
pub struct ArchiveHeader {
    /// Format version for compatibility checking.
    pub format_version: String,
    /// Schema URL for validation.
    pub schema: Option<String>,
    /// Export metadata.
    pub metadata: ExportMetadata,
}

impl ArchiveHeader {
    /// The header of an archive of `format_version` (with its `schema` URL;
    /// both are the codec's), made at `exported_at`, every count 0 and no
    /// embedding model.
    pub fn new(
        format_version: &str,
        schema: &str,
        export_type: ExportType,
        compression: CompressionType,
        exported_at: DateTime<Utc>,
    ) -> Self {
        Self {
            format_version: format_version.to_string(),
            schema: Some(schema.to_string()),
            metadata: ExportMetadata {
                exported_at,
                post_cortex_version: env!("CARGO_PKG_VERSION").to_string(),
                export_type,
                compression,
                session_count: 0,
                workspace_count: 0,
                update_count: 0,
                checkpoint_count: 0,
                embedding_count: 0,
                message_count: 0,
                card_count: 0,
                document_count: 0,
                embedding_model: None,
            },
        }
    }
}

/// The sections of an archive, in archive order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArchiveSection {
    /// The sessions, each with its records and message stream.
    Sessions,
    /// The workspaces, each with its radar dismissals.
    Workspaces,
    /// The checkpoints of the exported sessions.
    Checkpoints,
    /// The stored vectors.
    Embeddings,
    /// Every stored version of the exported cards.
    Cards,
    /// Every stored version of the exported workspaces' documents.
    Documents,
}

/// One part of an archive.
#[derive(Debug, Clone)]
pub enum ArchivePart {
    /// The header; first.
    Header(Box<ArchiveHeader>),
    /// A session.
    Session(Box<ExportedSession>),
    /// A workspace.
    Workspace(Box<ExportedWorkspace>),
    /// A checkpoint.
    Checkpoint(Box<SessionCheckpoint>),
    /// A vector.
    Embedding(Box<ExportedEmbedding>),
    /// A card version.
    Card(Box<Card>),
    /// A document version.
    Document(Box<Document>),
}

impl ArchivePart {
    /// The section the part belongs to (`None` for the header).
    pub fn section(&self) -> Option<ArchiveSection> {
        match self {
            Self::Header(_) => None,
            Self::Session(_) => Some(ArchiveSection::Sessions),
            Self::Workspace(_) => Some(ArchiveSection::Workspaces),
            Self::Checkpoint(_) => Some(ArchiveSection::Checkpoints),
            Self::Embedding(_) => Some(ArchiveSection::Embeddings),
            Self::Card(_) => Some(ArchiveSection::Cards),
            Self::Document(_) => Some(ArchiveSection::Documents),
        }
    }
}

/// How large a written archive is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArchiveSize {
    /// Bytes of the archive's JSON, before compression.
    pub json_bytes: u64,
    /// Bytes written (after compression).
    pub written_bytes: u64,
}

/// The card scopes an export of `export_type` covers: the host's user card,
/// the user card of each of `people` and every exported workspace's in a
/// full export, the workspace's in a workspace export, none for selected
/// sessions.
pub fn card_scopes(
    export_type: &ExportType,
    workspaces: impl IntoIterator<Item = Uuid>,
    people: impl IntoIterator<Item = Uuid>,
) -> Vec<CardScope> {
    let workspaces = workspaces.into_iter().map(CardScope::Workspace);
    match export_type {
        ExportType::Full => std::iter::once(CardScope::User)
            .chain(people.into_iter().map(CardScope::Person))
            .chain(workspaces)
            .collect(),
        ExportType::SelectiveWorkspace { .. } => workspaces.collect(),
        ExportType::SelectiveSessions { .. } => Vec::new(),
    }
}

/// The versions of each card in `cards`, oldest first, the cards in the
/// order they first appear.
pub fn card_histories(cards: Vec<Card>) -> Vec<(CardScope, Vec<Card>)> {
    let mut by_scope: Vec<(CardScope, Vec<Card>)> = Vec::new();
    for card in cards {
        match by_scope.iter_mut().find(|(scope, _)| *scope == card.scope) {
            Some((_, versions)) => versions.push(card),
            None => by_scope.push((card.scope, vec![card])),
        }
    }
    for (_, versions) in &mut by_scope {
        versions.sort_by_key(|c| c.version);
    }
    by_scope
}
