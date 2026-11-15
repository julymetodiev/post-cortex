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
| `core::cache` | `SessionCache`: the LRU-style session cache (DashMap + atomic hit/miss counters) |
| `core::timeout_utils` | Default deadlines for MCP and storage operations |
| `session` | `ActiveSession` + components |
| `graph` | `SimpleEntityGraph` (petgraph-backed), retrieval graph, temporal views, analytics |
| `workspace` | `WorkspaceManager`, `Workspace`, `SessionRole` |
| `summary` | `StructuredSummaryView`, `SummaryGenerator` |
| `services` | Request and response DTOs of the memory capability services (`post_cortex_memory::capabilities`) |
| `redact` | `Redactor` / `RedactionConfig` — replaces credentials, e-mail addresses, phone numbers and public IPv4 addresses with `[REDACTED:<kind>]` |
| `proto` | Re-export of `post_cortex_proto` |

## Example

A write request, as every transport (MCP, gRPC, REST) builds it before handing it to `Records::write` in `post-cortex-memory`:

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

Redaction runs on that request inside `Records::write`; it can also be applied directly:

```rust,no_run
use post_cortex_core::redact::{RedactionConfig, RedactionCounts, Redactor};

let redactor = Redactor::new(&RedactionConfig::default())?;

let mut counts = RedactionCounts::new();
let clean = redactor.redact_str("mail ops@example.com", &mut counts);
println!("{clean} ({counts:?})"); // mail [REDACTED:email] ...
# Ok::<(), post_cortex_core::redact::RedactError>(())
```

## MSRV

Rust 1.95 (edition 2024). Enforced via `[workspace.package] rust-version`.

## License

MIT — see [LICENSE](../../LICENSE).
