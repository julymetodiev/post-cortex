# post-cortex-embeddings

[![Crates.io](https://img.shields.io/crates/v/post-cortex-embeddings.svg)](https://crates.io/crates/post-cortex-embeddings)
[![Docs.rs](https://docs.rs/post-cortex-embeddings/badge.svg)](https://docs.rs/post-cortex-embeddings)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Embedding engines + HNSW vector database for [post-cortex](https://docs.rs/post-cortex).

Self-contained ML stack — anyone needing a Candle-backed BERT embedder or an HNSW index for nearest-neighbour search can depend on this crate without pulling the post-cortex daemon or orchestrator. Implements [`EmbeddingBackend`] (BERT via Candle, static-hash fallback) and ships [`VectorDB`] (HNSW with optional product quantisation).

## Install

```toml
[dependencies]
post-cortex-embeddings = "0.2"           # full BERT + HNSW (default)
# or, if you only need the data types (VectorMetadata, SearchMatch, ...):
post-cortex-embeddings = { version = "0.2", default-features = false }
```

## Features

| Feature | Default | What it enables |
|---------|---------|-----------------|
| `bert` | yes | Candle + Tokenizers + hf-hub for the BERT backend |
| `otel` | no | OpenTelemetry instrumentation hooks |

When `bert` is off the crate compiles only `vector_db::types` — useful for storage backends that need `VectorMetadata` round-tripping without pulling the ML stack.

## Example

```rust,no_run
use post_cortex_embeddings::{
    LocalEmbeddingEngine, EmbeddingConfig, VectorDB, VectorDbConfig, VectorMetadata,
};
use std::sync::Arc;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let engine = Arc::new(LocalEmbeddingEngine::new(EmbeddingConfig::default()).await?);
let db = Arc::new(VectorDB::new(VectorDbConfig::default()));

let vector = engine.embed("hello world").await?;
let meta = VectorMetadata::new(
    "doc-1".into(),
    "hello world".into(),
    "session-1".into(),
    "UpdateContent".into(),
);
db.add_vector(1, vector.clone(), meta).await?;

let hits = db.search(&vector, 5).await?;
for hit in hits {
    println!("{} (similarity {:.3})", hit.metadata.text, hit.similarity);
}
# Ok(()) }
```

## License

MIT — see [LICENSE](../../LICENSE).
