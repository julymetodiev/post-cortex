// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::content_vectorizer::ContentKind;
use chrono::Utc;

fn result(session_id: Uuid, text: &str) -> SemanticSearchResult {
    SemanticSearchResult {
        validity: None,
        signals: None,
        document: None,
        confidence: None,
        content_id: Uuid::new_v4().to_string(),
        source: crate::content_vectorizer::HitSource::Session(session_id),
        content_type: ContentKind::UpdateContent,
        text_content: text.to_string(),
        similarity_score: 0.9,
        importance_score: 0.7,
        timestamp: Utc::now(),
        combined_score: 0.84,
        neighbour_of: None,
        author: None,
        title: None,
        derived_from: Vec::new(),
        full_chars: None,
    }
}

fn empty() -> Arc<[SemanticSearchResult]> {
    Arc::from(Vec::new())
}

#[test]
fn test_query_cache_creation() {
    let config = QueryCacheConfig::default();
    let cache = QueryCache::new(config);

    let stats = cache.get_stats();
    assert_eq!(stats.total_queries, 0);
    assert_eq!(stats.cache_hits, 0);
}

#[test]
fn test_cache_and_retrieve() {
    let cache = QueryCache::default();
    let sid = Uuid::new_v4();
    let results: Arc<[SemanticSearchResult]> = Arc::from(vec![result(sid, "hello")]);

    cache.insert(
        "test query".into(),
        Arc::clone(&results),
        12345,
        CacheScope::Global,
    );

    let hit = cache.get(12345, "test query").expect("hit");
    // Shared, not copied.
    assert!(Arc::ptr_eq(&hit, &results));
    let stats = cache.get_stats();
    assert_eq!(stats.cache_hits, 1);
    assert_eq!(stats.total_queries, 1);
}

#[test]
fn test_different_params_miss() {
    let cache = QueryCache::default();
    cache.insert("query".into(), empty(), 123, CacheScope::Global);
    assert!(cache.get(456, "query").is_none());
    // Same hash but different text (collision guard) misses too.
    assert!(cache.get(123, "other").is_none());
    assert_eq!(cache.get_stats().cache_misses, 2);
}

#[test]
fn test_cache_expiration() {
    let config = QueryCacheConfig {
        ttl_minutes: 0, // Immediate expiration
        ..Default::default()
    };

    let cache = QueryCache::new(config);
    cache.insert("test".into(), empty(), 123, CacheScope::Global);

    // Should not find expired entry, and the lookup removes it.
    assert!(cache.get(123, "test").is_none());
    assert!(cache.is_empty());
    assert_eq!(cache.get_stats().expired_removed, 1);
}

#[test]
fn test_invalidate_session_drops_covering_scopes() {
    let cache = QueryCache::default();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();

    cache.insert("q".into(), empty(), 1, CacheScope::Session(a));
    cache.insert("q".into(), empty(), 2, CacheScope::Session(b));
    cache.insert("q".into(), empty(), 3, CacheScope::Global);
    cache.insert(
        "q".into(),
        empty(),
        4,
        CacheScope::Sessions(Arc::from(vec![a, b])),
    );
    cache.insert(
        "q".into(),
        empty(),
        5,
        CacheScope::Sessions(Arc::from(vec![b])),
    );

    cache.invalidate_session(a);

    assert!(cache.get(1, "q").is_none(), "own session entry");
    assert!(
        cache.get(3, "q").is_none(),
        "global entry must not go stale"
    );
    assert!(cache.get(4, "q").is_none(), "multi-session entry with a");
    assert!(cache.get(2, "q").is_some());
    assert!(cache.get(5, "q").is_some());
    assert_eq!(cache.len(), 2);
}

#[test]
fn test_capacity_is_enforced() {
    let cache = QueryCache::new(QueryCacheConfig {
        max_cache_size: 20,
        ..Default::default()
    });
    for i in 0..100u64 {
        cache.insert(format!("q{i}"), empty(), i, CacheScope::Global);
        assert!(cache.len() <= 20);
    }
    assert!(cache.get_stats().evicted_entries > 0);
    // The newest entry always survives its own insert.
    assert!(cache.get(99, "q99").is_some());
}

#[test]
fn test_replacing_key_does_not_evict() {
    let cache = QueryCache::new(QueryCacheConfig {
        max_cache_size: 2,
        ..Default::default()
    });
    cache.insert("a".into(), empty(), 1, CacheScope::Global);
    cache.insert("b".into(), empty(), 2, CacheScope::Global);
    cache.insert("b".into(), empty(), 2, CacheScope::Global);
    assert_eq!(cache.len(), 2);
    assert_eq!(cache.get_stats().evicted_entries, 0);
}

#[test]
fn test_memory_estimate_is_incremental() {
    let cache = QueryCache::default();
    let sid = Uuid::new_v4();
    let results: Arc<[SemanticSearchResult]> = Arc::from(vec![result(sid, "some text")]);

    cache.insert(
        "q".into(),
        Arc::clone(&results),
        1,
        CacheScope::Session(sid),
    );
    let one = cache.get_stats().estimated_memory_bytes;
    assert!(one > 0);

    cache.insert(
        "q".into(),
        Arc::clone(&results),
        1,
        CacheScope::Session(sid),
    );
    assert_eq!(cache.get_stats().estimated_memory_bytes, one, "replace");

    cache.insert("q".into(), results, 2, CacheScope::Session(sid));
    assert_eq!(cache.get_stats().estimated_memory_bytes, 2 * one);

    cache.invalidate_session(sid);
    assert_eq!(cache.get_stats().estimated_memory_bytes, 0);
    assert_eq!(cache.get_stats().current_cache_size, 0);
}
