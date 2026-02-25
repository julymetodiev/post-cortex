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

//! SurrealDB storage backend for post-cortex.
//!
//! This module provides a SurrealDB-based storage implementation with:
//! - Native graph support for entity relationships
//! - HNSW vector indexing for embeddings
//! - Efficient session and workspace management
//!
//! Enable with: `cargo build --features surrealdb-storage`

use crate::core::context_update::{
    ContextUpdate, EntityData, EntityRelationship, EntityType, RelationType,
};
use crate::core::vector_db::{SearchMatch, VectorMetadata};
use crate::core::structured_context::StructuredContext;
use crate::graph::entity_graph::{EntityNetwork, SimpleEntityGraph};
use crate::session::active_session::{ActiveSession, ChangeRecord, CodeReference, UserPreferences};
use crate::storage::rocksdb_storage::{SessionCheckpoint, StoredWorkspace};
use crate::storage::traits::{GraphStorage, Storage, VectorStorage};
use crate::workspace::SessionRole;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};
use surrealdb::opt::auth::Root;
use surrealdb::types::SurrealValue;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Embedding dimension (must match the embedding model)
const EMBEDDING_DIMENSION: usize = 384;

/// SurrealDB storage implementation supporting both local (RocksDB) and remote (WebSocket) backends
#[derive(Clone)]
pub struct SurrealDBStorage {
    db: Arc<Surreal<Any>>,
    #[allow(dead_code)]
    namespace: String,
    #[allow(dead_code)]
    database: String,
}

// ============================================================================
// SurrealDB Record Types
// ============================================================================

/// Session record for SurrealDB - NORMALIZED (no JSON blobs for context!)
/// Context updates are stored in context_update table
/// Entities are stored in entity table (native graph)
/// Relationships are stored via RELATE (native graph)
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct SessionRecord {
    session_id: String,
    name: Option<String>,
    description: Option<String>,
    created_at: String,
    last_updated: String,
    // User preferences as JSON (small, queryable)
    user_preferences: JsonValue,
    // Configuration (native scalars)
    max_extracted_entities: u32,
    max_referenced_entities: u32,
    enable_smart_entity_ranking: bool,
    // Metrics (native scalars)
    total_entity_truncations: u32,
    total_entities_truncated: u32,
    // Vectorization tracking
    vectorized_update_ids: Vec<String>,
    // Total updates count (for pagination)
    total_updates: u32,
}

/// Context update record for SurrealDB - hybrid storage
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct ContextUpdateRecord {
    update_id: String,
    session_id: String,
    timestamp: String,
    update_type: String,
    // Full update as JSON (queryable object!)
    update_data: JsonValue,
}

/// Entity record for SurrealDB graph nodes
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct EntityRecord {
    session_id: String,
    name: String,
    entity_type: String,
    first_mentioned: String,
    last_mentioned: String,
    mention_count: u32,
    importance_score: f32,
    description: Option<String>,
}

/// Embedding record for vector storage
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct EmbeddingRecord {
    content_id: String,
    session_id: String,
    vector: Vec<f32>,
    text: String,
    content_type: String,
    timestamp: String,
    metadata: HashMap<String, String>,
}

/// KNN search result with distance from HNSW index
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct KnnResult {
    content_id: String,
    session_id: String,
    text: String,
    content_type: String,
    timestamp: String,
    metadata: HashMap<String, String>,
    distance: f32,
}

/// Workspace record for SurrealDB
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct WorkspaceRecord {
    workspace_id: String,
    name: String,
    description: String,
    created_at: u64,
}

/// Workspace-Session association record
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct WorkspaceSessionRecord {
    workspace_id: String,
    session_id: String,
    role: String,
    added_at: u64,
}

/// Checkpoint record for SurrealDB - hybrid storage
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct CheckpointRecord {
    checkpoint_id: String,
    session_id: String,
    created_at: String,
    // Complete context snapshot as JSON (queryable)
    structured_context: JsonValue,
    recent_updates: JsonValue,
    code_references: JsonValue,
    change_history: JsonValue,
    // Metadata (native scalars)
    total_updates: u32,
    context_quality_score: f32,
    compression_ratio: f32,
}

impl SurrealDBStorage {
    /// Create a new SurrealDB storage instance with remote WebSocket connection
    ///
    /// # Arguments
    /// * `endpoint` - WebSocket endpoint (e.g., "localhost:8000" or "ws://localhost:8000")
    /// * `username` - Database username (optional, for authentication)
    /// * `password` - Database password (optional, for authentication)
    ///
    /// # Example
    /// ```bash
    /// docker run -d -p 8000:8000 surrealdb/surrealdb:latest start --user root --pass root
    /// ```
    /// ```rust
    /// let storage = SurrealDBStorage::new("localhost:8000", Some("root"), Some("root"), None, None).await?;
    /// ```
    pub async fn new(
        endpoint: &str,
        username: Option<&str>,
        password: Option<&str>,
        namespace: Option<&str>,
        database: Option<&str>,
    ) -> Result<Self> {
        // Normalize endpoint (add ws:// if not present and not using other engines)
        let endpoint = if endpoint.contains("://") {
            endpoint.to_string()
        } else {
            format!("ws://{}", endpoint)
        };

        info!(
            "SurrealDBStorage: Connecting to remote SurrealDB at {}",
            endpoint
        );

        // Connect to SurrealDB via WebSocket
        let db = connect(&endpoint).await?;

        // Authenticate if credentials provided
        if let (Some(user), Some(pass)) = (username, password) {
            db.signin(Root {
                username: user.to_string(),
                password: pass.to_string(),
            })
            .await?;
            info!("SurrealDBStorage: Authenticated as {}", user);
        }

        let namespace = namespace.unwrap_or("post_cortex").to_string();
        let database = database.unwrap_or("main").to_string();

        info!(
            "SurrealDBStorage: Using namespace '{}', database '{}'",
            namespace, database
        );

        // Select namespace and database
        db.use_ns(&namespace).use_db(&database).await?;

        let storage = Self {
            db: Arc::new(db),
            namespace,
            database,
        };

        // Initialize schema
        storage.initialize_schema().await?;

        info!("SurrealDBStorage: Remote instance initialized successfully");

        Ok(storage)
    }

    /// Select all records from a table
    async fn select_all<T: for<'de> Deserialize<'de> + SurrealValue>(
        &self,
        table: &str,
    ) -> surrealdb::Result<Vec<T>> {
        self.db.select(table).await
    }

    /// Select a single record by ID (table, id)
    async fn select_one<T: for<'de> Deserialize<'de> + SurrealValue>(
        &self,
        table: &str,
        id: &str,
    ) -> surrealdb::Result<Option<T>> {
        self.db.select((table, id)).await
    }

    /// Delete a record and return it (table, id)
    async fn delete<T: for<'de> Deserialize<'de> + SurrealValue>(
        &self,
        table: &str,
        id: &str,
    ) -> surrealdb::Result<Option<T>> {
        self.db.delete((table, id)).await
    }

    /// Initialize the database schema
    async fn initialize_schema(&self) -> Result<()> {
        info!("SurrealDBStorage: Initializing schema...");

        // First, remove old binary 'data' fields if they exist (schema migration)
        let cleanup = r#"
            -- Remove old binary data fields from previous schema versions
            REMOVE FIELD IF EXISTS data ON session;
            REMOVE FIELD IF EXISTS data ON context_update;
            REMOVE FIELD IF EXISTS data ON checkpoint;
        "#;

        // Ignore cleanup errors (fields may not exist)
        let _ = self.db.query(cleanup).await;

        // Define tables with schema - using SCHEMALESS for complex nested JSON storage
        // This allows storing arbitrary JSON structures while still being queryable
        let schema = r#"
            -- Sessions table (SCHEMALESS to allow complex nested JSON)
            DEFINE TABLE IF NOT EXISTS session SCHEMALESS;
            DEFINE INDEX IF NOT EXISTS idx_session_id ON session FIELDS session_id UNIQUE;
            DEFINE INDEX IF NOT EXISTS idx_session_created ON session FIELDS created_at;

            -- Context updates table (SCHEMALESS for complex nested JSON)
            DEFINE TABLE IF NOT EXISTS context_update SCHEMALESS;
            DEFINE INDEX IF NOT EXISTS idx_update_id ON context_update FIELDS update_id UNIQUE;
            DEFINE INDEX IF NOT EXISTS idx_update_session ON context_update FIELDS session_id;
            DEFINE INDEX IF NOT EXISTS idx_update_timestamp ON context_update FIELDS timestamp;
            -- Composite index for efficient session queries with ORDER BY timestamp
            DEFINE INDEX IF NOT EXISTS idx_update_session_time ON context_update FIELDS session_id, timestamp;

            -- Entities table (graph nodes)
            DEFINE TABLE IF NOT EXISTS entity SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS session_id ON entity TYPE string;
            DEFINE FIELD IF NOT EXISTS name ON entity TYPE string;
            DEFINE FIELD IF NOT EXISTS entity_type ON entity TYPE string;
            DEFINE FIELD IF NOT EXISTS first_mentioned ON entity TYPE string;
            DEFINE FIELD IF NOT EXISTS last_mentioned ON entity TYPE string;
            DEFINE FIELD IF NOT EXISTS mention_count ON entity TYPE int;
            DEFINE FIELD IF NOT EXISTS importance_score ON entity TYPE float;
            DEFINE FIELD IF NOT EXISTS description ON entity TYPE option<string>;
            DEFINE INDEX IF NOT EXISTS idx_entity_session_name ON entity FIELDS session_id, name UNIQUE;
            DEFINE INDEX IF NOT EXISTS idx_entity_session ON entity FIELDS session_id;
            DEFINE INDEX IF NOT EXISTS idx_entity_type ON entity FIELDS entity_type;
            DEFINE INDEX IF NOT EXISTS idx_entity_importance ON entity FIELDS importance_score;
            -- Composite index for efficient entity queries with ORDER BY importance_score
            DEFINE INDEX IF NOT EXISTS idx_entity_session_importance ON entity FIELDS session_id, importance_score;

            -- Relation tables for graph edges (schemaless for flexibility)
            DEFINE TABLE IF NOT EXISTS required_by SCHEMALESS;
            DEFINE INDEX IF NOT EXISTS idx_required_by_session ON required_by FIELDS session_id;

            DEFINE TABLE IF NOT EXISTS leads_to SCHEMALESS;
            DEFINE INDEX IF NOT EXISTS idx_leads_to_session ON leads_to FIELDS session_id;

            DEFINE TABLE IF NOT EXISTS related_to SCHEMALESS;
            DEFINE INDEX IF NOT EXISTS idx_related_to_session ON related_to FIELDS session_id;

            DEFINE TABLE IF NOT EXISTS conflicts_with SCHEMALESS;
            DEFINE INDEX IF NOT EXISTS idx_conflicts_with_session ON conflicts_with FIELDS session_id;

            DEFINE TABLE IF NOT EXISTS depends_on SCHEMALESS;
            DEFINE INDEX IF NOT EXISTS idx_depends_on_session ON depends_on FIELDS session_id;

            DEFINE TABLE IF NOT EXISTS implements SCHEMALESS;
            DEFINE INDEX IF NOT EXISTS idx_implements_session ON implements FIELDS session_id;

            DEFINE TABLE IF NOT EXISTS caused_by SCHEMALESS;
            DEFINE INDEX IF NOT EXISTS idx_caused_by_session ON caused_by FIELDS session_id;

            DEFINE TABLE IF NOT EXISTS solves SCHEMALESS;
            DEFINE INDEX IF NOT EXISTS idx_solves_session ON solves FIELDS session_id;

            -- Embeddings table with vector support
            DEFINE TABLE IF NOT EXISTS embedding SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS content_id ON embedding TYPE string;
            DEFINE FIELD IF NOT EXISTS session_id ON embedding TYPE string;
            DEFINE FIELD IF NOT EXISTS vector ON embedding TYPE array<float>;
            DEFINE FIELD IF NOT EXISTS text ON embedding TYPE string;
            DEFINE FIELD IF NOT EXISTS content_type ON embedding TYPE string;
            DEFINE FIELD IF NOT EXISTS timestamp ON embedding TYPE string;
            DEFINE FIELD IF NOT EXISTS metadata ON embedding TYPE object;
            DEFINE INDEX IF NOT EXISTS idx_embedding_content ON embedding FIELDS content_id UNIQUE;
            DEFINE INDEX IF NOT EXISTS idx_embedding_session ON embedding FIELDS session_id;
            DEFINE INDEX IF NOT EXISTS idx_embedding_type ON embedding FIELDS content_type;
            -- HNSW vector index for fast similarity search (384-dim MiniLM embeddings)
            -- This enables O(log n) KNN instead of O(n) full scan!
            DEFINE INDEX IF NOT EXISTS idx_embedding_hnsw ON embedding FIELDS vector HNSW DIMENSION 384 DIST COSINE;

            -- Workspaces table
            DEFINE TABLE IF NOT EXISTS workspace SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS workspace_id ON workspace TYPE string;
            DEFINE FIELD IF NOT EXISTS name ON workspace TYPE string;
            DEFINE FIELD IF NOT EXISTS description ON workspace TYPE string;
            DEFINE FIELD IF NOT EXISTS created_at ON workspace TYPE int;
            DEFINE INDEX IF NOT EXISTS idx_workspace_id ON workspace FIELDS workspace_id UNIQUE;
            DEFINE INDEX IF NOT EXISTS idx_workspace_name ON workspace FIELDS name;

            -- Workspace-Session associations
            DEFINE TABLE IF NOT EXISTS workspace_session SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS workspace_id ON workspace_session TYPE string;
            DEFINE FIELD IF NOT EXISTS session_id ON workspace_session TYPE string;
            DEFINE FIELD IF NOT EXISTS role ON workspace_session TYPE string;
            DEFINE FIELD IF NOT EXISTS added_at ON workspace_session TYPE int;
            DEFINE INDEX IF NOT EXISTS idx_ws_workspace ON workspace_session FIELDS workspace_id;
            DEFINE INDEX IF NOT EXISTS idx_ws_session ON workspace_session FIELDS session_id;
            DEFINE INDEX IF NOT EXISTS idx_ws_unique ON workspace_session FIELDS workspace_id, session_id UNIQUE;

            -- Checkpoints table (SCHEMALESS for complex nested JSON)
            DEFINE TABLE IF NOT EXISTS checkpoint SCHEMALESS;
            DEFINE INDEX IF NOT EXISTS idx_checkpoint_id ON checkpoint FIELDS checkpoint_id UNIQUE;
            DEFINE INDEX IF NOT EXISTS idx_checkpoint_session ON checkpoint FIELDS session_id;
        "#;

        self.db.query(schema).await?;

        info!("SurrealDBStorage: Schema initialized successfully");

        Ok(())
    }

    /// Get the relation table name for a relation type
    fn relation_table_name(relation_type: &RelationType) -> &'static str {
        match relation_type {
            RelationType::RequiredBy => "required_by",
            RelationType::LeadsTo => "leads_to",
            RelationType::RelatedTo => "related_to",
            RelationType::ConflictsWith => "conflicts_with",
            RelationType::DependsOn => "depends_on",
            RelationType::Implements => "implements",
            RelationType::CausedBy => "caused_by",
            RelationType::Solves => "solves",
        }
    }

    /// Generate a deterministic ID for a relationship to avoid duplicates
    fn relation_id(
        session_id: Uuid,
        from_entity: &str,
        to_entity: &str,
        rel_type: &RelationType,
    ) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        session_id.hash(&mut hasher);
        from_entity.hash(&mut hasher);
        to_entity.hash(&mut hasher);
        format!("{:?}", rel_type).hash(&mut hasher);
        let hash = hasher.finish();

        format!("{}_{:x}", session_id, hash)
    }

    /// Generate a deterministic ID for an entity
    fn entity_id(session_id: Uuid, name: &str) -> String {
        format!("{}_{}", session_id, name)
    }

    /// Parse RFC3339 datetime string, fallback to current time on error
    fn parse_datetime(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now())
    }

    /// Parse relation type from string
    #[allow(dead_code)]
    fn parse_relation_type(s: &str) -> RelationType {
        match s {
            "required_by" => RelationType::RequiredBy,
            "leads_to" => RelationType::LeadsTo,
            "related_to" => RelationType::RelatedTo,
            "conflicts_with" => RelationType::ConflictsWith,
            "depends_on" => RelationType::DependsOn,
            "implements" => RelationType::Implements,
            "caused_by" => RelationType::CausedBy,
            "solves" => RelationType::Solves,
            _ => RelationType::RelatedTo,
        }
    }

    /// Parse entity type from string
    fn parse_entity_type(s: &str) -> EntityType {
        match s.to_lowercase().as_str() {
            "technology" => EntityType::Technology,
            "concept" => EntityType::Concept,
            "problem" => EntityType::Problem,
            "solution" => EntityType::Solution,
            "decision" => EntityType::Decision,
            "codecomponent" | "code_component" => EntityType::CodeComponent,
            _ => EntityType::Concept,
        }
    }
}

// ============================================================================
// Storage Trait Implementation
// ============================================================================

#[async_trait]
impl Storage for SurrealDBStorage {
    async fn save_session(&self, session: &ActiveSession) -> Result<()> {
        debug!(
            "SurrealDBStorage: Saving session with ID: {} (normalized)",
            session.id()
        );

        // Get all context updates for normalized storage
        let all_updates: Vec<ContextUpdate> = session.hot_context.iter();
        let total_updates = all_updates.len() as u32;

        // Save session metadata ONLY (no JSON blobs for context!)
        let record = SessionRecord {
            session_id: session.id().to_string(),
            name: session.name().clone(),
            description: session.description().clone(),
            created_at: session.created_at().to_rfc3339(),
            last_updated: Utc::now().to_rfc3339(),
            user_preferences: serde_json::to_value(session.user_preferences())?,
            max_extracted_entities: session.max_extracted_entities as u32,
            max_referenced_entities: session.max_referenced_entities as u32,
            enable_smart_entity_ranking: session.enable_smart_entity_ranking,
            total_entity_truncations: session.total_entity_truncations as u32,
            total_entities_truncated: session.total_entities_truncated as u32,
            vectorized_update_ids: session
                .vectorized_update_ids
                .iter()
                .map(|id| id.to_string())
                .collect(),
            total_updates,
        };

        // Upsert session metadata
        let _: Option<SessionRecord> = self
            .db
            .upsert(("session", session.id().to_string()))
            .content(record)
            .await?;

        // Save ALL context updates to normalized table
        // This is the source of truth - not JSON blobs!
        if !all_updates.is_empty() {
            if let Err(e) = self.batch_save_updates(session.id(), all_updates).await {
                warn!("Failed to save context updates: {}", e);
            }
        }

        // Save entities to native graph table
        for entity in session.entity_graph.get_all_entities() {
            if let Err(e) = self.upsert_entity(session.id(), &entity).await {
                warn!("Failed to save entity '{}': {}", entity.name, e);
            }
        }

        // Save relationships via native RELATE
        for rel in session.entity_graph.get_all_relationships() {
            if let Err(e) = self.create_relationship(session.id(), &rel).await {
                warn!(
                    "Failed to save relationship '{}' -> '{}': {}",
                    rel.from_entity, rel.to_entity, e
                );
            }
        }

        debug!(
            "SurrealDBStorage: Session saved (normalized) - {} updates, {} entities",
            total_updates,
            session.entity_graph.get_all_entities().len()
        );

        Ok(())
    }

    async fn load_session(&self, session_id: Uuid) -> Result<ActiveSession> {
        debug!(
            "SurrealDBStorage: Loading session with ID: {} (normalized)",
            session_id
        );

        // Load session metadata
        let record: Option<SessionRecord> =
            self.db.select(("session", session_id.to_string())).await?;

        let r = record.ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        // Load ALL context updates from normalized table (source of truth!)
        let all_updates = self
            .load_session_updates(session_id)
            .await
            .unwrap_or_default();
        debug!(
            "SurrealDBStorage: Loaded {} updates from context_update table",
            all_updates.len()
        );

        // Hot context = last 50 updates (same as HotContext::MAX_SIZE)
        let hot_context: Vec<ContextUpdate> =
            all_updates.iter().rev().take(50).rev().cloned().collect();

        // Load entities from native graph table
        let entities = self.list_entities(session_id).await.unwrap_or_default();
        let mut entity_graph = SimpleEntityGraph::new();
        for entity in &entities {
            entity_graph.add_or_update_entity(
                entity.name.clone(),
                entity.entity_type.clone(),
                entity.last_mentioned,
                entity.description.as_deref().unwrap_or(""),
            );
        }

        // Load relationships from native graph and add to entity_graph
        let relationships = self
            .load_all_relationships(session_id)
            .await
            .unwrap_or_default();
        for rel in relationships {
            entity_graph.add_relationship(rel);
        }
        debug!(
            "SurrealDBStorage: Loaded {} entities, graph rebuilt",
            entities.len()
        );

        // Rebuild StructuredContext from updates
        let current_state = Self::rebuild_structured_context(&all_updates);

        // Extract code_references and change_history from updates
        let code_references = Self::extract_code_references(&all_updates);
        let change_history = Self::extract_change_history(&all_updates);

        // Parse user preferences
        let user_preferences: UserPreferences = serde_json::from_value(r.user_preferences)
            .unwrap_or_else(|_| UserPreferences {
                auto_save_enabled: true,
                context_retention_days: 30,
                max_hot_context_size: 50,
                auto_summary_threshold: 100,
                important_keywords: Vec::new(),
            });

        // Parse vectorized_update_ids
        let vectorized_ids: Vec<Uuid> = r
            .vectorized_update_ids
            .iter()
            .filter_map(|s| Uuid::parse_str(s).ok())
            .collect();

        // Reconstruct ActiveSession from normalized data
        let session = ActiveSession::from_components(
            session_id,
            r.name,
            r.description,
            DateTime::parse_from_rfc3339(&r.created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            DateTime::parse_from_rfc3339(&r.last_updated)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            user_preferences,
            hot_context,
            Vec::new(), // warm_context - compressed from older updates if needed
            Vec::new(), // cold_context - summaries generated on demand
            current_state,
            all_updates, // All updates from normalized table
            code_references,
            change_history,
            entity_graph,
            r.max_extracted_entities as usize,
            r.max_referenced_entities as usize,
            r.enable_smart_entity_ranking,
            r.total_entity_truncations as usize,
            r.total_entities_truncated as usize,
            vectorized_ids,
        );

        debug!(
            "SurrealDBStorage: Session loaded (normalized) - {} updates, {} entities",
            session.hot_context.len(),
            entities.len()
        );
        Ok(session)
    }

    async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        debug!("SurrealDBStorage: Deleting session with ID: {}", session_id);

        // Delete session
        let _: Option<SessionRecord> = self.db.delete(("session", session_id.to_string())).await?;

        // Delete all context updates for this session
        self.db
            .query("DELETE context_update WHERE session_id = $session_id")
            .bind(("session_id", session_id.to_string()))
            .await?;

        // Delete all entities for this session
        self.db
            .query("DELETE entity WHERE session_id = $session_id")
            .bind(("session_id", session_id.to_string()))
            .await?;

        // Delete all embeddings for this session
        self.db
            .query("DELETE embedding WHERE session_id = $session_id")
            .bind(("session_id", session_id.to_string()))
            .await?;

        // Delete all relations for this session
        for table in [
            "required_by",
            "leads_to",
            "related_to",
            "conflicts_with",
            "depends_on",
            "implements",
            "caused_by",
            "solves",
        ] {
            self.db
                .query(format!("DELETE {} WHERE session_id = $session_id", table))
                .bind(("session_id", session_id.to_string()))
                .await?;
        }

        debug!("SurrealDBStorage: Session deleted successfully");

        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<Uuid>> {
        debug!("SurrealDBStorage: Listing sessions");

        let records: Vec<SessionRecord> = self.select_all("session").await?;

        let sessions: Vec<Uuid> = records
            .into_iter()
            .filter_map(|r| Uuid::parse_str(&r.session_id).ok())
            .collect();

        debug!("SurrealDBStorage: Found {} sessions", sessions.len());

        Ok(sessions)
    }

    async fn session_exists(&self, session_id: Uuid) -> Result<bool> {
        let record: Option<SessionRecord> =
            self.db.select(("session", session_id.to_string())).await?;

        Ok(record.is_some())
    }

    async fn batch_save_updates(
        &self,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        debug!(
            "SurrealDBStorage: Batch saving {} updates for session {}",
            updates.len(),
            session_id
        );

        for update in updates {
            let record = ContextUpdateRecord {
                update_id: update.id.to_string(),
                session_id: session_id.to_string(),
                timestamp: update.timestamp.to_rfc3339(),
                update_type: format!("{:?}", update.update_type),
                update_data: serde_json::to_value(&update)?,
            };

            let _: Option<ContextUpdateRecord> = self
                .db
                .upsert(("context_update", record.update_id.clone()))
                .content(record)
                .await?;
        }

        debug!("SurrealDBStorage: Batch save completed");

        Ok(())
    }

    async fn load_session_updates(&self, session_id: Uuid) -> Result<Vec<ContextUpdate>> {
        debug!(
            "SurrealDBStorage: Loading updates for session {}",
            session_id
        );
        info!(
            "SurrealDBStorage: Fetching updates for session {}...",
            session_id
        );

        let mut updates = Vec::new();
        let mut start = 0;
        let limit = 1000;

        loop {
            let mut response = self
                .db
                .query("SELECT * FROM context_update WHERE session_id = $session_id ORDER BY timestamp LIMIT $limit START $start")
                .bind(("session_id", session_id.to_string()))
                .bind(("limit", limit))
                .bind(("start", start))
                .await?;

            let records: Vec<ContextUpdateRecord> = response.take(0)?;
            let count = records.len();

            if count == 0 {
                break;
            }

            // Parse update_data JSON back to ContextUpdate
            for r in records {
                if let Ok(update) = serde_json::from_value(r.update_data) {
                    updates.push(update);
                }
            }

            if updates.len() % 1000 == 0 {
                info!(
                    "SurrealDBStorage: Loaded {} updates for session {}...",
                    updates.len(),
                    session_id
                );
            }

            if count < limit {
                break;
            }

            start += count;
        }

        debug!("SurrealDBStorage: Loaded {} updates", updates.len());

        Ok(updates)
    }

    async fn save_checkpoint(&self, checkpoint: &SessionCheckpoint) -> Result<()> {
        debug!(
            "SurrealDBStorage: Saving checkpoint with ID: {}",
            checkpoint.id
        );

        let record = CheckpointRecord {
            checkpoint_id: checkpoint.id.to_string(),
            session_id: checkpoint.session_id.to_string(),
            created_at: checkpoint.created_at.to_rfc3339(),
            structured_context: serde_json::to_value(&checkpoint.structured_context)?,
            recent_updates: serde_json::to_value(&checkpoint.recent_updates)?,
            code_references: serde_json::to_value(&checkpoint.code_references)?,
            change_history: serde_json::to_value(&checkpoint.change_history)?,
            total_updates: checkpoint.total_updates as u32,
            context_quality_score: checkpoint.context_quality_score,
            compression_ratio: checkpoint.compression_ratio,
        };

        let _: Option<CheckpointRecord> = self
            .db
            .upsert(("checkpoint", checkpoint.id.to_string()))
            .content(record)
            .await?;

        debug!("SurrealDBStorage: Checkpoint saved successfully");

        Ok(())
    }

    async fn load_checkpoint(&self, checkpoint_id: Uuid) -> Result<SessionCheckpoint> {
        debug!(
            "SurrealDBStorage: Loading checkpoint with ID: {}",
            checkpoint_id
        );

        let record: Option<CheckpointRecord> = self
            .db
            .select(("checkpoint", checkpoint_id.to_string()))
            .await?;

        match record {
            Some(r) => {
                let checkpoint = SessionCheckpoint {
                    id: Uuid::parse_str(&r.checkpoint_id)?,
                    session_id: Uuid::parse_str(&r.session_id)?,
                    created_at: DateTime::parse_from_rfc3339(&r.created_at)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    structured_context: serde_json::from_value(r.structured_context)?,
                    recent_updates: serde_json::from_value(r.recent_updates)?,
                    code_references: serde_json::from_value(r.code_references)?,
                    change_history: serde_json::from_value(r.change_history)?,
                    total_updates: r.total_updates as usize,
                    context_quality_score: r.context_quality_score,
                    compression_ratio: r.compression_ratio,
                };
                Ok(checkpoint)
            }
            None => Err(anyhow::anyhow!("Checkpoint not found")),
        }
    }

    async fn list_checkpoints(&self) -> Result<Vec<SessionCheckpoint>> {
        debug!("SurrealDBStorage: Listing checkpoints");

        let records: Vec<CheckpointRecord> = self.select_all("checkpoint").await?;

        let checkpoints: Vec<SessionCheckpoint> = records
            .into_iter()
            .filter_map(|r| {
                Some(SessionCheckpoint {
                    id: Uuid::parse_str(&r.checkpoint_id).ok()?,
                    session_id: Uuid::parse_str(&r.session_id).ok()?,
                    created_at: DateTime::parse_from_rfc3339(&r.created_at)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    structured_context: serde_json::from_value(r.structured_context).ok()?,
                    recent_updates: serde_json::from_value(r.recent_updates).ok()?,
                    code_references: serde_json::from_value(r.code_references).ok()?,
                    change_history: serde_json::from_value(r.change_history).ok()?,
                    total_updates: r.total_updates as usize,
                    context_quality_score: r.context_quality_score,
                    compression_ratio: r.compression_ratio,
                })
            })
            .collect();

        debug!("SurrealDBStorage: Listed {} checkpoints", checkpoints.len());

        Ok(checkpoints)
    }

    async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> Result<()> {
        debug!(
            "SurrealDBStorage: Saving workspace {} ({})",
            name, workspace_id
        );

        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let record = WorkspaceRecord {
            workspace_id: workspace_id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            created_at,
        };

        let _: Option<WorkspaceRecord> = self
            .db
            .upsert(("workspace", workspace_id.to_string()))
            .content(record)
            .await?;

        // Add session associations
        for session_id in session_ids {
            self.add_session_to_workspace(workspace_id, *session_id, SessionRole::Primary)
                .await?;
        }

        debug!("SurrealDBStorage: Workspace saved successfully");

        Ok(())
    }

    async fn delete_workspace(&self, workspace_id: Uuid) -> Result<()> {
        debug!("SurrealDBStorage: Deleting workspace {}", workspace_id);

        // Delete workspace
        let _: Option<WorkspaceRecord> = self
            .db
            .delete(("workspace", workspace_id.to_string()))
            .await?;

        // Delete all workspace-session associations
        self.db
            .query("DELETE workspace_session WHERE workspace_id = $workspace_id")
            .bind(("workspace_id", workspace_id.to_string()))
            .await?;

        debug!("SurrealDBStorage: Workspace deleted successfully");

        Ok(())
    }

    async fn list_workspaces(&self) -> Result<Vec<StoredWorkspace>> {
        debug!("SurrealDBStorage: Listing workspaces");

        let records: Vec<WorkspaceRecord> = self.select_all("workspace").await?;

        let mut workspaces = Vec::new();
        for record in records {
            if let Ok(workspace_id) = Uuid::parse_str(&record.workspace_id) {
                // Get sessions for this workspace
                let mut response = self
                    .db
                    .query("SELECT * FROM workspace_session WHERE workspace_id = $workspace_id")
                    .bind(("workspace_id", record.workspace_id.clone()))
                    .await?;

                let session_records: Vec<WorkspaceSessionRecord> = response.take(0)?;

                let sessions: Vec<(Uuid, SessionRole)> = session_records
                    .into_iter()
                    .filter_map(|s| {
                        Uuid::parse_str(&s.session_id).ok().map(|id| {
                            let role = match s.role.as_str() {
                                "Primary" => SessionRole::Primary,
                                "Related" => SessionRole::Related,
                                "Dependency" => SessionRole::Dependency,
                                "Shared" => SessionRole::Shared,
                                _ => SessionRole::Primary,
                            };
                            (id, role)
                        })
                    })
                    .collect();

                workspaces.push(StoredWorkspace {
                    id: workspace_id,
                    name: record.name,
                    description: record.description,
                    sessions,
                    created_at: record.created_at,
                });
            }
        }

        debug!("SurrealDBStorage: Listed {} workspaces", workspaces.len());

        Ok(workspaces)
    }

    async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: SessionRole,
    ) -> Result<()> {
        debug!(
            "SurrealDBStorage: Adding session {} to workspace {} with role {:?}",
            session_id, workspace_id, role
        );

        let added_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let record = WorkspaceSessionRecord {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
            role: format!("{:?}", role),
            added_at,
        };

        let key = format!("{}_{}", workspace_id, session_id);
        let _: Option<WorkspaceSessionRecord> = self
            .db
            .upsert(("workspace_session", key))
            .content(record)
            .await?;

        Ok(())
    }

    async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<()> {
        debug!(
            "SurrealDBStorage: Removing session {} from workspace {}",
            session_id, workspace_id
        );

        let key = format!("{}_{}", workspace_id, session_id);
        let _: Option<WorkspaceSessionRecord> = self.delete("workspace_session", &key).await?;

        Ok(())
    }

    async fn compact(&self) -> Result<()> {
        debug!("SurrealDBStorage: Compact requested (no-op for SurrealDB)");
        // SurrealDB handles compaction internally
        Ok(())
    }

    async fn get_key_count(&self) -> Result<usize> {
        let mut response = self
            .db
            .query(
                r#"
                RETURN (SELECT count() FROM session GROUP ALL).count +
                       (SELECT count() FROM context_update GROUP ALL).count +
                       (SELECT count() FROM entity GROUP ALL).count +
                       (SELECT count() FROM embedding GROUP ALL).count +
                       (SELECT count() FROM workspace GROUP ALL).count +
                       (SELECT count() FROM checkpoint GROUP ALL).count
            "#,
            )
            .await?;

        let count: Option<i64> = response.take(0)?;

        Ok(count.unwrap_or(0) as usize)
    }

    async fn get_stats(&self) -> Result<String> {
        let mut response = self
            .db
            .query(
                r#"
                RETURN {
                    sessions: (SELECT count() FROM session GROUP ALL).count,
                    updates: (SELECT count() FROM context_update GROUP ALL).count,
                    entities: (SELECT count() FROM entity GROUP ALL).count,
                    embeddings: (SELECT count() FROM embedding GROUP ALL).count,
                    workspaces: (SELECT count() FROM workspace GROUP ALL).count,
                    checkpoints: (SELECT count() FROM checkpoint GROUP ALL).count
                }
            "#,
            )
            .await?;

        let stats: Option<serde_json::Value> = response.take(0)?;

        Ok(serde_json::to_string_pretty(&stats.unwrap_or_default())?)
    }
}

// ============================================================================
// GraphStorage Trait Implementation
// ============================================================================

#[async_trait]
impl GraphStorage for SurrealDBStorage {
    async fn upsert_entity(&self, session_id: Uuid, entity: &EntityData) -> Result<()> {
        debug!(
            "SurrealDBStorage: Upserting entity '{}' for session {}",
            entity.name, session_id
        );

        let entity_id = Self::entity_id(session_id, &entity.name);

        let record = EntityRecord {
            session_id: session_id.to_string(),
            name: entity.name.clone(),
            entity_type: format!("{:?}", entity.entity_type),
            first_mentioned: entity.first_mentioned.to_rfc3339(),
            last_mentioned: entity.last_mentioned.to_rfc3339(),
            mention_count: entity.mention_count,
            importance_score: entity.importance_score,
            description: entity.description.clone(),
        };

        // Use query with backticks for IDs containing special chars (UUIDs have dashes)
        let query = format!("UPSERT entity:`{}` CONTENT $content", entity_id);
        self.db.query(query).bind(("content", record)).await?;

        Ok(())
    }

    async fn get_entity(&self, session_id: Uuid, name: &str) -> Result<Option<EntityData>> {
        let entity_id = Self::entity_id(session_id, name);

        let record: Option<EntityRecord> = self.select_one("entity", &entity_id).await?;

        Ok(record.map(|r| EntityData {
            name: r.name,
            entity_type: Self::parse_entity_type(&r.entity_type),
            first_mentioned: Self::parse_datetime(&r.first_mentioned),
            last_mentioned: Self::parse_datetime(&r.last_mentioned),
            mention_count: r.mention_count,
            importance_score: r.importance_score,
            description: r.description,
        }))
    }

    async fn list_entities(&self, session_id: Uuid) -> Result<Vec<EntityData>> {
        info!(
            "SurrealDBStorage: Fetching entities for session {}...",
            session_id
        );
        let mut all_entities = Vec::new();
        let mut start = 0;
        let limit = 1000;

        loop {
            let mut response = self
                .db
                .query("SELECT * FROM entity WHERE session_id = $session_id ORDER BY importance_score DESC LIMIT $limit START $start")
                .bind(("session_id", session_id.to_string()))
                .bind(("limit", limit))
                .bind(("start", start))
                .await?;

            let records: Vec<EntityRecord> = response.take(0)?;
            let count = records.len();

            if count == 0 {
                break;
            }

            for r in records {
                all_entities.push(EntityData {
                    name: r.name,
                    entity_type: Self::parse_entity_type(&r.entity_type),
                    first_mentioned: Self::parse_datetime(&r.first_mentioned),
                    last_mentioned: Self::parse_datetime(&r.last_mentioned),
                    mention_count: r.mention_count,
                    importance_score: r.importance_score,
                    description: r.description,
                });
            }

            if all_entities.len() % 1000 == 0 {
                info!(
                    "SurrealDBStorage: Loaded {} entities for session {}...",
                    all_entities.len(),
                    session_id
                );
            }

            if count < limit {
                break;
            }

            start += count;
        }

        Ok(all_entities)
    }

    async fn delete_entity(&self, session_id: Uuid, name: &str) -> Result<()> {
        let entity_id = Self::entity_id(session_id, name);
        let _: Option<EntityRecord> = self.delete("entity", &entity_id).await?;
        Ok(())
    }

    async fn create_relationship(
        &self,
        session_id: Uuid,
        relationship: &EntityRelationship,
    ) -> Result<()> {
        debug!(
            "SurrealDBStorage: Creating relationship {} -> {} ({:?})",
            relationship.from_entity, relationship.to_entity, relationship.relation_type
        );

        let from_id = Self::entity_id(session_id, &relationship.from_entity);
        let to_id = Self::entity_id(session_id, &relationship.to_entity);
        let table = Self::relation_table_name(&relationship.relation_type);
        let rel_id = Self::relation_id(
            session_id,
            &relationship.from_entity,
            &relationship.to_entity,
            &relationship.relation_type,
        );

        // Use UPSERT for the relationship table with deterministic ID
        // Note: In SurrealDB, for graph edges, you can also UPSERT the edge table directly
        let query = format!(
            "UPSERT {}:`{}` SET in = entity:`{}`, out = entity:`{}`, context = $context, session_id = $session_id",
            table, rel_id, from_id, to_id
        );

        self.db
            .query(query)
            .bind(("context", relationship.context.clone()))
            .bind(("session_id", session_id.to_string()))
            .await?;

        Ok(())
    }

    async fn find_related_entities(
        &self,
        session_id: Uuid,
        entity_name: &str,
    ) -> Result<Vec<String>> {
        let entity_id = Self::entity_id(session_id, entity_name);

        // Query all outgoing and incoming relations (backticks for IDs with special chars)
        let query = format!(
            r#"
            SELECT array::distinct(array::flatten([
                (SELECT VALUE out.name FROM entity:`{}`->*->entity WHERE session_id = $session_id),
                (SELECT VALUE in.name FROM entity:`{}`<-*<-entity WHERE session_id = $session_id)
            ])) AS related
            "#,
            entity_id, entity_id
        );

        let mut response = self
            .db
            .query(query)
            .bind(("session_id", session_id.to_string()))
            .await?;

        #[derive(Deserialize, SurrealValue)]
        struct RelatedResult {
            related: Vec<String>,
        }

        let results: Option<RelatedResult> = response.take(0)?;

        Ok(results.map(|r| r.related).unwrap_or_default())
    }

    async fn find_related_by_type(
        &self,
        session_id: Uuid,
        entity_name: &str,
        relation_type: &RelationType,
    ) -> Result<Vec<String>> {
        let entity_id = Self::entity_id(session_id, entity_name);
        let table = Self::relation_table_name(relation_type);

        let query = format!(
            r#"
            SELECT array::distinct(array::flatten([
                (SELECT VALUE out.name FROM entity:`{}`->{table}->entity WHERE session_id = $session_id),
                (SELECT VALUE in.name FROM entity:`{}`<-{table}<-entity WHERE session_id = $session_id)
            ])) AS related
            "#,
            entity_id, entity_id
        );

        let mut response = self
            .db
            .query(query)
            .bind(("session_id", session_id.to_string()))
            .await?;

        #[derive(Deserialize, SurrealValue)]
        struct RelatedResult {
            related: Vec<String>,
        }

        let results: Option<RelatedResult> = response.take(0)?;

        Ok(results.map(|r| r.related).unwrap_or_default())
    }

    async fn find_shortest_path(
        &self,
        session_id: Uuid,
        from: &str,
        to: &str,
    ) -> Result<Option<Vec<String>>> {
        let from_id = Self::entity_id(session_id, from);
        let to_id = Self::entity_id(session_id, to);

        // Use SurrealDB's graph traversal (backticks for IDs with special chars)
        let query = format!(
            r#"
            SELECT VALUE array::flatten([
                entity:`{}`.name,
                (SELECT VALUE name FROM entity:`{}`->*..5->entity:`{}` WHERE session_id = $session_id),
                entity:`{}`.name
            ])
            "#,
            from_id, from_id, to_id, to_id
        );

        let mut response = self
            .db
            .query(query)
            .bind(("session_id", session_id.to_string()))
            .await?;

        let path: Option<Vec<String>> = response.take(0)?;

        Ok(path.filter(|p| !p.is_empty()))
    }

    async fn get_entity_network(
        &self,
        session_id: Uuid,
        center: &str,
        max_depth: usize,
    ) -> Result<EntityNetwork> {
        let entity_id = Self::entity_id(session_id, center);

        // Get entities within max_depth hops (backticks for IDs with special chars)
        let entity_query = format!(
            r#"
            SELECT name, entity_type, first_mentioned, last_mentioned,
                   mention_count, importance_score, description
            FROM entity:`{}`<->*..{}->entity WHERE session_id = $session_id
            "#,
            entity_id, max_depth
        );

        let mut response = self
            .db
            .query(entity_query)
            .bind(("session_id", session_id.to_string()))
            .await?;

        let entity_records: Vec<EntityRecord> = response.take(0)?;

        let mut entities: BTreeMap<String, EntityData> = entity_records
            .into_iter()
            .map(|r| {
                (
                    r.name.clone(),
                    EntityData {
                        name: r.name,
                        entity_type: Self::parse_entity_type(&r.entity_type),
                        first_mentioned: Self::parse_datetime(&r.first_mentioned),
                        last_mentioned: Self::parse_datetime(&r.last_mentioned),
                        mention_count: r.mention_count,
                        importance_score: r.importance_score,
                        description: r.description,
                    },
                )
            })
            .collect();

        // Add center entity if not already included
        if let Some(center_entity) = self.get_entity(session_id, center).await? {
            entities.insert(center.to_string(), center_entity);
        }

        // Get relationships (simplified - just get direct connections from center)
        let mut relationships = Vec::new();

        // Query each relation type
        for (table, rel_type) in [
            ("required_by", RelationType::RequiredBy),
            ("leads_to", RelationType::LeadsTo),
            ("related_to", RelationType::RelatedTo),
            ("conflicts_with", RelationType::ConflictsWith),
            ("depends_on", RelationType::DependsOn),
            ("implements", RelationType::Implements),
            ("caused_by", RelationType::CausedBy),
            ("solves", RelationType::Solves),
        ] {
            let rel_query = format!(
                r#"
                SELECT in.name AS from_entity, out.name AS to_entity, context
                FROM {}
                WHERE session_id = $session_id
                "#,
                table
            );

            let mut rel_response = self
                .db
                .query(rel_query)
                .bind(("session_id", session_id.to_string()))
                .await?;

            #[derive(Deserialize, SurrealValue)]
            struct RelRecord {
                from_entity: String,
                to_entity: String,
                context: String,
            }

            let rel_records: Vec<RelRecord> = rel_response.take(0).unwrap_or_default();

            for record in rel_records {
                if entities.contains_key(&record.from_entity)
                    && entities.contains_key(&record.to_entity)
                {
                    relationships.push(EntityRelationship {
                        from_entity: record.from_entity,
                        to_entity: record.to_entity,
                        relation_type: rel_type.clone(),
                        context: record.context,
                    });
                }
            }
        }

        Ok(EntityNetwork {
            center: center.to_string(),
            entities,
            relationships,
        })
    }
}

// ============================================================================
// Helper Methods for SurrealDB
// ============================================================================

impl SurrealDBStorage {
    /// Load all relationships for a session from native graph
    async fn load_all_relationships(&self, session_id: Uuid) -> Result<Vec<EntityRelationship>> {
        info!(
            "SurrealDBStorage: Fetching relationships for session {}...",
            session_id
        );
        let mut all_relationships = Vec::new();

        // Query each relation type table
        let relation_tables = [
            ("required_by", RelationType::RequiredBy),
            ("leads_to", RelationType::LeadsTo),
            ("related_to", RelationType::RelatedTo),
            ("conflicts_with", RelationType::ConflictsWith),
            ("depends_on", RelationType::DependsOn),
            ("implements", RelationType::Implements),
            ("caused_by", RelationType::CausedBy),
            ("solves", RelationType::Solves),
        ];

        for (table, rel_type) in relation_tables {
            debug!(
                "SurrealDBStorage: Processing relationship table '{}'...",
                table
            );
            let mut start = 0;
            let limit = 1000;

            loop {
                let query = format!(
                    r#"
                    SELECT
                        in.name AS from_entity,
                        out.name AS to_entity,
                        context
                    FROM {}
                    WHERE session_id = $session_id
                    LIMIT $limit START $start
                    "#,
                    table
                );

                let mut response = self
                    .db
                    .query(query)
                    .bind(("session_id", session_id.to_string()))
                    .bind(("limit", limit))
                    .bind(("start", start))
                    .await?;

                #[derive(Deserialize, SurrealValue)]
                struct RelRecord {
                    from_entity: String,
                    to_entity: String,
                    context: String,
                }

                let records: Vec<RelRecord> = response.take(0).unwrap_or_default();
                let count = records.len();

                if count == 0 {
                    break;
                }

                if count > 0 {
                    info!(
                        "SurrealDBStorage: Loaded {} relationships from '{}' (start: {})...",
                        count, table, start
                    );
                }

                for record in records {
                    all_relationships.push(EntityRelationship {
                        from_entity: record.from_entity,
                        to_entity: record.to_entity,
                        relation_type: rel_type.clone(),
                        context: record.context,
                    });
                }

                if all_relationships.len() > 0 && all_relationships.len() % 1000 == 0 {
                    debug!(
                        "SurrealDBStorage: Loaded {} relationships...",
                        all_relationships.len()
                    );
                }

                if count < limit {
                    break;
                }

                start += count;
            }
        }

        debug!(
            "SurrealDBStorage: Loaded {} relationships for session {}",
            all_relationships.len(),
            session_id
        );

        Ok(all_relationships)
    }

    /// Rebuild StructuredContext from context updates
    fn rebuild_structured_context(updates: &[ContextUpdate]) -> StructuredContext {
        use crate::core::context_update::UpdateType;
        use crate::core::structured_context::{
            ConceptItem, DecisionItem, FlowItem, QuestionItem, QuestionStatus,
        };

        let mut context = StructuredContext::default();

        for update in updates {
            // Extract key decisions
            if update.update_type == UpdateType::DecisionMade {
                context.key_decisions.push(DecisionItem {
                    description: update.content.title.clone(),
                    context: update.content.description.clone(),
                    alternatives: update.content.details.clone(),
                    confidence: if update.user_marked_important {
                        0.9
                    } else {
                        0.7
                    },
                    timestamp: update.timestamp,
                });
            }

            // Extract questions/answers
            if update.update_type == UpdateType::QuestionAnswered {
                context.open_questions.push(QuestionItem {
                    question: update.content.title.clone(),
                    context: update.content.description.clone(),
                    status: QuestionStatus::Answered,
                    timestamp: update.timestamp,
                    last_updated: update.timestamp,
                });
            }

            // Add to conversation flow (limit to recent 500)
            if context.conversation_flow.len() < 500 {
                context.add_flow_item(FlowItem {
                    step_description: format!(
                        "{}: {}",
                        format!("{:?}", update.update_type),
                        update.content.title
                    ),
                    timestamp: update.timestamp,
                    related_updates: vec![update.id],
                    outcome: if update.content.description.is_empty() {
                        None
                    } else {
                        Some(update.content.description.clone())
                    },
                });
            }

            // Extract key concepts from entities
            for entity in &update.creates_entities {
                let already_exists = context.key_concepts.iter().any(|c| c.name == *entity);
                if !already_exists && context.key_concepts.len() < 50 {
                    context.key_concepts.push(ConceptItem {
                        name: entity.clone(),
                        definition: String::new(),
                        examples: Vec::new(),
                        related_concepts: update.references_entities.clone(),
                        timestamp: update.timestamp,
                    });
                }
            }
        }

        context
    }

    /// Extract code_references from context updates
    /// Returns HashMap<file_path, Vec<CodeReference>> - multiple references per file
    fn extract_code_references(updates: &[ContextUpdate]) -> HashMap<String, Vec<CodeReference>> {
        let mut refs: HashMap<String, Vec<CodeReference>> = HashMap::new();

        for update in updates {
            if let Some(code_ref) = &update.related_code {
                // Convert from core::context_update::CodeReference to session::active_session::CodeReference
                let session_ref = CodeReference {
                    file_path: code_ref.file_path.clone(),
                    start_line: code_ref.start_line,
                    end_line: code_ref.end_line,
                    code_snippet: code_ref.code_snippet.clone(),
                    commit_hash: code_ref.commit_hash.clone(),
                    branch: code_ref.branch.clone(),
                    change_description: code_ref.change_description.clone(),
                };
                // Collect all references for each file path
                refs.entry(code_ref.file_path.clone())
                    .or_default()
                    .push(session_ref);
            }
        }

        refs
    }

    /// Extract change_history from context updates
    /// Creates ChangeRecord for each CodeChanged update
    fn extract_change_history(updates: &[ContextUpdate]) -> Vec<ChangeRecord> {
        use crate::core::context_update::UpdateType;

        updates
            .iter()
            .filter(|u| u.update_type == UpdateType::CodeChanged)
            .map(|u| ChangeRecord {
                id: u.id,
                timestamp: u.timestamp,
                change_type: "CodeChanged".to_string(),
                description: format!("{}: {}", u.content.title, u.content.description),
                related_update_id: u.parent_update,
            })
            .collect()
    }
}

// ============================================================================
// VectorStorage Trait Implementation
// ============================================================================

#[async_trait]
impl VectorStorage for SurrealDBStorage {
    async fn add_vector(&self, vector: Vec<f32>, metadata: VectorMetadata) -> Result<String> {
        if vector.len() != EMBEDDING_DIMENSION {
            return Err(anyhow::anyhow!(
                "Vector dimension mismatch: expected {}, got {}",
                EMBEDDING_DIMENSION,
                vector.len()
            ));
        }

        debug!(
            "SurrealDBStorage: Adding vector for content {}",
            metadata.id
        );

        let record = EmbeddingRecord {
            content_id: metadata.id.clone(),
            session_id: metadata.source.clone(),
            vector,
            text: metadata.text,
            content_type: metadata.content_type,
            timestamp: metadata.timestamp.to_rfc3339(),
            metadata: metadata.metadata,
        };

        let _: Option<EmbeddingRecord> = self
            .db
            .upsert(("embedding", metadata.id.clone()))
            .content(record)
            .await?;

        Ok(metadata.id)
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

    async fn search(&self, query: &[f32], k: usize) -> Result<Vec<SearchMatch>> {
        if query.len() != EMBEDDING_DIMENSION {
            return Err(anyhow::anyhow!(
                "Query vector dimension mismatch: expected {}, got {}",
                EMBEDDING_DIMENSION,
                query.len()
            ));
        }

        debug!("SurrealDBStorage: HNSW search for {} nearest vectors", k);

        // Use native HNSW KNN search - O(log n) instead of O(n)!
        // The <|K|> operator uses the HNSW index for fast approximate nearest neighbor search
        let query_vec: Vec<f32> = query.to_vec();
        let mut response = self
            .db
            .query("SELECT *, vector::distance::knn() AS distance FROM embedding WHERE vector <|$k|> $query_vec")
            .bind(("k", k as i64))
            .bind(("query_vec", query_vec))
            .await?;

        let records: Vec<KnnResult> = response.take(0)?;

        let matches: Vec<SearchMatch> = records
            .into_iter()
            .map(|r| {
                // Convert distance to similarity (cosine distance -> similarity)
                let similarity = 1.0 - r.distance.min(1.0);
                SearchMatch {
                    vector_id: 0,
                    similarity,
                    metadata: VectorMetadata {
                        id: r.content_id,
                        text: r.text,
                        source: r.session_id,
                        content_type: r.content_type,
                        timestamp: Self::parse_datetime(&r.timestamp),
                        metadata: r.metadata,
                    },
                }
            })
            .collect();

        debug!("SurrealDBStorage: HNSW found {} matches", matches.len());

        Ok(matches)
    }

    async fn search_in_session(
        &self,
        query: &[f32],
        k: usize,
        session_id: &str,
    ) -> Result<Vec<SearchMatch>> {
        if query.len() != EMBEDDING_DIMENSION {
            return Err(anyhow::anyhow!(
                "Query vector dimension mismatch: expected {}, got {}",
                EMBEDDING_DIMENSION,
                query.len()
            ));
        }

        debug!(
            "SurrealDBStorage: HNSW search for {} nearest vectors in session {}",
            k, session_id
        );

        // Use HNSW with post-filtering: fetch more results, filter by session
        // This is more efficient than full table scan for large datasets
        let fetch_count = (k * 5).max(50); // Fetch 5x more to ensure enough after filtering
        let query_vec: Vec<f32> = query.to_vec();

        let mut response = self
            .db
            .query("SELECT *, vector::distance::knn() AS distance FROM embedding WHERE vector <|$fetch_count|> $query_vec")
            .bind(("fetch_count", fetch_count as i64))
            .bind(("query_vec", query_vec))
            .await?;

        let records: Vec<KnnResult> = response.take(0)?;

        // Post-filter by session and convert to SearchMatch
        let matches: Vec<SearchMatch> = records
            .into_iter()
            .filter(|r| r.session_id == session_id)
            .take(k)
            .map(|r| {
                let similarity = 1.0 - r.distance.min(1.0);
                SearchMatch {
                    vector_id: 0,
                    similarity,
                    metadata: VectorMetadata {
                        id: r.content_id,
                        text: r.text,
                        source: r.session_id,
                        content_type: r.content_type,
                        timestamp: Self::parse_datetime(&r.timestamp),
                        metadata: r.metadata,
                    },
                }
            })
            .collect();

        debug!(
            "SurrealDBStorage: HNSW found {} matches in session",
            matches.len()
        );

        Ok(matches)
    }

    async fn search_by_content_type(
        &self,
        query: &[f32],
        k: usize,
        content_type: &str,
    ) -> Result<Vec<SearchMatch>> {
        if query.len() != EMBEDDING_DIMENSION {
            return Err(anyhow::anyhow!(
                "Query vector dimension mismatch: expected {}, got {}",
                EMBEDDING_DIMENSION,
                query.len()
            ));
        }

        debug!(
            "SurrealDBStorage: HNSW search for {} nearest vectors of type {}",
            k, content_type
        );

        // Use HNSW with post-filtering by content type
        let fetch_count = (k * 5).max(50);
        let query_vec: Vec<f32> = query.to_vec();

        let mut response = self
            .db
            .query("SELECT *, vector::distance::knn() AS distance FROM embedding WHERE vector <|$fetch_count|> $query_vec")
            .bind(("fetch_count", fetch_count as i64))
            .bind(("query_vec", query_vec))
            .await?;

        let records: Vec<KnnResult> = response.take(0)?;

        // Post-filter by content_type
        let matches: Vec<SearchMatch> = records
            .into_iter()
            .filter(|r| r.content_type == content_type)
            .take(k)
            .map(|r| {
                let similarity = 1.0 - r.distance.min(1.0);
                SearchMatch {
                    vector_id: 0,
                    similarity,
                    metadata: VectorMetadata {
                        id: r.content_id,
                        text: r.text,
                        source: r.session_id,
                        content_type: r.content_type,
                        timestamp: Self::parse_datetime(&r.timestamp),
                        metadata: r.metadata,
                    },
                }
            })
            .collect();

        debug!(
            "SurrealDBStorage: HNSW found {} matches of type {}",
            matches.len(),
            content_type
        );

        Ok(matches)
    }

    async fn remove_vector(&self, id: &str) -> Result<bool> {
        let result: Option<EmbeddingRecord> = self.delete("embedding", id).await?;
        Ok(result.is_some())
    }

    async fn has_session_embeddings(&self, session_id: &str) -> bool {
        let count = self.count_session_embeddings(session_id).await;
        count > 0
    }

    async fn count_session_embeddings(&self, session_id: &str) -> usize {
        let result = self
            .db
            .query("SELECT count() FROM embedding WHERE session_id = $session_id GROUP ALL")
            .bind(("session_id", session_id.to_string()))
            .await;

        if let Ok(mut response) = result {
            #[derive(Deserialize, SurrealValue)]
            struct CountResult {
                count: i64,
            }
            if let Ok(Some(count)) = response.take::<Option<CountResult>>(0) {
                return count.count as usize;
            }
        }

        0
    }

    async fn total_count(&self) -> usize {
        let result = self
            .db
            .query("SELECT count() FROM embedding GROUP ALL")
            .await;

        if let Ok(mut response) = result {
            #[derive(Deserialize, SurrealValue)]
            struct CountResult {
                count: i64,
            }
            if let Ok(Some(count)) = response.take::<Option<CountResult>>(0) {
                return count.count as usize;
            }
        }

        0
    }

    async fn get_session_vectors(
        &self,
        session_id: &str,
    ) -> Result<Vec<(Vec<f32>, VectorMetadata)>> {
        let mut response = self
            .db
            .query("SELECT * FROM embedding WHERE session_id = $session_id")
            .bind(("session_id", session_id.to_string()))
            .await?;

        let records: Vec<EmbeddingRecord> = response.take(0)?;

        Ok(records
            .into_iter()
            .map(|r| {
                (
                    r.vector,
                    VectorMetadata {
                        id: r.content_id,
                        text: r.text,
                        source: r.session_id,
                        content_type: r.content_type,
                        timestamp: Self::parse_datetime(&r.timestamp),
                        metadata: r.metadata,
                    },
                )
            })
            .collect())
    }

    async fn get_all_vectors(&self) -> Result<Vec<(Vec<f32>, VectorMetadata)>> {
        let mut all_vectors = Vec::new();
        let limit = 1000;
        let mut start = 0;

        loop {
            let mut response = self
                .db
                .query("SELECT * FROM embedding LIMIT $limit START $start")
                .bind(("limit", limit))
                .bind(("start", start))
                .await?;

            let records: Vec<EmbeddingRecord> = response.take(0)?;

            if records.is_empty() {
                break;
            }

            for r in records {
                all_vectors.push((
                    r.vector,
                    VectorMetadata {
                        id: r.content_id,
                        text: r.text,
                        source: r.session_id,
                        content_type: r.content_type,
                        timestamp: Self::parse_datetime(&r.timestamp),
                        metadata: r.metadata,
                    },
                ));
            }

            start += limit;
        }

        Ok(all_vectors)
    }
}

// ============================================================================
// Import Functions
// ============================================================================

impl SurrealDBStorage {
    /// Import data from an ExportData structure
    pub async fn import_data(
        &self,
        data: crate::storage::export_import::ExportData,
        options: &crate::storage::export_import::ImportOptions,
    ) -> Result<crate::storage::export_import::ImportResult> {
        use crate::storage::export_import::{ImportResult, SUPPORTED_IMPORT_VERSIONS};

        info!(
            "SurrealDBStorage: Starting import: {} sessions, {} workspaces",
            data.sessions.len(),
            data.workspaces.len()
        );

        // Check format version compatibility
        if !SUPPORTED_IMPORT_VERSIONS.contains(&data.format_version.as_str()) {
            return Err(anyhow::anyhow!(
                "Incompatible export format version: {}. Supported: {:?}",
                data.format_version,
                SUPPORTED_IMPORT_VERSIONS
            ));
        }

        let mut result = ImportResult::default();

        // Import sessions
        for exported_session in data.sessions {
            let session_id = exported_session.session.id();

            // Apply filter if specified
            if let Some(ref filter) = options.session_filter {
                if !filter.contains(&session_id) {
                    continue;
                }
            }

            // Check if session exists
            let exists = self.session_exists(session_id).await?;

            if exists {
                if options.skip_existing {
                    result.sessions_skipped += 1;
                    continue;
                } else if !options.overwrite {
                    result.errors.push(format!(
                        "Session {} already exists (use --overwrite or --skip-existing)",
                        session_id
                    ));
                    continue;
                }
            }

            // Import session
            match self.save_session(&exported_session.session).await {
                Ok(()) => {
                    result.sessions_imported += 1;

                    // Import updates
                    if !exported_session.updates.is_empty() {
                        match self
                            .batch_save_updates(session_id, exported_session.updates.clone())
                            .await
                        {
                            Ok(()) => {
                                result.updates_imported += exported_session.updates.len();
                            }
                            Err(e) => {
                                result.errors.push(format!(
                                    "Failed to import updates for session {}: {}",
                                    session_id, e
                                ));
                            }
                        }
                    }
                }
                Err(e) => {
                    result
                        .errors
                        .push(format!("Failed to import session {}: {}", session_id, e));
                }
            }
        }

        // Import workspaces
        for workspace in data.workspaces {
            // Apply filter if specified
            if let Some(ref filter) = options.workspace_filter {
                if !filter.contains(&workspace.id) {
                    continue;
                }
            }

            // Check if workspace exists
            let existing = self.list_workspaces().await?;
            let exists = existing.iter().any(|w| w.id == workspace.id);

            if exists {
                if options.skip_existing {
                    result.workspaces_skipped += 1;
                    continue;
                } else if !options.overwrite {
                    result.errors.push(format!(
                        "Workspace {} already exists (use --overwrite or --skip-existing)",
                        workspace.id
                    ));
                    continue;
                } else {
                    // Delete existing workspace before reimporting
                    self.delete_workspace(workspace.id).await?;
                }
            }

            // Import workspace
            let session_ids: Vec<Uuid> = workspace.sessions.iter().map(|(id, _)| *id).collect();
            match self
                .save_workspace_metadata(
                    workspace.id,
                    &workspace.name,
                    &workspace.description,
                    &session_ids,
                )
                .await
            {
                Ok(()) => {
                    // Add session associations with roles
                    for (session_id, role) in &workspace.sessions {
                        if let Err(e) = self
                            .add_session_to_workspace(workspace.id, *session_id, role.clone())
                            .await
                        {
                            result.errors.push(format!(
                                "Failed to add session {} to workspace {}: {}",
                                session_id, workspace.id, e
                            ));
                        }
                    }
                    result.workspaces_imported += 1;
                }
                Err(e) => {
                    result.errors.push(format!(
                        "Failed to import workspace {}: {}",
                        workspace.id, e
                    ));
                }
            }
        }

        // Import checkpoints
        for checkpoint in data.checkpoints {
            match self.save_checkpoint(&checkpoint).await {
                Ok(()) => result.checkpoints_imported += 1,
                Err(e) => {
                    result.errors.push(format!(
                        "Failed to import checkpoint {}: {}",
                        checkpoint.id, e
                    ));
                }
            }
        }

        // Import embeddings (v1.2.0+)
        if !data.embeddings.is_empty() {
            match self.import_embeddings(&data.embeddings).await {
                Ok(count) => result.embeddings_imported = count,
                Err(e) => {
                    result
                        .errors
                        .push(format!("Failed to import embeddings: {}", e));
                }
            }
        }

        info!(
            "SurrealDBStorage: Import complete: {} sessions, {} workspaces, {} updates, {} embeddings, {} errors",
            result.sessions_imported,
            result.workspaces_imported,
            result.updates_imported,
            result.embeddings_imported,
            result.errors.len()
        );

        Ok(result)
    }

    /// Export all data from the database
    pub async fn export_full(
        &self,
        options: &crate::storage::export_import::ExportOptions,
    ) -> Result<crate::storage::export_import::ExportData> {
        use crate::storage::export_import::{ExportData, ExportType, ExportedWorkspace};

        info!("SurrealDBStorage: Starting full database export");

        let mut export = ExportData::new(ExportType::Full, options.compression);

        // Export all sessions
        let session_ids = self.list_sessions().await?;
        info!("SurrealDBStorage: Exporting {} sessions", session_ids.len());

        for session_id in session_ids {
            info!("SurrealDBStorage: Processing session {}...", session_id);
            match self.export_session_data(session_id).await {
                Ok(session_data) => export.sessions.push(session_data),
                Err(e) => {
                    info!(
                        "SurrealDBStorage: Warning: Failed to export session {}: {}",
                        session_id, e
                    );
                }
            }
        }

        // Export all workspaces
        let workspaces = self.list_workspaces().await?;
        info!(
            "SurrealDBStorage: Exporting {} workspaces",
            workspaces.len()
        );
        export.workspaces = workspaces
            .into_iter()
            .map(ExportedWorkspace::from)
            .collect();

        // Export checkpoints if requested
        if options.include_checkpoints {
            export.checkpoints = self.list_checkpoints().await?;
            info!(
                "SurrealDBStorage: Exported {} checkpoints",
                export.checkpoints.len()
            );
        }

        // Export all embeddings
        export.embeddings = self.export_all_embeddings().await?;
        info!(
            "SurrealDBStorage: Exported {} embeddings",
            export.embeddings.len()
        );

        export.update_counts();
        info!(
            "SurrealDBStorage: Export complete: {} sessions, {} workspaces, {} updates, {} embeddings",
            export.metadata.session_count,
            export.metadata.workspace_count,
            export.metadata.update_count,
            export.metadata.embedding_count
        );

        Ok(export)
    }

    /// Export specific sessions
    pub async fn export_sessions(
        &self,
        session_ids: Vec<Uuid>,
        options: &crate::storage::export_import::ExportOptions,
    ) -> Result<crate::storage::export_import::ExportData> {
        use crate::storage::export_import::{ExportData, ExportType};

        info!(
            "SurrealDBStorage: Starting selective export of {} sessions",
            session_ids.len()
        );

        let mut export = ExportData::new(
            ExportType::SelectiveSessions {
                session_ids: session_ids.clone(),
            },
            options.compression,
        );

        for session_id in &session_ids {
            match self.export_session_data(*session_id).await {
                Ok(session_data) => export.sessions.push(session_data),
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Failed to export session {}: {}",
                        session_id,
                        e
                    ));
                }
            }
        }

        // Export embeddings for these sessions
        export.embeddings = self.export_session_embeddings(&session_ids).await?;

        export.update_counts();
        Ok(export)
    }

    /// Export a workspace and all its sessions
    pub async fn export_workspace(
        &self,
        workspace_id: Uuid,
        options: &crate::storage::export_import::ExportOptions,
    ) -> Result<crate::storage::export_import::ExportData> {
        use crate::storage::export_import::{ExportData, ExportType, ExportedWorkspace};

        info!(
            "SurrealDBStorage: Starting workspace export for {}",
            workspace_id
        );

        let mut export = ExportData::new(
            ExportType::SelectiveWorkspace { workspace_id },
            options.compression,
        );

        // Find the workspace
        let workspaces = self.list_workspaces().await?;
        let workspace = workspaces
            .into_iter()
            .find(|w| w.id == workspace_id)
            .ok_or_else(|| anyhow::anyhow!("Workspace {} not found", workspace_id))?;

        // Export the workspace
        export
            .workspaces
            .push(ExportedWorkspace::from(workspace.clone()));

        // Export all sessions in the workspace
        let session_ids: Vec<Uuid> = workspace.sessions.iter().map(|(id, _)| *id).collect();
        for session_id in &session_ids {
            match self.export_session_data(*session_id).await {
                Ok(session_data) => export.sessions.push(session_data),
                Err(e) => {
                    info!(
                        "SurrealDBStorage: Warning: Failed to export session {} from workspace: {}",
                        session_id, e
                    );
                }
            }
        }

        // Export embeddings for workspace sessions
        export.embeddings = self.export_session_embeddings(&session_ids).await?;

        export.update_counts();
        Ok(export)
    }

    /// Export a single session with its updates
    async fn export_session_data(
        &self,
        session_id: Uuid,
    ) -> Result<crate::storage::export_import::ExportedSession> {
        use crate::storage::export_import::ExportedSession;

        let session = self.load_session(session_id).await?;
        let updates = self.load_session_updates(session_id).await?;

        Ok(ExportedSession { session, updates })
    }

    /// Export all embeddings from the database
    async fn export_all_embeddings(
        &self,
    ) -> Result<Vec<crate::storage::export_import::ExportedEmbedding>> {
        use crate::storage::export_import::ExportedEmbedding;

        let mut all_embeddings = Vec::new();
        let mut start = 0;
        let limit = 500; // Embeddings are large, use smaller batch size

        loop {
            let mut response = self
                .db
                .query("SELECT * FROM embedding LIMIT $limit START $start")
                .bind(("limit", limit))
                .bind(("start", start))
                .await?;

            let records: Vec<EmbeddingRecord> = response.take(0)?;
            let count = records.len();

            if count == 0 {
                break;
            }

            for r in records {
                all_embeddings.push(ExportedEmbedding {
                    content_id: r.content_id,
                    session_id: r.session_id,
                    vector: r.vector,
                    text: r.text,
                    content_type: r.content_type,
                    timestamp: r.timestamp,
                    metadata: r.metadata,
                });
            }

            if all_embeddings.len() % 500 == 0 {
                info!(
                    "SurrealDBStorage: Exported {} embeddings...",
                    all_embeddings.len()
                );
            }

            if count < limit {
                break;
            }

            start += count;
        }

        Ok(all_embeddings)
    }

    /// Export embeddings for specific sessions
    async fn export_session_embeddings(
        &self,
        session_ids: &[Uuid],
    ) -> Result<Vec<crate::storage::export_import::ExportedEmbedding>> {
        use crate::storage::export_import::ExportedEmbedding;

        let session_id_strs: Vec<String> = session_ids.iter().map(|id| id.to_string()).collect();
        let mut all_embeddings = Vec::new();
        let mut start = 0;
        let limit = 500;

        loop {
            let mut response = self
                .db
                .query("SELECT * FROM embedding WHERE session_id IN $session_ids LIMIT $limit START $start")
                .bind(("session_ids", session_id_strs.clone()))
                .bind(("limit", limit))
                .bind(("start", start))
                .await?;

            let records: Vec<EmbeddingRecord> = response.take(0)?;
            let count = records.len();

            if count == 0 {
                break;
            }

            for r in records {
                all_embeddings.push(ExportedEmbedding {
                    content_id: r.content_id,
                    session_id: r.session_id,
                    vector: r.vector,
                    text: r.text,
                    content_type: r.content_type,
                    timestamp: r.timestamp,
                    metadata: r.metadata,
                });
            }

            if all_embeddings.len() % 500 == 0 {
                info!(
                    "SurrealDBStorage: Exported {} session embeddings...",
                    all_embeddings.len()
                );
            }

            if count < limit {
                break;
            }

            start += count;
        }

        Ok(all_embeddings)
    }

    /// Import embeddings from export data
    async fn import_embeddings(
        &self,
        embeddings: &[crate::storage::export_import::ExportedEmbedding],
    ) -> Result<usize> {
        let mut count = 0;
        for emb in embeddings {
            let record = EmbeddingRecord {
                content_id: emb.content_id.clone(),
                session_id: emb.session_id.clone(),
                vector: emb.vector.clone(),
                text: emb.text.clone(),
                content_type: emb.content_type.clone(),
                timestamp: emb.timestamp.clone(),
                metadata: emb.metadata.clone(),
            };

            let _: Option<EmbeddingRecord> = self
                .db
                .create(("embedding", emb.content_id.as_str()))
                .content(record)
                .await?;
            count += 1;
        }
        Ok(count)
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Compute cosine similarity between two vectors (used in tests)
#[cfg(test)]
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot_product / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_surrealdb_session_operations() {
        let storage = SurrealDBStorage::new("mem://", None, None, None, None)
            .await
            .expect("Failed to create SurrealDB storage");

        // Create test session
        let session_id = Uuid::new_v4();
        let session = ActiveSession::new(
            session_id,
            Some("Test Session".to_string()),
            Some("A test session".to_string()),
        );

        // Save session
        storage
            .save_session(&session)
            .await
            .expect("Failed to save session");

        // Load session
        let loaded = storage
            .load_session(session_id)
            .await
            .expect("Failed to load session");
        assert_eq!(session.id(), loaded.id());

        // Check exists
        assert!(storage.session_exists(session_id).await.unwrap());

        // Delete
        storage
            .delete_session(session_id)
            .await
            .expect("Failed to delete session");
        assert!(!storage.session_exists(session_id).await.unwrap());
    }

    #[tokio::test]
    async fn test_entity_operations() {
        let storage = SurrealDBStorage::new("mem://", None, None, None, None)
            .await
            .expect("Failed to create SurrealDB storage");

        let session_id = Uuid::new_v4();
        let entity = EntityData {
            name: "rust".to_string(),
            entity_type: EntityType::Technology,
            first_mentioned: Utc::now(),
            last_mentioned: Utc::now(),
            mention_count: 1,
            importance_score: 1.0,
            description: Some("Rust programming language".to_string()),
        };

        // Upsert entity
        storage
            .upsert_entity(session_id, &entity)
            .await
            .expect("Failed to upsert entity");

        // Get entity
        let loaded = storage
            .get_entity(session_id, "rust")
            .await
            .expect("Failed to get entity");
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().name, "rust");

        // List entities
        let entities = storage
            .list_entities(session_id)
            .await
            .expect("Failed to list entities");
        assert_eq!(entities.len(), 1);

        // Delete entity
        storage
            .delete_entity(session_id, "rust")
            .await
            .expect("Failed to delete entity");
        let deleted = storage
            .get_entity(session_id, "rust")
            .await
            .expect("Failed to check deleted");
        assert!(deleted.is_none());
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.0001);

        let c = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &c) - 0.0).abs() < 0.0001);

        let d = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &d) - (-1.0)).abs() < 0.0001);
    }

    /// Test remote connection to Docker SurrealDB (requires running Docker container)
    /// Run with: cargo test --features surrealdb-storage test_remote_surrealdb -- --ignored
    #[tokio::test]
    #[ignore = "Requires running Docker SurrealDB at localhost:8000"]
    async fn test_remote_surrealdb_connection() {
        // Connect to Docker SurrealDB (default: root/root)
        let storage =
            SurrealDBStorage::new("localhost:8000", Some("root"), Some("root"), None, None)
                .await
                .expect("Failed to connect to remote SurrealDB");

        // Create test session
        let session_id = Uuid::new_v4();
        let session = ActiveSession::new(
            session_id,
            Some("Remote Test Session".to_string()),
            Some("Testing remote connection".to_string()),
        );

        // Save session
        storage
            .save_session(&session)
            .await
            .expect("Failed to save session to remote");

        // Load session
        let loaded = storage
            .load_session(session_id)
            .await
            .expect("Failed to load session from remote");
        assert_eq!(session.id(), loaded.id());
        assert_eq!(
            loaded.name().clone(),
            Some("Remote Test Session".to_string())
        );

        // Clean up
        storage
            .delete_session(session_id)
            .await
            .expect("Failed to delete session from remote");

        println!("Remote SurrealDB connection test passed!");
    }

    // ========================================================================
    // Normalized Storage Tests
    // ========================================================================

    /// Helper to create test session with pre-populated data
    fn create_test_session_with_updates(
        session_id: Uuid,
        name: &str,
        updates: Vec<ContextUpdate>,
        entity_graph: SimpleEntityGraph,
    ) -> ActiveSession {
        ActiveSession::from_components(
            session_id,
            Some(name.to_string()),
            Some("Test session".to_string()),
            Utc::now(),
            Utc::now(),
            UserPreferences {
                auto_save_enabled: true,
                context_retention_days: 30,
                max_hot_context_size: 50,
                auto_summary_threshold: 100,
                important_keywords: Vec::new(),
            },
            updates.clone(), // hot_context_vec
            Vec::new(),      // warm_context
            Vec::new(),      // cold_context
            StructuredContext::default(),
            updates,        // incremental_updates
            HashMap::new(), // code_references
            Vec::new(),     // change_history
            entity_graph,
            100,        // max_extracted_entities
            50,         // max_referenced_entities
            true,       // enable_smart_entity_ranking
            0,          // total_entity_truncations
            0,          // total_entities_truncated
            Vec::new(), // vectorized_update_ids
        )
    }

    #[tokio::test]
    async fn test_normalized_context_updates_roundtrip() {
        use crate::core::context_update::{UpdateContent, UpdateType};

        let storage = SurrealDBStorage::new("mem://", None, None, None, None)
            .await
            .expect("Failed to create SurrealDB storage");

        let session_id = Uuid::new_v4();

        // Create context updates
        let update1 = ContextUpdate {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            update_type: UpdateType::QuestionAnswered,
            content: UpdateContent {
                title: "How does SurrealDB work?".to_string(),
                description: "SurrealDB is a multi-model database".to_string(),
                details: vec!["Graph support".to_string(), "Vector search".to_string()],
                examples: vec![],
                implications: vec![],
            },
            related_code: None,
            parent_update: None,
            user_marked_important: true,
            creates_entities: vec!["SurrealDB".to_string()],
            creates_relationships: vec![],
            references_entities: vec![],
        };

        let update2 = ContextUpdate {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            update_type: UpdateType::DecisionMade,
            content: UpdateContent {
                title: "Use normalized storage".to_string(),
                description: "Store context updates in separate table".to_string(),
                details: vec!["No JSON blobs".to_string()],
                examples: vec![],
                implications: vec!["Better queryability".to_string()],
            },
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: vec!["NormalizedStorage".to_string()],
            creates_relationships: vec![],
            references_entities: vec!["SurrealDB".to_string()],
        };

        // Create session with updates
        let session = create_test_session_with_updates(
            session_id,
            "Normalized Test",
            vec![update1, update2],
            SimpleEntityGraph::new(),
        );

        // Save session (should save updates to context_update table)
        storage
            .save_session(&session)
            .await
            .expect("Failed to save session");

        // Verify updates are in context_update table
        let loaded_updates = storage
            .load_session_updates(session_id)
            .await
            .expect("Failed to load updates");
        assert_eq!(
            loaded_updates.len(),
            2,
            "Should have 2 updates in normalized table"
        );

        // Load session (should rebuild from normalized tables)
        let loaded = storage
            .load_session(session_id)
            .await
            .expect("Failed to load session");

        // Verify hot_context was rebuilt
        let hot_updates: Vec<_> = loaded.hot_context.iter();
        assert_eq!(hot_updates.len(), 2, "Hot context should have 2 updates");

        // Verify update content is preserved
        let found_qa = hot_updates.iter().any(|u| {
            u.update_type == UpdateType::QuestionAnswered
                && u.content.title == "How does SurrealDB work?"
        });
        assert!(found_qa, "Should find QuestionAnswered update");

        let found_decision = hot_updates.iter().any(|u| {
            u.update_type == UpdateType::DecisionMade && u.content.title == "Use normalized storage"
        });
        assert!(found_decision, "Should find DecisionMade update");
    }

    #[tokio::test]
    async fn test_normalized_entities_and_relationships() {
        let storage = SurrealDBStorage::new("mem://", None, None, None, None)
            .await
            .expect("Failed to create SurrealDB storage");

        let session_id = Uuid::new_v4();

        // Create entity graph with entities and relationships
        let mut entity_graph = SimpleEntityGraph::new();
        entity_graph.add_or_update_entity(
            "Rust".to_string(),
            EntityType::Technology,
            Utc::now(),
            "Programming language",
        );
        entity_graph.add_or_update_entity(
            "SurrealDB".to_string(),
            EntityType::Technology,
            Utc::now(),
            "Multi-model database",
        );

        // Add relationship
        let relationship = EntityRelationship {
            from_entity: "Rust".to_string(),
            to_entity: "SurrealDB".to_string(),
            relation_type: RelationType::Implements,
            context: "Rust client for SurrealDB".to_string(),
        };
        entity_graph.add_relationship(relationship);

        // Create session with entity graph
        let session =
            create_test_session_with_updates(session_id, "Graph Test", Vec::new(), entity_graph);

        // Save session
        storage
            .save_session(&session)
            .await
            .expect("Failed to save session");

        // Verify entities in native table
        let entities = storage
            .list_entities(session_id)
            .await
            .expect("Failed to list entities");
        assert_eq!(entities.len(), 2, "Should have 2 entities");

        let rust_entity = entities.iter().find(|e| e.name == "Rust");
        assert!(rust_entity.is_some(), "Should find Rust entity");
        assert_eq!(rust_entity.unwrap().entity_type, EntityType::Technology);

        // Verify relationships via load_all_relationships
        let relationships = storage
            .load_all_relationships(session_id)
            .await
            .expect("Failed to load relationships");
        assert_eq!(relationships.len(), 1, "Should have 1 relationship");
        assert_eq!(relationships[0].from_entity, "Rust");
        assert_eq!(relationships[0].to_entity, "SurrealDB");
        assert_eq!(relationships[0].relation_type, RelationType::Implements);
        assert_eq!(relationships[0].context, "Rust client for SurrealDB");

        // Load session and verify graph is rebuilt
        let loaded = storage
            .load_session(session_id)
            .await
            .expect("Failed to load session");

        let loaded_entities = loaded.entity_graph.get_all_entities();
        assert_eq!(
            loaded_entities.len(),
            2,
            "Loaded session should have 2 entities"
        );

        let loaded_relationships = loaded.entity_graph.get_all_relationships();
        assert_eq!(
            loaded_relationships.len(),
            1,
            "Loaded session should have 1 relationship"
        );
        assert_eq!(loaded_relationships[0].context, "Rust client for SurrealDB");
    }

    #[test]
    fn test_extract_code_references() {
        use crate::core::context_update::{
            CodeReference as CoreCodeRef, UpdateContent, UpdateType,
        };

        let updates = vec![
            ContextUpdate {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                update_type: UpdateType::CodeChanged,
                content: UpdateContent {
                    title: "Fix bug".to_string(),
                    description: "Fixed null pointer".to_string(),
                    details: vec![],
                    examples: vec![],
                    implications: vec![],
                },
                related_code: Some(CoreCodeRef {
                    file_path: "src/main.rs".to_string(),
                    start_line: 10,
                    end_line: 20,
                    code_snippet: "fn main() {}".to_string(),
                    commit_hash: Some("abc123".to_string()),
                    branch: Some("main".to_string()),
                    change_description: "Fixed bug".to_string(),
                }),
                parent_update: None,
                user_marked_important: false,
                creates_entities: vec![],
                creates_relationships: vec![],
                references_entities: vec![],
            },
            ContextUpdate {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                update_type: UpdateType::CodeChanged,
                content: UpdateContent {
                    title: "Add feature".to_string(),
                    description: "Added logging".to_string(),
                    details: vec![],
                    examples: vec![],
                    implications: vec![],
                },
                related_code: Some(CoreCodeRef {
                    file_path: "src/main.rs".to_string(), // Same file, second reference
                    start_line: 30,
                    end_line: 40,
                    code_snippet: "fn log() {}".to_string(),
                    commit_hash: Some("def456".to_string()),
                    branch: Some("feature".to_string()),
                    change_description: "Added logging".to_string(),
                }),
                parent_update: None,
                user_marked_important: false,
                creates_entities: vec![],
                creates_relationships: vec![],
                references_entities: vec![],
            },
            ContextUpdate {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                update_type: UpdateType::QuestionAnswered,
                content: UpdateContent {
                    title: "Question".to_string(),
                    description: "Answer".to_string(),
                    details: vec![],
                    examples: vec![],
                    implications: vec![],
                },
                related_code: None, // No code reference
                parent_update: None,
                user_marked_important: false,
                creates_entities: vec![],
                creates_relationships: vec![],
                references_entities: vec![],
            },
        ];

        let refs = SurrealDBStorage::extract_code_references(&updates);

        // Should have 1 file with 2 references
        assert_eq!(refs.len(), 1, "Should have 1 file path");
        assert!(refs.contains_key("src/main.rs"), "Should have src/main.rs");

        let main_refs = refs.get("src/main.rs").unwrap();
        assert_eq!(
            main_refs.len(),
            2,
            "Should have 2 references for src/main.rs"
        );

        // Verify both references are captured
        assert!(
            main_refs.iter().any(|r| r.start_line == 10),
            "Should have first reference"
        );
        assert!(
            main_refs.iter().any(|r| r.start_line == 30),
            "Should have second reference"
        );
    }

    #[test]
    fn test_extract_change_history() {
        use crate::core::context_update::{UpdateContent, UpdateType};

        let updates = vec![
            ContextUpdate {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                update_type: UpdateType::CodeChanged,
                content: UpdateContent {
                    title: "Refactor storage".to_string(),
                    description: "Split into modules".to_string(),
                    details: vec![],
                    examples: vec![],
                    implications: vec![],
                },
                related_code: None,
                parent_update: None,
                user_marked_important: false,
                creates_entities: vec![],
                creates_relationships: vec![],
                references_entities: vec![],
            },
            ContextUpdate {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                update_type: UpdateType::QuestionAnswered, // Not CodeChanged
                content: UpdateContent {
                    title: "Question".to_string(),
                    description: "Answer".to_string(),
                    details: vec![],
                    examples: vec![],
                    implications: vec![],
                },
                related_code: None,
                parent_update: None,
                user_marked_important: false,
                creates_entities: vec![],
                creates_relationships: vec![],
                references_entities: vec![],
            },
            ContextUpdate {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                update_type: UpdateType::CodeChanged,
                content: UpdateContent {
                    title: "Add tests".to_string(),
                    description: "Unit tests for helpers".to_string(),
                    details: vec![],
                    examples: vec![],
                    implications: vec![],
                },
                related_code: None,
                parent_update: None,
                user_marked_important: false,
                creates_entities: vec![],
                creates_relationships: vec![],
                references_entities: vec![],
            },
        ];

        let history = SurrealDBStorage::extract_change_history(&updates);

        // Should only include CodeChanged updates
        assert_eq!(history.len(), 2, "Should have 2 change records");
        assert!(
            history
                .iter()
                .any(|r| r.description.contains("Refactor storage"))
        );
        assert!(history.iter().any(|r| r.description.contains("Add tests")));
        assert!(!history.iter().any(|r| r.description.contains("Question")));
    }

    #[test]
    fn test_rebuild_structured_context() {
        use crate::core::context_update::{UpdateContent, UpdateType};

        let updates = vec![
            ContextUpdate {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                update_type: UpdateType::DecisionMade,
                content: UpdateContent {
                    title: "Use SurrealDB".to_string(),
                    description: "For graph and vector storage".to_string(),
                    details: vec!["Option A".to_string(), "Option B".to_string()],
                    examples: vec![],
                    implications: vec![],
                },
                related_code: None,
                parent_update: None,
                user_marked_important: true,
                creates_entities: vec!["SurrealDB".to_string()],
                creates_relationships: vec![],
                references_entities: vec![],
            },
            ContextUpdate {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                update_type: UpdateType::QuestionAnswered,
                content: UpdateContent {
                    title: "How to normalize?".to_string(),
                    description: "Store in separate tables".to_string(),
                    details: vec![],
                    examples: vec![],
                    implications: vec![],
                },
                related_code: None,
                parent_update: None,
                user_marked_important: false,
                creates_entities: vec![],
                creates_relationships: vec![],
                references_entities: vec!["SurrealDB".to_string()],
            },
        ];

        let context = SurrealDBStorage::rebuild_structured_context(&updates);

        // Should have 1 decision
        assert_eq!(context.key_decisions.len(), 1, "Should have 1 decision");
        assert_eq!(context.key_decisions[0].description, "Use SurrealDB");
        assert_eq!(
            context.key_decisions[0].confidence, 0.9,
            "Important = high confidence"
        );
        assert_eq!(context.key_decisions[0].alternatives.len(), 2);

        // Should have 1 question
        assert_eq!(context.open_questions.len(), 1, "Should have 1 question");
        assert_eq!(context.open_questions[0].question, "How to normalize?");

        // Should have conversation flow
        assert_eq!(
            context.conversation_flow.len(),
            2,
            "Should have 2 flow items"
        );

        // Should have key concepts from entities
        assert_eq!(context.key_concepts.len(), 1, "Should have 1 concept");
        assert_eq!(context.key_concepts[0].name, "SurrealDB");
    }

    #[tokio::test]
    async fn test_full_normalized_roundtrip() {
        use crate::core::context_update::{
            CodeReference as CoreCodeRef, UpdateContent, UpdateType,
        };

        let storage = SurrealDBStorage::new("mem://", None, None, None, None)
            .await
            .expect("Failed to create SurrealDB storage");

        let session_id = Uuid::new_v4();

        // Create entity graph with entity
        let mut entity_graph = SimpleEntityGraph::new();
        entity_graph.add_or_update_entity(
            "TestEntity".to_string(),
            EntityType::Concept,
            Utc::now(),
            "Test description",
        );

        // Create context update with code reference
        let update = ContextUpdate {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            update_type: UpdateType::CodeChanged,
            content: UpdateContent {
                title: "Implement feature".to_string(),
                description: "Added new functionality".to_string(),
                details: vec!["Detail 1".to_string()],
                examples: vec![],
                implications: vec![],
            },
            related_code: Some(CoreCodeRef {
                file_path: "src/lib.rs".to_string(),
                start_line: 100,
                end_line: 150,
                code_snippet: "pub fn new_feature() {}".to_string(),
                commit_hash: Some("abc123".to_string()),
                branch: Some("main".to_string()),
                change_description: "New feature implementation".to_string(),
            }),
            parent_update: None,
            user_marked_important: true,
            creates_entities: vec!["TestEntity".to_string()],
            creates_relationships: vec![],
            references_entities: vec![],
        };

        // Create session with update and entity graph
        let session = create_test_session_with_updates(
            session_id,
            "Full Roundtrip Test",
            vec![update],
            entity_graph,
        );

        // Save
        storage
            .save_session(&session)
            .await
            .expect("Failed to save session");

        // Load
        let loaded = storage
            .load_session(session_id)
            .await
            .expect("Failed to load session");

        // Verify all data is preserved
        assert_eq!(
            loaded.name().clone(),
            Some("Full Roundtrip Test".to_string())
        );
        assert_eq!(loaded.hot_context.len(), 1);

        // Verify entities
        let entities = loaded.entity_graph.get_all_entities();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name, "TestEntity");

        // Verify code_references extracted
        let code_refs = &loaded.code_references;
        assert!(
            code_refs.contains_key("src/lib.rs"),
            "Should have code reference"
        );
        let lib_refs = code_refs.get("src/lib.rs").unwrap();
        assert_eq!(lib_refs.len(), 1);
        assert_eq!(lib_refs[0].start_line, 100);

        // Verify change_history extracted
        let change_history = &loaded.change_history;
        assert_eq!(change_history.len(), 1);
        assert!(change_history[0].description.contains("Implement feature"));

        // Verify structured context rebuilt
        // (The update is CodeChanged, so it won't create decisions/questions,
        // but it will be in conversation_flow)
        assert!(!loaded.current_state.conversation_flow.is_empty());
    }
}
