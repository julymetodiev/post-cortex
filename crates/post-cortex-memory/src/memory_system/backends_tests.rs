// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Storage backends for tests that restart a memory system over the same
//! storage.

use std::sync::Arc;

use chrono::Utc;
use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::services::UpdateContextRequest;
use post_cortex_ports::storage::VectorStore;
use post_cortex_ports::{EmbeddingEngine, Stores};

use crate::capabilities::MemoryPorts;
use uuid::Uuid;

use super::persister::PersistSink;
use super::{MemoryRuntime, SystemConfig};

/// Vector dimension of the backends: the default embedding model's.
const DIMENSION: usize = 256;

/// A runtime with the capability services over it: tests reach the runtime
/// state through `Deref` and the operations through `mem`.
#[derive(Clone)]
pub(crate) struct TestSystem {
    pub(crate) rt: Arc<MemoryRuntime>,
    pub(crate) mem: crate::capabilities::Memory,
    vectors: Option<Arc<dyn VectorStore>>,
}

impl TestSystem {
    pub(crate) fn new(rt: MemoryRuntime) -> Self {
        let rt = Arc::new(rt);
        Self {
            mem: crate::capabilities::test_support::memory(&rt),
            rt,
            vectors: None,
        }
    }

    /// A system whose backend persists embeddings in `vectors`.
    pub(crate) fn over(rt: MemoryRuntime, vectors: Arc<dyn VectorStore>) -> Self {
        Self {
            vectors: Some(vectors),
            ..Self::new(rt)
        }
    }

    /// The embeddings the backend persisted.
    pub(crate) fn stored(&self) -> &Arc<dyn VectorStore> {
        self.vectors.as_ref().unwrap()
    }
}

impl std::ops::Deref for TestSystem {
    type Target = Arc<MemoryRuntime>;

    fn deref(&self) -> &Self::Target {
        &self.rt
    }
}

/// The engine of the default model (model2vec, loaded on first use).
pub(crate) fn default_engine() -> Arc<dyn EmbeddingEngine> {
    Arc::new(post_cortex_embeddings::LazyEmbeddingEngine::new(
        post_cortex_embeddings::EmbeddingConfig::default(),
    ))
}

/// A model-free engine of the backends' dimension.
pub(crate) fn hash_engine() -> Arc<dyn EmbeddingEngine> {
    Arc::new(post_cortex_embeddings::test_support::hash_engine(DIMENSION))
}

/// One storage backend; [`Self::start`] builds a memory system on it, a
/// second call stands for a daemon restart (hydrating from storage, a new
/// search index loading the stored vectors).
pub(crate) struct Backend {
    pub(crate) name: &'static str,
    stores: Stores,
    vectors: Arc<dyn VectorStore>,
    _dir: Option<tempfile::TempDir>,
}

impl Backend {
    /// A memory system without embeddings.
    pub(crate) async fn start(&self) -> TestSystem {
        self.start_with(SystemConfig {
            enable_embeddings: false,
            ..Default::default()
        })
        .await
    }

    /// A memory system with `config`, embedding with the default model.
    pub(crate) async fn start_with(&self, config: SystemConfig) -> TestSystem {
        self.start_with_engine(config, default_engine()).await
    }

    /// A memory system with `config`, embedding with `engine`.
    pub(crate) async fn start_with_engine(
        &self,
        config: SystemConfig,
        engine: Arc<dyn EmbeddingEngine>,
    ) -> TestSystem {
        TestSystem::over(
            MemoryRuntime::new(self.ports(Some(engine), None), config)
                .await
                .unwrap(),
            Arc::clone(&self.vectors),
        )
    }

    /// A memory system without embeddings whose session writes go to
    /// `sink` (e.g. one that fails on demand).
    pub(crate) async fn start_with_sink(&self, sink: Arc<dyn PersistSink>) -> TestSystem {
        self.start_with_sink_and(
            SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
            sink,
        )
        .await
    }

    /// A memory system with `config` whose session writes go to `sink`.
    pub(crate) async fn start_with_sink_and(
        &self,
        config: SystemConfig,
        sink: Arc<dyn PersistSink>,
    ) -> TestSystem {
        TestSystem::over(
            MemoryRuntime::new_with_sink(self.ports(Some(default_engine()), None), sink, config)
                .await
                .unwrap(),
            Arc::clone(&self.vectors),
        )
    }

    /// The capability services over a memory system without embeddings.
    pub(crate) async fn memory(&self) -> crate::capabilities::Memory {
        self.start().await.mem
    }

    /// The backend's ports.
    pub(crate) fn stores(&self) -> &Stores {
        &self.stores
    }

    /// The backend's persisted embeddings.
    pub(crate) fn vectors(&self) -> &Arc<dyn VectorStore> {
        &self.vectors
    }

    /// The memory ports over this backend: a new search index over its
    /// embeddings (or `vectors` in their place), `engine`, the JSON codec.
    pub(crate) fn ports(
        &self,
        engine: Option<Arc<dyn EmbeddingEngine>>,
        vectors: Option<Arc<dyn VectorStore>>,
    ) -> MemoryPorts {
        let vectors = vectors.unwrap_or_else(|| Arc::clone(&self.vectors));
        let index = post_cortex_embeddings::MemoryIndex::new(DIMENSION, Some(vectors));
        MemoryPorts {
            stores: self.stores.clone(),
            index: Some(Arc::new(index)),
            engine,
            reranker: None,
            accurate_reranker: None,
            archive: Arc::new(post_cortex_storage::JsonArchive),
            blobs: Some(Arc::new(post_cortex_blobs::ObjectBlobs::in_memory())),
        }
    }
}

/// RocksDB in a temp dir, and SurrealDB `mem://`.
pub(crate) async fn backends() -> Vec<Backend> {
    let dir = tempfile::tempdir().unwrap();
    let rocks = Arc::new(
        post_cortex_storage::RealRocksDBStorage::new(dir.path())
            .await
            .unwrap(),
    );
    let surreal = Arc::new(
        post_cortex_storage::SurrealDBStorage::new_with_dimension(
            "mem://", None, None, None, None, DIMENSION,
        )
        .await
        .unwrap(),
    );
    vec![
        Backend {
            name: "rocksdb",
            stores: Stores::of(Arc::clone(&rocks)),
            vectors: rocks,
            _dir: Some(dir),
        },
        Backend {
            name: "surrealdb",
            stores: Stores::of(Arc::clone(&surreal)),
            vectors: surreal,
            _dir: None,
        },
    ]
}

/// A valid write of a decision titled `title` to `session_id`.
pub(crate) fn request(session_id: Uuid, title: &str, description: &str) -> UpdateContextRequest {
    let entity = |name: &str| EntityData {
        aliases: Vec::new(),
        name: name.to_string(),
        entity_type: EntityType::Concept,
        first_mentioned: Utc::now(),
        last_mentioned: Utc::now(),
        mention_count: 1,
        importance_score: 1.0,
        description: None,
    };
    UpdateContextRequest {
        meta: Default::default(),
        session_id,
        interaction_type: UpdateType::DecisionMade,
        content: UpdateContent {
            title: title.into(),
            description: description.into(),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        entities: vec![entity("Lighthouse"), entity("Keeper")],
        relations: vec![EntityRelationship {
            from_entity: "Lighthouse".into(),
            to_entity: "Keeper".into(),
            relation_type: RelationType::RelatedTo,
            context: "test".into(),
        }],
        code_reference: None,
        author: None,
        derived_from: Vec::new(),
    }
}

/// Storage writes that fail while `failing` is set.
pub(crate) struct FlakySink {
    inner: super::persister::StorageSink,
    pub(crate) failing: std::sync::atomic::AtomicBool,
}

impl FlakySink {
    /// Writes to `backend`, not failing yet.
    pub(crate) fn new(backend: &Backend) -> Self {
        Self {
            inner: super::persister::StorageSink::new(Arc::clone(&backend.stores().sessions)),
            failing: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

#[async_trait::async_trait]
impl PersistSink for FlakySink {
    async fn write(
        &self,
        session: &post_cortex_core::session::active_session::ActiveSession,
        changes: post_cortex_ports::SessionChanges,
    ) -> Result<(), post_cortex_ports::PortError> {
        if self.failing.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(post_cortex_ports::PortError::Unavailable(
                "injected storage failure".to_string(),
            ));
        }
        self.inner.write(session, changes).await
    }

    async fn delete(&self, session_id: Uuid) -> Result<bool, post_cortex_ports::PortError> {
        self.inner.delete(session_id).await
    }
}

/// A configuration with embeddings on.
pub(crate) fn embeddings_config() -> SystemConfig {
    SystemConfig {
        enable_embeddings: true,
        ..Default::default()
    }
}

/// RocksDB in `dir` (the caller keeps the directory).
pub(crate) async fn rocksdb_backend(dir: &tempfile::TempDir) -> Backend {
    let rocks = Arc::new(
        post_cortex_storage::RealRocksDBStorage::new(dir.path())
            .await
            .unwrap(),
    );
    Backend {
        name: "rocksdb",
        stores: Stores::of(Arc::clone(&rocks)),
        vectors: rocks,
        _dir: None,
    }
}

/// A runtime with `config` over RocksDB in `dir`, embedding with the
/// default model.
pub(crate) async fn rocksdb_runtime(
    dir: &tempfile::TempDir,
    config: SystemConfig,
) -> Result<MemoryRuntime, crate::memory_system::RuntimeError> {
    let backend = rocksdb_backend(dir).await;
    MemoryRuntime::new(backend.ports(Some(default_engine()), None), config).await
}

/// [`rocksdb_runtime`] with the services, embedding with `engine`.
pub(crate) async fn rocksdb_system(
    dir: &tempfile::TempDir,
    config: SystemConfig,
    engine: Arc<dyn EmbeddingEngine>,
) -> TestSystem {
    rocksdb_backend(dir)
        .await
        .start_with_engine(config, engine)
        .await
}

/// The capability services with `config` over RocksDB in `dir`.
pub(crate) async fn rocksdb_memory_in(
    dir: &tempfile::TempDir,
    config: SystemConfig,
) -> Result<crate::capabilities::Memory, crate::capabilities::OpenError> {
    let backend = rocksdb_backend(dir).await;
    crate::capabilities::Memory::over_ports(backend.ports(Some(default_engine()), None), config)
        .await
}
