// Copyright (c) 2025 Julius ML
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
use crate::core::context_update::{ContextUpdate, EntityType, UpdateType};
use crate::core::structured_context::StructuredContext;
use crate::graph::entity_graph::SimpleEntityGraph;
use crate::session::session_components::{HotContext, SessionMetadata};

use chrono::DateTime;
use chrono::Utc;
use dashmap::DashSet;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(feature = "embeddings")]
use std::sync::OnceLock;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::time::timeout;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

#[cfg(feature = "embeddings")]
use crate::core::ner_engine::NEREngine;

// Global shared NER engine (lazy loaded on first use)
#[cfg(feature = "embeddings")]
static GLOBAL_NER_ENGINE: OnceLock<Arc<NEREngine>> = OnceLock::new();

// Global stop word sets for O(1) lookup (lazy initialized)
static ENGLISH_STOP_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "a", "an", "the", "this", "that", "these", "those", "in", "on", "at", "by", "for", "with",
        "from", "to", "of", "and", "or", "but", "nor", "so", "yet", "all", "some", "any", "each",
        "every", "both", "few", "many", "total", "using", "used", "uses", "use", "made", "make",
        "now", "then", "when", "where", "how", "what", "why", "one", "two", "three", "first",
        "second", "last",
        // Common English verbs that aren't meaningful entities
        "based", "returns", "generates", "creates", "uses", "contains", "includes", "provides",
        "requires", "supports", "handles", "processes", "represents", "implements", "defines",
        "specifies", "manages", "performs", "produces", "accepts", "receives", "sends", "stores",
        "loads", "reads", "writes", "calls", "checks", "finds", "gets", "sets", "takes", "makes",
        "gives", "keeps", "shows", "starts", "stops", "runs", "builds", "tests", "fixes", "adds",
        "removes", "updates", "changes", "moves", "needs", "works", "allows", "enables", "ensures",
        "helps", "serves", "follows", "leads", "holds", "passes", "turns", "pulls", "pushes",
        "drops", "wraps", "maps", "splits", "joins", "trims", "clips", "pads", "fills", "counts",
        "sorts", "filters", "parses", "formats", "renders", "draws", "prints", "clears", "resets",
        "inits", "spans", "truncate", "truncates", "truncated",
        // Common English adjectives/adverbs that aren't entities
        "new", "old", "good", "bad", "big", "small", "large", "high", "low", "full", "empty",
        "fast", "slow", "long", "short", "true", "false", "valid", "invalid", "enabled", "disabled",
        "simple", "complex", "basic", "advanced", "current", "previous", "next", "local", "remote",
        "public", "private", "internal", "external", "global", "default", "custom", "generic",
        "abstract", "concrete", "explicit", "implicit", "automatic", "manual", "optional", "required",
        // Generic nouns that are rarely meaningful as entities in technical text
        "line", "lines", "column", "columns", "row", "rows", "item", "items", "entry", "entries",
        "node", "nodes", "edge", "edges", "path", "paths", "point", "points", "step", "steps",
        "part", "parts", "side", "type", "types", "kind", "mode", "level", "layer", "stage",
        "state", "status", "event", "events", "action", "actions", "task", "tasks", "job", "jobs",
        "file", "files", "dir", "dirs", "name", "names", "label", "labels", "tag", "tags",
        "value", "values", "param", "params", "field", "fields", "attr", "attrs",
        // Programming words that appear everywhere but aren't specific entities
        "struct", "trait", "impl", "enum", "mod", "crate", "method", "function",
        "object", "class", "instance", "interface", "module", "package", "library",
        "code", "test", "tests", "spec", "docs", "doc", "comment", "comments",
        "body", "head", "tail", "list", "output", "input", "index", "limit",
        "count", "size", "length", "width", "height", "depth", "weight", "score",
        "response", "request", "message", "messages", "command", "commands",
        "argument", "arguments", "property", "properties", "parameter", "parameters",
        "element", "elements", "resource", "resources", "handler", "handlers",
        "callback", "callbacks", "iterator", "iterators", "receiver", "receiver",
        "endpoint", "endpoints", "variable", "variables", "constant", "constants",
        "operation", "operations", "component", "components", "reference", "references",
        "description", "implementation", "configuration", "connection", "connections",
        "directory", "directories", "position", "positions", "location", "locations",
        "expected", "original", "modified", "returned", "received", "selected",
        "existing", "matching", "starting", "stopping", "building", "creating",
        "updating", "removing", "checking", "tracking", "handling", "processing",
        "following", "resulting", "remaining", "containing", "including", "providing",
    ]
    .into_iter()
    .collect()
});

// Rust language keywords and common programming noise words — never valid entities
static PROGRAMMING_KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // Rust keywords
        "pub", "fn", "impl", "struct", "enum", "mod", "use", "let", "mut", "self", "super",
        "crate", "trait", "type", "where", "async", "await", "match", "return", "break",
        "continue", "loop", "for", "while", "if", "else", "ref", "move", "dyn", "box",
        "unsafe", "extern", "static", "const", "in", "as", "true", "false",
        // Generic type/value noise
        "str", "int", "bool", "void", "null", "none", "ok", "err", "num",
        "max", "min", "get", "set", "put", "del", "run", "log", "fmt", "std",
        "io", "fs", "os", "env", "buf", "tmp", "src", "dst", "len", "idx", "ptr",
        // Very short noise abbreviations
        "ctx", "req", "res", "msg", "cfg", "opt", "arg", "ret", "val", "var",
        "key", "map", "vec", "arr", "tbl", "col", "row", "pos",
    ]
    .into_iter()
    .collect()
});

static BULGARIAN_STOP_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // Common Bulgarian stop words
        "и",
        "в",
        "на",
        "за",
        "с",
        "от",
        "до",
        "по",
        "при",
        "без",
        "е",
        "са",
        "си",
        "съм",
        "беше",
        "бяха",
        "ще",
        "би",
        "това",
        "тази",
        "тези",
        "този",
        "онзи",
        "която",
        "който",
        "които",
        "как",
        "какво",
        "къде",
        "кога",
        "защо",
        "кой",
        "не",
        "да",
        "ли",
        "че",
        "ако",
        "когато",
        "докато",
        "или",
        "но",
        "а",
        "още",
        "само",
        "вече",
        "тук",
        "там",
        "всички",
        "всяка",
        "всеки",
        "няколко",
        "много",
        "малко",
        "го",
        "му",
        "й",
        "ги",
        "им",
        "ме",
        "ти",
        "ви",
        "ни",
        "един",
        "една",
        "едно",
        "два",
        "две",
        "три",
        // Common verbs
        "прави",
        "работи",
        "работят",
        "има",
        "имат",
        "мога",
        "може",
        "могат",
        "трябва",
        "искам",
        "иска",
        "използва",
        "използваме",
        "използвам",
        "използват",
        "осигурява",
        "осигуряват",
        "позволява",
        "позволяват",
        "оркестрира",
        "оркестрират",
        "върна",
        "връща",
        "връщат",
        "търсения",
        "търсене",
        "достъп",
        "данни",
        "параметри",
        "сесии",
        "сесия",
        "система",
        "системи",
        // Common prepositions and conjunctions
        "през",
        "след",
        "преди",
        "около",
        "между",
        "над",
        "под",
        "към",
        "чрез",
        "според",
        "заради",
        "поради",
        // Common adjectives and descriptors
        "различни",
        "различен",
        "различна",
        "различно",
        "семантични",
        "семантичен",
        "семантична",
        "семантично",
        "семантичните",
        "приблизително",
        "приблизителен",
        "приблизителна",
        "по-добър",
        "по-добра",
        "по-добро",
        "по-добре",
        "висока",
        "висок",
        "високо",
        "високи",
        "вместо",
        "индексът",
        "индекса",
        "индекси",
        "използване",
        "използването",
        // Common nouns that are too generic
        "начин",
        "начина",
        "начини",
        "вид",
        "вида",
        "нещо",
        "неща",
        "всичко",
        "нищо",
        // Noisy technical words
        "детайли",
        "обновяване",
        "контекст",
        "пример",
        "резултат",
        "функция",
        "метод",
        "клас",
    ]
    .into_iter()
    .collect()
});

static COMMON_ENGLISH_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "the",
        "and",
        "for",
        "are",
        "but",
        "not",
        "you",
        "all",
        "can",
        "had",
        "her",
        "was",
        "one",
        "our",
        "out",
        "day",
        "get",
        "has",
        "him",
        "his",
        "how",
        "its",
        "may",
        "new",
        "now",
        "old",
        "see",
        "two",
        "who",
        "boy",
        "did",
        "man",
        "men",
        "put",
        "say",
        "she",
        "too",
        "use",
        "this",
        "that",
        "with",
        "have",
        "from",
        "they",
        "know",
        "want",
        "been",
        "good",
        "much",
        "some",
        "time",
        "very",
        "when",
        "come",
        "here",
        "just",
        "like",
        "long",
        "make",
        "many",
        "over",
        "such",
        "take",
        "than",
        "them",
        "well",
        "were",
        "will",
        "your",
        "about",
        "after",
        "again",
        "back",
        "could",
        "every",
        "first",
        "going",
        "house",
        "little",
        "might",
        "never",
        "only",
        "other",
        "right",
        "should",
        "through",
        "under",
        "where",
        "while",
        "would",
        "write",
        "years",
        "because",
        "before",
        "being",
        "between",
        "during",
        "without",
        "within",
        "against",
        "across",
        "around",
        "behind",
        "beside",
        "beneath",
        "beyond",
        "inside",
        "outside",
        "toward",
        "towards",
        "underneath",
        "thing",
        "stuff",
        "something",
        "anything",
        "everything",
        // Noisy words from technical context
        "details",
        "storing",
        "tokens",
        "update",
        "context",
        "example",
        "result",
        "using",
        "into",
        "feature",
        "function",
        "method",
        "class",
        // Additional programming noise (common words appearing in code discussion)
        "string",
        "buffer",
        "vector",
        "array",
        "error",
        "value",
        "field",
        "param",
        "config",
        "option",
        "handle",
        "layer",
        "block",
        "chunk",
        "batch",
        "queue",
        "stack",
        "table",
        "record",
        "model",
        "schema",
        "query",
        "index",
        "entry",
        "token",
        "digit",
        "check",
        "valid",
        "match",
        "found",
        "given",
        "since",
        "along",
        "there",
        "these",
        "those",
        "their",
        "which",
        "where",
        "while",
        "whose",
    ]
    .into_iter()
    .collect()
});

/// Pre-load the global NER engine (call during daemon startup for best performance)
///
/// This function loads the DistilBERT-NER model into memory. Subsequent entity
/// extractions will use the loaded model automatically.
///
/// Returns true if NER engine was loaded successfully, false otherwise.
#[cfg(feature = "embeddings")]
pub async fn preload_ner_engine() -> bool {
    get_ner_engine().await.is_some()
}

/// Get or initialize the global NER engine (lazy loading)
#[cfg(feature = "embeddings")]
async fn get_ner_engine() -> Option<Arc<NEREngine>> {
    // Fast path - engine already loaded
    if let Some(engine) = GLOBAL_NER_ENGINE.get() {
        return Some(engine.clone());
    }

    // Slow path - load engine (first call only)
    info!("Loading global NER engine for first time...");
    let mut engine = NEREngine::new();
    match engine.load_model().await {
        Ok(_) => {
            info!("Global NER engine loaded successfully");
            let arc_engine = Arc::new(engine);
            // Try to set (may fail if another thread set it first, but that's ok)
            let _ = GLOBAL_NER_ENGINE.set(arc_engine.clone());
            Some(arc_engine)
        }
        Err(e) => {
            warn!(
                "Failed to load NER engine: {}. Entity extraction will use fallback method.",
                e
            );
            None
        }
    }
}

/// ActiveSession with lock-free granular components
/// Uses Arc-wrapped lock-free structures for concurrent access and cheap cloning
///
/// **Copy-on-Write (CoW) Semantics:**
/// Heavy fields are wrapped in Arc for efficient cloning. When the session needs
/// to be modified, use `Arc::make_mut()` which will:
/// - Return a mutable reference if this is the only owner
/// - Clone the data only if there are other owners
///
/// This dramatically reduces cloning overhead when sessions are frequently updated.
#[derive(Clone, Debug)]
pub struct ActiveSession {
    // Metadata (immutable or rare updates)
    pub metadata: Arc<SessionMetadata>,
    pub last_updated: DateTime<Utc>,

    // Lock-free tiered context storage
    pub hot_context: Arc<HotContext>, // Lock-free hot updates (DashMap-based)
    pub warm_context: Arc<Vec<CompressedUpdate>>, // CoW: Compressed updates (storage)
    pub cold_context: Arc<Vec<StructuredSummary>>, // CoW: Periodic summaries (storage)

    // Structured context - CoW wrapped for efficient updates
    pub current_state: Arc<StructuredContext>, // CoW: Current queryable state
    pub incremental_updates: Arc<Vec<ContextUpdate>>, // CoW: All incremental updates (biggest!)

    // Code integration - CoW wrapped
    pub code_references: Arc<HashMap<String, Vec<CodeReference>>>, // CoW: By file path
    pub change_history: Arc<Vec<ChangeRecord>>,                    // CoW: Change history

    // Entity graph - CoW wrapped for efficient graph updates
    pub entity_graph: Arc<SimpleEntityGraph>,

    // Entity extraction configuration (small, cheap to clone)
    pub max_extracted_entities: usize,
    pub max_referenced_entities: usize,
    pub enable_smart_entity_ranking: bool,

    // Entity extraction metrics (small, cheap to clone)
    pub total_entity_truncations: usize,
    pub total_entities_truncated: usize,

    // Vectorization tracking (lock-free set for concurrent access)
    pub vectorized_update_ids: Arc<DashSet<Uuid>>,
}

// Serialization helper - contains data in serializable form
#[derive(Serialize, Deserialize)]
struct ActiveSessionData {
    id: Uuid,
    name: Option<String>,
    description: Option<String>,
    created_at: DateTime<Utc>,
    last_updated: DateTime<Utc>,
    user_preferences: UserPreferences,
    hot_context: VecDeque<ContextUpdate>,
    warm_context: Vec<CompressedUpdate>,
    cold_context: Vec<StructuredSummary>,
    current_state: StructuredContext,
    incremental_updates: Vec<ContextUpdate>,
    code_references: HashMap<String, Vec<CodeReference>>,
    change_history: Vec<ChangeRecord>,
    entity_graph: SimpleEntityGraph,
    #[serde(default = "default_max_entities")]
    max_extracted_entities: usize,
    #[serde(default = "default_max_entities")]
    max_referenced_entities: usize,
    #[serde(default = "default_true")]
    enable_smart_entity_ranking: bool,
    #[serde(default)]
    total_entity_truncations: usize,
    #[serde(default)]
    total_entities_truncated: usize,
    #[serde(default)]
    vectorized_update_ids: Vec<Uuid>,
}

fn default_max_entities() -> usize {
    15
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CompressedUpdate {
    pub update: ContextUpdate,
    pub compression_ratio: f32,
    pub compressed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StructuredSummary {
    pub summary_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub context_snapshot: StructuredContext,
    pub referenced_updates: Vec<Uuid>,
    pub summary_quality: f32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CodeReference {
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub code_snippet: String,
    pub commit_hash: Option<String>,
    pub branch: Option<String>,
    pub change_description: String,
}

// Remove duplicate CodeReference definition since we're using the one from core
// Removed duplicate field declarations - using CodeReference from core::context_update
// No extra closing brace needed here

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ChangeRecord {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub change_type: String,
    pub description: String,
    pub related_update_id: Option<Uuid>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UserPreferences {
    pub auto_save_enabled: bool,
    pub context_retention_days: u32,
    pub max_hot_context_size: usize,
    pub auto_summary_threshold: usize,
    pub important_keywords: Vec<String>,
}

// Custom Serialize implementation - extract data from Arc-wrapped components
impl Serialize for ActiveSession {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Dereference Arc before cloning for serialization
        let data = ActiveSessionData {
            id: self.metadata.id,
            name: self.metadata.name.clone(),
            description: self.metadata.description.clone(),
            created_at: self.metadata.created_at,
            last_updated: self.last_updated,
            user_preferences: self.metadata.user_preferences.clone(),
            hot_context: VecDeque::from(self.hot_context.snapshot()),
            warm_context: (*self.warm_context).clone(),
            cold_context: (*self.cold_context).clone(),
            current_state: (*self.current_state).clone(),
            incremental_updates: (*self.incremental_updates).clone(),
            code_references: (*self.code_references).clone(),
            change_history: (*self.change_history).clone(),
            entity_graph: (*self.entity_graph).clone(),
            max_extracted_entities: self.max_extracted_entities,
            max_referenced_entities: self.max_referenced_entities,
            enable_smart_entity_ranking: self.enable_smart_entity_ranking,
            total_entity_truncations: self.total_entity_truncations,
            total_entities_truncated: self.total_entities_truncated,
            vectorized_update_ids: self.vectorized_update_ids.iter().map(|id| *id).collect(),
        };
        data.serialize(serializer)
    }
}

// Custom Deserialize implementation - reconstruct Arc-wrapped components
impl<'de> Deserialize<'de> for ActiveSession {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let data = ActiveSessionData::deserialize(deserializer)?;

        let max_hot_size = data.user_preferences.max_hot_context_size;

        let metadata = Arc::new(SessionMetadata::new(
            data.id,
            data.name,
            data.description,
            data.user_preferences,
        ));

        let hot_context = Arc::new(HotContext::from_deque(data.hot_context, max_hot_size));

        // Reconstruct vectorized_update_ids DashSet from Vec
        let vectorized_ids = Arc::new(DashSet::new());
        for id in data.vectorized_update_ids {
            vectorized_ids.insert(id);
        }

        // Wrap deserialized data in Arc for CoW semantics
        Ok(ActiveSession {
            metadata,
            last_updated: data.last_updated,
            hot_context,
            warm_context: Arc::new(data.warm_context),
            cold_context: Arc::new(data.cold_context),
            current_state: Arc::new(data.current_state),
            incremental_updates: Arc::new(data.incremental_updates),
            code_references: Arc::new(data.code_references),
            change_history: Arc::new(data.change_history),
            entity_graph: Arc::new(data.entity_graph),
            max_extracted_entities: data.max_extracted_entities,
            max_referenced_entities: data.max_referenced_entities,
            enable_smart_entity_ranking: data.enable_smart_entity_ranking,
            total_entity_truncations: data.total_entity_truncations,
            total_entities_truncated: data.total_entities_truncated,
            vectorized_update_ids: vectorized_ids,
        })
    }
}

impl Default for ActiveSession {
    fn default() -> Self {
        Self::new(Uuid::new_v4(), None, None)
    }
}

impl ActiveSession {
    pub fn new(id: Uuid, name: Option<String>, description: Option<String>) -> Self {
        let user_preferences = UserPreferences {
            auto_save_enabled: true,
            context_retention_days: 30,
            max_hot_context_size: 50,
            auto_summary_threshold: 100,
            important_keywords: vec![],
        };

        let metadata = Arc::new(SessionMetadata::new(
            id,
            name,
            description,
            user_preferences.clone(),
        ));

        Self {
            metadata,
            last_updated: Utc::now(),
            hot_context: Arc::new(HotContext::new(50)),
            warm_context: Arc::new(Vec::new()),
            cold_context: Arc::new(Vec::new()),
            current_state: Arc::new(StructuredContext::new()),
            incremental_updates: Arc::new(Vec::new()),
            code_references: Arc::new(HashMap::new()),
            change_history: Arc::new(Vec::new()),
            entity_graph: Arc::new(SimpleEntityGraph::new()),
            max_extracted_entities: 15,
            max_referenced_entities: 15,
            enable_smart_entity_ranking: true,
            total_entity_truncations: 0,
            total_entities_truncated: 0,
            vectorized_update_ids: Arc::new(DashSet::new()),
        }
    }

    /// Reconstruct an ActiveSession from individual components (used for SurrealDB native storage)
    #[allow(clippy::too_many_arguments)]
    pub fn from_components(
        id: Uuid,
        name: Option<String>,
        description: Option<String>,
        created_at: DateTime<Utc>,
        last_updated: DateTime<Utc>,
        user_preferences: UserPreferences,
        hot_context_vec: Vec<ContextUpdate>,
        warm_context: Vec<CompressedUpdate>,
        cold_context: Vec<StructuredSummary>,
        current_state: StructuredContext,
        incremental_updates: Vec<ContextUpdate>,
        code_references: HashMap<String, Vec<CodeReference>>,
        change_history: Vec<ChangeRecord>,
        entity_graph: SimpleEntityGraph,
        max_extracted_entities: usize,
        max_referenced_entities: usize,
        enable_smart_entity_ranking: bool,
        total_entity_truncations: usize,
        total_entities_truncated: usize,
        vectorized_update_ids: Vec<Uuid>,
    ) -> Self {
        let max_hot_size = user_preferences.max_hot_context_size;

        let mut metadata = SessionMetadata::new(id, name, description, user_preferences);
        metadata.created_at = created_at;

        let hot_context = HotContext::from_deque(VecDeque::from(hot_context_vec), max_hot_size);

        let vectorized_ids = Arc::new(DashSet::new());
        for vid in vectorized_update_ids {
            vectorized_ids.insert(vid);
        }

        Self {
            metadata: Arc::new(metadata),
            last_updated,
            hot_context: Arc::new(hot_context),
            warm_context: Arc::new(warm_context),
            cold_context: Arc::new(cold_context),
            current_state: Arc::new(current_state),
            incremental_updates: Arc::new(incremental_updates),
            code_references: Arc::new(code_references),
            change_history: Arc::new(change_history),
            entity_graph: Arc::new(entity_graph),
            max_extracted_entities,
            max_referenced_entities,
            enable_smart_entity_ranking,
            total_entity_truncations,
            total_entities_truncated,
            vectorized_update_ids: vectorized_ids,
        }
    }

    // Convenience getters for metadata fields
    pub fn id(&self) -> Uuid {
        self.metadata.id
    }

    pub fn name(&self) -> Option<String> {
        self.metadata.name.clone()
    }

    pub fn description(&self) -> Option<String> {
        self.metadata.description.clone()
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        self.metadata.created_at
    }

    pub fn user_preferences(&self) -> &UserPreferences {
        &self.metadata.user_preferences
    }

    #[instrument(skip(self, update), fields(session_id = %self.id()))]
    pub async fn add_incremental_update(&mut self, update: ContextUpdate) -> anyhow::Result<()> {
        info!(
            "ActiveSession: Starting add_incremental_update for update ID: {}",
            update.id
        );
        info!("Update type: {:?}", update.update_type);
        info!("Content title: '{}'", update.content.title);
        info!("Content description: '{}'", update.content.description);

        // Limit content size to prevent processing issues
        let mut limited_update = update.clone();
        if limited_update.content.description.len() > 2000 {
            limited_update.content.description.truncate(1800);
            limited_update
                .content
                .description
                .push_str("... (truncated)");
            warn!("ActiveSession: Content description truncated to prevent timeout");
        }
        if limited_update.content.title.len() > 200 {
            limited_update.content.title.truncate(190);
            limited_update.content.title.push_str("...");
        }

        // Add to hot context (lock-free)
        debug!("ActiveSession: Adding to hot context");
        self.hot_context.push(limited_update.clone());
        debug!(
            "ActiveSession: Hot context updated, size: {}",
            self.hot_context.len()
        );

        // Update structured state with timeout
        debug!("ActiveSession: Calling update_current_state");
        match timeout(
            Duration::from_secs(3),
            self.update_current_state(&limited_update),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                warn!("ActiveSession: update_current_state timed out");
                return Err(anyhow::anyhow!("Current state update timeout"));
            }
        }
        debug!("ActiveSession: update_current_state completed");

        // Update entity graph with timeout
        debug!("ActiveSession: Calling update_entity_graph");
        match timeout(
            Duration::from_secs(5),
            self.update_entity_graph(&limited_update),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                warn!("ActiveSession: update_entity_graph timed out");
                return Err(anyhow::anyhow!("Entity graph update timeout"));
            }
        }
        debug!("ActiveSession: update_entity_graph completed");

        // Add code reference if present with timeout
        debug!("ActiveSession: checking related_code reference");
        if let Some(code_ref) = &limited_update.related_code {
            debug!("ActiveSession: Code reference found, processing...");
            let code_ref_clone = CodeReference {
                file_path: code_ref.file_path.clone(),
                start_line: code_ref.start_line,
                end_line: code_ref.end_line,
                code_snippet: code_ref.code_snippet.clone(),
                commit_hash: code_ref.commit_hash.clone(),
                branch: code_ref.branch.clone(),
                change_description: code_ref.change_description.clone(),
            };
            debug!("ActiveSession: Calling add_code_reference");
            match timeout(
                Duration::from_secs(2),
                self.add_code_reference(&code_ref_clone),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => {
                    warn!("ActiveSession: add_code_reference timed out");
                    // Continue without failing the entire operation
                }
            }
            debug!("ActiveSession: add_code_reference completed");
        } else {
            debug!("ActiveSession: No code reference in update");
        }

        // Record change (sync now)
        debug!("ActiveSession: Calling record_change");
        self.record_change(&limited_update)?;
        debug!("ActiveSession: record_change completed");

        // Maintain context size (sync now)
        debug!("ActiveSession: Calling maintain_context");
        self.maintain_context()?;
        debug!("ActiveSession: maintain_context completed");

        // Update last updated timestamp
        debug!("ActiveSession: Updating timestamp");
        self.last_updated = Utc::now();

        // Add to incremental updates (use original update for storage)
        // Use Arc::make_mut for CoW semantics - only clones if there are other owners
        Arc::make_mut(&mut self.incremental_updates).push(limited_update.clone());

        info!("ActiveSession: add_incremental_update completed successfully");

        Ok(())
    }

    /// Fast path: same as add_incremental_update but skips update_entity_graph.
    /// Entity graph update should be applied separately via apply_entity_graph_update.
    pub async fn add_incremental_update_fast(
        &mut self,
        update: ContextUpdate,
    ) -> anyhow::Result<()> {
        debug!(
            "ActiveSession: Starting add_incremental_update_fast for update ID: {}",
            update.id
        );

        // Limit content size to prevent processing issues
        let mut limited_update = update.clone();
        if limited_update.content.description.len() > 2000 {
            limited_update.content.description.truncate(1800);
            limited_update
                .content
                .description
                .push_str("... (truncated)");
            warn!("ActiveSession: Content description truncated to prevent timeout");
        }
        if limited_update.content.title.len() > 200 {
            limited_update.content.title.truncate(190);
            limited_update.content.title.push_str("...");
        }

        // Add to hot context (lock-free)
        self.hot_context.push(limited_update.clone());

        // Update structured state with timeout
        match timeout(
            Duration::from_secs(3),
            self.update_current_state(&limited_update),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                warn!("ActiveSession: update_current_state timed out");
                return Err(anyhow::anyhow!("Current state update timeout"));
            }
        }

        // Add code reference if present with timeout
        if let Some(code_ref) = &limited_update.related_code {
            let code_ref_clone = CodeReference {
                file_path: code_ref.file_path.clone(),
                start_line: code_ref.start_line,
                end_line: code_ref.end_line,
                code_snippet: code_ref.code_snippet.clone(),
                commit_hash: code_ref.commit_hash.clone(),
                branch: code_ref.branch.clone(),
                change_description: code_ref.change_description.clone(),
            };
            match timeout(
                Duration::from_secs(2),
                self.add_code_reference(&code_ref_clone),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => {
                    warn!("ActiveSession: add_code_reference timed out");
                }
            }
        }

        // Record change + maintain context (sync, cheap)
        self.record_change(&limited_update)?;
        self.maintain_context()?;
        self.last_updated = Utc::now();

        // Add to incremental updates (CoW)
        Arc::make_mut(&mut self.incremental_updates).push(limited_update);

        debug!("ActiveSession: add_incremental_update_fast completed successfully");

        Ok(())
    }

    /// Apply entity graph update only. Used as background task after CAS success.
    pub async fn apply_entity_graph_update(
        &mut self,
        update: &ContextUpdate,
    ) -> anyhow::Result<()> {
        match timeout(Duration::from_secs(5), self.update_entity_graph(update)).await {
            Ok(result) => result?,
            Err(_) => {
                warn!("ActiveSession: background update_entity_graph timed out");
            }
        }
        Ok(())
    }

    /// Rebuild the entity graph by clearing it and replaying all updates through NER extraction.
    /// Returns (entities_before, entities_after) counts.
    pub async fn rebuild_entity_graph_from_updates(&mut self) -> anyhow::Result<(usize, usize)> {
        let entity_graph = Arc::make_mut(&mut self.entity_graph);
        let entities_before = entity_graph.entity_count();
        entity_graph.clear();

        let updates: Vec<ContextUpdate> = self.incremental_updates.as_ref().clone();
        let total = updates.len();
        info!(
            "Rebuilding entity graph: {} updates to process, {} entities cleared",
            total, entities_before
        );

        for (i, update) in updates.iter().enumerate() {
            if (i + 1) % 10 == 0 || i + 1 == total {
                info!("Rebuilding entity graph: {}/{} updates processed", i + 1, total);
            }
            self.update_entity_graph(update).await?;
        }

        let entities_after = self.entity_graph.entity_count();
        info!(
            "Entity graph rebuild complete: {} -> {} entities",
            entities_before, entities_after
        );
        Ok((entities_before, entities_after))
    }

    async fn update_current_state(&mut self, update: &ContextUpdate) -> anyhow::Result<()> {
        // Use Arc::make_mut for CoW semantics on current_state
        // This will only clone if there are other Arc references
        let current_state = Arc::make_mut(&mut self.current_state);

        // Update structured context based on update type
        match &update.update_type {
            UpdateType::QuestionAnswered => {
                // Add question to open questions (since Q&A implies there was a question)
                current_state
                    .open_questions
                    .push(crate::core::structured_context::QuestionItem {
                        question: update.content.title.clone(),
                        context: update.content.description.clone(),
                        status: crate::core::structured_context::QuestionStatus::Answered,
                        timestamp: update.timestamp,
                        last_updated: update.timestamp,
                    });

                // Extract key concepts from Q&A content
                Self::extract_and_add_concepts_to_state(
                    current_state,
                    &update.content.title,
                    &update.content.description,
                    &update.content.details,
                    update.timestamp,
                );

                // Add to conversation flow
                current_state
                    .conversation_flow
                    .push(crate::core::structured_context::FlowItem {
                        step_description: format!("Q&A: {}", update.content.title),
                        timestamp: update.timestamp,
                        related_updates: vec![update.id],
                        outcome: Some(update.content.description.clone()),
                    });
            }
            UpdateType::ProblemSolved => {
                // Extract key concepts from problem/solution content
                Self::extract_and_add_concepts_to_state(
                    current_state,
                    &update.content.title,
                    &update.content.description,
                    &update.content.details,
                    update.timestamp,
                );

                // Add to conversation flow
                current_state
                    .conversation_flow
                    .push(crate::core::structured_context::FlowItem {
                        step_description: format!("Problem Solved: {}", update.content.title),
                        timestamp: update.timestamp,
                        related_updates: vec![update.id],
                        outcome: Some(update.content.description.clone()),
                    });
            }
            UpdateType::CodeChanged => {
                // Extract key concepts from code change content
                Self::extract_and_add_concepts_to_state(
                    current_state,
                    &update.content.title,
                    &update.content.description,
                    &update.content.details,
                    update.timestamp,
                );

                // Add to conversation flow
                current_state
                    .conversation_flow
                    .push(crate::core::structured_context::FlowItem {
                        step_description: format!("Code Change: {}", update.content.title),
                        timestamp: update.timestamp,
                        related_updates: vec![update.id],
                        outcome: Some(update.content.description.clone()),
                    });
            }
            UpdateType::DecisionMade => {
                // Add to key decisions
                current_state
                    .key_decisions
                    .push(crate::core::structured_context::DecisionItem {
                        description: update.content.title.clone(),
                        context: update.content.description.clone(),
                        alternatives: update.content.details.clone(),
                        confidence: 1.0,
                        timestamp: update.timestamp,
                    });

                // Add to conversation flow
                current_state
                    .conversation_flow
                    .push(crate::core::structured_context::FlowItem {
                        step_description: format!("Decision Made: {}", update.content.title),
                        timestamp: update.timestamp,
                        related_updates: vec![update.id],
                        outcome: Some(update.content.description.clone()),
                    });
            }
            UpdateType::ConceptDefined => {
                // Add to key concepts
                current_state
                    .key_concepts
                    .push(crate::core::structured_context::ConceptItem {
                        name: update.content.title.clone(),
                        definition: update.content.description.clone(),
                        examples: update.content.examples.clone(),
                        related_concepts: update.content.details.clone(),
                        timestamp: update.timestamp,
                    });

                // Add to conversation flow
                current_state
                    .conversation_flow
                    .push(crate::core::structured_context::FlowItem {
                        step_description: format!("Concept Defined: {}", update.content.title),
                        timestamp: update.timestamp,
                        related_updates: vec![update.id],
                        outcome: Some(update.content.description.clone()),
                    });
            }
            UpdateType::RequirementAdded => {
                // Add to technical specifications
                current_state.technical_specifications.push(
                    crate::core::structured_context::SpecItem {
                        title: update.content.title.clone(),
                        description: update.content.description.clone(),
                        requirements: update.content.details.clone(),
                        constraints: update.content.implications.clone(),
                        timestamp: update.timestamp,
                    },
                );

                // Add to conversation flow
                current_state
                    .conversation_flow
                    .push(crate::core::structured_context::FlowItem {
                        step_description: format!("Requirement Added: {}", update.content.title),
                        timestamp: update.timestamp,
                        related_updates: vec![update.id],
                        outcome: Some(update.content.description.clone()),
                    });
            }
        }

        Ok(())
    }

    /// Extract key concepts from content and add them to key_concepts (static version for CoW)
    /// Takes a mutable reference to StructuredContext to avoid borrowing issues with Arc::make_mut
    fn extract_and_add_concepts_to_state(
        current_state: &mut StructuredContext,
        title: &str,
        description: &str,
        details: &[String],
        timestamp: chrono::DateTime<Utc>,
    ) {
        use std::collections::HashSet;

        let mut concept_entities = HashSet::new();
        let full_text = format!("{} {} {}", title, description, details.join(" "));

        // Extract entities as potential concepts using static helper
        let extracted_entities = Self::extract_entities_from_text_static(&full_text);

        // Filter for concept-worthy entities (score threshold)
        for entity in extracted_entities {
            let score = Self::calculate_entity_score_static(&entity, &full_text);
            // Lower threshold to include more concepts (was 2.5, now 1.5)
            if score >= 1.5 && entity.len() >= 3 && entity.len() <= 25 {
                concept_entities.insert(entity);
            }
        }

        // Also extract explicit concept indicators
        Self::extract_explicit_concepts_static(&full_text, &mut concept_entities);

        // Convert to ConceptItem structures and add to key_concepts
        for concept_name in concept_entities {
            // Skip if this concept already exists (avoid duplicates)
            if current_state
                .key_concepts
                .iter()
                .any(|c| c.name.to_lowercase() == concept_name.to_lowercase())
            {
                continue;
            }

            current_state
                .key_concepts
                .push(crate::core::structured_context::ConceptItem {
                    name: concept_name.clone(),
                    definition: format!("Key concept extracted from: {}", title),
                    examples: vec![],
                    related_concepts: vec![],
                    timestamp,
                });

            // Limit the number of concepts to prevent overflow
            if current_state.key_concepts.len() >= 50 {
                current_state.key_concepts.remove(0); // Remove oldest
            }
        }
    }

    async fn update_entity_graph(&mut self, update: &ContextUpdate) -> anyhow::Result<()> {
        info!(
            "update_entity_graph: Starting entity graph update for update {}",
            update.id
        );

        // Extract entities from update content if not explicitly provided
        let mut extracted_entities = update.creates_entities.clone();
        let mut referenced_entities = update.references_entities.clone();
        debug!(
            "update_entity_graph: {} created entities, {} referenced entities",
            extracted_entities.len(),
            referenced_entities.len()
        );

        info!(
            "Explicit entities: creates={}, references={}",
            extracted_entities.len(),
            referenced_entities.len()
        );

        // If no entities were explicitly provided, extract from content
        if extracted_entities.is_empty() && referenced_entities.is_empty() {
            let content_text = format!(
                "{} {} {}",
                update.content.title,
                update.content.description,
                update.content.details.join(" ")
            );
            info!("Extracting entities from text: '{}'", content_text);

            let auto_extracted = self.extract_entities_from_text(&content_text);
            info!(
                "Auto-extracted {} entities: {:?}",
                auto_extracted.len(),
                auto_extracted
            );

            extracted_entities.extend(auto_extracted.clone());
            referenced_entities.extend(auto_extracted);
        }

        // Prepare content text for entity scoring
        let content_text = format!(
            "{} {} {}",
            update.content.title,
            update.content.description,
            update.content.details.join(" ")
        );

        // Smart ranking and truncation of entities
        let (ranked_extracted, extracted_truncated) = self.rank_and_truncate_entities(
            extracted_entities,
            &content_text,
            self.max_extracted_entities,
            self.enable_smart_entity_ranking,
        );
        let (ranked_referenced, referenced_truncated) = self.rank_and_truncate_entities(
            referenced_entities,
            &content_text,
            self.max_referenced_entities,
            self.enable_smart_entity_ranking,
        );

        extracted_entities = ranked_extracted;
        referenced_entities = ranked_referenced;

        // Update truncation metrics
        let total_truncated = extracted_truncated + referenced_truncated;
        if total_truncated > 0 {
            self.total_entity_truncations += 1;
            self.total_entities_truncated += total_truncated;
        }

        info!(
            "Final entity counts - extracted: {} (truncated: {}), referenced: {} (truncated: {}). Total session truncations: {}, total entities lost: {}",
            extracted_entities.len(),
            extracted_truncated,
            referenced_entities.len(),
            referenced_truncated,
            self.total_entity_truncations,
            self.total_entities_truncated
        );

        // Pre-compute entity types and relationships before taking mutable borrow of entity_graph
        // This avoids borrowing self while entity_graph is mutably borrowed
        let mut entity_type_map = std::collections::HashMap::new();

        for name in &extracted_entities {
            entity_type_map.insert(
                name.clone(),
                self.infer_entity_type(&update.update_type, name),
            );
        }
        for name in &referenced_entities {
            if !entity_type_map.contains_key(name) {
                entity_type_map.insert(
                    name.clone(),
                    self.infer_entity_type(&update.update_type, name),
                );
            }
        }

        // Combine all known entities for relationship extraction
        let mut all_entities = extracted_entities.clone();
        all_entities.extend(referenced_entities.clone());
        all_entities.sort();
        all_entities.dedup();

        // Extract relationships from text (heuristic)
        let extracted_rels = if !all_entities.is_empty() {
            self.extract_relationships_from_text(&content_text, &all_entities)
        } else {
            Vec::new()
        };

        // Take mutable borrow of entity_graph using CoW semantics
        let entity_graph = Arc::make_mut(&mut self.entity_graph);

        // Add extracted entities to graph
        info!(
            "DEBUG: Adding {} extracted entities to graph",
            extracted_entities.len()
        );
        for name in &extracted_entities {
            let entity_type = entity_type_map.get(name).unwrap().clone();
            entity_graph.add_or_update_entity(
                name.clone(),
                entity_type,
                update.timestamp,
                &format!("Extracted from: {}", update.content.title),
            );
        }
        debug!("update_entity_graph: extracted entities added");

        // Update referenced entities
        debug!(
            "update_entity_graph: updating {} referenced entities",
            referenced_entities.len()
        );
        for name in &referenced_entities {
            // Only update timestamp if exists
            if entity_graph.has_entity(name) {
                entity_graph.update_entity_timestamp(name, update.timestamp);
            } else {
                // If referenced but not exists, add it (weak inference)
                let entity_type = entity_type_map.get(name).unwrap().clone();
                entity_graph.add_or_update_entity(
                    name.clone(),
                    entity_type,
                    update.timestamp,
                    &format!("Referenced in: {}", update.content.title),
                );
            }
        }
        debug!("update_entity_graph: entity references added");

        // Create explicit relationships
        debug!(
            "update_entity_graph: creating {} explicit relationships",
            update.creates_relationships.len()
        );
        for relationship in &update.creates_relationships {
            entity_graph.add_relationship(relationship.clone());
        }
        debug!("update_entity_graph: explicit relationships created");

        // Add heuristically extracted relationships
        debug!(
            "update_entity_graph: adding {} heuristic relationships",
            extracted_rels.len()
        );
        for rel in extracted_rels {
            entity_graph.add_relationship(rel);
        }

        // NOTE: Removed auto-generation of "Co-mentioned" relationships.
        // This was creating noise (99%+ useless relations) by linking ALL entities
        // from the same update. Only explicit relationships from text analysis
        // (extracted_rels above) are now added.

        debug!("update_entity_graph: completed successfully");
        Ok(())
    }

    fn infer_entity_type(&self, update_type: &UpdateType, entity_name: &str) -> EntityType {
        match update_type {
            UpdateType::CodeChanged => EntityType::CodeComponent,
            UpdateType::ProblemSolved => EntityType::Solution,
            UpdateType::DecisionMade => EntityType::Decision,
            UpdateType::ConceptDefined => EntityType::Concept,
            _ => {
                // Enhanced heuristics based on entity name
                let name_lower = entity_name.to_lowercase();

                // Technologies
                if name_lower.contains("rust")
                    || name_lower.contains("cargo")
                    || name_lower.contains("postgresql")
                    || name_lower.contains("postgres")
                    || name_lower.contains("jwt")
                    || name_lower.contains("json")
                    || name_lower.contains("api")
                    || name_lower.contains("http")
                    || name_lower.contains("sql")
                    || name_lower.contains("database")
                    || name_lower.contains("redis")
                    || name_lower.contains("docker")
                    || name_lower.contains("kubernetes")
                    || name_lower.contains("git")
                    || name_lower.contains("server")
                    || name_lower.contains("client")
                    || name_lower.ends_with(".rs")
                    || name_lower.ends_with(".sql")
                    || name_lower.ends_with(".toml")
                    || name_lower.ends_with(".json")
                {
                    EntityType::Technology
                }
                // Problems
                else if name_lower.contains("bug")
                    || name_lower.contains("issue")
                    || name_lower.contains("problem")
                    || name_lower.contains("error")
                    || name_lower.contains("fail")
                    || name_lower.contains("crash")
                    || name_lower.contains("exception")
                    || name_lower.contains("panic")
                {
                    EntityType::Problem
                }
                // Code components
                else if name_lower.contains("function")
                    || name_lower.contains("method")
                    || name_lower.contains("struct")
                    || name_lower.contains("enum")
                    || name_lower.contains("trait")
                    || name_lower.contains("impl")
                    || name_lower.contains("module")
                    || name_lower.contains("lib")
                    || name_lower.contains("crate")
                    || name_lower.contains("package")
                    || name_lower.contains("/")
                    || name_lower.contains("::")
                {
                    EntityType::CodeComponent
                }
                // Solutions
                else if name_lower.contains("fix")
                    || name_lower.contains("solution")
                    || name_lower.contains("resolve")
                    || name_lower.contains("implement")
                    || name_lower.contains("patch")
                    || name_lower.contains("update")
                {
                    EntityType::Solution
                }
                // Default to concept
                else {
                    EntityType::Concept
                }
            }
        }
    }

    // ========== Entity Intelligence Helpers ==========

    /// Clean entity name by removing punctuation (safe, defensive)
    fn clean_entity_name(&self, name: &str) -> Option<String> {
        if name.is_empty() {
            return None;
        }

        let cleaned = name
            .trim()
            .trim_end_matches(|c: char| c.is_ascii_punctuation() && c != '(' && c != ')')
            .trim_start_matches(|c: char| c.is_ascii_punctuation() && c != '(' && c != ')')
            .trim();

        if cleaned.is_empty() {
            return None;
        }

        Some(cleaned.to_string())
    }

    /// Check if entity name is a stop word (O(1) HashSet lookup)
    fn is_stop_word(&self, name: &str) -> bool {
        if name.is_empty() {
            return true;
        }

        let normalized = name.to_lowercase();
        ENGLISH_STOP_WORDS.contains(normalized.as_str())
            || PROGRAMMING_KEYWORDS.contains(normalized.as_str())
            || normalized.parse::<f64>().is_ok()
    }

    /// Normalize entity name (safe, returns None on error)
    fn normalize_entity(&self, name: &str) -> Option<String> {
        let cleaned = self.clean_entity_name(name)?;

        // Remove function call parentheses
        let mut normalized = if cleaned.ends_with("()") {
            cleaned.trim_end_matches("()").to_string()
        } else {
            cleaned
        };

        // Safety check after removing ()
        if normalized.is_empty() {
            return None;
        }

        // Normalize common technical terms
        let lower = normalized.to_lowercase();
        normalized = match lower.as_str() {
            "rwlock" | "rw_lock" => "RwLock".to_string(),
            "mutex" => "Mutex".to_string(),
            "arc" => "Arc".to_string(),
            "hashmap" | "hash_map" => "HashMap".to_string(),
            "vec" => "Vec".to_string(),
            "string" => "String".to_string(),
            "option" => "Option".to_string(),
            "result" => "Result".to_string(),
            _ => normalized,
        };

        Some(normalized)
    }

    /// Validate entity (safe, defensive checks)
    fn is_valid_entity(&self, name: &str) -> bool {
        if name.is_empty() {
            return false;
        }

        let cleaned = match self.clean_entity_name(name) {
            Some(c) => c,
            None => return false,
        };

        if cleaned.len() < 2 || cleaned.len() > 50 {
            return false;
        }

        if self.is_stop_word(&cleaned) {
            return false;
        }

        // Reject pure numbers and number-like tokens (384, 1024, 50kb, 8ms, 66-84)
        let alpha_count = cleaned.chars().filter(|c| c.is_alphabetic()).count();
        if alpha_count < 3 {
            return false;
        }

        // Reject pure all-lowercase words under 8 chars with no structural indicators.
        // A word passes if it:
        //   - has a capital letter (CamelCase / acronym / proper noun), OR
        //   - contains an underscore (snake_case), OR
        //   - contains a dot (path like std.fs), OR
        //   - is at least 8 chars long (library names like surrealdb, petgraph)
        let has_capital = cleaned.chars().any(|c| c.is_uppercase());
        let has_underscore = cleaned.contains('_');
        let has_dot = cleaned.contains('.');

        if !has_capital && !has_underscore && !has_dot && cleaned.len() < 8 {
            return false;
        }

        true
    }

    // ========== End Entity Intelligence Helpers ==========

    // Public for integration tests
    pub fn extract_entities_from_text(&self, text: &str) -> Vec<String> {
        use std::collections::HashSet;

        info!("extract_entities_from_text: Processing text: '{}'", text);

        // Try NER-based extraction first (if embeddings feature is enabled and NER is already loaded)
        #[cfg(feature = "embeddings")]
        {
            // Fast path: Use NER if already loaded (no async needed)
            if let Some(engine) = GLOBAL_NER_ENGINE.get() {
                match engine.extract_entities(text) {
                    Ok(recognized_entities) => {
                        info!("NER extracted {} entities", recognized_entities.len());
                        // Convert RecognizedEntity to Vec<String>
                        let entity_names: Vec<String> =
                            recognized_entities.into_iter().map(|e| e.text).collect();
                        return entity_names;
                    }
                    Err(e) => {
                        info!(
                            "NER extraction failed: {}. Falling back to pattern matching",
                            e
                        );
                    }
                }
            } else {
                debug!("NER engine not loaded yet, using pattern matching");
                // Note: First call will use pattern matching. NER will be loaded in background
                // for subsequent calls. To pre-load, call get_ner_engine() during daemon startup.
            }
        }

        // Fallback: Use pattern-based extraction
        let mut entities = HashSet::new();
        let text_lower = text.to_lowercase();

        // Check for non-ASCII text (e.g., Cyrillic, Chinese, etc.)
        let is_ascii = text.is_ascii();
        if !is_ascii {
            info!("Non-ASCII text detected, using intelligent multilingual entity extraction");

            // Extract all words with frequency counting
            let words: Vec<&str> = text.split_whitespace().collect();
            let mut term_freq: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();

            for word in &words {
                // Clean punctuation from word boundaries
                let cleaned =
                    word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');
                if !cleaned.is_empty() {
                    *term_freq.entry(cleaned.to_lowercase()).or_insert(0) += 1;
                }
            }

            // Score and filter terms
            for (term, count) in term_freq {
                let mut score = 0.0;

                // Filter stop words first (Bulgarian and English)
                if self.is_bulgarian_stop_word(&term) || self.is_common_word(&term) {
                    continue;
                }

                // Length scoring (prefer longer, more specific terms)
                if term.len() >= 8 {
                    score += 3.5; // Very long technical terms (ContentVectorizer, SemanticQueryEngine)
                } else if term.len() >= 5 {
                    score += 2.5; // Long technical terms (dashmap, atomic)
                } else if term.len() >= 3 {
                    score += 1.0; // Short but potentially meaningful (cpu, api)
                } else {
                    continue; // Skip very short terms
                }

                // Frequency bonus (repeated terms are important)
                score += (count as f64 * 0.8).min(2.5);

                // Technical indicators bonus (STRONG signals)
                if term.contains('_') || term.contains('-') {
                    score += 3.0; // snake_case or hyphenated terms → definitely technical
                }

                // Compound technical patterns (angle brackets, generics)
                if term.contains('<') || term.contains('>') {
                    score += 3.5; // RwLock<HashMap>, Vec<String>
                }

                // CamelCase detection (works for any script)
                let has_mixed_case = term.chars().any(|c| c.is_uppercase())
                    && term.chars().any(|c| c.is_lowercase());
                if has_mixed_case {
                    score += 2.5; // CamelCase → likely technical
                }

                // Numbers in term (version numbers, technical IDs)
                if term.chars().any(|c| c.is_numeric()) {
                    score += 2.0; // AtomicU64, http2
                }

                // All uppercase (acronyms, constants)
                if term.len() >= 2 && term.chars().all(|c| c.is_uppercase() || !c.is_alphabetic()) {
                    score += 3.0; // HNSW, CPU, API
                }

                // Known technical suffixes (language-agnostic)
                if term.ends_with("engine")
                    || term.ends_with("config")
                    || term.ends_with("manager")
                    || term.ends_with("handler")
                    || term.ends_with("vectorizer")
                    || term.ends_with("controller")
                {
                    score += 2.5;
                }

                // Accept if score >= 3.0 (balanced threshold for quality)
                if score >= 3.0 && term.len() <= 35 {
                    entities.insert(term);
                }
            }

            // Also extract quoted terms and code patterns (language-agnostic)
            for word in &words {
                let word_str = *word;

                // Backticks, quotes (code/technical terms)
                if (word_str.starts_with('`') && word_str.ends_with('`'))
                    || (word_str.starts_with('"') && word_str.ends_with('"'))
                    || (word_str.starts_with('\'') && word_str.ends_with('\''))
                {
                    let term = word_str
                        .trim_matches(|c: char| c == '`' || c == '"' || c == '\'')
                        .to_string();
                    if term.len() >= 3 && term.len() <= 25 {
                        entities.insert(term);
                    }
                }

                // File extensions
                if word_str.contains('.')
                    && (word_str.ends_with(".rs")
                        || word_str.ends_with(".sql")
                        || word_str.ends_with(".json")
                        || word_str.ends_with(".toml")
                        || word_str.ends_with(".md"))
                {
                    entities.insert(word_str.to_string());
                }
            }

            let final_result = entities.into_iter().take(20).collect::<Vec<_>>();
            info!(
                "Intelligent multilingual extracted entities: {:?}",
                final_result
            );
            return final_result;
        }

        // Extract entities using multiple intelligent patterns (only for ASCII text)
        self.extract_proper_nouns(text, &mut entities);
        self.extract_technical_terms(&text_lower, &mut entities);
        self.extract_quoted_terms(text, &mut entities);
        self.extract_compound_terms(text, &mut entities);
        self.extract_domain_specific_terms(&text_lower, &mut entities);

        info!("Found {} entities from pattern matching", entities.len());

        // Extract file paths (simple pattern)
        let words: Vec<&str> = text.split_whitespace().collect();
        for word in words {
            if word.contains('.')
                && (word.ends_with(".rs")
                    || word.ends_with(".sql")
                    || word.ends_with(".json")
                    || word.ends_with(".toml"))
                && word.len() > 5
            {
                entities.insert(word.to_string());
            }
        }

        // Clean, normalize, and filter entities (safe with Option handling)
        let mut cleaned_entities = HashSet::new();
        for entity in entities.iter() {
            // Validate first
            if !self.is_valid_entity(entity) {
                continue;
            }

            // Normalize (returns None on error)
            if let Some(normalized) = self.normalize_entity(entity) {
                // Re-validate after normalization
                if self.is_valid_entity(&normalized) && !normalized.is_empty() {
                    cleaned_entities.insert(normalized);
                }
            }
        }

        info!(
            "Entities after cleaning: {} -> {} (filtered {} invalid)",
            entities.len(),
            cleaned_entities.len(),
            entities.len().saturating_sub(cleaned_entities.len())
        );

        // Score and filter entities by relevance
        let scored_entities = self.score_entities(&cleaned_entities, text);
        let final_result = scored_entities.into_iter().take(20).collect::<Vec<_>>();
        info!("Final extracted entities: {:?}", final_result);
        final_result
    }

    /// Extract proper nouns and capitalized terms.
    ///
    /// CamelCase terms (e.g. FileWatcher, PcxClient, SurrealDB) are kept in their
    /// original casing because `is_valid_entity` requires mixed-case or underscores
    /// for short terms.  Plain Title-case words that appear at sentence start are
    /// filtered out to avoid noise like "The", "Returns", "Based", etc.
    fn extract_proper_nouns(&self, text: &str, entities: &mut std::collections::HashSet<String>) {
        // Match CamelCase identifiers: must contain at least one lowercase letter after the
        // initial uppercase, and at least one more uppercase somewhere (true CamelCase).
        // This catches FileWatcher, PcxClient, SurrealDB, ConversationLoop etc.
        let camel_case_regex = regex::Regex::new(r"\b[A-Z][a-z]+(?:[A-Z][a-zA-Z0-9]*)+\b")
            .expect("Built-in regex pattern should always compile");

        // Also match ALL_CAPS acronyms like HNSW, API, RPC, PCX (min 2 chars)
        let acronym_regex = regex::Regex::new(r"\b[A-Z]{2,8}\b")
            .expect("Built-in regex pattern should always compile");

        // Plain Title-case words (only one capital at the start) that are NOT
        // CamelCase.  We still collect these but apply stricter filtering below.
        let title_case_regex = regex::Regex::new(r"\b[A-Z][a-z]{3,}\b")
            .expect("Built-in regex pattern should always compile");

        // Sentence-start / common English words to exclude from plain title-case
        let sentence_starters = [
            "The", "This", "That", "These", "Those", "When", "Where", "What", "Why", "How", "Who",
            "Which", "Will", "Would", "Could", "Should", "Must", "Can", "May", "Might", "And",
            "But", "Or", "Not", "So", "Yet", "For", "Nor", "Because", "Although", "Since",
            "While", "Until", "Unless", "Before", "After", "During", "Through", "With", "From",
            "Into", "Upon", "Using", "Based", "Returns", "Creates", "Generates", "Implements",
            "Provides", "Contains", "Supports", "Handles", "Defines", "Manages", "Performs",
            "Stores", "Loads", "Reads", "Writes", "Calls", "Checks", "Finds", "Gets", "Sets",
            "Takes", "Makes", "Runs", "Builds", "Tests", "Adds", "Removes", "Updates", "Changes",
        ];

        // CamelCase — high confidence, keep as-is
        for cap in camel_case_regex.find_iter(text) {
            let original = cap.as_str();
            if original.len() >= 4 && original.len() <= 40 {
                entities.insert(original.to_string());
            }
        }

        // Acronyms — keep uppercase
        for cap in acronym_regex.find_iter(text) {
            let original = cap.as_str();
            // Skip very common English abbreviations
            if !matches!(original, "I" | "A" | "OR" | "AND" | "BUT" | "FOR" | "NOT" | "TO" | "IN" | "IS" | "IT" | "BE" | "DO" | "GO") {
                entities.insert(original.to_string());
            }
        }

        // Plain Title-case — lower confidence, only keep if not a sentence-starter
        // and the lowercased form passes is_valid_entity (length >= 7 or has indicators)
        for cap in title_case_regex.find_iter(text) {
            let original = cap.as_str();
            if sentence_starters.contains(&original) {
                continue;
            }
            // Require length >= 7 for plain title-case words (eliminates most noise)
            if original.len() >= 7 && original.len() <= 25 {
                entities.insert(original.to_string());
            }
        }
    }

    /// Extract technical and domain-specific terms using patterns
    fn extract_technical_terms(
        &self,
        text_lower: &str,
        entities: &mut std::collections::HashSet<String>,
    ) {
        // Skip regex patterns for non-ASCII text to prevent infinite loops
        if !text_lower.is_ascii() {
            return;
        }

        // Programming language patterns
        let prog_patterns = [
            r"\b\w+script\b",
            r"\b\w+lang\b",
            r"\b\w+\+\+\b",
            r"\b\w*sql\b",
            r"\b\w*db\b",
            r"\b\w*api\b",
            r"\b\w*json\b",
            r"\b\w*xml\b",
        ];

        // Extract lowercase technical terms with frequency-based scoring.
        // Regex requires at least one underscore OR digit within the token so that
        // plain dictionary words like "based", "returns", "generates" are never
        // entered into the candidate pool in the first place.
        // For longer words (≥8 chars) we allow plain lowercase because something
        // like "dashmap", "surrealdb", "petgraph" is very unlikely to be noise.
        let snake_or_versioned_regex =
            regex::Regex::new(r"\b[a-z][a-z0-9]*(?:[_][a-z0-9]+)+[a-z0-9]*\b|(?:\b[a-z][a-z]*[0-9][a-z0-9]*\b)")
                .expect("Built-in regex pattern should always compile");
        let long_lowercase_regex = regex::Regex::new(r"\b[a-z]{8,20}\b")
            .expect("Built-in regex pattern should always compile");

        // Step 1: Collect all candidates with frequency count
        let mut term_freq: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        // snake_case / versioned terms (always strong candidates)
        for cap in snake_or_versioned_regex.find_iter(text_lower) {
            let term = cap.as_str().to_string();
            *term_freq.entry(term).or_insert(0) += 1;
        }

        // Long plain-lowercase words (≥8 chars) — likely proper nouns / library names
        for cap in long_lowercase_regex.find_iter(text_lower) {
            let term = cap.as_str().to_string();
            term_freq.entry(term).or_insert(0);
            // Don't double-count — just ensure entry exists
        }

        // Step 2: Score each term based on multiple factors
        for (term, count) in term_freq {
            // Skip known stop/keyword words early
            if ENGLISH_STOP_WORDS.contains(term.as_str())
                || PROGRAMMING_KEYWORDS.contains(term.as_str())
                || COMMON_ENGLISH_WORDS.contains(term.as_str())
            {
                continue;
            }

            let mut score = 0.0;

            // snake_case is a very strong technical indicator
            if term.contains('_') {
                score += 4.0; // file_watcher, pcx_client → definitely technical identifiers
            }

            // Digit in term — versioned or numeric technical identifiers
            if term.chars().any(|c| c.is_ascii_digit()) {
                score += 3.0; // http2, blake3, v8 → technical
            }

            // Length bonus (longer plain-lowercase terms more likely to be library names)
            if term.len() >= 10 {
                score += 3.0; // surrealdb, dashmap, petgraph-level names
            } else if term.len() >= 8 {
                score += 2.0; // lockfree, tokioruntime
            } else if term.len() >= 5 && (term.contains('_') || term.chars().any(|c| c.is_ascii_digit())) {
                score += 1.0; // Only give length bonus to shorter terms if they have indicators
            }

            // Frequency bonus — cap low to prevent common words from gaming the score
            score += (count as f64 * 0.4).min(1.5);

            // Accept only at a meaningfully high threshold — 3.0 requires at least
            // one strong technical signal (snake_case, digit, or length ≥ 10)
            if score >= 3.0 {
                entities.insert(term);
            }
        }

        // Process patterns
        let process_patterns = [
            r"\b\w*-free\b",
            r"\b\w*safe\b",
            r"\b\w*async\b",
            r"\b\w*sync\b",
            r"\b\w*lock\b",
            r"\b\w*thread\b",
            r"\b\w*process\b",
            r"\b\w*cache\b",
        ];

        // Architecture patterns
        let arch_patterns = [
            r"\bmicro\w*\b",
            r"\bmulti-\w+\b",
            r"\bdistributed-\w+\b",
            r"\bcloud-\w+\b",
            r"\bserver\w*\b",
            r"\bclient\w*\b",
            r"\bprotocol\w*\b",
        ];

        let all_patterns = [
            prog_patterns.as_slice(),
            process_patterns.as_slice(),
            arch_patterns.as_slice(),
        ]
        .concat();

        // Limit processing to prevent timeout
        const MAX_PATTERNS: usize = 10;

        for (processed_patterns, pattern) in all_patterns.into_iter().enumerate() {
            if processed_patterns >= MAX_PATTERNS {
                break;
            }
            if let Ok(regex) = regex::Regex::new(pattern) {
                let mut matches = 0;
                for mat in regex.find_iter(text_lower) {
                    let term = mat.as_str();
                    if term.len() >= 3 && term.len() <= 20 {
                        entities.insert(term.to_string());
                        matches += 1;
                        if matches > 20 {
                            // Limit matches per pattern
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Extract terms in quotes or backticks (code identifiers, library names).
    /// Casing is preserved to retain CamelCase/snake_case signal.
    fn extract_quoted_terms(&self, text: &str, entities: &mut std::collections::HashSet<String>) {
        let quoted_patterns = [
            r#"["']([^"']{2,40})["']"#, // Single/double quotes
            r"`([^`]{2,40})`",           // Backticks — highest confidence for code identifiers
        ];

        for pattern in quoted_patterns {
            if let Ok(regex) = regex::Regex::new(pattern) {
                for cap in regex.captures_iter(text) {
                    if let Some(quoted_term) = cap.get(1) {
                        let term = quoted_term.as_str().to_string(); // preserve casing
                        if term.len() >= 2
                            && term.len() <= 40
                            && !term.chars().all(|c| c.is_numeric())
                        {
                            entities.insert(term);
                        }
                    }
                }
            }
        }
    }

    /// Extract compound terms with hyphens, underscores, dots, or CamelCase.
    /// Original casing is preserved so that downstream scoring and `is_valid_entity`
    /// can reward CamelCase and ALL_CAPS terms appropriately.
    fn extract_compound_terms(&self, text: &str, entities: &mut std::collections::HashSet<String>) {
        let compound_patterns = [
            r"\b[a-zA-Z]+-[a-zA-Z]+(?:-[a-zA-Z]+)*\b",   // Hyphenated: lock-free, multi-thread
            r"\b[a-zA-Z]+_[a-zA-Z]+(?:_[a-zA-Z]+)*\b",   // snake_case: file_watcher, pcx_client
            r"\b[A-Z][a-z]*[A-Z][a-zA-Z0-9]*\b",          // CamelCase: FileWatcher, PcxClient
        ];

        for pattern in compound_patterns {
            if let Ok(regex) = regex::Regex::new(pattern) {
                for cap in regex.find_iter(text) {
                    let term = cap.as_str().to_string(); // preserve original casing
                    if term.len() >= 4 && term.len() <= 40 {
                        entities.insert(term);
                    }
                }
            }
        }
    }

    /// Extract domain-specific terms based on context clues
    fn extract_domain_specific_terms(
        &self,
        text_lower: &str,
        entities: &mut std::collections::HashSet<String>,
    ) {
        // Look for terms near context indicators
        let context_indicators = [
            ("using", 2),
            ("with", 2),
            ("via", 2),
            ("through", 2),
            ("technology", 3),
            ("system", 3),
            ("framework", 3),
            ("library", 3),
            ("protocol", 3),
            ("method", 3),
            ("approach", 3),
            ("solution", 3),
            ("tool", 2),
            ("service", 3),
            ("platform", 3),
            ("engine", 3),
        ];

        let words: Vec<&str> = text_lower.split_whitespace().collect();

        for (indicator, range) in context_indicators {
            if let Some(pos) = words.iter().position(|&w| w == indicator) {
                // Look for entities around the indicator
                let start = pos.saturating_sub(range);
                let end = (pos + range + 1).min(words.len());

                for &word in &words[start..end] {
                    if word != indicator && word.len() >= 3 && word.len() <= 20 {
                        // Filter out common English words
                        if !self.is_common_word(word) {
                            entities.insert(word.to_string());
                        }
                    }
                }
            }
        }
    }

    /// Check if a word is a common English word that shouldn't be an entity (O(1) HashSet lookup)
    fn is_common_word(&self, word: &str) -> bool {
        COMMON_ENGLISH_WORDS.contains(word)
    }

    /// Check if a word is a common Bulgarian stop word (O(1) HashSet lookup)
    fn is_bulgarian_stop_word(&self, word: &str) -> bool {
        BULGARIAN_STOP_WORDS.contains(word)
    }

    /// Score entities based on frequency, length, and context relevance
    fn score_entities(
        &self,
        entities: &std::collections::HashSet<String>,
        original_text: &str,
    ) -> Vec<String> {
        let mut scored: Vec<(String, f64)> = entities
            .iter()
            .map(|entity| {
                let score = self.calculate_entity_score(entity, original_text);
                (entity.clone(), score)
            })
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        scored.into_iter().map(|(entity, _)| entity).collect()
    }

    /// Calculate relevance score for an entity.
    ///
    /// CamelCase, snake_case, ALL_CAPS, and versioned identifiers score highest.
    /// Plain short lowercase words score very low and are relied upon to be filtered
    /// by `is_valid_entity` before reaching this function.
    fn calculate_entity_score(&self, entity: &str, text: &str) -> f64 {
        let mut score = 0.0;
        let lower = entity.to_lowercase();

        // Strong structural bonuses — these are the primary signals of a real identifier
        let has_upper = entity.chars().any(|c| c.is_uppercase());
        let has_underscore = entity.contains('_');
        let has_digit = entity.chars().any(|c| c.is_ascii_digit());
        let is_camel = has_upper && entity.chars().any(|c| c.is_lowercase()); // mixed case
        let is_all_caps = has_upper && entity.chars().all(|c| c.is_uppercase() || !c.is_alphabetic());
        let is_snake = has_underscore;

        if is_camel {
            score += 3.0; // FileWatcher, PcxClient, ConversationLoop
        }
        if is_snake {
            score += 2.5; // file_watcher, pcx_client, context_update
        }
        if is_all_caps && entity.len() >= 2 {
            score += 2.5; // HNSW, API, PCX, RPC
        }
        if has_digit {
            score += 1.5; // blake3, http2, v8
        }

        // Base score from length (still useful for tie-breaking)
        let length_score = match entity.len() {
            1..=2 => 0.0,
            3..=4 => 0.1,
            5..=7 => 0.4,
            8..=12 => 0.8,
            13..=20 => 0.6,
            _ => 0.3,
        };
        score += length_score;

        // Frequency score (more mentions = more important) - case insensitive
        let freq_count = text.to_lowercase().matches(&lower).count() as f64;
        score += (freq_count * 0.4).min(2.0);

        // Technical term indicators in the identifier itself
        let tech_suffixes = ["api", "db", "sql", "json", "xml", "http", "tcp", "udp", "rpc", "sdk"];
        if tech_suffixes.iter().any(|&suffix| lower.ends_with(suffix)) {
            score += 1.0;
        }

        // Architecture/component suffixes strongly indicate meaningful entities
        let component_suffixes = [
            "watcher", "client", "server", "engine", "manager", "handler", "service",
            "registry", "storage", "context", "session", "system", "processor", "extractor",
            "controller", "scheduler", "dispatcher", "executor", "listener", "observer",
            "provider", "factory", "builder", "parser", "formatter", "writer", "reader",
            "loop", "runtime", "pool", "cache", "queue", "store",
        ];
        if component_suffixes.iter().any(|&s| lower.ends_with(s)) {
            score += 2.0;
        }

        // Penalize very common words that slipped through (last-resort safety net)
        if ENGLISH_STOP_WORDS.contains(lower.as_str())
            || PROGRAMMING_KEYWORDS.contains(lower.as_str())
            || COMMON_ENGLISH_WORDS.contains(lower.as_str())
        {
            score *= 0.05;
        }

        score
    }

    /// Static version of calculate_entity_score for use with CoW patterns.
    /// Mirrors the instance method — both must stay in sync.
    fn calculate_entity_score_static(entity: &str, text: &str) -> f64 {
        let mut score = 0.0;
        let lower = entity.to_lowercase();

        let has_upper = entity.chars().any(|c| c.is_uppercase());
        let has_underscore = entity.contains('_');
        let has_digit = entity.chars().any(|c| c.is_ascii_digit());
        let is_camel = has_upper && entity.chars().any(|c| c.is_lowercase());
        let is_all_caps =
            has_upper && entity.chars().all(|c| c.is_uppercase() || !c.is_alphabetic());
        let is_snake = has_underscore;

        if is_camel {
            score += 3.0;
        }
        if is_snake {
            score += 2.5;
        }
        if is_all_caps && entity.len() >= 2 {
            score += 2.5;
        }
        if has_digit {
            score += 1.5;
        }

        let length_score = match entity.len() {
            1..=2 => 0.0,
            3..=4 => 0.1,
            5..=7 => 0.4,
            8..=12 => 0.8,
            13..=20 => 0.6,
            _ => 0.3,
        };
        score += length_score;

        let freq_count = text.to_lowercase().matches(&lower).count() as f64;
        score += (freq_count * 0.4).min(2.0);

        let tech_suffixes = ["api", "db", "sql", "json", "xml", "http", "tcp", "udp", "rpc", "sdk"];
        if tech_suffixes.iter().any(|&suffix| lower.ends_with(suffix)) {
            score += 1.0;
        }

        let component_suffixes = [
            "watcher", "client", "server", "engine", "manager", "handler", "service",
            "registry", "storage", "context", "session", "system", "processor", "extractor",
            "controller", "scheduler", "dispatcher", "executor", "listener", "observer",
            "provider", "factory", "builder", "parser", "formatter", "writer", "reader",
            "loop", "runtime", "pool", "cache", "queue", "store",
        ];
        if component_suffixes.iter().any(|&s| lower.ends_with(s)) {
            score += 2.0;
        }

        if ENGLISH_STOP_WORDS.contains(lower.as_str())
            || PROGRAMMING_KEYWORDS.contains(lower.as_str())
            || COMMON_ENGLISH_WORDS.contains(lower.as_str())
        {
            score *= 0.05;
        }

        score
    }

    /// Static version of extract_explicit_concepts for use with CoW patterns
    fn extract_explicit_concepts_static(
        text: &str,
        concepts: &mut std::collections::HashSet<String>,
    ) {
        let text_lower = text.to_lowercase();

        // Look for concept indicators
        let concept_indicators = [
            "concept",
            "principle",
            "pattern",
            "approach",
            "methodology",
            "framework",
            "architecture",
            "design",
            "strategy",
            "technique",
            "algorithm",
            "model",
            "abstraction",
            "paradigm",
        ];

        // Extract terms following concept indicators
        for indicator in &concept_indicators {
            let pattern = format!(
                r"{}\s+(is|are|was|were|involves|uses|implements|provides)\s+([a-zA-Z][a-zA-Z\s]{{2,30}})",
                indicator
            );
            if let Ok(regex) = regex::Regex::new(&pattern) {
                for cap in regex.captures_iter(text) {
                    if let Some(concept_match) = cap.get(2) {
                        let concept = concept_match.as_str().trim();
                        if concept.len() >= 3 && concept.len() <= 25 {
                            concepts.insert(concept.to_lowercase());
                        }
                    }
                }
            }
        }

        // Extract quoted definitions
        let quoted_patterns = [
            r#"concept\s+of\s+[""']([^""']{2,25})[""']"#,
            r#"definition\s+[""']([^""']{2,25})[""']"#,
            r#"principle\s+[""']([^""']{2,25})[""']"#,
        ];

        for pattern in quoted_patterns {
            if let Ok(regex) = regex::Regex::new(pattern) {
                for cap in regex.captures_iter(&text_lower) {
                    if let Some(concept_match) = cap.get(1) {
                        let concept = concept_match.as_str().trim();
                        if concept.len() >= 3 && concept.len() <= 25 {
                            concepts.insert(concept.to_string());
                        }
                    }
                }
            }
        }

        // Extract CamelCase terms
        if let Ok(camelcase_regex) = regex::Regex::new(r"\b[A-Z][a-zA-Z]*[A-Z][a-zA-Z]*\b") {
            for cap in camelcase_regex.find_iter(text) {
                let camelcase_term = cap.as_str();
                if camelcase_term.len() >= 4 && camelcase_term.len() <= 20 {
                    let common_camelcase =
                        ["This", "That", "These", "Those", "When", "Where", "What"];
                    if !common_camelcase.contains(&camelcase_term) {
                        concepts.insert(camelcase_term.to_lowercase());
                    }
                }
            }
        }
    }

    /// Extract relationships between entities based on text patterns
    fn extract_relationships_from_text(
        &self,
        text: &str,
        entities: &[String],
    ) -> Vec<crate::core::context_update::EntityRelationship> {
        use crate::core::context_update::{EntityRelationship, RelationType};

        let mut relationships = Vec::new();
        // Analyze the full text as one block for short updates, or split by newlines
        // Splitting by '.' can be risky with abbreviations or code snippets.
        // For Context Updates, the description is usually one coherent thought.
        let lower_text = text.to_lowercase();

        // Map of keywords to relation types
        let patterns = [
            (vec![
                "depends on", "depends", "relies on", "needs", "requires", "based on",
                "зависи от", "изисква", "базира се на", "стъпва на", "се нуждае от"
            ], RelationType::DependsOn),
            (vec![
                "implements", "extends", "inherits",
                "имплементира", "реализира", "наследява", "разширява", "внедрява"
            ], RelationType::Implements),
            (vec![
                "calls", "uses", "invokes", "connects to", "connects", "communicates with",
                "използва", "свързва се с", "комуникира с", "работи с", "вика", "се обръща към", "има връзка с"
            ], RelationType::RelatedTo),
            (vec![
                "causes", "leads to", "triggers", "results in", "generates", "starts",
                "води до", "причинява", "създава", "генерира", "предизвиква", "отключва", "стартира"
            ], RelationType::LeadsTo),
            (vec![
                "solves", "fixes", "resolves", "handles", "removes",
                "решава", "поправя", "оправя", "фиксва", "затваря", "устранява", "премахва"
            ], RelationType::Solves),
        ];

        // Find all entities present in the text
        let present_entities: Vec<&String> = entities
            .iter()
            .filter(|e| lower_text.contains(&e.to_lowercase()))
            .collect();

        if present_entities.len() >= 2 {
            // Check for relationships between pairs of entities
            for (i, e1) in present_entities.iter().enumerate() {
                for e2 in present_entities.iter().skip(i + 1) {
                    if e1 == e2 {
                        continue;
                    }

                    let e1_lower = e1.to_lowercase();
                    let e2_lower = e2.to_lowercase();

                    // Check which relationship pattern appears
                    for (keywords, rel_type) in &patterns {
                        if keywords.iter().any(|k| lower_text.contains(k)) {
                            // Find positions
                            let pos1 = lower_text.find(&e1_lower).unwrap_or(0);
                            let pos2 = lower_text.find(&e2_lower).unwrap_or(0);

                            // Determine direction
                            let (from, to) = if pos1 < pos2 { (e1, e2) } else { (e2, e1) };

                            // Validate that the keyword is actually BETWEEN the entities
                            // This reduces false positives significantly
                            let start = pos1.min(pos2) + from.len();
                            let end = pos1.max(pos2);

                            let segment = if start < end {
                                &lower_text[start..end]
                            } else {
                                ""
                            };

                            if keywords.iter().any(|k| segment.contains(k)) {
                                relationships.push(EntityRelationship {
                                    from_entity: from.to_string(),
                                    to_entity: to.to_string(),
                                    relation_type: rel_type.clone(),
                                    context: text.trim().to_string(),
                                });
                                // Found a relationship for this pair, move to next pair
                                break;
                            }
                        }
                    }
                }
            }
        }

        relationships
    }

    /// Static version of extract_entities_from_text for use with CoW patterns
    /// Simplified version that uses pattern-based extraction only
    fn extract_entities_from_text_static(text: &str) -> Vec<String> {
        use std::collections::HashSet;

        let mut entities = HashSet::new();
        let text_lower = text.to_lowercase();

        // Extract capitalized terms (proper nouns)
        if let Ok(capitalized_regex) = regex::Regex::new(r"\b[A-Z][a-zA-Z-]{2,}\b") {
            let common_words = [
                "The", "This", "That", "These", "Those", "When", "Where", "What", "Why", "How",
                "Who", "Which", "Will", "Would", "Could", "Should", "Must", "Can", "May", "Might",
                "And", "But", "Or", "Not", "So", "Yet", "For", "Nor", "Because", "Although",
                "Since", "While",
            ];
            for cap in capitalized_regex.find_iter(text) {
                let original = cap.as_str();
                let term = original.to_lowercase();
                if term.len() >= 3 && term.len() <= 20 && !common_words.contains(&original) {
                    entities.insert(term);
                }
            }
        }

        // Extract compound terms (hyphenated, underscored, CamelCase)
        if let Ok(compound_regex) = regex::Regex::new(r"\b[a-zA-Z]+-[a-zA-Z]+(?:-[a-zA-Z]+)*\b") {
            for cap in compound_regex.find_iter(text) {
                let term = cap.as_str().to_lowercase();
                if term.len() >= 4 && term.len() <= 25 {
                    entities.insert(term);
                }
            }
        }

        if let Ok(camel_regex) = regex::Regex::new(r"\b[A-Z][a-z]*[A-Z][a-zA-Z]*\b") {
            for cap in camel_regex.find_iter(text) {
                let term = cap.as_str().to_lowercase();
                if term.len() >= 4 && term.len() <= 25 {
                    entities.insert(term);
                }
            }
        }

        // Score and filter
        let mut scored: Vec<(String, f64)> = entities
            .iter()
            .map(|entity| {
                let score = Self::calculate_entity_score_static(entity, &text_lower);
                (entity.clone(), score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(20).map(|(e, _)| e).collect()
    }

    /// Rank entities by importance and truncate to specified limit
    /// Uses frequency scoring to keep the most relevant entities
    fn rank_and_truncate_entities(
        &self,
        entities: Vec<String>,
        text: &str,
        max_count: usize,
        enable_ranking: bool,
    ) -> (Vec<String>, usize) {
        let original_count = entities.len();

        // If ranking is disabled or we're under the limit, just truncate
        if !enable_ranking || original_count <= max_count {
            let truncated = entities.into_iter().take(max_count).collect();
            return (truncated, 0);
        }

        // Score all entities
        let mut scored: Vec<(String, f64)> = entities
            .into_iter()
            .map(|entity| {
                let score = self.calculate_entity_score(&entity, text);
                (entity, score)
            })
            .collect();

        // Sort by score (highest first)
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Take top N
        let ranked_entities: Vec<String> = scored
            .into_iter()
            .take(max_count)
            .map(|(entity, _score)| entity)
            .collect();

        let truncated_count = original_count.saturating_sub(max_count);

        if truncated_count > 0 {
            info!(
                "Smart ranking: kept top {} of {} entities (truncated {})",
                ranked_entities.len(),
                original_count,
                truncated_count
            );
        }

        (ranked_entities, truncated_count)
    }

    // NOTE: Removed auto_generate_relationships and infer_relationship_type functions.
    // They were creating "Co-mentioned" noise relationships (99%+ useless).
    // Only explicit relationships from text pattern analysis are now used.

    async fn add_code_reference(&mut self, code_ref: &CodeReference) -> anyhow::Result<()> {
        // Use Arc::make_mut for CoW semantics on code_references
        let code_references = Arc::make_mut(&mut self.code_references);
        let code_refs = code_references
            .entry(code_ref.file_path.clone())
            .or_default();
        code_refs.push(code_ref.clone());
        Ok(())
    }

    fn record_change(&mut self, update: &ContextUpdate) -> anyhow::Result<()> {
        // Use Arc::make_mut for CoW semantics on change_history
        Arc::make_mut(&mut self.change_history).push(ChangeRecord {
            id: Uuid::new_v4(),
            timestamp: update.timestamp,
            change_type: format!("{:?}", update.update_type),
            description: update.content.description.clone(),
            related_update_id: Some(update.id),
        });
        Ok(())
    }

    fn maintain_context(&mut self) -> anyhow::Result<()> {
        // Note: HotContext now manages its own capacity automatically
        // No need to manually move to warm - capacity is enforced on push

        // Create summary if needed
        if self.should_create_summary() {
            self.create_periodic_summary()?;
        }

        Ok(())
    }

    fn should_create_summary(&self) -> bool {
        let threshold = self.metadata.user_preferences.auto_summary_threshold;
        let len = self.incremental_updates.len();
        // Guard against threshold=0 (is_multiple_of(0) returns true for any number)
        threshold > 0 && len > 0 && len % threshold == 0
    }

    fn create_periodic_summary(&mut self) -> anyhow::Result<()> {
        // Create a summary from current state
        // Note: Arc clone is cheap (just ref count increment), and we need immutable access
        let summary = StructuredSummary {
            summary_id: Uuid::new_v4(),
            created_at: Utc::now(),
            context_snapshot: (*self.current_state).clone(),
            referenced_updates: self.incremental_updates.iter().map(|u| u.id).collect(),
            summary_quality: 1.0, // Placeholder - would calculate actual quality
        };

        // Use Arc::make_mut for CoW semantics on cold_context
        Arc::make_mut(&mut self.cold_context).push(summary);
        Ok(())
    }

    /// Update the session name
    pub fn set_name(&mut self, name: Option<String>) {
        let new_metadata = Arc::new(SessionMetadata::new(
            self.metadata.id,
            name,
            self.metadata.description.clone(),
            self.metadata.user_preferences.clone(),
        ));
        self.metadata = new_metadata;
        self.last_updated = Utc::now();
    }

    /// Update the session description
    pub fn set_description(&mut self, description: Option<String>) {
        let new_metadata = Arc::new(SessionMetadata::new(
            self.metadata.id,
            self.metadata.name.clone(),
            description,
            self.metadata.user_preferences.clone(),
        ));
        self.metadata = new_metadata;
        self.last_updated = Utc::now();
    }

    /// Update both name and description (preserves existing values if None provided)
    pub fn update_metadata(&mut self, name: Option<String>, description: Option<String>) {
        let final_name = if name.is_some() {
            name
        } else {
            self.metadata.name.clone()
        };
        let final_description = if description.is_some() {
            description
        } else {
            self.metadata.description.clone()
        };

        let new_metadata = Arc::new(SessionMetadata::new(
            self.metadata.id,
            final_name,
            final_description,
            self.metadata.user_preferences.clone(),
        ));
        self.metadata = new_metadata;
        self.last_updated = Utc::now();
    }

    /// Get the current name and description
    pub fn get_metadata(&self) -> (Option<String>, Option<String>) {
        (
            self.metadata.name.clone(),
            self.metadata.description.clone(),
        )
    }
}
