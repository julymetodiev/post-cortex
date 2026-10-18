<p align="center">
  <img src="design/v1.svg" width="128" height="128" alt="Post-Cortex Logo">
</p>

<h1 align="center">Post-Cortex</h1>

<p align="center"><strong>Persistent Memory for AI Assistants</strong></p>

Post-Cortex is an MCP server that gives AI assistants long-term memory. It stores conversations, decisions, and insights in a searchable knowledge base with automatic entity extraction.

## Features

- **Persistent Memory** — conversations survive across sessions
- **Semantic Search** — `potion-multilingual-128M` (Model2Vec) by default. ~50 MB on disk, ms-per-text inference, multilingual (Latin / Cyrillic / CJK / Greek / Arabic) out of the box. Heavier BERT models stay available behind the `bert` feature.
- **Graph-RAG** — search results enriched with entity-graph traversal and relationship paths
- **Knowledge Graph** — every write carries typed `entities` + `relations` (MCP contract since 0.3.0); the graph stays in sync with the prose
- **Single-entrypoint architecture** — every transport (gRPC, MCP, future REST) routes through the same canonical `MemoryServiceImpl::update_context`; no parallel codepaths to drift between
- **Durable, non-blocking writes** — a write returns once the storage backend has acknowledged it (`durable` in the response); embeddings + entity-graph merge + summary refresh land on a bounded background pipeline. Persists are coalesced and ordered per session.
- **Redaction** — credentials, e-mail addresses, phone numbers and public IPv4 addresses become `[REDACTED:<kind>]` before storage, on every transport (on by default; `PC_REDACT=0` disables)
- **Auth + Host allowlist** — optional bearer token (`PC_AUTH_TOKEN`) on HTTP and gRPC; `/mcp` and `/api/*` reject unknown `Host` headers (DNS-rebinding protection, `PC_ALLOWED_HOSTS`)
- **Privacy-first** — all processing runs locally, no external API calls (the embedding model is downloaded from Hugging Face once, on first use)
- **Lock-free Rust core** — ArcSwap session state, atomic CAS write path; vector search is an exact top-k scan up to 10 000 vectors and HNSW above that, with per-session indexes
- **Flexible storage** — RocksDB (embedded, default) or SurrealDB (local / remote, distributed)

## Workspace layout

This repository is a Cargo workspace of 8 publishable crates. Pick the one that matches your need — depending on the facade `post-cortex` pulls everything, but most consumers only want a subset:

| Crate | Pick when you need… | crates.io |
|-------|---------------------|-----------|
| [`post-cortex`](crates/post-cortex/) | The full stack in one dep | [![crates.io](https://img.shields.io/crates/v/post-cortex.svg)](https://crates.io/crates/post-cortex) |
| [`post-cortex-core`](crates/post-cortex-core/) | Domain types + traits only (no I/O, no ML) | [![crates.io](https://img.shields.io/crates/v/post-cortex-core.svg)](https://crates.io/crates/post-cortex-core) |
| [`post-cortex-proto`](crates/post-cortex-proto/) | gRPC wire types (client-side) | [![crates.io](https://img.shields.io/crates/v/post-cortex-proto.svg)](https://crates.io/crates/post-cortex-proto) |
| [`post-cortex-embeddings`](crates/post-cortex-embeddings/) | Model2Vec (default) + opt-in BERT embedders + vector DB (exact / HNSW) | [![crates.io](https://img.shields.io/crates/v/post-cortex-embeddings.svg)](https://crates.io/crates/post-cortex-embeddings) |
| [`post-cortex-storage`](crates/post-cortex-storage/) | RocksDB + SurrealDB backends | [![crates.io](https://img.shields.io/crates/v/post-cortex-storage.svg)](https://crates.io/crates/post-cortex-storage) |
| [`post-cortex-memory`](crates/post-cortex-memory/) | `ConversationMemorySystem` orchestrator | [![crates.io](https://img.shields.io/crates/v/post-cortex-memory.svg)](https://crates.io/crates/post-cortex-memory) |
| [`post-cortex-mcp`](crates/post-cortex-mcp/) | MCP tool functions (embed in any MCP host) | [![crates.io](https://img.shields.io/crates/v/post-cortex-mcp.svg)](https://crates.io/crates/post-cortex-mcp) |
| [`post-cortex-daemon`](crates/post-cortex-daemon/) | `pcx` CLI + rmcp/axum/tonic server | [![crates.io](https://img.shields.io/crates/v/post-cortex-daemon.svg)](https://crates.io/crates/post-cortex-daemon) |

Dependency graph (acyclic):

```text
post-cortex-proto ──► post-cortex-core ──► post-cortex-storage ◄── post-cortex-embeddings
                                                  │                        │
                                                  ▼                        │
                                           post-cortex-memory ◄────────────┘
                                                  │
                                                  ▼
                                           post-cortex-mcp
                                                  │
                                                  ▼
                                           post-cortex-daemon
                                                  │
                                                  ▼
                                           post-cortex (facade)
```

`A ──► B` means B depends on A. `post-cortex-embeddings` depends on no other workspace crate.

`post-cortex-core` carries no storage, ML or server dependencies (only `tonic` / `prost` through `post-cortex-proto`) — downstream Rust projects can take it for the type system alone without dragging in RocksDB, Model2Vec / Candle, or the server stack.

## Installation

```bash
# Cargo (any platform with Rust 1.95+) — ships the pcx CLI from crates.io
cargo install post-cortex-daemon --features "embeddings,surrealdb-storage"

# Homebrew (macOS / Linux)
brew install julymetodiev/tap/post-cortex

# Or download a prebuilt binary from a release
curl -L https://github.com/julymetodiev/post-cortex/releases/latest/download/pcx-aarch64-apple-darwin -o /usr/local/bin/pcx
chmod +x /usr/local/bin/pcx
```

### As a library

```toml
[dependencies]
# Everything in one dep — pulls the full stack including the daemon.
post-cortex = "0.4"

# Or pick the leanest crate for your use case:
# - post-cortex-core           — types + traits only (no I/O, no ML)
# - post-cortex-mcp            — MCP tool functions, embed in any MCP host
# - post-cortex-embeddings     — Model2Vec + vector DB, no orchestrator
# - post-cortex-memory         — ConversationMemorySystem + canonical service
```

## Quick Start

### 1. Configure MCP (once, globally)

```bash
# HTTP transport (recommended, requires daemon running)
claude mcp add --scope user --transport http post-cortex http://127.0.0.1:3737/mcp

# Or stdio transport (no daemon needed)
claude mcp add --scope user --transport stdio post-cortex -- pcx
```

This registers Post-Cortex for all projects on your machine.

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

Nine tools:

| Tool | Purpose |
|------|---------|
| `session` | Create / list / load / search / update_metadata / delete sessions |
| `update_conversation_context` | Store knowledge (qa, decisions, problems, code changes); pass an `updates` array for bulk mode. Requires typed `entities` + `relations` arrays (`minItems: 1`, since 0.3.0) so the entity graph is never silently empty |
| `semantic_search` | Embedding search across a session, a workspace, or globally; recency-bias tunable. Every scope returns the same result fields (0.4.0) |
| `get_structured_summary` | Get session overview (decisions, insights, entities) |
| `query_conversation_context` | Structured queries — entity relationships, keyword search, importance scoring |
| `assemble_context` | Graph-aware retrieval without embeddings — entities named in the query, one-hop graph expansion, impact analysis and recency, merged into one payload |
| `manage_workspace` | Organize sessions into workspaces |
| `manage_entity` | Delete entity / single update; cascades typed edges |
| `admin` | Health, vectorize-session, vectorize-stats, create-checkpoint |

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
| Vector Search | In-process: exact top-k up to 10 000 vectors, HNSW above | Same in-process index; vectors persisted in an `embedding` table with an HNSW index |

Configure in `~/.post-cortex/daemon.toml`. See [Daemon Mode docs](docs/DAEMON_MODE.md).

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PC_HOST` | 127.0.0.1 | Bind address |
| `PC_PORT` | 3737 | HTTP / MCP port |
| `PC_GRPC_PORT` | 3738 | gRPC port (`0` disables gRPC) |
| `PC_DATA_DIR` | ~/.post-cortex/data | Storage location |
| `PC_STORAGE_BACKEND` | rocksdb | `rocksdb` or `surrealdb` |
| `PC_SURREALDB_ENDPOINT` | unset | SurrealDB WebSocket endpoint, e.g. `ws://localhost:8000` |
| `PC_SURREALDB_USER` / `PC_SURREALDB_PASS` | unset | SurrealDB credentials (non-`root` users sign in at namespace level) |
| `PC_AUTH_TOKEN` | unset | Bearer token for HTTP (except `/health`) and gRPC |
| `PC_REDACT` | unset (on) | `0` / `false` / `no` / `off` disables redaction on write |
| `PC_ALLOWED_HOSTS` | loopback + `host` | Extra `Host` headers accepted on `/mcp` and `/api/*` (comma-separated `host` or `host:port`); set when binding to `0.0.0.0` — see [Allowed hosts](docs/DAEMON_MODE.md#allowed-hosts-dns-rebinding) |

## Development

```bash
# Build the workspace with the full feature set
cargo build --release --features "embeddings,surrealdb-storage"

# Run the unit-test suite (no network, no external state)
cargo test --workspace --lib

# Run the live E2E test against a real SurrealDB + HuggingFace Hub
# (ignored by default; needs POST_CORTEX_E2E_SURREAL_* env vars and
#  ~50 MB potion-multilingual-128M download on first run)
POST_CORTEX_E2E_SURREAL_USER=root \
POST_CORTEX_E2E_SURREAL_PASS=root  \
cargo test -p post-cortex --features "embeddings,surrealdb-storage" \
    --test integration_e2e_surrealdb -- --ignored --nocapture

# Benchmarks
cargo bench
```

### Supply-chain hygiene

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo deny check` — see [`deny.toml`](deny.toml)
- `cargo audit` — see [`.cargo/audit.toml`](.cargo/audit.toml)

CI enforces all four on every push to `main`.

## Release notes

The current release is `0.4.0` — bearer-token auth, write-path redaction
and a `Host` allowlist on the HTTP endpoints (RUSTSEC-2026-0189);
row-backed session storage; write-path, vector-search and read-path
performance work. BERT is now opt-in (`--features bert`).

Before upgrading:

- **Binding to `0.0.0.0` / `::`:** set `allowed_hosts` (or
  `PC_ALLOWED_HOSTS`) to the names and IPs clients put in their URL, e.g.
  `allowed_hosts = ["my-mac.tailnet.ts.net", "100.64.0.7"]`, or remote MCP
  and REST calls get `403`.
- **SurrealDB:** back up the database first — the schema change is
  forward-only and an older daemon will not start against it afterwards.
- **RocksDB:** data written by 0.4.0 is not readable by older binaries.

See [CHANGELOG.md](CHANGELOG.md) for the full diff and migration notes.

## License

MIT — see [LICENSE](LICENSE).
