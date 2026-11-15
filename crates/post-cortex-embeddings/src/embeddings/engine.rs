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
//! Selects a backend based on [`EmbeddingConfig::model_type`] and runs it
//! under one policy for every backend: the batch runs on the blocking pool,
//! at most [`EmbeddingConfig::max_concurrent_ops`] batches at once (one
//! semaphore), in slices of the backend's [`EmbeddingBackend::batch_size`],
//! within [`EmbeddingConfig::operation_timeout_secs`]. A caller that stops
//! waiting (its deadline dropped the call, or the timeout passed) leaves
//! the queue at once, and a running batch stops before its next slice.

use crate::blocking;
use crate::error::{Error, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tracing::{debug, info};

use crate::embeddings::backend::EmbeddingBackend;
#[cfg(feature = "bert")]
use crate::embeddings::backends::BertBackend;
#[cfg(feature = "model2vec")]
use crate::embeddings::backends::Model2VecBackend;
use crate::embeddings::config::{EmbeddingConfig, EmbeddingModelType};

/// Local embedding engine.
pub struct LocalEmbeddingEngine {
    backend: Arc<dyn EmbeddingBackend>,
    config: EmbeddingConfig,
    /// One permit per batch running on the blocking pool.
    slots: Arc<Semaphore>,
}

/// The backend of `model_type`, or an error naming the feature it needs.
async fn load_backend(model_type: EmbeddingModelType) -> Result<Arc<dyn EmbeddingBackend>> {
    if model_type.is_model2vec() {
        #[cfg(feature = "model2vec")]
        {
            return Ok(Arc::new(Model2VecBackend::load(model_type).await?));
        }
        #[cfg(not(feature = "model2vec"))]
        {
            return Err(Error::FeatureDisabled {
                model: model_type.to_string(),
                feature: "model2vec",
            });
        }
    }
    #[cfg(feature = "bert")]
    {
        Ok(Arc::new(BertBackend::load(model_type).await?))
    }
    #[cfg(not(feature = "bert"))]
    {
        Err(Error::FeatureDisabled {
            model: model_type.to_string(),
            feature: "bert",
        })
    }
}

impl LocalEmbeddingEngine {
    /// Create a new embedding engine with the given configuration.
    pub async fn new(config: EmbeddingConfig) -> Result<Self> {
        info!(
            "Initializing embedding engine with model: {}",
            config.model_type
        );

        let backend = load_backend(config.model_type).await?;
        Ok(Self::with_backend(backend, config))
    }

    /// An engine over `backend` instead of the backend of
    /// `config.model_type`; concurrency and timeout come from `config`.
    pub fn with_backend(backend: Arc<dyn EmbeddingBackend>, config: EmbeddingConfig) -> Self {
        Self {
            backend,
            slots: Arc::new(Semaphore::new(config.max_concurrent_ops.max(1))),
            config,
        }
    }

    /// The model this engine runs.
    pub fn model_type(&self) -> EmbeddingModelType {
        self.config.model_type
    }

    /// Get embedding dimension.
    pub fn embedding_dimension(&self) -> usize {
        self.backend.embedding_dimension()
    }

    /// Encode a single text into an embedding.
    pub async fn encode_text(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.encode_batch(vec![text.to_string()]).await?;
        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| Error::NoEmbedding(self.config.model_type.to_string()))
    }

    /// Encode a batch of texts into embeddings, one per text in input
    /// order.
    ///
    /// # Errors
    /// `Timeout` when waiting for a slot and running took longer than the
    /// configured timeout; the backend's error; `Dimension` or `Inference`
    /// when the backend returned vectors of another length or count.
    pub async fn encode_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let secs = self.config.operation_timeout_secs;
        let count = texts.len();
        let started = std::time::Instant::now();
        let embeddings = timeout(Duration::from_secs(secs), self.run(texts))
            .await
            .map_err(|_| Error::Timeout(secs))??;
        debug!("Encoded {} texts in {:?}", count, started.elapsed());
        Ok(embeddings)
    }

    /// `texts` on the blocking pool, once a slot is free, a slice of the
    /// backend's batch size at a time.
    async fn run(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let backend = Arc::clone(&self.backend);
        blocking::run(&self.slots, move |abandoned| {
            let dimension = backend.embedding_dimension();
            let mut out = Vec::with_capacity(texts.len());
            for slice in texts.chunks(backend.batch_size().max(1)) {
                abandoned.check()?;
                let vectors = backend.embed(slice)?;
                if vectors.len() != slice.len() {
                    return Err(Error::Inference(format!(
                        "{} vectors for {} texts",
                        vectors.len(),
                        slice.len()
                    )));
                }
                if let Some(v) = vectors.iter().find(|v| v.len() != dimension) {
                    return Err(Error::Dimension {
                        expected: dimension,
                        actual: v.len(),
                    });
                }
                out.extend(vectors);
            }
            Ok(out)
        })
        .await
    }
}

#[cfg(test)]
mod tests;
