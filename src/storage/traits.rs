// Copyright (c) 2025, 2026 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Storage trait abstractions for post-cortex.
//!
//! This module defines the core storage traits that allow different backends
//! (RocksDB, SurrealDB, etc.) to be used interchangeably.

use crate::core::context_update::{ContextUpdate, EntityData, EntityRelationship, RelationType};
use crate::core::vector_db::{SearchMatch, VectorMetadata};
use crate::daemon::grpc_service::pb::{
    CascadeInvalidateReport, FreshnessEntry, SourceReference, SymbolId,
};
use crate::graph::entity_graph::EntityNetwork;
use crate::session::active_session::ActiveSession;
use crate::storage::rocksdb_storage::{SessionCheckpoint, StoredWorkspace};
use crate::workspace::SessionRole;
use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

/// Report returned when checking if sources are still fresh
#[derive(Debug, Clone)]
pub struct FreshnessReportExt {
    pub entries: Vec<FreshnessEntry>,
}
///
/// This trait defines the fundamental storage operations required for
/// session management, context updates, and workspace persistence.
///
/// Implementations must be Send + Sync for concurrent access.
#[async_trait]
pub trait Storage: FreshnessStorage + Send + Sync {
    // ===== Session Operations =====

    /// Save a session to storage
    async fn save_session(&self, session: &ActiveSession) -> Result<()>;

    /// Load a session from storage
    async fn load_session(&self, session_id: Uuid) -> Result<ActiveSession>;

    /// Delete a session and all related data
    async fn delete_session(&self, session_id: Uuid) -> Result<()>;

    /// Delete all entities and relationships for a session (used by entity graph rebuild)
    async fn clear_session_entities(&self, session_id: Uuid) -> Result<()>;

    /// List all session IDs
    async fn list_sessions(&self) -> Result<Vec<Uuid>>;

    /// Check if a session exists
    async fn session_exists(&self, session_id: Uuid) -> Result<bool>;

    // ===== Context Update Operations =====

    /// Batch save multiple context updates efficiently
    async fn batch_save_updates(&self, session_id: Uuid, updates: Vec<ContextUpdate>)
    -> Result<()>;

    /// Save session and context updates in a single atomic write.
    /// Default implementation calls save_session then batch_save_updates sequentially.
    /// Backends can override for a single WriteBatch operation.
    async fn save_session_with_updates(
        &self,
        session: &ActiveSession,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        self.save_session(session).await?;
        if !updates.is_empty() {
            self.batch_save_updates(session_id, updates).await?;
        }
        Ok(())
    }

    /// Load all updates for a session
    async fn load_session_updates(&self, session_id: Uuid) -> Result<Vec<ContextUpdate>>;

    // ===== Checkpoint Operations =====

    /// Save a session checkpoint
    async fn save_checkpoint(&self, checkpoint: &SessionCheckpoint) -> Result<()>;

    /// Load a session checkpoint
    async fn load_checkpoint(&self, checkpoint_id: Uuid) -> Result<SessionCheckpoint>;

    /// List all checkpoints
    async fn list_checkpoints(&self) -> Result<Vec<SessionCheckpoint>>;

    // ===== Workspace Operations =====

    /// Save workspace metadata
    async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> Result<()>;

    /// Delete a workspace
    async fn delete_workspace(&self, workspace_id: Uuid) -> Result<()>;

    /// List all workspaces
    async fn list_workspaces(&self) -> Result<Vec<StoredWorkspace>>;

    /// Add a session to a workspace
    async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: SessionRole,
    ) -> Result<()>;

    /// Remove a session from a workspace
    async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<()>;

    // ===== Utility Operations =====

    /// Force database compaction
    async fn compact(&self) -> Result<()>;

    /// Get estimated number of keys in database
    async fn get_key_count(&self) -> Result<usize>;

    /// Get database statistics as a string
    async fn get_stats(&self) -> Result<String>;
}

/// Extended storage trait for graph-native operations.
///
/// This trait is implemented by backends with native graph support (e.g., SurrealDB).
/// For backends without native graph support (e.g., RocksDB), graph operations
/// are handled by the in-memory SimpleEntityGraph.
#[async_trait]
pub trait GraphStorage: Storage {
    // ===== Entity Operations =====

    /// Insert or update an entity
    async fn upsert_entity(&self, session_id: Uuid, entity: &EntityData) -> Result<()>;

    /// Get an entity by name
    async fn get_entity(&self, session_id: Uuid, name: &str) -> Result<Option<EntityData>>;

    /// List all entities for a session
    async fn list_entities(&self, session_id: Uuid) -> Result<Vec<EntityData>>;

    /// Delete an entity
    async fn delete_entity(&self, session_id: Uuid, name: &str) -> Result<()>;

    // ===== Relationship Operations =====

    /// Create a relationship between entities
    async fn create_relationship(
        &self,
        session_id: Uuid,
        relationship: &EntityRelationship,
    ) -> Result<()>;

    /// Find all entities related to a given entity
    async fn find_related_entities(
        &self,
        session_id: Uuid,
        entity_name: &str,
    ) -> Result<Vec<String>>;

    /// Find entities related by a specific relation type
    async fn find_related_by_type(
        &self,
        session_id: Uuid,
        entity_name: &str,
        relation_type: &RelationType,
    ) -> Result<Vec<String>>;

    /// Find the shortest path between two entities
    async fn find_shortest_path(
        &self,
        session_id: Uuid,
        from: &str,
        to: &str,
    ) -> Result<Option<Vec<String>>>;

    /// Get the entity network (subgraph) around a center entity
    async fn get_entity_network(
        &self,
        session_id: Uuid,
        center: &str,
        max_depth: usize,
    ) -> Result<EntityNetwork>;
}

/// Vector storage trait for semantic search.
///
/// This trait can be implemented by:
/// - SurrealDB (native HNSW)
/// - VectorDB (in-memory)
/// - External services (Qdrant, etc.)
#[async_trait]
pub trait VectorStorage: Send + Sync {
    /// Add a vector with metadata
    async fn add_vector(&self, vector: Vec<f32>, metadata: VectorMetadata) -> Result<String>;

    /// Add multiple vectors in a batch
    async fn add_vectors_batch(
        &self,
        vectors: Vec<(Vec<f32>, VectorMetadata)>,
    ) -> Result<Vec<String>>;

    /// Search for similar vectors
    async fn search(&self, query: &[f32], k: usize) -> Result<Vec<SearchMatch>>;

    /// Search within a specific session
    async fn search_in_session(
        &self,
        query: &[f32],
        k: usize,
        session_id: &str,
    ) -> Result<Vec<SearchMatch>>;

    /// Search with a content type filter
    async fn search_by_content_type(
        &self,
        query: &[f32],
        k: usize,
        content_type: &str,
    ) -> Result<Vec<SearchMatch>>;

    /// Remove a vector by ID
    async fn remove_vector(&self, id: &str) -> Result<bool>;

    /// Check if a session has any embeddings
    async fn has_session_embeddings(&self, session_id: &str) -> bool;

    /// Count embeddings for a session
    async fn count_session_embeddings(&self, session_id: &str) -> usize;

    /// Get total vector count
    async fn total_count(&self) -> usize;

    /// Get all vectors for a session (for loading into memory)
    async fn get_session_vectors(
        &self,
        session_id: &str,
    ) -> Result<Vec<(Vec<f32>, VectorMetadata)>>;

    /// Get all vectors from storage (for loading into memory on startup)
    async fn get_all_vectors(&self) -> Result<Vec<(Vec<f32>, VectorMetadata)>>;
}

/// Storage trait for source freshness tracking.
///
/// This trait manages tracking where information in PCX originally came from,
/// allowing efficient invalidation when the source files change.
#[async_trait]
pub trait FreshnessStorage: Send + Sync {
    /// Register or update a source reference for an entry
    async fn register_source(&self, session_id: Uuid, reference: SourceReference) -> Result<()>;

    /// Check if an entry's stored hash matches the provided hash.
    /// When the stored entry has a FunctionScope with ast_hash, compares that
    /// instead of the file-level hash (semantic freshness).
    async fn check_freshness(&self, entry_id: &str, file_hash: &[u8]) -> Result<FreshnessEntry>;

    /// Semantic freshness check: compare ast_hash when available, fallback to file hash.
    /// The `ast_hash` and `symbol_name` are optional — if provided and the stored entry
    /// has a FunctionScope, ast_hash comparison is used instead of file-level hash.
    async fn check_freshness_semantic(
        &self,
        entry_id: &str,
        file_hash: &[u8],
        _ast_hash: Option<&[u8]>,
        _symbol_name: Option<&str>,
    ) -> Result<FreshnessEntry> {
        // Default: delegate to basic check_freshness (backends override for semantic)
        self.check_freshness(entry_id, file_hash).await
    }

    /// Invalidate (remove) a source reference, usually because the source changed
    /// or the entry is being deleted. Returns count of removed references.
    async fn invalidate_source(&self, file_path: &str) -> Result<u32>;

    /// Get all entries associated with a specific file path
    async fn get_entries_by_source(&self, file_path: &str) -> Result<Vec<SourceReference>>;

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

    /// Batch freshness check: check multiple entries in a single storage round-trip.
    ///
    /// Each element of `entries` is `(entry_id, file_hash, ast_hash, symbol_name)`.
    /// Returns one `FreshnessEntry` per input element, in the same order.
    ///
    /// The default implementation loops over `check_freshness_semantic` calls.
    /// Backends with native batch support (e.g., SurrealDB) should override this
    /// with a single `WHERE entry_id IN $ids` query.
    async fn check_freshness_batch(
        &self,
        entries: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)>,
    ) -> Result<Vec<FreshnessEntry>> {
        let mut results = Vec::with_capacity(entries.len());
        for (entry_id, file_hash, ast_hash, symbol_name) in entries {
            let result = self
                .check_freshness_semantic(
                    &entry_id,
                    &file_hash,
                    ast_hash.as_deref(),
                    symbol_name.as_deref(),
                )
                .await
                .unwrap_or_else(|_| FreshnessEntry {
                    entry_id: entry_id.clone(),
                    file_path: String::new(),
                    status: crate::daemon::grpc_service::pb::FreshnessStatus::Unknown as i32,
                    stored_hash: Vec::new(),
                    current_hash: file_hash,
                });
            results.push(result);
        }
        Ok(results)
    }
}

/// Unified storage backend enum for runtime selection.
///
/// This enum wraps the different storage implementations and provides
/// a unified interface for the rest of the application.
#[derive(Clone)]
pub enum StorageBackend {
    /// RocksDB-based local storage (default)
    RocksDB(crate::storage::RealRocksDBStorage),

    /// SurrealDB-based storage with native graph support
    #[cfg(feature = "surrealdb-storage")]
    SurrealDB(std::sync::Arc<crate::storage::surrealdb_storage::SurrealDBStorage>),
}

impl StorageBackend {
    /// Check if this backend supports native graph operations
    pub fn supports_native_graph(&self) -> bool {
        match self {
            StorageBackend::RocksDB(_) => false,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(_) => true,
        }
    }

    /// Check if this backend supports native vector operations
    pub fn supports_native_vectors(&self) -> bool {
        match self {
            StorageBackend::RocksDB(_) => false,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(_) => true,
        }
    }
}

// Implement Storage trait for StorageBackend via delegation
#[async_trait]
impl Storage for StorageBackend {
    async fn save_session(&self, session: &ActiveSession) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => storage.save_session(session).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.save_session(session).await,
        }
    }

    async fn load_session(&self, session_id: Uuid) -> Result<ActiveSession> {
        match self {
            StorageBackend::RocksDB(storage) => storage.load_session(session_id).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.load_session(session_id).await,
        }
    }

    async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => storage.delete_session(session_id).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.delete_session(session_id).await,
        }
    }

    async fn clear_session_entities(&self, session_id: Uuid) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => storage.clear_session_entities(session_id).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.clear_session_entities(session_id).await,
        }
    }

    async fn list_sessions(&self) -> Result<Vec<Uuid>> {
        match self {
            StorageBackend::RocksDB(storage) => storage.list_sessions().await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.list_sessions().await,
        }
    }

    async fn session_exists(&self, session_id: Uuid) -> Result<bool> {
        match self {
            StorageBackend::RocksDB(storage) => storage.session_exists(session_id).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.session_exists(session_id).await,
        }
    }

    async fn batch_save_updates(
        &self,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage.batch_save_updates(session_id, updates).await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage.batch_save_updates(session_id, updates).await
            }
        }
    }

    async fn save_session_with_updates(
        &self,
        session: &ActiveSession,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage
                    .save_session_with_updates(session, session_id, updates)
                    .await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(_) => {
                // Use default sequential implementation for SurrealDB
                self.save_session(session).await?;
                if !updates.is_empty() {
                    self.batch_save_updates(session_id, updates).await?;
                }
                Ok(())
            }
        }
    }

    async fn load_session_updates(&self, session_id: Uuid) -> Result<Vec<ContextUpdate>> {
        match self {
            StorageBackend::RocksDB(storage) => storage.load_session_updates(session_id).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.load_session_updates(session_id).await,
        }
    }

    async fn save_checkpoint(&self, checkpoint: &SessionCheckpoint) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => storage.save_checkpoint(checkpoint).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.save_checkpoint(checkpoint).await,
        }
    }

    async fn load_checkpoint(&self, checkpoint_id: Uuid) -> Result<SessionCheckpoint> {
        match self {
            StorageBackend::RocksDB(storage) => storage.load_checkpoint(checkpoint_id).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.load_checkpoint(checkpoint_id).await,
        }
    }

    async fn list_checkpoints(&self) -> Result<Vec<SessionCheckpoint>> {
        match self {
            StorageBackend::RocksDB(storage) => storage.list_checkpoints().await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.list_checkpoints().await,
        }
    }

    async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage
                    .save_workspace_metadata(workspace_id, name, description, session_ids)
                    .await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage
                    .save_workspace_metadata(workspace_id, name, description, session_ids)
                    .await
            }
        }
    }

    async fn delete_workspace(&self, workspace_id: Uuid) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => storage.delete_workspace(workspace_id).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.delete_workspace(workspace_id).await,
        }
    }

    async fn list_workspaces(&self) -> Result<Vec<StoredWorkspace>> {
        match self {
            StorageBackend::RocksDB(storage) => storage.list_workspaces().await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.list_workspaces().await,
        }
    }

    async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: SessionRole,
    ) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage
                    .add_session_to_workspace(workspace_id, session_id, role)
                    .await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage
                    .add_session_to_workspace(workspace_id, session_id, role)
                    .await
            }
        }
    }

    async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage
                    .remove_session_from_workspace(workspace_id, session_id)
                    .await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage
                    .remove_session_from_workspace(workspace_id, session_id)
                    .await
            }
        }
    }

    async fn compact(&self) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => storage.compact().await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.compact().await,
        }
    }

    async fn get_key_count(&self) -> Result<usize> {
        match self {
            StorageBackend::RocksDB(storage) => storage.get_key_count().await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.get_key_count().await,
        }
    }

    async fn get_stats(&self) -> Result<String> {
        match self {
            StorageBackend::RocksDB(storage) => storage.get_stats().await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.get_stats().await,
        }
    }
}

// Implement GraphStorage trait for StorageBackend via delegation
#[async_trait]
impl GraphStorage for StorageBackend {
    async fn upsert_entity(&self, session_id: Uuid, entity: &EntityData) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => storage.upsert_entity(session_id, entity).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.upsert_entity(session_id, entity).await,
        }
    }

    async fn get_entity(&self, session_id: Uuid, name: &str) -> Result<Option<EntityData>> {
        match self {
            StorageBackend::RocksDB(storage) => storage.get_entity(session_id, name).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.get_entity(session_id, name).await,
        }
    }

    async fn list_entities(&self, session_id: Uuid) -> Result<Vec<EntityData>> {
        match self {
            StorageBackend::RocksDB(storage) => storage.list_entities(session_id).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.list_entities(session_id).await,
        }
    }

    async fn delete_entity(&self, session_id: Uuid, name: &str) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => storage.delete_entity(session_id, name).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.delete_entity(session_id, name).await,
        }
    }

    async fn create_relationship(
        &self,
        session_id: Uuid,
        relationship: &EntityRelationship,
    ) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage.create_relationship(session_id, relationship).await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage.create_relationship(session_id, relationship).await
            }
        }
    }

    async fn find_related_entities(
        &self,
        session_id: Uuid,
        entity_name: &str,
    ) -> Result<Vec<String>> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage.find_related_entities(session_id, entity_name).await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage.find_related_entities(session_id, entity_name).await
            }
        }
    }

    async fn find_related_by_type(
        &self,
        session_id: Uuid,
        entity_name: &str,
        relation_type: &RelationType,
    ) -> Result<Vec<String>> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage
                    .find_related_by_type(session_id, entity_name, relation_type)
                    .await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage
                    .find_related_by_type(session_id, entity_name, relation_type)
                    .await
            }
        }
    }

    async fn find_shortest_path(
        &self,
        session_id: Uuid,
        from: &str,
        to: &str,
    ) -> Result<Option<Vec<String>>> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage.find_shortest_path(session_id, from, to).await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage.find_shortest_path(session_id, from, to).await
            }
        }
    }

    async fn get_entity_network(
        &self,
        session_id: Uuid,
        center: &str,
        max_depth: usize,
    ) -> Result<EntityNetwork> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage
                    .get_entity_network(session_id, center, max_depth)
                    .await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage
                    .get_entity_network(session_id, center, max_depth)
                    .await
            }
        }
    }
}

// Implement FreshnessStorage trait for StorageBackend via delegation
#[async_trait]
impl FreshnessStorage for StorageBackend {
    async fn register_source(&self, session_id: Uuid, reference: SourceReference) -> Result<()> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage.register_source(session_id, reference).await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage.register_source(session_id, reference).await
            }
        }
    }

    async fn check_freshness(&self, entry_id: &str, file_hash: &[u8]) -> Result<FreshnessEntry> {
        match self {
            StorageBackend::RocksDB(storage) => storage.check_freshness(entry_id, file_hash).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage.check_freshness(entry_id, file_hash).await
            }
        }
    }

    async fn invalidate_source(&self, file_path: &str) -> Result<u32> {
        match self {
            StorageBackend::RocksDB(storage) => storage.invalidate_source(file_path).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.invalidate_source(file_path).await,
        }
    }

    async fn get_entries_by_source(&self, file_path: &str) -> Result<Vec<SourceReference>> {
        match self {
            StorageBackend::RocksDB(storage) => storage.get_entries_by_source(file_path).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.get_entries_by_source(file_path).await,
        }
    }

    async fn check_freshness_semantic(
        &self,
        entry_id: &str,
        file_hash: &[u8],
        ast_hash: Option<&[u8]>,
        symbol_name: Option<&str>,
    ) -> Result<FreshnessEntry> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage
                    .check_freshness_semantic(entry_id, file_hash, ast_hash, symbol_name)
                    .await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage
                    .check_freshness_semantic(entry_id, file_hash, ast_hash, symbol_name)
                    .await
            }
        }
    }

    async fn register_symbol_dependencies(
        &self,
        from: SymbolId,
        to_symbols: Vec<SymbolId>,
    ) -> Result<u32> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage.register_symbol_dependencies(from, to_symbols).await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage.register_symbol_dependencies(from, to_symbols).await
            }
        }
    }

    async fn cascade_invalidate(
        &self,
        changed: SymbolId,
        new_ast_hash: Vec<u8>,
        max_depth: u32,
    ) -> Result<CascadeInvalidateReport> {
        match self {
            StorageBackend::RocksDB(storage) => {
                storage
                    .cascade_invalidate(changed, new_ast_hash, max_depth)
                    .await
            }
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => {
                storage
                    .cascade_invalidate(changed, new_ast_hash, max_depth)
                    .await
            }
        }
    }

    async fn check_freshness_batch(
        &self,
        entries: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)>,
    ) -> Result<Vec<FreshnessEntry>> {
        match self {
            StorageBackend::RocksDB(storage) => storage.check_freshness_batch(entries).await,
            #[cfg(feature = "surrealdb-storage")]
            StorageBackend::SurrealDB(storage) => storage.check_freshness_batch(entries).await,
        }
    }
}

/// Storage configuration for backend selection
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StorageConfig {
    /// Storage backend type
    pub backend: StorageBackendType,

    /// Path to storage data directory
    pub path: std::path::PathBuf,

    /// SurrealDB-specific configuration
    #[cfg(feature = "surrealdb-storage")]
    pub surrealdb: Option<SurrealDBConfig>,
}

/// Storage backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackendType {
    /// RocksDB local storage (default)
    RocksDB,

    /// SurrealDB storage with native graph support
    #[cfg(feature = "surrealdb-storage")]
    SurrealDB,
}

impl Default for StorageBackendType {
    fn default() -> Self {
        StorageBackendType::RocksDB
    }
}

impl StorageBackendType {
    /// Parse storage backend type from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "rocksdb" | "rocks" => Some(StorageBackendType::RocksDB),
            #[cfg(feature = "surrealdb-storage")]
            "surrealdb" | "surreal" => Some(StorageBackendType::SurrealDB),
            _ => None,
        }
    }
}

/// SurrealDB-specific configuration
#[cfg(feature = "surrealdb-storage")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SurrealDBConfig {
    /// SurrealDB namespace
    pub namespace: String,

    /// SurrealDB database name
    pub database: String,

    /// TiKV endpoints (for distributed mode)
    pub tikv_endpoints: Option<Vec<String>>,
}

#[cfg(feature = "surrealdb-storage")]
impl Default for SurrealDBConfig {
    fn default() -> Self {
        Self {
            namespace: "post_cortex".to_string(),
            database: "main".to_string(),
            tikv_endpoints: None,
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: StorageBackendType::default(),
            path: dirs::data_local_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("post-cortex")
                .join("data"),
            #[cfg(feature = "surrealdb-storage")]
            surrealdb: None,
        }
    }
}
