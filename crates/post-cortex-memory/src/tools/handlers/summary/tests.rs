// Copyright (c) 2025, 2026 Julius ML
// MIT License

use super::*;
use pcx_module_api::ToolOutput;
use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::services::UpdateContextRequest;
use std::sync::Arc;

async fn memory_with_a_decision() -> (Arc<Memory>, uuid::Uuid, tempfile::TempDir) {
    let (memory, dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let memory = Arc::new(memory);
    let sid = memory.sessions.create(None, None).await.unwrap();
    let entity = |name: &str| EntityData {
        aliases: Vec::new(),
        name: name.into(),
        entity_type: EntityType::Concept,
        first_mentioned: chrono::Utc::now(),
        last_mentioned: chrono::Utc::now(),
        mention_count: 1,
        importance_score: 1.0,
        description: None,
    };
    memory
        .records
        .write(UpdateContextRequest {
            meta: Default::default(),
            session_id: sid,
            interaction_type: UpdateType::DecisionMade,
            content: UpdateContent {
                title: "Use cards".into(),
                description: "always in front".into(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            entities: vec![entity("cards"), entity("sessions")],
            relations: vec![EntityRelationship {
                from_entity: "cards".into(),
                to_entity: "sessions".into(),
                relation_type: RelationType::RelatedTo,
                context: "injected".into(),
            }],
            code_reference: None,
            author: None,
            derived_from: Vec::new(),
        })
        .await
        .unwrap();
    (memory, sid, dir)
}

fn data(reply: ToolOutput) -> serde_json::Value {
    assert!(!reply.failed);
    reply.data.expect("data")
}

/// A section asked for with `include` keeps its data, with its own limit.
#[tokio::test]
async fn included_sections_keep_their_data() {
    let (memory, sid, _dir) = memory_with_a_decision().await;
    let body = serde_json::json!({
        "session_id": sid.to_string(),
        "include": ["decisions", "entities", "questions"],
        "entities_limit": 1,
    });

    let data = data(handle(&memory, body).await.unwrap());

    let decisions = data["key_decisions"].as_array().expect("decisions data");
    assert_eq!(decisions.len(), 1, "{data}");
    assert_eq!(
        data["entity_summaries"].as_array().map(Vec::len),
        Some(1),
        "{data}"
    );
    assert!(data["open_questions"].is_array(), "{data}");
    assert!(data.get("key_insights").is_none(), "not asked for: {data}");
}
