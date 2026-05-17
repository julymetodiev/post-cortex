# post-cortex-daemon

[![Crates.io](https://img.shields.io/crates/v/post-cortex-daemon.svg)](https://crates.io/crates/post-cortex-daemon)
[![Docs.rs](https://docs.rs/post-cortex-daemon/badge.svg)](https://docs.rs/post-cortex-daemon)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

HTTP / gRPC / SSE / stdio daemon for [post-cortex](https://docs.rs/post-cortex). Ships the unified `pcx` CLI binary.

Hosts:
- the **rmcp Model Context Protocol** surface (streaming-HTTP + stdio transports),
- the **tonic gRPC API** (canonical post-cortex wire surface),
- the **axum HTTP server** with REST endpoints for the `pcx` CLI client and the SSE broadcast plumbing,
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
post-cortex-daemon = "0.2"
```

## Running the daemon

```sh
# Start the daemon (binds to 127.0.0.1:51900 by default)
pcx start

# Status / stop
pcx status
pcx stop

# Scaffold Claude Code integration in the current repo
pcx setup
```

Environment variables:

| Var | Default | Effect |
|-----|---------|--------|
| `PC_HOST` | `127.0.0.1` | HTTP / SSE bind host |
| `PC_PORT` | `51900` | HTTP / SSE port |
| `PC_GRPC_PORT` | `51901` | gRPC port |
| `PC_DATA_DIR` | `~/.post-cortex/data` | RocksDB data directory |
| `RUST_LOG` | `info` | tracing-subscriber filter |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | _unset_ | OTLP collector endpoint (with `otel` feature) |

## Features

| Feature | Default | What it enables |
|---------|---------|-----------------|
| `embeddings` | yes | BERT + HNSW via `post-cortex-embeddings/bert` |
| `surrealdb-storage` | no | SurrealDB backend (forwards to `post-cortex-memory`) |
| `surrealdb-tikv` | no | SurrealDB + TiKV distributed KV |
| `otel` | no | OpenTelemetry OTLP exporter wiring |

## Example: daemon with OTel

See [`examples/with_otel.rs`](examples/with_otel.rs) for a runnable demo + local Jaeger collector docker-compose snippet.

## License

MIT — see [LICENSE](../../LICENSE).
