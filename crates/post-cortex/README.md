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
| Embedding engine + HNSW | [`post-cortex-embeddings`](https://docs.rs/post-cortex-embeddings) |
| Full memory orchestrator | [`post-cortex-memory`](https://docs.rs/post-cortex-memory) |
| MCP tool functions | [`post-cortex-mcp`](https://docs.rs/post-cortex-mcp) |
| Running daemon + `pcx` CLI | [`post-cortex-daemon`](https://docs.rs/post-cortex-daemon) |

## Install

```toml
[dependencies]
post-cortex = "0.2"
```

## Example

```rust,no_run
use post_cortex::{ConversationMemorySystem, SystemConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let system = ConversationMemorySystem::new(SystemConfig::default()).await?;
    let session = system.create_session(None, None).await?;
    println!("created session {session}");
    Ok(())
}
```

## Features

| Feature | Default | What it enables |
|---------|---------|-----------------|
| `embeddings` | yes | BERT + HNSW via `post-cortex-embeddings` |
| `surrealdb-storage` | no | SurrealDB backend |
| `surrealdb-tikv` | no | SurrealDB + TiKV distributed KV |
| `otel` | no | OpenTelemetry exporter wiring |

## Architecture

```text
post-cortex            (this facade)
├── post-cortex-core         (domain — no I/O)
├── post-cortex-proto        (gRPC wire types)
├── post-cortex-embeddings   (BERT + HNSW)
├── post-cortex-storage      (RocksDB + SurrealDB)
├── post-cortex-memory       (orchestrator)
├── post-cortex-mcp          (MCP tools, pure lib)
└── post-cortex-daemon       (rmcp + axum + tonic + pcx CLI)
```

Lock-free design (DashMap, ArcSwap, atomic ops, actor pattern) throughout the storage + session layers — see [`post-cortex-core`](https://docs.rs/post-cortex-core) for the lock-free invariants.

## MSRV

Rust 1.85 (edition 2024).

## License

MIT — see [LICENSE](../../LICENSE).
