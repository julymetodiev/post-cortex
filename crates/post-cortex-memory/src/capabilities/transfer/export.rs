// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Export: what an [`ExportScope`] covers, read through the storage ports
//! and the search index, the same for every backend.
//!
//! Every scope carries the same sections: the sessions (each record once),
//! the workspaces with their radar dismissals, the checkpoints of the
//! exported sessions (with `include_checkpoints`), their message streams
//! and stored vectors (message vectors only with `include_messages`, and
//! only of exported messages), the cards of the scope, and every version
//! of the exported workspaces' documents.
//!
//! The archive's header carries the counts and precedes the sessions, so
//! an export makes two passes over the sessions, one at a time: [`plan`]
//! counts, [`ExportPlan::write`] writes. Neither holds more than one
//! session's records, messages and vectors, or more than one document's
//! versions. Writes that land between the
//! passes make the counts differ from the content by those writes.

use std::collections::{HashMap, HashSet};
use std::io::Write;

use chrono::Utc;
use post_cortex_core::archive::{
    ArchiveHeader, ArchivePart, ArchiveSize, ExportOptions, ExportScope, ExportType,
    ExportedEmbedding, ExportedSession, ExportedWorkspace, card_scopes,
};
use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_core::core::message;
use post_cortex_ports::EntrySelect;
use tracing::info;
use uuid::Uuid;

use crate::capabilities::transfer::convert::{exported_embedding, exported_workspace};
use crate::capabilities::transfer::merge::{ALL_VERSIONS, is_message_embedding};
use crate::capabilities::transfer::parts::PartWriter;
use crate::capabilities::transfer::{ArchivePorts, TransferError};

/// An export counted and ready to write.
pub struct ExportPlan {
    /// The archive's header, with the counts of the counting pass. Its
    /// time is the plan's; a caller may set another before writing.
    pub header: ArchiveHeader,
    ports: ArchivePorts,
    options: ExportOptions,
    sessions: Vec<Uuid>,
    workspaces: Vec<ExportedWorkspace>,
    cards: Vec<Card>,
    /// The exported documents, by workspace; their versions are read when
    /// written.
    documents: Vec<(Uuid, Uuid)>,
}

/// Session `id` as exported (its messages with `include_messages`), or the
/// error that it could not be loaded.
async fn load_session(
    ports: &ArchivePorts,
    id: Uuid,
    options: &ExportOptions,
) -> Result<Result<ExportedSession, post_cortex_ports::PortError>, TransferError> {
    let session = match ports.stores.sessions.load_session(id).await {
        Ok(session) => session,
        Err(e) => return Ok(Err(e)),
    };
    let messages = if options.include_messages {
        ports.stores.messages.load_messages(id, 0..u64::MAX).await?
    } else {
        Vec::new()
    };
    Ok(Ok(ExportedSession {
        session,
        updates: Vec::new(),
        messages,
    }))
}

/// The positions of `session`'s exported messages.
fn positions(session: &ExportedSession) -> HashSet<u64> {
    session.messages.iter().map(|m| m.seq).collect()
}

/// The stored vectors of session `id`, in content-id order. Vectors of
/// messages go only with `include_messages`, and only those of exported
/// messages (`positions`): a vector whose message is gone (left by an
/// older import or an incomplete delete) would be dropped by the import,
/// so the counts say what an import writes.
async fn vectors(
    ports: &ArchivePorts,
    id: Uuid,
    positions: &HashSet<u64>,
    options: &ExportOptions,
) -> Result<Vec<ExportedEmbedding>, TransferError> {
    let Some(index) = &ports.index else {
        return Ok(Vec::new());
    };
    let mut entries: Vec<ExportedEmbedding> = index
        .entries(&id.to_string(), &EntrySelect::All)
        .await?
        .into_iter()
        .map(exported_embedding)
        .filter(|e| {
            !is_message_embedding(e)
                || (options.include_messages
                    && message::seq_of_content_id(&e.content_id)
                        .is_some_and(|seq| positions.contains(&seq)))
        })
        .collect();
    entries.sort_by(|a, b| a.content_id.cmp(&b.content_id));
    Ok(entries)
}

/// Count what `scope` covers in `ports` (the first pass).
///
/// # Errors
/// A workspace of the scope does not exist, a session of it cannot be
/// loaded (every such session is named), or storage failed.
pub async fn plan(
    ports: &ArchivePorts,
    scope: &ExportScope,
    options: ExportOptions,
) -> Result<ExportPlan, TransferError> {
    let stores = &ports.stores;
    let (export_type, sessions, stored) = match scope {
        ExportScope::All => (
            ExportType::Full,
            stores.sessions.list_sessions().await?,
            stores.workspaces.list_workspaces().await?,
        ),
        ExportScope::Sessions(ids) => (
            ExportType::SelectiveSessions {
                session_ids: ids.clone(),
            },
            ids.clone(),
            Vec::new(),
        ),
        ExportScope::Workspace(id) => {
            let workspace = stores
                .workspaces
                .list_workspaces()
                .await?
                .into_iter()
                .find(|w| w.id == *id)
                .ok_or(TransferError::WorkspaceNotFound(*id))?;
            let members = workspace.sessions.iter().map(|(s, _)| *s).collect();
            (
                ExportType::SelectiveWorkspace { workspace_id: *id },
                members,
                vec![workspace],
            )
        }
    };
    let mut header = ArchiveHeader::new(
        ports.codec.format_version(),
        ports.codec.schema_url(),
        export_type,
        options.compression,
        Utc::now(),
    );
    header.metadata.embedding_model = options.embedding_model.clone();

    let mut workspaces = Vec::with_capacity(stored.len());
    for workspace in stored {
        let mut exported = exported_workspace(workspace);
        exported.dismissals = stores.dismissals.list_dismissals(exported.id).await?;
        workspaces.push(exported);
    }
    let mut cards = Vec::new();
    let mut documents = Vec::new();
    let mut document_count = 0;
    let people = match header.metadata.export_type {
        ExportType::Full => stores.cards.card_people().await?,
        _ => Vec::new(),
    };
    for scope in card_scopes(
        &header.metadata.export_type,
        workspaces.iter().map(|w| w.id),
        people,
    ) {
        let mut versions = stores.cards.card_history(scope, ALL_VERSIONS).await?;
        versions.reverse();
        cards.extend(versions);
        // The documents go with their workspace, as its card does.
        if let CardScope::Workspace(workspace_id) = scope {
            for latest in stores
                .documents
                .latest_documents(workspace_id, None)
                .await?
            {
                // Counted as stored: an import may have left gaps in the
                // version numbers.
                document_count += stores
                    .documents
                    .document_history(workspace_id, latest.id, ALL_VERSIONS)
                    .await?
                    .len();
                documents.push((workspace_id, latest.id));
            }
        }
    }

    let counts = &mut header.metadata;
    let mut failed = Vec::new();
    for &id in &sessions {
        let session = match load_session(ports, id, &options).await? {
            Ok(session) => session,
            Err(e) => {
                failed.push((id, e));
                continue;
            }
        };
        counts.session_count += 1;
        counts.update_count += session.record_count();
        counts.message_count += session.messages.len();
        counts.embedding_count += vectors(ports, id, &positions(&session), &options)
            .await?
            .len();
        if options.include_checkpoints {
            counts.checkpoint_count += stores.checkpoints.list_checkpoints(id).await?.len();
        }
    }
    if !failed.is_empty() {
        return Err(TransferError::SessionsNotLoaded(failed));
    }
    counts.workspace_count = workspaces.len();
    counts.card_count = cards.len();
    counts.document_count = document_count;
    info!(
        "Export of {} sessions, {} workspaces: {} records, {} vectors, {} messages",
        counts.session_count,
        counts.workspace_count,
        counts.update_count,
        counts.embedding_count,
        counts.message_count
    );
    Ok(ExportPlan {
        header,
        ports: ports.clone(),
        options,
        sessions,
        workspaces,
        cards,
        documents,
    })
}

impl ExportPlan {
    /// Write the archive into `out` (the second pass), through the codec on
    /// a blocking thread. On failure the archive is left unfinished.
    ///
    /// # Errors
    /// A session can no longer be loaded, storage failed, or writing
    /// failed.
    pub async fn write(self, out: Box<dyn Write + Send>) -> Result<ArchiveSize, TransferError> {
        let writer = PartWriter::start(
            std::sync::Arc::clone(&self.ports.codec),
            out,
            self.options.compression,
            self.options.pretty,
        );
        match self.send(&writer).await {
            Ok(()) => writer.finish().await,
            Err(e) => {
                writer.abandon().await;
                Err(e)
            }
        }
    }

    /// Send every part to `writer`; stops early when the writer failed
    /// (its error is then [`PartWriter::finish`]'s).
    async fn send(self, writer: &PartWriter) -> Result<(), TransferError> {
        let Self {
            header,
            ports,
            options,
            sessions,
            workspaces,
            cards,
            documents,
        } = self;
        if !writer.send(ArchivePart::Header(Box::new(header))).await {
            return Ok(());
        }
        let mut exported_positions: HashMap<Uuid, HashSet<u64>> = HashMap::new();
        for &id in &sessions {
            let session = load_session(&ports, id, &options)
                .await?
                .map_err(|e| TransferError::SessionsNotLoaded(vec![(id, e)]))?;
            exported_positions.insert(id, positions(&session));
            if !writer.send(ArchivePart::Session(Box::new(session))).await {
                return Ok(());
            }
        }
        for workspace in workspaces {
            if !writer
                .send(ArchivePart::Workspace(Box::new(workspace)))
                .await
            {
                return Ok(());
            }
        }
        if options.include_checkpoints {
            for &id in &sessions {
                for checkpoint in ports.stores.checkpoints.list_checkpoints(id).await? {
                    if !writer
                        .send(ArchivePart::Checkpoint(Box::new(checkpoint)))
                        .await
                    {
                        return Ok(());
                    }
                }
            }
        }
        let none = HashSet::new();
        for &id in &sessions {
            let positions = exported_positions.get(&id).unwrap_or(&none);
            for vector in vectors(&ports, id, positions, &options).await? {
                if !writer.send(ArchivePart::Embedding(Box::new(vector))).await {
                    return Ok(());
                }
            }
        }
        for card in cards {
            if !writer.send(ArchivePart::Card(Box::new(card))).await {
                return Ok(());
            }
        }
        for (workspace_id, id) in documents {
            let mut versions = ports
                .stores
                .documents
                .document_history(workspace_id, id, ALL_VERSIONS)
                .await?;
            versions.reverse();
            for version in versions {
                if !writer.send(ArchivePart::Document(Box::new(version))).await {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}
