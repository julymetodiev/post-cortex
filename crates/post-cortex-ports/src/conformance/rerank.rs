// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The reranker port's contract: one relevance in [0, 1] per document, in
//! input order, and none for no documents. How well a model ranks is the
//! adapter's own test.

use std::sync::Arc;

use crate::rerank::Reranker;

/// Scores that differ by less than this are the same score (a batch of
/// another shape may round the last digits differently).
const TOLERANCE: f32 = 1e-3;

const QUERY: &str = "What did Caroline research?";

fn documents() -> Vec<Arc<str>> {
    [
        "Melanie: I went camping with the kids last weekend.",
        "Caroline: I researched adoption agencies yesterday.",
        "Melanie: The weather has been great lately.",
    ]
    .map(Arc::from)
    .to_vec()
}

/// No documents, no scores.
pub async fn no_documents_no_scores(reranker: &dyn Reranker) {
    assert!(reranker.score(QUERY, &[]).await.unwrap().is_empty());
}

/// One relevance in [0, 1] per document, following the documents when
/// their order changes.
pub async fn one_score_per_document_in_input_order(reranker: &dyn Reranker) {
    let documents = documents();
    let scores = reranker.score(QUERY, &documents).await.unwrap();
    assert_eq!(scores.len(), documents.len());
    assert!(scores.iter().all(|s| (0.0..=1.0).contains(s)), "{scores:?}");

    let reversed: Vec<Arc<str>> = documents.iter().rev().cloned().collect();
    let mut again = reranker.score(QUERY, &reversed).await.unwrap();
    again.reverse();
    assert!(
        scores
            .iter()
            .zip(&again)
            .all(|(a, b)| (a - b).abs() < TOLERANCE),
        "{scores:?} vs {again:?}"
    );
}

/// One `#[tokio::test]` per reranker case, against the reranker `$open`
/// opens (an async expression evaluating to
/// `(Arc<dyn Reranker>, keep_alive)`).
#[macro_export]
macro_rules! rerank_conformance_suite {
    ($open:expr) => {
        $crate::rerank_conformance_cases!(
            $open;
            no_documents_no_scores,
            one_score_per_document_in_input_order
        );
    };
}

/// The expansion of [`rerank_conformance_suite!`].
#[doc(hidden)]
#[macro_export]
macro_rules! rerank_conformance_cases {
    ($open:expr; $($case:ident),+) => {
        $(
            #[tokio::test(flavor = "multi_thread")]
            async fn $case() {
                let (reranker, _keep) = $open.await;
                $crate::conformance::rerank::$case(&*reranker).await;
            }
        )+
    };
}
