# post-cortex-memory

[![Crates.io](https://img.shields.io/crates/v/post-cortex-memory.svg)](https://crates.io/crates/post-cortex-memory)
[![Docs.rs](https://docs.rs/post-cortex-memory/badge.svg)](https://docs.rs/post-cortex-memory)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Conversation memory orchestrator for [post-cortex](https://docs.rs/post-cortex).

Ties [`post-cortex-core`](https://docs.rs/post-cortex-core), [`post-cortex-storage`](https://docs.rs/post-cortex-storage), [`post-cortex-embeddings`](https://docs.rs/post-cortex-embeddings), and [`post-cortex-proto`](https://docs.rs/post-cortex-proto) into a single lock-free memory hierarchy with async pipelines and a canonical service trait.

Writes go through `memory.records.write` (`capabilities::Records`): the request is redacted (see `post_cortex_core::redact`), validated, and returned only after the storage write is acknowledged (`durable` in the response). Embedding, entity-graph merge and summary refresh then run on the bounded background pipeline. Session persists are coalesced and ordered per session, so the newest snapshot is always written last.

## Install

```toml
[dependencies]
post-cortex-memory = "0.4"
```

## What's inside

| Module | Contents |
|--------|----------|
| `Memory` | Builds every capability service over one runtime from its ports (`Memory::over_ports`, `over_ports_with`; the daemon's `compose` opens the adapters) |
| `capabilities::*` | The services: `Records`, `Search`, `Assembly`, `Views`, `Entities`, `Sessions`, `Workspaces`, `Projects`, `Cards`, `Messages`, `Freshness`, `Transfer`, `Admin`, each with one typed error |
| `PipelineConfig` | Bounded queue for the derived work after a write (embedding the record and its entities) |
| `memory_system::*` | The runtime (crate-private): session cache, workspace registry, coalescing per-session persister, circuit breaker |
| `content_vectorizer::*` | Embedding ingestion + semantic search over per-session vector indexes |
| `query_cache` | Semantic-search result cache keyed by a hash of the full query parameters, checked before the query is embedded and invalidated per session on writes |
| `context_assembly` | Graph-aware context retrieval (entity match, one-hop graph, impact analysis, recency — no embeddings) |
| `scoring` | Relevance scoring helpers (temporal decay, composite) |

## Example

```rust,no_run
use post_cortex_memory::{Memory, MemoryPorts, SystemConfig};
use std::time::Duration;

/// `ports` come from the composition root (`post_cortex_daemon::compose`).
async fn run(ports: MemoryPorts) -> Result<(), Box<dyn std::error::Error>> {
    let memory = Memory::over_ports(ports, SystemConfig::default()).await?;
    let session_id = memory.sessions.create(Some("demo".into()), None).await?;
    println!("created session {session_id}");
    println!("active sessions: {}", memory.admin.health().active_sessions);

    // Listing reads session metadata only; sessions are not loaded.
    for summary in memory.sessions.list_summaries().await? {
        println!("{} {:?}", summary.id, summary.name);
    }

    // On shutdown, write every queued session persist.
    memory.admin.flush_all(Duration::from_secs(10)).await;
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
