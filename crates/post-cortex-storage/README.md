# post-cortex-storage

[![Crates.io](https://img.shields.io/crates/v/post-cortex-storage.svg)](https://crates.io/crates/post-cortex-storage)
[![Docs.rs](https://docs.rs/post-cortex-storage/badge.svg)](https://docs.rs/post-cortex-storage)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Storage backends for [post-cortex](https://docs.rs/post-cortex). Provides the [`Storage`] trait plus the lock-free RocksDB backend ([`RealRocksDBStorage`]) and an optional SurrealDB backend behind the `surrealdb-storage` feature.

## Install

```toml
[dependencies]
post-cortex-storage = "0.2"                                          # RocksDB only
post-cortex-storage = { version = "0.2", features = ["surrealdb-storage"] }  # + SurrealDB
```

## Features

| Feature | Default | What it enables |
|---------|---------|-----------------|
| `surrealdb-storage` | no | SurrealDB backend (kv-mem + protocol-ws) |
| `surrealdb-tikv` | no | SurrealDB with TiKV distributed KV |
| `otel` | no | OpenTelemetry instrumentation hooks |

The RocksDB backend is always compiled — it's the post-cortex default and what the daemon ships with.

## Example

```rust,no_run
use post_cortex_storage::{RealRocksDBStorage, Storage};
use std::sync::Arc;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let storage = Arc::new(RealRocksDBStorage::new("./pcx-data")?);
// `Storage` is `dyn`-compatible — pass `Arc<dyn Storage>` to any consumer.
let session_count = storage.list_sessions().await?.len();
println!("{session_count} sessions persisted");
# Ok(()) }
```

## License

MIT — see [LICENSE](../../LICENSE).
