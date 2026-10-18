# post-cortex-core

[![Crates.io](https://img.shields.io/crates/v/post-cortex-core.svg)](https://crates.io/crates/post-cortex-core)
[![Docs.rs](https://docs.rs/post-cortex-core/badge.svg)](https://docs.rs/post-cortex-core)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Core domain library for [post-cortex](https://docs.rs/post-cortex).

This crate is the dependency root of the workspace. It contains the types, traits, error hierarchy, and lightweight domain modules (session, graph, workspace, summary, structured context, redaction) needed to describe a post-cortex conversation. **Transport-agnostic — no `axum`, `rmcp`, `rocksdb`, or `candle`.** The only gRPC-related dependency is `tonic` / `prost`, pulled in through [`post-cortex-proto`](https://docs.rs/post-cortex-proto) for the shared wire types.

Library users embedding post-cortex into their own Rust applications depend on `post-cortex-core` if they want the type system only, or on the orchestrator crate [`post-cortex-memory`](https://docs.rs/post-cortex-memory) (which transitively pulls core + storage + embeddings) if they want a ready-to-use memory system.

## Install

```toml
[dependencies]
post-cortex-core = "0.4"
```

## What's inside

| Module | Contents |
|--------|----------|
| `core::error` | `SystemError` enum, `Result<T>` alias — `thiserror`-derived (re-exported at the crate root) |
| `core::context_update` | `ContextUpdate`, `UpdateContent`, `UpdateType`, `EntityData`, `EntityType`, `EntityRelationship`, `RelationType`, `CodeReference` |
| `core::structured_context` | `StructuredContext` projection types |
| `core::cache` | Lock-free LRU-style cache primitives (DashMap + atomic metrics) |
| `core::timeout_utils` | Retry + timeout helpers |
| `session` | `ActiveSession` + components |
| `graph` | `EntityGraph` (petgraph-backed) + GraphRAG |
| `workspace` | `WorkspaceManager`, `Workspace`, `SessionRole` |
| `summary` | `StructuredSummaryView`, `SummaryGenerator` |
| `services` | `PostCortexService` trait (canonical service interface) + request/response types |
| `redact` | `Redactor` / `RedactionConfig` — replaces credentials, e-mail addresses, phone numbers and public IPv4 addresses with `[REDACTED:<kind>]` |
| `proto` | Re-export of `post_cortex_proto` |

## Example

A canonical write request, as every transport (MCP, gRPC) builds it before handing it to `PostCortexService::update_context`:

```rust,no_run
use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::services::UpdateContextRequest;
use uuid::Uuid;

let now = chrono::Utc::now();
let entity = |name: &str, entity_type| EntityData {
    name: name.into(),
    entity_type,
    first_mentioned: now,
    last_mentioned: now,
    mention_count: 1,
    importance_score: 1.0,
    description: None,
};

let request = UpdateContextRequest {
    session_id: Uuid::new_v4(),
    interaction_type: UpdateType::DecisionMade,
    content: UpdateContent {
        title: "Adopt PostgreSQL".into(),
        description: "Picked PostgreSQL over MySQL for the relational store".into(),
        details: vec!["better JSON support, mature replication".into()],
        examples: vec![],
        implications: vec![],
    },
    entities: vec![
        entity("PostgreSQL", EntityType::Technology),
        entity("relational store", EntityType::Concept),
    ],
    relations: vec![EntityRelationship {
        from_entity: "PostgreSQL".into(),
        to_entity: "relational store".into(),
        relation_type: RelationType::Implements,
        context: "PostgreSQL backs the relational store".into(),
    }],
    code_reference: None,
};

println!("update for session {}", request.session_id);
```

Redaction runs on that request inside the canonical service; it can also be applied directly:

```rust,no_run
use post_cortex_core::redact::{RedactionConfig, RedactionCounts, Redactor};

let (redactor, invalid_patterns) = Redactor::new(&RedactionConfig::default());
assert!(invalid_patterns.is_empty());

let mut counts = RedactionCounts::new();
let clean = redactor.redact_str("mail ops@example.com", &mut counts);
println!("{clean} ({counts:?})"); // mail [REDACTED:email] ...
```

## Implementing a custom service

Implement the `PostCortexService` trait against your own storage + embeddings stack and the rest of the post-cortex ecosystem (MCP tools, gRPC clients, summary view) will work unchanged:

```rust,no_run
use async_trait::async_trait;
use post_cortex_core::SystemError;
use post_cortex_core::services::*;

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
    async fn update_context(&self, _: UpdateContextRequest) -> Result<UpdateContextResponse, SystemError> { todo!() }
    async fn bulk_update_context(&self, _: BulkUpdateContextRequest) -> Result<BulkUpdateContextResponse, SystemError> { todo!() }
    async fn semantic_search(&self, _: SemanticSearchRequest) -> Result<SemanticSearchResponse, SystemError> { todo!() }
    async fn query_context(&self, _: QueryContextRequest) -> Result<QueryContextResponse, SystemError> { todo!() }
    async fn assemble_context(&self, _: AssembleContextRequest) -> Result<AssembleContextResponse, SystemError> { todo!() }
    async fn manage_session(&self, _: ManageSessionRequest) -> Result<ManageSessionResponse, SystemError> { todo!() }
    async fn manage_workspace(&self, _: ManageWorkspaceRequest) -> Result<ManageWorkspaceResponse, SystemError> { todo!() }
    async fn manage_entity(&self, _: ManageEntityRequest) -> Result<ManageEntityResponse, SystemError> { todo!() }
    async fn get_structured_summary(&self, _: StructuredSummaryRequest) -> Result<StructuredSummaryResponse, SystemError> { todo!() }
    async fn admin(&self, _: AdminRequest) -> Result<AdminResponse, SystemError> { todo!() }
}
```

## MSRV

Rust 1.95 (edition 2024). Enforced via `[workspace.package] rust-version`.

## License

MIT — see [LICENSE](../../LICENSE).
