// Copyright (c) 2025 Julius ML
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

//! Common test utilities and fixtures

use anyhow::Result;
use post_cortex_memory::{Memory, SystemConfig};
use std::sync::Arc;
use tempfile::{TempDir, tempdir};
use uuid::Uuid;

/// Test fixture builder pattern for creating test systems with content.
/// Uses SurrealDB kv-mem backend (same storage path as production) when
/// the `surrealdb-storage` feature is enabled, otherwise falls back to RocksDB.
pub struct TestFixture {
    pub system: Arc<Memory>,
    pub session_id: Uuid,
    pub _temp_dir: TempDir,
}

impl TestFixture {
    /// Create a new test fixture with a session
    pub async fn new() -> Result<Self> {
        let temp_dir = tempdir()?;
        let config = SystemConfig {
            enable_embeddings: true,
            auto_vectorize_on_update: true,
            ..Default::default()
        };

        // Use SurrealDB kv-mem backend — same storage path as production daemon
        #[cfg(feature = "surrealdb-storage")]
        let storage = post_cortex::compose::StorageConfig::SurrealDb {
            endpoint: "mem://".to_string(),
            username: None,
            password: None,
            namespace: Some("test".to_string()),
            database: Some("test".to_string()),
        };
        #[cfg(not(feature = "surrealdb-storage"))]
        let storage = rocksdb(temp_dir.path());

        let system = Arc::new(open_memory(storage, config).await?);

        let session_id = system
            .sessions
            .create(
                Some("test-session".to_string()),
                Some("Test session".to_string()),
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        Ok(Self {
            system,
            session_id,
            _temp_dir: temp_dir,
        })
    }

    /// Create fixture and add content
    pub async fn with_content(content: &[&str]) -> Result<Self> {
        let fixture = Self::new().await?;

        for text in content {
            // The write path of a record: stored (graph merged), then
            // embedded as the pipeline's embedding worker does.
            let system = &fixture.system;
            write_text(&system.records, fixture.session_id, text).await?;
            assert!(
                system
                    .records
                    .wait_idle(std::time::Duration::from_secs(120))
                    .await
            );
        }

        // Wait for vectorization
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        Ok(fixture)
    }

    /// Search with recency bias
    pub async fn search_with_bias(
        &self,
        query: &str,
        bias: f32,
    ) -> Result<Vec<post_cortex_memory::content_vectorizer::SemanticSearchResult>> {
        let options = post_cortex_memory::content_vectorizer::SearchOptions {
            recency_bias: Some(bias),
            ..Default::default()
        };
        let results = self
            .system
            .search
            .search(
                post_cortex_memory::capabilities::SearchScope::Session(self.session_id),
                query,
                options,
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        Ok(results)
    }
}

/// A valid write (through the one write path, [`Records::write`]) of a
/// record of `update_type` titled `title` with `description`. It names two
/// entities, as every write must; their names appear in no test text.
pub fn record(
    session_id: Uuid,
    update_type: post_cortex::core::context_update::UpdateType,
    title: &str,
    description: &str,
) -> post_cortex::services::UpdateContextRequest {
    use post_cortex::core::context_update::{
        EntityData, EntityRelationship, EntityType, RelationType, UpdateContent,
    };
    let entity = |name: &str| EntityData {
        aliases: Vec::new(),
        name: name.to_string(),
        entity_type: EntityType::Concept,
        first_mentioned: chrono::Utc::now(),
        last_mentioned: chrono::Utc::now(),
        mention_count: 1,
        importance_score: 1.0,
        description: None,
    };
    post_cortex::services::UpdateContextRequest {
        meta: Default::default(),
        session_id,
        interaction_type: update_type,
        content: UpdateContent {
            title: title.to_string(),
            description: description.to_string(),
            details: Vec::new(),
            examples: Vec::new(),
            implications: Vec::new(),
        },
        entities: vec![entity("zq-fixture-a"), entity("zq-fixture-b")],
        relations: vec![EntityRelationship {
            from_entity: "zq-fixture-a".into(),
            to_entity: "zq-fixture-b".into(),
            relation_type: RelationType::RelatedTo,
            context: "fixture".into(),
        }],
        code_reference: None,
        author: None,
        derived_from: Vec::new(),
    }
}

/// Write a concept record whose description is `text` and wait until it
/// is embedded; returns its id.
pub async fn write_text(
    records: &post_cortex_memory::capabilities::Records,
    session_id: Uuid,
    text: &str,
) -> Result<Uuid> {
    let req = record(
        session_id,
        post_cortex::core::context_update::UpdateType::ConceptDefined,
        "Incremental Update",
        text,
    );
    write(records, req).await
}

/// Write `req` and wait until it is embedded; returns its id.
pub async fn write(
    records: &post_cortex_memory::capabilities::Records,
    req: post_cortex::services::UpdateContextRequest,
) -> Result<Uuid> {
    let resp = records.write(req).await.map_err(|e| anyhow::anyhow!(e))?;
    assert!(records.wait_idle(std::time::Duration::from_secs(120)).await);
    Ok(resp.entry_id)
}

/// Memory with `config` over `storage`, composed as the daemon composes it.
pub async fn open_memory(
    storage: post_cortex::compose::StorageConfig,
    config: SystemConfig,
) -> Result<Memory> {
    let models = post_cortex::compose::Models::new(&post_cortex::compose::ModelConfig::default());
    post_cortex::compose::open_memory(
        &storage,
        &models,
        None,
        config,
        post_cortex::PipelineConfig::default(),
    )
    .await
    .map_err(|e| anyhow::anyhow!(e))
}

/// RocksDB in `dir`.
pub fn rocksdb(dir: &std::path::Path) -> post_cortex::compose::StorageConfig {
    post_cortex::compose::StorageConfig::RocksDb {
        data_directory: dir.to_path_buf(),
    }
}

/// Memory with embeddings over RocksDB in a temp dir (the recency tests).
pub async fn embedded_memory() -> Result<(Arc<Memory>, tempfile::TempDir)> {
    let temp_dir = tempdir()?;
    let config = SystemConfig {
        enable_embeddings: true,
        auto_vectorize_on_update: true,
        ..Default::default()
    };

    let system = Arc::new(open_memory(rocksdb(temp_dir.path()), config).await?);

    Ok((system, temp_dir))
}
