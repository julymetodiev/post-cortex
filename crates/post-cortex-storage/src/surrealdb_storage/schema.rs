// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The schema: the tables, fields and indexes this build reads and writes.

use crate::port_error::Result;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use tracing::info;

/// Fail unless the selected database holds the post-cortex tables.
pub async fn check_initialised(db: &Surreal<Any>, name: &str) -> Result<()> {
    let mut response = db
        .query("RETURN [(INFO FOR DB).tables.session, (INFO FOR DB).tables.embedding];")
        .await?;
    let tables: Vec<Option<String>> = response.take(0)?;
    if tables.iter().any(Option::is_none) {
        return Err(post_cortex_ports::PortError::unavailable(format!(
            "SurrealDB database {name} has no post-cortex schema (no session or \
             embedding table). Check surrealdb_namespace and surrealdb_database \
             in daemon.toml; a read-only open never creates the schema."
        ))
        .into());
    }
    Ok(())
}

/// Initialize the database schema.
pub async fn initialize(db: &Surreal<Any>) -> Result<()> {
    info!("SurrealDBStorage: Initializing schema");

    // First, remove old binary 'data' fields if they exist (schema migration)
    let cleanup = r#"
        -- Remove old binary data fields from previous schema versions
        REMOVE FIELD IF EXISTS data ON session;
        REMOVE FIELD IF EXISTS data ON context_update;
        REMOVE FIELD IF EXISTS data ON checkpoint;
    "#;

    db.query(cleanup).await?.check()?;

    // Define tables with schema - using SCHEMALESS for complex nested JSON storage
    // This allows storing arbitrary JSON structures while still being queryable.
    // One transaction: on a store that syncs every commit (RocksDB), each of
    // the ~130 statements on its own was one sync, 1 s on every tenant load
    // (64 ms in one); a statement that fails fails the whole schema.
    let schema = r#"
        BEGIN TRANSACTION;
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
        DEFINE FIELD IF NOT EXISTS aliases ON entity TYPE option<array<string>>;
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
        -- Free-form string map (`VectorMetadata::metadata`). FLEXIBLE: a
        -- SCHEMAFULL object field otherwise rejects every key. Databases
        -- created with the old, non-FLEXIBLE definition are upgraded
        -- below, only when needed.
        DEFINE FIELD IF NOT EXISTS metadata ON embedding TYPE object FLEXIBLE;
        -- Content ids are unique per session only (e.g. `entity:<name>`
        -- exists in many sessions); records are keyed `{session}_{content}`.
        -- The old globally-unique index made sessions overwrite each other.
        REMOVE INDEX IF EXISTS idx_embedding_content ON embedding;
        DEFINE INDEX IF NOT EXISTS idx_embedding_content_id ON embedding FIELDS content_id;
        DEFINE INDEX IF NOT EXISTS idx_embedding_session_content ON embedding FIELDS session_id, content_id UNIQUE;
        DEFINE INDEX IF NOT EXISTS idx_embedding_session ON embedding FIELDS session_id;
        DEFINE INDEX IF NOT EXISTS idx_embedding_type ON embedding FIELDS content_type;
        -- No vector or full-text index: search (vectors and BM25) runs on
        -- the in-memory index the memory layer loads from this table (the
        -- HNSW index of data version 1 is removed by migration step 2).
        -- The BM25 index of earlier versions is removed: on a distributed
        -- store (TiKV) it took a round trip per matching row, past every
        -- search's deadline.
        REMOVE INDEX IF EXISTS idx_embedding_text ON embedding;
        REMOVE ANALYZER IF EXISTS pcx_text;

        -- Raw message stream. Keyed
        -- `{session}_{seq}`; `turn_id` makes appends idempotent.
        DEFINE TABLE IF NOT EXISTS message SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS session_id ON message TYPE string;
        DEFINE FIELD IF NOT EXISTS seq ON message TYPE int;
        DEFINE FIELD IF NOT EXISTS turn_id ON message TYPE string;
        DEFINE FIELD IF NOT EXISTS role ON message TYPE string;
        DEFINE FIELD IF NOT EXISTS author ON message TYPE option<string>;
        DEFINE FIELD IF NOT EXISTS source_session ON message TYPE option<string>;
        DEFINE FIELD IF NOT EXISTS timestamp ON message TYPE string;
        DEFINE FIELD IF NOT EXISTS text ON message TYPE string;
        DEFINE FIELD IF NOT EXISTS tools ON message TYPE array<string>;
        DEFINE FIELD IF NOT EXISTS files ON message TYPE array<string>;
        -- An agent's turn whole (W18): the turn it follows and its blocks
        -- as JSON; none on captured turns and rows written before.
        DEFINE FIELD IF NOT EXISTS parent ON message TYPE option<string>;
        DEFINE FIELD IF NOT EXISTS blocks ON message TYPE option<string>;
        DEFINE INDEX IF NOT EXISTS idx_message_session_seq ON message FIELDS session_id, seq UNIQUE;
        DEFINE INDEX IF NOT EXISTS idx_message_session_turn ON message FIELDS session_id, turn_id UNIQUE;
        -- Next position of each stream, keyed by session id.
        DEFINE TABLE IF NOT EXISTS message_next SCHEMALESS;
        -- The summary of each stream, keyed by session id: count, first and
        -- last position with their times, the authors (messages/summary.rs).
        DEFINE TABLE IF NOT EXISTS message_stream SCHEMALESS;

        -- Project and user cards, one row per
        -- version, keyed `{scope}_{version}`.
        DEFINE TABLE IF NOT EXISTS card SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS scope ON card TYPE string;
        DEFINE FIELD IF NOT EXISTS version ON card TYPE int;
        DEFINE FIELD IF NOT EXISTS text ON card TYPE string;
        DEFINE FIELD IF NOT EXISTS updated_at ON card TYPE string;
        DEFINE FIELD IF NOT EXISTS author ON card TYPE option<string>;
        DEFINE INDEX IF NOT EXISTS idx_card_scope_version ON card FIELDS scope, version UNIQUE;

        -- Documents of modules, one row per version, keyed
        -- `{workspace}_{id}_{version}`; `latest` marks each one's latest.
        DEFINE TABLE IF NOT EXISTS document SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS workspace_id ON document TYPE string;
        DEFINE FIELD IF NOT EXISTS doc_id ON document TYPE string;
        DEFINE FIELD IF NOT EXISTS kind ON document TYPE string;
        DEFINE FIELD IF NOT EXISTS version ON document TYPE int;
        DEFINE FIELD IF NOT EXISTS data ON document TYPE string;
        DEFINE FIELD IF NOT EXISTS title ON document TYPE string;
        DEFINE FIELD IF NOT EXISTS text ON document TYPE string;
        DEFINE FIELD IF NOT EXISTS updated_at ON document TYPE string;
        DEFINE FIELD IF NOT EXISTS author ON document TYPE option<string>;
        DEFINE FIELD IF NOT EXISTS latest ON document TYPE bool;
        DEFINE INDEX IF NOT EXISTS idx_document_version ON document FIELDS workspace_id, doc_id, version UNIQUE;
        DEFINE INDEX IF NOT EXISTS idx_document_latest ON document FIELDS workspace_id, latest, kind;

        -- The order and the keys of the latest versions (document_index.rs).
        DEFINE TABLE IF NOT EXISTS document_order SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS workspace_id ON document_order TYPE string;
        DEFINE FIELD IF NOT EXISTS kind ON document_order TYPE string;
        DEFINE FIELD IF NOT EXISTS doc_id ON document_order TYPE string;
        DEFINE FIELD IF NOT EXISTS version ON document_order TYPE int;
        DEFINE FIELD IF NOT EXISTS inv ON document_order TYPE int;
        DEFINE INDEX IF NOT EXISTS idx_document_order_kind ON document_order FIELDS workspace_id, kind;
        DEFINE TABLE IF NOT EXISTS document_key SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS workspace_id ON document_key TYPE string;
        DEFINE FIELD IF NOT EXISTS kind ON document_key TYPE string;
        DEFINE FIELD IF NOT EXISTS key ON document_key TYPE string;
        DEFINE FIELD IF NOT EXISTS doc_id ON document_key TYPE string;
        DEFINE FIELD IF NOT EXISTS version ON document_key TYPE int;
        DEFINE FIELD IF NOT EXISTS inv ON document_key TYPE int;
        DEFINE INDEX IF NOT EXISTS idx_document_key ON document_key FIELDS workspace_id, kind, key;
        DEFINE INDEX IF NOT EXISTS idx_document_key_doc ON document_key FIELDS workspace_id, doc_id;
        DEFINE TABLE IF NOT EXISTS document_unkeyed SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS workspace_id ON document_unkeyed TYPE string;
        DEFINE FIELD IF NOT EXISTS kind ON document_unkeyed TYPE string;
        DEFINE FIELD IF NOT EXISTS doc_id ON document_unkeyed TYPE string;
        DEFINE FIELD IF NOT EXISTS version ON document_unkeyed TYPE int;
        DEFINE FIELD IF NOT EXISTS inv ON document_unkeyed TYPE int;
        DEFINE INDEX IF NOT EXISTS idx_document_unkeyed ON document_unkeyed FIELDS workspace_id, kind;

        -- Workspaces table
        DEFINE TABLE IF NOT EXISTS workspace SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS workspace_id ON workspace TYPE string;
        DEFINE FIELD IF NOT EXISTS name ON workspace TYPE string;
        -- Its unique index is data version 4's, after the older rows got theirs.
        DEFINE FIELD IF NOT EXISTS slug ON workspace TYPE string;
        DEFINE FIELD IF NOT EXISTS description ON workspace TYPE string;
        DEFINE FIELD IF NOT EXISTS created_at ON workspace TYPE int;
        DEFINE FIELD IF NOT EXISTS projects ON workspace TYPE option<array<object>>;
        DEFINE FIELD IF NOT EXISTS projects[*].key ON workspace TYPE string;
        DEFINE FIELD IF NOT EXISTS projects[*].session_id ON workspace TYPE string;
        -- Unix seconds the workspace was archived; NONE while active.
        DEFINE FIELD IF NOT EXISTS archived_at ON workspace TYPE option<int>;
        -- Whether its accurate searches may use the remote reranker; data
        -- version 5 gave the older rows false. ALWAYS: a row written before
        -- it (data version 4's slug step updates rows of version 3) gets false
        -- instead of failing for NONE; OVERWRITE gives a database that has the
        -- field without ALWAYS this definition.
        DEFINE FIELD OVERWRITE remote_rerank ON workspace TYPE bool DEFAULT ALWAYS false;
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

        -- Radar items a workspace dismissed, one row per (kind, key).
        DEFINE TABLE IF NOT EXISTS radar_dismissal SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS workspace_id ON radar_dismissal TYPE string;
        DEFINE FIELD IF NOT EXISTS kind ON radar_dismissal TYPE string;
        DEFINE FIELD IF NOT EXISTS key ON radar_dismissal TYPE string;
        DEFINE FIELD IF NOT EXISTS dismissed_at ON radar_dismissal TYPE string;
        DEFINE FIELD IF NOT EXISTS author ON radar_dismissal TYPE option<string>;
        DEFINE INDEX IF NOT EXISTS idx_radar_dismissal ON radar_dismissal FIELDS workspace_id, kind, key UNIQUE;

        -- The organisation's bus (docs/host.md, events of a module): one row
        -- per event keyed by its position, which a create of a taken key
        -- refuses, so two appends never share one; the next position in
        -- `bus_next:log`, kept so none is reused after old events go; the
        -- cursors keyed `{follower}|{name}`.
        DEFINE TABLE IF NOT EXISTS bus_event SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS position ON bus_event TYPE int;
        DEFINE FIELD IF NOT EXISTS event_id ON bus_event TYPE string;
        DEFINE FIELD IF NOT EXISTS name ON bus_event TYPE string;
        DEFINE FIELD IF NOT EXISTS module ON bus_event TYPE string;
        DEFINE FIELD IF NOT EXISTS workspace_id ON bus_event TYPE option<string>;
        DEFINE FIELD IF NOT EXISTS author ON bus_event TYPE option<string>;
        -- `at` as written (RFC 3339, read back exactly), `at_ns` its
        -- nanoseconds since the epoch, which a drop compares.
        DEFINE FIELD IF NOT EXISTS at ON bus_event TYPE string;
        DEFINE FIELD IF NOT EXISTS at_ns ON bus_event TYPE int;
        DEFINE FIELD IF NOT EXISTS payload_type ON bus_event TYPE string;
        DEFINE FIELD IF NOT EXISTS payload ON bus_event TYPE array;
        DEFINE INDEX IF NOT EXISTS idx_bus_event_name ON bus_event FIELDS name, position;
        DEFINE INDEX IF NOT EXISTS idx_bus_event_at ON bus_event FIELDS at_ns;
        DEFINE TABLE IF NOT EXISTS bus_next SCHEMALESS;
        DEFINE TABLE IF NOT EXISTS bus_cursor SCHEMALESS;
        -- The newest position dropped of each name, keyed by the name.
        DEFINE TABLE IF NOT EXISTS bus_dropped SCHEMALESS;

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
        DEFINE FIELD IF NOT EXISTS session_id ON source_reference TYPE option<string>;
        -- Block (line-range) scopes; NONE for whole-file and function ones.
        DEFINE FIELD IF NOT EXISTS block_start ON source_reference TYPE option<int>;
        DEFINE FIELD IF NOT EXISTS block_end ON source_reference TYPE option<int>;
        DEFINE FIELD IF NOT EXISTS block_hash ON source_reference TYPE option<array>;
        DEFINE INDEX IF NOT EXISTS idx_source_ref_entry ON source_reference FIELDS entry_id UNIQUE;
        DEFINE INDEX IF NOT EXISTS idx_source_ref_path ON source_reference FIELDS file_path;
        COMMIT TRANSACTION;
    "#;

    db.query(schema).await?.check()?;

    upgrade_metadata_field(db).await?;
    info!("SurrealDBStorage: Schema initialized successfully");

    Ok(())
}

/// Make `embedding.metadata` FLEXIBLE on databases created with the old
/// definition. Only rewrites the field when it is not FLEXIBLE already,
/// so an older binary never overwrites a newer definition.
async fn upgrade_metadata_field(db: &Surreal<Any>) -> Result<()> {
    let mut response = db
        .query("RETURN (INFO FOR TABLE embedding).fields.metadata;")
        .await?;
    let current: Option<String> = response.take(0)?;
    if current
        .as_deref()
        .is_some_and(|d| !d.to_ascii_uppercase().contains("FLEXIBLE"))
    {
        info!("SurrealDBStorage: Making embedding.metadata FLEXIBLE (was {current:?})");
        db.query("DEFINE FIELD OVERWRITE metadata ON embedding TYPE object FLEXIBLE;")
            .await?
            .check()?;
    }
    Ok(())
}
