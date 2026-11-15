// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Retrieval is pinned: a fixed corpus (records, entities and captured
//! turns in three sessions, two of them in a workspace) searched with
//! several queries, scopes, corpora, filters and modes returns exactly the
//! results in `tests/fixtures/retrieval-pin.json` (ids, scores, order), for
//! the hash and the model2vec engines on both backends. The fixture was
//! written by this test on the tree before the `SearchIndex` port
//! (`PIN_UPDATE=1` rewrites it).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::core::message::{MessageRole, NewMessage};
use post_cortex_core::services::{AppendMessagesRequest, UpdateContextRequest};
use post_cortex_core::workspace::SessionRole;
use post_cortex_ports::{Corpus, Reranker};
use uuid::Uuid;

use crate::capabilities::SearchScope;
use crate::capabilities::{CreateWorkspace, Memory};
use crate::content_vectorizer::{SearchMode, SearchOptions, SemanticSearchResult};
use crate::memory_system::backends_tests::{Backend, backends, embeddings_config, hash_engine};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/retrieval-pin.json"
);

/// Scores each document by the share of the query's words it contains.
struct OverlapReranker;

#[async_trait]
impl Reranker for OverlapReranker {
    async fn score(
        &self,
        query: &str,
        documents: &[Arc<str>],
    ) -> post_cortex_ports::error::Result<Vec<f32>> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        Ok(documents
            .iter()
            .map(|d| {
                let d = d.to_lowercase();
                words.iter().filter(|w| d.contains(w.as_str())).count() as f32
                    / words.len().max(1) as f32
            })
            .collect())
    }
}

/// (session label, title, description, entities, author).
type Record = (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    Option<&'static str>,
);

const RECORDS: &[Record] = &[
    (
        "a",
        "RocksDB compaction",
        "We tuned RocksDB level compaction to cut write amplification on the session column family.",
        &["RocksDB", "compaction"],
        Some("julius"),
    ),
    (
        "a",
        "Bloom filters",
        "Bloom filters on the update rows keep point reads of records under a millisecond.",
        &["RocksDB", "bloom filter"],
        None,
    ),
    (
        "a",
        "Vector arena",
        "The vector index keeps normalised embeddings in one contiguous arena and scans them exactly.",
        &["vector index", "arena"],
        Some("claude-code"),
    ),
    (
        "a",
        "BM25 terms",
        "Lexical search drops query words that appear in more than a fifth of the records.",
        &["BM25", "query terms"],
        None,
    ),
    (
        "a",
        "Rank fusion",
        "Vector and BM25 rankings are fused with reciprocal rank fusion before the reranker sees them.",
        &["rank fusion", "BM25"],
        Some("claude-code"),
    ),
    (
        "a",
        "Reranker pool",
        "The cross-encoder reranks the top three hundred fused candidates in local mode.",
        &["reranker", "rank fusion"],
        None,
    ),
    (
        "a",
        "Retrieval graph",
        "The retrieval graph links records through shared terms and entity relations of one session.",
        &["retrieval graph", "entity graph"],
        None,
    ),
    (
        "a",
        "Session persister",
        "Session writes are coalesced per session by the persister and written in order.",
        &["persister", "session"],
        Some("julius"),
    ),
    (
        "b",
        "Daemon deploy",
        "We deploy the pcx daemon with launchd on macOS and systemd on Linux servers.",
        &["daemon", "launchd"],
        Some("julius"),
    ),
    (
        "b",
        "Health endpoint",
        "The health endpoint reports the vector load, request counters and the pipeline backlog.",
        &["health", "daemon"],
        None,
    ),
    (
        "b",
        "SurrealDB server",
        "Production runs SurrealDB as a server; the daemon connects over WebSocket with credentials.",
        &["SurrealDB", "daemon"],
        Some("claude-code"),
    ),
    (
        "b",
        "Export archive",
        "pcx export writes a JSON archive with sessions, workspaces, cards, messages and vectors.",
        &["export", "archive"],
        None,
    ),
    (
        "b",
        "Import refusal",
        "Import refuses an archive whose vectors were embedded with another model before writing.",
        &["import", "archive"],
        Some("julius"),
    ),
    (
        "c",
        "Weather small talk",
        "We talked about the weather and the rain on the coast for a while.",
        &["weather", "coast"],
        None,
    ),
    (
        "c",
        "Compaction stalls",
        "Write stalls during compaction were fixed by raising the level zero slowdown trigger.",
        &["compaction", "RocksDB"],
        Some("julius"),
    ),
    (
        "c",
        "Model download",
        "The model2vec weights are downloaded from the Hugging Face hub on first use.",
        &["model2vec", "Hugging Face"],
        None,
    ),
];

/// (session label, author, text) of captured turns.
const TURNS: &[(&str, &str, &str)] = &[
    (
        "c",
        "julius",
        "Can you check why the archive import refused my export from the laptop?",
    ),
    (
        "c",
        "claude-code",
        "The archive was embedded with MiniLM and this instance runs model2vec, so import refused it.",
    ),
    (
        "c",
        "julius",
        "Then re-embed the export with the new model and import it again.",
    ),
    (
        "a",
        "julius",
        "Compaction still stalls on the big session, look at the write amplification.",
    ),
];

fn entity(name: &str) -> EntityData {
    EntityData {
        aliases: Vec::new(),
        name: name.to_string(),
        entity_type: EntityType::Concept,
        first_mentioned: Utc::now(),
        last_mentioned: Utc::now(),
        mention_count: 1,
        importance_score: 1.0,
        description: None,
    }
}

/// The corpus written into `memory`: session and record labels by id.
async fn corpus(memory: &Memory) -> (HashMap<String, String>, BTreeMap<&'static str, Uuid>) {
    let mut labels = HashMap::new();
    let mut sessions = BTreeMap::new();
    for name in ["a", "b", "c"] {
        let id = memory
            .sessions
            .create(Some(name.into()), None)
            .await
            .unwrap();
        labels.insert(id.to_string(), format!("s:{name}"));
        sessions.insert(name, id);
    }
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "pin".into(),
            description: String::new(),
        })
        .await
        .unwrap();
    for name in ["a", "b"] {
        memory
            .workspaces
            .add_session(ws.id, sessions[name], SessionRole::Primary)
            .await
            .unwrap();
    }
    for (session, title, description, entities, author) in RECORDS {
        let req = UpdateContextRequest {
            session_id: sessions[session],
            interaction_type: UpdateType::DecisionMade,
            content: UpdateContent {
                title: (*title).into(),
                description: (*description).into(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            entities: entities.iter().map(|e| entity(e)).collect(),
            relations: vec![EntityRelationship {
                from_entity: entities[0].into(),
                to_entity: entities[1].into(),
                relation_type: RelationType::RelatedTo,
                context: "pin".into(),
            }],
            code_reference: None,
            author: author.map(String::from),
            derived_from: vec![],
            meta: Default::default(),
        };
        let written = memory.records.write(req).await.unwrap();
        labels.insert(written.entry_id.to_string(), format!("r:{title}"));
    }
    for (i, (session, author, text)) in TURNS.iter().enumerate() {
        memory
            .messages
            .append(AppendMessagesRequest {
                session_id: sessions[session],
                messages: vec![NewMessage {
                    turn_id: format!("t{i}"),
                    role: if *author == "julius" {
                        MessageRole::User
                    } else {
                        MessageRole::Assistant
                    },
                    author: Some((*author).into()),
                    source_session: None,
                    timestamp: Utc::now(),
                    text: (*text).into(),
                    tools: vec![],
                    files: vec![],
                    parent: None,
                    blocks: Vec::new(),
                }],
            })
            .await
            .unwrap();
    }
    assert!(memory.records.wait_idle(Duration::from_secs(120)).await);
    (labels, sessions)
}

fn line(r: &SemanticSearchResult, labels: &HashMap<String, String>) -> String {
    let label = |id: &str| labels.get(id).cloned().unwrap_or_else(|| id.to_string());
    format!(
        "{} {} {:?} sim={:?} comb={:?} nb={:?} author={:?}",
        label(&r.source.id().to_string()),
        label(&r.content_id),
        r.content_type,
        r.similarity_score,
        r.combined_score,
        r.neighbour_of.as_deref().map(label),
        r.author,
    )
}

/// (case name, query, scope by session labels, options).
fn cases(
    sessions: &BTreeMap<&str, Uuid>,
    workspace: Uuid,
) -> Vec<(String, &'static str, SearchScope, SearchOptions)> {
    let fast = |limit: usize| SearchOptions {
        limit: Some(limit),
        mode: Some(SearchMode::Fast),
        ..Default::default()
    };
    let local = SearchOptions {
        limit: Some(8),
        mode: Some(SearchMode::Local),
        ..Default::default()
    };
    let a = sessions["a"];
    let c = sessions["c"];
    vec![
        (
            "global compaction".into(),
            "rocksdb compaction write stalls",
            SearchScope::Global,
            fast(10),
        ),
        (
            "global deploy".into(),
            "launchd systemd deploy",
            SearchScope::Global,
            fast(20),
        ),
        (
            "global ranking".into(),
            "vector search ranking fusion",
            SearchScope::Global,
            fast(6),
        ),
        (
            "session a fast".into(),
            "rank fusion and reranker",
            SearchScope::Session(a),
            fast(8),
        ),
        (
            "session a local".into(),
            "rank fusion and reranker",
            SearchScope::Session(a),
            local.clone(),
        ),
        (
            "session a local graph".into(),
            "retrieval graph entity relations",
            SearchScope::Session(a),
            local.clone(),
        ),
        (
            "sessions a c".into(),
            "compaction",
            SearchScope::Sessions(vec![a, c]),
            fast(8),
        ),
        (
            "workspace deploy".into(),
            "SurrealDB server credentials",
            SearchScope::Workspace(workspace),
            fast(8),
        ),
        (
            "global all corpora".into(),
            "archive import refused",
            SearchScope::Global,
            SearchOptions {
                corpus: Corpus::All,
                ..fast(10)
            },
        ),
        (
            "session c messages".into(),
            "archive import refused",
            SearchScope::Session(c),
            SearchOptions {
                corpus: Corpus::Messages,
                ..fast(10)
            },
        ),
        (
            "global by author".into(),
            "compaction archive daemon",
            SearchScope::Global,
            SearchOptions {
                author: Some("julius".into()),
                ..fast(10)
            },
        ),
        (
            "global prefer author".into(),
            "vector rank fusion",
            SearchScope::Global,
            SearchOptions {
                prefer_author: Some("claude-code".into()),
                ..fast(10)
            },
        ),
        (
            "global local".into(),
            "export archive model",
            SearchScope::Global,
            local,
        ),
    ]
}

/// Every case's result lines for one engine on one backend.
async fn run(memory: &Memory) -> BTreeMap<String, Vec<String>> {
    let (labels, sessions) = corpus(memory).await;
    let workspace = memory.workspaces.find_by_name("pin").unwrap().id;
    let mut out = BTreeMap::new();
    for (name, query, scope, options) in cases(&sessions, workspace) {
        let results = memory.search.search(scope, query, options).await.unwrap();
        out.insert(name, results.iter().map(|r| line(r, &labels)).collect());
    }
    out
}

/// The capability services over `backend` with embeddings on, embedding
/// with `engine` (`hash` or `model2vec`) and reranking `local` searches
/// with `reranker`.
async fn pinned_memory(backend: &Backend, engine: &str, reranker: Arc<dyn Reranker>) -> Memory {
    let engine: Arc<dyn post_cortex_ports::EmbeddingEngine> = match engine {
        "hash" => hash_engine(),
        _ => Arc::new(
            post_cortex_embeddings::LocalEmbeddingEngine::new(
                post_cortex_embeddings::EmbeddingConfig::default(),
            )
            .await
            .unwrap(),
        ),
    };
    let mut ports = backend.ports(Some(engine), None);
    ports.reranker = Some(reranker);
    Memory::over_ports(ports, embeddings_config())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn retrieval_is_pinned() {
    let mut all: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for engine in ["hash", "model2vec"] {
        for backend in backends().await {
            let memory = pinned_memory(&backend, engine, Arc::new(OverlapReranker)).await;
            all.insert(format!("{engine} {}", backend.name), run(&memory).await);
        }
    }
    let got = serde_json::to_string_pretty(&all).unwrap() + "\n";
    if std::env::var_os("PIN_UPDATE").is_some() {
        std::fs::write(FIXTURE, &got).unwrap();
    }
    let want = std::fs::read_to_string(FIXTURE).expect("fixture missing: run with PIN_UPDATE=1");
    let want: BTreeMap<String, BTreeMap<String, Vec<String>>> =
        serde_json::from_str(&want).unwrap();
    for (run, cases) in &want {
        for (case, lines) in cases {
            assert_eq!(
                all.get(run).and_then(|c| c.get(case)),
                Some(lines),
                "{run} / {case}"
            );
        }
    }
    assert_eq!(all, want);
}
