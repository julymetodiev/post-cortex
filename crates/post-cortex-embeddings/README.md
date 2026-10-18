# post-cortex-embeddings

[![Crates.io](https://img.shields.io/crates/v/post-cortex-embeddings.svg)](https://crates.io/crates/post-cortex-embeddings)
[![Docs.rs](https://docs.rs/post-cortex-embeddings/badge.svg)](https://docs.rs/post-cortex-embeddings)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)

Embedding engines + in-memory vector database for [post-cortex](https://docs.rs/post-cortex).

Self-contained ML stack — no dependency on any other post-cortex crate, so anyone needing a local text embedder or a nearest-neighbour index can depend on this crate without pulling the daemon or orchestrator.

- **`LocalEmbeddingEngine`** selects an `EmbeddingBackend` from `EmbeddingConfig::model_type`. The default is `PotionMultilingual` — Model2Vec static embeddings (`minishlab/potion-multilingual-128M`, 256 dimensions, multilingual, no GPU). `PotionCode` (`potion-code-16M`) is the other Model2Vec model. The BERT models (`MiniLM`, `MultilingualMiniLM`, `TinyBERT`, `BGESmall`, via Candle) need the opt-in `bert` feature. `StaticSimilarityMRL` is a hash-based fallback that does not produce semantic vectors.
- **`VectorDB`** is a DashMap-backed in-memory index with per-source (session) and per-content-id indexes. Up to `exact_search_threshold` vectors (default 10 000) searches are exact bounded top-k scans and no HNSW graph is kept; past it the HNSW graph is built once in bulk and maintained incrementally. `search_in_sources` always scans only the given sessions' vectors. Quantization and product quantization are off by default.

## Install

```toml
[dependencies]
post-cortex-embeddings = "0.4"                                     # Model2Vec + VectorDB (default)
post-cortex-embeddings = { version = "0.4", features = ["bert"] }  # + BERT via Candle
# or without any model backend (VectorDB, VectorMetadata, SearchMatch, hash fallback only):
post-cortex-embeddings = { version = "0.4", default-features = false }
```

## Features

| Feature | Default | What it enables |
|---------|---------|-----------------|
| `model2vec` | yes | `model2vec-rs` backend for the Potion models |
| `bert` | no | Candle + Tokenizers + hf-hub for the BERT backend |
| `otel` | no | Placeholder; currently enables nothing |

With both `model2vec` and `bert` off the crate still compiles `VectorDB` and its types, which is how `post-cortex-storage` uses it. Selecting a model whose backend is not compiled in fails at `LocalEmbeddingEngine::new` with an error naming the missing feature.

## Example

```rust,no_run
use post_cortex_embeddings::{
    EmbeddingConfig, LocalEmbeddingEngine, VectorDB, VectorDbConfig, VectorMetadata,
};

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Downloads potion-multilingual-128M (~50 MB) on first use.
    let engine = LocalEmbeddingEngine::new(EmbeddingConfig::default()).await?;

    // The index dimension must match the model (256 for the default model).
    let db = VectorDB::new(VectorDbConfig {
        dimension: engine.embedding_dimension(),
        ..VectorDbConfig::default()
    })?;

    let vector = engine.encode_text("hello world").await?;
    let meta = VectorMetadata::new(
        "doc-1".into(),       // id
        "hello world".into(), // text
        "session-1".into(),   // source (session id)
        "update".into(),      // content type
    );
    db.add_vector(vector.clone(), meta)?;

    for hit in db.search_in_sources(&vector, 5, &["session-1"])? {
        println!("{} (similarity {:.3})", hit.metadata.text, hit.similarity);
    }
    Ok(())
}
```

## License

MIT — see [LICENSE](../../LICENSE).
