# post-cortex-storage

[![Crates.io](https://img.shields.io/crates/v/post-cortex-storage.svg)](https://crates.io/crates/post-cortex-storage)
[![Docs.rs](https://docs.rs/post-cortex-storage/badge.svg)](https://docs.rs/post-cortex-storage)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Storage backends for [post-cortex](https://docs.rs/post-cortex). Provides the `Storage` trait (plus `VectorStorage` / `GraphStorage`), the lock-free RocksDB backend (`RealRocksDBStorage`) and an optional SurrealDB backend (`SurrealDBStorage`) behind the `surrealdb-storage` feature.

Session blobs and their context updates are stored as separate rows (RocksDB keeps a `session_updates:{id}` index; blobs written by 0.3.x are migrated on load). Removals are passed explicitly through `Storage::save_session_with_changes`, and `Storage::list_session_summaries` lists sessions without loading them. Data written by 0.4 is not readable by older versions.

## Install

```toml
[dependencies]
post-cortex-storage = "0.4"                                          # RocksDB only
post-cortex-storage = { version = "0.4", features = ["surrealdb-storage"] }  # + SurrealDB
```

## Features

| Feature | Default | What it enables |
|---------|---------|-----------------|
| `surrealdb-storage` | no | SurrealDB backend (SurrealDB SDK 3.3, `kv-mem` + `protocol-ws`) |
| `surrealdb-tikv` | no | SurrealDB with TiKV distributed KV (implies `surrealdb-storage`) |
| `otel` | no | Placeholder; currently enables nothing |

The RocksDB backend is always compiled — it's the post-cortex default and what the daemon ships with.

The SurrealDB schema changed in 0.4.0 (embedding records keyed `{session_id}_{content_id}`, unique `(session_id, content_id)` index). The change is forward-only: back up the database before the first start, an older daemon will not start against it afterwards.

## Example

```rust,no_run
use post_cortex_storage::{RealRocksDBStorage, Storage};
use std::sync::Arc;

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // `new_with_dimension` rejects embeddings whose length does not match the
    // model (256 for the default potion-multilingual-128M); `new` accepts any.
    let storage: Arc<dyn Storage> =
        Arc::new(RealRocksDBStorage::new_with_dimension("./pcx-data", 256).await?);

    for summary in storage.list_session_summaries().await? {
        println!("{} {:?} ({} updates)", summary.id, summary.name, summary.update_count);
    }
    Ok(())
}
```

## License

MIT — see [LICENSE](../../LICENSE).
