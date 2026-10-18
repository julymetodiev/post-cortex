# post-cortex-memory

[![Crates.io](https://img.shields.io/crates/v/post-cortex-memory.svg)](https://crates.io/crates/post-cortex-memory)
[![Docs.rs](https://docs.rs/post-cortex-memory/badge.svg)](https://docs.rs/post-cortex-memory)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Conversation memory orchestrator for [post-cortex](https://docs.rs/post-cortex).

Ties [`post-cortex-core`](https://docs.rs/post-cortex-core), [`post-cortex-storage`](https://docs.rs/post-cortex-storage), [`post-cortex-embeddings`](https://docs.rs/post-cortex-embeddings), and [`post-cortex-proto`](https://docs.rs/post-cortex-proto) into a single lock-free memory hierarchy with async pipelines and a canonical service trait.

Writes go through `MemoryServiceImpl::update_context`: the request is redacted (see `post_cortex_core::redact`), validated, and returned only after the storage write is acknowledged (`durable` in the response). Embedding, entity-graph merge and summary refresh then run on the bounded background pipeline. Session persists are coalesced and ordered per session, so the newest snapshot is always written last.

## Install

```toml
[dependencies]
post-cortex-memory = "0.4"
```

## What's inside

| Module | Contents |
|--------|----------|
| `ConversationMemorySystem` | Top-level orchestrator |
| `MemoryServiceImpl` | Implementation of `post_cortex_core::services::PostCortexService` |
| `Pipeline` / `PipelineConfig` | Bounded MPSC work queues for the derived work after a write (embeddings, graph, summaries) |
| `memory_system::*` | Session manager, storage actor, coalescing per-session persister, circuit breaker |
| `content_vectorizer::*` | Embedding ingestion + semantic search over per-session vector indexes |
| `semantic_query_engine` | High-level query API over vector + storage |
| `query_cache` | Semantic-search result cache keyed by a hash of the full query parameters, checked before the query is embedded and invalidated per session on writes |
| `context_assembly` | Graph-aware context retrieval (entity match, one-hop graph, impact analysis, recency — no embeddings) |
| `jsonpath` | JSONPath subset behind the `select` parameter of `semantic_search` / `assemble_context` |
| `scoring` | Relevance scoring helpers (temporal decay, composite) |

## Example

```rust,no_run
use post_cortex_core::services::PostCortexService; // from post-cortex-core
use post_cortex_memory::{ConversationMemorySystem, MemoryServiceImpl, SystemConfig};
use std::sync::Arc;
use std::time::Duration;

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = SystemConfig {
        data_directory: "./pcx-data".into(),
        ..Default::default()
    };
    let system = Arc::new(ConversationMemorySystem::new(config).await?);
    let session_id = system
        .create_session(Some("demo".into()), None)
        .await?;
    println!("created session {session_id}");

    // Every transport writes through the canonical service.
    let service = MemoryServiceImpl::new(system.clone());
    println!("health: {}", service.health().await?.status);

    // Listing reads session metadata only; sessions are not loaded.
    for summary in system.list_session_summaries().await? {
        println!("{} {:?}", summary.id, summary.name);
    }

    // On shutdown, write every queued session persist.
    system.flush_all(Duration::from_secs(10)).await;
    Ok(())
}
```

## Features

| Feature | Default | What it enables |
|---------|---------|-----------------|
| `embeddings` | yes | Embedding-backed code paths (vectorization, semantic search). The Model2Vec backend of `post-cortex-embeddings` is always compiled in |
| `bert` | no | BERT backend (`post-cortex-embeddings/bert`); selecting a BERT model without it fails at engine load |
| `surrealdb-storage` | no | Forward to `post-cortex-storage/surrealdb-storage` |
| `otel` | no | Pulls the OpenTelemetry crates (`opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`, `tracing-opentelemetry`) |

## License

MIT — see [LICENSE](../../LICENSE).
