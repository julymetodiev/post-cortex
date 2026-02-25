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
use post_cortex::core::memory_system::{
    ConversationMemorySystem, SystemConfig,
};
use std::sync::Arc;
use tempfile::{tempdir, TempDir};
use uuid::Uuid;

/// Test fixture builder pattern for creating test systems with content
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

        let system = Arc::new(
            ConversationMemorySystem::new(config)
                .await
                .map_err(|e| anyhow::anyhow!(e))?,
        );

        let session_id = system
            .create_session(Some("test-session".to_string()), Some("Test session".to_string()))
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

    /// Create fixture with named session
    pub async fn with_session_name(name: &str, description: &str) -> Result<Self> {
        let fixture = Self::new().await?;

        let session_id = fixture
            .system
            .create_session(Some(name.to_string()), Some(description.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        Ok(Self {
            system: fixture.system,
            session_id,
            _temp_dir: fixture._temp_dir,
        })
    }

    /// Search with recency bias
    pub async fn search_with_bias(
        &self,
        query: &str,
        bias: f32,
    ) -> Result<Vec<post_cortex::core::content_vectorizer::SemanticSearchResult>> {
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

    /// Search without recency bias (default)
    pub async fn search(
        &self,
        query: &str,
    ) -> Result<Vec<post_cortex::core::content_vectorizer::SemanticSearchResult>> {
        self.search_with_bias(query, 0.0).await
    }

    /// Add content to existing session
    pub async fn add_content(&mut self, text: &str) -> Result<()> {
        self.system
            .add_incremental_update(self.session_id, text.to_string(), None)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        // Wait for vectorization
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        Ok(())
    }

    /// Get semantic engine
    pub async fn engine(&self) -> Result<Arc<post_cortex::core::semantic_query_engine::SemanticQueryEngine>> {
        self.system
            .ensure_semantic_engine_initialized()
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }
}
