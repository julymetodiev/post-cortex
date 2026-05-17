# post-cortex-memory

[![Crates.io](https://img.shields.io/crates/v/post-cortex-memory.svg)](https://crates.io/crates/post-cortex-memory)
[![Docs.rs](https://docs.rs/post-cortex-memory/badge.svg)](https://docs.rs/post-cortex-memory)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Conversation memory orchestrator for [post-cortex](https://docs.rs/post-cortex).

Ties [`post-cortex-core`](https://docs.rs/post-cortex-core), [`post-cortex-storage`](https://docs.rs/post-cortex-storage), [`post-cortex-embeddings`](https://docs.rs/post-cortex-embeddings), and [`post-cortex-proto`](https://docs.rs/post-cortex-proto) into a single lock-free memory hierarchy with async pipelines and a canonical service trait.

## Install

```toml
[dependencies]
post-cortex-memory = "0.2"
```

## What's inside

| Module | Contents |
|--------|----------|
| `ConversationMemorySystem` | Top-level orchestrator |
| `MemoryServiceImpl` | Implementation of `post_cortex_core::services::PostCortexService` |
| `Pipeline` / `PipelineConfig` | Bounded MPSC work queues for non-blocking writes |
| `memory_system::*` | Session manager, storage actor, circuit breaker |
| `content_vectorizer::*` | Embedding ingestion + semantic search engine |
| `semantic_query_engine` | High-level query API over vector + storage |
| `query_cache` | LRU cache for repeated semantic queries |
| `context_assembly` | Graph-aware context retrieval |
| `scoring` | Relevance scoring helpers (temporal decay, composite) |
| `performance` | Performance metrics + monitor |

## Example

```rust,no_run
use post_cortex_memory::{ConversationMemorySystem, SystemConfig};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let config = SystemConfig {
    data_directory: "./pcx-data".into(),
    ..Default::default()
};
let system = ConversationMemorySystem::new(config).await?;
let session_id = system.create_session(None, None).await?;
println!("created session {session_id}");
# Ok(()) }
```

## Features

| Feature | Default | What it enables |
|---------|---------|-----------------|
| `embeddings` | yes | Pull the BERT backend from `post-cortex-embeddings` |
| `surrealdb-storage` | no | Forward to `post-cortex-storage/surrealdb-storage` |
| `otel` | no | OpenTelemetry exporter wiring |

## License

MIT — see [LICENSE](../../LICENSE).
