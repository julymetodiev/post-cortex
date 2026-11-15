// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Transfer: export and import in the `pcx export` format (ARCHITECTURE
//! section 4) on a running instance: the same data the CLI reads and writes
//! with the daemon stopped, with the in-memory state kept in step.
//!
//! Both stream: an export counts, then writes one session at a time into
//! the caller's output ([`ExportPlan`]); an import reads an archive file
//! twice, a preview pass that refuses a broken archive or another model's
//! vectors before anything is written, then the write pass. An archive that
//! arrives in pieces is spooled to a temporary file first ([`ArchiveSpool`]).

use crate::capabilities::derived::DerivedQueue;
use crate::pipeline::Embed;
use std::sync::Arc;
use std::time::Duration;

use post_cortex_core::archive::{
    EmbeddingModelInfo, ExportOptions, ExportScope, ImportOptions, ImportResult,
};
use post_cortex_ports::storage::Stores;
use post_cortex_ports::{ArchiveCodec, EmbeddingEngine, SearchIndex};
use tracing::{info, warn};
use uuid::Uuid;

use crate::capabilities::MemoryPorts;
use crate::memory_system::MemoryRuntime;

mod convert;
mod export;
mod import;
mod merge;
mod model;
mod parts;
mod preview;
mod spool;

pub use export::ExportPlan;
pub use preview::{CardHead, CardHeads, ConflictResolution, ImportConflict, ImportPreview};
pub use spool::{ArchiveFile, ArchiveSpool};

use parts::PartReader;

/// What an export reads and an import writes: the storage ports, the
/// search index (its entries are the archive's vectors) and the codec
/// (its format version).
#[derive(Clone)]
pub struct ArchivePorts {
    /// The storage ports.
    pub stores: Stores,
    /// The search index; without one an archive carries no vectors.
    pub index: Option<Arc<dyn SearchIndex>>,
    /// The archive codec.
    pub codec: Arc<dyn ArchiveCodec>,
}

/// Longest wait for queued session writes before an export or import.
const FLUSH_LIMIT: Duration = Duration::from_secs(30);

/// Why an export or import failed.
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    /// The archive's vectors were embedded with another model than this
    /// instance's; nothing was written.
    #[error("Import refused, nothing was written: {0}")]
    ModelMismatch(String),
    /// A port failed (storage, the index).
    #[error(transparent)]
    Port(#[from] post_cortex_ports::PortError),
    /// The archive given for an import is not one this build reads: not an
    /// archive, cut, or of an unsupported format version.
    #[error("not a readable pcx export: {0}")]
    Archive(post_cortex_ports::PortError),
    /// The workspace of a workspace export does not exist.
    #[error("workspace {0} not found")]
    WorkspaceNotFound(Uuid),
    /// Sessions of the export could not be loaded; nothing was exported.
    #[error("export failed: {} session(s) could not be loaded: {}", .0.len(), describe_failed(.0))]
    SessionsNotLoaded(Vec<(Uuid, post_cortex_ports::PortError)>),
    /// The archive sent for an import is larger than the limit (bytes).
    #[error("archive larger than {0} bytes")]
    TooLarge(u64),
    /// No archive data was received.
    #[error("no archive data received")]
    Empty,
    /// The archive file could not be written or read.
    #[error("archive file: {0}")]
    File(#[from] std::io::Error),
    /// The codec's thread failed.
    #[error("archive task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

fn describe_failed(failed: &[(Uuid, post_cortex_ports::PortError)]) -> String {
    failed
        .iter()
        .map(|(id, e)| format!("{id}: {e}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The Transfer capability service.
#[derive(Clone)]
pub struct Transfer {
    rt: Arc<MemoryRuntime>,
    ports: ArchivePorts,
    engine: Option<Arc<dyn EmbeddingEngine>>,
    /// Embeds the imported documents' latest versions.
    derived: DerivedQueue,
}

impl Transfer {
    /// Transfer over `ports`: every store, the index and the archive codec
    /// it reads and writes archives through, and the engine whose model
    /// exports record.
    pub(crate) fn new(rt: Arc<MemoryRuntime>, ports: &MemoryPorts, derived: DerivedQueue) -> Self {
        Self {
            rt,
            ports: ArchivePorts {
                stores: ports.stores.clone(),
                index: ports.index.clone(),
                codec: Arc::clone(&ports.archive),
            },
            engine: ports.engine.clone(),
            derived,
        }
    }

    /// The model this instance embeds with (none with embeddings off: the
    /// vectors are then not used and any archive fits).
    fn model(&self) -> Option<EmbeddingModelInfo> {
        if !self.rt.config.enable_embeddings {
            return None;
        }
        self.engine.as_ref().map(|engine| engine.model())
    }

    /// `options` with this instance's model as the import target's, unless
    /// the caller set one.
    fn with_model(&self, mut options: ImportOptions) -> ImportOptions {
        if options.embedding_model.is_none() {
            options.embedding_model = self.model();
        }
        options
    }

    /// Plan the export of `scope` in the `pcx export` format: its counts
    /// (the first pass). Session writes still queued in memory are written
    /// first, so the archive holds them. [`ExportPlan::write`] writes it.
    ///
    /// # Errors
    /// A session or workspace of the scope does not exist, or storage
    /// failed.
    pub async fn plan_export(
        &self,
        scope: ExportScope,
        mut options: ExportOptions,
    ) -> Result<ExportPlan, TransferError> {
        // The archive records the model its vectors were embedded with.
        if options.embedding_model.is_none() {
            options.embedding_model = self.model();
        }
        if let ExportScope::Workspace(id) = &scope
            && self.rt.workspace_manager.get_workspace(id).is_none()
        {
            return Err(TransferError::WorkspaceNotFound(*id));
        }
        let unwritten = self.rt.persister.flush_all(FLUSH_LIMIT).await;
        if unwritten > 0 {
            warn!("Export: {unwritten} session(s) with changes not yet written");
        }
        export::plan(&self.ports, &scope, options).await
    }

    /// A spool for an archive that arrives in pieces, at most the
    /// configured `max_import_bytes`.
    ///
    /// # Errors
    /// The temporary file cannot be created.
    pub fn spool(&self) -> Result<ArchiveSpool, TransferError> {
        ArchiveSpool::new(self.rt.config.max_import_bytes)
    }

    /// The preview pass over `archive`.
    async fn scan(
        &self,
        archive: &ArchiveFile,
        options: &ImportOptions,
    ) -> Result<preview::Scan, TransferError> {
        let parts = PartReader::start(Arc::clone(&self.ports.codec), archive)?;
        preview::scan(
            &self.ports.stores,
            self.ports.codec.as_ref(),
            parts,
            options,
        )
        .await
    }

    /// What importing `archive` with `options` would do: the counts it
    /// covers (with the messages whose turns are new here), the sessions
    /// and workspaces this system already has and the cards whose history
    /// diverged from this system's. Writes nothing.
    ///
    /// # Errors
    /// The archive cannot be read, its format version is not supported, or
    /// storage failed.
    pub async fn preview_import(
        &self,
        archive: &ArchiveFile,
        options: &ImportOptions,
    ) -> Result<ImportPreview, TransferError> {
        Ok(self
            .scan(archive, &self.with_model(options.clone()))
            .await?
            .preview)
    }

    /// Import `archive` as `pcx import` does, then bring memory in step:
    /// imported workspaces replace their in-memory copies, and imported
    /// sessions that were loaded are reloaded from storage. Sessions without
    /// stored embeddings are vectorized on their first search. Imported
    /// messages get their vectors at once: those the archive restored are
    /// loaded, the others embedded (with embeddings enabled). Returns the
    /// preview (what the archive held, before anything was written) and
    /// the result.
    ///
    /// Writes to a session that run while it is imported over may be lost
    /// (last writer wins).
    ///
    /// # Errors
    /// The archive cannot be read, its format version is not supported, its
    /// vectors were embedded with another model (nothing is written), or
    /// storage failed. Per-item failures are in [`ImportResult::errors`].
    pub async fn import(
        &self,
        archive: &ArchiveFile,
        options: ImportOptions,
    ) -> Result<(ImportPreview, ImportResult), TransferError> {
        let options = self.with_model(options);
        let scan = self.scan(archive, &options).await?;
        if let Some(why) = model::mismatch(&scan.vectors, options.embedding_model.as_ref()) {
            return Err(TransferError::ModelMismatch(why));
        }

        // Queued writes of a session imported over must not land after it.
        self.rt.persister.flush_all(FLUSH_LIMIT).await;
        let parts = PartReader::start(Arc::clone(&self.ports.codec), archive)?;
        let result = import::import(&self.ports, parts, &options).await?;

        let stored_workspaces = self.ports.stores.workspaces.list_workspaces().await?;
        for stored in stored_workspaces {
            if scan.workspaces.contains(&stored.id) {
                crate::capabilities::hydrate_workspace(&self.rt.workspace_manager, stored)?;
            }
        }
        for &session_id in &scan.sessions {
            self.reload_session(session_id).await;
        }
        for &session_id in &result.message_sessions {
            self.vectorize_imported_messages(session_id).await;
        }
        for &(workspace_id, id) in &result.documents {
            self.derived.submit(workspace_id, Embed::Document(id)).await;
        }
        info!(
            "Imported {} sessions, {} workspaces, {} updates, {} messages, {} cards, {} documents ({} errors)",
            result.sessions_imported,
            result.workspaces_imported,
            result.updates_imported,
            result.messages_imported,
            result.cards_imported,
            result.documents_imported,
            result.errors.len()
        );
        Ok((scan.preview, result))
    }

    /// Give the messages imported into `session_id` their vectors: the
    /// ones the archive restored are in the index already; embed the rest.
    async fn vectorize_imported_messages(&self, session_id: Uuid) {
        if !self.rt.config.enable_embeddings {
            return;
        }
        if let Err(e) = self.rt.vectorize_session_messages(session_id).await {
            warn!("Imported messages of session {session_id} not embedded: {e}");
        }
    }

    /// Replace the cached copy of `session_id` (if any) with the stored one,
    /// and drop what was derived from the old copy.
    async fn reload_session(&self, session_id: Uuid) {
        self.rt
            .session_manager
            .with_load_gate(session_id, async {
                if self.rt.session_manager.cached(session_id).is_none() {
                    return;
                }
                match self.rt.session_manager.load_stored(session_id).await {
                    Ok(Some(session)) => {
                        self.rt.session_manager.install(session_id, session);
                    }
                    Ok(None) => {}
                    Err(e) => warn!("Imported session {session_id} not reloaded: {e}"),
                }
            })
            .await;
        self.rt.session_manager.invalidate_derived(&session_id);
    }
}
#[cfg(test)]
mod tests;
