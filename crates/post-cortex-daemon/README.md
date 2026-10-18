# post-cortex-daemon

[![Crates.io](https://img.shields.io/crates/v/post-cortex-daemon.svg)](https://crates.io/crates/post-cortex-daemon)
[![Docs.rs](https://docs.rs/post-cortex-daemon/badge.svg)](https://docs.rs/post-cortex-daemon)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

HTTP / gRPC / SSE / stdio daemon for [post-cortex](https://docs.rs/post-cortex). Ships the unified `pcx` CLI binary.

Hosts:
- the **rmcp Model Context Protocol** surface (streaming-HTTP at `/mcp` + stdio transports) with nine tools: `session`, `update_conversation_context`, `semantic_search`, `get_structured_summary`, `query_conversation_context`, `manage_workspace`, `assemble_context`, `manage_entity`, `admin`,
- the **tonic gRPC API** (canonical post-cortex wire surface),
- the **axum HTTP server** with REST endpoints (`/api/*`) for the `pcx` CLI client and the SSE broadcast plumbing,
- the **stdio bridge** that lets MCP hosts spawn the daemon as a child process,
- the unified **`pcx` CLI binary** (`[[bin]] name = "pcx"`).

The daemon owns no domain logic — every read / write / search / manage operation delegates downward into [`post-cortex-memory`](https://docs.rs/post-cortex-memory).

## Install

```sh
# Install the pcx CLI from crates.io
cargo install post-cortex-daemon
pcx --help
```

Or as a library dependency (e.g. to embed the daemon in your own bin):

```toml
[dependencies]
post-cortex-daemon = "0.4"
```

## Running the daemon

```sh
# Start the daemon (HTTP/MCP on 127.0.0.1:3737, gRPC on 127.0.0.1:3738 by default)
pcx start

# Status / stop
pcx status
pcx stop

# Scaffold Claude Code integration in the current repo
pcx setup
```

Configuration lives in `~/.post-cortex/daemon.toml`; environment variables override it, and `pcx start` flags override both.

Environment variables:

| Var | Default | Effect |
|-----|---------|--------|
| `PC_HOST` | `127.0.0.1` | Bind host (HTTP / SSE and gRPC) |
| `PC_PORT` | `3737` | HTTP / MCP / SSE port |
| `PC_GRPC_PORT` | `3738` | gRPC port (`0` disables gRPC) |
| `PC_DATA_DIR` | `~/.post-cortex/data` | RocksDB data directory |
| `PC_STORAGE_BACKEND` | `rocksdb` | `rocksdb` or `surrealdb` (needs the `surrealdb-storage` feature) |
| `PC_SURREALDB_ENDPOINT` | _unset_ | SurrealDB WebSocket endpoint, e.g. `ws://localhost:8000` |
| `PC_SURREALDB_USER` / `PC_SURREALDB_PASS` | _unset_ | SurrealDB credentials; a user other than `root` signs in at namespace level (namespace `post_cortex`, database `main` unless set in `daemon.toml`) |
| `PC_AUTH_TOKEN` | _unset_ | Bearer token required on every HTTP route except `/health` and on every gRPC call |
| `PC_ALLOWED_HOSTS` | loopback + `PC_HOST` | Extra `Host` headers (`host` or `host:port`, comma-separated) accepted on `/mcp` and `/api/*`; others get 403. Set it when binding to `0.0.0.0`. `*` disables the check |
| `PC_REDACT` | _unset_ (redaction on) | `0` / `false` / `no` / `off` disables redaction of credentials, e-mail addresses, phone numbers and public IPv4 addresses on write |
| `RUST_LOG` | `info` | tracing-subscriber filter |

When binding to `0.0.0.0` / `::` (tailnet, LAN), list the names and IPs clients put in their URL, or remote MCP and REST calls get `403 Forbidden: Host header is not allowed`:

```toml
# ~/.post-cortex/daemon.toml (`pcx init` writes a complete one;
# host, port and data_directory are required keys)
host = "0.0.0.0"
port = 3737
data_directory = "/home/me/.post-cortex/data"
auth_token = "change-me"
allowed_hosts = ["my-mac.tailnet.ts.net", "100.64.0.7"]
```

or, without a config file, `PC_HOST=0.0.0.0 PC_AUTH_TOKEN=change-me PC_ALLOWED_HOSTS=my-mac.tailnet.ts.net,100.64.0.7 pcx start`.

## Features

| Feature | Default | What it enables |
|---------|---------|-----------------|
| `embeddings` | yes | Semantic search and vectorization (Model2Vec `potion-multilingual-128M` by default) |
| `bert` | no | BERT embedding backend via `post-cortex-memory/bert` (Candle) |
| `surrealdb-storage` | no | SurrealDB backend (forwards to `post-cortex-memory`) |
| `surrealdb-tikv` | no | SurrealDB + TiKV distributed KV |
| `otel` | no | OpenTelemetry crates via `post-cortex-memory/otel`. The OTLP exporter is not wired yet: `observability::init` only reports whether `OTEL_EXPORTER_OTLP_ENDPOINT` is set |

## Example: observability init

See [`examples/with_otel.rs`](examples/with_otel.rs) for a small demo of `observability::init` (honours `RUST_LOG` and `OTEL_LOG_FORMAT` = `compact` / `pretty` / `json`).

## License

MIT — see [LICENSE](../../LICENSE).
