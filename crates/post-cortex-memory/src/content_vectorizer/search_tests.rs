// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Search over the vectorizer: full-text fusion, rerankers and modes, graph
//! terms, in-memory BM25 and chunked long updates.

use super::{ContentVectorizer, SearchOptions};
use crate::content_vectorizer::VectorScope;
use crate::content_vectorizer::test_doubles::FakeReranker;
use crate::content_vectorizer::tests::copies;
use crate::content_vectorizer::tests::session_with;
use crate::content_vectorizer::tests::update;
use crate::content_vectorizer::tests::vectorizer;
use crate::memory_system::VectorizedIds;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::IndexFilter;
use std::sync::Arc;
use uuid::Uuid;

/// A record the vector search does not rank in the top `limit` comes back
/// once BM25 ranks it first, and results never exceed `limit`. A query of
/// only common words ranks by the vectors alone.
#[tokio::test]
async fn full_text_hits_are_fused_with_vector_results() {
    let marks = VectorizedIds::default();
    let (v, _) = vectorizer().await;
    let titles: Vec<(String, String)> = (0..30)
        .map(|i| (format!("filler topic number {i}"), format!("e{i}")))
        .chain([("zebracorn migration plan".to_string(), "zebra".to_string())])
        .collect();
    let refs: Vec<(&str, &str)> = titles
        .iter()
        .map(|(t, e)| (t.as_str(), e.as_str()))
        .collect();
    let session = session_with(&refs).await;
    v.vectorize_session(&session, &marks).await.unwrap();
    let zebra = session
        .records()
        .iter()
        .find(|u| u.content.title.starts_with("zebracorn"))
        .unwrap()
        .id
        .to_string();
    let opts = || SearchOptions {
        recency_bias: Some(0.0),
        ..SearchOptions::default()
    };
    let has_zebra = |rs: &[super::SemanticSearchResult]| rs.iter().any(|r| r.content_id == zebra);

    let vector_only = v
        .search(
            "filler topic number",
            3,
            VectorScope::Sessions(&[session.id()]),
            opts(),
        )
        .await
        .unwrap();
    assert_eq!(vector_only.len(), 3);
    assert!(!has_zebra(&vector_only));

    // Words on most records are not searched for, so the query needs one
    // that is not ("migration").
    let fused = v
        .search(
            "filler topic number migration",
            3,
            VectorScope::Sessions(&[session.id()]),
            opts(),
        )
        .await
        .unwrap();
    assert_eq!(fused.len(), 3);
    assert!(
        has_zebra(&fused),
        "{:?}",
        fused.iter().map(|r| &r.text_content).collect::<Vec<_>>()
    );

    // Other sessions' full-text hits stay out of a session-scoped search.
    let other = session_with(&[("unrelated", "u")]).await;
    let fused_other = v
        .search(
            "filler topic number",
            3,
            VectorScope::Sessions(&[other.id()]),
            opts(),
        )
        .await
        .unwrap();
    assert!(!has_zebra(&fused_other));
}

/// The reranker reorders the whole candidate pool, not just the top
/// `limit`: a record the vector search ranks below `limit` comes first once
/// the reranker prefers it. A failing reranker leaves the fused order.
#[tokio::test]
async fn reranker_reorders_the_candidate_pool() {
    let marks = VectorizedIds::default();
    let titles: Vec<(String, String)> = (0..30)
        .map(|i| (format!("filler topic number {i}"), format!("e{i}")))
        .chain([("zebracorn migration plan".to_string(), "zebra".to_string())])
        .collect();
    let refs: Vec<(&str, &str)> = titles
        .iter()
        .map(|(t, e)| (t.as_str(), e.as_str()))
        .collect();
    let session = session_with(&refs).await;
    let opts = || SearchOptions {
        recency_bias: Some(0.0),
        ..SearchOptions::default()
    };
    let texts = |rs: &[super::SemanticSearchResult]| {
        rs.iter()
            .map(|r| r.text_content.clone())
            .collect::<Vec<_>>()
    };

    let (mut v, _) = vectorizer().await;
    v.set_reranker(Arc::new(FakeReranker {
        needle: "zebracorn",
    }));
    v.vectorize_session(&session, &marks).await.unwrap();
    let reranked = v
        .search(
            "filler topic number",
            3,
            VectorScope::Sessions(&[session.id()]),
            opts(),
        )
        .await
        .unwrap();
    assert_eq!(reranked.len(), 3);
    assert!(
        reranked[0].text_content.contains("zebracorn"),
        "{:?}",
        texts(&reranked)
    );

    let (mut down, _) = vectorizer().await;
    down.set_reranker(Arc::new(FakeReranker { needle: "" }));
    down.vectorize_session(&session, &marks).await.unwrap();
    let fallback = down
        .search(
            "filler topic number",
            3,
            VectorScope::Sessions(&[session.id()]),
            opts(),
        )
        .await
        .unwrap();
    assert_eq!(fallback.len(), 3);
    assert!(
        !fallback[0].text_content.contains("zebracorn"),
        "{:?}",
        texts(&fallback)
    );
}

/// Each mode uses its own reranker, `accurate` the remote one only where
/// it is allowed (else the local one), `accurate` is the default when a
/// remote reranker is set, and the query cache keeps modes and remote
/// reranking apart.
#[tokio::test]
async fn search_modes_pick_their_reranker() {
    let marks = VectorizedIds::default();
    let titles: Vec<(String, String)> = (0..30)
        .map(|i| (format!("filler topic number {i}"), format!("e{i}")))
        .chain([
            ("zebracorn migration plan".to_string(), "zebra".to_string()),
            ("quokka release notes".to_string(), "quokka".to_string()),
        ])
        .collect();
    let refs: Vec<(&str, &str)> = titles
        .iter()
        .map(|(t, e)| (t.as_str(), e.as_str()))
        .collect();
    let session = session_with(&refs).await;
    let (mut v, _) = vectorizer().await;
    v.set_reranker(Arc::new(FakeReranker {
        needle: "zebracorn",
    }));
    v.set_accurate_reranker(Arc::new(FakeReranker { needle: "quokka" }));
    v.vectorize_session(&session, &marks).await.unwrap();

    use super::SearchMode::{Accurate, Fast, Local};
    let id = session.id();
    assert!(top(&v, id, Some(Local), true).await.contains("zebracorn"));
    assert!(top(&v, id, Some(Accurate), true).await.contains("quokka"));
    assert!(
        top(&v, id, Some(Accurate), false)
            .await
            .contains("zebracorn"),
        "not allowed: the local reranker"
    );
    assert!(
        top(&v, id, None, true).await.contains("quokka"),
        "default is accurate"
    );
    let fast = top(&v, id, Some(Fast), true).await;
    assert!(
        !fast.contains("zebracorn") && !fast.contains("quokka"),
        "{fast}"
    );

    v.set_default_mode(Local);
    assert!(top(&v, id, None, true).await.contains("zebracorn"));
}

/// Text of the best result for "filler topic number" in `mode`, the
/// remote reranker allowed when `remote`.
async fn top(
    v: &ContentVectorizer,
    session: Uuid,
    mode: Option<super::SearchMode>,
    remote: bool,
) -> String {
    let opts = SearchOptions {
        recency_bias: Some(0.0),
        mode,
        remote_rerank: remote,
        ..SearchOptions::default()
    };
    v.search(
        "filler topic number",
        3,
        VectorScope::Sessions(&[session]),
        opts,
    )
    .await
    .unwrap()[0]
        .text_content
        .clone()
}

/// Vectorizing stores each update's graph terms and agent entities in the
/// vector's metadata, and `graph_records` reads them back for the session's
/// retrieval graph.
#[tokio::test]
async fn update_vectors_carry_graph_terms_and_entities() {
    let marks = VectorizedIds::default();
    use post_cortex_core::graph::retrieval_graph::RetrievalGraph;

    let titles: Vec<(String, String)> = (0..40)
        .map(|i| (format!("filler topic number {i}"), format!("e{i}")))
        .chain([
            (
                "Caroline adopted a puppy called Biscuit".to_string(),
                "puppy".to_string(),
            ),
            (
                "Biscuit chewed the sofa yesterday".to_string(),
                "sofa".to_string(),
            ),
        ])
        .collect();
    let refs: Vec<(&str, &str)> = titles
        .iter()
        .map(|(t, e)| (t.as_str(), e.as_str()))
        .collect();
    let session = session_with(&refs).await;
    let (v, _) = vectorizer().await;
    v.vectorize_session(&session, &marks).await.unwrap();

    let records = v.graph_records(session.id()).await.unwrap();
    assert_eq!(records.len(), 42, "entity vectors are not records");
    let adopted = records
        .iter()
        .find(|r| r.terms.contains("biscuit") && r.terms.contains("adopt"))
        .expect("terms stored with the vector");
    assert!(
        adopted.entities.iter().any(|e| e == "puppy"),
        "{:?}",
        adopted.entities
    );

    let id_of = |title: &str| {
        session
            .records()
            .iter()
            .find(|u| u.content.title == title)
            .unwrap()
            .id
            .to_string()
    };
    let ranked = RetrievalGraph::build(records, &[]).rank("What did Caroline's puppy chew?", 5);
    assert_eq!(
        ranked.first(),
        Some(&id_of("Caroline adopted a puppy called Biscuit"))
    );
    assert!(
        ranked.contains(&id_of("Biscuit chewed the sofa yesterday")),
        "second hop"
    );
}

/// BM25 comes from the vector DB's in-memory index, scoped to the session.
#[tokio::test]
async fn in_memory_bm25_is_scoped_to_the_session() {
    let marks = VectorizedIds::default();
    let titles: Vec<(String, String)> = (0..30)
        .map(|i| (format!("filler topic number {i}"), format!("e{i}")))
        .chain([("zebracorn migration plan".to_string(), "zebra".to_string())])
        .collect();
    let refs: Vec<(&str, &str)> = titles
        .iter()
        .map(|(t, e)| (t.as_str(), e.as_str()))
        .collect();
    let session = session_with(&refs).await;
    let other = session_with(&[("zebracorn in another session", "z")]).await;
    let in_session = IndexFilter {
        sessions: Some(vec![session.id().to_string()]),
        ..Default::default()
    };

    let (v, _) = vectorizer().await;
    v.vectorize_session(&session, &marks).await.unwrap();
    v.vectorize_session(&other, &marks).await.unwrap();
    // "filler" is on most records, so it is not searched for.
    assert!(
        v.index()
            .lexical("filler", &in_session, 5)
            .await
            .unwrap()
            .is_empty()
    );
    let hits = v
        .index()
        .lexical("filler zebracorn", &in_session, 5)
        .await
        .unwrap();
    assert!(
        hits[0].meta.text.contains("zebracorn migration"),
        "{hits:?}"
    );
    let session_id = session.id().to_string();
    assert!(hits.iter().all(|h| h.meta.source == session_id));

    // Search finds the rare word through it.
    let results = v
        .search(
            "zebracorn",
            10,
            VectorScope::Sessions(&[session.id()]),
            SearchOptions {
                recency_bias: Some(0.0),
                ..SearchOptions::default()
            },
        )
        .await
        .unwrap();
    assert!(results.iter().any(|r| r.text_content.contains("zebracorn")));
}

/// A long update is embedded as chunks, so a passage far past the first
/// chunk is found; the hit is the update itself (one result, the update's
/// id), and removing the update removes every chunk.
#[tokio::test]
async fn long_updates_are_chunked_and_found_whole() {
    let marks = VectorizedIds::default();
    let mut long = update("design notes", "notes");
    let filler: String = (0..300)
        .map(|i| format!("ordinary sentence {i}. "))
        .collect();
    long.content.description = format!("{filler} The quokkaplan deadline is Friday. {filler}");
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    session.apply_record(long.clone());
    for i in 0..5 {
        session.apply_record(update(&format!("short note {i}"), &format!("n{i}")));
    }

    let (v, _) = vectorizer().await;
    v.vectorize_session(&session, &marks).await.unwrap();
    let long_id = long.id.to_string();
    let mut chunk_ids = Vec::new();
    for n in 2.. {
        let id = format!("{long_id}#{n}");
        if copies(&v, session.id(), &id).await == 0 {
            break;
        }
        chunk_ids.push(id);
    }
    assert!(chunk_ids.len() > 3, "long update split into chunks");

    let results = v
        .search(
            "quokkaplan deadline",
            5,
            VectorScope::Sessions(&[session.id()]),
            SearchOptions {
                recency_bias: Some(0.0),
                ..SearchOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        results[0].content_id, long_id,
        "the chunk answers for the update"
    );
    assert_eq!(
        results.iter().filter(|r| r.content_id == long_id).count(),
        1,
        "one result per update"
    );

    v.remove_contents(session.id(), std::slice::from_ref(&long_id))
        .await
        .unwrap();
    assert_eq!(copies(&v, session.id(), &long_id).await, 0);
    for id in &chunk_ids {
        assert_eq!(copies(&v, session.id(), id).await, 0);
    }
}
