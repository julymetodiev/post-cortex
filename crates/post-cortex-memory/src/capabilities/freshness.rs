// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Freshness: source references of records, staleness checks and
//! invalidation (ARCHITECTURE section 4). Invalidation is scoped to a
//! session (its records about the file go, with their graph); the
//! unscoped form is the explicit administrative one.

use std::sync::Arc;

use futures::stream::{self, StreamExt};
use post_cortex_core::core::freshness::{
    CascadeInvalidateReport, FreshnessEntry, SourceReference, SymbolId,
};
use post_cortex_ports::{FreshnessStore, InvalidationScope, StaleEntryInfo};
use tracing::{error, info};
use uuid::Uuid;

use crate::memory_system::MemoryRuntime;

/// Source registrations of one batch in flight at a time.
const REGISTER_CONCURRENCY: usize = 16;

/// Why a freshness operation failed.
#[derive(Debug, thiserror::Error)]
pub enum FreshnessError {
    /// The session does not exist.
    #[error("Session {0} not found")]
    SessionNotFound(Uuid),
    /// The change could not be swapped in, or storage failed.
    #[error("{0}")]
    Storage(#[from] crate::memory_system::RuntimeError),
}

/// The one mapping of a storage port error to this capability's error.
fn storage(e: post_cortex_ports::PortError) -> FreshnessError {
    FreshnessError::Storage(e.into())
}

/// One entry of a batch check: entry id, current file hash, and the AST
/// hash and symbol name of a function-scoped check.
pub type FreshnessCheck = (String, Vec<u8>, Option<Vec<u8>>, Option<String>);

/// The Freshness capability service.
#[derive(Clone)]
pub struct Freshness {
    rt: Arc<MemoryRuntime>,
    store: Arc<dyn FreshnessStore>,
}

impl Freshness {
    pub(crate) fn new(rt: Arc<MemoryRuntime>, store: Arc<dyn FreshnessStore>) -> Self {
        Self { rt, store }
    }

    /// Register (or re-register) the source reference of a record.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn register(
        &self,
        session_id: Uuid,
        reference: SourceReference,
    ) -> Result<(), FreshnessError> {
        self.store
            .register_source(session_id, reference)
            .await
            .map_err(storage)
    }

    /// Register a batch of source references, a bounded number at a time;
    /// returns how many were stored (failures are logged).
    pub async fn register_batch(&self, session_id: Uuid, references: Vec<SourceReference>) -> u32 {
        let results: Vec<_> = stream::iter(references.into_iter().map(|reference| {
            let entry_id = reference.entry_id.clone();
            let store = Arc::clone(&self.store);
            async move { (entry_id, store.register_source(session_id, reference).await) }
        }))
        .buffer_unordered(REGISTER_CONCURRENCY)
        .collect()
        .await;
        let mut registered = 0;
        for (entry_id, result) in results {
            match result {
                Ok(()) => registered += 1,
                Err(e) => error!("RegisterSourceBatch: failed for entry_id={entry_id}: {e}"),
            }
        }
        registered
    }

    /// Check a batch of entries in one storage round trip; one result per
    /// entry, in order.
    ///
    /// # Errors
    /// Storage failed (a stored reference that cannot be read fails the
    /// batch).
    pub async fn check_batch(
        &self,
        checks: Vec<FreshnessCheck>,
    ) -> Result<Vec<FreshnessEntry>, FreshnessError> {
        self.store
            .check_freshness_batch(checks)
            .await
            .map_err(storage)
    }

    /// `file_path` changed: remove the session's records about it, rebuild
    /// its graph, and mark the session's references to the file stale.
    /// Other sessions are untouched. Returns (references invalidated,
    /// entities after the rebuild).
    ///
    /// # Errors
    /// The session does not exist, contention, or storage failed.
    pub async fn invalidate(
        &self,
        session_id: Uuid,
        file_path: &str,
    ) -> Result<(u32, usize), FreshnessError> {
        let rt = &self.rt;
        let handle = rt
            .session_manager
            .get_session(session_id)
            .await
            .map_err(FreshnessError::Storage)?
            .ok_or(FreshnessError::SessionNotFound(session_id))?;

        // The records about the file and the graph rebuilt without them are
        // one swap (retried under contention).
        let mut removed: Vec<Uuid> = Vec::new();
        let installed =
            MemoryRuntime::cas_update_session(&handle, session_id, "invalidate", |session| {
                removed = session.remove_updates_for_file(file_path);
                if removed.is_empty() {
                    return false;
                }
                session.rebuild_entity_graph();
                true
            })
            .await
            .map_err(FreshnessError::Storage)?;

        let entities_after = match &installed {
            Some(session) => {
                for id in &removed {
                    handle.vectorized().unmark(id);
                }
                // Named: storage never infers a removal. The graph was
                // rebuilt, so it is written whole.
                rt.persister
                    .persist_rebuild_nowait(&handle, removed.clone());
                let content_ids: Vec<String> = removed.iter().map(Uuid::to_string).collect();
                rt.drop_content_vectors(session_id, &content_ids).await;
                session.entity_graph().entity_count()
            }
            None => handle.load().entity_graph().entity_count(),
        };

        let invalidated = self
            .store
            .invalidate_source(InvalidationScope::Session(session_id), file_path)
            .await
            .map_err(storage)?;
        info!(
            "Invalidated {} for session {}: {} source references, {} records removed, {} entities",
            file_path,
            session_id,
            invalidated,
            removed.len(),
            entities_after,
        );
        Ok((invalidated, entities_after))
    }

    /// The administrative invalidation: mark every session's references to
    /// `file_path` stale (no records are removed). Returns how many.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn invalidate_all_sessions(&self, file_path: &str) -> Result<u32, FreshnessError> {
        self.store
            .invalidate_source(InvalidationScope::AllSessions, file_path)
            .await
            .map_err(storage)
    }

    /// The references to `file_path` still marked stale.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn stale_entries(
        &self,
        file_path: &str,
    ) -> Result<Vec<StaleEntryInfo>, FreshnessError> {
        self.store
            .get_stale_entries_by_source(file_path)
            .await
            .map_err(storage)
    }

    /// Register that `from` depends on `to`; returns the edges created.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn register_symbol_dependencies(
        &self,
        from: SymbolId,
        to: Vec<SymbolId>,
    ) -> Result<u32, FreshnessError> {
        self.store
            .register_symbol_dependencies(from, to)
            .await
            .map_err(storage)
    }

    /// `changed` got `new_ast_hash`: invalidate the entries whose symbols
    /// depend on it, transitively up to `max_depth`.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn cascade_invalidate(
        &self,
        changed: SymbolId,
        new_ast_hash: Vec<u8>,
        max_depth: u32,
    ) -> Result<CascadeInvalidateReport, FreshnessError> {
        self.store
            .cascade_invalidate(changed, new_ast_hash, max_depth)
            .await
            .map_err(storage)
    }
}
