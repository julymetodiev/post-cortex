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

use post_cortex_proto::pb::SourceReference;

// ============================================================================
// SurrealDB Deserialization Helpers
// ============================================================================

/// SurrealDB's NONE type doesn't deserialize cleanly to Option<String>.
/// This helper accepts both null/missing and string values gracefully.
pub(super) fn deserialize_surreal_option<'de, D>(
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
pub(super) struct SessionRecord {
    pub(super) session_id: String,
    #[serde(default, deserialize_with = "deserialize_surreal_option")]
    pub(super) name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_surreal_option")]
    pub(super) description: Option<String>,
    pub(super) created_at: String,
    pub(super) last_updated: String,
    // User preferences as JSON (small, queryable)
    pub(super) user_preferences: JsonValue,
    // Vectorization tracking
    pub(super) vectorized_update_ids: Vec<String>,
    // Total updates count (for pagination)
    pub(super) total_updates: u32,
}

/// Context update record for SurrealDB - hybrid storage
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub(super) struct ContextUpdateRecord {
    pub(super) update_id: String,
    pub(super) session_id: String,
    pub(super) timestamp: String,
    pub(super) update_type: String,
    // Full update as JSON (queryable object!)
    pub(super) update_data: JsonValue,
}

/// Entity record for SurrealDB graph nodes
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub(super) struct EntityRecord {
    pub(super) session_id: String,
    pub(super) name: String,
    pub(super) entity_type: String,
    pub(super) first_mentioned: String,
    pub(super) last_mentioned: String,
    pub(super) mention_count: u32,
    pub(super) importance_score: f32,
    pub(super) description: Option<String>,
}

/// Embedding record for vector storage
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub(super) struct EmbeddingRecord {
    pub(super) content_id: String,
    pub(super) session_id: String,
    pub(super) vector: Vec<f32>,
    pub(super) text: String,
    pub(super) content_type: String,
    pub(super) timestamp: String,
    pub(super) metadata: HashMap<String, String>,
}

/// KNN search result with distance from HNSW index
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub(super) struct KnnResult {
    pub(super) content_id: String,
    pub(super) session_id: String,
    pub(super) text: String,
    pub(super) content_type: String,
    pub(super) timestamp: String,
    pub(super) metadata: HashMap<String, String>,
    pub(super) distance: f32,
}

/// Workspace record for SurrealDB
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub(super) struct WorkspaceRecord {
    pub(super) workspace_id: String,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) created_at: u64,
}

/// Workspace-Session association record
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub(super) struct WorkspaceSessionRecord {
    pub(super) workspace_id: String,
    pub(super) session_id: String,
    pub(super) role: String,
    pub(super) added_at: u64,
}

/// Checkpoint record for SurrealDB - hybrid storage
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub(super) struct CheckpointRecord {
    pub(super) checkpoint_id: String,
    pub(super) session_id: String,
    pub(super) created_at: String,
    // Complete context snapshot as JSON (queryable)
    pub(super) structured_context: JsonValue,
    pub(super) recent_updates: JsonValue,
    pub(super) code_references: JsonValue,
    pub(super) change_history: JsonValue,
    // Metadata (native scalars)
    pub(super) total_updates: u32,
    pub(super) context_quality_score: f32,
    pub(super) compression_ratio: f32,
}

/// Source reference record for SurrealDB
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub(super) struct SourceReferenceRecord {
    pub(super) entry_id: String,
    pub(super) file_path: String,
    pub(super) content_hash: Vec<u8>,
    pub(super) captured_at_unix: i64,
    #[serde(default)]
    pub(super) symbol_name: Option<String>,
    #[serde(default)]
    pub(super) symbol_type: Option<String>,
    #[serde(default)]
    pub(super) ast_hash: Option<Vec<u8>>,
    #[serde(default)]
    pub(super) imports: Option<Vec<String>>,
    /// 0 = fresh (default), 1 = stale (marked by invalidate_source / cascade_invalidate).
    /// Allows distinguishing "known stale" from "hash mismatch stale" vs "Unknown".
    #[serde(default)]
    pub(super) status: i32,
}

/// Symbol dependency record for SurrealDB
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub(super) struct SymbolDepRecord {
    pub(super) from_file: String,
    pub(super) from_symbol: String,
    pub(super) to_file: String,
    pub(super) to_symbol: String,
    pub(super) to_symbol_type: String,
}

pub(super) fn source_record_to_reference(r: SourceReferenceRecord) -> SourceReference {
    use post_cortex_proto::pb::{FunctionScope, SourceScope, source_scope};
    let scope = r.symbol_name.as_ref().map(|name| SourceScope {
        scope: Some(source_scope::Scope::Function(FunctionScope {
            name: name.clone(),
            ast_hash: r.ast_hash.clone().unwrap_or_default(),
            symbol_type: r.symbol_type.clone().unwrap_or_default(),
            imports: r.imports.clone().unwrap_or_default(),
        })),
    });

    SourceReference {
        entry_id: r.entry_id,
        file_path: r.file_path,
        content_hash: r.content_hash,
        captured_at_unix: r.captured_at_unix,
        scope,
    }
}
