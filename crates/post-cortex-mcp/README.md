# post-cortex-mcp

[![Crates.io](https://img.shields.io/crates/v/post-cortex-mcp.svg)](https://crates.io/crates/post-cortex-mcp)
[![Docs.rs](https://docs.rs/post-cortex-mcp/badge.svg)](https://docs.rs/post-cortex-mcp)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Model Context Protocol (MCP) tool definitions for [post-cortex](https://docs.rs/post-cortex).

**Pure library** — no `rmcp`, `axum`, `tonic` server, or transport runtime dependency. Each public function takes the typed parameters of a single MCP tool, dispatches into the post-cortex domain layer, and returns an `MCPToolResult`. Embed these tools in your own MCP server runtime (rmcp, custom servers, anything else) without pulling the post-cortex daemon.

The functions share one process-wide `ConversationMemorySystem`: inject yours with `inject_memory_system`, or let `get_memory_system` open the default one under `~/.post-cortex/data`. Context writes go through the canonical `MemoryServiceImpl`, so they get the same redaction and validation as every other transport.

## Install

```toml
[dependencies]
post-cortex-mcp = "0.4"
```

## Modules

The nine tools served by `post-cortex-daemon` (`session`, `update_conversation_context`, `semantic_search`, `get_structured_summary`, `query_conversation_context`, `manage_workspace`, `assemble_context`, `manage_entity`, `admin`) are built on these functions:

| Module | Tool surface |
|--------|--------------|
| `session` | session listing / load / search / metadata update, checkpoints, mark-important |
| `update_context` | single (`update_conversation_context`) + bulk (`bulk_update_conversation_context`) context writes |
| `query` | structured queries against session context |
| `search` | semantic search (session / global), related content, vectorization |
| `analysis` | structured summary, decisions, insights, entity importance + network views |
| `workspace` | workspace CRUD + session membership |
| `schemas` | JSON Schema for the MCP tool surface |

## Example

Every write carries typed `entities` and `relations` (at least one each; relations must point at listed entities):

```rust,no_run
use post_cortex_mcp::{EntityItem, RelationItem, inject_memory_system, update_conversation_context};
use post_cortex_memory::{ConversationMemorySystem, SystemConfig};
use std::collections::HashMap;
use std::sync::Arc;

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let system = Arc::new(ConversationMemorySystem::new(SystemConfig::default()).await?);
    let session_id = system.create_session(None, None).await?;
    inject_memory_system(system);

    let content = HashMap::from([
        ("title".to_string(), "Picked Rust over Go".to_string()),
        ("description".to_string(), "Memory safety without a GC".to_string()),
    ]);
    let entities = vec![
        EntityItem { name: "Rust".into(), entity_type: "technology".into() },
        EntityItem { name: "Go".into(), entity_type: "technology".into() },
    ];
    let relations = vec![RelationItem {
        from_entity: "Rust".into(),
        to_entity: "Go".into(),
        relation_type: "conflicts_with".into(),
        context: "picked one of the two for the service".into(),
    }];

    let result = update_conversation_context(
        "decision_made".to_string(),
        content,
        entities,
        relations,
        None, // code_reference
        session_id,
    )
    .await?;

    assert!(result.success);
    Ok(())
}
```

## Features

| Feature | Default | What it enables |
|---------|---------|-----------------|
| `embeddings` | yes | Semantic search and vectorization tools (forwards to `post-cortex-memory/embeddings`) |
| `otel` | no | Placeholder; currently enables nothing |

## License

MIT — see [LICENSE](../../LICENSE).
