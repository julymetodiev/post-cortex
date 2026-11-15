// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use chrono::Utc;
use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
};
use uuid::Uuid;

pub(crate) fn entity(name: &str) -> EntityData {
    EntityData {
        aliases: Vec::new(),
        name: name.to_string(),
        entity_type: EntityType::Concept,
        first_mentioned: Utc::now(),
        last_mentioned: Utc::now(),
        mention_count: 1,
        importance_score: 1.0,
        description: None,
    }
}

pub(crate) fn relation(from: &str, to: &str) -> EntityRelationship {
    EntityRelationship {
        from_entity: from.to_string(),
        to_entity: to.to_string(),
        relation_type: RelationType::RelatedTo,
        context: "test relation".to_string(),
    }
}

/// A valid write of a concept to a random session.
pub(crate) fn good_request() -> UpdateContextRequest {
    UpdateContextRequest {
        meta: Default::default(),
        session_id: Uuid::new_v4(),
        interaction_type: UpdateType::ConceptDefined,
        content: UpdateContent {
            title: "Some concept".into(),
            description: "A short definition".into(),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        entities: vec![entity("Foo"), entity("Bar")],
        relations: vec![relation("Foo", "Bar")],
        code_reference: None,
        author: None,
        derived_from: Vec::new(),
    }
}

fn rejected(req: &UpdateContextRequest, part: &str) -> bool {
    matches!(request(req), Err(RecordError::InvalidArgument(ref m)) if m.contains(part))
}

#[test]
fn validation_rejects_empty_title_and_description() {
    let mut req = good_request();
    req.content.title = String::new();
    req.content.description = String::new();
    assert!(rejected(&req, "both empty"));
}

#[test]
fn validation_rejects_empty_entities() {
    let mut req = good_request();
    req.entities = vec![];
    assert!(rejected(&req, "entities must not be empty"));
}

#[test]
fn validation_rejects_empty_relations() {
    let mut req = good_request();
    req.relations = vec![];
    assert!(rejected(&req, "relations must not be empty"));
}

#[test]
fn validation_rejects_self_relation() {
    let mut req = good_request();
    req.relations = vec![relation("Foo", "Foo")];
    assert!(rejected(&req, "self-relations"));
}

#[test]
fn validation_rejects_dangling_relation_endpoint() {
    let mut req = good_request();
    req.relations = vec![relation("Foo", "Ghost")];
    assert!(rejected(&req, "Ghost"));
}

#[test]
fn validation_rejects_empty_relation_context() {
    let mut req = good_request();
    req.relations[0].context = "   ".into();
    assert!(rejected(&req, "context must not be empty"));
}

#[test]
fn validation_accepts_good_request() {
    assert!(request(&good_request()).is_ok());
}

/// The record a write becomes is the request's content as given: title,
/// description, type, author; entity names and typed entities in sync.
#[test]
fn the_record_is_the_request() {
    let mut req = good_request();
    req.author = Some("claude".into());
    let update = record(req.clone());
    assert_eq!(update.content.title, "Some concept");
    assert_eq!(update.content.description, "A short definition");
    assert_eq!(update.update_type, UpdateType::ConceptDefined);
    assert_eq!(update.author.as_deref(), Some("claude"));
    let names: Vec<&str> = update.creates_entities.iter().map(String::as_str).collect();
    let typed: Vec<&str> = update
        .typed_entities
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(names, ["Foo", "Bar"]);
    assert_eq!(typed, names);
    assert_eq!(update.creates_relationships.len(), 1);
}
