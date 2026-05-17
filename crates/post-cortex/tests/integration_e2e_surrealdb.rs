#![allow(missing_docs)]
//! End-to-end test against a real SurrealDB + real Model2Vec embeddings.
//!
//! Drives the full canonical write path from an MCP-style typed
//! [`UpdateContextRequest`] through:
//!
//! 1. [`MemoryServiceImpl::update_context`] (validation, metadata,
//!    persistence)
//! 2. SurrealDB persist via the storage actor (`ws://truenas.local:8000`)
//! 3. The non-blocking [`Pipeline`] (embedding + graph + summary workers)
//! 4. Real `potion-multilingual-128M` vectorisation (HF Hub download
//!    cached on first run)
//! 5. Drained pipeline backlog + cleared per-session pending set
//!
//! Marked `#[ignore]` because it needs:
//!
//! - Network access to `ws://truenas.local:8000`
//! - HuggingFace Hub reachability (~50 MB Model2Vec checkpoint on first
//!   run)
//!
//! Run with:
//!
//! ```sh
//! cargo test -p post-cortex --features surrealdb-storage \
//!     --test integration_e2e_surrealdb -- --ignored --nocapture
//! ```

#![cfg(all(feature = "embeddings", feature = "surrealdb-storage"))]

use std::sync::Arc;

use post_cortex::{ConversationMemorySystem, SystemConfig};
use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::services::{PostCortexService, UpdateContextRequest};
use post_cortex_memory::services::MemoryServiceImpl;
use post_cortex_storage::traits::StorageBackendType;

fn entity(name: &str, kind: EntityType) -> EntityData {
    let now = chrono::Utc::now();
    EntityData {
        name: name.to_string(),
        entity_type: kind,
        first_mentioned: now,
        last_mentioned: now,
        mention_count: 1,
        importance_score: 1.0,
        description: None,
    }
}

fn relation(from: &str, to: &str, rel: RelationType, ctx: &str) -> EntityRelationship {
    EntityRelationship {
        from_entity: from.to_string(),
        to_entity: to.to_string(),
        relation_type: rel,
        context: ctx.to_string(),
    }
}

async fn drain_pipeline(svc: &MemoryServiceImpl, max_wait_ms: u64) {
    let step_ms = 50u64;
    let steps = max_wait_ms / step_ms;
    for _ in 0..steps {
        if svc.pipeline().backlog() == 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(step_ms)).await;
    }
}

fn init_tracing() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let filter = std::env::var("RUST_LOG")
            .unwrap_or_else(|_| "post_cortex_memory=debug,post_cortex_embeddings=debug".into());
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

#[tokio::test]
#[ignore = "requires ws://truenas.local:8000 SurrealDB + HF Hub network"]
async fn e2e_update_context_against_remote_surrealdb_and_real_embeddings() {
    init_tracing();
    // Endpoint / namespace / database / credentials all come from env
    // vars so the test stays runnable against any reachable SurrealDB
    // instance (defaults match the user's TrueNAS deployment):
    //   POST_CORTEX_E2E_SURREAL_URL=ws://truenas.local:8000
    //   POST_CORTEX_E2E_SURREAL_NS=post_cortex
    //   POST_CORTEX_E2E_SURREAL_DB=main
    //   POST_CORTEX_E2E_SURREAL_USER=root
    //   POST_CORTEX_E2E_SURREAL_PASS=…
    let endpoint = std::env::var("POST_CORTEX_E2E_SURREAL_URL")
        .unwrap_or_else(|_| "ws://truenas.local:8000".to_string());
    let namespace =
        std::env::var("POST_CORTEX_E2E_SURREAL_NS").unwrap_or_else(|_| "post_cortex".to_string());
    let database =
        std::env::var("POST_CORTEX_E2E_SURREAL_DB").unwrap_or_else(|_| "main".to_string());
    let username = std::env::var("POST_CORTEX_E2E_SURREAL_USER").ok();
    let password = std::env::var("POST_CORTEX_E2E_SURREAL_PASS").ok();

    let mut config = SystemConfig {
        storage_backend: StorageBackendType::SurrealDB,
        surrealdb_endpoint: Some(endpoint),
        surrealdb_username: username,
        surrealdb_password: password,
        surrealdb_namespace: Some(namespace),
        surrealdb_database: Some(database),
        ..Default::default()
    };
    // Explicit so the test is robust against future Default tweaks.
    config.enable_embeddings = true;
    config.auto_vectorize_on_update = true;
    config.embeddings_model_type = "PotionMultilingual".to_string();
    config.vector_dimension = 256;

    let system = Arc::new(
        ConversationMemorySystem::new(config)
            .await
            .expect("ConversationMemorySystem must build against the remote SurrealDB"),
    );
    let svc = MemoryServiceImpl::new(Arc::clone(&system));

    // Pre-warm the vectorizer so the measured write isn't dominated by
    // the first-call HF Hub model download. Subsequent calls hit the
    // OnceCell fast path.
    let session_id = system
        .create_session(
            Some("e2e-warmup".into()),
            Some("E2E pipeline drain warm-up".into()),
        )
        .await
        .expect("create_session must succeed");

    let warmup_req = UpdateContextRequest {
        session_id,
        interaction_type: UpdateType::ConceptDefined,
        content: UpdateContent {
            title: "Warmup".into(),
            description: "Загряваме pipeline + embeddings".into(),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        entities: vec![
            entity("Pipeline", EntityType::Concept),
            entity("Warmup", EntityType::Concept),
        ],
        relations: vec![relation(
            "Warmup",
            "Pipeline",
            RelationType::RelatedTo,
            "Initial warm-up touches the pipeline",
        )],
        code_reference: None,
    };
    let _ = svc
        .update_context(warmup_req)
        .await
        .expect("warmup update_context must succeed");
    drain_pipeline(&svc, 30_000).await;

    // -- Measured write -------------------------------------------------
    let req = UpdateContextRequest {
        session_id,
        interaction_type: UpdateType::DecisionMade,
        content: UpdateContent {
            title: "Switch primary embedding to Potion".into(),
            description: "Решихме да минем на minishlab/potion-multilingual-128M като \
                 default — по-малък, по-бърз, multilingual."
                .into(),
            details: vec![],
            examples: vec![],
            implications: vec!["Existing HNSW indices must be rebuilt (dim 384 → 256)".into()],
        },
        entities: vec![
            entity("PotionMultilingual", EntityType::Technology),
            entity("MultilingualMiniLM", EntityType::Technology),
            entity("EmbeddingPipeline", EntityType::Concept),
        ],
        relations: vec![
            relation(
                "PotionMultilingual",
                "MultilingualMiniLM",
                RelationType::ConflictsWith,
                "Replaces the previous default — vectors are different dim",
            ),
            relation(
                "PotionMultilingual",
                "EmbeddingPipeline",
                RelationType::Implements,
                "Static-embedding backend behind the canonical embedding pipeline",
            ),
        ],
        code_reference: None,
    };

    let start = std::time::Instant::now();
    let resp = svc
        .update_context(req)
        .await
        .expect("update_context must succeed on warm path");
    let write_latency = start.elapsed();

    assert!(resp.durable, "write must be acknowledged durable");
    assert_eq!(resp.session_id, session_id);
    assert!(
        write_latency.as_millis() < 500,
        "warm-path update_context took {write_latency:?} — should stay sub-500ms"
    );

    // -- Wait for the bounded pipeline to drain ------------------------
    drain_pipeline(&svc, 60_000).await;
    assert_eq!(
        svc.pipeline().backlog(),
        0,
        "pipeline backlog must drain within 60s"
    );

    // -- Verify the entry made it into the entity graph + vectoriser ---
    let session_arc = system
        .get_session(session_id)
        .await
        .expect("session must be cached after update_context");
    let session = session_arc.load();

    // Entity graph populated from this update plus the warm-up.
    let creates: usize = session
        .hot_context
        .iter()
        .iter()
        .map(|u| u.creates_entities.len() + u.creates_relationships.len())
        .sum();
    assert!(
        creates > 0,
        "session hot_context must contain entity/relation deltas from the writes"
    );

    // Embedding pipeline has caught up — no pending entries.
    let pending = session.pending_vectorization_count();
    assert_eq!(
        pending, 0,
        "pending_vectorization_count must be 0 after pipeline drain (got {pending})"
    );

    // -- Verify SurrealDB persistence -----------------------------------
    // Round-trip the session via storage. After drop + reload the
    // session must still contain our context updates.
    drop(session);
    drop(session_arc);

    let reloaded = system
        .get_conversation_context(session_id)
        .await
        .expect("session must round-trip through SurrealDB");
    assert!(
        reloaded.contains("Potion") || reloaded.contains("potion"),
        "reloaded session must mention our decision title; got: {reloaded:?}"
    );
}
