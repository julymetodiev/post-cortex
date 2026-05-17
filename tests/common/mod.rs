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
use post_cortex_memory::{ConversationMemorySystem, SystemConfig};
#[cfg(feature = "surrealdb-storage")]
use post_cortex::storage::traits::StorageBackendType;
use std::sync::Arc;
use tempfile::{TempDir, tempdir};
use uuid::Uuid;

/// Test fixture builder pattern for creating test systems with content.
/// Uses SurrealDB kv-mem backend (same storage path as production) when
/// the `surrealdb-storage` feature is enabled, otherwise falls back to RocksDB.
pub struct TestFixture {
    pub system: Arc<ConversationMemorySystem>,
    pub session_id: Uuid,
    pub _temp_dir: TempDir,
}

impl TestFixture {
    /// Create a new test fixture with a session
    pub async fn new() -> Result<Self> {
        let temp_dir = tempdir()?;
        let mut config = SystemConfig::default();
        config.data_directory = temp_dir.path().to_str().unwrap().to_string();
        config.enable_embeddings = true;
        config.auto_vectorize_on_update = true;

        // Use SurrealDB kv-mem backend — same storage path as production daemon
        #[cfg(feature = "surrealdb-storage")]
        {
            config.storage_backend = StorageBackendType::SurrealDB;
            config.surrealdb_endpoint = Some("mem://".to_string());
            config.surrealdb_namespace = Some("test".to_string());
            config.surrealdb_database = Some("test".to_string());
        }

        let system = Arc::new(
            ConversationMemorySystem::new(config)
                .await
                .map_err(|e| anyhow::anyhow!(e))?,
        );

        let session_id = system
            .create_session(
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
            fixture
                .system
                .add_incremental_update(fixture.session_id, text.to_string(), None)
                .await
                .map_err(|e| anyhow::anyhow!(e))?;
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
        let engine = self
            .system
            .ensure_semantic_engine_initialized()
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        let results = engine
            .semantic_search_session(self.session_id, query, None, None, Some(bias))
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        Ok(results)
    }
}
