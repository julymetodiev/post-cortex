// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The vector database: exact top-k against a naive ranking (short and
//! parallel scans), upserts and deletes by (session, content id), slot
//! reuse, filters, BM25 and deterministic ties.

use std::collections::HashMap;

use post_cortex_ports::{
    AUTHOR_KEY, Corpus, EntrySelect, IndexFilter, MESSAGE_CONTENT_TYPE, VectorMetadata,
};

use super::*;

/// Deterministic pseudo-random vectors (xorshift).
fn vectors(n: usize, dim: usize, seed: u64) -> Vec<Vec<f32>> {
    let mut state = seed.max(1);
    (0..n)
        .map(|_| {
            (0..dim)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    (state % 2000) as f32 / 1000.0 - 1.0
                })
                .collect()
        })
        .collect()
}

fn meta(session: &str, id: &str, text: &str) -> VectorMetadata {
    VectorMetadata {
        id: id.into(),
        text: text.into(),
        source: session.into(),
        content_type: "DecisionPoint".into(),
        timestamp: chrono::DateTime::UNIX_EPOCH,
        metadata: HashMap::new(),
    }
}

/// The ids of the `k` rows most similar to `query`, by plain cosine.
fn naive(rows: &[Vec<f32>], query: &[f32], k: usize) -> Vec<String> {
    let mut scored: Vec<(usize, f32)> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| (i, cosine_similarity(row, query)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored
        .iter()
        .take(k)
        .map(|(i, _)| format!("u{i}"))
        .collect()
}

fn loaded(rows: &[Vec<f32>]) -> VectorDB {
    let db = VectorDB::new(rows[0].len());
    let report = db.upsert(rows.iter().enumerate().map(|(i, row)| {
        (
            row.clone(),
            meta("s", &format!("u{i}"), &format!("text {i}")),
        )
    }));
    assert_eq!(report.loaded, rows.len());
    db
}

fn ids(hits: &[post_cortex_ports::IndexHit]) -> Vec<String> {
    hits.iter().map(|h| h.meta.id.clone()).collect()
}

#[test]
fn a_short_scan_returns_the_true_top_k_in_order() {
    let rows = vectors(500, 32, 7);
    let db = loaded(&rows);
    for query in vectors(5, 32, 99) {
        let hits = db.nearest(&query, &IndexFilter::default(), 10);
        assert_eq!(ids(&hits), naive(&rows, &query, 10));
        assert!(hits.windows(2).all(|w| w[0].similarity >= w[1].similarity));
    }
}

/// Past one thread's share the scan runs in parallel and still finds the
/// exact top-k.
#[test]
fn a_parallel_scan_returns_the_true_top_k() {
    let rows = vectors(40_000, 16, 3);
    let db = loaded(&rows);
    let query = &vectors(1, 16, 11)[0];
    let hits = db.nearest(query, &IndexFilter::default(), 25);
    assert_eq!(ids(&hits), naive(&rows, query, 25));
}

#[test]
fn rows_are_normalised_and_similarity_is_cosine() {
    let db = VectorDB::new(3);
    db.upsert([(vec![3.0, 4.0, 0.0], meta("s", "u1", "t"))]);
    let entries = db.entries("s", &EntrySelect::All);
    assert_eq!(entries[0].vector, vec![0.6, 0.8, 0.0]);
    let hits = db.nearest(&[30.0, 40.0, 0.0], &IndexFilter::default(), 1);
    assert!((hits[0].similarity - 1.0).abs() < 1e-6);
}

/// An upsert replaces the entry of its (session, content id) and reuses
/// freed slots; another session's entry under the same id stays.
#[test]
fn upserts_replace_and_deletes_free_slots_for_reuse() {
    let db = VectorDB::new(2);
    db.upsert([
        (vec![1.0, 0.0], meta("a", "u1", "first")),
        (vec![0.0, 1.0], meta("b", "u1", "other")),
    ]);
    db.upsert([(vec![0.0, 1.0], meta("a", "u1", "second"))]);
    assert_eq!(db.len(), 2);
    let a = db.entries("a", &EntrySelect::All);
    assert_eq!(&*a[0].meta.text, "second");
    assert_eq!(
        db.per_session(),
        HashMap::from([("a".into(), 1), ("b".into(), 1)])
    );
    let terms = db.query_terms("first");
    assert!(db.lexical(&terms, &IndexFilter::default(), 5).is_empty());

    let before = db.memory_bytes();
    for round in 0..50 {
        db.upsert([(vec![1.0, 1.0], meta("a", "tmp", &format!("round {round}")))]);
        assert_eq!(db.delete("a", &["tmp".to_string()]).0, 1);
    }
    assert_eq!(db.len(), 2);
    assert!(db.memory_bytes() <= before + 64, "slots are reused");
}

#[test]
fn a_delete_takes_the_chunks_and_names_them_for_storage() {
    let db = VectorDB::new(2);
    db.upsert([
        (vec![1.0, 0.0], meta("a", "u1", "one")),
        (vec![1.0, 0.0], meta("a", "u1#2", "two")),
        (vec![1.0, 0.0], meta("a", "u1#3", "three")),
        (vec![1.0, 0.0], meta("b", "u1", "other")),
    ]);
    let (removed, stored) = db.delete("a", &["u1".to_string(), "missing".to_string()]);
    assert_eq!(removed, 3);
    assert_eq!(stored, ["u1", "u1#2", "u1#3", "missing"]);
    assert_eq!(db.len(), 1);
    assert_eq!(db.delete("nobody", &["u1".to_string()]).0, 0);
}

#[test]
fn filters_keep_to_sessions_corpus_author_and_dates() {
    let db = VectorDB::new(2);
    let mut message = meta("a", "message:1", "a turn about rust");
    message.content_type = MESSAGE_CONTENT_TYPE.into();
    let mut by_julius = meta("b", "u3", "rust by julius");
    by_julius
        .metadata
        .insert(AUTHOR_KEY.into(), "julius".into());
    by_julius.timestamp = chrono::DateTime::UNIX_EPOCH + chrono::Duration::days(2);
    db.upsert([
        (vec![1.0, 0.0], meta("a", "u1", "rust record")),
        (vec![1.0, 0.1], message),
        (vec![1.0, 0.2], by_julius),
    ]);
    let q = [1.0, 0.0];
    let in_a = IndexFilter {
        sessions: Some(vec!["a".into(), "a".into(), "z".into()]),
        ..Default::default()
    };
    assert_eq!(ids(&db.nearest(&q, &in_a, 10)), ["u1"]);
    let messages = IndexFilter {
        corpus: Corpus::Messages,
        ..Default::default()
    };
    assert_eq!(ids(&db.nearest(&q, &messages, 10)), ["message:1"]);
    let author = IndexFilter {
        author: Some("julius".into()),
        ..Default::default()
    };
    assert_eq!(ids(&db.nearest(&q, &author, 10)), ["u3"]);
    let day = chrono::DateTime::UNIX_EPOCH + chrono::Duration::days(1);
    let later = IndexFilter {
        date_range: Some((day, day + chrono::Duration::days(5))),
        ..Default::default()
    };
    assert_eq!(ids(&db.nearest(&q, &later, 10)), ["u3"]);

    let terms = db.query_terms("rust");
    assert_eq!(ids(&db.lexical(&terms, &in_a, 10)), ["u1"]);
    let all = IndexFilter {
        corpus: Corpus::All,
        ..Default::default()
    };
    assert_eq!(db.lexical(&terms, &all, 10).len(), 3);
}

#[test]
fn score_answers_in_the_given_order() {
    let db = VectorDB::new(2);
    db.upsert([
        (vec![1.0, 0.0], meta("a", "u1", "one")),
        (vec![0.0, 1.0], meta("a", "u2", "two")),
        (vec![1.0, 0.0], meta("b", "u3", "three")),
    ]);
    let ids_given = ["u2", "missing", "u3", "u1"].map(String::from);
    let hits = db.score(&[1.0, 0.0], "a", &ids_given);
    assert_eq!(ids(&hits), ["u2", "u1"]);
    assert!(hits[1].similarity > hits[0].similarity);
}

/// Equal scores rank by what the entries are, whatever order they were
/// stored in.
#[test]
fn ties_do_not_depend_on_insertion_order() {
    let entries: Vec<(Vec<f32>, VectorMetadata)> = (0..20)
        .map(|i| {
            (
                vec![1.0, 0.0],
                meta("s", &format!("u{i}"), "same words here"),
            )
        })
        .collect();
    let forward = VectorDB::new(2);
    forward.upsert(entries.clone());
    let backward = VectorDB::new(2);
    backward.upsert(entries.into_iter().rev());
    let q = [1.0, 0.0];
    let filter = IndexFilter::default();
    assert_eq!(
        ids(&forward.nearest(&q, &filter, 5)),
        ids(&backward.nearest(&q, &filter, 5))
    );
    let terms = forward.query_terms("words");
    assert_eq!(
        ids(&forward.lexical(&terms, &filter, 5)),
        ids(&backward.lexical(&terms, &filter, 5))
    );
}

#[test]
fn vectors_of_another_dimension_are_refused_and_counted() {
    let db = VectorDB::new(2);
    let report = db.upsert([
        (vec![1.0, 0.0], meta("a", "u1", "one")),
        (vec![1.0, 0.0, 0.0], meta("a", "u2", "two")),
    ]);
    assert_eq!((report.loaded, report.rejected_dimension), (1, 1));
}
