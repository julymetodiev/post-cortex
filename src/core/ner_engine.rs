// Copyright (c) 2026 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! GLiNER-RelEx ONNX Named Entity Recognition + Relation Extraction Engine
//!
//! Implements joint NER and relation extraction using the
//! knowledgator/gliner-relex-large-v0.5 model (UniEncoderSpanRelexGLiNER,
//! 467.8M params, DeBERTa-v3-large).
//!
//! Pipeline: DeBERTa encoder → BiLSTM → SpanMarkerV0 → GCN → pair MLP
//! Entity types and relation labels are specified at inference time.

#[cfg(feature = "embeddings")]
use anyhow::Result;
#[cfg(feature = "embeddings")]
use dashmap::DashMap;
#[cfg(feature = "embeddings")]
use ndarray::{Array1, Array2, s};
#[cfg(feature = "embeddings")]
use ndarray_npy::read_npy;
#[cfg(feature = "embeddings")]
use ort::session::Session;
#[cfg(feature = "embeddings")]
use ort::value::TensorRef;
#[cfg(feature = "embeddings")]
use parking_lot::Mutex;
#[cfg(feature = "embeddings")]
use regex::Regex;
#[cfg(feature = "embeddings")]
use std::sync::Arc;
#[cfg(feature = "embeddings")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "embeddings")]
use tokenizers::Tokenizer;
#[cfg(feature = "embeddings")]
use tracing::{debug, info};

// ─── Entity types ────────────────────────────────────────────────────────

/// Software entity types recognized by GLiNER-RelEx
#[cfg(feature = "embeddings")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntityType {
    Library,
    Framework,
    API,
    DataStructure,
    Algorithm,
    Protocol,
    Language,
    Tool,
    Function,
    Class,
    Person,
    Organization,
    Location,
    Miscellaneous,
    // New variants for RelEx schema
    Model,
    Database,
    Dataset,
}

#[cfg(feature = "embeddings")]
impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Library => write!(f, "Library"),
            Self::Framework => write!(f, "Framework"),
            Self::API => write!(f, "API"),
            Self::DataStructure => write!(f, "DataStructure"),
            Self::Algorithm => write!(f, "Algorithm"),
            Self::Protocol => write!(f, "Protocol"),
            Self::Language => write!(f, "Language"),
            Self::Tool => write!(f, "Tool"),
            Self::Function => write!(f, "Function"),
            Self::Class => write!(f, "Class"),
            Self::Person => write!(f, "Person"),
            Self::Organization => write!(f, "Organization"),
            Self::Location => write!(f, "Location"),
            Self::Miscellaneous => write!(f, "Miscellaneous"),
            Self::Model => write!(f, "Model"),
            Self::Database => write!(f, "Database"),
            Self::Dataset => write!(f, "Dataset"),
        }
    }
}

#[cfg(feature = "embeddings")]
impl EntityType {
    fn from_label(label: &str) -> Self {
        match label {
            "library" => Self::Library,
            "framework" => Self::Framework,
            "api" => Self::API,
            "data_structure" => Self::DataStructure,
            "algorithm" => Self::Algorithm,
            "protocol" => Self::Protocol,
            "language" => Self::Language,
            "tool" => Self::Tool,
            "function" => Self::Function,
            "class" => Self::Class,
            "person" => Self::Person,
            "organization" => Self::Organization,
            "location" => Self::Location,
            "model" => Self::Model,
            "database" => Self::Database,
            "dataset" => Self::Dataset,
            _ => Self::Miscellaneous,
        }
    }

    fn as_label(&self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Framework => "framework",
            Self::API => "api",
            Self::DataStructure => "data_structure",
            Self::Algorithm => "algorithm",
            Self::Protocol => "protocol",
            Self::Language => "language",
            Self::Tool => "tool",
            Self::Function => "function",
            Self::Class => "class",
            Self::Person => "person",
            Self::Organization => "organization",
            Self::Location => "location",
            Self::Model => "model",
            Self::Database => "database",
            Self::Dataset => "dataset",
            Self::Miscellaneous => "miscellaneous",
        }
    }

    /// Convert BIO tag to entity type (backwards compat for old callers)
    pub fn from_bio_tag(tag: &str) -> Option<Self> {
        match tag {
            t if t.contains("PER") => Some(Self::Person),
            t if t.contains("ORG") => Some(Self::Organization),
            t if t.contains("LOC") => Some(Self::Location),
            t if t.contains("MISC") => Some(Self::Miscellaneous),
            _ => None,
        }
    }
}

// ─── Public output types ─────────────────────────────────────────────────

/// Recognized entity with position and confidence
#[cfg(feature = "embeddings")]
#[derive(Debug, Clone)]
pub struct RecognizedEntity {
    pub text: String,
    pub entity_type: EntityType,
    pub confidence: f32,
    pub start: usize,
    pub end: usize,
}

/// Recognized relation between two entities
#[cfg(feature = "embeddings")]
#[derive(Debug, Clone)]
pub struct RecognizedRelation {
    pub head: RecognizedEntity,
    pub tail: RecognizedEntity,
    pub relation_type: String,
    pub confidence: f32,
}

// ─── Label schemas ───────────────────────────────────────────────────────

/// Entity labels with descriptions for RelEx (dict format improves typing accuracy).
/// Each entry is (label_key, description).
#[cfg(feature = "embeddings")]
const ENTITY_LABELS: &[(&str, &str)] = &[
    (
        "library",
        "a specific named software library, crate, or reusable package such as tokio, DashMap, React, lodash, numpy",
    ),
    (
        "framework",
        "a specific named software framework such as Django, Rails, Next.js, axum, PyTorch, TensorFlow",
    ),
    (
        "language",
        "a programming or query language such as Rust, Python, JavaScript, SQL",
    ),
    (
        "database",
        "a specific named database system such as PostgreSQL, Redis, RocksDB, SurrealDB, sled",
    ),
    (
        "protocol",
        "a named network protocol or API standard such as gRPC, HTTP, REST, MCP, WebSocket",
    ),
    ("person", "a person name"),
    (
        "model",
        "a specific named pretrained AI or ML model such as BERT, DistilBERT, GPT-4, GLiNER, DeBERTa, Whisper, CLIP",
    ),
    (
        "algorithm",
        "a specific named algorithm or data structure such as HNSW, B-tree, PageRank, binary search, bloom filter",
    ),
    (
        "tool",
        "a specific named software tool, server, container, or infrastructure platform such as Docker, Nginx, Kubernetes, Git",
    ),
];

#[cfg(feature = "embeddings")]
const RELATION_LABELS: &[&str] = &[
    "built with",
    "uses",
    "created by",
    "replaced by",
    "alternative to",
    "based on",
    "connects to",
    "part of",
];

// Special token IDs for the RelEx tokenizer
#[cfg(feature = "embeddings")]
const TOK_BOS: u32 = 1;
#[cfg(feature = "embeddings")]
const TOK_EOS: u32 = 2;
#[cfg(feature = "embeddings")]
const TOK_ENT: u32 = 128001; // <<ENT>>
#[cfg(feature = "embeddings")]
const TOK_SEP: u32 = 128002; // <<SEP>>
#[cfg(feature = "embeddings")]
const TOK_REL: u32 = 128003; // <<REL>>

#[cfg(feature = "embeddings")]
const MAX_WIDTH: usize = 12;
#[cfg(feature = "embeddings")]
const ENTITY_THRESHOLD: f32 = 0.6;
#[cfg(feature = "embeddings")]
const ADJACENCY_THRESHOLD: f32 = 0.5;
#[cfg(feature = "embeddings")]
const RELATION_THRESHOLD: f32 = 0.85;
#[cfg(feature = "embeddings")]
const NER_CACHE_MAX_SIZE: usize = 1000;

// ─── Internal weight layer types ────────────────────────────────────────

#[cfg(feature = "embeddings")]
struct Linear {
    weight: Array2<f32>,
    bias: Array1<f32>,
}

#[cfg(feature = "embeddings")]
impl Linear {
    fn load(dir: &str, prefix: &str) -> Result<Self> {
        let weight: Array2<f32> = read_npy(format!("{dir}/{prefix}.weight.npy"))
            .map_err(|e| anyhow::anyhow!("load weight {prefix}: {e}"))?;
        let bias: Array1<f32> = read_npy(format!("{dir}/{prefix}.bias.npy"))
            .map_err(|e| anyhow::anyhow!("load bias {prefix}: {e}"))?;
        Ok(Self { weight, bias })
    }

    fn forward(&self, x: &Array2<f32>) -> Array2<f32> {
        x.dot(&self.weight.t()) + &self.bias
    }
}

#[cfg(feature = "embeddings")]
struct Mlp {
    linear1: Linear,
    linear2: Linear,
}

#[cfg(feature = "embeddings")]
impl Mlp {
    /// Two-layer MLP with ReLU activation between layers.
    fn forward(&self, x: &Array2<f32>) -> Array2<f32> {
        let h = self.linear1.forward(x).mapv(|v| v.max(0.0));
        self.linear2.forward(&h)
    }
}

/// SpanMarkerV0: projects word embeddings into span representations by
/// concatenating projected start and end word embeddings, then applying ReLU
/// and a final output projection.
#[cfg(feature = "embeddings")]
struct SpanMarkerV0 {
    project_start: Mlp,
    project_end: Mlp,
    out_project: Mlp,
}

#[cfg(feature = "embeddings")]
impl SpanMarkerV0 {
    fn load(dir: &str) -> Result<Self> {
        Ok(Self {
            project_start: Mlp {
                linear1: Linear::load(dir, "span.project_start.linear0")?,
                linear2: Linear::load(dir, "span.project_start.linear1")?,
            },
            project_end: Mlp {
                linear1: Linear::load(dir, "span.project_end.linear0")?,
                linear2: Linear::load(dir, "span.project_end.linear1")?,
            },
            out_project: Mlp {
                linear1: Linear::load(dir, "span.out_project.linear0")?,
                linear2: Linear::load(dir, "span.out_project.linear1")?,
            },
        })
    }

    /// Returns (span_reps [W*max_width, hidden], valid_mask [W*max_width]).
    fn forward(&self, word_embs: &Array2<f32>, max_width: usize) -> (Array2<f32>, Vec<bool>) {
        let num_words = word_embs.nrows();
        let hidden = word_embs.ncols();

        let start_proj = self.project_start.forward(word_embs);
        let end_proj = self.project_end.forward(word_embs);

        let num_spans = num_words * max_width;
        let mut concat = Array2::<f32>::zeros((num_spans, hidden * 2));
        let mut valid = vec![false; num_spans];

        for start in 0..num_words {
            for w in 0..max_width {
                let end = start + w;
                let idx = start * max_width + w;
                if end < num_words {
                    concat
                        .slice_mut(s![idx, ..hidden])
                        .assign(&start_proj.row(start));
                    concat
                        .slice_mut(s![idx, hidden..])
                        .assign(&end_proj.row(end));
                    valid[idx] = true;
                }
            }
        }

        // ReLU before out_project (as in the Python reference)
        concat.mapv_inplace(|v| v.max(0.0));
        let span_rep = self.out_project.forward(&concat);
        (span_rep, valid)
    }
}

/// GCN (Graph Convolutional Network) for modeling entity interactions.
/// Takes entity span representations and produces a refined adjacency matrix.
#[cfg(feature = "embeddings")]
struct Gcn {
    linear: Linear,
    proj: Linear,
}

#[cfg(feature = "embeddings")]
impl Gcn {
    fn load(dir: &str) -> Result<Self> {
        Ok(Self {
            linear: Linear::load(dir, "gcn.linear")?,
            proj: Linear::load(dir, "gcn.proj")?,
        })
    }

    /// Returns the final adjacency matrix (E, E) over the entity span reps.
    fn forward(&self, span_reps: &Array2<f32>) -> Array2<f32> {
        let e = span_reps.nrows();

        // Initial adjacency: A0[i,j] = sigmoid(rep[i] · rep[j])
        let dot = span_reps.dot(&span_reps.t());
        let a0 = dot.mapv(|v| sigmoid(v));

        // Add self-loops
        let mut a = a0;
        for i in 0..e {
            a[[i, i]] += 1.0;
        }

        // Symmetric normalization: D^{-1/2} A D^{-1/2}
        let degree: Vec<f32> = (0..e).map(|i| a.row(i).sum()).collect();
        let d_inv_sqrt: Vec<f32> = degree
            .iter()
            .map(|&d| if d > 1e-8 { 1.0 / d.sqrt() } else { 0.0 })
            .collect();

        // a_norm = D^{-1/2} @ A @ D^{-1/2}
        let mut a_norm = Array2::<f32>::zeros((e, e));
        for i in 0..e {
            for j in 0..e {
                a_norm[[i, j]] = d_inv_sqrt[i] * a[[i, j]] * d_inv_sqrt[j];
            }
        }

        // H = ReLU(A_norm @ span_reps @ linear.W^T + linear.bias)
        let aggr = a_norm.dot(span_reps);
        let h = self.linear.forward(&aggr).mapv(|v| v.max(0.0));

        // proj_H
        let proj_h = self.proj.forward(&h);

        // Final adjacency: sigmoid(proj_H @ proj_H^T)
        let dot2 = proj_h.dot(&proj_h.t());
        dot2.mapv(|v| sigmoid(v))
    }
}

// ─── Tokenization ────────────────────────────────────────────────────────

#[cfg(feature = "embeddings")]
struct TextToken {
    text: String,
    start: usize,
    end: usize,
}

#[cfg(feature = "embeddings")]
fn tokenize_text(text: &str) -> Vec<TextToken> {
    let re = Regex::new(
        r"(?i)(?:https?://[^\s]+|www\.[^\s]+)|[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}|@[a-z0-9_]+|\w+(?:[-_]\w+)*|\S",
    ).unwrap();
    let lower = text.to_lowercase();
    re.find_iter(&lower)
        .map(|m| TextToken {
            text: m.as_str().to_string(),
            start: m.start(),
            end: m.end(),
        })
        .collect()
}

/// Tokenized input for the RelEx model.
///
/// Format: [BOS] <<ENT>> label_desc <<ENT>> label_desc ... <<SEP>>
///         <<REL>> rel1 <<REL>> rel2 ... <<SEP>> word1 word2 ... [EOS]
#[cfg(feature = "embeddings")]
struct TokenizedInput {
    input_ids: Vec<u32>,
    /// Index into input_ids for each <<ENT>> marker token position.
    ent_marker_positions: Vec<usize>,
    /// Index into input_ids for each <<REL>> marker token position.
    rel_marker_positions: Vec<usize>,
    /// For each token in the text section: (token_idx_in_input_ids, word_idx).
    /// Only the first subword token per word is used for first-subword pooling.
    word_first_subword: Vec<usize>,
    /// Original whitespace-tokenized text words.
    text_tokens: Vec<TextToken>,
}

#[cfg(feature = "embeddings")]
fn tokenize_prompt(tokenizer: &Tokenizer, text_tokens: Vec<TextToken>) -> TokenizedInput {
    let mut input_ids: Vec<u32> = Vec::new();
    let mut ent_marker_positions: Vec<usize> = Vec::new();
    let mut rel_marker_positions: Vec<usize> = Vec::new();

    // BOS
    input_ids.push(TOK_BOS);

    // Entity section: <<ENT>> label: description for each entity type
    for (label, description) in ENTITY_LABELS {
        // <<ENT>> marker
        ent_marker_positions.push(input_ids.len());
        input_ids.push(TOK_ENT);

        // Tokenize "label: description"
        let label_text = format!("{}: {}", label, description);
        let enc = tokenizer
            .encode(label_text.as_str(), false)
            .expect("tokenize entity label");
        for &id in enc.get_ids() {
            input_ids.push(id);
        }
    }

    // <<SEP>> between entity section and relation section
    input_ids.push(TOK_SEP);

    // Relation section: <<REL>> relation_label for each relation type
    for rel in RELATION_LABELS {
        // <<REL>> marker
        rel_marker_positions.push(input_ids.len());
        input_ids.push(TOK_REL);

        // Tokenize the relation label text
        let enc = tokenizer
            .encode(*rel, false)
            .expect("tokenize relation label");
        for &id in enc.get_ids() {
            input_ids.push(id);
        }
    }

    // <<SEP>> between relation section and text section
    input_ids.push(TOK_SEP);

    // Text section: first-subword pooling — track the index of the first
    // subword token for each whitespace word.
    let mut word_first_subword: Vec<usize> = Vec::with_capacity(text_tokens.len());
    for text_tok in &text_tokens {
        let word_start_idx = input_ids.len();
        let enc = tokenizer
            .encode(text_tok.text.as_str(), false)
            .expect("tokenize text token");
        for &id in enc.get_ids() {
            input_ids.push(id);
        }
        // Record position of the first subword for this word
        word_first_subword.push(word_start_idx);
    }

    // EOS
    input_ids.push(TOK_EOS);

    TokenizedInput {
        input_ids,
        ent_marker_positions,
        rel_marker_positions,
        word_first_subword,
        text_tokens,
    }
}

// ─── Embedding extraction ────────────────────────────────────────────────

#[cfg(feature = "embeddings")]
struct ExtractedEmbeddings {
    /// Word embeddings from first-subword pooling: (W, 768)
    word_embeddings: Array2<f32>,
    /// Entity prompt embeddings at <<ENT>> positions: (C_ent, 768)
    ent_prompt_embeddings: Array2<f32>,
    /// Relation prompt embeddings at <<REL>> positions: (C_rel, 768)
    rel_prompt_embeddings: Array2<f32>,
}

#[cfg(feature = "embeddings")]
fn extract_embeddings(hidden_states: &Array2<f32>, input: &TokenizedInput) -> ExtractedEmbeddings {
    let hidden_size = hidden_states.ncols();
    let num_words = input.text_tokens.len();
    let c_ent = input.ent_marker_positions.len();
    let c_rel = input.rel_marker_positions.len();

    // Word embeddings: first subword per word
    let mut word_embs = Array2::<f32>::zeros((num_words, hidden_size));
    for (word_idx, &tok_pos) in input.word_first_subword.iter().enumerate() {
        if tok_pos < hidden_states.nrows() {
            word_embs
                .row_mut(word_idx)
                .assign(&hidden_states.row(tok_pos));
        }
    }

    // Entity prompt embeddings at <<ENT>> marker positions
    let mut ent_embs = Array2::<f32>::zeros((c_ent, hidden_size));
    for (i, &pos) in input.ent_marker_positions.iter().enumerate() {
        if pos < hidden_states.nrows() {
            ent_embs.row_mut(i).assign(&hidden_states.row(pos));
        }
    }

    // Relation prompt embeddings at <<REL>> marker positions
    let mut rel_embs = Array2::<f32>::zeros((c_rel, hidden_size));
    for (i, &pos) in input.rel_marker_positions.iter().enumerate() {
        if pos < hidden_states.nrows() {
            rel_embs.row_mut(i).assign(&hidden_states.row(pos));
        }
    }

    ExtractedEmbeddings {
        word_embeddings: word_embs,
        ent_prompt_embeddings: ent_embs,
        rel_prompt_embeddings: rel_embs,
    }
}

// ─── Math helpers ────────────────────────────────────────────────────────

#[cfg(feature = "embeddings")]
#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[cfg(feature = "embeddings")]
fn score_spans(span_rep: &Array2<f32>, prompt_rep: &Array2<f32>) -> Array2<f32> {
    span_rep.dot(&prompt_rep.t()).mapv(sigmoid)
}

// ─── NMS entity extraction ───────────────────────────────────────────────

#[cfg(feature = "embeddings")]
fn extract_entities_from_scores(
    scores: &Array2<f32>,
    valid_mask: &[bool],
    text_tokens: &[TextToken],
    original_text: &str,
    threshold: f32,
) -> Vec<RecognizedEntity> {
    let num_words = text_tokens.len();
    let num_fields = ENTITY_LABELS.len();
    let mut all_spans: Vec<RecognizedEntity> = Vec::new();

    for field_idx in 0..num_fields {
        let mut field_spans: Vec<RecognizedEntity> = Vec::new();

        for start in 0..num_words {
            for w in 0..MAX_WIDTH {
                let end_word = start + w;
                let span_idx = start * MAX_WIDTH + w;

                if end_word >= num_words || !valid_mask[span_idx] {
                    continue;
                }

                let conf = scores[[span_idx, field_idx]];
                if conf < threshold {
                    continue;
                }

                let char_start = text_tokens[start].start;
                let char_end = text_tokens[end_word].end;

                if char_end > original_text.len() {
                    continue;
                }

                let span_text = &original_text[char_start..char_end];
                if span_text.trim().is_empty() {
                    continue;
                }

                field_spans.push(RecognizedEntity {
                    text: span_text.trim().to_string(),
                    entity_type: EntityType::from_label(ENTITY_LABELS[field_idx].0),
                    confidence: conf,
                    start: char_start,
                    end: char_end,
                });
            }
        }

        // Sort by confidence descending, then apply greedy NMS within field
        field_spans.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());
        let mut selected: Vec<RecognizedEntity> = Vec::new();
        for span in field_spans {
            let overlaps = selected
                .iter()
                .any(|s| !(span.end <= s.start || span.start >= s.end));
            if !overlaps {
                selected.push(span);
            }
        }

        all_spans.extend(selected);
    }

    all_spans.sort_by_key(|s| s.start);
    all_spans
}

// ─── Post-filter ─────────────────────────────────────────────────────────

/// Generic phrases that are descriptions, not named entities.
#[cfg(feature = "embeddings")]
const GENERIC_PHRASES: &[&str] = &[
    // ML/AI generic
    "embedding model",
    "language model",
    "pretrained model",
    "base model",
    "ml model",
    "ai model",
    "neural network",
    "deep learning",
    "llm inference",
    "model inference",
    "vector search",
    "similarity search",
    "coding agent",
    "chat model",
    "fine-tuned model",
    // Infrastructure generic
    "write-ahead log",
    "session data",
    "entity ids",
    "forward slashes",
    "async context",
    "blocking call",
    "terminal rendering",
    "reverse proxy",
    "lock-free concurrency",
    "persistent storage",
    "key-value database",
    "custom entity types",
    "inference time",
    "named entity recognition",
    "conversation memory",
    "memory system",
    "async runtime",
    // Compound nouns that are not specific products
    "http api",
    "rest api",
    "web server",
    "web framework",
    "query language",
    "query builder",
    "query engine",
    "build tool",
    "build system",
    "package manager",
    "text editor",
    "code editor",
    "file system",
    "task runner",
    "test runner",
    "test framework",
    "config file",
    "log file",
    "data file",
    "api endpoint",
    "api server",
    "api client",
    "database system",
    "database engine",
    "storage engine",
    "token classification",
    "text classification",
    "entity recognition",
    "relation extraction",
    "persistent memory storage",
];

/// Multi-word generic suffixes — phrases ending with these are descriptions.
#[cfg(feature = "embeddings")]
const GENERIC_SUFFIXES: &[&str] = &[
    " model",
    " inference",
    " search",
    " storage",
    " context",
    " rendering",
    " concurrency",
    " log",
    " data",
    " call",
    " types",
    " time",
    " recognition",
    " memory",
    " system",
    " proxy",
    " database",
    " runtime",
    " engine",
    " handler",
    " manager",
    " server",
    " client",
    " layer",
    " module",
    " component",
    " service",
    " worker",
    " queue",
    " pool",
    " cache",
    " buffer",
    " stream",
    " pipe",
    " socket",
    " thread",
    " process",
    " task",
    " job",
    " event",
    " listener",
    " watcher",
    " observer",
    " middleware",
    " endpoint",
    " interface",
    " binding",
    " wrapper",
    " controller",
    " router",
    " resolver",
    " loader",
    " parser",
    " compiler",
    " linker",
    " builder",
    " factory",
    " adapter",
    " bridge",
    " decorator",
];

/// Multi-word generic prefixes — phrases starting with these are descriptions.
#[cfg(feature = "embeddings")]
const GENERIC_PREFIXES: &[&str] = &[
    "the ",
    "a ",
    "an ",
    "this ",
    "that ",
    "our ",
    "my ",
    "new ",
    "old ",
    "custom ",
    "internal ",
    "external ",
    "main ",
    "primary ",
    "default ",
    "simple ",
    "basic ",
    "raw ",
    "local ",
    "remote ",
    "async ",
    "sync ",
];

/// Known type corrections for systematic model misclassifications.
/// Key is lowercased entity text.
#[cfg(feature = "embeddings")]
const TYPE_CORRECTIONS: &[(&str, &str)] = &[
    // Datasets misclassified as language/protocol
    ("conll", "dataset"),
    ("imagenet", "dataset"),
    ("glue", "dataset"),
    ("squad", "dataset"),
    ("coco", "dataset"),
    ("wikitext", "dataset"),
    // Tools misclassified
    ("docker", "tool"),
    ("nginx", "tool"),
    ("git", "tool"),
    ("kubernetes", "tool"),
    ("k8s", "tool"),
    // Framework misclassified as model
    ("axon", "framework"),
    // Languages misclassified
    ("node.js", "language"),
    ("surrealql", "language"),
];

/// Returns true if the entity text looks like a genuine named entity
/// (not a code fragment, generic phrase, or internal identifier).
#[cfg(feature = "embeddings")]
fn is_valid_entity(text: &str) -> bool {
    let t = text.trim();

    // Length bounds
    if t.len() < 2 || t.len() > 40 {
        return false;
    }

    // Code fragments: double underscores, brackets, Rust paths, snake_case, method calls
    if t.contains("__") {
        return false;
    }
    if t.contains("::") {
        return false;
    }
    if t.contains('(') || t.contains(')') || t.contains('{') || t.contains('[') {
        return false;
    }
    if t.contains('/') {
        return false;
    }
    // snake_case identifiers (all-lowercase with underscores)
    if t.contains('_') && t == t.to_lowercase() && !t.contains(' ') {
        return false;
    }
    // Method call patterns: .method(
    if t.contains(".(") {
        return false;
    }
    // db. / self. prefixes
    let lower = t.to_lowercase();
    if lower.starts_with("db.") || lower.starts_with("self.") {
        return false;
    }
    // Reference/pointer syntax
    if t.starts_with('&') || t.starts_with('*') {
        return false;
    }
    // Pure numbers
    if t.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    // Generic phrases (case-insensitive exact match)
    if GENERIC_PHRASES.contains(&lower.as_str()) {
        return false;
    }

    // Multi-word phrases ending with generic suffixes
    if t.contains(' ') {
        for suffix in GENERIC_SUFFIXES {
            if lower.ends_with(suffix) {
                return false;
            }
        }
        // Multi-word phrases starting with generic prefixes
        for prefix in GENERIC_PREFIXES {
            if lower.starts_with(prefix) {
                return false;
            }
        }
    }

    true
}

/// Apply type corrections for known systematic misclassifications.
#[cfg(feature = "embeddings")]
fn correct_entity_type(entity: &mut RecognizedEntity) {
    let key = entity.text.trim().to_lowercase();
    for &(pattern, corrected) in TYPE_CORRECTIONS {
        if key == pattern {
            entity.entity_type = EntityType::from_label(corrected);
            return;
        }
    }
}

/// Validate a relation for semantic consistency.
#[cfg(feature = "embeddings")]
fn is_valid_relation(rel: &RecognizedRelation) -> bool {
    // Self-relation
    if rel.head.text.to_lowercase() == rel.tail.text.to_lowercase() {
        return false;
    }

    // Both entities must individually pass the entity filter
    if !is_valid_entity(&rel.head.text) || !is_valid_entity(&rel.tail.text) {
        return false;
    }

    let head_type = rel.head.entity_type.as_label();
    let tail_type = rel.tail.entity_type.as_label();

    // "X uses Y" where X is a language and Y is a language/library/framework
    // — co-listed things are not "uses" relations
    if rel.relation_type == "uses"
        && head_type == "language"
        && matches!(tail_type, "language" | "library" | "framework")
    {
        return false;
    }

    // "X connects to Y" between two databases — they are separate systems
    if rel.relation_type == "connects to" && head_type == "database" && tail_type == "database" {
        return false;
    }

    // "X based on Y" where Y is a dataset — model was trained on it, not based on
    if rel.relation_type == "based on" && tail_type == "dataset" {
        return false;
    }

    // Person can only be the target of "created by"
    if head_type == "person" && rel.relation_type != "created by" {
        return false;
    }

    true
}

// ─── BiLSTM ONNX session ─────────────────────────────────────────────────

/// Runs the BiLSTM ONNX session on word embeddings.
/// Input: words_embedding[B, W, 768] → Output: words_embedding_rnn[B, W, 768]
#[cfg(feature = "embeddings")]
fn run_bilstm(bilstm: &mut Session, word_embs: &Array2<f32>) -> Result<Array2<f32>> {
    let (num_words, hidden_size) = (word_embs.nrows(), word_embs.ncols());
    let data: Vec<f32> = word_embs.iter().copied().collect();
    let input = TensorRef::from_array_view(([1usize, num_words, hidden_size], data.as_slice()))
        .map_err(|e| anyhow::anyhow!("bilstm input tensor: {e}"))?;

    let outputs = bilstm
        .run(ort::inputs!["words_embedding" => input])
        .map_err(|e| anyhow::anyhow!("bilstm run: {e}"))?;

    let result = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("bilstm extract: {e}"))?;
    let (shape, result_data) = result;

    // Shape is [1, W, 768] → reshape to [W, 768]
    let w = shape[1] as usize;
    let h = shape[2] as usize;
    Array2::from_shape_vec((w, h), result_data.to_vec())
        .map_err(|e| anyhow::anyhow!("bilstm reshape: {e}"))
}

// ─── Public NER Engine ───────────────────────────────────────────────────

/// GLiNER-RelEx ONNX engine for joint NER and Relation Extraction.
///
/// Model: knowledgator/gliner-relex-large-v0.5 (DeBERTa-v3-large, 467.8M params)
/// Entity types and relation labels are specified at inference time.
///
/// ONNX sessions are wrapped in `parking_lot::Mutex` because `ort::Session::run`
/// requires `&mut self`. The lock is held ONLY during `Session::run` and released
/// immediately after — all downstream ndarray work is lock-free.
#[cfg(feature = "embeddings")]
pub struct NEREngine {
    encoder: Mutex<Option<Session>>,
    bilstm: Mutex<Option<Session>>,
    span_marker: Option<SpanMarkerV0>,
    prompt_rep: Option<Mlp>,
    gcn: Option<Gcn>,
    pair_rep: Option<Mlp>,
    tokenizer: Option<Arc<Tokenizer>>,
    is_loaded: Arc<AtomicBool>,
    cache: Arc<DashMap<String, Vec<RecognizedEntity>>>,
}

#[cfg(feature = "embeddings")]
impl NEREngine {
    pub fn new() -> Self {
        Self {
            encoder: Mutex::new(None),
            bilstm: Mutex::new(None),
            span_marker: None,
            prompt_rep: None,
            gcn: None,
            pair_rep: None,
            tokenizer: None,
            is_loaded: Arc::new(AtomicBool::new(false)),
            cache: Arc::new(DashMap::new()),
        }
    }

    /// Get the model directory path.
    /// Checks: $PCX_GLINER_MODEL, then ~/.post-cortex/models/gliner-relex-large-v0.5
    fn model_dir() -> Result<String> {
        if let Ok(path) = std::env::var("PCX_GLINER_MODEL") {
            if std::path::Path::new(&path)
                .join("onnx/encoder.onnx")
                .exists()
            {
                return Ok(path);
            }
        }

        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        let default_path = home.join(".post-cortex/models/gliner-relex-large-v0.5");
        if default_path.join("onnx/encoder.onnx").exists() {
            return Ok(default_path.to_string_lossy().to_string());
        }

        Err(anyhow::anyhow!(
            "GLiNER-RelEx model not found. Expected at \
             ~/.post-cortex/models/gliner-relex-large-v0.5/ \
             or set PCX_GLINER_MODEL env var."
        ))
    }

    /// Load GLiNER-RelEx ONNX model from local directory.
    pub async fn load_model(&mut self) -> Result<()> {
        if self.is_loaded.load(Ordering::Acquire) {
            debug!("GLiNER-RelEx NER model already loaded");
            return Ok(());
        }

        let model_dir = Self::model_dir()?;
        info!("Loading GLiNER-RelEx model from {}...", model_dir);

        // Load tokenizer
        let tokenizer = Tokenizer::from_file(format!("{model_dir}/tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("load tokenizer: {e}"))?;

        // Load ONNX encoder (DeBERTa-v3-large + projection 1024→768)
        let encoder = Session::builder()
            .map_err(|e| anyhow::anyhow!("ort session builder: {e}"))?
            .with_intra_threads(4)
            .map_err(|e| anyhow::anyhow!("ort threads: {e}"))?
            .commit_from_file(format!("{model_dir}/onnx/encoder.onnx"))
            .map_err(|e| anyhow::anyhow!("load encoder: {e}"))?;

        // Load ONNX BiLSTM (BiLSTM 768→384*2=768)
        let bilstm = Session::builder()
            .map_err(|e| anyhow::anyhow!("ort session builder bilstm: {e}"))?
            .with_intra_threads(2)
            .map_err(|e| anyhow::anyhow!("ort threads bilstm: {e}"))?
            .commit_from_file(format!("{model_dir}/onnx/rnn.onnx"))
            .map_err(|e| anyhow::anyhow!("load bilstm: {e}"))?;

        let weights_dir = format!("{model_dir}/onnx/weights");

        // Load SpanMarkerV0 weights
        let span_marker = SpanMarkerV0::load(&weights_dir)?;

        // Load prompt_rep_layer MLP (768→3072→768)
        let prompt_rep = Mlp {
            linear1: Linear::load(&weights_dir, "prompt_rep.linear0")?,
            linear2: Linear::load(&weights_dir, "prompt_rep.linear1")?,
        };

        // Load GCN weights
        let gcn = Gcn::load(&weights_dir)?;

        // Load pair_rep_layer MLP (1536→3072→768)
        let pair_rep = Mlp {
            linear1: Linear::load(&weights_dir, "pair_rep.linear0")?,
            linear2: Linear::load(&weights_dir, "pair_rep.linear1")?,
        };

        *self.encoder.lock() = Some(encoder);
        *self.bilstm.lock() = Some(bilstm);
        self.span_marker = Some(span_marker);
        self.prompt_rep = Some(prompt_rep);
        self.gcn = Some(gcn);
        self.pair_rep = Some(pair_rep);
        self.tokenizer = Some(Arc::new(tokenizer));
        self.is_loaded.store(true, Ordering::Release);

        info!(
            "GLiNER-RelEx model loaded ({} entity types, {} relation types)",
            ENTITY_LABELS.len(),
            RELATION_LABELS.len(),
        );
        Ok(())
    }

    /// Extract named entities from text (NER only).
    pub fn extract_entities(&self, text: &str) -> Result<Vec<RecognizedEntity>> {
        let (entities, _relations) = self.extract_entities_and_relations(text)?;
        Ok(entities)
    }

    /// Extract entities and relations jointly.
    pub fn extract_entities_and_relations(
        &self,
        text: &str,
    ) -> Result<(Vec<RecognizedEntity>, Vec<RecognizedRelation>)> {
        if !self.is_loaded.load(Ordering::Acquire) {
            return Err(anyhow::anyhow!("model not loaded"));
        }

        // Check entity cache (relations are not cached separately)
        if let Some(cached) = self.cache.get(text) {
            return Ok((cached.clone(), vec![]));
        }

        let (mut entities, relations) = self.run_inference(text)?;

        // Apply entity post-filter and type corrections
        entities.retain(|e| is_valid_entity(&e.text));
        for e in &mut entities {
            correct_entity_type(e);
        }

        // Cache entities with bounded eviction
        if self.cache.len() >= NER_CACHE_MAX_SIZE {
            let keys_to_remove: Vec<String> = self
                .cache
                .iter()
                .take(NER_CACHE_MAX_SIZE / 2)
                .map(|entry| entry.key().clone())
                .collect();
            for key in keys_to_remove {
                self.cache.remove(&key);
            }
        }
        self.cache.insert(text.to_string(), entities.clone());

        Ok((entities, relations))
    }

    fn run_inference(
        &self,
        text: &str,
    ) -> Result<(Vec<RecognizedEntity>, Vec<RecognizedRelation>)> {
        let tokenizer = self
            .tokenizer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("tokenizer not loaded"))?;
        let span_marker = self
            .span_marker
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("span_marker not loaded"))?;
        let prompt_rep = self
            .prompt_rep
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("prompt_rep not loaded"))?;
        let gcn = self
            .gcn
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("gcn not loaded"))?;
        let pair_rep = self
            .pair_rep
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("pair_rep not loaded"))?;

        // Stage 1: Whitespace tokenize text
        let text_tokens = tokenize_text(text);
        if text_tokens.is_empty() {
            return Ok((vec![], vec![]));
        }

        // Stage 2: Build full prompt input
        let input = tokenize_prompt(tokenizer, text_tokens);
        let seq_len = input.input_ids.len();
        let input_ids_i64: Vec<i64> = input.input_ids.iter().map(|&id| id as i64).collect();
        let attention_mask_i64: Vec<i64> = vec![1i64; seq_len];

        // Stage 3: Run encoder — hold lock only for Session::run
        let input_ids_tensor =
            TensorRef::from_array_view(([1usize, seq_len], input_ids_i64.as_slice()))
                .map_err(|e| anyhow::anyhow!("input tensor: {e}"))?;
        let attn_mask_tensor =
            TensorRef::from_array_view(([1usize, seq_len], attention_mask_i64.as_slice()))
                .map_err(|e| anyhow::anyhow!("attn tensor: {e}"))?;

        let token_embeds = {
            let mut enc = self.encoder.lock();
            let encoder = enc
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("encoder not loaded"))?;
            let outputs = encoder
                .run(ort::inputs![
                    "input_ids" => input_ids_tensor,
                    "attention_mask" => attn_mask_tensor
                ])
                .map_err(|e| anyhow::anyhow!("encoder run: {e}"))?;

            let hs = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow::anyhow!("extract hidden: {e}"))?;
            let (shape, data) = hs;
            let hidden_size = shape[2] as usize;
            let seq_actual = shape[1] as usize;
            Array2::from_shape_vec((seq_actual, hidden_size), data.to_vec())
                .map_err(|e| anyhow::anyhow!("reshape hidden: {e}"))?
        };

        // Stage 4: Extract word, entity-prompt, and relation-prompt embeddings
        let embs = extract_embeddings(&token_embeds, &input);
        let num_words = embs.word_embeddings.nrows();
        if num_words == 0 {
            return Ok((vec![], vec![]));
        }

        // Stage 5: BiLSTM on word embeddings — hold lock only for Session::run
        let word_embs_rnn = {
            let mut lstm = self.bilstm.lock();
            let bilstm = lstm
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("bilstm not loaded"))?;
            run_bilstm(bilstm, &embs.word_embeddings)?
        };

        // Stage 6: prompt_rep_layer MLP on entity prompt embeddings
        let processed_ent_prompts = prompt_rep.forward(&embs.ent_prompt_embeddings);

        // Stage 7: SpanMarkerV0 → span representations
        let (span_reps, valid_mask) = span_marker.forward(&word_embs_rnn, MAX_WIDTH);

        // Stage 8: Entity scores = sigmoid(span_reps @ processed_ent_prompts.T)
        let entity_scores = score_spans(&span_reps, &processed_ent_prompts);

        // Stage 9: NMS + entity extraction
        let entities = extract_entities_from_scores(
            &entity_scores,
            &valid_mask,
            &input.text_tokens,
            text,
            ENTITY_THRESHOLD,
        );

        if entities.is_empty() {
            return Ok((entities, vec![]));
        }

        // Stage 10: Collect span_reps for detected entities
        // Map each entity to its span_rep index (start_word * MAX_WIDTH + 0 is the
        // single-word span; for multi-word spans start * MAX_WIDTH + width).
        let entity_span_reps: Array2<f32> = {
            let mut rows: Vec<Array1<f32>> = Vec::with_capacity(entities.len());
            for ent in &entities {
                // Find the word index for this entity's start char position
                let start_word = input
                    .text_tokens
                    .iter()
                    .position(|t| t.start == ent.start)
                    .unwrap_or(0);
                let end_word = input
                    .text_tokens
                    .iter()
                    .position(|t| t.end == ent.end)
                    .unwrap_or(start_word);
                let width = end_word.saturating_sub(start_word);
                let span_idx = (start_word * MAX_WIDTH + width).min(span_reps.nrows() - 1);
                rows.push(span_reps.row(span_idx).to_owned());
            }
            let e = rows.len();
            let h = rows[0].len();
            let flat: Vec<f32> = rows.iter().flat_map(|r| r.iter().copied()).collect();
            Array2::from_shape_vec((e, h), flat)
                .map_err(|e| anyhow::anyhow!("entity span_rep stack: {e}"))?
        };

        // Stage 11: GCN adjacency over entity span_reps
        let adj_final = gcn.forward(&entity_span_reps);

        // Stage 12: Build entity pairs where adj_final[i,j] > adjacency threshold
        let num_ents = entities.len();
        let mut pair_head_reps: Vec<Array1<f32>> = Vec::new();
        let mut pair_tail_reps: Vec<Array1<f32>> = Vec::new();
        let mut pair_indices: Vec<(usize, usize)> = Vec::new();

        for i in 0..num_ents {
            for j in 0..num_ents {
                if i == j {
                    continue;
                }
                if adj_final[[i, j]] > ADJACENCY_THRESHOLD {
                    pair_head_reps.push(entity_span_reps.row(i).to_owned());
                    pair_tail_reps.push(entity_span_reps.row(j).to_owned());
                    pair_indices.push((i, j));
                }
            }
        }

        if pair_indices.is_empty() {
            return Ok((entities, vec![]));
        }

        // Stage 13: pair_rep_layer on concatenated [head, tail] representations
        let num_pairs = pair_indices.len();
        let hidden = entity_span_reps.ncols();
        let mut concat = Array2::<f32>::zeros((num_pairs, hidden * 2));
        for k in 0..num_pairs {
            concat.slice_mut(s![k, ..hidden]).assign(&pair_head_reps[k]);
            concat.slice_mut(s![k, hidden..]).assign(&pair_tail_reps[k]);
        }
        let pair_reps = pair_rep.forward(&concat);

        // Stage 14: Relation scores = sigmoid(pair_reps @ rel_prompt_embs.T)
        let relation_scores = score_spans(&pair_reps, &embs.rel_prompt_embeddings);

        // Stage 15: Collect relations above threshold
        let mut relations: Vec<RecognizedRelation> = Vec::new();
        for k in 0..num_pairs {
            let (head_idx, tail_idx) = pair_indices[k];
            for (rel_idx, &rel_label) in RELATION_LABELS.iter().enumerate() {
                let score = relation_scores[[k, rel_idx]];
                if score >= RELATION_THRESHOLD {
                    let rel = RecognizedRelation {
                        head: entities[head_idx].clone(),
                        tail: entities[tail_idx].clone(),
                        relation_type: rel_label.to_string(),
                        confidence: score,
                    };
                    if is_valid_relation(&rel) {
                        relations.push(rel);
                    }
                }
            }
        }

        Ok((entities, relations))
    }

    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }
}

#[cfg(feature = "embeddings")]
impl Default for NEREngine {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
#[cfg(feature = "embeddings")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ner_engine_creation() {
        let engine = NEREngine::new();
        assert!(!engine.is_loaded.load(Ordering::Relaxed));
        assert_eq!(engine.cache_size(), 0);
    }

    #[test]
    fn test_entity_type_from_bio_tag() {
        assert_eq!(EntityType::from_bio_tag("B-PER"), Some(EntityType::Person));
        assert_eq!(
            EntityType::from_bio_tag("I-ORG"),
            Some(EntityType::Organization)
        );
        assert_eq!(
            EntityType::from_bio_tag("B-LOC"),
            Some(EntityType::Location)
        );
        assert_eq!(
            EntityType::from_bio_tag("I-MISC"),
            Some(EntityType::Miscellaneous)
        );
        assert_eq!(EntityType::from_bio_tag("O"), None);
    }

    #[test]
    fn test_entity_type_from_label_roundtrip() {
        for (label, _desc) in ENTITY_LABELS {
            let et = EntityType::from_label(label);
            assert_eq!(et.as_label(), *label, "roundtrip failed for {label}");
        }
    }

    #[test]
    fn test_is_valid_entity_rejects_code_fragments() {
        assert!(!is_valid_entity("db.select()"));
        assert!(!is_valid_entity("self.method"));
        assert!(!is_valid_entity("serde_json"));
        assert!(!is_valid_entity("axon::tools"));
        assert!(!is_valid_entity("__init__"));
        assert!(!is_valid_entity("42"));
        assert!(!is_valid_entity("src/lib.rs"));
        assert!(!is_valid_entity("&str"));
        assert!(!is_valid_entity("a"));
    }

    #[test]
    fn test_is_valid_entity_accepts_named_entities() {
        assert!(is_valid_entity("tokio"));
        assert!(is_valid_entity("DashMap"));
        assert!(is_valid_entity("PostgreSQL"));
        assert!(is_valid_entity("GLiNER"));
        assert!(is_valid_entity("DeBERTa-v3-large"));
        assert!(is_valid_entity("Julius"));
    }

    #[test]
    fn test_is_valid_entity_rejects_generic_phrases() {
        assert!(!is_valid_entity("embedding model"));
        assert!(!is_valid_entity("async runtime"));
        assert!(!is_valid_entity("the Rust compiler"));
        assert!(!is_valid_entity("a new framework"));
        assert!(!is_valid_entity("Candle inference")); // ends with " inference"
        assert!(!is_valid_entity("custom entity types"));
    }

    #[test]
    fn test_correct_entity_type_conll() {
        let mut e = RecognizedEntity {
            text: "CoNLL".to_string(),
            entity_type: EntityType::Language,
            confidence: 0.9,
            start: 0,
            end: 5,
        };
        correct_entity_type(&mut e);
        assert_eq!(e.entity_type, EntityType::Dataset);
    }

    #[test]
    fn test_correct_entity_type_docker() {
        let mut e = RecognizedEntity {
            text: "Docker".to_string(),
            entity_type: EntityType::Framework,
            confidence: 0.8,
            start: 0,
            end: 6,
        };
        correct_entity_type(&mut e);
        assert_eq!(e.entity_type, EntityType::Tool);
    }

    #[test]
    fn test_is_valid_relation_rejects_self() {
        let ent = RecognizedEntity {
            text: "Rust".to_string(),
            entity_type: EntityType::Language,
            confidence: 0.9,
            start: 0,
            end: 4,
        };
        let rel = RecognizedRelation {
            head: ent.clone(),
            tail: ent.clone(),
            relation_type: "uses".to_string(),
            confidence: 0.9,
        };
        assert!(!is_valid_relation(&rel));
    }

    #[test]
    fn test_is_valid_relation_rejects_language_uses_language() {
        let rel = RecognizedRelation {
            head: RecognizedEntity {
                text: "Rust".to_string(),
                entity_type: EntityType::Language,
                confidence: 0.9,
                start: 0,
                end: 4,
            },
            tail: RecognizedEntity {
                text: "React".to_string(),
                entity_type: EntityType::Framework,
                confidence: 0.9,
                start: 10,
                end: 15,
            },
            relation_type: "uses".to_string(),
            confidence: 0.9,
        };
        assert!(!is_valid_relation(&rel));
    }

    #[test]
    fn test_is_valid_relation_accepts_built_with() {
        let rel = RecognizedRelation {
            head: RecognizedEntity {
                text: "Post-Cortex".to_string(),
                entity_type: EntityType::Framework,
                confidence: 0.9,
                start: 0,
                end: 11,
            },
            tail: RecognizedEntity {
                text: "Rust".to_string(),
                entity_type: EntityType::Language,
                confidence: 0.9,
                start: 20,
                end: 24,
            },
            relation_type: "built with".to_string(),
            confidence: 0.92,
        };
        assert!(is_valid_relation(&rel));
    }

    #[test]
    fn test_tokenize_text_basic() {
        let tokens = tokenize_text("tokio and axum");
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].text, "tokio");
        assert_eq!(tokens[2].text, "axum");
    }

    #[tokio::test]
    #[ignore] // Requires model files at ~/.post-cortex/models/gliner-relex-large-v0.5/
    async fn test_relex_model_loading() {
        let mut engine = NEREngine::new();
        let result = engine.load_model().await;
        assert!(result.is_ok(), "Failed to load: {:?}", result.err());
        assert!(engine.is_loaded.load(Ordering::Relaxed));
    }

    #[tokio::test]
    #[ignore] // Requires model files
    async fn test_software_entity_extraction() {
        let mut engine = NEREngine::new();
        engine.load_model().await.unwrap();

        let test_cases = vec![
            "Post-Cortex is an intelligent conversation memory system built with Rust.",
            "It uses RocksDB for persistent storage and HNSW for vector similarity search.",
            "We decided to replace DistilBERT with GLiNER2 for named entity recognition.",
            "Axon is a coding agent that connects to Post-Cortex via gRPC using tonic.",
            "PostgreSQL and Redis are used with axum and tower for the HTTP API.",
        ];

        for text in test_cases {
            println!("\n--- Testing: {} ---", text);
            let (entities, relations) = engine.extract_entities_and_relations(text).unwrap();
            println!("Entities ({}):", entities.len());
            for e in &entities {
                println!(
                    "  '{}' => {:?} ({:.0}%)",
                    e.text,
                    e.entity_type,
                    e.confidence * 100.0
                );
            }
            println!("Relations ({}):", relations.len());
            for r in &relations {
                println!(
                    "  '{}' --[{}]--> '{}' ({:.0}%)",
                    r.head.text,
                    r.relation_type,
                    r.tail.text,
                    r.confidence * 100.0
                );
            }
        }
    }
}
