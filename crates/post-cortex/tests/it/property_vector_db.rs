// Property-based tests of the vector database (proptest): dimensions are
// checked, a search returns at most k hits best first with cosine
// similarities in [-1, 1], an upsert of a content again replaces it, a
// deleted entry is never found, and the counts follow the operations.

use post_cortex_embeddings::VectorDB;
use post_cortex_ports::{EntrySelect, IndexFilter, VectorMetadata};
use proptest::prelude::*;

fn dimension_strategy() -> impl Strategy<Value = usize> {
    prop_oneof![Just(16), Just(128), Just(256), Just(384)]
}

/// Vectors of any length (the database normalises them).
fn vector_strategy(dim: usize) -> impl Strategy<Value = Vec<f32>> {
    prop::collection::vec(-1.0f32..1.0f32, dim)
}

fn meta(i: usize, session: &str) -> VectorMetadata {
    VectorMetadata::new(
        format!("u{i}"),
        format!("text number {i}"),
        session.to_string(),
        "DecisionPoint".into(),
    )
}

fn loaded(vectors: &[Vec<f32>]) -> VectorDB {
    let db = VectorDB::new(vectors[0].len());
    db.upsert(
        vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (v.clone(), meta(i, if i % 2 == 0 { "even" } else { "odd" }))),
    );
    db
}

proptest! {
    /// Vectors of another dimension are refused and counted; the rest stay.
    #[test]
    fn prop_dimension_validation(
        dim in dimension_strategy(),
        wrong in 1usize..64,
    ) {
        let db = VectorDB::new(dim);
        let report = db.upsert([
            (vec![0.5; dim], meta(0, "s")),
            (vec![0.5; dim + wrong], meta(1, "s")),
        ]);
        prop_assert_eq!((report.loaded, report.rejected_dimension), (1, 1));
        prop_assert_eq!(db.len(), 1);
    }

    /// At most k hits, best first, similarities within [-1, 1].
    #[test]
    fn prop_search_bounds_order_and_range(
        vectors in prop::collection::vec(vector_strategy(32), 1..60),
        query in vector_strategy(32),
        k in 1usize..20,
    ) {
        let db = loaded(&vectors);
        let hits = db.nearest(&query, &IndexFilter::default(), k);
        prop_assert!(hits.len() <= k.min(vectors.len()));
        prop_assert!(hits.windows(2).all(|w| w[0].similarity >= w[1].similarity));
        prop_assert!(hits.iter().all(|h| (-1.0001..=1.0001).contains(&h.similarity)));
    }

    /// A stored vector is its own best match, at similarity 1.
    #[test]
    fn prop_identical_vector_similarity(vector in vector_strategy(64)) {
        prop_assume!(vector.iter().any(|x| x.abs() > 1e-3));
        let db = loaded(std::slice::from_ref(&vector));
        let hits = db.nearest(&vector, &IndexFilter::default(), 1);
        prop_assert!((hits[0].similarity - 1.0).abs() < 1e-4);
    }

    /// Upserting the same contents again leaves one entry per content.
    #[test]
    fn prop_upsert_idempotency(
        vectors in prop::collection::vec(vector_strategy(16), 1..30),
        rounds in 1usize..4,
    ) {
        let db = loaded(&vectors);
        for _ in 0..rounds {
            db.upsert(
                vectors
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (v.clone(), meta(i, if i % 2 == 0 { "even" } else { "odd" }))),
            );
        }
        prop_assert_eq!(db.len(), vectors.len());
        let per_session: usize = db.per_session().values().sum();
        prop_assert_eq!(per_session, vectors.len());
    }

    /// Deleted entries are never found again, and the rest still are.
    #[test]
    fn prop_delete_consistency(
        vectors in prop::collection::vec(vector_strategy(16), 2..40),
        query in vector_strategy(16),
    ) {
        let db = loaded(&vectors);
        let gone: Vec<String> = (0..vectors.len()).step_by(2).map(|i| format!("u{i}")).collect();
        let (removed, _) = db.delete("even", &gone);
        prop_assert_eq!(removed, gone.len());
        prop_assert_eq!(db.len(), vectors.len() - gone.len());
        let hits = db.nearest(&query, &IndexFilter::default(), vectors.len());
        prop_assert_eq!(hits.len(), vectors.len() - gone.len());
        prop_assert!(hits.iter().all(|h| h.meta.source == "odd"));
        prop_assert!(db.entries("even", &EntrySelect::All).is_empty());
    }
}
