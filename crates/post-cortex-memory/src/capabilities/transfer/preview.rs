// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Previewing an import: what conflicts with what storage holds, and how
//! each conflict would be resolved. The preview is the import's first pass
//! over the archive: it reads every part, so a broken or unreadable archive
//! is refused before anything is written.

use crate::capabilities::transfer::TransferError;
use crate::capabilities::transfer::merge::{
    ALL_VERSIONS, DocumentRuns, card_covered, merge_card, merge_document,
};
use crate::capabilities::transfer::model::{ArchivedVectors, mismatch};
use crate::capabilities::transfer::parts::PartReader;
use chrono::{DateTime, Utc};
use post_cortex_core::archive::{ArchivePart, ImportOptions, card_histories};
use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_ports::ArchiveCodec;
use post_cortex_ports::storage::Stores;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// What an import does with a session, workspace or card the target
/// already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolution {
    /// Not imported; reported as an error (neither skip nor overwrite set).
    Error,
    /// Left as it is (`skip_existing`).
    Skip,
    /// Replaced by the archive's copy (`overwrite`); for a card, the
    /// archive's history replaces the target's.
    Overwrite,
    /// A card whose history diverged: the target's latest version stays
    /// (the default; also when the archive's was written later but
    /// `cards_newer` was not asked for).
    Keep,
    /// A card whose history diverged: the archive's latest version was
    /// written later and `cards_newer` was asked for, so it is added as the
    /// target's next version.
    Update,
}

impl ConflictResolution {
    fn of(options: &ImportOptions) -> Self {
        if options.skip_existing {
            Self::Skip
        } else if options.overwrite {
            Self::Overwrite
        } else {
            Self::Error
        }
    }

    /// `error`, `skip` or `overwrite`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Skip => "skip",
            Self::Overwrite => "overwrite",
            Self::Keep => "keep",
            Self::Update => "update",
        }
    }
}

/// A session or workspace of an archive that the target already has, or a
/// card whose history diverged from the target's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportConflict {
    /// `session`, `workspace` or `card`.
    pub kind: &'static str,
    /// Its id (a card's workspace; the nil id for the user card).
    pub id: Uuid,
    /// Its name in the archive (`user` for the user card, the workspace's
    /// name for its card).
    pub name: String,
    /// What the import does with it.
    pub resolution: ConflictResolution,
    /// For a card: the latest version on each side, so a person can choose.
    pub card: Option<CardHeads>,
}

/// The latest version of a diverged card on each side of an import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardHeads {
    /// The target's (what is here).
    pub target: CardHead,
    /// The archive's.
    pub archive: CardHead,
}

/// One side's latest version of a card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardHead {
    /// Its version number in its own history.
    pub version: u64,
    /// When it was written.
    pub updated_at: DateTime<Utc>,
    /// Its first non-empty line.
    pub first_line: String,
}

impl CardHead {
    fn of(card: &Card) -> Self {
        Self {
            version: card.version,
            updated_at: card.updated_at,
            first_line: card
                .text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or_default()
                .to_string(),
        }
    }

    /// `v2, 2026-10-20 14:42 UTC: "first line"`.
    pub fn describe(&self) -> String {
        format!(
            "v{}, {}: {:?}",
            self.version,
            self.updated_at.format("%Y-%m-%d %H:%M UTC"),
            self.first_line
        )
    }
}

impl ImportConflict {
    /// One line for a person: what it is, what the import does and, for a
    /// card, both latest versions.
    pub fn describe(&self) -> String {
        let what = match (self.kind, self.id.is_nil()) {
            ("card", true) => "user card".to_string(),
            ("card", false) => format!("card of workspace {:?} ({})", self.name, self.id),
            (kind, _) => format!("{kind} {:?} ({})", self.name, self.id),
        };
        match &self.card {
            Some(heads) => format!(
                "{what}: {}; here {}; archive {}",
                self.resolution.as_str(),
                heads.target.describe(),
                heads.archive.describe()
            ),
            None => format!("{what}: {}", self.resolution.as_str()),
        }
    }
}

/// What importing an archive would do, without writing anything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ImportPreview {
    /// Format version of the archive.
    pub format_version: String,
    /// Sessions the import covers (after the filters).
    pub sessions: usize,
    /// Workspaces the import covers (after the filters).
    pub workspaces: usize,
    /// Records (context updates) of those sessions.
    pub updates: usize,
    /// Checkpoints in the archive.
    pub checkpoints: usize,
    /// Embeddings in the archive (written by backends that store them).
    pub embeddings: usize,
    /// Captured messages of those sessions.
    pub messages: usize,
    /// Of those, messages whose turn the target does not store yet.
    pub messages_new: usize,
    /// Cards (one per scope) the import covers.
    pub cards: usize,
    /// Documents the import covers.
    pub documents: usize,
    /// Sessions and workspaces the target already has, and cards and
    /// documents whose history diverged from the target's.
    pub conflicts: Vec<ImportConflict>,
    /// What an import would refuse (an archive embedded with another
    /// model than the target's).
    pub warnings: Vec<String>,
}

/// What the preview pass learns: the preview, what the archive says about
/// its vectors (an import refuses another model's) and what an import
/// covers.
#[derive(Debug, Default)]
pub struct Scan {
    /// What importing would do.
    pub preview: ImportPreview,
    /// The archive's vectors.
    pub vectors: ArchivedVectors,
    /// The sessions an import covers (after the filters), in archive order.
    pub sessions: Vec<Uuid>,
    /// The workspaces an import covers (after the filters).
    pub workspaces: HashSet<Uuid>,
}

/// Read the archive `parts` (the preview pass) and compare them with what
/// `stores` hold: what importing with `options` would do, its conflicts and
/// whether its vectors fit (`options.embedding_model`). Writes nothing.
///
/// # Errors
/// The archive's format version is not supported, the bytes are not an
/// archive, or storage failed.
pub async fn scan(
    stores: &Stores,
    codec: &dyn ArchiveCodec,
    mut parts: PartReader,
    options: &ImportOptions,
) -> Result<Scan, TransferError> {
    let target_sessions: HashSet<Uuid> =
        stores.sessions.list_sessions().await?.into_iter().collect();
    let target_workspaces: HashSet<Uuid> = stores
        .workspaces
        .list_workspaces()
        .await?
        .into_iter()
        .map(|w| w.id)
        .collect();
    let resolution = ConflictResolution::of(options);
    let mut scan = Scan::default();
    let mut names: HashMap<Uuid, String> = HashMap::new();
    let mut cards = Vec::new();
    let mut documents = DocumentRuns::default();
    while let Some(part) = parts.next().await {
        let preview = &mut scan.preview;
        match part {
            ArchivePart::Header(header) => {
                codec
                    .check_version(&header.format_version)
                    .map_err(TransferError::Archive)?;
                preview.format_version = header.format_version;
                scan.vectors.model = header.metadata.embedding_model;
            }
            ArchivePart::Session(exported) => {
                let id = exported.session.session.id();
                if options
                    .session_filter
                    .as_ref()
                    .is_some_and(|filter| !filter.contains(&id))
                {
                    continue;
                }
                scan.sessions.push(id);
                preview.sessions += 1;
                preview.updates += exported.record_count();
                preview.messages += exported.messages.len();
                let exists = target_sessions.contains(&id);
                let known: Option<HashSet<String>> = if exists && !exported.messages.is_empty() {
                    Some(
                        stores
                            .messages
                            .load_messages(id, 0..u64::MAX)
                            .await?
                            .into_iter()
                            .map(|m| m.turn_id)
                            .collect(),
                    )
                } else {
                    None
                };
                let mut seen = HashSet::new();
                preview.messages_new += exported
                    .messages
                    .iter()
                    .filter(|m| {
                        known.as_ref().is_none_or(|k| !k.contains(&m.turn_id))
                            && seen.insert(m.turn_id.as_str())
                    })
                    .count();
                if exists {
                    preview.conflicts.push(ImportConflict {
                        kind: "session",
                        id,
                        name: exported
                            .session
                            .session
                            .metadata()
                            .name
                            .clone()
                            .unwrap_or_default(),
                        resolution,
                        card: None,
                    });
                }
            }
            ArchivePart::Workspace(workspace) => {
                if options
                    .workspace_filter
                    .as_ref()
                    .is_some_and(|filter| !filter.contains(&workspace.id))
                {
                    continue;
                }
                preview.workspaces += 1;
                scan.workspaces.insert(workspace.id);
                if target_workspaces.contains(&workspace.id) {
                    preview.conflicts.push(ImportConflict {
                        kind: "workspace",
                        id: workspace.id,
                        name: workspace.name.clone(),
                        resolution,
                        card: None,
                    });
                }
                names.insert(workspace.id, workspace.name);
            }
            ArchivePart::Checkpoint(_) => preview.checkpoints += 1,
            ArchivePart::Embedding(embedding) => {
                preview.embeddings += 1;
                scan.vectors.add(&embedding);
            }
            ArchivePart::Card(card) => cards.push(*card),
            ArchivePart::Document(version) => {
                if let Some(document) = documents.push(*version) {
                    document_conflict(stores, &mut scan, document, options).await?;
                }
            }
        }
    }
    parts.finish().await?;
    if let Some(document) = documents.finish() {
        document_conflict(stores, &mut scan, document, options).await?;
    }
    for (scope, archived) in card_histories(cards) {
        if !card_covered(scope, options, &scan.workspaces) {
            continue;
        }
        scan.preview.cards += 1;
        let mut ours = stores.cards.card_history(scope, ALL_VERSIONS).await?;
        ours.reverse();
        if let (_, Some(resolution)) = merge_card(&archived, &ours, options) {
            let (id, name) = match scope {
                CardScope::User => (Uuid::nil(), "user".to_string()),
                CardScope::Person(id) => (id, scope.key()),
                CardScope::Workspace(id) => (id, names.get(&id).cloned().unwrap_or_default()),
            };
            let heads = match (ours.last(), archived.last()) {
                (Some(target), Some(archive)) => Some(CardHeads {
                    target: CardHead::of(target),
                    archive: CardHead::of(archive),
                }),
                _ => None,
            };
            scan.preview.conflicts.push(ImportConflict {
                kind: "card",
                id,
                name,
                resolution,
                card: heads,
            });
        }
    }
    scan.preview.warnings = mismatch(&scan.vectors, options.embedding_model.as_ref())
        .into_iter()
        .collect();
    Ok(scan)
}

/// Count one archived document (its versions, oldest first) if its
/// workspace is imported, and note its conflict with the target's history.
async fn document_conflict(
    stores: &Stores,
    scan: &mut Scan,
    archived: Vec<post_cortex_core::core::document::Document>,
    options: &ImportOptions,
) -> Result<(), TransferError> {
    let Some(latest) = archived.last() else {
        return Ok(());
    };
    if !scan.workspaces.contains(&latest.workspace_id) {
        return Ok(());
    }
    scan.preview.documents += 1;
    let mut ours = stores
        .documents
        .document_history(latest.workspace_id, latest.id, ALL_VERSIONS)
        .await?;
    ours.reverse();
    if let (_, Some(resolution)) = merge_document(&archived, &ours, options) {
        scan.preview.conflicts.push(ImportConflict {
            kind: "document",
            id: latest.id,
            name: latest.title.clone(),
            resolution,
            card: None,
        });
    }
    Ok(())
}
