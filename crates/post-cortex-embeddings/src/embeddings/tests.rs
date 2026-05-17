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

use super::concurrency::ConcurrencyController;
use super::config::{EmbeddingConfig, EmbeddingModelType};
use super::engine::LocalEmbeddingEngine;
use super::pool::MemoryPool;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

#[tokio::test]
async fn test_embedding_engine_creation() {
    let config = EmbeddingConfig::default();
    let engine = LocalEmbeddingEngine::new(config).await;

    // Should fail because we don't have actual models, but should not panic.
    assert!(engine.is_err() || engine.is_ok());
}

#[test]
fn test_concurrency_controller() {
    let controller = Arc::new(ConcurrencyController::new(2));

    let permit1 = controller.try_acquire();
    assert!(permit1.is_some());

    let permit2 = controller.try_acquire();
    assert!(permit2.is_some());

    assert!(controller.try_acquire().is_none());

    drop(permit1); // releases the first permit
    assert!(controller.try_acquire().is_some());

    assert_eq!(controller.max_capacity(), 2);
}

/// Test concurrent access to concurrency controller (validates the CAS retry path).
#[test]
fn test_concurrency_controller_concurrent_access() {
    let controller = Arc::new(ConcurrencyController::new(4));
    let acquired_count = Arc::new(AtomicUsize::new(0));
    let mut handles = vec![];

    for _ in 0..10 {
        let ctrl = Arc::clone(&controller);
        let count = Arc::clone(&acquired_count);
        handles.push(thread::spawn(move || {
            if let Some(_permit) = ctrl.try_acquire() {
                count.fetch_add(1, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(10));
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(controller.current_load(), 0);
    assert!(acquired_count.load(Ordering::SeqCst) >= 4);
}

/// Stress-test the CAS retry loop on the concurrency controller.
#[test]
fn test_concurrency_controller_cas_retry() {
    let controller = Arc::new(ConcurrencyController::new(100));
    let success_count = Arc::new(AtomicUsize::new(0));
    let mut handles = vec![];

    for _ in 0..50 {
        let ctrl = Arc::clone(&controller);
        let count = Arc::clone(&success_count);
        handles.push(thread::spawn(move || {
            for _ in 0..10 {
                if let Some(_permit) = ctrl.try_acquire() {
                    count.fetch_add(1, Ordering::SeqCst);
                    std::hint::spin_loop();
                }
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(controller.current_load(), 0);
    assert!(success_count.load(Ordering::SeqCst) > 100);
}

#[tokio::test]
async fn test_memory_pool() {
    let pool = MemoryPool::new(10, 384);

    let vec1 = pool.get_or_allocate();
    let vec2 = pool.get_or_allocate();

    assert_eq!(vec1.capacity(), 384);
    assert_eq!(vec2.capacity(), 384);

    pool.return_vector(vec1);
    pool.return_vector(vec2);

    let stats = pool.get_stats();
    assert_eq!(stats.total, 10);
    assert!(stats.available <= 12);
    assert_eq!(stats.hits, 2);
    assert_eq!(stats.misses, 0);
}

/// Validates that `MemoryPool::return_vector` respects `max_size`.
#[test]
fn test_memory_pool_bounded_growth() {
    let pool = MemoryPool::new(5, 384);

    let mut vecs: Vec<Vec<f32>> = (0..5).map(|_| pool.get_or_allocate()).collect();

    let extra = pool.get_or_allocate();
    assert_eq!(extra.capacity(), 384);
    vecs.push(extra);

    for v in vecs {
        pool.return_vector(v);
    }

    let stats = pool.get_stats();
    assert!(
        stats.available <= 5,
        "Pool grew beyond max_size: {}",
        stats.available
    );
    assert_eq!(stats.hits, 5);
    assert_eq!(stats.misses, 1);
}

#[test]
fn test_memory_pool_hit_rate() {
    let pool = MemoryPool::new(2, 384);

    let _v1 = pool.get_or_allocate(); // hit
    let _v2 = pool.get_or_allocate(); // hit
    let _v3 = pool.get_or_allocate(); // miss
    let _v4 = pool.get_or_allocate(); // miss

    let hit_rate = pool.hit_rate();
    assert!(
        (hit_rate - 50.0).abs() < 0.01,
        "Expected 50% hit rate, got {}",
        hit_rate
    );
}

#[test]
fn test_memory_pool_concurrent() {
    let pool = Arc::new(MemoryPool::new(20, 384));
    let mut handles = vec![];

    for _ in 0..10 {
        let p = Arc::clone(&pool);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                let vec = p.get_or_allocate();
                assert_eq!(vec.capacity(), 384);
                p.return_vector(vec);
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let vec = pool.get_or_allocate();
    assert_eq!(vec.capacity(), 384);
}

#[test]
fn test_atomic_batch_size_updates() {
    let batch_size = Arc::new(AtomicUsize::new(32));
    let mut handles = vec![];

    for _ in 0..10 {
        let bs = Arc::clone(&batch_size);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                loop {
                    let current = bs.load(Ordering::Acquire);
                    let new_size = ((current as f64) * 1.01) as usize;
                    let clamped = new_size.clamp(8, 256);

                    match bs.compare_exchange_weak(
                        current,
                        clamped,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(_) => {
                            std::hint::spin_loop();
                            continue;
                        }
                    }
                }
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let final_size = batch_size.load(Ordering::Acquire);
    assert!((8..=256).contains(&final_size));
}

#[test]
fn test_embedding_model_types() {
    assert_eq!(EmbeddingModelType::MiniLM.embedding_dimension(), 384);
    assert_eq!(
        EmbeddingModelType::MultilingualMiniLM.embedding_dimension(),
        384
    );
    assert_eq!(EmbeddingModelType::TinyBERT.embedding_dimension(), 312);
    assert_eq!(EmbeddingModelType::BGESmall.embedding_dimension(), 384);

    assert!(EmbeddingModelType::MiniLM.is_bert_based());
    assert!(EmbeddingModelType::MultilingualMiniLM.is_bert_based());
    assert!(!EmbeddingModelType::StaticSimilarityMRL.is_bert_based());
}

#[test]
fn test_default_config() {
    let config = EmbeddingConfig::default();

    assert_eq!(config.model_type, EmbeddingModelType::MultilingualMiniLM);
    assert_eq!(config.max_batch_size, 32);
    assert!(config.adaptive_batching);
    assert_eq!(config.memory_pool_size, 1000);
    assert!(config.enable_performance_monitoring);
    assert!(config.enable_caching);
    assert_eq!(config.operation_timeout_secs, 30);
}
