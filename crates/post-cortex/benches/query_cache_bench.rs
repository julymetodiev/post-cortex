use chrono::Utc;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use uuid::Uuid;

use post_cortex_memory::content_vectorizer::{ContentType, SemanticSearchResult};
use post_cortex_memory::query_cache::{QueryCache, QueryCacheConfig};

fn create_test_results() -> Vec<SemanticSearchResult> {
    vec![
        SemanticSearchResult {
            content_id: Uuid::new_v4().to_string(),
            session_id: Uuid::new_v4(),
            content_type: ContentType::UpdateContent,
            text_content: "Test result 1".to_string(),
            similarity_score: 0.9,
            importance_score: 0.5,
            timestamp: Utc::now(),
            combined_score: 0.78,
        },
        SemanticSearchResult {
            content_id: Uuid::new_v4().to_string(),
            session_id: Uuid::new_v4(),
            content_type: ContentType::UpdateContent,
            text_content: "Test result 2".to_string(),
            similarity_score: 0.8,
            importance_score: 0.5,
            timestamp: Utc::now(),
            combined_score: 0.71,
        },
    ]
}

fn bench_cache_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_cache_search");

    for &load in &[16usize, 64, 256] {
        group.bench_with_input(
            BenchmarkId::new("search_hit_rate", load),
            &load,
            |b, &load| {
                b.iter_batched(
                    || {
                        let cache = QueryCache::new(QueryCacheConfig::default());
                        let results = create_test_results();
                        for i in 0..load {
                            let query_text = format!("query {}", i);
                            let query_vector =
                                vec![i as f32 * 0.1, i as f32 * 0.2, i as f32 * 0.3];
                            let _ = cache.cache_results(
                                query_text,
                                query_vector,
                                results.clone(),
                                i as u64,
                                None,
                            );
                        }
                        cache
                    },
                    |cache| {
                        for j in 0..load {
                            let query_text = format!("query {}", j % load);
                            let query_vector =
                                vec![j as f32 * 0.1, j as f32 * 0.2, j as f32 * 0.3];
                            let params_hash = (j % load) as u64;
                            let hit = cache.search(&query_text, &query_vector, params_hash);
                            black_box(hit);
                        }
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_cache_insert(c: &mut Criterion) {
    c.bench_function("query_cache_insert_100", |b| {
        b.iter_batched(
            || {
                (
                    QueryCache::new(QueryCacheConfig::default()),
                    create_test_results(),
                )
            },
            |(cache, results)| {
                for i in 0..100usize {
                    let query_text = format!("query {}", i);
                    let query_vector = vec![i as f32 * 0.1, i as f32 * 0.2, i as f32 * 0.3];
                    let _ = cache.cache_results(
                        query_text,
                        query_vector,
                        results.clone(),
                        i as u64,
                        None,
                    );
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, bench_cache_search, bench_cache_insert);
criterion_main!(benches);
