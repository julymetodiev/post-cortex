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

//! Constructor, schema initialization, and shared helpers for
//! [`SurrealDBStorage`].

use anyhow::Result;
use serde::Deserialize;
use std::sync::Arc;
use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Root;
use surrealdb::types::SurrealValue;
use tracing::info;
use uuid::Uuid;

use post_cortex_core::core::context_update::{EntityType, RelationType};

use super::SurrealDBStorage;

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
    pub(super) async fn select_all<T: for<'de> Deserialize<'de> + SurrealValue>(
        &self,
        table: &str,
    ) -> surrealdb::Result<Vec<T>> {
        self.db.select(table).await
    }

    /// Select a single record by ID (table, id)
    pub(super) async fn select_one<T: for<'de> Deserialize<'de> + SurrealValue>(
        &self,
        table: &str,
        id: &str,
    ) -> surrealdb::Result<Option<T>> {
        self.db.select((table, id)).await
    }

    /// Delete a record and return it (table, id)
    pub(super) async fn delete<T: for<'de> Deserialize<'de> + SurrealValue>(
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

            -- Source references table
            DEFINE TABLE IF NOT EXISTS source_reference SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS entry_id ON source_reference TYPE string;
            DEFINE FIELD IF NOT EXISTS file_path ON source_reference TYPE string;
            DEFINE FIELD IF NOT EXISTS content_hash ON source_reference TYPE array;
            DEFINE FIELD IF NOT EXISTS captured_at_unix ON source_reference TYPE int;
            DEFINE FIELD IF NOT EXISTS symbol_name ON source_reference TYPE option<string>;
            DEFINE FIELD IF NOT EXISTS symbol_type ON source_reference TYPE option<string>;
            DEFINE FIELD IF NOT EXISTS ast_hash ON source_reference TYPE option<array>;
            DEFINE FIELD IF NOT EXISTS imports ON source_reference TYPE option<array>;
            DEFINE FIELD IF NOT EXISTS status ON source_reference TYPE int DEFAULT 0;
            DEFINE INDEX IF NOT EXISTS idx_source_ref_entry ON source_reference FIELDS entry_id UNIQUE;
            DEFINE INDEX IF NOT EXISTS idx_source_ref_path ON source_reference FIELDS file_path;
        "#;

        self.db.query(schema).await?;

        info!("SurrealDBStorage: Schema initialized successfully");

        Ok(())
    }

    /// Get the relation table name for a relation type
    pub(super) fn relation_table_name(relation_type: &RelationType) -> &'static str {
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
    pub(super) fn relation_id(
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
    pub(super) fn entity_id(session_id: Uuid, name: &str) -> String {
        format!("{}_{}", session_id, name)
    }

    /// Parse RFC3339 datetime string, fallback to current time on error
    pub(super) fn parse_datetime(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now())
    }

    /// Parse relation type from string
    #[allow(dead_code)]
    pub(super) fn parse_relation_type(s: &str) -> RelationType {
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
    pub(super) fn parse_entity_type(s: &str) -> EntityType {
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
