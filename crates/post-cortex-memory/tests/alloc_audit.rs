// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The allocation audit of the write and search paths (ARCHITECTURE
//! section 10, W8), over RocksDB, the in-memory index and the model-free
//! hash engine (so the numbers are the memory system's, not a model's):
//! a record write (with its derived embedding work) must cost the same in a
//! small and in a large session, since a save writes what it changes. Its
//! own test binary: dhat counts every allocation of the process.
//!
//! `cargo test -p post-cortex-memory --release --test alloc_audit -- --nocapture`
//! also prints the counts per write and per search, and writes the
//! allocation sites to `dhat-heap.json` in the temporary directory.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::services::UpdateContextRequest;
use post_cortex_embeddings::MemoryIndex;
use post_cortex_memory::capabilities::SearchScope;
use post_cortex_memory::content_vectorizer::SearchOptions;
use post_cortex_memory::{Memory, MemoryPorts, SystemConfig};
use post_cortex_ports::Stores;
use post_cortex_storage::{JsonArchive, RealRocksDBStorage};
use uuid::Uuid;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const DIMENSION: usize = 256;
/// Records of the small session when its writes are measured.
const SMALL: usize = 50;
/// Records of the large session when its writes are measured.
const LARGE: usize = 600;
/// Writes and searches measured.
const MEASURED: usize = 30;
/// How much more a write in the large session may cost than one in the
/// small session (it costs the same rows; the rest is noise).
const GROWTH: f64 = 1.25;

fn entity(name: &str) -> EntityData {
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

/// A decision of LoCoMo-turn length naming two entities.
fn request(session_id: Uuid, i: usize) -> UpdateContextRequest {
    let topic = ["storage", "search", "ranking", "capture", "export"][i % 5];
    UpdateContextRequest {
        meta: Default::default(),
        session_id,
        interaction_type: UpdateType::DecisionMade,
        content: UpdateContent {
            title: format!("Decision {i} about {topic}"),
            description: format!(
                "On day {} we decided how {topic} handles case {i}: keep the batch small, \
                 write once, and measure before changing it again.",
                i / 7
            ),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        entities: vec![entity(topic), entity(&format!("case {}", i % 40))],
        relations: vec![EntityRelationship {
            from_entity: topic.to_string(),
            to_entity: format!("case {}", i % 40),
            relation_type: RelationType::RelatedTo,
            context: "audit".into(),
        }],
        code_reference: None,
        author: None,
        derived_from: Vec::new(),
    }
}

/// Blocks and bytes allocated while `f` ran, per `n` operations.
async fn per_op<F, Fut>(n: usize, f: F) -> (u64, u64)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let before = dhat::HeapStats::get();
    f().await;
    let after = dhat::HeapStats::get();
    (
        (after.total_blocks - before.total_blocks) / n as u64,
        (after.total_bytes - before.total_bytes) / n as u64,
    )
}

/// Blocks and bytes per write of [`MEASURED`] records, numbered from
/// `first`, with their derived work.
async fn per_write(memory: &Memory, session: Uuid, first: usize) -> (u64, u64) {
    per_op(MEASURED, || async {
        for i in first..first + MEASURED {
            memory.records.write(request(session, i)).await.unwrap();
        }
        assert!(memory.records.wait_idle(Duration::from_secs(60)).await);
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_costs_the_same_in_a_small_and_a_large_session() {
    // The allocation sites go to `dhat-heap.json` in the temporary
    // directory (open it in dhat's viewer).
    let _profiler = dhat::Profiler::builder()
        .file_name(std::env::temp_dir().join("dhat-heap.json"))
        .build();
    let dir = tempfile::tempdir().unwrap();
    let rocks = Arc::new(RealRocksDBStorage::new(dir.path()).await.unwrap());
    let ports = MemoryPorts {
        stores: Stores::of(Arc::clone(&rocks)),
        index: Some(Arc::new(MemoryIndex::new(DIMENSION, Some(rocks)))),
        engine: Some(Arc::new(post_cortex_embeddings::test_support::hash_engine(
            DIMENSION,
        ))),
        reranker: None,
        accurate_reranker: None,
        archive: Arc::new(JsonArchive),
        blobs: None,
    };
    let config = SystemConfig {
        enable_embeddings: true,
        ..Default::default()
    };
    let memory = Memory::over_ports(ports, config).await.unwrap();
    let session = memory.sessions.create(None, None).await.unwrap();
    for i in 0..SMALL {
        memory.records.write(request(session, i)).await.unwrap();
    }
    assert!(memory.records.wait_idle(Duration::from_secs(60)).await);
    let small = per_write(&memory, session, SMALL).await;
    for i in SMALL + MEASURED..LARGE {
        memory.records.write(request(session, i)).await.unwrap();
    }
    assert!(memory.records.wait_idle(Duration::from_secs(60)).await);
    let large = per_write(&memory, session, LARGE).await;
    println!(
        "write (with its embedding): {} blocks, {} bytes at {SMALL} records; \
         {} blocks, {} bytes at {LARGE} records",
        small.0, small.1, large.0, large.1
    );
    assert!(
        (large.1 as f64) < small.1 as f64 * GROWTH,
        "a write costs {} bytes at {LARGE} records, {} at {SMALL}",
        large.1,
        small.1
    );

    for (name, scope) in [
        ("global", SearchScope::Global),
        ("session", SearchScope::Session(session)),
    ] {
        let (blocks, bytes) = per_op(MEASURED, || async {
            for i in 0..MEASURED {
                // A new query each time: the query cache must not answer.
                let query = format!("how does ranking handle case {i}");
                let hits = memory
                    .search
                    .search(scope.clone(), &query, SearchOptions::default())
                    .await
                    .unwrap();
                assert!(!hits.is_empty());
            }
        })
        .await;
        println!("search {name}: {blocks} blocks, {bytes} bytes per query");
    }
}
