// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Import: an archive written through the storage ports and the search
//! index, the same for every backend, part by part as the codec reads it
//! (the write pass; the preview pass before it has read the whole archive,
//! so its version, its model and its bytes are known good).
//!
//! `skip_existing` leaves a stored session or workspace as it is (its
//! messages, card, dismissals and documents are still merged); `overwrite` replaces it
//! (see `SessionStore::replace_session`, `WorkspaceStore::restore_workspace`);
//! without either an existing one is an error of the result.

use std::collections::{HashMap, HashSet};

use post_cortex_core::archive::{
    ArchivePart, ExportedSession, ExportedWorkspace, ImportOptions, ImportResult,
};
use post_cortex_core::core::card::Card;
use post_cortex_core::slug::workspace_slug;
use post_cortex_ports::{IndexEntry, SessionChanges};
use tracing::info;
use uuid::Uuid;

use crate::capabilities::transfer::convert::{index_entry, stored_workspace};
use crate::capabilities::transfer::merge::{
    DocumentRuns, Positions, restore_cards, restore_dismissals, restore_document, restore_messages,
    restored_embedding,
};
use crate::capabilities::transfer::parts::PartReader;
use crate::capabilities::transfer::{ArchivePorts, TransferError};

/// Vectors written to the index at once.
const VECTOR_BATCH: usize = 512;

/// An import in progress.
struct Importing<'a> {
    ports: &'a ArchivePorts,
    options: &'a ImportOptions,
    result: ImportResult,
    /// Where the archived messages were stored.
    positions: Positions,
    /// The workspaces stored before the import.
    existing: HashSet<Uuid>,
    /// The slugs stored and given, by workspace.
    slugs: HashMap<String, Uuid>,
    /// The workspaces imported, skipped or overwritten (their cards merge).
    covered: HashSet<Uuid>,
    /// Vectors waiting for the next batch.
    vectors: Vec<IndexEntry>,
    /// The archive's cards (merged at the end).
    cards: Vec<Card>,
    /// The archive's documents, one at a time.
    documents: DocumentRuns,
}

/// Import the archive `parts` into `ports`.
///
/// # Errors
/// The archive's format version is not readable, the archive cannot be read,
/// or storage failed to list what it holds. Per-item failures are in
/// [`ImportResult::errors`].
pub async fn import(
    ports: &ArchivePorts,
    mut parts: PartReader,
    options: &ImportOptions,
) -> Result<ImportResult, TransferError> {
    let stored = ports.stores.workspaces.list_workspaces().await?;
    let mut importing = Importing {
        ports,
        options,
        result: ImportResult::default(),
        positions: Positions::new(),
        existing: stored.iter().map(|w| w.id).collect(),
        slugs: stored.into_iter().map(|w| (w.slug, w.id)).collect(),
        covered: HashSet::new(),
        vectors: Vec::new(),
        cards: Vec::new(),
        documents: DocumentRuns::default(),
    };
    while let Some(part) = parts.next().await {
        match part {
            ArchivePart::Header(header) => ports
                .codec
                .check_version(&header.format_version)
                .map_err(TransferError::Archive)?,
            ArchivePart::Session(session) => importing.session(*session).await?,
            ArchivePart::Workspace(workspace) => importing.workspace(*workspace).await,
            ArchivePart::Checkpoint(checkpoint) => {
                match ports.stores.checkpoints.save_checkpoint(&checkpoint).await {
                    Ok(()) => importing.result.checkpoints_imported += 1,
                    Err(e) => importing.result.errors.push(format!(
                        "Failed to import checkpoint {}: {}",
                        checkpoint.id, e
                    )),
                }
            }
            ArchivePart::Embedding(embedding) => {
                if let Some(embedding) = restored_embedding(*embedding, &importing.positions) {
                    importing.vector(index_entry(embedding)).await;
                }
            }
            ArchivePart::Card(card) => importing.cards.push(*card),
            ArchivePart::Document(version) => {
                if let Some(document) = importing.documents.push(*version) {
                    importing.document(document).await;
                }
            }
        }
    }
    parts.finish().await?;
    if let Some(document) = importing.documents.finish() {
        importing.document(document).await;
    }
    importing.flush_vectors().await;
    let Importing {
        mut result,
        cards,
        covered,
        ..
    } = importing;
    restore_cards(&ports.stores, cards, options, &covered, &mut result).await;

    info!(
        "Import complete: {} sessions, {} workspaces, {} updates, {} messages, {} cards, \
         {} documents, {} embeddings, {} errors",
        result.sessions_imported,
        result.workspaces_imported,
        result.updates_imported,
        result.messages_imported,
        result.cards_imported,
        result.documents_imported,
        result.embeddings_imported,
        result.errors.len()
    );
    Ok(result)
}

impl Importing<'_> {
    /// Merge one document's archived versions, if its workspace is
    /// imported.
    async fn document(&mut self, versions: Vec<post_cortex_core::core::document::Document>) {
        let covered = versions
            .first()
            .is_some_and(|d| self.covered.contains(&d.workspace_id));
        if covered {
            restore_document(&self.ports.stores, versions, self.options, &mut self.result).await;
        }
    }

    async fn session(&mut self, exported: ExportedSession) -> Result<(), TransferError> {
        let stores = &self.ports.stores;
        let session_id = exported.session.session.id();
        if let Some(filter) = &self.options.session_filter
            && !filter.contains(&session_id)
        {
            return Ok(());
        }
        let exists = stores.sessions.session_exists(session_id).await?;
        if exists {
            if self.options.skip_existing {
                self.result.sessions_skipped += 1;
                self.messages(session_id, exported).await;
                return Ok(());
            } else if !self.options.overwrite {
                self.result.errors.push(format!(
                    "Session {session_id} already exists (use --overwrite or --skip-existing)"
                ));
                return Ok(());
            }
        }
        // Overwrite replaces the stored session (its records, graph,
        // vectors and messages), the same as a workspace overwrite; then
        // the session is saved with every record (files before v1.4.0
        // carry them beside the session) and its whole graph.
        let session = &exported.session.session;
        let records = exported.records();
        let count = records.len();
        let replaced = if exists {
            stores
                .sessions
                .replace_session(session, &exported.session.vectorized)
                .await
        } else {
            Ok(())
        };
        let changes = SessionChanges {
            updates: records,
            whole_graph: true,
            vectorized: exported.session.vectorized.clone(),
            ..Default::default()
        };
        let saved = match replaced {
            Ok(()) => stores.sessions.save_session(session, changes).await,
            Err(e) => Err(e),
        };
        if let Err(e) = saved {
            self.result
                .errors
                .push(format!("Failed to import session {session_id}: {e}"));
            return Ok(());
        }
        self.result.sessions_imported += 1;
        self.result.updates_imported += count;
        self.messages(session_id, exported).await;
        Ok(())
    }

    async fn messages(&mut self, session_id: Uuid, exported: ExportedSession) {
        restore_messages(
            &self.ports.stores,
            session_id,
            exported.messages,
            &mut self.result,
            &mut self.positions,
        )
        .await;
    }

    async fn workspace(&mut self, mut workspace: ExportedWorkspace) {
        let stores = &self.ports.stores;
        if let Some(filter) = &self.options.workspace_filter
            && !filter.contains(&workspace.id)
        {
            return;
        }
        let dismissals = std::mem::take(&mut workspace.dismissals);
        let id = workspace.id;
        if self.existing.contains(&id) {
            if self.options.skip_existing {
                self.result.workspaces_skipped += 1;
                self.covered.insert(id);
                restore_dismissals(stores, id, dismissals, &mut self.result).await;
                return;
            } else if !self.options.overwrite {
                self.result.errors.push(format!(
                    "Workspace {id} already exists (use --overwrite or --skip-existing)"
                ));
                return;
            }
        }
        // Created as archived (its creation time, members with their
        // roles, projects and archive state in one write); on overwrite
        // it replaces the stored row and members, and the target's card
        // and dismissals stay unless the archive carries its own.
        // Its own slug, unless another workspace has it (or the archive is
        // older than slugs): then one the way a new workspace gets it.
        let taken =
            |slug: &str, slugs: &HashMap<String, Uuid>| slugs.get(slug).is_some_and(|w| *w != id);
        let slug = match workspace.slug.take() {
            Some(slug) if !taken(&slug, &self.slugs) => slug,
            _ => workspace_slug(&workspace.name, id, |base| taken(base, &self.slugs)),
        };
        self.slugs.insert(slug.clone(), id);
        match stores
            .workspaces
            .restore_workspace(&stored_workspace(workspace, slug))
            .await
        {
            Ok(()) => {
                self.result.workspaces_imported += 1;
                self.covered.insert(id);
                restore_dismissals(stores, id, dismissals, &mut self.result).await;
            }
            Err(e) => self
                .result
                .errors
                .push(format!("Failed to import workspace {id}: {e}")),
        }
    }

    async fn vector(&mut self, entry: Result<IndexEntry, post_cortex_ports::PortError>) {
        match entry {
            Ok(entry) => self.vectors.push(entry),
            Err(e) => self
                .result
                .errors
                .push(format!("Embedding not imported: {e:#}")),
        }
        if self.vectors.len() >= VECTOR_BATCH {
            self.flush_vectors().await;
        }
    }

    /// Write the waiting vectors to the index.
    async fn flush_vectors(&mut self) {
        if self.vectors.is_empty() {
            return;
        }
        let batch = std::mem::take(&mut self.vectors);
        let count = batch.len();
        let stored = match &self.ports.index {
            Some(index) => index.upsert(batch).await,
            None => Err(post_cortex_ports::PortError::Unavailable(
                "no search index to store them in".into(),
            )),
        };
        match stored {
            Ok(()) => self.result.embeddings_imported += count,
            Err(e) => self
                .result
                .errors
                .push(format!("Failed to import {count} embeddings: {e:#}")),
        }
    }
}
