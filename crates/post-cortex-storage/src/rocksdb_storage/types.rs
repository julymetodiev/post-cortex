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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredWorkspace {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub sessions: Vec<(Uuid, SessionRole)>,
    pub created_at: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredWorkspaceSession {
    pub workspace_id: Uuid,
    pub session_id: Uuid,
    pub role: SessionRole,
    pub added_at: u64,
}

/// Stored entity record for RocksDB persistence
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredEntity {
    pub session_id: Uuid,
    pub name: String,
    pub entity_type: String,
    pub first_mentioned: DateTime<Utc>,
    pub last_mentioned: DateTime<Utc>,
    pub mention_count: u32,
    pub importance_score: f32,
    pub description: Option<String>,
}

impl StoredEntity {
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
    pub session_id: Uuid,
    pub from_entity: String,
    pub to_entity: String,
    pub relation_type: String,
    pub context: String,
}

impl StoredRelationship {
    pub fn from_relationship(session_id: Uuid, rel: &EntityRelationship) -> Self {
        Self {
            session_id,
            from_entity: rel.from_entity.clone(),
            to_entity: rel.to_entity.clone(),
            relation_type: format!("{:?}", rel.relation_type),
            context: rel.context.clone(),
        }
    }

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
    pub content_id: String,
    pub session_id: String,
    pub vector: Vec<f32>,
    pub text: String,
    pub content_type: String,
    pub timestamp: DateTime<Utc>,
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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionCheckpoint {
    pub id: Uuid,
    pub session_id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,

    // Complete context snapshot
    pub structured_context: StructuredContext,
    pub recent_updates: Vec<ContextUpdate>,
    pub code_references: HashMap<String, Vec<CodeReference>>,
    pub change_history: Vec<ChangeRecord>,

    // Metadata
    pub total_updates: usize,
    pub context_quality_score: f32,
    pub compression_ratio: f32,
}
