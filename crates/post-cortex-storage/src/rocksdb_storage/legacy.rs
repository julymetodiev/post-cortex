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

//! Bincode layouts written before `ContextUpdate::author`,
//! `ContextUpdate::derived_from` and `ContextUpdate::meta` existed.
//!
//! Bincode is not self-describing: a struct with a new field cannot read
//! rows written without it. Update rows end with the update, so a newer
//! layout fails cleanly on an older row (the input ends before the new
//! field) and [`decode_update`] falls back layout by layout: current, then
//! [`DerivedContextUpdate`], [`AuthoredContextUpdate`] and
//! [`LegacyContextUpdate`]. Checkpoints hold a list of updates followed by
//! more fields, where a misread would not fail cleanly, so each layout has
//! its marker: [`CHECKPOINT_V4`] for the current one, [`CHECKPOINT_V3`]
//! for derived, [`CHECKPOINT_V2`] for authored updates, none for legacy
//! ones.
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
pub(super) const CHECKPOINT_V4: &[u8] = b"pcx-ckpt-v4\0";

/// Marks a checkpoint written with [`DerivedContextUpdate`]s.
const CHECKPOINT_V3: &[u8] = b"pcx-ckpt-v3\0";

/// Marks a checkpoint written with [`AuthoredContextUpdate`]s.
const CHECKPOINT_V2: &[u8] = b"pcx-ckpt-v2\0";

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
            derived_from: Vec::new(),
            meta: Default::default(),
        }
    }
}

/// `ContextUpdate` with `author`, before `derived_from`. Bincode writes a
/// struct as its fields in order, so this is the legacy fields followed by
/// `author`.
#[derive(Deserialize)]
struct AuthoredContextUpdate(LegacyContextUpdate, Option<String>);

impl From<AuthoredContextUpdate> for ContextUpdate {
    fn from(AuthoredContextUpdate(update, author): AuthoredContextUpdate) -> Self {
        Self {
            author,
            ..update.into()
        }
    }
}

/// `ContextUpdate` with `author` and `derived_from`, before `meta`.
#[derive(Deserialize)]
struct DerivedContextUpdate(LegacyContextUpdate, Option<String>, Vec<String>);

impl From<DerivedContextUpdate> for ContextUpdate {
    fn from(DerivedContextUpdate(update, author, derived_from): DerivedContextUpdate) -> Self {
        Self {
            author,
            derived_from,
            ..update.into()
        }
    }
}

/// `SessionCheckpoint` holding updates in an older layout `U`.
#[derive(Deserialize)]
struct LegacySessionCheckpoint<U> {
    id: Uuid,
    session_id: Uuid,
    created_at: DateTime<Utc>,
    structured_context: StructuredContext,
    recent_updates: Vec<U>,
    code_references: HashMap<String, Vec<CodeReference>>,
    change_history: Vec<ChangeRecord>,
    total_updates: usize,
    context_quality_score: f32,
    compression_ratio: f32,
}

impl<U: Into<ContextUpdate>> From<LegacySessionCheckpoint<U>> for SessionCheckpoint {
    fn from(c: LegacySessionCheckpoint<U>) -> Self {
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

/// An update row in the current layout, else in the layout before `meta`,
/// before `derived_from`, before `author`.
pub(super) fn decode_update(bytes: &[u8]) -> Option<ContextUpdate> {
    bincode_decode::<ContextUpdate>(bytes)
        .or_else(|| bincode_decode::<DerivedContextUpdate>(bytes).map(Into::into))
        .or_else(|| bincode_decode::<AuthoredContextUpdate>(bytes).map(Into::into))
        .or_else(|| bincode_decode::<LegacyContextUpdate>(bytes).map(Into::into))
}

/// A checkpoint in the layout its marker names; unmarked ones are from
/// before `author`.
pub(super) fn decode_checkpoint(bytes: &[u8]) -> Option<SessionCheckpoint> {
    if let Some(rest) = bytes.strip_prefix(CHECKPOINT_V4) {
        bincode_decode::<SessionCheckpoint>(rest)
    } else if let Some(rest) = bytes.strip_prefix(CHECKPOINT_V3) {
        bincode_decode::<LegacySessionCheckpoint<DerivedContextUpdate>>(rest).map(Into::into)
    } else if let Some(rest) = bytes.strip_prefix(CHECKPOINT_V2) {
        bincode_decode::<LegacySessionCheckpoint<AuthoredContextUpdate>>(rest).map(Into::into)
    } else {
        bincode_decode::<LegacySessionCheckpoint<LegacyContextUpdate>>(bytes).map(Into::into)
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

        let authored = encode(&(
            OldUpdate {
                id,
                timestamp: Utc::now(),
                update_type: UpdateType::DecisionMade,
                content: &content,
                related_code: None,
                parent_update: None,
                user_marked_important: false,
                creates_entities: Vec::new(),
                creates_relationships: Vec::new(),
                references_entities: Vec::new(),
                typed_entities: Vec::new(),
            },
            Some("hermes:eq-lead"),
        ));
        let update = decode_update(&authored).expect("authored row");
        assert_eq!(update.author.as_deref(), Some("hermes:eq-lead"));
        assert!(update.derived_from.is_empty());

        let derived = encode(&(
            OldUpdate {
                id,
                timestamp: Utc::now(),
                update_type: UpdateType::DecisionMade,
                content: &content,
                related_code: None,
                parent_update: None,
                user_marked_important: false,
                creates_entities: Vec::new(),
                creates_relationships: Vec::new(),
                references_entities: Vec::new(),
                typed_entities: Vec::new(),
            },
            Some("claude-code"),
            vec!["message:3"],
        ));
        let update = decode_update(&derived).expect("derived row");
        assert_eq!(update.author.as_deref(), Some("claude-code"));
        assert_eq!(update.derived_from, ["message:3"]);
        assert_eq!(update.meta, Default::default());

        let mut new = update.clone();
        new.meta.supersedes = vec![Uuid::new_v4()];
        new.meta.confidence = Some(0.5);
        let decoded = decode_update(&encode(&new)).expect("current row");
        assert_eq!(decoded.derived_from, ["message:3"]);
        assert_eq!(decoded.meta, new.meta);
    }

    /// Mirror of the checkpoint layout around updates of type `U`.
    #[derive(Serialize)]
    struct OldCheckpoint<U> {
        id: Uuid,
        session_id: Uuid,
        created_at: DateTime<Utc>,
        structured_context: StructuredContext,
        recent_updates: Vec<U>,
        code_references: HashMap<String, Vec<CodeReference>>,
        change_history: Vec<ChangeRecord>,
        total_updates: usize,
        context_quality_score: f32,
        compression_ratio: f32,
    }

    fn checkpoint<U>(recent_updates: Vec<U>) -> OldCheckpoint<U> {
        OldCheckpoint {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            created_at: Utc::now(),
            structured_context: StructuredContext::new(),
            recent_updates,
            code_references: HashMap::new(),
            change_history: Vec::new(),
            total_updates: 9,
            context_quality_score: 0.5,
            compression_ratio: 2.0,
        }
    }

    /// Each checkpoint layout is read by its marker, and the fields after
    /// the updates come through intact.
    #[test]
    fn checkpoints_of_every_layout_decode() {
        let content = content();
        let old = || OldUpdate {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            update_type: UpdateType::DecisionMade,
            content: &content,
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: Vec::new(),
            creates_relationships: Vec::new(),
            references_entities: Vec::new(),
            typed_entities: Vec::new(),
        };

        let legacy = decode_checkpoint(&encode(&checkpoint(vec![old(), old()]))).unwrap();
        assert_eq!(legacy.recent_updates.len(), 2);
        assert_eq!(legacy.total_updates, 9);

        let mut v2 = CHECKPOINT_V2.to_vec();
        v2.extend(encode(&checkpoint(vec![(old(), Some("alice"))])));
        let authored = decode_checkpoint(&v2).unwrap();
        assert_eq!(authored.recent_updates[0].author.as_deref(), Some("alice"));
        assert_eq!(authored.compression_ratio, 2.0);

        let mut v3 = CHECKPOINT_V3.to_vec();
        v3.extend(encode(&checkpoint(vec![(
            old(),
            Some("bob"),
            vec!["message:1"],
        )])));
        let derived = decode_checkpoint(&v3).unwrap();
        assert_eq!(derived.recent_updates[0].derived_from, ["message:1"]);
        assert_eq!(derived.total_updates, 9);

        let mut update = derived.recent_updates[0].clone();
        update.meta.retired_at = Some(Utc::now());
        let mut v4 = CHECKPOINT_V4.to_vec();
        v4.extend(encode(&checkpoint(vec![update.clone()])));
        let current = decode_checkpoint(&v4).unwrap();
        assert_eq!(current.recent_updates[0].meta, update.meta);
        assert_eq!(current.compression_ratio, 2.0);
    }
}
