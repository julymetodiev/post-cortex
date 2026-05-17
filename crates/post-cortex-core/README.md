# post-cortex-core

[![Crates.io](https://img.shields.io/crates/v/post-cortex-core.svg)](https://crates.io/crates/post-cortex-core)
[![Docs.rs](https://docs.rs/post-cortex-core/badge.svg)](https://docs.rs/post-cortex-core)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Core domain library for [post-cortex](https://docs.rs/post-cortex).

This crate is the dependency root of the workspace. It contains the types, traits, error hierarchy, and lightweight domain modules (session, graph, workspace, summary, structured context) needed to describe a post-cortex conversation. **Transport-agnostic — no `axum`, `tonic` transport, `rmcp`, `rocksdb`, or `candle`.**

Library users embedding post-cortex into their own Rust applications depend on `post-cortex-core` if they want the type system only, or on the orchestrator crate [`post-cortex-memory`](https://docs.rs/post-cortex-memory) (which transitively pulls core + storage + embeddings) if they want a ready-to-use memory system.

## Install

```toml
[dependencies]
post-cortex-core = "0.2"
```

## What's inside

| Module | Contents |
|--------|----------|
| `error` | `SystemError` enum, `Result<T>` alias — `thiserror`-derived |
| `context_update` | `ContextUpdate`, `EntityData`, `EntityRelationship`, `UpdateType` |
| `structured_context` | `StructuredContext` projection types |
| `cache` | Generic LRU cache primitives |
| `timeout_utils` | Retry + timeout helpers |
| `session::` | `ActiveSession` + components |
| `graph::` | `EntityGraph` (petgraph-backed) + GraphRAG |
| `workspace::` | `WorkspaceManager`, `SessionRole` |
| `summary::` | `StructuredSummaryView`, `SummaryGenerator` |
| `services::` | `PostCortexService` trait (canonical service interface) |
| `proto` | Re-export of `post_cortex_proto` |

## Example

```rust,no_run
use post_cortex_core::core::context_update::{
    ContextUpdate, UpdateContent, UpdateType, EntityData, EntityType, EntityRelationship,
};
use uuid::Uuid;

let update = ContextUpdate {
    id: Uuid::new_v4(),
    session_id: Uuid::new_v4(),
    timestamp: chrono::Utc::now(),
    update_type: UpdateType::DecisionMade,
    content: UpdateContent {
        title: "Adopt PostgreSQL".into(),
        description: "Picked PostgreSQL over MySQL for the relational store".into(),
        rationale: Some("better JSON support, mature replication".into()),
        ..Default::default()
    },
    entities: vec![],
    relationships: vec![],
    code_references: vec![],
    metadata: None,
};

println!("created update: {}", update.id);
```

## Implementing a custom service

Implement the [`PostCortexService`] trait against your own storage + embeddings stack and the rest of the post-cortex ecosystem (MCP tools, gRPC clients, summary view) will work unchanged:

```rust,no_run
use async_trait::async_trait;
use post_cortex_core::services::*;
use post_cortex_core::SystemError;

struct MyService;

#[async_trait]
impl PostCortexService for MyService {
    async fn health(&self) -> Result<HealthReport, SystemError> {
        Ok(HealthReport {
            status: "ok".into(),
            active_sessions: 0,
            memory_usage_bytes: 0,
            pipeline_backlog: 0,
            uptime_seconds: 0,
        })
    }
    # async fn update_context(&self, _: UpdateContextRequest) -> Result<UpdateContextResponse, SystemError> { unimplemented!() }
    # async fn bulk_update_context(&self, _: BulkUpdateContextRequest) -> Result<BulkUpdateContextResponse, SystemError> { unimplemented!() }
    # async fn semantic_search(&self, _: SemanticSearchRequest) -> Result<SemanticSearchResponse, SystemError> { unimplemented!() }
    # async fn query_context(&self, _: QueryContextRequest) -> Result<QueryContextResponse, SystemError> { unimplemented!() }
    # async fn assemble_context(&self, _: AssembleContextRequest) -> Result<AssembleContextResponse, SystemError> { unimplemented!() }
    # async fn manage_session(&self, _: ManageSessionRequest) -> Result<ManageSessionResponse, SystemError> { unimplemented!() }
    # async fn manage_workspace(&self, _: ManageWorkspaceRequest) -> Result<ManageWorkspaceResponse, SystemError> { unimplemented!() }
    # async fn manage_entity(&self, _: ManageEntityRequest) -> Result<ManageEntityResponse, SystemError> { unimplemented!() }
    # async fn get_structured_summary(&self, _: StructuredSummaryRequest) -> Result<StructuredSummaryResponse, SystemError> { unimplemented!() }
    # async fn admin(&self, _: AdminRequest) -> Result<AdminResponse, SystemError> { unimplemented!() }
}
```

## MSRV

Rust 1.85 (edition 2024). Enforced via `[workspace.package] rust-version`.

## License

MIT — see [LICENSE](../../LICENSE).
