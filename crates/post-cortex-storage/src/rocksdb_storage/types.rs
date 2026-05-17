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

//! Persisted record types and shared constants for the RocksDB backend.

use post_cortex_core::core::context_update::{ContextUpdate, EntityData, EntityRelationship, RelationType};
use post_cortex_core::core::structured_context::StructuredContext;
use post_cortex_embeddings::VectorMetadata;
use post_cortex_core::session::active_session::{ChangeRecord, CodeReference};
use post_cortex_core::workspace::SessionRole;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Required embedding dimension for vector storage (must match model output)
pub const EMBEDDING_DIMENSION: usize = 384;

/// Workspace record persisted in RocksDB.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredWorkspace {
    /// Workspace ID.
    pub id: Uuid,
    /// Workspace display name.
    pub name: String,
    /// Workspace description.
    pub description: String,
    /// Member sessions and their roles.
    pub sessions: Vec<(Uuid, SessionRole)>,
    /// Creation timestamp (Unix seconds).
    pub created_at: u64,
}

/// Per-session workspace membership record.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredWorkspaceSession {
    /// Workspace this membership belongs to.
    pub workspace_id: Uuid,
    /// Session that is a member of the workspace.
    pub session_id: Uuid,
    /// Role of the session within the workspace.
    pub role: SessionRole,
    /// Timestamp the session was added (Unix seconds).
    pub added_at: u64,
}

/// Stored entity record for RocksDB persistence
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredEntity {
    /// Session the entity belongs to.
    pub session_id: Uuid,
    /// Entity name (unique within the session).
    pub name: String,
    /// Entity type, stored as its `Debug` representation.
    pub entity_type: String,
    /// Timestamp the entity was first mentioned.
    pub first_mentioned: DateTime<Utc>,
    /// Timestamp the entity was most recently mentioned.
    pub last_mentioned: DateTime<Utc>,
    /// Total number of times the entity has been mentioned.
    pub mention_count: u32,
    /// Computed importance score in `[0.0, 1.0]`.
    pub importance_score: f32,
    /// Optional human-readable description.
    pub description: Option<String>,
}

impl StoredEntity {
    /// Build a `StoredEntity` from an in-memory `EntityData` plus its session ID.
    pub fn from_entity_data(session_id: Uuid, entity: &EntityData) -> Self {
        Self {
            session_id,
            name: entity.name.clone(),
            entity_type: format!("{:?}", entity.entity_type),
            first_mentioned: entity.first_mentioned,
            last_mentioned: entity.last_mentioned,
            mention_count: entity.mention_count,
            importance_score: entity.importance_score,
            description: entity.description.clone(),
        }
    }

    /// Convert this stored record back into an in-memory `EntityData`.
    pub fn to_entity_data(&self) -> EntityData {
        use post_cortex_core::core::context_update::EntityType;
        EntityData {
            name: self.name.clone(),
            entity_type: match self.entity_type.as_str() {
                "Technology" => EntityType::Technology,
                "Concept" => EntityType::Concept,
                "Problem" => EntityType::Problem,
                "Solution" => EntityType::Solution,
                "Decision" => EntityType::Decision,
                "CodeComponent" => EntityType::CodeComponent,
                _ => EntityType::Concept,
            },
            first_mentioned: self.first_mentioned,
            last_mentioned: self.last_mentioned,
            mention_count: self.mention_count,
            importance_score: self.importance_score,
            description: self.description.clone(),
        }
    }
}

/// Stored relationship record for RocksDB persistence
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredRelationship {
    /// Session the relationship belongs to.
    pub session_id: Uuid,
    /// Source entity name.
    pub from_entity: String,
    /// Target entity name.
    pub to_entity: String,
    /// Relation type, stored as its `Debug` representation.
    pub relation_type: String,
    /// Free-form context for the relationship.
    pub context: String,
}

impl StoredRelationship {
    /// Build a `StoredRelationship` from an in-memory `EntityRelationship`.
    pub fn from_relationship(session_id: Uuid, rel: &EntityRelationship) -> Self {
        Self {
            session_id,
            from_entity: rel.from_entity.clone(),
            to_entity: rel.to_entity.clone(),
            relation_type: format!("{:?}", rel.relation_type),
            context: rel.context.clone(),
        }
    }

    /// Convert this stored record back into an in-memory `EntityRelationship`.
    pub fn to_relationship(&self) -> EntityRelationship {
        EntityRelationship {
            from_entity: self.from_entity.clone(),
            to_entity: self.to_entity.clone(),
            relation_type: self
                .relation_type
                .parse()
                .unwrap_or(RelationType::RelatedTo),
            context: self.context.clone(),
        }
    }
}

/// Stored embedding record for RocksDB persistence
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredEmbedding {
    /// Stable content ID this embedding is keyed by.
    pub content_id: String,
    /// Session that produced the embedding.
    pub session_id: String,
    /// Embedding vector.
    pub vector: Vec<f32>,
    /// Source text that was embedded.
    pub text: String,
    /// Content type tag (e.g. `"message"`, `"summary"`).
    pub content_type: String,
    /// Timestamp of the embedded content.
    pub timestamp: DateTime<Utc>,
    /// Free-form key/value metadata attached to the embedding.
    pub metadata: HashMap<String, String>,
}

impl StoredEmbedding {
    /// Create from vector and metadata
    pub fn new(vector: Vec<f32>, metadata: VectorMetadata) -> Self {
        Self {
            content_id: metadata.id,
            session_id: metadata.source,
            vector,
            text: metadata.text,
            content_type: metadata.content_type,
            timestamp: metadata.timestamp,
            metadata: metadata.metadata,
        }
    }

    /// Convert to VectorMetadata
    pub fn to_metadata(&self) -> VectorMetadata {
        VectorMetadata {
            id: self.content_id.clone(),
            text: self.text.clone(),
            source: self.session_id.clone(),
            content_type: self.content_type.clone(),
            timestamp: self.timestamp,
            metadata: self.metadata.clone(),
        }
    }
}

/// Point-in-time snapshot of a session, used for restore and analysis.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionCheckpoint {
    /// Checkpoint ID.
    pub id: Uuid,
    /// Session this checkpoint was taken from.
    pub session_id: Uuid,
    /// Timestamp the checkpoint was created.
    pub created_at: chrono::DateTime<chrono::Utc>,

    // Complete context snapshot
    /// Snapshot of the session's structured context at checkpoint time.
    pub structured_context: StructuredContext,
    /// Recent context updates included in the snapshot.
    pub recent_updates: Vec<ContextUpdate>,
    /// Code references grouped by file path.
    pub code_references: HashMap<String, Vec<CodeReference>>,
    /// Change history records included in the snapshot.
    pub change_history: Vec<ChangeRecord>,

    // Metadata
    /// Total number of updates the session contained at checkpoint time.
    pub total_updates: usize,
    /// Computed context-quality score in `[0.0, 1.0]`.
    pub context_quality_score: f32,
    /// Compression ratio achieved by the snapshot.
    pub compression_ratio: f32,
}
