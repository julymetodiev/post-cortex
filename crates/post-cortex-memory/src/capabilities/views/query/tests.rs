// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use post_cortex_core::core::context_update::UpdateType;
use post_cortex_core::core::context_update::{EntityRelationship, RelationType, UpdateContent};
use uuid::Uuid;

fn update(update_type: UpdateType, title: &str) -> ContextUpdate {
    ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        timestamp: chrono::Utc::now(),
        update_type,
        content: UpdateContent {
            title: title.to_string(),
            description: String::new(),
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
        typed_entities: vec![],
        author: None,
        derived_from: Vec::new(),
    }
}

#[test]
fn decisions_cover_the_full_log_not_just_hot_context() {
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    // More than the 50-entry hot tier.
    for i in 0..60 {
        session.apply_record(update(UpdateType::DecisionMade, &format!("d{i}")));
    }
    session.apply_record(update(UpdateType::CodeChanged, "c"));

    match answer(
        &session,
        ContextQuery::GetDecisions { since: None },
        chrono::Utc::now(),
    ) {
        ContextResponse::Decisions(d) => assert_eq!(d.len(), 60),
        other => panic!("unexpected response: {other:?}"),
    }
    match answer(
        &session,
        ContextQuery::SearchUpdates { query: "D0".into() },
        chrono::Utc::now(),
    ) {
        // Case-insensitive; "d0" is the oldest update, long out of hot.
        ContextResponse::SearchResults(r) => assert_eq!(r.len(), 1),
        other => panic!("unexpected response: {other:?}"),
    }
}

/// Questions come from Q&A records, which are answered: none is open.
#[test]
fn answered_questions_are_not_open() {
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    session.apply_record(update(UpdateType::QuestionAnswered, "done?"));
    assert_eq!(session.current_state().answered_questions.len(), 1);

    match answer(&session, ContextQuery::GetOpenQuestions, chrono::Utc::now()) {
        ContextResponse::OpenQuestions(q) => assert!(q.is_empty(), "{q:?}"),
        other => panic!("unexpected response: {other:?}"),
    }
}

#[test]
fn network_and_importance_return_real_data() {
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    session.entity_graph_mut().add_relationship(
        EntityRelationship {
            from_entity: "a".into(),
            to_entity: "b".into(),
            relation_type: RelationType::RelatedTo,
            context: String::new(),
        },
        chrono::Utc::now(),
    );

    match answer(
        &session,
        ContextQuery::GetEntityNetwork {
            center_entity: "a".into(),
            max_depth: 2,
        },
        chrono::Utc::now(),
    ) {
        ContextResponse::EntityNetwork(n) => {
            assert_eq!(n.center, "a");
            assert!(n.entities.contains_key("b"));
            assert_eq!(n.relationships.len(), 1);
        }
        other => panic!("unexpected response: {other:?}"),
    }

    match answer(
        &session,
        ContextQuery::AnalyzeEntityImportance,
        chrono::Utc::now(),
    ) {
        ContextResponse::ImportanceAnalysis(a) => assert_eq!(a.len(), 2),
        other => panic!("unexpected response: {other:?}"),
    }
}

/// A query whose required text is empty is refused, with the field named
/// (audit 2026-10-22: the transport passed "" and the query ran on it).
#[test]
fn queries_name_their_missing_text() {
    assert_eq!(
        missing_field(&ContextQuery::GetEntityContext {
            entity_name: " ".into()
        }),
        Some("entity_name")
    );
    assert_eq!(
        missing_field(&ContextQuery::FindCodeReferences {
            file_path: String::new()
        }),
        Some("file_path")
    );
    assert_eq!(
        missing_field(&ContextQuery::GetEntityContext {
            entity_name: "a".into()
        }),
        None
    );
    assert_eq!(missing_field(&ContextQuery::GetOpenQuestions), None);
}
