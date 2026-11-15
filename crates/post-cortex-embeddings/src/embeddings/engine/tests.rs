// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Records the slices it receives; each vector is the text's length.
struct RecordingBackend {
    seen: parking_lot::Mutex<Vec<Vec<String>>>,
}

impl EmbeddingBackend for RecordingBackend {
    fn embedding_dimension(&self) -> usize {
        1
    }
    fn batch_size(&self) -> usize {
        8
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.seen.lock().push(texts.to_vec());
        Ok(texts.iter().map(|t| vec![t.len() as f32]).collect())
    }
}

/// Sleeps `pause` per slice of one text and records how many slices ran
/// at once and in total.
struct SlowBackend {
    pause: Duration,
    running: AtomicUsize,
    most: AtomicUsize,
    slices: AtomicUsize,
}

impl SlowBackend {
    fn new(pause: Duration) -> Self {
        Self {
            pause,
            running: AtomicUsize::new(0),
            most: AtomicUsize::new(0),
            slices: AtomicUsize::new(0),
        }
    }
}

impl EmbeddingBackend for SlowBackend {
    fn embedding_dimension(&self) -> usize {
        1
    }
    fn batch_size(&self) -> usize {
        1
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        self.slices.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.pause);
        self.running.fetch_sub(1, Ordering::SeqCst);
        Ok(texts.iter().map(|_| vec![0.0]).collect())
    }
}

/// Returns one vector too few.
struct ShortBackend;

impl EmbeddingBackend for ShortBackend {
    fn embedding_dimension(&self) -> usize {
        2
    }
    fn batch_size(&self) -> usize {
        4
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().skip(1).map(|_| vec![0.0, 1.0]).collect())
    }
}

fn config(max_concurrent_ops: usize, operation_timeout_secs: u64) -> EmbeddingConfig {
    EmbeddingConfig {
        model_type: EmbeddingModelType::MiniLM,
        max_concurrent_ops,
        operation_timeout_secs,
    }
}

#[tokio::test]
async fn inputs_are_sliced_by_the_backend_batch_in_order_without_losing_any() {
    let backend = Arc::new(RecordingBackend {
        seen: parking_lot::Mutex::new(Vec::new()),
    });
    let engine = LocalEmbeddingEngine::with_backend(backend.clone(), config(2, 30));

    let texts: Vec<String> = (0..21).map(|i| "x".repeat(i)).collect();
    let embeddings = engine.encode_batch(texts.clone()).await.unwrap();

    let expected: Vec<Vec<f32>> = (0..21).map(|i| vec![i as f32]).collect();
    assert_eq!(embeddings, expected);
    let seen = backend.seen.lock();
    assert_eq!(seen.iter().map(Vec::len).collect::<Vec<_>>(), vec![8, 8, 5]);
    assert_eq!(seen.concat(), texts);
}

/// However many callers, no more batches run than the engine has slots.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_batches_are_bounded_by_the_slots() {
    let backend = Arc::new(SlowBackend::new(Duration::from_millis(30)));
    let engine = Arc::new(LocalEmbeddingEngine::with_backend(
        backend.clone(),
        config(2, 30),
    ));
    let calls: Vec<_> = (0..6)
        .map(|i| {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move { engine.encode_batch(vec![format!("t{i}")]).await })
        })
        .collect();
    for call in calls {
        call.await.unwrap().unwrap();
    }
    assert_eq!(backend.most.load(Ordering::SeqCst), 2);
}

/// A batch past its timeout fails with `Timeout`, and its remaining
/// slices never run.
#[tokio::test]
async fn a_batch_past_its_timeout_fails_and_stops() {
    let backend = Arc::new(SlowBackend::new(Duration::from_millis(400)));
    let engine = LocalEmbeddingEngine::with_backend(backend.clone(), config(1, 1));
    let texts: Vec<String> = (0..10).map(|i| i.to_string()).collect();

    let err = engine.encode_batch(texts).await.unwrap_err();
    assert!(matches!(err, Error::Timeout(1)), "{err}");
    // The slice running at the timeout finishes, then the work stops.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(backend.slices.load(Ordering::SeqCst) <= 4);
}

/// A caller that drops the call (its deadline) gives its slot back.
#[tokio::test]
async fn a_dropped_call_frees_its_slot() {
    let backend = Arc::new(SlowBackend::new(Duration::from_millis(100)));
    let engine = LocalEmbeddingEngine::with_backend(backend.clone(), config(1, 30));
    let texts: Vec<String> = (0..50).map(|i| i.to_string()).collect();
    let dropped =
        tokio::time::timeout(Duration::from_millis(150), engine.encode_batch(texts)).await;
    assert!(dropped.is_err());

    let next = tokio::time::timeout(
        Duration::from_secs(2),
        engine.encode_batch(vec!["after".to_string()]),
    )
    .await;
    assert_eq!(next.unwrap().unwrap(), vec![vec![0.0]]);
    assert!(backend.slices.load(Ordering::SeqCst) < 10);
}

#[tokio::test]
async fn a_backend_that_loses_vectors_is_an_error() {
    let engine = LocalEmbeddingEngine::with_backend(Arc::new(ShortBackend), config(1, 30));
    let err = engine
        .encode_batch(vec!["a".to_string(), "b".to_string()])
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Inference(_)), "{err}");
}

/// One engine behind an `Arc` serves several callers at once (memory
/// systems sharing a loaded model) and reports its model.
#[tokio::test]
async fn shared_engine_serves_concurrent_callers() {
    let engine = Arc::new(LocalEmbeddingEngine::with_backend(
        Arc::new(crate::test_support::HashBackend::new(64)),
        EmbeddingConfig {
            model_type: EmbeddingModelType::PotionCode,
            ..Default::default()
        },
    ));
    assert_eq!(engine.model_type(), EmbeddingModelType::PotionCode);
    let tasks: Vec<_> = (0..4)
        .map(|i| {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move { engine.encode_text(&format!("text {i}")).await })
        })
        .collect();
    for task in tasks {
        let vector = task.await.unwrap().unwrap();
        assert_eq!(vector.len(), engine.embedding_dimension());
    }
}
