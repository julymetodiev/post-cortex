//! End-to-end test against a real SurrealDB + real Model2Vec embeddings.
//!
//! Drives the full canonical write path from an MCP-style typed
//! [`UpdateContextRequest`] through:
//!
//! 1. `Records::write` (validation, metadata,
//!    persistence)
//! 2. SurrealDB persist via the storage actor (`ws://localhost:8000`)
//! 3. The non-blocking pipeline (embedding + graph + summary workers)
//! 4. Real `potion-multilingual-128M` vectorisation (HF Hub download
//!    cached on first run)
//! 5. Drained pipeline backlog + cleared per-session pending set
//!
//! Marked `#[ignore]` because it needs:
//!
//! - Network access to `ws://localhost:8000`
//! - HuggingFace Hub reachability (~50 MB Model2Vec checkpoint on first
//!   run)
//!
//! Run with:
//!
//! ```sh
//! cargo test -p post-cortex --features surrealdb-storage \
//!     --test it integration_e2e_surrealdb -- --ignored --nocapture
//! ```

#![cfg(feature = "surrealdb-storage")]

use std::sync::Arc;

use post_cortex::{Memory, SystemConfig};
use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::services::UpdateContextRequest;

fn entity(name: &str, kind: EntityType) -> EntityData {
    let now = chrono::Utc::now();
    EntityData {
        aliases: Vec::new(),
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

async fn drain_pipeline(svc: &Memory, max_wait_ms: u64) {
    svc.records
        .wait_idle(std::time::Duration::from_millis(max_wait_ms))
        .await;
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
#[ignore = "requires ws://localhost:8000 SurrealDB + HF Hub network"]
async fn e2e_update_context_against_remote_surrealdb_and_real_embeddings() {
    init_tracing();
    // Endpoint / namespace / database / credentials all come from env
    // vars so the test stays runnable against any reachable SurrealDB
    // instance (defaults are a SurrealDB on this machine):
    //   POST_CORTEX_E2E_SURREAL_URL=ws://localhost:8000
    //   POST_CORTEX_E2E_SURREAL_NS=post_cortex
    //   POST_CORTEX_E2E_SURREAL_DB=main
    //   POST_CORTEX_E2E_SURREAL_USER=root
    //   POST_CORTEX_E2E_SURREAL_PASS=…
    let endpoint = std::env::var("POST_CORTEX_E2E_SURREAL_URL")
        .unwrap_or_else(|_| "ws://localhost:8000".to_string());
    let namespace =
        std::env::var("POST_CORTEX_E2E_SURREAL_NS").unwrap_or_else(|_| "post_cortex".to_string());
    let database =
        std::env::var("POST_CORTEX_E2E_SURREAL_DB").unwrap_or_else(|_| "main".to_string());
    let username = std::env::var("POST_CORTEX_E2E_SURREAL_USER").ok();
    let password = std::env::var("POST_CORTEX_E2E_SURREAL_PASS").ok();

    let storage = post_cortex::compose::StorageConfig::SurrealDb {
        endpoint,
        username,
        password,
        namespace: Some(namespace),
        database: Some(database),
    };
    // Explicit so the test is robust against future Default tweaks.
    let config = SystemConfig {
        enable_embeddings: true,
        auto_vectorize_on_update: true,
        ..Default::default()
    };
    let models = post_cortex::compose::Models::new(&post_cortex::compose::ModelConfig {
        embedding_model: post_cortex_embeddings::EmbeddingModelType::PotionMultilingual,
        ..Default::default()
    });

    let system = Arc::new(
        post_cortex::compose::open_memory(
            &storage,
            &models,
            None,
            config,
            post_cortex::PipelineConfig::default(),
        )
        .await
        .expect("Memory must build against the remote SurrealDB"),
    );
    let svc = Arc::clone(&system);

    // Pre-warm the vectorizer so the measured write isn't dominated by
    // the first-call HF Hub model download. Subsequent calls hit the
    // OnceCell fast path.
    let session_id = system
        .sessions
        .create(
            Some("e2e-warmup".into()),
            Some("E2E pipeline drain warm-up".into()),
        )
        .await
        .expect("create_session must succeed");

    let warmup_req = UpdateContextRequest {
        meta: Default::default(),
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
        author: None,
        derived_from: Vec::new(),
    };
    let _ = svc
        .records
        .write(warmup_req)
        .await
        .expect("warmup update_context must succeed");
    drain_pipeline(&svc, 30_000).await;

    // -- Measured write -------------------------------------------------
    let req = UpdateContextRequest {
        meta: Default::default(),
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
        author: None,
        derived_from: Vec::new(),
    };

    let start = std::time::Instant::now();
    let resp = svc
        .records
        .write(req)
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
        svc.admin.health().pipeline_backlog,
        0,
        "pipeline backlog must drain within 60s"
    );

    // -- Verify the entry made it into the entity graph + vectoriser ---
    let session = system
        .sessions
        .get(session_id)
        .await
        .expect("session must be cached after update_context");

    // Entity graph populated from this update plus the warm-up.
    let creates: usize = session
        .hot_records()
        .map(|u| u.creates_entities.len() + u.creates_relationships.len())
        .sum();
    assert!(
        creates > 0,
        "the session's hot window must contain entity/relation deltas from the writes"
    );

    // Embedding pipeline has caught up — no pending entries.
    let pending = system
        .sessions
        .pending_vectorization_count(session_id)
        .await
        .expect("pending count");
    assert_eq!(
        pending, 0,
        "pending_vectorization_count must be 0 after pipeline drain (got {pending})"
    );

    // -- Verify SurrealDB persistence -----------------------------------
    // Round-trip the session via storage. After drop + reload the
    // session must still contain our context updates.
    drop(session);

    let reloaded = system
        .sessions
        .get(session_id)
        .await
        .expect("session must round-trip through SurrealDB")
        .context_summary();
    assert!(
        reloaded.contains("Potion") || reloaded.contains("potion"),
        "reloaded session must mention our decision title; got: {reloaded:?}"
    );
}
