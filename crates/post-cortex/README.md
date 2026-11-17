# post-cortex

[![Crates.io](https://img.shields.io/crates/v/post-cortex.svg)](https://crates.io/crates/post-cortex)
[![Docs.rs](https://docs.rs/post-cortex/badge.svg)](https://docs.rs/post-cortex)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Intelligent conversation memory system for AI assistants — persistent knowledge storage, semantic search, knowledge graph, MCP + gRPC transports.

This is the **facade meta-crate**: one dependency line for the whole post-cortex stack. Direct dependence on a single member crate is usually preferable for production deployments (smaller compile time, fewer transitive deps).

## Picking the right crate

| Goal | Crate |
|------|-------|
| Domain types, traits, error | [`post-cortex-core`](https://docs.rs/post-cortex-core) |
| gRPC client (no server) | [`post-cortex-proto`](https://docs.rs/post-cortex-proto) |
| Custom storage backend | [`post-cortex-storage`](https://docs.rs/post-cortex-storage) |
| Embedding engine + vector index | [`post-cortex-embeddings`](https://docs.rs/post-cortex-embeddings) |
| Full memory orchestrator | [`post-cortex-memory`](https://docs.rs/post-cortex-memory) |
| Operation table, response shapes, MCP server | [`post-cortex-transport`](https://docs.rs/post-cortex-transport) |
| Running daemon + `pcx` CLI | [`post-cortex-daemon`](https://docs.rs/post-cortex-daemon) |

## Install

```toml
[dependencies]
post-cortex = "0.4"
```

## Example

```rust,no_run
use post_cortex::compose::{ModelConfig, Models, StorageConfig, open_memory};
use post_cortex::{PipelineConfig, SystemConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let storage = StorageConfig::RocksDb {
        data_directory: "./pcx-data".into(),
    };
    let models = Models::new(&ModelConfig::default());
    let memory =
        open_memory(&storage, &models, SystemConfig::default(), PipelineConfig::default()).await?;
    let session = memory.sessions.create(None, None).await?;
    println!("created session {session}");
    Ok(())
}
```

## Features

| Feature | Default | What it enables |
|---------|---------|-----------------|
| `embeddings` | yes | Semantic search and vectorization (Model2Vec `potion-multilingual-128M` by default) |
| `bert` | no | BERT embedding backend (Candle) via `post-cortex-memory/bert` and `post-cortex-daemon/bert` |
| `surrealdb-storage` | no | SurrealDB backend |
| `otel` | no | Placeholder; currently enables nothing (use `post-cortex-daemon/otel`) |

## Architecture

```text
post-cortex            (this facade)
├── post-cortex-core         (domain — no I/O)
├── post-cortex-proto        (gRPC wire types)
├── post-cortex-embeddings   (Model2Vec / BERT + vector index)
├── post-cortex-storage      (RocksDB + SurrealDB)
├── post-cortex-memory       (orchestrator)
├── post-cortex-transport    (operation table, MCP server)
└── post-cortex-daemon       (rmcp + axum + tonic + pcx CLI)
```

Lock-free design (DashMap, ArcSwap, atomic ops, actor pattern) throughout the storage + session layers — see [`post-cortex-core`](https://docs.rs/post-cortex-core) for the lock-free invariants.

## MSRV

Rust 1.95 (edition 2024).

## License

MIT — see [LICENSE](../../LICENSE).
