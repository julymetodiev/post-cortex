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
//! Pipeline: backbone.onnx (DeBERTa encoder + BiLSTM + SpanMarker + prompt MLP
//! + entity scoring) → GCN → pair MLP → relation scoring
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
    "depends on",
    "required by",
    "implements",
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
const ENTITY_THRESHOLD: f32 = 0.7;
#[cfg(feature = "embeddings")]
const ADJACENCY_THRESHOLD: f32 = 0.5;
#[cfg(feature = "embeddings")]
const RELATION_THRESHOLD: f32 = 0.7;
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
    // NOTE: Do NOT lowercase — the tokenizer is case-sensitive (SentencePiece/DeBERTa).
    // "Post" and "post" produce different token IDs → different embeddings.
    // The (?i) flag only makes the regex pattern case-insensitive, not the captured text.
    let re = Regex::new(
        r"(?i)(?:https?://[^\s]+|www\.[^\s]+)|[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}|@[a-zA-Z0-9_]+|\w+(?:[-_]\w+)*|\S",
    ).unwrap();
    re.find_iter(text)
        .map(|m| TextToken {
            text: m.as_str().to_string(),
            start: m.start(),
            end: m.end(),
        })
        .collect()
}

/// Tokenized input for the RelEx model.
///
/// Format: [BOS] <<ENT>> label_name <<ENT>> label_name ... <<SEP>>
///         <<REL>> rel1 <<REL>> rel2 ... <<SEP>> word1 word2 ... [EOS]
#[cfg(feature = "embeddings")]
struct TokenizedInput {
    input_ids: Vec<u32>,
    /// Position of the first subword token for EVERY "word" in the entire input
    /// sequence (BOS excluded; special tokens like <<ENT>>, <<SEP>>, <<REL>>
    /// each count as one word with mask=1 at their own position).
    /// Used to build the `words_mask` input to backbone.onnx.
    all_first_subword: Vec<usize>,
    /// Original whitespace-tokenized text words.
    text_tokens: Vec<TextToken>,
}

#[cfg(feature = "embeddings")]
fn tokenize_prompt(tokenizer: &Tokenizer, text_tokens: Vec<TextToken>) -> TokenizedInput {
    let mut input_ids: Vec<u32> = Vec::new();
    // Tracks first-subword positions for ALL words (prompt + text) — words_mask.
    let mut all_first_subword: Vec<usize> = Vec::new();

    // BOS — not counted as a "word" in the is_split_into_words sense; the Python
    // tokenizer wraps the whole sequence and BOS appears at position 0 without a
    // corresponding word_id. We skip it in all_first_subword.
    input_ids.push(TOK_BOS);

    // Entity section: <<ENT>> label_name for each entity type.
    // NOTE: Only the label name goes into the prompt (e.g. "library"),
    // NOT the description. The description is used by the Python dict-label
    // API to guide the model's internal matching, but the actual tokenized
    // prompt must match what the model was trained on: <<ENT>> label_name.
    for (label, _description) in ENTITY_LABELS {
        // <<ENT>> is a single-token word
        all_first_subword.push(input_ids.len());
        input_ids.push(TOK_ENT);

        // Tokenize the label name — each label text is one "word" in the
        // is_split_into_words sense; mark its first subword token.
        let enc = tokenizer
            .encode(*label, false)
            .expect("tokenize entity label");
        let ids = enc.get_ids();
        if !ids.is_empty() {
            all_first_subword.push(input_ids.len());
            for &id in ids {
                input_ids.push(id);
            }
        }
    }

    // <<SEP>> between entity section and relation section
    all_first_subword.push(input_ids.len());
    input_ids.push(TOK_SEP);

    // Relation section: <<REL>> relation_label for each relation type.
    for rel in RELATION_LABELS {
        // <<REL>> is a single-token word
        all_first_subword.push(input_ids.len());
        input_ids.push(TOK_REL);

        // Tokenize the relation label text.  Relation labels can be multi-word
        // (e.g. "built with"), but in the is_split_into_words model each space-
        // separated piece is a distinct "word".  We tokenize the full label as a
        // single string here; the first token of the whole encoded label is what
        // gets words_mask=1 (matching how Python encodes it as a single element
        // in the pre-tokenized word list).
        let enc = tokenizer
            .encode(*rel, false)
            .expect("tokenize relation label");
        let ids = enc.get_ids();
        if !ids.is_empty() {
            all_first_subword.push(input_ids.len());
            for &id in ids {
                input_ids.push(id);
            }
        }
    }

    // <<SEP>> between relation section and text section
    all_first_subword.push(input_ids.len());
    input_ids.push(TOK_SEP);

    // Text section: mark the first subword token for each whitespace word.
    for text_tok in &text_tokens {
        let word_start_idx = input_ids.len();
        let enc = tokenizer
            .encode(text_tok.text.as_str(), false)
            .expect("tokenize text token");
        let ids = enc.get_ids();
        if !ids.is_empty() {
            all_first_subword.push(word_start_idx);
            for &id in ids {
                input_ids.push(id);
            }
        }
    }

    // EOS — not a user word, skip from all_first_subword (matches Python where
    // the trailing [EOS] token has word_id=None).
    input_ids.push(TOK_EOS);

    TokenizedInput {
        input_ids,
        all_first_subword,
        text_tokens,
    }
}

// ─── Math helpers ────────────────────────────────────────────────────────

#[cfg(feature = "embeddings")]
#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

// ─── NMS entity extraction ───────────────────────────────────────────────

/// Extract entities from backbone entity_logits [L*K, C] (already sigmoid-applied).
/// `valid_mask` is [L*K] — 1 for valid spans (end < num_words).
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

    // Cross-field deduplication: if multiple fields produce overlapping spans,
    // keep only the one with the highest confidence.
    all_spans.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());
    let mut deduped: Vec<RecognizedEntity> = Vec::new();
    for span in all_spans {
        let overlaps = deduped
            .iter()
            .any(|s| !(span.end <= s.start || span.start >= s.end));
        if !overlaps {
            deduped.push(span);
        }
    }

    deduped.sort_by_key(|s| s.start);
    deduped
}

// ─── Post-filter ─────────────────────────────────────────────────────────

/// Generic phrases that are descriptions, not named entities.
#[cfg(feature = "embeddings")]
const GENERIC_PHRASES: &[&str] = &[
    // Pronouns and common false positives (single words)
    "it",
    "we",
    "he",
    "she",
    "they",
    "i",
    "you",
    "us",
    "them",
    "this",
    "that",
    "these",
    "those",
    "the",
    "a",
    "an",
    // PCX internal labels that appear in content_text
    "incremental update",
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
/// Convenience wrapper without confidence — used in tests.
#[cfg(all(feature = "embeddings", test))]
fn is_valid_entity(text: &str) -> bool {
    is_valid_entity_with_confidence(text, 0.0)
}

/// Like `is_valid_entity`, but high-confidence NER entities (≥ 0.85) bypass
/// the generic suffix/prefix heuristic filters. The model already decided this
/// is a named entity — code-fragment and length checks still apply.
#[cfg(feature = "embeddings")]
fn is_valid_entity_with_confidence(text: &str, confidence: f32) -> bool {
    let t = text.trim();

    // Length bounds — always enforced
    if t.len() < 2 || t.len() > 40 {
        return false;
    }

    // Code fragments — always enforced (model can't override these)
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

    // High-confidence NER entities skip the generic phrase/suffix/prefix filters.
    // The model is confident this is a named entity (e.g. "ONNX Runtime", "NER cache"),
    // not a generic description like "async runtime" or "embedding model".
    const HIGH_CONFIDENCE: f32 = 0.85;
    if confidence >= HIGH_CONFIDENCE {
        return true;
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

    // Both entities must individually pass the entity filter (with confidence)
    if !is_valid_entity_with_confidence(&rel.head.text, rel.head.confidence)
        || !is_valid_entity_with_confidence(&rel.tail.text, rel.tail.confidence)
    {
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

// ─── Public NER Engine ───────────────────────────────────────────────────

/// GLiNER-RelEx ONNX engine for joint NER and Relation Extraction.
///
/// Model: knowledgator/gliner-relex-large-v0.5 (DeBERTa-v3-large, 467.8M params)
/// Entity types and relation labels are specified at inference time.
///
/// The backbone ONNX session is wrapped in `parking_lot::Mutex` because
/// `ort::Session::run` requires `&mut self`. The lock is held ONLY during
/// `Session::run` and released immediately after — all downstream ndarray
/// work is lock-free.
#[cfg(feature = "embeddings")]
pub struct NEREngine {
    /// Single backbone ONNX session covering DeBERTa encoder + BiLSTM +
    /// SpanMarker + prompt MLP + entity scoring.
    backbone: Mutex<Option<Session>>,
    gcn: Option<Gcn>,
    pair_rep: Option<Mlp>,
    tokenizer: Option<Arc<Tokenizer>>,
    is_loaded: Arc<AtomicBool>,
    cache: Arc<DashMap<String, (Vec<RecognizedEntity>, Vec<RecognizedRelation>)>>,
}

#[cfg(feature = "embeddings")]
impl NEREngine {
    pub fn new() -> Self {
        Self {
            backbone: Mutex::new(None),
            gcn: None,
            pair_rep: None,
            tokenizer: None,
            is_loaded: Arc::new(AtomicBool::new(false)),
            cache: Arc::new(DashMap::new()),
        }
    }

    /// Get the model directory path.
    /// Checks $PCX_GLINER_MODEL/backbone.onnx, then
    /// ~/.cache/gliner-relex-onnx/backbone.onnx
    fn model_dir() -> Result<String> {
        if let Ok(path) = std::env::var("PCX_GLINER_MODEL") {
            if std::path::Path::new(&path).join("backbone.onnx").exists() {
                return Ok(path);
            }
        }

        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        let cache_path = home.join(".cache/gliner-relex-onnx");
        if cache_path.join("backbone.onnx").exists() {
            return Ok(cache_path.to_string_lossy().to_string());
        }

        Err(anyhow::anyhow!(
            "GLiNER-RelEx model not found. Expected backbone.onnx at \
             ~/.cache/gliner-relex-onnx/ or set PCX_GLINER_MODEL env var."
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

        // Load single backbone ONNX session (DeBERTa encoder + BiLSTM +
        // SpanMarker + prompt MLP + entity scoring, ~1.7 GB).
        let backbone = Session::builder()
            .map_err(|e| anyhow::anyhow!("ort session builder: {e}"))?
            .with_intra_threads(4)
            .map_err(|e| anyhow::anyhow!("ort threads: {e}"))?
            .commit_from_file(format!("{model_dir}/backbone.onnx"))
            .map_err(|e| anyhow::anyhow!("load backbone: {e}"))?;

        let weights_dir = format!("{model_dir}/weights");

        // Load GCN weights
        let gcn = Gcn::load(&weights_dir)?;

        // Load pair_rep_layer MLP (1536→3072→768)
        let pair_rep = Mlp {
            linear1: Linear::load(&weights_dir, "pair_rep.linear0")?,
            linear2: Linear::load(&weights_dir, "pair_rep.linear1")?,
        };

        *self.backbone.lock() = Some(backbone);
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

        // Check cache (entities + relations)
        if let Some(cached) = self.cache.get(text) {
            let (entities, relations) = cached.clone();
            return Ok((entities, relations));
        }

        let (mut entities, relations) = self.run_inference(text)?;

        // Apply entity post-filter and type corrections.
        // High-confidence NER entities (≥ 0.85) bypass the generic suffix/prefix filter —
        // the model is confident enough that it's a named entity, not a generic phrase.
        entities.retain(|e| is_valid_entity_with_confidence(&e.text, e.confidence));
        for e in &mut entities {
            correct_entity_type(e);
        }

        // Cache with bounded eviction
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
        self.cache
            .insert(text.to_string(), (entities.clone(), relations.clone()));

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

        // Stage 2: Build full prompt input (also records all_first_subword)
        let input = tokenize_prompt(tokenizer, text_tokens);
        let seq_len = input.input_ids.len();
        let num_words = input.text_tokens.len();

        if num_words == 0 {
            return Ok((vec![], vec![]));
        }

        let input_ids_i64: Vec<i64> = input.input_ids.iter().map(|&id| id as i64).collect();
        let attention_mask_i64: Vec<i64> = vec![1i64; seq_len];

        // Stage 3: Build words_mask — 1-indexed word indices for TEXT words only.
        // Python's prepare_word_mask skips prompt words (skip_first_words) and
        // assigns 1, 2, 3, ... to the first subword of each text word.
        // all_first_subword includes prompt + text positions; only the last
        // num_words entries are text words.
        let num_prompt_words = input.all_first_subword.len() - num_words;
        let mut words_mask_i64 = vec![0i64; seq_len];
        for (i, &pos) in input.all_first_subword.iter().skip(num_prompt_words).enumerate() {
            if pos < seq_len {
                words_mask_i64[pos] = (i + 1) as i64; // 1-indexed
            }
        }

        // Stage 4: text_lengths — number of text words
        let text_lengths_i64: Vec<i64> = vec![num_words as i64];

        // Stage 5: span_idx [num_spans, 2] — (start_word, end_word) pairs
        // Stage 6: span_mask [num_spans] — 1 where span is valid
        let num_spans = num_words * MAX_WIDTH;
        let mut span_idx_flat: Vec<i64> = vec![0i64; num_spans * 2];
        let mut span_mask_bool: Vec<bool> = vec![false; num_spans];

        for start in 0..num_words {
            for w in 0..MAX_WIDTH {
                let end = start + w;
                let flat_idx = start * MAX_WIDTH + w;
                if end < num_words {
                    span_idx_flat[flat_idx * 2] = start as i64;
                    span_idx_flat[flat_idx * 2 + 1] = end as i64;
                    span_mask_bool[flat_idx] = true;
                }
                // padding entries remain [0, 0] / false
            }
        }

        // Stage 7: Run backbone.onnx — hold lock only for Session::run
        // Outputs: entity_logits [1, L, K, C], span_reps [1, L*K, D],
        //          rel_prompt_embeds [1, R, D]
        let (entity_logits_flat, entity_logits_shape, span_reps_2d, rel_prompt_embeds_2d) = {
            let input_ids_tensor =
                TensorRef::from_array_view(([1usize, seq_len], input_ids_i64.as_slice()))
                    .map_err(|e| anyhow::anyhow!("backbone input_ids tensor: {e}"))?;
            let attn_mask_tensor =
                TensorRef::from_array_view(([1usize, seq_len], attention_mask_i64.as_slice()))
                    .map_err(|e| anyhow::anyhow!("backbone attention_mask tensor: {e}"))?;
            let words_mask_tensor =
                TensorRef::from_array_view(([1usize, seq_len], words_mask_i64.as_slice()))
                    .map_err(|e| anyhow::anyhow!("backbone words_mask tensor: {e}"))?;
            let text_lengths_tensor =
                TensorRef::from_array_view(([1usize, 1usize], text_lengths_i64.as_slice()))
                    .map_err(|e| anyhow::anyhow!("backbone text_lengths tensor: {e}"))?;
            let span_idx_tensor =
                TensorRef::from_array_view(([1usize, num_spans, 2usize], span_idx_flat.as_slice()))
                    .map_err(|e| anyhow::anyhow!("backbone span_idx tensor: {e}"))?;
            let span_mask_tensor =
                TensorRef::from_array_view(([1usize, num_spans], span_mask_bool.as_slice()))
                    .map_err(|e| anyhow::anyhow!("backbone span_mask tensor: {e}"))?;

            let mut bb = self.backbone.lock();
            let backbone = bb
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("backbone not loaded"))?;

            let outputs = backbone
                .run(ort::inputs![
                    "input_ids"      => input_ids_tensor,
                    "attention_mask" => attn_mask_tensor,
                    "words_mask"     => words_mask_tensor,
                    "text_lengths"   => text_lengths_tensor,
                    "span_idx"       => span_idx_tensor,
                    "span_mask"      => span_mask_tensor
                ])
                .map_err(|e| anyhow::anyhow!("backbone run: {e}"))?;

            // entity_logits [1, L, K, C]
            let logits_raw = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow::anyhow!("extract entity_logits: {e}"))?;
            let (logits_shape, logits_data) = logits_raw;
            let logits_shape_owned: Vec<usize> =
                logits_shape.iter().map(|&d| d as usize).collect();
            let logits_flat: Vec<f32> = logits_data.to_vec();

            // span_reps [1, L*K, D]
            let span_raw = outputs[1]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow::anyhow!("extract span_reps: {e}"))?;
            let (span_shape, span_data) = span_raw;
            let span_lk = span_shape[1] as usize;
            let span_d = span_shape[2] as usize;
            let span_2d = Array2::from_shape_vec((span_lk, span_d), span_data.to_vec())
                .map_err(|e| anyhow::anyhow!("reshape span_reps: {e}"))?;

            // rel_prompt_embeds [1, R, D]
            let rel_raw = outputs[2]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow::anyhow!("extract rel_prompt_embeds: {e}"))?;
            let (rel_shape, rel_data) = rel_raw;
            let rel_r = rel_shape[1] as usize;
            let rel_d = rel_shape[2] as usize;
            let rel_2d = Array2::from_shape_vec((rel_r, rel_d), rel_data.to_vec())
                .map_err(|e| anyhow::anyhow!("reshape rel_prompt_embeds: {e}"))?;

            (logits_flat, logits_shape_owned, span_2d, rel_2d)
        };

        // Stage 8: Reshape entity_logits [1, L, K, C] → [L*K, C], apply sigmoid
        // entity_logits_shape = [1, L, K, C]
        let l = entity_logits_shape[1]; // num_words
        let k = entity_logits_shape[2]; // MAX_WIDTH
        let c = entity_logits_shape[3]; // num_entity_labels
        let lk = l * k;

        if lk != num_spans || c != ENTITY_LABELS.len() {
            return Err(anyhow::anyhow!(
                "backbone entity_logits shape mismatch: expected [{}, {}, {}], got [{}, {}, {}]",
                num_words,
                MAX_WIDTH,
                ENTITY_LABELS.len(),
                l,
                k,
                c
            ));
        }

        // entity_logits_flat is in row-major order [1, L, K, C]:
        // flat index = l_idx * K * C + k_idx * C + c_idx
        // We want [L*K, C]: row = l_idx * K + k_idx, col = c_idx — same memory layout.
        let entity_scores_raw =
            Array2::from_shape_vec((lk, c), entity_logits_flat)
                .map_err(|e| anyhow::anyhow!("reshape entity_logits to [L*K, C]: {e}"))?;
        // Apply sigmoid to get probabilities
        let entity_scores = entity_scores_raw.mapv(sigmoid);

        // Stage 9: NMS + entity extraction
        let entities = extract_entities_from_scores(
            &entity_scores,
            &span_mask_bool,
            &input.text_tokens,
            text,
            ENTITY_THRESHOLD,
        );

        if entities.is_empty() {
            return Ok((entities, vec![]));
        }

        // Stage 10: Collect span_reps for detected entities from backbone output.
        // span_reps [L*K, D]: flat index = start_word * MAX_WIDTH + width
        let entity_span_reps: Array2<f32> = {
            let mut rows: Vec<Array1<f32>> = Vec::with_capacity(entities.len());
            for ent in &entities {
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
                let flat_idx =
                    (start_word * MAX_WIDTH + width).min(span_reps_2d.nrows() - 1);
                rows.push(span_reps_2d.row(flat_idx).to_owned());
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
            debug!(
                "No entity pairs above adjacency threshold {}",
                ADJACENCY_THRESHOLD
            );
            return Ok((entities, vec![]));
        }

        // Stage 13: pair_rep_layer on concatenated [head, tail] representations
        let num_pairs = pair_indices.len();
        let hidden = entity_span_reps.ncols();
        let mut concat = Array2::<f32>::zeros((num_pairs, hidden * 2));
        for k_idx in 0..num_pairs {
            concat
                .slice_mut(s![k_idx, ..hidden])
                .assign(&pair_head_reps[k_idx]);
            concat
                .slice_mut(s![k_idx, hidden..])
                .assign(&pair_tail_reps[k_idx]);
        }
        let pair_reps = pair_rep.forward(&concat);

        // Stage 14: Relation scores = sigmoid(pair_reps @ rel_prompt_embeds.T)
        let relation_scores = pair_reps
            .dot(&rel_prompt_embeds_2d.t())
            .mapv(sigmoid);

        // Stage 15: Collect relations above threshold
        let mut relations: Vec<RecognizedRelation> = Vec::new();
        for k_idx in 0..num_pairs {
            let (head_idx, tail_idx) = pair_indices[k_idx];
            for (rel_idx, &rel_label) in RELATION_LABELS.iter().enumerate() {
                let score = relation_scores[[k_idx, rel_idx]];
                // Debug: log near-threshold scores to calibrate RELATION_THRESHOLD
                if score >= 0.5 {
                    debug!(
                        "REL_SCORE: '{}' --[{}]--> '{}' = {:.3}  {}",
                        entities[head_idx].text,
                        rel_label,
                        entities[tail_idx].text,
                        score,
                        if score >= RELATION_THRESHOLD {
                            "✓"
                        } else {
                            "✗ (below threshold)"
                        }
                    );
                }
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

    /// Extract entities (with PCX types) and relations (as PCX EntityRelationship) for graph integration.
    /// Primary method for `update_entity_graph` — typed edges from NER replace heuristic co-occurrence.
    pub fn extract_for_graph(
        &self,
        text: &str,
    ) -> Result<(
        Vec<(String, crate::core::context_update::EntityType)>,
        Vec<crate::core::context_update::EntityRelationship>,
    )> {
        let (entities, relations) = self.extract_entities_and_relations(text)?;

        let pcx_entities: Vec<(String, crate::core::context_update::EntityType)> = entities
            .iter()
            .map(|e| (e.text.clone(), ner_to_pcx_entity_type(&e.entity_type)))
            .collect();

        let pcx_relations: Vec<crate::core::context_update::EntityRelationship> = relations
            .iter()
            .map(|r| crate::core::context_update::EntityRelationship {
                from_entity: r.head.text.clone(),
                to_entity: r.tail.text.clone(),
                relation_type: ner_to_pcx_relation_type(&r.relation_type),
                context: format!(
                    "{} --[{}]--> {} ({:.0}%)",
                    r.head.text,
                    r.relation_type,
                    r.tail.text,
                    r.confidence * 100.0
                ),
            })
            .collect();

        Ok((pcx_entities, pcx_relations))
    }

    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }
}

/// Map NER EntityType → PCX EntityType
#[cfg(feature = "embeddings")]
fn ner_to_pcx_entity_type(et: &EntityType) -> crate::core::context_update::EntityType {
    use crate::core::context_update::EntityType as PcxET;
    match et {
        EntityType::Library
        | EntityType::Framework
        | EntityType::Language
        | EntityType::Database
        | EntityType::Protocol
        | EntityType::Tool
        | EntityType::Model
        | EntityType::Algorithm => PcxET::Technology,
        EntityType::Person | EntityType::Organization => PcxET::Concept,
        EntityType::Dataset => PcxET::Concept,
        EntityType::API => PcxET::Technology,
        EntityType::DataStructure => PcxET::Technology,
        EntityType::Function | EntityType::Class => PcxET::CodeComponent,
        EntityType::Location | EntityType::Miscellaneous => PcxET::Concept,
    }
}

/// Map NER relation label → PCX RelationType
#[cfg(feature = "embeddings")]
fn ner_to_pcx_relation_type(rel: &str) -> crate::core::context_update::RelationType {
    use crate::core::context_update::RelationType as RT;
    match rel {
        "built with" | "uses" | "based on" => RT::DependsOn,
        "replaced by" => RT::LeadsTo,
        "alternative to" | "connects to" | "created by" => RT::RelatedTo,
        "part of" => RT::RequiredBy,
        _ => RT::RelatedTo,
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
    fn test_high_confidence_bypasses_suffix_filter() {
        // Without confidence, suffix filter kills these
        assert!(!is_valid_entity("ONNX Runtime"));
        assert!(!is_valid_entity("NER cache"));
        assert!(!is_valid_entity("NER engine"));
        assert!(!is_valid_entity("async runtime"));

        // With high confidence (≥ 0.85), NER model overrides suffix filter
        assert!(is_valid_entity_with_confidence("ONNX Runtime", 0.90));
        assert!(is_valid_entity_with_confidence("NER cache", 0.85));
        assert!(is_valid_entity_with_confidence("NER engine", 0.88));

        // But code fragments are ALWAYS rejected regardless of confidence
        assert!(!is_valid_entity_with_confidence("db.select()", 0.99));
        assert!(!is_valid_entity_with_confidence("axon::tools", 0.95));
        assert!(!is_valid_entity_with_confidence("serde_json", 0.92));
        assert!(!is_valid_entity_with_confidence("&str", 0.99));

        // Low confidence still gets suffix-filtered
        assert!(!is_valid_entity_with_confidence("async runtime", 0.60));
        assert!(!is_valid_entity_with_confidence("embedding model", 0.70));
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
    #[ignore] // Requires model files at ~/.cache/gliner-relex-onnx/backbone.onnx
    async fn test_relex_model_loading() {
        let mut engine = NEREngine::new();
        let result = engine.load_model().await;
        assert!(result.is_ok(), "Failed to load: {:?}", result.err());
        assert!(engine.is_loaded.load(Ordering::Relaxed));
    }

    #[tokio::test]
    #[ignore] // Requires model files
    async fn test_software_entity_extraction() {
        // Enable debug logging to see REL_SCORE lines
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init();

        let mut engine = NEREngine::new();
        engine.load_model().await.unwrap();

        // Short sentences — basic entity extraction
        let test_cases = vec![
            "Post-Cortex is an intelligent conversation memory system built with Rust.",
            "It uses RocksDB for persistent storage and HNSW for vector similarity search.",
            "We decided to replace DistilBERT with GLiNER2 for named entity recognition.",
            "Axon is a coding agent that connects to Post-Cortex via gRPC using tonic.",
            "PostgreSQL and Redis are used with axum and tower for the HTTP API.",
        ];

        for text in &test_cases {
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

        // Rich multi-sentence texts from test_context_assembly_pipeline — these
        // produce typed entity relations when the model sees explicit dependency
        // language ("depends on", "connects to", "required by", etc.)
        let pipeline_texts = vec![
            (
                "Axon architecture",
                "Axon is a coding agent built in Rust that connects to Post-Cortex via gRPC \
                 using tonic. Post-Cortex provides semantic search and entity graph storage. \
                 Axon depends on Post-Cortex for context assembly and knowledge recall. \
                 The gRPC service is implemented with tonic on port 3738.",
            ),
            (
                "Storage architecture",
                "Post-Cortex uses RocksDB for entity graph persistence and SurrealDB for \
                 relational queries. The FreshnessStorage trait depends on RocksDB for \
                 AST hash storage. SurrealDB implements the GraphStorage trait. \
                 Both storage backends are required by Post-Cortex for full functionality.",
            ),
            (
                "GLiNER-RelEx integration",
                "Replaced GLiNER2 with GLiNER-RelEx for joint NER and relation extraction. \
                 GLiNER-RelEx depends on the DeBERTa-v3-large encoder and ONNX Runtime for \
                 inference. The NER engine implements entity extraction which is required by \
                 the entity graph. The entity graph depends on NER engine for typed edges. \
                 ONNX Runtime is required by GLiNER-RelEx for model inference.",
            ),
            (
                "NER cache bug fix",
                "Fixed a critical bug in the NER cache. The NER cache previously stored only \
                 entities and lost all relations on cache hits. This caused the entity graph \
                 to have empty relationships. The fix changes the cache to store both entities \
                 and relations together. The NER cache depends on NER engine for extraction \
                 results. The entity graph depends on NER cache for cached lookups.",
            ),
            (
                "Tokio runtime decision",
                "Decided to use tokio multi-threaded runtime for all async operations in Axon. \
                 Key reasons: mature ecosystem, excellent performance, wide library support. \
                 tokio is required by tonic for the gRPC transport layer.",
            ),
            (
                "AssembleContext RPC",
                "Added graph-aware context assembly to Post-Cortex. The AssembleContext RPC \
                 traverses the entity graph, performs impact analysis on dependencies, and \
                 packs context items within a token budget using greedy knapsack.",
            ),
            (
                "Token budget strategy",
                "Context assembly uses a greedy knapsack algorithm to pack items within the \
                 token budget. Items are scored by recency and semantic relevance, then packed \
                 highest-score-first. The ContextAssembler depends on AssembleContext RPC. \
                 The token budget defaults to 4000 tokens for regular calls and 8000 for \
                 extended context requests.",
            ),
        ];

        let mut total_entities = 0;
        let mut total_relations = 0;

        for (title, text) in &pipeline_texts {
            println!("\n=== Pipeline text: {} ===", title);
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
            total_entities += entities.len();
            total_relations += relations.len();
        }

        println!("\n=== TOTALS: {} entities, {} relations ===", total_entities, total_relations);
        assert!(total_entities > 0, "Pipeline texts should produce entities");
        assert!(total_relations > 0, "Pipeline texts should produce at least some relations");
    }
}
