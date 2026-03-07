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
use crate::core::context_update::{ContextUpdate, EntityData, EntityRelationship, RelationType};
use crate::core::vector_db::{
    VectorDB, VectorDbConfig, SearchMatch, VectorMetadata,
};
use crate::core::structured_context::StructuredContext;
use crate::graph::entity_graph::EntityNetwork;
use crate::session::active_session::{ActiveSession, ChangeRecord, CodeReference};
use crate::storage::traits::{FreshnessStorage, GraphStorage, Storage, VectorStorage};
use crate::workspace::SessionRole;
use crate::daemon::grpc_service::pb::SourceReference;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use prost::Message;
use rocksdb::{DB, Options, WriteBatch};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info};

use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredWorkspace {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub sessions: Vec<(Uuid, crate::workspace::SessionRole)>,
    pub created_at: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredWorkspaceSession {
    pub workspace_id: Uuid,
    pub session_id: Uuid,
    pub role: crate::workspace::SessionRole,
    pub added_at: u64,
}

/// Required embedding dimension for vector storage (must match model output)
pub const EMBEDDING_DIMENSION: usize = 384;

/// Stored entity record for RocksDB persistence
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredEntity {
    pub session_id: Uuid,
    pub name: String,
    pub entity_type: String,
    pub first_mentioned: DateTime<Utc>,
    pub last_mentioned: DateTime<Utc>,
    pub mention_count: u32,
    pub importance_score: f32,
    pub description: Option<String>,
}

impl StoredEntity {
    pub fn from_entity_data(session_id: Uuid, entity: &EntityData) -> Self {
        Self {
            session_id,
            name: entity.name.clone(),
            entity_type: format!("{:?}", entity.entity_type),
            first_mentioned: entity.first_mentioned,
            last_mentioned: entity.last_mentioned,
            mention_count: entity.mention_count,
            importance_score: entity.importance_score,
            description: entity.description.clone(),
        }
    }

    pub fn to_entity_data(&self) -> EntityData {
        use crate::core::context_update::EntityType;
        EntityData {
            name: self.name.clone(),
            entity_type: match self.entity_type.as_str() {
                "Technology" => EntityType::Technology,
                "Concept" => EntityType::Concept,
                "Problem" => EntityType::Problem,
                "Solution" => EntityType::Solution,
                "Decision" => EntityType::Decision,
                "CodeComponent" => EntityType::CodeComponent,
                _ => EntityType::Concept,
            },
            first_mentioned: self.first_mentioned,
            last_mentioned: self.last_mentioned,
            mention_count: self.mention_count,
            importance_score: self.importance_score,
            description: self.description.clone(),
        }
    }
}

/// Stored relationship record for RocksDB persistence
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredRelationship {
    pub session_id: Uuid,
    pub from_entity: String,
    pub to_entity: String,
    pub relation_type: String,
    pub context: String,
}

impl StoredRelationship {
    pub fn from_relationship(session_id: Uuid, rel: &EntityRelationship) -> Self {
        Self {
            session_id,
            from_entity: rel.from_entity.clone(),
            to_entity: rel.to_entity.clone(),
            relation_type: format!("{:?}", rel.relation_type),
            context: rel.context.clone(),
        }
    }

    pub fn to_relationship(&self) -> EntityRelationship {
        EntityRelationship {
            from_entity: self.from_entity.clone(),
            to_entity: self.to_entity.clone(),
            relation_type: self
                .relation_type
                .parse()
                .unwrap_or(RelationType::RelatedTo),
            context: self.context.clone(),
        }
    }
}

// ============================================================================
// FreshnessStorage Trait Implementation
// ============================================================================

#[async_trait]
impl FreshnessStorage for RealRocksDBStorage {
    async fn register_source(&self, _session_id: uuid::Uuid, reference: SourceReference) -> Result<()> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let key = format!("source_ref:{}", reference.entry_id);
            use prost::Message;
            let mut data = Vec::with_capacity(reference.encoded_len());
            reference.encode(&mut data)?;
            db.put(key.as_bytes(), data)?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;
        Ok(())
    }

    async fn check_freshness(
        &self,
        entry_id: &str,
        file_hash: &[u8],
    ) -> Result<crate::daemon::grpc_service::pb::FreshnessEntry> {
        let db = self.db.clone();
        let entry_id = entry_id.to_string();
        let current_hash = file_hash.to_vec();

        tokio::task::spawn_blocking(move || -> Result<crate::daemon::grpc_service::pb::FreshnessEntry> {
            let key = format!("source_ref:{}", entry_id);
            match db.get(key.as_bytes())? {
                Some(data) => {
                    let reference = SourceReference::decode(&data[..])
                            .map_err(|e| anyhow::anyhow!("Failed to deserialize source reference: {}", e))?;
                    
                    let is_fresh = reference.content_hash == current_hash;

                    let status = if is_fresh {
                        crate::daemon::grpc_service::pb::FreshnessStatus::Fresh as i32
                    } else {
                        crate::daemon::grpc_service::pb::FreshnessStatus::Stale as i32
                    };

                    Ok(crate::daemon::grpc_service::pb::FreshnessEntry {
                        entry_id,
                        file_path: reference.file_path,
                        status,
                        stored_hash: reference.content_hash,
                        current_hash,
                    })
                }
                None => {
                    // Entry has no source tracking
                    Ok(crate::daemon::grpc_service::pb::FreshnessEntry {
                        entry_id,
                        file_path: String::new(),
                        status: crate::daemon::grpc_service::pb::FreshnessStatus::Unknown as i32,
                        stored_hash: Vec::new(),
                        current_hash,
                    })
                }
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    async fn invalidate_source(&self, file_path: &str) -> Result<u32> {
        let db = self.db.clone();
        let query_path = file_path.to_string();

        tokio::task::spawn_blocking(move || -> Result<u32> {
            let mut count = 0;
            let iter = db.iterator(rocksdb::IteratorMode::From(
                b"source_ref:",
                rocksdb::Direction::Forward,
            ));

            let mut keys_to_delete = Vec::new();

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with("source_ref:") {
                    break;
                }

                use prost::Message;
                if let Ok(reference) = SourceReference::decode(&value[..]) {
                    if reference.file_path == query_path {
                        keys_to_delete.push(key.to_vec());
                    }
                }
            }

            for key in keys_to_delete {
                db.delete(&key)?;
                count += 1;
            }

            Ok(count)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    async fn get_entries_by_source(&self, file_path: &str) -> Result<Vec<crate::daemon::grpc_service::pb::SourceReference>> {
        let db = self.db.clone();
        let query_path = file_path.to_string();

        tokio::task::spawn_blocking(move || -> Result<Vec<crate::daemon::grpc_service::pb::SourceReference>> {
            let mut matches = Vec::new();
            let iter = db.iterator(rocksdb::IteratorMode::From(
                b"source_ref:",
                rocksdb::Direction::Forward,
            ));

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with("source_ref:") {
                    break;
                }

                if let Ok(reference) = SourceReference::decode(&value[..]) {
                    if reference.file_path == query_path {
                        matches.push(reference);
                    }
                }
            }

            Ok(matches)
        })
        .await
        .map_err(|e: tokio::task::JoinError| anyhow::anyhow!("Task join error: {}", e))?
    }

    async fn check_freshness_semantic(
        &self,
        entry_id: &str,
        file_hash: &[u8],
        ast_hash: Option<&[u8]>,
        _symbol_name: Option<&str>,
    ) -> Result<crate::daemon::grpc_service::pb::FreshnessEntry> {
        let db = self.db.clone();
        let entry_id = entry_id.to_string();
        let current_file_hash = file_hash.to_vec();
        let current_ast_hash = ast_hash.map(|h| h.to_vec());

        tokio::task::spawn_blocking(move || -> Result<crate::daemon::grpc_service::pb::FreshnessEntry> {
            let key = format!("source_ref:{}", entry_id);
            match db.get(key.as_bytes())? {
                Some(data) => {
                    use prost::Message;
                    let reference = SourceReference::decode(&data[..])
                        .map_err(|e| anyhow::anyhow!("Failed to deserialize source reference: {}", e))?;

                    // Semantic freshness: prefer ast_hash comparison when both sides have it
                    let is_fresh = if let (Some(client_ast), Some(scope)) =
                        (&current_ast_hash, &reference.scope)
                    {
                        use crate::daemon::grpc_service::pb::source_scope::Scope;
                        match &scope.scope {
                            Some(Scope::Function(func)) if !func.ast_hash.is_empty() => {
                                // AST-level: compare function body hashes
                                func.ast_hash == *client_ast
                            }
                            _ => reference.content_hash == current_file_hash,
                        }
                    } else {
                        // Fallback: file-level hash comparison
                        reference.content_hash == current_file_hash
                    };

                    let status = if is_fresh {
                        crate::daemon::grpc_service::pb::FreshnessStatus::Fresh as i32
                    } else {
                        crate::daemon::grpc_service::pb::FreshnessStatus::Stale as i32
                    };

                    Ok(crate::daemon::grpc_service::pb::FreshnessEntry {
                        entry_id,
                        file_path: reference.file_path,
                        status,
                        stored_hash: reference.content_hash,
                        current_hash: current_file_hash,
                    })
                }
                None => Ok(crate::daemon::grpc_service::pb::FreshnessEntry {
                    entry_id,
                    file_path: String::new(),
                    status: crate::daemon::grpc_service::pb::FreshnessStatus::Unknown as i32,
                    stored_hash: Vec::new(),
                    current_hash: current_file_hash,
                }),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    async fn register_symbol_dependencies(
        &self,
        from: crate::daemon::grpc_service::pb::SymbolId,
        to_symbols: Vec<crate::daemon::grpc_service::pb::SymbolId>,
    ) -> Result<u32> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<u32> {
            let mut count = 0u32;
            let from_key = format!("{}::{}", from.file_path, from.symbol_name);

            for to in &to_symbols {
                let to_key = if to.file_path.is_empty() {
                    // Name-only dependency (file unknown)
                    format!("::{}", to.symbol_name)
                } else {
                    format!("{}::{}", to.file_path, to.symbol_name)
                };
                // Forward edge: symbol_dep:{from} -> {to}
                let fwd = format!("symbol_dep:{}|{}", from_key, to_key);
                db.put(fwd.as_bytes(), to.symbol_type.as_bytes())?;
                // Reverse edge: symbol_rdep:{to} -> {from} (for cascade lookup)
                let rev = format!("symbol_rdep:{}|{}", to_key, from_key);
                db.put(rev.as_bytes(), from.symbol_type.as_bytes())?;

                // Also store name-only reverse edge if file is known,
                // so cascade can match even when dependency was registered without file
                if !to.file_path.is_empty() {
                    let name_only_rev = format!("symbol_rdep:::{}|{}", to.symbol_name, from_key);
                    db.put(name_only_rev.as_bytes(), from.symbol_type.as_bytes())?;
                }
                count += 1;
            }

            Ok(count)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    async fn cascade_invalidate(
        &self,
        changed: crate::daemon::grpc_service::pb::SymbolId,
        _new_ast_hash: Vec<u8>,
        max_depth: u32,
    ) -> Result<crate::daemon::grpc_service::pb::CascadeInvalidateReport> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<crate::daemon::grpc_service::pb::CascadeInvalidateReport> {
            use prost::Message;
            use std::collections::{HashSet, VecDeque};

            let changed_key = format!("{}::{}", changed.file_path, changed.symbol_name);

            // BFS: find all symbols that depend on the changed symbol (reverse edges)
            let mut visited = HashSet::new();
            let mut queue = VecDeque::new();
            queue.push_back((changed_key.clone(), 0u32));
            visited.insert(changed_key.clone());

            let mut dependent_symbols = Vec::new();

            while let Some((sym_key, depth)) = queue.pop_front() {
                if depth > 0 {
                    dependent_symbols.push(sym_key.clone());
                }
                if depth >= max_depth {
                    continue;
                }

                // Find reverse deps: who depends on sym_key?
                // Search both full key (file::symbol) and name-only (::symbol)
                let sym_name = sym_key.split("::").last().unwrap_or(&sym_key);
                let prefixes = [
                    format!("symbol_rdep:{}|", sym_key),
                    format!("symbol_rdep:::{}|", sym_name),
                ];

                for prefix in &prefixes {
                    let iter = db.iterator(rocksdb::IteratorMode::From(
                        prefix.as_bytes(),
                        rocksdb::Direction::Forward,
                    ));

                    for item in iter {
                        let (key, _) = item?;
                        let key_str = String::from_utf8_lossy(&key);
                        if !key_str.starts_with(prefix.as_str()) {
                            break;
                        }
                        let dep_key = key_str[prefix.len()..].to_string();
                        if visited.insert(dep_key.clone()) {
                            queue.push_back((dep_key, depth + 1));
                        }
                    }
                }
            }

            // Invalidate source_ref entries for the changed symbol's file
            let mut direct_count = 0u32;
            let mut cascade_count = 0u32;

            // Direct: invalidate entries for the changed symbol's file
            let source_iter = db.iterator(rocksdb::IteratorMode::From(
                b"source_ref:",
                rocksdb::Direction::Forward,
            ));
            let mut keys_to_delete = Vec::new();

            for item in source_iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);
                if !key_str.starts_with("source_ref:") {
                    break;
                }

                if let Ok(reference) = SourceReference::decode(&value[..]) {
                    if let Some(ref scope) = reference.scope {
                        use crate::daemon::grpc_service::pb::source_scope::Scope;
                        if let Some(Scope::Function(ref func)) = scope.scope {
                            let ref_key = format!("{}::{}", reference.file_path, func.name);
                            if ref_key == changed_key {
                                keys_to_delete.push(key.to_vec());
                                direct_count += 1;
                            } else if dependent_symbols.contains(&ref_key) {
                                keys_to_delete.push(key.to_vec());
                                cascade_count += 1;
                            }
                        }
                    }
                }
            }

            for key in keys_to_delete {
                db.delete(&key)?;
            }

            Ok(crate::daemon::grpc_service::pb::CascadeInvalidateReport {
                direct_invalidations: direct_count,
                cascade_invalidations: cascade_count,
                invalidated_symbols: dependent_symbols,
            })
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }
}


/// Real RocksDB storage implementation for high performance
///
/// Uses a hybrid approach:
/// - RocksDB for persistent storage on disk
/// - In-memory VectorDB with HNSW index for O(log n) vector search
#[derive(Clone)]
pub struct RealRocksDBStorage {
    db: Arc<DB>,
    #[allow(dead_code)]
    data_dir: PathBuf,
    /// In-memory HNSW index for fast vector search
    vector_index: Arc<VectorDB>,
}

impl RealRocksDBStorage {
    /// Create new RocksDB storage instance
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let data_dir = path.as_ref().to_path_buf();

        // Create directory if it doesn't exist
        if !data_dir.exists() {
            std::fs::create_dir_all(&data_dir)?;
        }

        let db_path = data_dir.join("rocksdb");

        // Configure RocksDB options for balanced performance and safety
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        // Memory settings - balanced for daemon use (max ~512MB for write buffers)
        opts.set_max_open_files(1000); // Reduced from 10000 - most systems have lower limits
        opts.set_write_buffer_size(64 * 1024 * 1024); // 64MB per buffer (was 512MB)
        opts.set_max_write_buffer_number(6); // Max 384MB for write buffers (was 32 = 16GB!)
        opts.set_min_write_buffer_number_to_merge(2); // Merge sooner (was 4)
        opts.set_target_file_size_base(64 * 1024 * 1024); // 64MB SST files (was 1GB)

        // Write throttling - prevent memory exhaustion
        opts.set_level_zero_slowdown_writes_trigger(20); // Enable throttling (was 0 = disabled)
        opts.set_level_zero_stop_writes_trigger(36); // Reduced from 2000

        // Durability settings - balanced for daemon use
        opts.set_use_fsync(false); // fdatasync is sufficient for most cases
        opts.set_bytes_per_sync(1024 * 1024); // Sync every 1MB (was 0 = never)

        // Compaction settings
        opts.set_compaction_style(rocksdb::DBCompactionStyle::Universal);

        // Enable prefix bloom filter (16 bytes)
        // NOTE: prefix_iterator() only works correctly when the search prefix is >= 16 bytes.
        // For shorter prefixes like "session:" (8 bytes), use iterator() with IteratorMode::From instead.
        opts.set_prefix_extractor(rocksdb::SliceTransform::create_fixed_prefix(16));

        let db = DB::open(&opts, &db_path)?;
        let db = Arc::new(db);

        info!(
            "RealRocksDBStorage: Initialized RocksDB at {}",
            db_path.display()
        );

        // Create in-memory HNSW index for fast vector search
        let vector_config = VectorDbConfig {
            dimension: EMBEDDING_DIMENSION,
            enable_hnsw_index: true,
            max_connections: 16,
            num_layers: 4,
            ..Default::default()
        };
        let vector_index = Arc::new(VectorDB::new(vector_config)?);

        let storage = Self {
            db: db.clone(),
            data_dir,
            vector_index,
        };

        // Load existing embeddings from RocksDB into the in-memory HNSW index
        storage.rebuild_hnsw_index().await?;

        Ok(storage)
    }

    /// Rebuild the in-memory HNSW index from RocksDB embeddings using batch insertion
    async fn rebuild_hnsw_index(&self) -> Result<()> {
        let embeddings = self.load_all_embeddings().await?;
        let count = embeddings.len();

        if count == 0 {
            info!("RealRocksDBStorage: No embeddings to load into HNSW index");
            return Ok(());
        }

        info!(
            "RealRocksDBStorage: Loading {} embeddings into HNSW index...",
            count
        );

        let start = std::time::Instant::now();
        const BATCH_SIZE: usize = 100;
        let mut loaded = 0;

        for chunk in embeddings.chunks(BATCH_SIZE) {
            let batch: Vec<(Vec<f32>, _)> = chunk
                .iter()
                .map(|e| (e.vector.clone(), e.to_metadata()))
                .collect();

            match self.vector_index.add_vectors_batch(batch) {
                Ok(ids) => loaded += ids.len(),
                Err(e) => {
                    tracing::warn!("Failed to add embedding batch to HNSW index: {}", e);
                }
            }

            if loaded % 500 == 0 && loaded > 0 {
                debug!(
                    "RealRocksDBStorage: HNSW rebuild progress: {}/{} vectors",
                    loaded, count
                );
            }
        }

        let elapsed = start.elapsed();
        info!(
            "RealRocksDBStorage: HNSW index ready with {} vectors (rebuilt in {:.1}ms)",
            self.vector_index.len(),
            elapsed.as_secs_f64() * 1000.0
        );

        Ok(())
    }

    /// Save session to RocksDB
    pub async fn save_session(&self, session: &ActiveSession) -> Result<()> {
        info!(
            "RealRocksDBStorage: Saving session with ID: {}",
            session.id()
        );

        let db = self.db.clone();
        let session = session.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            // Serialize session data with serde_json (bincode doesn't support deserialize_any)
            let session_data = serde_json::to_vec(&session)
                .map_err(|e| anyhow::anyhow!("Failed to serialize session: {}", e))?;

            info!(
                "RealRocksDBStorage: Session data serialized, size: {} bytes",
                session_data.len()
            );

            // Use binary key for better performance
            let key = format!("session:{}", session.id());

            // Write to RocksDB
            db.put(key.as_bytes(), &session_data)?;

            info!("RealRocksDBStorage: Session saved to RocksDB");
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Load session from RocksDB
    pub async fn load_session(&self, session_id: Uuid) -> Result<ActiveSession> {
        info!(
            "RealRocksDBStorage: Loading session with ID: {}",
            session_id
        );

        let db = self.db.clone();
        let key = format!("session:{}", session_id);

        tokio::task::spawn_blocking(move || -> Result<ActiveSession> {
            match db.get(key.as_bytes())? {
                Some(data) => {
                    info!(
                        "RealRocksDBStorage: Session data found, size: {} bytes",
                        data.len()
                    );

                    let session: ActiveSession = serde_json::from_slice(&data)
                        .map_err(|e| anyhow::anyhow!("Failed to deserialize session: {}", e))?;

                    info!("RealRocksDBStorage: Session deserialized successfully");
                    Ok(session)
                }
                None => {
                    info!("RealRocksDBStorage: Session not found");
                    Err(anyhow::anyhow!("Session not found"))
                }
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Save checkpoint to RocksDB
    pub async fn save_checkpoint(&self, checkpoint: &SessionCheckpoint) -> Result<()> {
        let db = self.db.clone();
        let checkpoint = checkpoint.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let checkpoint_data =
                bincode::serde::encode_to_vec(&checkpoint, bincode::config::standard())
                    .map_err(|e| anyhow::anyhow!("Failed to serialize checkpoint: {}", e))?;

            let key = format!("checkpoint:{}", checkpoint.id);
            db.put(key.as_bytes(), &checkpoint_data)?;

            info!(
                "RealRocksDBStorage: Checkpoint saved with ID: {}",
                checkpoint.id
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Load checkpoint from RocksDB
    pub async fn load_checkpoint(&self, checkpoint_id: Uuid) -> Result<SessionCheckpoint> {
        let db = self.db.clone();
        let key = format!("checkpoint:{}", checkpoint_id);

        tokio::task::spawn_blocking(move || -> Result<SessionCheckpoint> {
            match db.get(key.as_bytes())? {
                Some(data) => {
                    let (checkpoint, _): (SessionCheckpoint, usize) =
                        bincode::serde::decode_from_slice(&data, bincode::config::standard())
                            .map_err(|e| {
                                anyhow::anyhow!("Failed to deserialize checkpoint: {}", e)
                            })?;

                    info!(
                        "RealRocksDBStorage: Checkpoint loaded with ID: {}",
                        checkpoint_id
                    );
                    Ok(checkpoint)
                }
                None => Err(anyhow::anyhow!("Checkpoint not found")),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Batch save multiple updates efficiently
    pub async fn batch_save_updates(
        &self,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        let db = self.db.clone();
        let updates_len = updates.len();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut batch = WriteBatch::default();

            for update in &updates {
                let key = format!("session:{}:update:{}", session_id, update.id);
                let update_data =
                    bincode::serde::encode_to_vec(update, bincode::config::standard())
                        .map_err(|e| anyhow::anyhow!("Failed to serialize update: {}", e))?;

                batch.put(key.as_bytes(), &update_data);
            }

            db.write(batch)?;

            info!(
                "RealRocksDBStorage: Batch saved {} updates for session {}",
                updates_len, session_id
            );

            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Save session blob and context updates in a single atomic WriteBatch.
    /// Combines save_session + batch_save_updates into one spawn_blocking call.
    pub async fn save_session_with_updates(
        &self,
        session: &ActiveSession,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        let db = self.db.clone();
        let session = session.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut batch = rocksdb::WriteBatch::default();

            // Session blob (serde_json — required due to serde_json::Value fields)
            let session_key = format!("session:{}", session.id());
            let session_data = serde_json::to_vec(&session)
                .map_err(|e| anyhow::anyhow!("Failed to serialize session: {}", e))?;
            batch.put(session_key.as_bytes(), &session_data);

            // Context update records (bincode)
            for update in &updates {
                let update_key = format!("session:{}:update:{}", session_id, update.id);
                let update_data =
                    bincode::serde::encode_to_vec(update, bincode::config::standard())
                        .map_err(|e| anyhow::anyhow!("Failed to serialize update: {}", e))?;
                batch.put(update_key.as_bytes(), &update_data);
            }

            db.write(batch)?;

            debug!(
                "RealRocksDBStorage: Combined save - session {} + {} updates ({} bytes)",
                session_id,
                updates.len(),
                session_data.len()
            );

            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// List all session IDs
    pub async fn list_sessions(&self) -> Result<Vec<Uuid>> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<Vec<Uuid>> {
            let mut sessions = Vec::new();

            // Use iterator with seek - prefix_iterator doesn't work correctly
            // when prefix_extractor size (16 bytes) doesn't match our prefix (8 bytes "session:")
            let iter = db.iterator(rocksdb::IteratorMode::From(
                b"session:",
                rocksdb::Direction::Forward,
            ));
            for item in iter {
                let (key, _) = item?;
                let key_str = String::from_utf8_lossy(&key);

                // Stop when we've passed the "session:" prefix
                if !key_str.starts_with("session:") {
                    break;
                }

                // Skip update keys (format: "session:{id}:update:{update_id}")
                if key_str.contains(":update:") {
                    continue;
                }

                // Extract UUID from "session:{uuid}" format
                if let Some(uuid_str) = key_str.strip_prefix("session:")
                    && let Ok(uuid) = Uuid::parse_str(uuid_str)
                {
                    sessions.push(uuid);
                }
            }

            info!("RealRocksDBStorage: Found {} sessions", sessions.len());
            Ok(sessions)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Get database statistics
    pub async fn get_stats(&self) -> Result<String> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<String> {
            let stats = db
                .property_value(rocksdb::properties::STATS)?
                .unwrap_or_else(|| "No stats available".to_string());

            Ok(stats)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Compact database
    /// Force database compaction
    pub async fn compact(&self) -> Result<()> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            db.compact_range(None::<&[u8]>, None::<&[u8]>);
            info!("RealRocksDBStorage: Database compacted");
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Delete session and all related data
    pub async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut batch = WriteBatch::default();

            // Delete main session
            let session_key = format!("session:{}", session_id);
            batch.delete(session_key.as_bytes());

            // Delete all updates for this session
            // Key format: "session:{session_id}:update:{update_id}"
            let update_prefix = format!("session:{}:update:", session_id);
            let iter = db.iterator(rocksdb::IteratorMode::From(
                update_prefix.as_bytes(),
                rocksdb::Direction::Forward,
            ));
            let mut keys_to_delete = Vec::new();

            for item in iter {
                let (key, _) = item?;
                let key_str = String::from_utf8_lossy(&key);

                // Stop when we've passed our prefix (prefix_iterator may continue)
                if !key_str.starts_with(&update_prefix) {
                    break;
                }
                keys_to_delete.push(key.to_vec());
            }

            // Batch delete all related keys
            for key in keys_to_delete {
                batch.delete(&key);
            }
            db.write(batch)?;

            info!(
                "RealRocksDBStorage: Deleted session {} and related data",
                session_id
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Get estimated number of keys in database (O(1) using RocksDB property)
    pub async fn get_key_count(&self) -> Result<usize> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<usize> {
            // Use RocksDB's estimate-num-keys property for O(1) performance
            // This is an approximation but avoids full table scan
            if let Some(count_str) = db.property_value(rocksdb::properties::ESTIMATE_NUM_KEYS)? {
                if let Ok(count) = count_str.parse::<usize>() {
                    return Ok(count);
                }
            }

            // Fallback to counting if property not available (shouldn't happen)
            let mut count = 0;
            let iter = db.iterator(rocksdb::IteratorMode::Start);
            for item in iter {
                let _ = item?;
                count += 1;
            }

            Ok(count)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Check if session exists
    pub async fn session_exists(&self, session_id: Uuid) -> Result<bool> {
        let db = self.db.clone();
        let key = format!("session:{}", session_id);

        tokio::task::spawn_blocking(move || -> Result<bool> {
            Ok(db.get(key.as_bytes())?.is_some())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    // ========================================================================
    // Workspace Persistence Methods
    // ========================================================================

    /// List all workspaces with their sessions (hydrated from ws_session records)
    pub async fn list_workspaces(&self) -> Result<Vec<StoredWorkspace>> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<Vec<StoredWorkspace>> {
            let mut workspaces: HashMap<Uuid, StoredWorkspace> = HashMap::new();

            // First pass: Load all workspaces using iterator with seek
            // (prefix_iterator doesn't work with our 16-byte prefix extractor)
            let workspace_iter = db.iterator(rocksdb::IteratorMode::From(
                b"workspace:",
                rocksdb::Direction::Forward,
            ));
            for item in workspace_iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                // Stop when we've passed the "workspace:" prefix
                if !key_str.starts_with("workspace:") {
                    break;
                }

                if let Ok((mut workspace, _)) =
                    bincode::serde::decode_from_slice::<StoredWorkspace, _>(
                        &value,
                        bincode::config::standard(),
                    )
                {
                    // Clear stored sessions as they might be stale (source of truth is ws_session records)
                    workspace.sessions.clear();
                    workspaces.insert(workspace.id, workspace);
                }
            }

            // Second pass: Load all workspace-session associations using iterator with seek
            let ws_session_iter = db.iterator(rocksdb::IteratorMode::From(
                b"ws_session:",
                rocksdb::Direction::Forward,
            ));
            for item in ws_session_iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                // Stop when we've passed the "ws_session:" prefix
                if !key_str.starts_with("ws_session:") {
                    break;
                }

                if let Ok((ws_session, _)) = bincode::serde::decode_from_slice::<
                    StoredWorkspaceSession,
                    _,
                >(&value, bincode::config::standard())
                {
                    if let Some(ws) = workspaces.get_mut(&ws_session.workspace_id) {
                        ws.sessions.push((ws_session.session_id, ws_session.role));
                    }
                }
            }

            Ok(workspaces.into_values().collect())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Save workspace metadata to RocksDB
    pub async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> Result<()> {
        use crate::workspace::SessionRole;

        info!(
            "RealRocksDBStorage: Saving workspace {} ({})",
            name, workspace_id
        );

        let db = self.db.clone();
        let name = name.to_string();
        let description = description.to_string();
        let session_ids = session_ids.to_vec();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let workspace_data = StoredWorkspace {
                id: workspace_id,
                name,
                description,
                sessions: session_ids
                    .iter()
                    .map(|id| (*id, SessionRole::Primary))
                    .collect(),
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            };

            let data = bincode::serde::encode_to_vec(&workspace_data, bincode::config::standard())
                .map_err(|e| anyhow::anyhow!("Failed to serialize workspace: {}", e))?;

            let key = format!("workspace:{}", workspace_id);
            db.put(key.as_bytes(), data)?;

            info!(
                "RealRocksDBStorage: Workspace {} saved successfully",
                workspace_id
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Delete workspace from RocksDB
    pub async fn delete_workspace(&self, workspace_id: Uuid) -> Result<()> {
        info!("RealRocksDBStorage: Deleting workspace {}", workspace_id);

        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let key = format!("workspace:{}", workspace_id);
            db.delete(key.as_bytes())?;

            // Also delete all workspace-session associations
            let ws_session_prefix = format!("ws_session:{}:", workspace_id);
            let iter = db.iterator(rocksdb::IteratorMode::From(
                ws_session_prefix.as_bytes(),
                rocksdb::Direction::Forward,
            ));
            let mut keys_to_delete = Vec::new();

            for item in iter {
                let (key, _) = item?;
                let key_str = String::from_utf8_lossy(&key);

                // Stop when we've passed our prefix
                if !key_str.starts_with(&ws_session_prefix) {
                    break;
                }
                keys_to_delete.push(key.to_vec());
            }

            let mut batch = WriteBatch::default();
            for key in keys_to_delete {
                batch.delete(&key);
            }
            db.write(batch)?;

            info!(
                "RealRocksDBStorage: Workspace {} deleted successfully",
                workspace_id
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Add session to workspace association
    pub async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: crate::workspace::SessionRole,
    ) -> Result<()> {
        info!(
            "RealRocksDBStorage: Adding session {} to workspace {} with role {:?}",
            session_id, workspace_id, role
        );

        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let ws_session = StoredWorkspaceSession {
                workspace_id,
                session_id,
                role,
                added_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            };

            let data = bincode::serde::encode_to_vec(&ws_session, bincode::config::standard())
                .map_err(|e| anyhow::anyhow!("Failed to serialize workspace-session: {}", e))?;

            let key = format!("ws_session:{}:{}", workspace_id, session_id);
            db.put(key.as_bytes(), data)?;

            info!(
                "RealRocksDBStorage: Session {} added to workspace {} successfully",
                session_id, workspace_id
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Remove session from workspace association
    pub async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<()> {
        info!(
            "RealRocksDBStorage: Removing session {} from workspace {}",
            session_id, workspace_id
        );

        let db = self.db.clone();
        let key = format!("ws_session:{}:{}", workspace_id, session_id);

        tokio::task::spawn_blocking(move || -> Result<()> {
            db.delete(key.as_bytes())?;

            info!(
                "RealRocksDBStorage: Session {} removed from workspace {} successfully",
                session_id, workspace_id
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    // ========================================================================
    // Export/Import Support Methods
    // ========================================================================

    /// Load all updates for a session
    pub async fn load_session_updates(&self, session_id: Uuid) -> Result<Vec<ContextUpdate>> {
        let db = self.db.clone();
        let update_prefix = format!("session:{}:update:", session_id);

        tokio::task::spawn_blocking(move || -> Result<Vec<ContextUpdate>> {
            let mut updates = Vec::new();
            let iter = db.iterator(rocksdb::IteratorMode::From(
                update_prefix.as_bytes(),
                rocksdb::Direction::Forward,
            ));

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with(&update_prefix) {
                    break;
                }

                if let Ok((update, _)) = bincode::serde::decode_from_slice::<ContextUpdate, _>(
                    &value,
                    bincode::config::standard(),
                ) {
                    updates.push(update);
                }
            }

            info!(
                "RealRocksDBStorage: Loaded {} updates for session {}",
                updates.len(),
                session_id
            );
            Ok(updates)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// List all checkpoints
    pub async fn list_checkpoints(&self) -> Result<Vec<SessionCheckpoint>> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<Vec<SessionCheckpoint>> {
            let mut checkpoints = Vec::new();
            let iter = db.iterator(rocksdb::IteratorMode::From(
                b"checkpoint:",
                rocksdb::Direction::Forward,
            ));

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with("checkpoint:") {
                    break;
                }

                if let Ok((checkpoint, _)) =
                    bincode::serde::decode_from_slice::<SessionCheckpoint, _>(
                        &value,
                        bincode::config::standard(),
                    )
                {
                    checkpoints.push(checkpoint);
                }
            }

            info!("RealRocksDBStorage: Listed {} checkpoints", checkpoints.len());
            Ok(checkpoints)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }
}

// ============================================================================
// Storage Trait Implementation
// ============================================================================

#[async_trait]
impl Storage for RealRocksDBStorage {
    async fn save_session(&self, session: &ActiveSession) -> Result<()> {
        RealRocksDBStorage::save_session(self, session).await
    }

    async fn load_session(&self, session_id: Uuid) -> Result<ActiveSession> {
        RealRocksDBStorage::load_session(self, session_id).await
    }

    async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        RealRocksDBStorage::delete_session(self, session_id).await
    }

    async fn list_sessions(&self) -> Result<Vec<Uuid>> {
        RealRocksDBStorage::list_sessions(self).await
    }

    async fn session_exists(&self, session_id: Uuid) -> Result<bool> {
        RealRocksDBStorage::session_exists(self, session_id).await
    }

    async fn batch_save_updates(
        &self,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        RealRocksDBStorage::batch_save_updates(self, session_id, updates).await
    }

    async fn save_session_with_updates(
        &self,
        session: &ActiveSession,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        RealRocksDBStorage::save_session_with_updates(self, session, session_id, updates).await
    }

    async fn load_session_updates(&self, session_id: Uuid) -> Result<Vec<ContextUpdate>> {
        RealRocksDBStorage::load_session_updates(self, session_id).await
    }

    async fn save_checkpoint(&self, checkpoint: &SessionCheckpoint) -> Result<()> {
        RealRocksDBStorage::save_checkpoint(self, checkpoint).await
    }

    async fn load_checkpoint(&self, checkpoint_id: Uuid) -> Result<SessionCheckpoint> {
        RealRocksDBStorage::load_checkpoint(self, checkpoint_id).await
    }

    async fn list_checkpoints(&self) -> Result<Vec<SessionCheckpoint>> {
        RealRocksDBStorage::list_checkpoints(self).await
    }

    async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> Result<()> {
        RealRocksDBStorage::save_workspace_metadata(
            self,
            workspace_id,
            name,
            description,
            session_ids,
        )
        .await
    }

    async fn delete_workspace(&self, workspace_id: Uuid) -> Result<()> {
        RealRocksDBStorage::delete_workspace(self, workspace_id).await
    }

    async fn list_workspaces(&self) -> Result<Vec<StoredWorkspace>> {
        RealRocksDBStorage::list_workspaces(self).await
    }

    async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: SessionRole,
    ) -> Result<()> {
        RealRocksDBStorage::add_session_to_workspace(self, workspace_id, session_id, role).await
    }

    async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<()> {
        RealRocksDBStorage::remove_session_from_workspace(self, workspace_id, session_id).await
    }

    async fn compact(&self) -> Result<()> {
        RealRocksDBStorage::compact(self).await
    }

    async fn get_key_count(&self) -> Result<usize> {
        RealRocksDBStorage::get_key_count(self).await
    }

    async fn get_stats(&self) -> Result<String> {
        RealRocksDBStorage::get_stats(self).await
    }

    async fn clear_session_entities(&self, session_id: Uuid) -> Result<()> {
        let entities = self.load_session_entities(session_id).await?;
        for entity in &entities {
            self.delete_stored_entity(session_id, &entity.name).await?;
        }
        Ok(())
    }
}

// ============================================================================
// VectorStorage Trait Implementation - Embedding Persistence
// ============================================================================

/// Stored embedding record for RocksDB persistence
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredEmbedding {
    pub content_id: String,
    pub session_id: String,
    pub vector: Vec<f32>,
    pub text: String,
    pub content_type: String,
    pub timestamp: DateTime<Utc>,
    pub metadata: HashMap<String, String>,
}

impl StoredEmbedding {
    /// Create from vector and metadata
    pub fn new(vector: Vec<f32>, metadata: VectorMetadata) -> Self {
        Self {
            content_id: metadata.id,
            session_id: metadata.source,
            vector,
            text: metadata.text,
            content_type: metadata.content_type,
            timestamp: metadata.timestamp,
            metadata: metadata.metadata,
        }
    }

    /// Convert to VectorMetadata
    pub fn to_metadata(&self) -> VectorMetadata {
        VectorMetadata {
            id: self.content_id.clone(),
            text: self.text.clone(),
            source: self.session_id.clone(),
            content_type: self.content_type.clone(),
            timestamp: self.timestamp,
            metadata: self.metadata.clone(),
        }
    }
}

impl RealRocksDBStorage {
    /// Save an embedding to RocksDB
    pub async fn save_embedding(&self, embedding: &StoredEmbedding) -> Result<()> {
        let db = self.db.clone();
        let embedding = embedding.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let key = format!(
                "embedding:{}:{}",
                embedding.session_id, embedding.content_id
            );
            let data = bincode::serde::encode_to_vec(&embedding, bincode::config::standard())
                .map_err(|e| anyhow::anyhow!("Failed to serialize embedding: {}", e))?;
            db.put(key.as_bytes(), &data)?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Load all embeddings for a session
    pub async fn load_session_embeddings(&self, session_id: &str) -> Result<Vec<StoredEmbedding>> {
        let db = self.db.clone();
        let prefix = format!("embedding:{}:", session_id);

        tokio::task::spawn_blocking(move || -> Result<Vec<StoredEmbedding>> {
            let mut embeddings = Vec::new();
            let iter = db.iterator(rocksdb::IteratorMode::From(
                prefix.as_bytes(),
                rocksdb::Direction::Forward,
            ));

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with(&prefix) {
                    break;
                }

                if let Ok((embedding, _)) = bincode::serde::decode_from_slice::<StoredEmbedding, _>(
                    &value,
                    bincode::config::standard(),
                ) {
                    embeddings.push(embedding);
                }
            }

            Ok(embeddings)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Load all embeddings from storage
    /// Used for HNSW index rebuild and migration operations
    pub async fn load_all_embeddings(&self) -> Result<Vec<StoredEmbedding>> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<Vec<StoredEmbedding>> {
            let mut embeddings = Vec::new();
            let iter = db.iterator(rocksdb::IteratorMode::From(
                b"embedding:",
                rocksdb::Direction::Forward,
            ));

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with("embedding:") {
                    break;
                }

                if let Ok((embedding, _)) = bincode::serde::decode_from_slice::<StoredEmbedding, _>(
                    &value,
                    bincode::config::standard(),
                ) {
                    embeddings.push(embedding);
                }
            }

            Ok(embeddings)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Delete an embedding
    pub async fn delete_embedding(&self, session_id: &str, content_id: &str) -> Result<bool> {
        let db = self.db.clone();
        let key = format!("embedding:{}:{}", session_id, content_id);

        tokio::task::spawn_blocking(move || -> Result<bool> {
            let existed = db.get(key.as_bytes())?.is_some();
            db.delete(key.as_bytes())?;
            Ok(existed)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Count embeddings for a session
    pub async fn count_embeddings(&self, session_id: &str) -> usize {
        self.load_session_embeddings(session_id)
            .await
            .map(|e| e.len())
            .unwrap_or(0)
    }
}

#[async_trait]
impl VectorStorage for RealRocksDBStorage {
    async fn add_vector(&self, vector: Vec<f32>, metadata: VectorMetadata) -> Result<String> {
        // Validate embedding dimension for consistency with SurrealDB backend
        if vector.len() != EMBEDDING_DIMENSION {
            return Err(anyhow::anyhow!(
                "Invalid embedding dimension: expected {}, got {}",
                EMBEDDING_DIMENSION,
                vector.len()
            ));
        }
        let id = metadata.id.clone();

        // Add to in-memory HNSW index for fast search
        self.vector_index
            .add_vector(vector.clone(), metadata.clone())?;

        // Persist to RocksDB for durability
        let embedding = StoredEmbedding::new(vector, metadata);
        self.save_embedding(&embedding).await?;

        Ok(id)
    }

    async fn add_vectors_batch(
        &self,
        vectors: Vec<(Vec<f32>, VectorMetadata)>,
    ) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for (vector, metadata) in vectors {
            let id = self.add_vector(vector, metadata).await?;
            ids.push(id);
        }
        Ok(ids)
    }

    /// Search using HNSW index - O(log n) complexity
    async fn search(&self, query: &[f32], k: usize) -> Result<Vec<SearchMatch>> {
        // Use HNSW index for fast approximate nearest neighbor search
        self.vector_index.search(query, k)
    }

    /// Search within a session using HNSW with post-filtering
    async fn search_in_session(
        &self,
        query: &[f32],
        k: usize,
        session_id: &str,
    ) -> Result<Vec<SearchMatch>> {
        // Use HNSW with post-filtering: fetch more results, filter by session
        // This matches SurrealDB's approach for filtered queries
        let fetch_multiplier = 5;
        let results = self.vector_index.search(query, k * fetch_multiplier)?;

        // Filter by session and take top k
        let filtered: Vec<SearchMatch> = results
            .into_iter()
            .filter(|m| m.metadata.source == session_id)
            .take(k)
            .collect();

        Ok(filtered)
    }

    /// Search by content type using HNSW with post-filtering
    async fn search_by_content_type(
        &self,
        query: &[f32],
        k: usize,
        content_type: &str,
    ) -> Result<Vec<SearchMatch>> {
        // Use HNSW with post-filtering by content type
        let fetch_multiplier = 5;
        let results = self.vector_index.search(query, k * fetch_multiplier)?;

        // Filter by content type and take top k
        let filtered: Vec<SearchMatch> = results
            .into_iter()
            .filter(|m| m.metadata.content_type == content_type)
            .take(k)
            .collect();

        Ok(filtered)
    }

    async fn remove_vector(&self, id: &str) -> Result<bool> {
        // Find vector_id in HNSW index by content_id and remove it
        let mut removed = false;
        let vector_id = self.vector_index.find_vector_id_by_content_id(id);

        if let Some(vid) = vector_id {
            self.vector_index.remove_vector(vid)?;
            removed = true;
        }

        // Remove from RocksDB
        let embeddings = self.load_all_embeddings().await?;
        for e in embeddings {
            if e.content_id == id {
                self.delete_embedding(&e.session_id, &e.content_id).await?;
                return Ok(true);
            }
        }

        Ok(removed)
    }

    async fn has_session_embeddings(&self, session_id: &str) -> bool {
        self.count_embeddings(session_id).await > 0
    }

    async fn count_session_embeddings(&self, session_id: &str) -> usize {
        self.count_embeddings(session_id).await
    }

    async fn total_count(&self) -> usize {
        self.vector_index.len()
    }

    async fn get_session_vectors(
        &self,
        session_id: &str,
    ) -> Result<Vec<(Vec<f32>, VectorMetadata)>> {
        let embeddings = self.load_session_embeddings(session_id).await?;
        Ok(embeddings
            .into_iter()
            .map(|e| {
                let metadata = e.to_metadata();
                (e.vector, metadata)
            })
            .collect())
    }

    async fn get_all_vectors(&self) -> Result<Vec<(Vec<f32>, VectorMetadata)>> {
        let embeddings = self.load_all_embeddings().await?;
        Ok(embeddings
            .into_iter()
            .map(|e| {
                let metadata = e.to_metadata();
                (e.vector, metadata)
            })
            .collect())
    }
}

// ============================================================================
// GraphStorage Trait Implementation - Entity Graph Persistence
// ============================================================================

impl RealRocksDBStorage {
    /// Generate key for entity storage
    fn entity_key(session_id: Uuid, entity_name: &str) -> String {
        format!("entity:{}:{}", session_id, entity_name)
    }

    /// Generate key prefix for all entities in a session
    fn entity_prefix(session_id: Uuid) -> String {
        format!("entity:{}:", session_id)
    }

    /// Generate key for relationship storage
    fn relationship_key(
        session_id: Uuid,
        from_entity: &str,
        to_entity: &str,
        relation_type: &RelationType,
    ) -> String {
        format!(
            "relationship:{}:{}:{}:{:?}",
            session_id, from_entity, to_entity, relation_type
        )
    }

    /// Generate key prefix for all relationships in a session
    fn relationship_prefix(session_id: Uuid) -> String {
        format!("relationship:{}:", session_id)
    }

    /// Save an entity to RocksDB
    async fn save_entity(&self, entity: &StoredEntity) -> Result<()> {
        let db = self.db.clone();
        let entity = entity.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let key = Self::entity_key(entity.session_id, &entity.name);
            let data = bincode::serde::encode_to_vec(&entity, bincode::config::standard())
                .map_err(|e| anyhow::anyhow!("Failed to serialize entity: {}", e))?;
            db.put(key.as_bytes(), &data)?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Load an entity from RocksDB
    async fn load_entity(&self, session_id: Uuid, name: &str) -> Result<Option<StoredEntity>> {
        let db = self.db.clone();
        let key = Self::entity_key(session_id, name);

        tokio::task::spawn_blocking(move || -> Result<Option<StoredEntity>> {
            if let Some(data) = db.get(key.as_bytes())? {
                let (entity, _) = bincode::serde::decode_from_slice::<StoredEntity, _>(
                    &data,
                    bincode::config::standard(),
                )
                .map_err(|e| anyhow::anyhow!("Failed to deserialize entity: {}", e))?;
                Ok(Some(entity))
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Load all entities for a session
    async fn load_session_entities(&self, session_id: Uuid) -> Result<Vec<StoredEntity>> {
        let db = self.db.clone();
        let prefix = Self::entity_prefix(session_id);

        tokio::task::spawn_blocking(move || -> Result<Vec<StoredEntity>> {
            let mut entities = Vec::new();
            let iter = db.iterator(rocksdb::IteratorMode::From(
                prefix.as_bytes(),
                rocksdb::Direction::Forward,
            ));

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with(&prefix) {
                    break;
                }

                if let Ok((entity, _)) = bincode::serde::decode_from_slice::<StoredEntity, _>(
                    &value,
                    bincode::config::standard(),
                ) {
                    entities.push(entity);
                }
            }

            Ok(entities)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Delete an entity from RocksDB
    async fn delete_stored_entity(&self, session_id: Uuid, name: &str) -> Result<()> {
        let db = self.db.clone();
        let key = Self::entity_key(session_id, name);

        tokio::task::spawn_blocking(move || -> Result<()> {
            db.delete(key.as_bytes())?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Save a relationship to RocksDB
    async fn save_relationship(&self, relationship: &StoredRelationship) -> Result<()> {
        let db = self.db.clone();
        let relationship = relationship.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let rel_type: RelationType = relationship
                .relation_type
                .parse()
                .unwrap_or(RelationType::RelatedTo);
            let key = Self::relationship_key(
                relationship.session_id,
                &relationship.from_entity,
                &relationship.to_entity,
                &rel_type,
            );
            let data = bincode::serde::encode_to_vec(&relationship, bincode::config::standard())
                .map_err(|e| anyhow::anyhow!("Failed to serialize relationship: {}", e))?;
            db.put(key.as_bytes(), &data)?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Batch save entities and relationships in a single RocksDB WriteBatch.
    /// This is significantly more efficient than individual saves when updating
    /// multiple entities/relationships at once (1 spawn_blocking vs N).
    pub async fn batch_save_graph_data(
        &self,
        entities: &[StoredEntity],
        relationships: &[StoredRelationship],
    ) -> Result<()> {
        if entities.is_empty() && relationships.is_empty() {
            return Ok(());
        }

        let db = self.db.clone();
        let entities = entities.to_vec();
        let relationships = relationships.to_vec();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut batch = WriteBatch::default();

            for entity in &entities {
                let key = Self::entity_key(entity.session_id, &entity.name);
                let data = bincode::serde::encode_to_vec(entity, bincode::config::standard())
                    .map_err(|e| anyhow::anyhow!("Failed to serialize entity: {}", e))?;
                batch.put(key.as_bytes(), &data);
            }

            for rel in &relationships {
                let rel_type: RelationType = rel
                    .relation_type
                    .parse()
                    .unwrap_or(RelationType::RelatedTo);
                let key = Self::relationship_key(
                    rel.session_id,
                    &rel.from_entity,
                    &rel.to_entity,
                    &rel_type,
                );
                let data = bincode::serde::encode_to_vec(rel, bincode::config::standard())
                    .map_err(|e| anyhow::anyhow!("Failed to serialize relationship: {}", e))?;
                batch.put(key.as_bytes(), &data);
            }

            db.write(batch)?;
            debug!(
                "Batch saved {} entities + {} relationships",
                entities.len(),
                relationships.len()
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Batch save embeddings in a single RocksDB WriteBatch.
    pub async fn batch_save_embeddings(&self, embeddings: &[StoredEmbedding]) -> Result<()> {
        if embeddings.is_empty() {
            return Ok(());
        }

        let db = self.db.clone();
        let embeddings = embeddings.to_vec();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut batch = WriteBatch::default();

            for embedding in &embeddings {
                let key = format!(
                    "embedding:{}:{}",
                    embedding.session_id, embedding.content_id
                );
                let data = bincode::serde::encode_to_vec(embedding, bincode::config::standard())
                    .map_err(|e| anyhow::anyhow!("Failed to serialize embedding: {}", e))?;
                batch.put(key.as_bytes(), &data);
            }

            db.write(batch)?;
            debug!("Batch saved {} embeddings", embeddings.len());
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Load all relationships for a session
    async fn load_session_relationships(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<StoredRelationship>> {
        let db = self.db.clone();
        let prefix = Self::relationship_prefix(session_id);

        tokio::task::spawn_blocking(move || -> Result<Vec<StoredRelationship>> {
            let mut relationships = Vec::new();
            let iter = db.iterator(rocksdb::IteratorMode::From(
                prefix.as_bytes(),
                rocksdb::Direction::Forward,
            ));

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with(&prefix) {
                    break;
                }

                if let Ok((rel, _)) = bincode::serde::decode_from_slice::<StoredRelationship, _>(
                    &value,
                    bincode::config::standard(),
                ) {
                    relationships.push(rel);
                }
            }

            Ok(relationships)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }
}

#[async_trait]
impl GraphStorage for RealRocksDBStorage {
    async fn upsert_entity(&self, session_id: Uuid, entity: &EntityData) -> Result<()> {
        let stored = StoredEntity::from_entity_data(session_id, entity);
        self.save_entity(&stored).await
    }

    async fn get_entity(&self, session_id: Uuid, name: &str) -> Result<Option<EntityData>> {
        let stored = self.load_entity(session_id, name).await?;
        Ok(stored.map(|s| s.to_entity_data()))
    }

    async fn list_entities(&self, session_id: Uuid) -> Result<Vec<EntityData>> {
        let stored = self.load_session_entities(session_id).await?;
        Ok(stored.into_iter().map(|s| s.to_entity_data()).collect())
    }

    async fn delete_entity(&self, session_id: Uuid, name: &str) -> Result<()> {
        self.delete_stored_entity(session_id, name).await
    }

    async fn create_relationship(
        &self,
        session_id: Uuid,
        relationship: &EntityRelationship,
    ) -> Result<()> {
        let stored = StoredRelationship::from_relationship(session_id, relationship);
        self.save_relationship(&stored).await
    }

    async fn find_related_entities(
        &self,
        session_id: Uuid,
        entity_name: &str,
    ) -> Result<Vec<String>> {
        let relationships = self.load_session_relationships(session_id).await?;
        let mut related: Vec<String> = relationships
            .into_iter()
            .filter_map(|r| {
                if r.from_entity == entity_name {
                    Some(r.to_entity)
                } else if r.to_entity == entity_name {
                    Some(r.from_entity)
                } else {
                    None
                }
            })
            .collect();

        // Deduplicate
        related.sort();
        related.dedup();
        Ok(related)
    }

    async fn find_related_by_type(
        &self,
        session_id: Uuid,
        entity_name: &str,
        relation_type: &RelationType,
    ) -> Result<Vec<String>> {
        let type_str = format!("{:?}", relation_type);
        let relationships = self.load_session_relationships(session_id).await?;
        let mut related: Vec<String> = relationships
            .into_iter()
            .filter(|r| r.relation_type == type_str)
            .filter_map(|r| {
                if r.from_entity == entity_name {
                    Some(r.to_entity)
                } else if r.to_entity == entity_name {
                    Some(r.from_entity)
                } else {
                    None
                }
            })
            .collect();

        related.sort();
        related.dedup();
        Ok(related)
    }

    async fn find_shortest_path(
        &self,
        session_id: Uuid,
        from: &str,
        to: &str,
    ) -> Result<Option<Vec<String>>> {
        use std::collections::{HashSet, VecDeque};

        if from == to {
            return Ok(Some(vec![from.to_string()]));
        }

        let relationships = self.load_session_relationships(session_id).await?;

        // Build adjacency list
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for rel in &relationships {
            adjacency
                .entry(rel.from_entity.clone())
                .or_default()
                .push(rel.to_entity.clone());
            adjacency
                .entry(rel.to_entity.clone())
                .or_default()
                .push(rel.from_entity.clone());
        }

        // BFS for shortest path
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, Vec<String>)> = VecDeque::new();

        visited.insert(from.to_string());
        queue.push_back((from.to_string(), vec![from.to_string()]));

        while let Some((current, path)) = queue.pop_front() {
            if let Some(neighbors) = adjacency.get(&current) {
                for neighbor in neighbors {
                    if neighbor == to {
                        let mut final_path = path.clone();
                        final_path.push(neighbor.clone());
                        return Ok(Some(final_path));
                    }

                    if !visited.contains(neighbor) {
                        visited.insert(neighbor.clone());
                        let mut new_path = path.clone();
                        new_path.push(neighbor.clone());
                        queue.push_back((neighbor.clone(), new_path));
                    }
                }
            }
        }

        Ok(None)
    }

    async fn get_entity_network(
        &self,
        session_id: Uuid,
        center: &str,
        max_depth: usize,
    ) -> Result<EntityNetwork> {
        use std::collections::HashSet;

        let all_entities = self.load_session_entities(session_id).await?;
        let all_relationships = self.load_session_relationships(session_id).await?;

        // Build adjacency map
        let mut adjacency: HashMap<String, Vec<(String, &StoredRelationship)>> = HashMap::new();
        for rel in &all_relationships {
            adjacency
                .entry(rel.from_entity.clone())
                .or_default()
                .push((rel.to_entity.clone(), rel));
            adjacency
                .entry(rel.to_entity.clone())
                .or_default()
                .push((rel.from_entity.clone(), rel));
        }

        // BFS to collect entities within max_depth
        let mut visited: HashSet<String> = HashSet::new();
        let mut current_level: Vec<String> = vec![center.to_string()];
        visited.insert(center.to_string());

        for _ in 0..max_depth {
            let mut next_level = Vec::new();
            for entity in &current_level {
                if let Some(neighbors) = adjacency.get(entity) {
                    for (neighbor, _) in neighbors {
                        if !visited.contains(neighbor) {
                            visited.insert(neighbor.clone());
                            next_level.push(neighbor.clone());
                        }
                    }
                }
            }
            if next_level.is_empty() {
                break;
            }
            current_level = next_level;
        }

        // Collect entities in the network
        let entities: BTreeMap<String, EntityData> = all_entities
            .into_iter()
            .filter(|e| visited.contains(&e.name))
            .map(|e| (e.name.clone(), e.to_entity_data()))
            .collect();

        // Collect relationships where both endpoints are in the network
        let relationships: Vec<EntityRelationship> = all_relationships
            .into_iter()
            .filter(|r| visited.contains(&r.from_entity) && visited.contains(&r.to_entity))
            .map(|r| r.to_relationship())
            .collect();

        Ok(EntityNetwork {
            center: center.to_string(),
            entities,
            relationships,
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionCheckpoint {
    pub id: Uuid,
    pub session_id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,

    // Complete context snapshot
    pub structured_context: StructuredContext,
    pub recent_updates: Vec<ContextUpdate>,
    pub code_references: HashMap<String, Vec<CodeReference>>,
    pub change_history: Vec<ChangeRecord>,

    // Metadata
    pub total_updates: usize,
    pub context_quality_score: f32,
    pub compression_ratio: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    // use tokio_test;

    #[tokio::test]
    async fn test_rocksdb_session_operations() {
        let temp_dir = tempdir().expect("Failed to create temp directory");
        let storage = RealRocksDBStorage::new(temp_dir.path())
            .await
            .expect("Failed to create RocksDB storage");

        // Create test session
        let session_id = Uuid::new_v4();
        let session = ActiveSession::new(
            session_id,
            Some("Test Session".to_string()),
            Some("A test session for RocksDB storage operations".to_string()),
        );

        // Save session
        storage
            .save_session(&session)
            .await
            .expect("Failed to save session");

        // Load session
        let loaded_session = storage
            .load_session(session.id())
            .await
            .expect("Failed to load session");
        assert_eq!(session.id(), loaded_session.id());

        // Check if session exists
        assert!(
            storage
                .session_exists(session.id())
                .await
                .expect("Failed to check session existence")
        );

        // Delete session
        storage
            .delete_session(session.id())
            .await
            .expect("Failed to delete session");
        assert!(
            !storage
                .session_exists(session.id())
                .await
                .expect("Failed to check session existence after deletion")
        );
    }

    #[tokio::test]
    async fn test_rocksdb_graph_storage_entities() {
        use crate::core::context_update::EntityType;

        let temp_dir = tempdir().expect("Failed to create temp directory");
        let storage = RealRocksDBStorage::new(temp_dir.path())
            .await
            .expect("Failed to create RocksDB storage");

        let session_id = Uuid::new_v4();

        // Create test entity
        let entity = EntityData {
            name: "Rust".to_string(),
            entity_type: EntityType::Technology,
            first_mentioned: Utc::now(),
            last_mentioned: Utc::now(),
            mention_count: 5,
            importance_score: 0.8,
            description: Some("A systems programming language".to_string()),
        };

        // Upsert entity
        storage
            .upsert_entity(session_id, &entity)
            .await
            .expect("Failed to upsert entity");

        // Get entity
        let loaded = storage
            .get_entity(session_id, "Rust")
            .await
            .expect("Failed to get entity");
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.name, "Rust");
        assert_eq!(loaded.mention_count, 5);

        // List entities
        let entities = storage
            .list_entities(session_id)
            .await
            .expect("Failed to list entities");
        assert_eq!(entities.len(), 1);

        // Delete entity
        storage
            .delete_entity(session_id, "Rust")
            .await
            .expect("Failed to delete entity");

        let deleted = storage
            .get_entity(session_id, "Rust")
            .await
            .expect("Failed to check deleted entity");
        assert!(deleted.is_none());
    }

    #[tokio::test]
    async fn test_rocksdb_graph_storage_relationships() {
        use crate::core::context_update::EntityType;

        let temp_dir = tempdir().expect("Failed to create temp directory");
        let storage = RealRocksDBStorage::new(temp_dir.path())
            .await
            .expect("Failed to create RocksDB storage");

        let session_id = Uuid::new_v4();

        // Create test entities
        let rust = EntityData {
            name: "Rust".to_string(),
            entity_type: EntityType::Technology,
            first_mentioned: Utc::now(),
            last_mentioned: Utc::now(),
            mention_count: 5,
            importance_score: 0.8,
            description: None,
        };

        let tokio = EntityData {
            name: "Tokio".to_string(),
            entity_type: EntityType::Technology,
            first_mentioned: Utc::now(),
            last_mentioned: Utc::now(),
            mention_count: 3,
            importance_score: 0.6,
            description: None,
        };

        storage.upsert_entity(session_id, &rust).await.unwrap();
        storage.upsert_entity(session_id, &tokio).await.unwrap();

        // Create relationship
        let relationship = EntityRelationship {
            from_entity: "Tokio".to_string(),
            to_entity: "Rust".to_string(),
            relation_type: RelationType::DependsOn,
            context: "Tokio is built on Rust".to_string(),
        };

        storage
            .create_relationship(session_id, &relationship)
            .await
            .expect("Failed to create relationship");

        // Find related entities
        let related = storage
            .find_related_entities(session_id, "Rust")
            .await
            .expect("Failed to find related entities");
        assert_eq!(related.len(), 1);
        assert_eq!(related[0], "Tokio");

        // Find related by type
        let related_by_type = storage
            .find_related_by_type(session_id, "Rust", &RelationType::DependsOn)
            .await
            .expect("Failed to find related by type");
        assert_eq!(related_by_type.len(), 1);
    }

    #[tokio::test]
    async fn test_rocksdb_graph_storage_shortest_path() {
        use crate::core::context_update::EntityType;

        let temp_dir = tempdir().expect("Failed to create temp directory");
        let storage = RealRocksDBStorage::new(temp_dir.path())
            .await
            .expect("Failed to create RocksDB storage");

        let session_id = Uuid::new_v4();

        // Create chain: A -> B -> C
        for name in ["A", "B", "C"] {
            let entity = EntityData {
                name: name.to_string(),
                entity_type: EntityType::Concept,
                first_mentioned: Utc::now(),
                last_mentioned: Utc::now(),
                mention_count: 1,
                importance_score: 0.5,
                description: None,
            };
            storage.upsert_entity(session_id, &entity).await.unwrap();
        }

        // A -> B
        storage
            .create_relationship(
                session_id,
                &EntityRelationship {
                    from_entity: "A".to_string(),
                    to_entity: "B".to_string(),
                    relation_type: RelationType::LeadsTo,
                    context: String::new(),
                },
            )
            .await
            .unwrap();

        // B -> C
        storage
            .create_relationship(
                session_id,
                &EntityRelationship {
                    from_entity: "B".to_string(),
                    to_entity: "C".to_string(),
                    relation_type: RelationType::LeadsTo,
                    context: String::new(),
                },
            )
            .await
            .unwrap();

        // Find shortest path A -> C
        let path = storage
            .find_shortest_path(session_id, "A", "C")
            .await
            .expect("Failed to find shortest path");
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path, vec!["A", "B", "C"]);

        // No path exists to unconnected node
        let entity_d = EntityData {
            name: "D".to_string(),
            entity_type: EntityType::Concept,
            first_mentioned: Utc::now(),
            last_mentioned: Utc::now(),
            mention_count: 1,
            importance_score: 0.5,
            description: None,
        };
        storage.upsert_entity(session_id, &entity_d).await.unwrap();

        let no_path = storage
            .find_shortest_path(session_id, "A", "D")
            .await
            .expect("Failed to check no path");
        assert!(no_path.is_none());
    }

    #[tokio::test]
    async fn test_rocksdb_embedding_dimension_validation() {
        let temp_dir = tempdir().expect("Failed to create temp directory");
        let storage = RealRocksDBStorage::new(temp_dir.path())
            .await
            .expect("Failed to create RocksDB storage");

        let metadata = VectorMetadata {
            id: "test-id".to_string(),
            text: "test text".to_string(),
            source: Uuid::new_v4().to_string(),
            content_type: "qa".to_string(),
            timestamp: Utc::now(),
            metadata: HashMap::new(),
        };

        // Valid dimension (384)
        let valid_vector = vec![0.1f32; EMBEDDING_DIMENSION];
        let result = storage.add_vector(valid_vector, metadata.clone()).await;
        assert!(result.is_ok());

        // Invalid dimension (wrong size)
        let invalid_vector = vec![0.1f32; 100];
        let result = storage.add_vector(invalid_vector, metadata).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid embedding dimension")
        );
    }
}
