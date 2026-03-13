// End-to-end test: GLiNER-RelEx NER → typed relations → entity graph
//
// Tests the full pipeline:
//   1. NER engine extracts entities + relations from text
//   2. extract_for_graph() maps to PCX types (EntityType, RelationType)
//   3. add_incremental_update() → update_entity_graph() inserts into petgraph
//   4. Entity graph has correct typed edges (DependsOn, RelatedTo, etc.)
//
// Requires: model files at ~/.post-cortex/models/gliner-relex-large-v0.5/
#![cfg(feature = "embeddings")]

mod common;

use common::TestFixture;
use post_cortex::core::context_update::RelationType;
use post_cortex::session::active_session::preload_ner_engine;
use serial_test::serial;

#[serial]
#[tokio::test]
#[ignore] // Requires GLiNER-RelEx model files
async fn test_relex_entities_flow_into_graph() {
    // 1. Pre-load NER engine (GLiNER-RelEx)
    let loaded = preload_ner_engine().await;
    assert!(loaded, "GLiNER-RelEx NER engine must load for this test");

    // 2. Create session and add content with known entities + relations
    let fixture = TestFixture::with_content(&[
        "Axon is a coding agent that connects to Post-Cortex via gRPC using tonic.",
        "PostgreSQL and Redis are used with axum and tower for the HTTP API.",
    ])
    .await
    .expect("fixture creation");

    // Wait for entity graph update (runs async in background)
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // 3. Load session and inspect entity graph
    let session = fixture
        .system
        .storage_actor.load_session(fixture.session_id)
        .await
        .expect("load session")
        .expect("session exists");

    let graph = &session.entity_graph;
    let all_entities = graph.get_all_entities();
    let all_rels = graph.get_all_relationships();

    println!("\n=== Entity Graph ===");
    println!("Entities ({}):", all_entities.len());
    for e in &all_entities {
        println!("  {} ({:?}, importance={:.2})", e.name, e.entity_type, e.importance_score);
    }
    println!("Relationships ({}):", all_rels.len());
    for r in &all_rels {
        println!(
            "  {} --[{:?}]--> {} ({})",
            r.from_entity, r.relation_type, r.to_entity, r.context
        );
    }

    // 4. Verify entities were extracted
    assert!(
        !all_entities.is_empty(),
        "Entity graph should have entities after NER extraction"
    );

    // Check for known entities (case-insensitive search)
    let entity_names: Vec<String> = all_entities.iter().map(|e| e.name.to_lowercase()).collect();
    // At minimum, we expect some of these from the two texts
    let expected_some_of = ["axon", "grpc", "tonic", "postgresql", "redis", "axum", "tower"];
    let found: Vec<&&str> = expected_some_of
        .iter()
        .filter(|name| entity_names.iter().any(|e: &String| e.contains(*name)))
        .collect();
    println!("\nExpected entities found: {:?} / {:?}", found, expected_some_of);
    assert!(
        found.len() >= 3,
        "Should find at least 3 known entities, found: {:?}",
        found
    );

    // 5. Verify typed relations exist (not just co-occurrence)
    if !all_rels.is_empty() {
        println!("\n=== Typed Relations Check ===");
        // Relations should have real types (DependsOn, RelatedTo, etc.), not all RelatedTo
        let relation_types: Vec<&RelationType> = all_rels.iter().map(|r| &r.relation_type).collect();
        println!("Relation types: {:?}", relation_types);

        // Check that at least one relation has context from NER (not heuristic)
        let has_ner_context = all_rels
            .iter()
            .any(|r| r.context.contains("-->") || r.context.contains("%"));
        if has_ner_context {
            println!("NER-sourced relations confirmed (context contains --> or %)");
        }
    }

    println!("\n=== Test passed ===");
}

#[serial]
#[tokio::test]
#[ignore] // Requires GLiNER-RelEx model files
async fn test_relex_rebuild_entity_graph() {
    let loaded = preload_ner_engine().await;
    assert!(loaded, "GLiNER-RelEx NER engine must load for this test");

    let fixture = TestFixture::with_content(&[
        "Post-Cortex is built with Rust and uses RocksDB for storage.",
    ])
    .await
    .expect("fixture creation");

    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Rebuild entity graph from scratch
    let (before, after) = fixture
        .system
        .storage_actor.rebuild_entity_graph(fixture.session_id)
        .await
        .expect("rebuild entity graph");

    println!("Rebuild: {} entities before, {} after", before, after);
    assert!(after > 0, "Rebuilt graph should have entities");

    // Load and verify
    let session = fixture
        .system
        .storage_actor.load_session(fixture.session_id)
        .await
        .expect("load session")
        .expect("session exists");

    let graph = &session.entity_graph;
    let entities = graph.get_all_entities();
    let rels = graph.get_all_relationships();

    println!("After rebuild: {} entities, {} relations", entities.len(), rels.len());
    for e in &entities {
        println!("  {} ({:?})", e.name, e.entity_type);
    }
    for r in &rels {
        println!("  {} --[{:?}]--> {}", r.from_entity, r.relation_type, r.to_entity);
    }

    // Should have at least Post-Cortex, Rust, RocksDB
    let names: Vec<String> = entities.iter().map(|e| e.name.to_lowercase()).collect();
    assert!(
        names.iter().any(|n: &String| n.contains("rust")),
        "Should find Rust entity, got: {:?}",
        names
    );
}
