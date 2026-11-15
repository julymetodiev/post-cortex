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

//! Internal SurrealDB record types and serde helpers.
//!
//! These mirror table schemas defined in `core::initialize_schema` — keep them
//! in sync.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use surrealdb::types::SurrealValue;

use post_cortex_core::core::freshness::{BlockScope, FunctionScope, SourceReference, SourceScope};

// ============================================================================
// SurrealDB Deserialization Helpers
// ============================================================================

/// SurrealDB's NONE type doesn't deserialize cleanly to Option<String>.
/// This helper accepts both null/missing and string values gracefully.
pub fn deserialize_surreal_option<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct OptionStringVisitor;

    impl<'de> de::Visitor<'de> for OptionStringVisitor {
        type Value = Option<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string, null, or NONE")
        }

        fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D2: serde::Deserializer<'de>>(
            self,
            d: D2,
        ) -> std::result::Result<Self::Value, D2::Error> {
            Ok(Some(String::deserialize(d)?))
        }

        fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
            Ok(Some(v))
        }
    }

    deserializer.deserialize_option(OptionStringVisitor)
}

// ============================================================================
// SurrealDB Record Types
// ============================================================================

/// Session record for SurrealDB - NORMALIZED (no JSON blobs for context!)
/// Context updates are stored in context_update table
/// Entities are stored in entity table (native graph)
/// Relationships are stored via RELATE (native graph)
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct SessionRecord {
    pub session_id: String,
    #[serde(default, deserialize_with = "deserialize_surreal_option")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_surreal_option")]
    pub description: Option<String>,
    pub created_at: String,
    pub last_updated: String,
    // User preferences as JSON (small, queryable)
    pub user_preferences: JsonValue,
    // Vectorization tracking
    pub vectorized_update_ids: Vec<String>,
    // Total updates count (for pagination)
    pub total_updates: u32,
}

/// What a save writes of the `session` row: everything but the vectorized
/// ids, which the save adds and removes (MERGE keeps them).
#[derive(Debug, Clone, Serialize, SurrealValue)]
pub struct SessionHeadRecord {
    pub session_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub created_at: String,
    pub last_updated: String,
    pub user_preferences: JsonValue,
    pub total_updates: u32,
}

impl SessionHeadRecord {
    /// The head of `session`.
    ///
    /// # Errors
    /// Its preferences cannot be serialised.
    pub fn of(
        session: &post_cortex_core::session::active_session::ActiveSession,
    ) -> crate::port_error::Result<Self> {
        Ok(Self {
            session_id: session.id().to_string(),
            name: session.name(),
            description: session.description(),
            created_at: session.created_at().to_rfc3339(),
            last_updated: session.last_updated().to_rfc3339(),
            user_preferences: serde_json::to_value(session.user_preferences())?,
            total_updates: u32::try_from(session.records().len()).unwrap_or(u32::MAX),
        })
    }
}

/// Context update record for SurrealDB - hybrid storage
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct ContextUpdateRecord {
    pub update_id: String,
    pub session_id: String,
    pub timestamp: String,
    pub update_type: String,
    // Full update as JSON (queryable object!)
    pub update_data: JsonValue,
}

/// Entity record for SurrealDB graph nodes
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct EntityRecord {
    pub session_id: String,
    pub name: String,
    pub entity_type: String,
    pub first_mentioned: String,
    pub last_mentioned: String,
    pub mention_count: u32,
    pub importance_score: f32,
    pub description: Option<String>,
    /// Other names the entity was written under (NONE on rows from before
    /// entity resolution).
    pub aliases: Option<Vec<String>>,
}

/// Embedding record for vector storage
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct EmbeddingRecord {
    pub content_id: String,
    pub session_id: String,
    pub vector: Vec<f32>,
    pub text: String,
    pub content_type: String,
    pub timestamp: String,
    pub metadata: HashMap<String, String>,
}

/// Workspace record for SurrealDB
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct WorkspaceRecord {
    pub workspace_id: String,
    pub name: String,
    /// The segment of its addresses (data version 4 gave the older rows
    /// theirs).
    pub slug: String,
    pub description: String,
    pub created_at: u64,
    /// Projects resolved to the workspace (NONE on rows from before them).
    pub projects: Option<Vec<WorkspaceProjectRecord>>,
    /// Unix seconds the workspace was archived (NONE while active and on
    /// rows from before the field).
    pub archived_at: Option<u64>,
    /// Whether its accurate searches may use the remote reranker (data
    /// version 5 gave the older rows false).
    pub remote_rerank: bool,
}

/// A project bound to a workspace (`WorkspaceProject`).
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct WorkspaceProjectRecord {
    pub key: String,
    pub session_id: String,
}

/// Workspace-Session association record
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct WorkspaceSessionRecord {
    pub workspace_id: String,
    pub session_id: String,
    pub role: String,
    pub added_at: u64,
}

/// Checkpoint record for SurrealDB - hybrid storage
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct CheckpointRecord {
    pub checkpoint_id: String,
    pub session_id: String,
    pub created_at: String,
    // Complete context snapshot as JSON (queryable)
    pub structured_context: JsonValue,
    pub recent_updates: JsonValue,
    pub code_references: JsonValue,
    pub change_history: JsonValue,
    // Metadata (native scalars)
    pub total_updates: u32,
    pub context_quality_score: f32,
    pub compression_ratio: f32,
}

/// Source reference record for SurrealDB
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct SourceReferenceRecord {
    pub entry_id: String,
    pub file_path: String,
    pub content_hash: Vec<u8>,
    pub captured_at_unix: i64,
    #[serde(default)]
    pub symbol_name: Option<String>,
    #[serde(default)]
    pub symbol_type: Option<String>,
    #[serde(default)]
    pub ast_hash: Option<Vec<u8>>,
    #[serde(default)]
    pub imports: Option<Vec<String>>,
    /// First line of a block (line-range) scope.
    #[serde(default)]
    pub block_start: Option<i64>,
    /// Last line of a block scope.
    #[serde(default)]
    pub block_end: Option<i64>,
    /// Hash of a block scope's text.
    #[serde(default)]
    pub block_hash: Option<Vec<u8>>,
    /// [`crate::freshness::SourceStatus`] code: 0 registered (default),
    /// 1 stale (marked by invalidate_source / cascade_invalidate).
    #[serde(default)]
    pub status: i32,
    /// Session that registered the reference; `None` for records from
    /// before sessions were recorded.
    #[serde(default)]
    pub session_id: Option<String>,
}

impl SourceReferenceRecord {
    /// The record as the freshness state machine sees it; an unknown status
    /// code is [`Corrupt`](post_cortex_ports::PortError::Corrupt).
    pub fn state(&self) -> crate::port_error::Result<crate::freshness::StoredState<'_>> {
        Ok(crate::freshness::StoredState {
            file_path: &self.file_path,
            content_hash: &self.content_hash,
            ast_hash: self.ast_hash.as_deref().filter(|h| !h.is_empty()),
            status: crate::freshness::SourceStatus::from_code(
                i64::from(self.status),
                &self.entry_id,
            )?,
        })
    }
}

/// Symbol dependency record for SurrealDB
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct SymbolDepRecord {
    pub from_file: String,
    pub from_symbol: String,
    pub to_file: String,
    pub to_symbol: String,
    pub to_symbol_type: String,
}

/// The reference a record stores; a block line outside `u32` is
/// [`Corrupt`](post_cortex_ports::PortError::Corrupt).
pub fn source_record_to_reference(
    r: SourceReferenceRecord,
) -> crate::port_error::Result<SourceReference> {
    let line = |value: i64| {
        u32::try_from(value).map_err(|_| {
            post_cortex_ports::error::corrupt("source reference", &r.entry_id, "block line")
        })
    };
    let scope = match (r.symbol_name, r.block_start, r.block_end) {
        (Some(name), _, _) => SourceScope::Function(FunctionScope {
            name,
            ast_hash: r.ast_hash.unwrap_or_default(),
            symbol_type: r.symbol_type.unwrap_or_default(),
            imports: r.imports.unwrap_or_default(),
        }),
        (None, Some(start), Some(end)) => SourceScope::Block(BlockScope {
            start_line: line(start)?,
            end_line: line(end)?,
            content_hash: r.block_hash.unwrap_or_default(),
        }),
        _ => SourceScope::WholeFile,
    };
    Ok(SourceReference {
        entry_id: r.entry_id,
        file_path: r.file_path,
        content_hash: r.content_hash,
        captured_at_unix: r.captured_at_unix,
        scope,
    })
}
