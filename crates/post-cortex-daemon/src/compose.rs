// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The composition root (ARCHITECTURE section 5): the adapters the
//! configuration names, opened and handed to `Memory` as ports. The storage
//! backend, its connection, the data directory, the embedding model and its
//! dimension, the reranker files and keys are the daemon's configuration;
//! the memory `SystemConfig` holds capability settings only. The daemon,
//! the offline `pcx` admin commands and the cloud tenant all build memory
//! here, and serve the same routes ([`routes`]).

use std::sync::Arc;

use post_cortex_embeddings::{
    EmbeddingConfig, EmbeddingModelType, LazyEmbeddingEngine, MemoryIndex,
};
use post_cortex_memory::{Memory, MemoryPorts, PipelineConfig, SystemConfig};
use post_cortex_ports::storage::{Stores, VectorStore};
use post_cortex_ports::{BlobStore, EmbeddingEngine, Reranker, SearchIndex};
use post_cortex_storage::{JsonArchive, RealRocksDBStorage};
use tracing::{info, warn};

mod builtin;
mod dynamic;
mod events;
mod launch;
mod memory_writes;
mod modules;
mod routes;

pub use crate::daemon::host_allowlist::HostPolicy;
pub use builtin::Builtin;
pub use dynamic::keep_dynamic;
pub use events::{MemoryEvents, drop_old_events};
pub use launch::{CatalogLauncher, Limits};
pub use memory_writes::feed_memory_writes;
pub use modules::{BuiltinSpec, Modules, host};
pub use pcx_dynamic::LauncherConfig;
pub use pcx_module_launcher::Runtimes;
pub use routes::{BUILD, BrowserAccess, RouteConfig, grpc_routes, routes};

/// Tokens per (query, record) pair the cross-encoder sees; the retrieval
/// prototype was measured at 256.
#[cfg(feature = "rerank")]
const RERANKER_MAX_LENGTH: usize = 256;

/// Each document sent to Voyage is cut to about this many tokens: 31% fewer
/// tokens than whole records, no loss on LongMemEval.
#[cfg(feature = "rerank-voyage")]
const VOYAGE_MAX_DOC_TOKENS: usize = 256;

/// A Voyage call slower than this fails over to the local reranker.
#[cfg(feature = "rerank-voyage")]
const VOYAGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Where the stores live: the backend and its location.
#[derive(Debug, Clone)]
pub enum StorageConfig {
    /// RocksDB in this directory.
    RocksDb {
        /// The data directory.
        data_directory: std::path::PathBuf,
    },
    /// SurrealDB at this endpoint.
    SurrealDb {
        /// `ws://host:port`, `mem://`, ...
        endpoint: String,
        /// User name, if the server needs one.
        username: Option<String>,
        /// Password, if the server needs one.
        password: Option<String>,
        /// Namespace.
        namespace: Option<String>,
        /// Database.
        database: Option<String>,
    },
}

/// The model adapters: the embedding model and the rerankers.
#[derive(Debug, Clone, Default)]
pub struct ModelConfig {
    /// The embedding model; its dimension is the index's.
    pub embedding_model: EmbeddingModelType,
    /// HuggingFace id of the local cross-encoder, if any.
    pub reranker_model: Option<String>,
    /// Path of the ONNX Runtime library the cross-encoder loads (Linux).
    pub onnx_runtime: Option<String>,
    /// Voyage reranks `accurate` searches when set.
    pub voyage: Option<VoyageConfig>,
}

/// The Voyage rerank service.
#[derive(Clone, PartialEq, Eq)]
pub struct VoyageConfig {
    /// The rerank API (`post_cortex_embeddings::rerank::VOYAGE_ENDPOINT`
    /// for Voyage itself).
    pub endpoint: String,
    /// The API key.
    pub api_key: String,
    /// The rerank model, e.g. `rerank-3-lite`.
    pub model: String,
}

/// Without the API key: a configuration may be logged.
impl std::fmt::Debug for VoyageConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoyageConfig")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

/// The models one process shares between its memory instances (one
/// embedding engine per process, T3): built once, loaded on first use.
#[derive(Clone)]
pub struct Models {
    /// The embedding engine.
    pub engine: Arc<dyn EmbeddingEngine>,
    /// The local reranker.
    pub reranker: Option<Arc<dyn Reranker>>,
    /// The `accurate` reranker.
    pub accurate_reranker: Option<Arc<dyn Reranker>>,
}

impl Models {
    /// The models `config` names; nothing is loaded yet.
    pub fn new(config: &ModelConfig) -> Self {
        let engine: Arc<dyn EmbeddingEngine> =
            Arc::new(LazyEmbeddingEngine::new(EmbeddingConfig {
                model_type: config.embedding_model,
                ..Default::default()
            }));
        let reranker = config
            .reranker_model
            .as_deref()
            .and_then(|model| local_reranker(model, config.onnx_runtime.clone()));
        let remote = config.voyage.as_ref().and_then(voyage_reranker);
        let accurate_reranker = remote.map(|remote| match &reranker {
            Some(local) => Arc::new(post_cortex_embeddings::rerank::FallbackReranker::new(
                remote,
                Arc::clone(local),
            )) as Arc<dyn Reranker>,
            None => remote,
        });
        Self {
            engine,
            reranker,
            accurate_reranker,
        }
    }
}

#[cfg(feature = "rerank")]
fn local_reranker(model: &str, runtime: Option<String>) -> Option<Arc<dyn Reranker>> {
    Some(Arc::new(
        post_cortex_embeddings::rerank::LazyCrossEncoder::new(
            model.to_string(),
            RERANKER_MAX_LENGTH,
            runtime,
        ),
    ))
}

#[cfg(not(feature = "rerank"))]
fn local_reranker(model: &str, _runtime: Option<String>) -> Option<Arc<dyn Reranker>> {
    warn!("reranker_model = {model} is set but this build has no `rerank` feature");
    None
}

#[cfg(feature = "rerank-voyage")]
fn voyage_reranker(voyage: &VoyageConfig) -> Option<Arc<dyn Reranker>> {
    match post_cortex_embeddings::rerank::VoyageReranker::new(
        voyage.endpoint.clone(),
        voyage.api_key.clone(),
        voyage.model.clone(),
        VOYAGE_MAX_DOC_TOKENS,
        VOYAGE_TIMEOUT,
    ) {
        Ok(reranker) => {
            tracing::debug!("Reranking search results with Voyage {}", voyage.model);
            Some(Arc::new(reranker))
        }
        Err(e) => {
            warn!("Failed to set up the Voyage reranker: {:#}", e);
            None
        }
    }
}

#[cfg(not(feature = "rerank-voyage"))]
fn voyage_reranker(_voyage: &VoyageConfig) -> Option<Arc<dyn Reranker>> {
    warn!("A Voyage API key is set but this build has no `rerank-voyage` feature");
    None
}

/// A storage backend's ports: the capability stores and the persisted
/// embeddings.
pub struct OpenedStorage {
    /// The capability ports.
    pub stores: Stores,
    /// The persisted embeddings.
    pub vectors: Arc<dyn VectorStore>,
}

impl OpenedStorage {
    /// The ports of RocksDB `storage`.
    pub fn rocksdb(storage: RealRocksDBStorage) -> Self {
        let storage = Arc::new(storage);
        Self {
            stores: Stores::of(Arc::clone(&storage)),
            vectors: storage,
        }
    }

    /// The ports of SurrealDB `storage`.
    #[cfg(feature = "surrealdb-storage")]
    pub fn surrealdb(storage: post_cortex_storage::SurrealDBStorage) -> Self {
        let storage = Arc::new(storage);
        Self {
            stores: Stores::of(Arc::clone(&storage)),
            vectors: storage,
        }
    }
}

/// How a storage backend is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Schema and data migrations run; the caller writes.
    ReadWrite,
    /// Nothing is changed on open and no lock is taken: no schema
    /// statement, no migration, so a database another process (the daemon)
    /// serves can be read.
    ReadOnly,
}

/// Open the storage backend `config` names, for vectors of `dimension`.
///
/// # Errors
/// The backend could not be opened, or is not in this build.
pub async fn open_storage(
    config: &StorageConfig,
    dimension: usize,
    access: Access,
) -> Result<OpenedStorage, String> {
    match config {
        StorageConfig::RocksDb { data_directory } => {
            info!("Using RocksDB storage backend");
            let storage = match access {
                Access::ReadWrite => {
                    RealRocksDBStorage::new_with_dimension(data_directory, dimension).await
                }
                Access::ReadOnly => {
                    RealRocksDBStorage::open_read_only(data_directory, dimension).await
                }
            }
            .map_err(|e| {
                format!(
                    "Failed to open RocksDB at {}: {e}",
                    data_directory.display()
                )
            })?;
            Ok(OpenedStorage::rocksdb(storage))
        }
        #[cfg(feature = "surrealdb-storage")]
        StorageConfig::SurrealDb {
            endpoint,
            username,
            password,
            namespace,
            database,
        } => {
            info!("Using SurrealDB storage backend: {endpoint}");
            let storage = match access {
                Access::ReadWrite => {
                    post_cortex_storage::SurrealDBStorage::new_with_dimension(
                        endpoint,
                        username.as_deref(),
                        password.as_deref(),
                        namespace.as_deref(),
                        database.as_deref(),
                        dimension,
                    )
                    .await
                }
                Access::ReadOnly => {
                    post_cortex_storage::SurrealDBStorage::open_existing(
                        endpoint,
                        username.as_deref(),
                        password.as_deref(),
                        namespace.as_deref(),
                        database.as_deref(),
                    )
                    .await
                }
            }
            .map_err(|e| format!("Failed to open SurrealDB: {e}"))?;
            Ok(OpenedStorage::surrealdb(storage))
        }
        #[cfg(not(feature = "surrealdb-storage"))]
        StorageConfig::SurrealDb { .. } => Err(
            "storage_backend: surrealdb needs a build with the `surrealdb-storage` feature"
                .to_string(),
        ),
    }
}

/// The in-memory search index over `storage`'s persisted embeddings, for
/// vectors of `dimension` (the model's, checked when it opens).
pub fn search_index(storage: &OpenedStorage, dimension: usize) -> Arc<dyn SearchIndex> {
    Arc::new(MemoryIndex::new(
        dimension,
        Some(Arc::clone(&storage.vectors)),
    ))
}

/// The blob store on the S3 bucket `config` names.
///
/// # Errors
/// The configuration does not make an S3 client.
pub fn open_blobs(config: &post_cortex_blobs::S3Config) -> Result<Arc<dyn BlobStore>, String> {
    let blobs = post_cortex_blobs::ObjectBlobs::s3(config).map_err(|e| e.to_string())?;
    info!("Blobs on S3: {} bucket {}", config.endpoint, config.bucket);
    Ok(Arc::new(blobs))
}

/// The ports of memory over `storage`, embedding with `models`, its blobs
/// in `blobs` (none: no blobs).
///
/// # Errors
/// The search index could not be created.
pub fn ports(
    storage: OpenedStorage,
    models: &Models,
    blobs: Option<Arc<dyn BlobStore>>,
) -> Result<MemoryPorts, String> {
    let index = search_index(&storage, models.engine.model().dimension);
    Ok(MemoryPorts {
        stores: storage.stores,
        index: Some(index),
        engine: Some(Arc::clone(&models.engine)),
        reranker: models.reranker.clone(),
        accurate_reranker: models.accurate_reranker.clone(),
        archive: Arc::new(JsonArchive),
        blobs,
    })
}

/// Open the storage `storage` names and build `Memory` over it with
/// `models`, its blobs in `blobs` (none: no blobs), `system` and
/// `pipeline`.
///
/// # Errors
/// The storage or the index could not be opened, or memory not built.
pub async fn open_memory(
    storage: &StorageConfig,
    models: &Models,
    blobs: Option<Arc<dyn BlobStore>>,
    system: SystemConfig,
    pipeline: PipelineConfig,
) -> Result<Memory, String> {
    let opened = open_storage(storage, models.engine.model().dimension, Access::ReadWrite).await?;
    Memory::over_ports_with(ports(opened, models, blobs)?, system, pipeline)
        .await
        .map_err(|e| format!("Failed to initialize memory system: {e}"))
}
