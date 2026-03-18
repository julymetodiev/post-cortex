# GLiNER-RelEx NER Engine

Joint Named Entity Recognition + Relation Extraction engine for Post-Cortex, using the `knowledgator/gliner-relex-large-v0.5` model.

## Model

- **Architecture**: UniEncoderSpanRelexGLiNER (467.8M params)
- **Encoder**: DeBERTa-v3-large
- **Pipeline**: DeBERTa encoder → BiLSTM → SpanMarker → prompt MLP → entity scoring → GCN → pair MLP → relation scoring
- **ONNX backbone**: ~1.7 GB (`backbone.onnx`)

The backbone ONNX covers everything up to entity scoring + span representations + relation prompt embeddings. The remaining stages (GCN, pair building, pair MLP, relation scoring) run in Rust using `.npy` weight files.

## Required Files

Located at `$PCX_GLINER_MODEL/` or `~/.cache/gliner-relex-onnx/`:

```
backbone.onnx              # DeBERTa + BiLSTM + SpanMarker + prompt MLP (1.7 GB)
tokenizer.json             # HuggingFace tokenizer
tokenizer_config.json
config.json                # Model config (hidden_size, max_width, etc.)
weights/
  gcn.linear.weight.npy    # GCN layer weights
  gcn.linear.bias.npy
  gcn.proj.weight.npy      # GCN projection weights
  gcn.proj.bias.npy
  pair_rep.linear0.weight.npy  # Pair MLP first layer
  pair_rep.linear0.bias.npy
  pair_rep.linear1.weight.npy  # Pair MLP second layer
  pair_rep.linear1.bias.npy
```

## Export

Export script: `/Volumes/JuliusML/dev/JuliusML/Surge/experiments/gliner-bench/export_full_onnx.py`

```bash
cd /Volumes/JuliusML/dev/JuliusML/Surge/experiments/gliner-bench
.venv/bin/python3 export_full_onnx.py              # export
.venv/bin/python3 export_full_onnx.py --verify      # export + verify ONNX matches PyTorch
```

Verification confirms max score difference of 0.000001 between PyTorch and ONNX + manual pipeline.

## Thresholds

| Constant | Value | Purpose |
|----------|-------|---------|
| `ENTITY_THRESHOLD` | 0.7 | Min sigmoid score to accept an entity |
| `ADJACENCY_THRESHOLD` | 0.5 | Min adjacency score to consider an entity pair |
| `RELATION_THRESHOLD` | 0.7 | Min sigmoid score to accept a relation |

These match the Python benchmark (`bench_relex.py`) defaults.

## Special Tokens

| Token | ID | Usage |
|-------|------|-------|
| `[CLS]` / BOS | 1 | Begin of sequence |
| `[SEP]` / EOS | 2 | End of sequence |
| `<<ENT>>` | 128001 | Entity type marker |
| `<<SEP>>` | 128002 | Section separator |
| `<<REL>>` | 128003 | Relation type marker |

## Entity Labels (9 types)

| Label | Description |
|-------|-------------|
| `library` | Software library, crate, or reusable package (tokio, DashMap, numpy) |
| `framework` | Software framework (Django, Rails, axum, PyTorch) |
| `language` | Programming or query language (Rust, Python, SQL) |
| `database` | Database system (PostgreSQL, Redis, RocksDB, SurrealDB) |
| `protocol` | Network protocol or API standard (gRPC, HTTP, REST, MCP) |
| `person` | Person name |
| `model` | Pretrained AI/ML model (BERT, GPT-4, GLiNER, DeBERTa) |
| `algorithm` | Algorithm or data structure (HNSW, B-tree, PageRank) |
| `tool` | Software tool or infrastructure (Docker, Nginx, Kubernetes, Git) |

## Relation Labels (11 types)

`built with`, `uses`, `created by`, `replaced by`, `alternative to`, `based on`, `connects to`, `part of`, `depends on`, `required by`, `implements`

## Input Prompt Format

```
[CLS] <<ENT>> library <<ENT>> framework <<ENT>> language ... <<SEP>> <<REL>> built with <<REL>> uses ... <<SEP>> word1 word2 word3 ... [SEP]
```

- Entity and relation labels are specified at inference time (zero-shot)
- Only label keys go into the prompt, not descriptions
- `words_mask` is **1-indexed** — text words get values 1, 2, 3, ...; prompt words get 0

## Inference Pipeline (Rust)

```
1. tokenize_text(text)          → whitespace word tokens
2. tokenize_prompt(tokens)      → input_ids + words_mask positions
3. Build words_mask              → [0..0, 1, 0, 2, 0, 3, ...] (1-indexed, text words only)
4. Build text_lengths            → [num_words]
5. Build span_idx + span_mask    → all (start, end) pairs up to MAX_WIDTH=12
6. Run backbone.onnx             → entity_logits [B,L,K,C], span_reps [B,L*K,D], rel_prompt_embeds [B,R,D]
7. Sigmoid + NMS                 → entity extraction (threshold 0.7)
8. Gather entity span_reps       → entity representations
9. GCN forward                   → adjacency matrix (threshold 0.5)
10. Build entity pairs           → (head, tail) where adj > threshold
11. Pair MLP                     → pair representations
12. Dot product + sigmoid        → relation scores (threshold 0.7)
```

## Public API

```rust
impl NEREngine {
    pub fn new() -> Self;
    pub async fn load_model(&mut self) -> Result<()>;
    pub fn extract_entities(&self, text: &str) -> Result<Vec<RecognizedEntity>>;
    pub fn extract_entities_and_relations(&self, text: &str)
        -> Result<(Vec<RecognizedEntity>, Vec<RecognizedRelation>)>;
    pub fn extract_for_graph(&self, text: &str)
        -> Result<(Vec<(String, EntityType)>, Vec<EntityRelationship>)>;
    pub fn clear_cache(&self);
}
```

## Global Instance

```rust
// src/session/active_session.rs
static GLOBAL_NER_ENGINE: OnceLock<Arc<NEREngine>>;

pub async fn preload_ner_engine() -> bool;  // call during daemon startup
```

Thread-safe via `Arc<NEREngine>` + `DashMap` cache (lock-free, max 1000 entries).

## Feature Flag

Gated behind `--features embeddings` in Cargo.toml:
```toml
embeddings = ["candle-core", "candle-nn", "candle-transformers",
              "tokenizers", "hf-hub", "dep:ort", "dep:ndarray", "dep:ndarray-npy"]
```

## Key Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `ort` | 2.0.0-rc.11 | ONNX Runtime bindings |
| `tokenizers` | 0.22 | HuggingFace tokenizer |
| `ndarray` | 0.17.2 | Matrix operations for GCN/MLP |
| `ndarray-npy` | 0.10.0 | Load .npy weight files |

## Source Files

| File | Purpose |
|------|---------|
| `src/core/ner_engine.rs` | NER engine implementation (1820 lines) |
| `src/session/active_session.rs` | Global engine singleton + integration |
| `tests/integration_ner.rs` | NER extraction tests |
| `tests/integration_relex_graph.rs` | End-to-end NER → entity graph tests |

## Python Benchmark

```bash
cd /Volumes/JuliusML/dev/JuliusML/Surge/experiments/gliner-bench
.venv/bin/python3 bench_relex.py                     # full benchmark (12 texts)
.venv/bin/python3 bench_relex.py --threshold 0.8      # custom entity threshold
.venv/bin/python3 bench_relex.py --no-filter           # disable post-filter
```

## Known Limitations

- Short single-sentence texts produce lower scores than multi-sentence paragraphs (less context for the model)
- Entity type classification can be imprecise for domain-specific terms (e.g. "HNSW" classified as Protocol instead of Algorithm)
- Full ONNX export (including GCN + pair pipeline) is impossible due to dynamic tensor indexing in `build_entity_pairs`
- Backbone ONNX is ~1.7 GB — significant memory footprint
