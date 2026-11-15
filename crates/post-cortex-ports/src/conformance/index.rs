// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `SearchIndex` port: entries are keyed by (session, content id), a
//! content id takes its chunks, retrieval keeps to its filter, and a
//! vector of another dimension is refused. The index under test is empty
//! and holds [`DIMENSION`]-long vectors.

use std::collections::HashMap;

use crate::conformance::at;
use crate::index::{EntrySelect, IndexEntry, IndexFilter, SearchIndex};
use crate::vectors::{AUTHOR_KEY, Corpus, MESSAGE_CONTENT_TYPE, VectorMetadata};

/// The length of the vectors of the index under test.
pub const DIMENSION: usize = 16;

/// An entry of `session` whose vector points along axis `axis` (plus a
/// little of axis 0), with `text`.
pub fn entry(session: &str, content_id: &str, axis: usize, text: &str) -> IndexEntry {
    let mut vector = vec![0.0; DIMENSION];
    vector[axis] = 1.0;
    vector[0] += 0.1;
    IndexEntry {
        vector,
        meta: VectorMetadata {
            id: content_id.into(),
            text: text.into(),
            source: session.into(),
            content_type: "DecisionPoint".into(),
            timestamp: at(axis as i64),
            metadata: HashMap::new(),
        },
    }
}

/// `vector` scaled to length 1, as the index stores it.
fn normalised(mut vector: Vec<f32>) -> Vec<f32> {
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    vector.iter_mut().for_each(|x| *x /= norm);
    vector
}

fn axis(n: usize) -> Vec<f32> {
    let mut v = vec![0.0; DIMENSION];
    v[n] = 1.0;
    v
}

fn ids(entries: &[IndexEntry]) -> Vec<String> {
    let mut ids: Vec<String> = entries.iter().map(|e| e.meta.id.clone()).collect();
    ids.sort();
    ids
}

/// Upserting a content again replaces it; the same content id in another
/// session is its own entry.
pub async fn upsert_replaces_by_session_and_content(index: &dyn SearchIndex) {
    index
        .upsert(vec![
            entry("a", "u1", 1, "first"),
            entry("b", "u1", 2, "other"),
        ])
        .await
        .unwrap();
    index
        .upsert(vec![entry("a", "u1", 3, "second")])
        .await
        .unwrap();
    let a = index.entries("a", &EntrySelect::All).await.unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(&*a[0].meta.text, "second");
    let expected = normalised(entry("a", "u1", 3, "").vector);
    assert!(
        a[0].vector
            .iter()
            .zip(&expected)
            .all(|(x, y)| (x - y).abs() < 1e-6),
        "{:?}",
        a[0].vector
    );
    let stats = index.stats().await.unwrap();
    assert_eq!(stats.entries, 2);
    assert_eq!(stats.per_session.get("a"), Some(&1));
    assert_eq!(stats.dimension, DIMENSION);
}

/// A content id deletes its chunks, in its session only, and reports how
/// many entries went.
pub async fn delete_takes_chunks_in_the_session(index: &dyn SearchIndex) {
    index
        .upsert(vec![
            entry("a", "u1", 1, "one"),
            entry("a", "u1#2", 2, "one, part two"),
            entry("a", "u1#3", 3, "one, part three"),
            entry("a", "u2", 4, "two"),
            entry("b", "u1", 5, "other"),
        ])
        .await
        .unwrap();
    let gone = index
        .delete("a", &["u1".to_string(), "missing".to_string()])
        .await
        .unwrap();
    assert_eq!(gone, 3);
    assert_eq!(
        ids(&index.entries("a", &EntrySelect::All).await.unwrap()),
        ["u2"]
    );
    assert_eq!(
        ids(&index.entries("b", &EntrySelect::All).await.unwrap()),
        ["u1"]
    );
    assert_eq!(index.delete("a", &[]).await.unwrap(), 0);
}

/// Entries are selected by content id (with chunks) or by kind.
pub async fn entries_select_by_content_or_kind(index: &dyn SearchIndex) {
    let mut message = entry("a", "message:1", 4, "a turn");
    message.meta.content_type = MESSAGE_CONTENT_TYPE.into();
    index
        .upsert(vec![
            entry("a", "u1", 1, "one"),
            entry("a", "u1#2", 2, "one, part two"),
            entry("a", "u2", 3, "two"),
            message,
        ])
        .await
        .unwrap();
    let by_id = index
        .entries("a", &EntrySelect::ContentIds(vec!["u1".into()]))
        .await
        .unwrap();
    assert_eq!(ids(&by_id), ["u1", "u1#2"]);
    let by_kind = index
        .entries("a", &EntrySelect::Kinds(vec![MESSAGE_CONTENT_TYPE.into()]))
        .await
        .unwrap();
    assert_eq!(ids(&by_kind), ["message:1"]);
    assert!(
        index
            .entries("z", &EntrySelect::All)
            .await
            .unwrap()
            .is_empty()
    );
}

/// `nearest` ranks by similarity within the sessions, corpus, dates and
/// author of its filter.
pub async fn nearest_keeps_to_the_filter(index: &dyn SearchIndex) {
    let mut by_julius = entry("a", "u3", 3, "three");
    by_julius
        .meta
        .metadata
        .insert(AUTHOR_KEY.into(), "julius".into());
    let mut message = entry("a", "message:1", 1, "a turn");
    message.meta.content_type = MESSAGE_CONTENT_TYPE.into();
    index
        .upsert(vec![
            entry("a", "u1", 1, "one"),
            entry("a", "u2", 2, "two"),
            by_julius,
            entry("b", "u4", 1, "four"),
            message,
        ])
        .await
        .unwrap();
    let q = axis(1);
    let all = index
        .nearest(&q, &IndexFilter::default(), 10)
        .await
        .unwrap();
    let first: Vec<&str> = all.iter().take(2).map(|h| h.meta.id.as_str()).collect();
    assert!(first.contains(&"u1") && first.contains(&"u4"), "{first:?}");
    assert!(
        all.iter()
            .all(|h| h.meta.content_type != MESSAGE_CONTENT_TYPE)
    );
    assert!(all.windows(2).all(|w| w[0].similarity >= w[1].similarity));

    let in_a = IndexFilter {
        sessions: Some(vec!["a".into()]),
        ..Default::default()
    };
    let hits = index.nearest(&q, &in_a, 10).await.unwrap();
    assert_eq!(hits[0].meta.id, "u1");
    assert!(hits.iter().all(|h| h.meta.source == "a"));

    let messages = IndexFilter {
        corpus: Corpus::Messages,
        ..Default::default()
    };
    let hits = index.nearest(&q, &messages, 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].meta.id, "message:1");

    let author = IndexFilter {
        author: Some("julius".into()),
        ..Default::default()
    };
    let hits = index.nearest(&q, &author, 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].meta.id, "u3");

    let dates = IndexFilter {
        date_range: Some((at(2), at(3))),
        ..Default::default()
    };
    let mut dated: Vec<String> = index
        .nearest(&q, &dates, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|h| h.meta.id.clone())
        .collect();
    dated.sort();
    assert_eq!(dated, ["u2", "u3"]);
    assert_eq!(
        index
            .nearest(&q, &IndexFilter::default(), 1)
            .await
            .unwrap()
            .len(),
        1
    );
}

/// `score` answers for the given entries of one session, in order.
pub async fn score_keeps_the_given_order(index: &dyn SearchIndex) {
    index
        .upsert(vec![
            entry("a", "u1", 1, "one"),
            entry("a", "u2", 2, "two"),
            entry("b", "u3", 3, "three"),
        ])
        .await
        .unwrap();
    let ids = ["u2", "missing", "u3", "u1"].map(String::from);
    let hits = index.score(&axis(1), "a", &ids).await.unwrap();
    let got: Vec<&str> = hits.iter().map(|h| h.meta.id.as_str()).collect();
    assert_eq!(got, ["u2", "u1"]);
    assert!(hits[1].similarity > hits[0].similarity);
}

/// `lexical` finds the entries with the query's words, within its filter.
pub async fn lexical_finds_words_in_scope(index: &dyn SearchIndex) {
    index
        .upsert(vec![
            entry(
                "a",
                "u1",
                1,
                "compaction stalls on the session column family",
            ),
            entry("a", "u2", 2, "bloom filters keep point reads fast"),
            entry("b", "u3", 3, "compaction tuning on another node"),
        ])
        .await
        .unwrap();
    let in_a = IndexFilter {
        sessions: Some(vec!["a".into()]),
        ..Default::default()
    };
    let hits = index.lexical("compaction", &in_a, 10).await.unwrap();
    let got: Vec<&str> = hits.iter().map(|h| h.meta.id.as_str()).collect();
    assert_eq!(got, ["u1"]);
    let everywhere = index
        .lexical("compaction", &IndexFilter::default(), 10)
        .await
        .unwrap();
    assert_eq!(everywhere.len(), 2);
}

/// An entry of another dimension fails the upsert, which stores nothing.
pub async fn another_dimension_is_refused(index: &dyn SearchIndex) {
    let mut wrong = entry("a", "u2", 2, "two");
    wrong.vector.push(0.0);
    assert!(
        index
            .upsert(vec![entry("a", "u1", 1, "one"), wrong])
            .await
            .is_err()
    );
    assert!(
        index
            .entries("a", &EntrySelect::All)
            .await
            .unwrap()
            .is_empty()
    );
}

/// One `#[tokio::test]` per case, run against the index that `$open` (an
/// async expression evaluating to `(Arc<dyn SearchIndex>, keep_alive)`)
/// opens afresh, empty and of [`DIMENSION`], for each case.
#[macro_export]
macro_rules! index_conformance_suite {
    ($open:expr) => {
        $crate::index_conformance_cases!(
            $open;
            upsert_replaces_by_session_and_content,
            delete_takes_chunks_in_the_session,
            entries_select_by_content_or_kind,
            nearest_keeps_to_the_filter,
            score_keeps_the_given_order,
            lexical_finds_words_in_scope,
            another_dimension_is_refused
        );
    };
}

/// The expansion of [`index_conformance_suite!`].
#[doc(hidden)]
#[macro_export]
macro_rules! index_conformance_cases {
    ($open:expr; $($case:ident),+) => {
        $(
            #[tokio::test(flavor = "multi_thread")]
            async fn $case() {
                let (index, _keep) = $open.await;
                $crate::conformance::index::$case(&*index).await;
            }
        )+
    };
}
