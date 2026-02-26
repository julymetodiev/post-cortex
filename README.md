<p align="center">
  <img src="design/v1.svg" width="128" height="128" alt="Post-Cortex Logo">
</p>

<h1 align="center">Post-Cortex</h1>

<p align="center"><strong>Persistent Memory for AI Assistants</strong></p>

Post-Cortex is an MCP server that gives AI assistants long-term memory. It stores conversations, decisions, and insights in a searchable knowledge base with automatic entity extraction.

## Features

- **Persistent Memory** - Conversations survive across sessions
- **Semantic Search** - Find related content using AI embeddings with HNSW indexing
- **Graph-RAG** - Search results enriched with entity graph insights and relationship paths
- **Knowledge Graph** - Automatic entity and relationship extraction
- **Privacy-First** - All processing runs locally, no external APIs
- **Fast** - Lock-free Rust architecture, O(log n) vector search, <10ms queries
- **Flexible Storage** - RocksDB (embedded) or SurrealDB (distributed)

## Installation

```bash
# Homebrew (macOS/Linux)
brew install julymetodiev/tap/post-cortex

# Or download binary
curl -L https://github.com/julymetodiev/post-cortex/releases/latest/download/pcx-aarch64-apple-darwin -o /usr/local/bin/pcx
chmod +x /usr/local/bin/pcx
```

## Quick Start

### 1. Configure MCP

Add to your Claude config (`claude_desktop_config.json` or `.mcp.json`):

```json
{
  "mcpServers": {
    "post-cortex": {
      "command": "pcx"
    }
  }
}
```

### 2. Set Up Your Project

```bash
pcx setup
```

This creates a session, workspace, `CLAUDE.md` with memory rules, hooks for enforcement, and installs agent definitions. See **[Usage Guide](docs/USAGE_GUIDE.md)** for details.

### 3. Start Coding

```bash
claude
```

Claude will automatically search past knowledge before answering and log new discoveries.

## MCP Tools

| Tool | Purpose |
|------|---------|
| `session` | Create and list sessions |
| `update_conversation_context` | Store knowledge (qa, decisions, problems, code changes) |
| `semantic_search` | Find related content across sessions, workspaces, or globally |
| `get_structured_summary` | Get session overview (decisions, insights, entities) |
| `query_conversation_context` | Query entity relationships and keyword search |
| `manage_workspace` | Organize sessions into workspaces |

## Daemon Mode

For multiple Claude instances sharing the same memory:

```bash
pcx start    # Start daemon
pcx status   # Check status
pcx stop     # Stop daemon
```

Configure Claude for HTTP transport:
```json
{
  "mcpServers": {
    "post-cortex": {
      "type": "http",
      "url": "http://localhost:3737/mcp"
    }
  }
}
```

See [Daemon Mode docs](docs/DAEMON_MODE.md) for details.

## Data Export/Import

```bash
pcx export --output backup.json       # Full export
pcx export --output backup.json.gz    # Compressed
pcx import --input backup.json        # Import
```

## Storage Backends

| | RocksDB (default) | SurrealDB |
|---|---|---|
| Setup | Zero config | Requires server |
| Distribution | Embedded | Distributed |
| Vector Search | HNSW O(log n) | HNSW O(log n) |

Configure in `~/.post-cortex/daemon.toml`. See [Daemon Mode docs](docs/DAEMON_MODE.md).

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PC_HOST` | 127.0.0.1 | Bind address |
| `PC_PORT` | 3737 | Port |
| `PC_DATA_DIR` | ~/.post-cortex/data | Storage location |
| `PC_STORAGE_BACKEND` | rocksdb | `rocksdb` or `surrealdb` |

## Development

```bash
cargo build --release --features "embeddings,surrealdb-storage"
cargo test
cargo bench
```

## License

MIT
