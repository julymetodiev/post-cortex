// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Work derived from a write, on every backend: the entity-graph merge is
//! part of the write (one persist carries it), and a full derived-work
//! queue makes writers wait instead of spawning a task per item.

use std::time::Duration;

use uuid::Uuid;

use post_cortex_core::core::context_update::{
    ContextUpdate, EntityRelationship, EntityType, RelationType, TypedEntity, UpdateContent,
    UpdateType,
};

use super::SystemConfig;
use super::backends_tests::{backends, request};
use crate::pipeline::PipelineConfig;

/// A record that creates `Foo` and `Bar`.
fn metadata(title: &str) -> ContextUpdate {
    let entity = |name: &str| TypedEntity {
        name: name.into(),
        entity_type: EntityType::Concept,
    };
    ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        timestamp: chrono::Utc::now(),
        update_type: UpdateType::ConceptDefined,
        content: UpdateContent {
            title: title.into(),
            description: format!("{title}: long enough to be a record"),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: vec!["Foo".into(), "Bar".into()],
        creates_relationships: vec![EntityRelationship {
            from_entity: "Foo".into(),
            to_entity: "Bar".into(),
            relation_type: RelationType::RelatedTo,
            context: "test".into(),
        }],
        references_entities: vec![],
        typed_entities: vec![entity("Foo"), entity("Bar")],
        author: None,
        derived_from: Vec::new(),
    }
}

/// The stored session has the write's entities as soon as the durable write
/// returns: no later graph job (and no second blob write) is needed.
#[tokio::test]
async fn a_durable_write_stores_its_graph_merge() {
    for backend in backends().await {
        let memory = backend.start().await;
        let sid = memory.mem.sessions.create(None, None).await.unwrap();

        let committed = memory
            .mem
            .records
            .commit(sid, metadata("one"), true)
            .await
            .unwrap();
        assert!(committed.durable, "{}", backend.name);

        let stored = memory
            .session_manager
            .load_stored(sid)
            .await
            .unwrap()
            .unwrap()
            .session;
        let foo = stored.entity_graph().entities.get("Foo");
        assert_eq!(foo.map(|e| e.mention_count), Some(1), "{}", backend.name);
        assert_eq!(
            memory.derived_work.pending(),
            0,
            "{}: no graph job",
            backend.name
        );
    }
}

/// With a derived-work queue of one, many writes wait for room instead of
/// spawning a task per item: the work in flight stays bounded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_queue_makes_writers_wait_instead_of_spawning() {
    const CAPACITY: usize = 1;
    for backend in backends().await {
        let memory = backend
            .start_with(SystemConfig {
                enable_embeddings: true,
                ..Default::default()
            })
            .await;
        let service = crate::capabilities::test_support::memory_with(
            &memory.rt,
            PipelineConfig {
                embedding_capacity: CAPACITY,
                ..Default::default()
            },
        );
        let sid = memory.mem.sessions.create(None, None).await.unwrap();

        let mut most = 0;
        for i in 0..12 {
            service
                .records
                .write(request(sid, &format!("Record {i}"), "about the lighthouse"))
                .await
                .unwrap();
            most = most.max(memory.derived_work.pending());
        }
        // The queued items plus the one in the worker (counted by its queue
        // slot and by the embedding it runs), at most.
        assert!(
            most <= CAPACITY + 2,
            "{}: {most} derived jobs in flight with a queue of {CAPACITY}",
            backend.name
        );
        assert_eq!(
            memory.flush_all(Duration::from_secs(60)).await,
            0,
            "{}",
            backend.name
        );
        let handle = memory.get_session(sid).await.unwrap();
        assert_eq!(
            handle.vectorized().pending_count(&handle.load()),
            0,
            "{}: every record embedded",
            backend.name
        );
    }
}
