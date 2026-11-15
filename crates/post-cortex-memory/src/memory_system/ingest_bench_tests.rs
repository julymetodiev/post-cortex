// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Bulk-ingest throughput, the regression guard for the embedding queue:
//! one session, [`RECORDS`] records written as bulk calls of [`BULK`]
//! through the service (the write path a daemon runs), until every record
//! has its vector in storage. Prints the time the bulk calls took (`api`)
//! and the time until the derived work was done (`vectorized`), per backend
//! and engine.
//!
//! - `hash` (`test_support::hash_engine`) costs almost nothing per text: it
//!   measures the queue, the vector stores, the session persists and the
//!   entity pass, not embedding.
//! - `model2vec` (the default PotionMultilingual model, from the local
//!   Hugging Face cache) adds the real embedding cost on this machine's CPU.
//! - `rtt` puts every session write and vector call behind a fixed round
//!   trip ([`super::remote_tests`]), standing for a SurrealDB server; `mem://`
//!   and RocksDB run in process and pay none. The printed call counts
//!   (session writes, vector store writes) do not depend on the machine.
//!
//! Not run by default:
//! `cargo test -p post-cortex-memory --release --features surrealdb-storage --lib bench_bulk_ingest -- --ignored --nocapture`

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use post_cortex_core::services::BulkUpdateContextRequest;

use post_cortex_embeddings::{EmbeddingConfig, EmbeddingModelType, LocalEmbeddingEngine};

use super::backends_tests::{Backend, backends, embeddings_config, request};
use super::remote_tests::remote_system;

/// Records in the session (LoCoMo: 5882 turns in 10 conversations).
const RECORDS: usize = 3_000;
/// Records per bulk call (the LoCoMo gate sends about 600).
const BULK: usize = 600;

/// A conversational turn of about LoCoMo length.
fn turn(i: usize) -> String {
    format!(
        "Turn {i}: I went to the {} on day {} with my {} and we talked about {} for a while, \
         then planned to meet again next week.",
        ["market", "museum", "beach", "library", "concert"][i % 5],
        i / 7,
        ["sister", "neighbour", "colleague", "friend"][i % 4],
        [
            "painting",
            "running",
            "a new job",
            "the kids",
            "travel plans",
            "books"
        ][i % 6],
    )
}

/// Ingest [`RECORDS`] records into a new session of `memory`; returns the
/// time of the bulk calls and the time until every vector was stored.
async fn ingest(
    memory: &super::backends_tests::TestSystem,
    backend: &Backend,
) -> (Duration, Duration) {
    let service = memory.mem.clone();
    let sid = memory.mem.sessions.create(None, None).await.unwrap();
    let start = Instant::now();
    for chunk in (0..RECORDS).collect::<Vec<_>>().chunks(BULK) {
        let updates = chunk
            .iter()
            .map(|&i| request(sid, &format!("Turn {i}"), &turn(i)))
            .collect();
        service
            .records
            .bulk_write(BulkUpdateContextRequest {
                session_id: sid,
                updates,
            })
            .await
            .unwrap();
    }
    let api = start.elapsed();
    assert!(
        memory
            .derived_work
            .wait_idle(Duration::from_secs(900))
            .await,
        "derived work did not finish"
    );
    let vectorized = start.elapsed();
    let stored = backend
        .vectors()
        .count_session_embeddings(&sid.to_string())
        .await
        .unwrap();
    assert!(stored >= RECORDS, "{stored} of {RECORDS} records stored");
    (api, vectorized)
}

/// Round trip of the simulated server.
const RTT: Duration = Duration::from_millis(2);

async fn run(
    backend: &Backend,
    engine_name: &str,
    engine: Arc<dyn post_cortex_ports::EmbeddingEngine>,
    rtt: Duration,
) {
    let (memory, calls) = remote_system(backend, embeddings_config(), engine, rtt).await;
    let memory = super::backends_tests::TestSystem::new(memory);
    memory
        .mem
        .admin
        .warm_up(&tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    let (api, vectorized) = ingest(&memory, backend).await;
    println!(
        "bulk ingest {} x {RECORDS} (bulk {BULK}), {engine_name}, rtt {} ms: api {:.2} s, \
         vectorized {:.2} s ({:.0} records/s), session writes {}, vector writes {}",
        backend.name,
        rtt.as_millis(),
        api.as_secs_f64(),
        vectorized.as_secs_f64(),
        RECORDS as f64 / vectorized.as_secs_f64(),
        calls.session_writes.load(Ordering::SeqCst),
        calls.vector_writes.load(Ordering::SeqCst),
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark: run in release with --ignored --nocapture"]
async fn bench_bulk_ingest() {
    let hash = Arc::new(post_cortex_embeddings::test_support::hash_engine(256));
    let model2vec = Arc::new(
        LocalEmbeddingEngine::new(EmbeddingConfig {
            model_type: EmbeddingModelType::PotionMultilingual,
            ..Default::default()
        })
        .await
        .unwrap(),
    );
    for (engine_name, engine, rtt) in [
        ("hash", &hash, Duration::ZERO),
        ("model2vec", &model2vec, Duration::ZERO),
        ("model2vec", &model2vec, RTT),
    ] {
        for backend in backends().await {
            run(
                &backend,
                engine_name,
                Arc::clone(engine) as Arc<dyn post_cortex_ports::EmbeddingEngine>,
                rtt,
            )
            .await;
        }
    }
}
