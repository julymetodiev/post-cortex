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

//! `LocalEmbeddingEngine` — the public entry point.
//!
//! Selects a backend based on [`EmbeddingConfig::model_type`] and wraps the BERT
//! path with concurrency control, timeouts, and adaptive batch sizing. The static
//! path is dispatched directly.

use anyhow::Result;
use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use super::backend::EmbeddingBackend;
#[cfg(feature = "bert")]
use super::backends::BertBackend;
#[cfg(feature = "model2vec")]
use super::backends::Model2VecBackend;
use super::backends::StaticHashBackend;
use super::concurrency::ConcurrencyController;
use super::config::{EmbeddingConfig, EmbeddingModelType};
use super::pool::MemoryPool;

/// Local embedding engine.
pub struct LocalEmbeddingEngine {
    backend: Arc<dyn EmbeddingBackend>,
    config: EmbeddingConfig,
    /// Current batch size (atomic — adapts based on runtime performance).
    current_batch_size: AtomicUsize,
    /// Performance metric per observed batch size — drives adaptive batching.
    batch_performance_cache: Arc<DashMap<usize, f64>>,
    /// Concurrency gate for the BERT path.
    concurrency_controller: Arc<ConcurrencyController>,
}

impl LocalEmbeddingEngine {
    /// Create a new embedding engine with the given configuration.
    pub async fn new(config: EmbeddingConfig) -> Result<Self> {
        info!(
            "Initializing embedding engine with model: {:?}",
            config.model_type
        );

        let dimension = config.model_type.embedding_dimension();
        let backend: Arc<dyn EmbeddingBackend> = if config.model_type.is_model2vec() {
            #[cfg(feature = "model2vec")]
            {
                Arc::new(Model2VecBackend::load(config.model_type).await?)
            }
            #[cfg(not(feature = "model2vec"))]
            {
                return Err(anyhow::anyhow!(
                    "Model type {:?} requires the `model2vec` feature, which is disabled. \
                     Rebuild post-cortex-embeddings with `--features model2vec` or pick a \
                     different EmbeddingModelType.",
                    config.model_type
                ));
            }
        } else if config.model_type.is_bert_based() {
            #[cfg(feature = "bert")]
            {
                Arc::new(BertBackend::load(config.model_type).await?)
            }
            #[cfg(not(feature = "bert"))]
            {
                return Err(anyhow::anyhow!(
                    "Model type {:?} requires the `bert` feature, which is disabled. \
                     Rebuild post-cortex-embeddings with `--features bert` or pick a \
                     different EmbeddingModelType.",
                    config.model_type
                ));
            }
        } else {
            let pool = Arc::new(MemoryPool::new(config.memory_pool_size, dimension));
            Arc::new(StaticHashBackend::new(dimension, pool))
        };

        let concurrency_controller =
            Arc::new(ConcurrencyController::new(config.max_concurrent_ops));

        Ok(Self {
            backend,
            current_batch_size: AtomicUsize::new(config.max_batch_size),
            batch_performance_cache: Arc::new(DashMap::new()),
            concurrency_controller,
            config,
        })
    }

    /// Get current batch size.
    pub fn current_batch_size(&self) -> usize {
        self.current_batch_size.load(Ordering::Relaxed)
    }

    /// Get embedding dimension.
    pub fn embedding_dimension(&self) -> usize {
        self.backend.embedding_dimension()
    }

    /// Check if the active backend is BERT-based.
    pub fn is_bert_based(&self) -> bool {
        self.backend.is_bert_based()
    }

    /// Encode a single text into an embedding.
    pub async fn encode_text(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.encode_batch(vec![text.to_string()]).await?;
        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No embeddings generated"))
    }

    /// Encode a batch of texts into embeddings.
    pub async fn encode_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Non-BERT path: dispatch directly — no concurrency gating
        // required because both Model2Vec and the hash fallback are
        // ms-cheap. The hash fallback is the only case that does *not*
        // produce real semantic embeddings, so the warning is scoped to
        // that variant; Model2Vec is a legitimate static-embedding
        // backend and silently uses the same direct path.
        if !self.backend.is_bert_based() {
            if matches!(self.config.model_type, EmbeddingModelType::StaticSimilarityMRL) {
                warn!(
                    "Using StaticHashBackend for model_type {:?} — semantic search will NOT \
                     work correctly! Pick PotionMultilingual (default) or a BERT variant.",
                    self.config.model_type
                );
            }
            return self.backend.process_batch(texts).await;
        }

        info!(
            "Using BERT embeddings for model_type: {:?}, encoding {} texts",
            self.config.model_type,
            texts.len()
        );

        let total_start_time = std::time::Instant::now();
        let result = self.encode_batch_with_controls(texts.clone()).await;

        let total_time = total_start_time.elapsed();
        debug!("Encoded {} texts in {:?}", texts.len(), total_time);

        result
    }

    /// BERT path with concurrency, timeout, and adaptive batching.
    async fn encode_batch_with_controls(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let _permit = match self.concurrency_controller.try_acquire() {
            Some(permit) => permit,
            None => self.concurrency_controller.acquire().await?,
        };

        let batch_size = if self.config.adaptive_batching {
            self.get_adaptive_batch_size(texts.len()).await
        } else {
            self.current_batch_size()
        };

        let mut all_embeddings = Vec::new();

        for chunk in texts.chunks(batch_size) {
            let start_time = std::time::Instant::now();

            let batch_result = timeout(
                Duration::from_secs(self.config.operation_timeout_secs),
                self.backend.process_batch(chunk.to_vec()),
            )
            .await;

            match batch_result {
                Ok(Ok(batch_embeddings)) => {
                    all_embeddings.extend(batch_embeddings);
                    let time_ms = start_time.elapsed().as_millis() as f64;
                    self.update_batch_performance(chunk.len(), time_ms, 1.0);
                }
                Ok(Err(e)) => {
                    error!("Batch processing failed: {}", e);
                    self.update_batch_performance(
                        chunk.len(),
                        start_time.elapsed().as_millis() as f64,
                        0.0,
                    );
                    return Err(e);
                }
                Err(_) => {
                    error!("Batch processing timed out");
                    return Err(anyhow::anyhow!(
                        "Batch processing timed out after {} seconds",
                        self.config.operation_timeout_secs
                    ));
                }
            }
        }

        Ok(all_embeddings)
    }

    /// Get adaptive batch size based on recent performance history.
    async fn get_adaptive_batch_size(&self, text_count: usize) -> usize {
        let base_size = self.current_batch_size();

        if text_count <= base_size {
            return text_count;
        }

        let recent_performance: Vec<f64> = self
            .batch_performance_cache
            .iter()
            .take(10)
            .map(|entry| *entry.value())
            .collect();

        let avg_performance = if recent_performance.is_empty() {
            0.8 // Default success rate
        } else {
            recent_performance.iter().sum::<f64>() / recent_performance.len() as f64
        };

        if avg_performance > 0.9 {
            (base_size as f64 * 1.2) as usize
        } else if avg_performance < 0.7 {
            (base_size as f64 * 0.8) as usize
        } else {
            base_size
        }
    }

    /// Update batch performance stats (atomic CAS loop on current_batch_size).
    fn update_batch_performance(&self, batch_size: usize, time_ms: f64, success_rate: f64) {
        let metric = success_rate / (time_ms / batch_size as f64);
        self.batch_performance_cache.insert(batch_size, metric);

        loop {
            let current = self.current_batch_size.load(Ordering::Acquire);

            let new_size = if success_rate > 0.9 && time_ms < 1000.0 {
                (current as f64 * 1.1) as usize
            } else if success_rate < 0.7 || time_ms > 2000.0 {
                (current as f64 * 0.9) as usize
            } else {
                return; // No change needed
            };

            let clamped = new_size.clamp(8, 256);

            match self.current_batch_size.compare_exchange_weak(
                current,
                clamped,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(_) => {
                    std::hint::spin_loop();
                    continue;
                }
            }
        }
    }

    /// Get current concurrency load.
    pub fn current_concurrency_load(&self) -> usize {
        self.concurrency_controller.current_load()
    }

    /// Get concurrency stats: `(current_load, max_capacity)`.
    pub fn get_concurrency_stats(&self) -> (usize, usize) {
        (
            self.concurrency_controller.current_load(),
            self.concurrency_controller.max_capacity(),
        )
    }
}
