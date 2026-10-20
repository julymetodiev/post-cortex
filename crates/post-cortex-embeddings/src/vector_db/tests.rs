// Copyright (c) 2025, 2026 Julius ML
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

use super::*;

#[test]
fn test_vector_db_creation() {
    let config = VectorDbConfig::default();
    let db = VectorDB::new(config).unwrap();

    let stats = db.get_stats();
    assert_eq!(stats.total_vectors, 0);
    assert!(!stats.is_built);
}

#[test]
fn test_add_and_search_vector() {
    let mut config = VectorDbConfig::default();
    config.dimension = 3; // Use 3D vectors for testing
    let db = VectorDB::new(config).unwrap();

    let vector = vec![1.0, 0.0, 0.0];
    let metadata = VectorMetadata::new(
        "test1".to_string(),
        "session1".to_string(),
        "text".to_string(),
        "test content".to_string(),
    );

    let id = db.add_vector(vector.clone(), metadata).unwrap();
    assert_eq!(id, 0);

    let results = db.search(&vector, 5).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].vector_id, 0);
    assert!((results[0].similarity - 1.0).abs() < 0.001);
}

#[test]
fn test_vector_similarity() {
    let mut config = VectorDbConfig::default();
    config.dimension = 3; // Use 3D vectors for testing
    let db = VectorDB::new(config).unwrap();

    let vector1 = vec![1.0, 0.0, 0.0];
    let vector2 = vec![0.9, 0.1, 0.0]; // Similar vector

    let metadata1 = VectorMetadata::new(
        "test1".to_string(),
        "session1".to_string(),
        "text".to_string(),
        "content1".to_string(),
    );

    let metadata2 = VectorMetadata::new(
        "test2".to_string(),
        "session1".to_string(),
        "text".to_string(),
        "content2".to_string(),
    );

    db.add_vector(vector1.clone(), metadata1).unwrap();
    db.add_vector(vector2.clone(), metadata2).unwrap();

    let results = db.search(&vector1, 5).unwrap();
    assert_eq!(results.len(), 2);
    assert!(results[0].similarity > 0.9); // High similarity
}

#[test]
fn test_remove_vector() {
    let mut config = VectorDbConfig::default();
    config.dimension = 3; // Use 3D vectors for testing
    let db = VectorDB::new(config).unwrap();

    let vector = vec![1.0, 0.0, 0.0];
    let metadata = VectorMetadata::new(
        "test1".to_string(),
        "session1".to_string(),
        "text".to_string(),
        "test content".to_string(),
    );

    let id = db.add_vector(vector, metadata).unwrap();
    assert_eq!(db.get_stats().total_vectors, 1);

    let removed = db.remove_vector(id).unwrap();
    assert!(removed);
    assert_eq!(db.get_stats().total_vectors, 0);

    let results = db.search(&[1.0, 0.0, 0.0], 5).unwrap();
    assert_eq!(results.len(), 0);
}

#[test]
fn test_batch_operations() {
    let mut config = VectorDbConfig::default();
    config.dimension = 3; // Use 3D vectors for testing
    let db = VectorDB::new(config).unwrap();

    let vectors = vec![
        (
            vec![1.0, 0.0, 0.0],
            VectorMetadata::new(
                "test1".to_string(),
                "session1".to_string(),
                "text".to_string(),
                "content1".to_string(),
            ),
        ),
        (
            vec![0.0, 1.0, 0.0],
            VectorMetadata::new(
                "test2".to_string(),
                "session1".to_string(),
                "text".to_string(),
                "content2".to_string(),
            ),
        ),
        (
            vec![0.0, 0.0, 1.0],
            VectorMetadata::new(
                "test3".to_string(),
                "session1".to_string(),
                "text".to_string(),
                "content3".to_string(),
            ),
        ),
    ];

    let ids = db.add_vectors_batch(vectors).unwrap();
    assert_eq!(ids.len(), 3);
    assert_eq!(db.get_stats().total_vectors, 3);

    let results = db.search(&[1.0, 0.0, 0.0], 5).unwrap();
    // The search should find all 3 vectors since they are all similar to [1.0, 0.0, 0.0] to some degree
    assert!(!results.is_empty()); // At least the exact match should be found
}

// ---------------------------------------------------------------------------
// Search, secondary indexes, deferred HNSW, capacity
// ---------------------------------------------------------------------------

/// Deterministic pseudo-random vectors (no RNG dependency in tests).
fn pseudo_vectors(n: usize, dim: usize, seed: u64) -> Vec<Vec<f32>> {
    let step = |s: u64| {
        s.wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407)
    };
    let mut state = step(seed);
    (0..n)
        .map(|_| {
            (0..dim)
                .map(|_| {
                    state = step(state);
                    ((state >> 33) as f32 / (1u64 << 31) as f32) - 0.5
                })
                .collect()
        })
        .collect()
}

fn meta(id: &str, source: &str, content_type: &str) -> VectorMetadata {
    VectorMetadata::new(
        id.to_string(),
        format!("text {id}"),
        source.to_string(),
        content_type.to_string(),
    )
}

fn small_db(dim: usize) -> VectorDB {
    let mut config = VectorDbConfig::default();
    config.dimension = dim;
    VectorDB::new(config).unwrap()
}

fn naive_top_k(db_vectors: &[Vec<f32>], query: &[f32], k: usize) -> Vec<u32> {
    let mut scored: Vec<(u32, f32)> = db_vectors
        .iter()
        .enumerate()
        .map(|(i, v)| (i as u32, cosine_similarity(query, v)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().take(k).map(|(i, _)| i).collect()
}

fn ids_of(matches: Vec<SearchMatch>) -> Vec<u32> {
    matches.into_iter().map(|m| m.vector_id).collect()
}

#[test]
fn exact_search_returns_the_true_top_k_in_order() {
    let dim = 32;
    let db = small_db(dim);
    let vectors = pseudo_vectors(500, dim, 7);
    for (i, v) in vectors.iter().enumerate() {
        db.add_vector(v.clone(), meta(&format!("c{i}"), "s", "qa"))
            .unwrap();
    }
    let query = &pseudo_vectors(1, dim, 99)[0];
    assert_eq!(
        ids_of(db.search(query, 10).unwrap()),
        naive_top_k(&vectors, query, 10)
    );
    assert!(db.search(query, 0).unwrap().is_empty());
}

#[test]
fn filtered_search_finds_sparse_matches_and_respects_k() {
    let dim = 16;
    let db = small_db(dim);
    let vectors = pseudo_vectors(300, dim, 3);
    for (i, v) in vectors.iter().enumerate() {
        let ct = if i % 50 == 0 { "rare" } else { "common" };
        db.add_vector(v.clone(), meta(&format!("c{i}"), "s", ct))
            .unwrap();
    }
    let query = &vectors[1];
    let rare = db.search_by_content_type(query, 100, "rare").unwrap();
    assert_eq!(rare.len(), 6);
    assert!(rare.iter().all(|m| m.metadata.content_type == "rare"));
    assert!(rare.windows(2).all(|w| w[0].similarity >= w[1].similarity));

    let common = db.search_by_content_type(query, 5, "common").unwrap();
    assert_eq!(common.len(), 5);
    assert_eq!(common[0].vector_id, 1);
}

#[test]
fn secondary_indexes_track_adds_and_removes() {
    let db = small_db(3);
    let a = db
        .add_vector(vec![1.0, 0.0, 0.0], meta("u1", "s1", "UpdateContent"))
        .unwrap();
    let e = db
        .add_vector(
            vec![0.0, 1.0, 0.0],
            meta("entity:x", "s1", "EntityDescription"),
        )
        .unwrap();
    let dup = db
        .add_vector(vec![0.0, 0.0, 1.0], meta("u1", "s1", "UpdateContent"))
        .unwrap();
    db.add_vector(vec![1.0, 1.0, 0.0], meta("u2", "s2", "UpdateContent"))
        .unwrap();

    assert!(db.has_session_embeddings("s1"));
    assert!(!db.has_session_embeddings("nope"));
    assert_eq!(db.count_session_embeddings("s1"), 3);
    assert!(db.has_session_update_embeddings("s1"));
    assert!(db.has_update_embedding("u1"));
    assert_eq!(db.find_vector_id_by_content_id("u1"), Some(a));
    assert_eq!(db.find_vector_ids_by_content_id("u1"), vec![a, dup]);
    assert_eq!(
        db.get_vectorized_update_ids("s1"),
        vec!["u1".to_string(), "u1".to_string()]
    );

    // Removing the update vectors leaves only the entity: no update embeddings.
    assert_eq!(db.remove_by_content_id("u1"), 2);
    assert!(!db.has_update_embedding("u1"));
    assert_eq!(db.find_vector_id_by_content_id("u1"), None);
    assert!(db.has_session_embeddings("s1"));
    assert!(!db.has_session_update_embeddings("s1"));
    assert!(db.get_vectorized_update_ids("s1").is_empty());

    assert!(db.remove_vector(e).unwrap());
    assert!(!db.has_session_embeddings("s1"));
    assert_eq!(db.count_session_embeddings("s1"), 0);

    assert_eq!(db.remove_source("s2"), 1);
    assert!(db.is_empty());
    assert_eq!(db.get_stats().total_vectors, 0);

    db.add_vector(vec![1.0, 0.0, 0.0], meta("u3", "s3", "qa"))
        .unwrap();
    db.clear().unwrap();
    assert!(!db.has_session_embeddings("s3"));
    assert!(!db.has_update_embedding("u3"));
}

#[test]
fn source_search_matches_filtered_search_and_scores_only_that_source() {
    let dim = 16;
    let db = small_db(dim);
    for (i, v) in pseudo_vectors(400, dim, 11).into_iter().enumerate() {
        let source = format!("s{}", i % 4);
        db.add_vector(v, meta(&format!("c{i}"), &source, "qa"))
            .unwrap();
    }
    let query = &pseudo_vectors(1, dim, 5)[0];

    let via_index = ids_of(db.search_in_source(query, 7, "s1").unwrap());
    let via_filter = ids_of(
        db.search_with_filter(query, 7, |m| m.source == "s1")
            .unwrap(),
    );
    assert_eq!(via_index, via_filter);
    assert_eq!(via_index.len(), 7);

    let two = db
        .search_in_sources(query, 1000, &["s1", "s2", "s1"])
        .unwrap();
    assert_eq!(
        two.len(),
        200,
        "duplicate source must not duplicate results"
    );
    assert!(
        two.iter()
            .all(|m| m.metadata.source == "s1" || m.metadata.source == "s2")
    );

    let filtered = db
        .search_in_sources_with_filter(query, 1000, &["s3".to_string()], |m| m.id.ends_with('3'))
        .unwrap();
    assert!(!filtered.is_empty());
    assert!(
        filtered
            .iter()
            .all(|m| m.metadata.source == "s3" && m.metadata.id.ends_with('3'))
    );

    assert!(
        db.search_in_sources::<&str>(query, 5, &[])
            .unwrap()
            .is_empty()
    );
    assert!(db.search_in_source(query, 5, "missing").unwrap().is_empty());
}

#[test]
fn no_hnsw_graph_below_the_threshold() {
    let dim = 8;
    let mut config = VectorDbConfig::default();
    config.dimension = dim;
    config.exact_search_threshold = 1_000;
    let db = VectorDB::new(config).unwrap();
    for (i, v) in pseudo_vectors(1_000, dim, 1).into_iter().enumerate() {
        db.add_vector(v, meta(&format!("c{i}"), "s", "qa")).unwrap();
    }
    assert!(
        !db.get_stats().is_built,
        "graph must not exist at the threshold"
    );

    db.add_vector(vec![1.0; dim], meta("over", "s", "qa"))
        .unwrap();
    assert!(
        db.get_stats().is_built,
        "crossing the threshold builds the graph"
    );
}

#[test]
fn hnsw_after_threshold_has_good_recall_and_stays_live() {
    let dim = 24;
    let mut config = VectorDbConfig::default();
    config.dimension = dim;
    config.exact_search_threshold = 200;
    let db = VectorDB::new(config).unwrap();

    let vectors = pseudo_vectors(1_500, dim, 42);
    // Bulk load: one graph build at the end.
    let ids = db
        .add_vectors_bulk(
            vectors
                .iter()
                .enumerate()
                .map(|(i, v)| (v.clone(), meta(&format!("c{i}"), "s", "qa"))),
        )
        .unwrap();
    assert_eq!(ids.len(), vectors.len());
    assert!(db.get_stats().is_built);

    let queries = pseudo_vectors(20, dim, 777);
    let mut hits = 0;
    for q in &queries {
        let exact = naive_top_k(&vectors, q, 10);
        let approx = db
            .search_with_mode(q, 10, SearchMode::Balanced, Some(128))
            .unwrap();
        hits += approx
            .iter()
            .filter(|m| exact.contains(&m.vector_id))
            .count();
    }
    let recall = hits as f32 / (queries.len() * 10) as f32;
    assert!(recall >= 0.8, "recall@10 = {recall}");

    // Incremental inserts after the build are reachable through the graph.
    let probe = vec![0.3; dim];
    let probe_id = db
        .add_vector(probe.clone(), meta("probe", "s", "qa"))
        .unwrap();
    assert_eq!(db.search(&probe, 1).unwrap()[0].vector_id, probe_id);

    // Removing many nodes (including the entry point) keeps search working.
    assert_eq!(db.remove_vectors(&(0..200).collect::<Vec<u32>>()), 200);
    assert!(!db.search(&probe, 5).unwrap().is_empty());
}

#[test]
fn max_vectors_is_enforced() {
    let mut config = VectorDbConfig::default();
    config.dimension = 2;
    config.max_vectors = 3;
    let db = VectorDB::new(config).unwrap();
    for i in 0..3 {
        db.add_vector(vec![1.0, i as f32], meta(&format!("c{i}"), "s", "qa"))
            .unwrap();
    }
    let err = db
        .add_vector(vec![1.0, 9.0], meta("c9", "s", "qa"))
        .unwrap_err();
    assert!(err.to_string().contains("max_vectors"), "{err}");
    assert_eq!(db.len(), 3);

    db.remove_vector(0).unwrap();
    let ids = db
        .add_vectors_bulk((0..5).map(|i| (vec![0.0, i as f32], meta("b", "s", "qa"))))
        .unwrap();
    assert_eq!(ids.len(), 1, "bulk insert stops at the cap");
    assert_eq!(db.len(), 3);
}

#[test]
fn quantization_is_off_by_default() {
    let config = VectorDbConfig::default();
    assert!(!config.enable_quantization);
    let mut quantized = config.clone();
    quantized.dimension = 4;
    quantized.enable_quantization = true;
    let mut plain = config;
    plain.dimension = 4;

    let q = VectorDB::new(quantized).unwrap();
    let p = VectorDB::new(plain).unwrap();
    q.add_vector(vec![0.1; 4], meta("a", "s", "qa")).unwrap();
    p.add_vector(vec![0.1; 4], meta("a", "s", "qa")).unwrap();
    assert_eq!(
        q.get_stats().memory_usage_bytes - p.get_stats().memory_usage_bytes,
        4,
        "the option still stores one byte per dimension"
    );
}

/// Sanity benchmark, not run by default:
/// `cargo test -p post-cortex-embeddings --release --lib bench_ -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_load_and_search() {
    let dim = 256;
    let n = 20_000;
    let vectors = pseudo_vectors(n, dim, 1);
    let mut config = VectorDbConfig::default();
    config.dimension = dim;
    config.max_vectors = usize::MAX;
    let db = VectorDB::new(config).unwrap();

    let t = std::time::Instant::now();
    db.add_vectors_bulk(vectors.iter().enumerate().map(|(i, v)| {
        (
            v.clone(),
            meta(&format!("c{i}"), &format!("s{}", i % 100), "qa"),
        )
    }))
    .unwrap();
    println!("bulk load {n} x {dim}: {:?}", t.elapsed());

    let q = &pseudo_vectors(1, dim, 5)[0];
    let t = std::time::Instant::now();
    for _ in 0..100 {
        db.search_with_mode(q, 10, SearchMode::Exact, None).unwrap();
    }
    println!("exact search: {:?}/query", t.elapsed() / 100);
    let t = std::time::Instant::now();
    for _ in 0..100 {
        db.search(q, 10).unwrap();
    }
    println!("hnsw search: {:?}/query", t.elapsed() / 100);
    let t = std::time::Instant::now();
    for _ in 0..100 {
        db.search_in_source(q, 10, "s7").unwrap();
    }
    println!("source search (200 vectors): {:?}/query", t.elapsed() / 100);
}

/// On a DB past the exact-search threshold, a filter that keeps few of the
/// graph's candidates (here records among many nearer messages) still
/// returns k matches: the exact scan takes over.
#[test]
fn sparse_filter_on_hnsw_falls_back_to_the_exact_scan() {
    let dim = 24;
    let mut config = VectorDbConfig::default();
    config.dimension = dim;
    config.exact_search_threshold = 200;
    let db = VectorDB::new(config).unwrap();

    let query = vec![1.0; dim];
    // 1,000 messages right around the query, 20 records further away.
    let near = pseudo_vectors(1_000, dim, 7)
        .into_iter()
        .map(|v| v.iter().map(|x| 1.0 + 0.05 * x).collect::<Vec<f32>>());
    let far = pseudo_vectors(20, dim, 9);
    db.add_vectors_bulk(
        near.enumerate()
            .map(|(i, v)| (v, meta(&format!("m{i}"), "s", MESSAGE_CONTENT_TYPE)))
            .chain(
                far.into_iter()
                    .enumerate()
                    .map(|(i, v)| (v, meta(&format!("r{i}"), "s", "DecisionPoint"))),
            ),
    )
    .unwrap();
    assert!(db.get_stats().is_built);

    let records = db
        .search_with_filter(&query, 10, |m| m.content_type != MESSAGE_CONTENT_TYPE)
        .unwrap();
    assert_eq!(records.len(), 10);
    assert!(records.iter().all(|m| m.metadata.id.starts_with('r')));
}
