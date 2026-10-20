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

//! Bincode layouts written before `ContextUpdate::author` existed.
//!
//! Bincode is not self-describing: a struct with a new field cannot read
//! rows written without it. Update rows end with the update, so the current
//! layout fails cleanly on an old row (the input ends before `author`) and
//! [`decode_update`] falls back to [`LegacyContextUpdate`]. Checkpoints
//! hold a list of updates followed by more fields, where a misread would
//! not fail cleanly, so new checkpoints carry [`CHECKPOINT_V2`] in front
//! and unmarked ones are read as [`LegacySessionCheckpoint`].
//!
//! These structs are frozen: they must keep the field order and types of
//! the old layout.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use post_cortex_core::core::context_update::{
    CodeReference as UpdateCodeReference, ContextUpdate, EntityRelationship, TypedEntity,
    UpdateContent, UpdateType,
};
use post_cortex_core::core::structured_context::StructuredContext;
use post_cortex_core::session::active_session::{ChangeRecord, CodeReference};

use super::types::SessionCheckpoint;

/// Marks a checkpoint written with the current `ContextUpdate` layout.
pub(super) const CHECKPOINT_V2: &[u8] = b"pcx-ckpt-v2\0";

/// `ContextUpdate` before `author`.
#[derive(Deserialize)]
struct LegacyContextUpdate {
    id: Uuid,
    timestamp: DateTime<Utc>,
    update_type: UpdateType,
    content: UpdateContent,
    related_code: Option<UpdateCodeReference>,
    parent_update: Option<Uuid>,
    user_marked_important: bool,
    creates_entities: Vec<String>,
    creates_relationships: Vec<EntityRelationship>,
    references_entities: Vec<String>,
    typed_entities: Vec<TypedEntity>,
}

impl From<LegacyContextUpdate> for ContextUpdate {
    fn from(u: LegacyContextUpdate) -> Self {
        Self {
            id: u.id,
            timestamp: u.timestamp,
            update_type: u.update_type,
            content: u.content,
            related_code: u.related_code,
            parent_update: u.parent_update,
            user_marked_important: u.user_marked_important,
            creates_entities: u.creates_entities,
            creates_relationships: u.creates_relationships,
            references_entities: u.references_entities,
            typed_entities: u.typed_entities,
            author: None,
        }
    }
}

/// `SessionCheckpoint` holding legacy updates.
#[derive(Deserialize)]
struct LegacySessionCheckpoint {
    id: Uuid,
    session_id: Uuid,
    created_at: DateTime<Utc>,
    structured_context: StructuredContext,
    recent_updates: Vec<LegacyContextUpdate>,
    code_references: HashMap<String, Vec<CodeReference>>,
    change_history: Vec<ChangeRecord>,
    total_updates: usize,
    context_quality_score: f32,
    compression_ratio: f32,
}

impl From<LegacySessionCheckpoint> for SessionCheckpoint {
    fn from(c: LegacySessionCheckpoint) -> Self {
        Self {
            id: c.id,
            session_id: c.session_id,
            created_at: c.created_at,
            structured_context: c.structured_context,
            recent_updates: c.recent_updates.into_iter().map(Into::into).collect(),
            code_references: c.code_references,
            change_history: c.change_history,
            total_updates: c.total_updates,
            context_quality_score: c.context_quality_score,
            compression_ratio: c.compression_ratio,
        }
    }
}

fn bincode_decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Option<T> {
    bincode::serde::decode_from_slice::<T, _>(bytes, bincode::config::standard())
        .ok()
        .map(|(v, _)| v)
}

/// An update row in the current layout, else in the layout before `author`.
pub(super) fn decode_update(bytes: &[u8]) -> Option<ContextUpdate> {
    bincode_decode::<ContextUpdate>(bytes)
        .or_else(|| bincode_decode::<LegacyContextUpdate>(bytes).map(Into::into))
}

/// A checkpoint: marked ones in the current layout, unmarked ones in the
/// layout before `author`.
pub(super) fn decode_checkpoint(bytes: &[u8]) -> Option<SessionCheckpoint> {
    match bytes.strip_prefix(CHECKPOINT_V2) {
        Some(rest) => bincode_decode::<SessionCheckpoint>(rest),
        None => bincode_decode::<LegacySessionCheckpoint>(bytes).map(Into::into),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    /// Mirror of the legacy layout for writing old rows in tests.
    #[derive(Serialize)]
    struct OldUpdate<'a> {
        id: Uuid,
        timestamp: DateTime<Utc>,
        update_type: UpdateType,
        content: &'a UpdateContent,
        related_code: Option<UpdateCodeReference>,
        parent_update: Option<Uuid>,
        user_marked_important: bool,
        creates_entities: Vec<String>,
        creates_relationships: Vec<EntityRelationship>,
        references_entities: Vec<String>,
        typed_entities: Vec<TypedEntity>,
    }

    fn content() -> UpdateContent {
        UpdateContent {
            title: "decided".to_string(),
            description: "use rocksdb".to_string(),
            details: vec!["d".to_string()],
            examples: Vec::new(),
            implications: Vec::new(),
        }
    }

    fn encode<T: Serialize>(v: &T) -> Vec<u8> {
        bincode::serde::encode_to_vec(v, bincode::config::standard()).unwrap()
    }

    #[test]
    fn old_update_rows_still_decode() {
        let content = content();
        let id = Uuid::new_v4();
        let old = encode(&OldUpdate {
            id,
            timestamp: Utc::now(),
            update_type: UpdateType::DecisionMade,
            content: &content,
            related_code: None,
            parent_update: None,
            user_marked_important: true,
            creates_entities: vec!["RocksDB".to_string()],
            creates_relationships: Vec::new(),
            references_entities: Vec::new(),
            typed_entities: Vec::new(),
        });
        let update = decode_update(&old).expect("legacy row");
        assert_eq!(update.id, id);
        assert_eq!(update.content.description, "use rocksdb");
        assert!(update.user_marked_important);
        assert_eq!(update.author, None);

        let mut new = update.clone();
        new.author = Some("claude-code".to_string());
        let decoded = decode_update(&encode(&new)).expect("current row");
        assert_eq!(decoded.author.as_deref(), Some("claude-code"));
    }
}
