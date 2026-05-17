# post-cortex-mcp

[![Crates.io](https://img.shields.io/crates/v/post-cortex-mcp.svg)](https://crates.io/crates/post-cortex-mcp)
[![Docs.rs](https://docs.rs/post-cortex-mcp/badge.svg)](https://docs.rs/post-cortex-mcp)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Model Context Protocol (MCP) tool definitions for [post-cortex](https://docs.rs/post-cortex).

**Pure library** — no `rmcp`, `axum`, `tonic`, or transport runtime dependency. Each public function takes the typed parameters of a single MCP tool, dispatches into the post-cortex domain layer, and returns an `MCPToolResult`. Embed these tools in your own MCP server runtime (rmcp, custom servers, anything else) without pulling the post-cortex daemon.

## Install

```toml
[dependencies]
post-cortex-mcp = "0.2"
```

## Nine consolidated tools

| Module | Tool surface |
|--------|--------------|
| [`session`] | session lifecycle (create / list / load / search / update / delete / checkpoint) |
| [`update_context`] | single + bulk context writes |
| [`query`] | structured queries against session context |
| [`search`] | semantic search (session / global) + related content |
| [`analysis`] | structured summary, decisions, insights, entity importance + network views |
| [`workspace`] | workspace CRUD |
| [`schemas`] | JSON Schema for the MCP tool surface |

## Example

```rust,no_run
use post_cortex_mcp::{get_memory_system, update_conversation_context};
use post_cortex_memory::{ConversationMemorySystem, SystemConfig};
use std::collections::HashMap;
use std::sync::Arc;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let system = Arc::new(ConversationMemorySystem::new(SystemConfig::default()).await?);
let session_id = system.create_session(None, None).await?;

let mut content = HashMap::new();
content.insert("title".to_string(), "Picked Rust over Go".to_string());
content.insert("rationale".to_string(), "Memory safety + zero-cost".to_string());

let result = update_conversation_context(
    "decision_made".to_string(),
    content,
    None,
    session_id,
).await?;

assert!(result.success);
# Ok(()) }
```

## License

MIT — see [LICENSE](../../LICENSE).
