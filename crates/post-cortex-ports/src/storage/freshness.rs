// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The freshness port: source references, staleness and invalidation.

use crate::error::Result;
use async_trait::async_trait;
use post_cortex_core::core::freshness::{
    CascadeInvalidateReport, FreshnessEntry, SourceReference, SymbolId,
};
use uuid::Uuid;

/// A source reference that is stale: it was invalidated by
/// `invalidate_source` or `cascade_invalidate` and has not been registered
/// again since. Returned by `get_stale_entries_by_source`.
#[derive(Debug, Clone)]
pub struct StaleEntryInfo {
    /// Entry ID that was marked stale.
    pub entry_id: String,
    /// Symbol name associated with the entry (when known).
    pub symbol_name: Option<String>,
    /// Symbol type (e.g. `"fn"`, `"struct"`) associated with the entry.
    pub symbol_type: Option<String>,
}

/// Which source references an invalidation marks stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidationScope {
    /// The references one session registered (plus references stored
    /// before references recorded their session, whose owner is unknown).
    Session(Uuid),
    /// Every session's references to the file: an explicit administrative
    /// invalidation, never the side effect of one session's change.
    AllSessions,
}

/// Storage trait for source freshness tracking.
///
/// This trait manages tracking where information in PCX originally came from,
/// allowing efficient invalidation when the source files change.
#[async_trait]
pub trait FreshnessStore: Send + Sync {
    /// Register or update a source reference for an entry
    async fn register_source(&self, session_id: Uuid, reference: SourceReference) -> Result<()>;

    /// Check if an entry's stored hash matches the provided hash.
    /// When the stored entry has a FunctionScope with ast_hash, compares that
    /// instead of the file-level hash (semantic freshness).
    async fn check_freshness(&self, entry_id: &str, file_hash: &[u8]) -> Result<FreshnessEntry>;

    /// Semantic freshness check: when both the stored reference (a function
    /// scope) and the caller have an AST hash, they are compared instead of
    /// the file hashes.
    async fn check_freshness_semantic(
        &self,
        entry_id: &str,
        file_hash: &[u8],
        ast_hash: Option<&[u8]>,
        symbol_name: Option<&str>,
    ) -> Result<FreshnessEntry>;

    /// Mark stale the references to `file_path` within `scope`, usually
    /// because the file changed. A stale reference is kept: it reports
    /// `Stale` and is listed by `get_stale_entries_by_source` until it is
    /// registered again (see `post_cortex_storage::freshness`). Returns how many
    /// references were marked.
    async fn invalidate_source(&self, scope: InvalidationScope, file_path: &str) -> Result<u32>;

    /// Get all entries associated with a specific file path
    async fn get_entries_by_source(&self, file_path: &str) -> Result<Vec<SourceReference>>;

    /// Return entry_ids (plus symbol metadata) for all source_reference records that
    /// are still marked stale (status=1) for the given file.  Called by Axon after
    /// `register_source_batch` to detect symbols that were deleted from the file
    /// (they were marked stale by `invalidate_source` but never re-registered).
    async fn get_stale_entries_by_source(&self, file_path: &str) -> Result<Vec<StaleEntryInfo>>;

    /// Register symbol-level dependencies (e.g., fn foo depends on struct Bar).
    /// Used for cascade invalidation.
    async fn register_symbol_dependencies(
        &self,
        from: SymbolId,
        to_symbols: Vec<SymbolId>,
    ) -> Result<u32>;

    /// Cascade invalidate: when a symbol changes, invalidate all entries
    /// whose symbols depend on it (transitively up to max_depth).
    async fn cascade_invalidate(
        &self,
        changed: SymbolId,
        new_ast_hash: Vec<u8>,
        max_depth: u32,
    ) -> Result<CascadeInvalidateReport>;

    /// Batch freshness check: check multiple entries in a single storage
    /// round-trip, as [`Self::check_freshness_semantic`] would one by one.
    ///
    /// Each element of `entries` is `(entry_id, file_hash, ast_hash, symbol_name)`.
    /// Returns one `FreshnessEntry` per input element, in the same order. A
    /// stored reference that cannot be read fails the batch (it is never
    /// reported `Unknown`).
    async fn check_freshness_batch(
        &self,
        entries: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)>,
    ) -> Result<Vec<FreshnessEntry>>;
}
