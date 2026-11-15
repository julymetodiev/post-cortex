// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Tests of the session value.

mod codec;
mod mutate;
mod views;

use super::*;
use crate::core::context_update::{
    CodeReference, EntityRelationship, RelationType, UpdateContent, UpdateType,
};
use chrono::TimeZone;

pub(super) fn prefs(max_hot: usize) -> UserPreferences {
    UserPreferences {
        max_hot_context_size: max_hot,
        ..UserPreferences::default()
    }
}

pub(super) fn session_with(prefs: UserPreferences) -> ActiveSession {
    let created = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
    ActiveSession::from_records(
        SessionMetadata::new(Uuid::new_v4(), Some("s".into()), None, prefs, created),
        created,
        Vec::new(),
        SimpleEntityGraph::new(),
    )
}

pub(super) fn update(i: u32, update_type: UpdateType, with_code: bool) -> ContextUpdate {
    ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        timestamp: Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, i % 60).unwrap()
            + chrono::Duration::minutes(i64::from(i / 60)),
        update_type,
        content: UpdateContent {
            title: format!("title {i}"),
            description: format!("description {i}"),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        related_code: with_code.then(|| CodeReference {
            file_path: "src/lib.rs".into(),
            start_line: i,
            end_line: i + 1,
            code_snippet: format!("fn f{i}() {{}}"),
            commit_hash: None,
            branch: None,
            change_description: format!("change {i}"),
        }),
        parent_update: None,
        user_marked_important: false,
        creates_entities: vec![format!("e{i}")],
        creates_relationships: vec![EntityRelationship {
            from_entity: format!("e{i}"),
            to_entity: "hub".into(),
            relation_type: RelationType::DependsOn,
            context: String::new(),
        }],
        references_entities: vec![],
        typed_entities: vec![],
        author: None,
        derived_from: Vec::new(),
    }
}

/// Five records of different types (every other with code), hot window 3.
pub(super) fn populated_session() -> ActiveSession {
    let mut s = session_with(prefs(3));
    let types = [
        UpdateType::QuestionAnswered,
        UpdateType::DecisionMade,
        UpdateType::ConceptDefined,
        UpdateType::RequirementAdded,
        UpdateType::CodeChanged,
    ];
    for (i, t) in types.iter().enumerate() {
        s.apply_record(update(i as u32, *t, i % 2 == 0));
    }
    s
}
