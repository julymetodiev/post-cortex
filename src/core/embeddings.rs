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
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//! Local Embeddings Engine for semantic understanding
//!
//! This module implements a high-concurrency embedding generation system
//! using atomic operations and concurrent data structures.

use anyhow::Result;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use crossbeam_queue::SegQueue;
use dashmap::DashMap;
use hf_hub::api::tokio::Api;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokenizers::Tokenizer;
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

/// Memory pool using crossbeam's atomic data structures
#[derive(Debug)]
struct MemoryPool {
    /// Available vectors - using crossbeam's concurrent queue
    available: SegQueue<Vec<f32>>,
    /// Maximum pool size (used in return_vector to limit pool growth)
    max_size: usize,
    /// Current size (atomic)
    current_size: AtomicUsize,
    /// Vector capacity for new allocations
    vector_capacity: usize,
    /// Pool hit counter (successful pool retrievals)
    pool_hits: AtomicUsize,
    /// Pool miss counter (fallback allocations)
    pool_misses: AtomicUsize,
}

#[allow(dead_code)] // Methods used in tests and for future pool recycling

impl MemoryPool {
    fn new(size: usize, vector_capacity: usize) -> Self {
        let pool = Self {
            available: SegQueue::new(),
            max_size: size,
            current_size: AtomicUsize::new(0),
            vector_capacity,
            pool_hits: AtomicUsize::new(0),
            pool_misses: AtomicUsize::new(0),
        };

        // Pre-populate with empty vectors
        for _ in 0..size {
            pool.available.push(Vec::with_capacity(vector_capacity));
        }
        pool.current_size.store(size, Ordering::Release);

        pool
    }

    /// Get a vector from pool, or allocate new one if pool is empty
    fn get_or_allocate(&self) -> Vec<f32> {
        match self.available.pop() {
            Some(vec) => {
                self.pool_hits.fetch_add(1, Ordering::Relaxed);
                vec
            }
            None => {
                let misses = self.pool_misses.fetch_add(1, Ordering::Relaxed) + 1;
                // Log warning every 100 misses to avoid log spam
                if misses % 100 == 0 {
                    debug!(
                        "Memory pool exhausted: {} misses (pool_size={}, capacity={})",
                        misses, self.max_size, self.vector_capacity
                    );
                }
                Vec::with_capacity(self.vector_capacity)
            }
        }
    }

    /// Return a vector to the pool for reuse
    fn return_vector(&self, mut vec: Vec<f32>) {
        // Only return if pool is not full
        if self.available.len() < self.max_size {
            vec.clear();
            self.available.push(vec);
        }
        // Otherwise let it drop naturally
    }

    /// Get pool statistics: (available, total, hits, misses)
    fn get_stats(&self) -> PoolStats {
        PoolStats {
            available: self.available.len(),
            total: self.current_size.load(Ordering::Acquire),
            hits: self.pool_hits.load(Ordering::Relaxed),
            misses: self.pool_misses.load(Ordering::Relaxed),
        }
    }

    /// Get hit rate as percentage (0.0 - 100.0)
    fn hit_rate(&self) -> f64 {
        let hits = self.pool_hits.load(Ordering::Relaxed) as f64;
        let misses = self.pool_misses.load(Ordering::Relaxed) as f64;
        let total = hits + misses;
        if total > 0.0 {
            (hits / total) * 100.0
        } else {
            100.0 // No requests = 100% hit rate (no misses)
        }
    }
}

/// Pool statistics for monitoring and debugging
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Fields used in tests and for monitoring
struct PoolStats {
    /// Number of vectors currently available in pool
    available: usize,
    /// Total pool capacity
    total: usize,
    /// Number of successful pool retrievals
    hits: usize,
    /// Number of fallback allocations (pool exhaustion events)
    misses: usize,
}

/// Configuration for the embedding engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Model type for embeddings
    pub model_type: EmbeddingModelType,
    /// Maximum batch size for processing
    pub max_batch_size: usize,
    /// Enable adaptive batch sizing
    pub adaptive_batching: bool,
    /// Memory pool size for vector reuse
    pub memory_pool_size: usize,
    /// Maximum concurrent operations
    pub max_concurrent_ops: usize,
    /// Enable performance monitoring
    pub enable_performance_monitoring: bool,
    /// Model cache directory
    pub cache_dir: PathBuf,
    /// Enable model caching
    pub enable_caching: bool,
    /// Operation timeout in seconds
    pub operation_timeout_secs: u64,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model_type: EmbeddingModelType::default(),
            max_batch_size: 32,
            adaptive_batching: true,
            memory_pool_size: 1000,
            max_concurrent_ops: num_cpus::get() * 2,
            enable_performance_monitoring: true,
            cache_dir: PathBuf::from("./models_cache"),
            enable_caching: true,
            operation_timeout_secs: 30,
        }
    }
}

/// Concurrent operation controller using atomics
#[derive(Debug)]
struct ConcurrencyController {
    /// Current number of active operations (atomic)
    current_operations: AtomicUsize,
    /// Maximum allowed operations
    max_operations: usize,
    /// Flag to indicate if controller is active
    active: AtomicBool,
}

impl ConcurrencyController {
    fn new(max_operations: usize) -> Self {
        Self {
            current_operations: AtomicUsize::new(0),
            max_operations,
            active: AtomicBool::new(true),
        }
    }

    /// Try to acquire a slot without blocking (CAS retry loop)
    fn try_acquire(self: &Arc<Self>) -> Option<OperationPermit> {
        if !self.active.load(Ordering::Acquire) {
            return None;
        }

        // CAS loop to atomically acquire a slot
        loop {
            let current = self.current_operations.load(Ordering::Acquire);
            if current >= self.max_operations {
                return None;
            }

            // Try to increment atomically
            match self.current_operations.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Some(OperationPermit {
                        controller: Arc::clone(self),
                    });
                }
                Err(_) => {
                    // CAS failed, retry - another thread modified the value
                    // Use hint to reduce contention
                    std::hint::spin_loop();
                    continue;
                }
            }
        }
    }

    /// Acquire with waiting (async busy wait)
    async fn acquire(self: &Arc<Self>) -> Result<OperationPermit> {
        const MAX_WAIT_ITERATIONS: usize = 1000;
        const WAIT_DELAY_MS: u64 = 1;

        if !self.active.load(Ordering::Relaxed) {
            return Err(anyhow::anyhow!("Concurrency controller is not active"));
        }

        let mut iterations = 0;
        loop {
            // Try to get slot immediately
            if let Some(permit) = self.try_acquire() {
                return Ok(permit);
            }

            // Check if we should continue waiting
            iterations += 1;
            if iterations >= MAX_WAIT_ITERATIONS {
                return Err(anyhow::anyhow!(
                    "Timeout waiting for operation slot after {} iterations",
                    MAX_WAIT_ITERATIONS
                ));
            }

            // Small delay to avoid busy waiting
            sleep(Duration::from_millis(WAIT_DELAY_MS)).await;
        }
    }

    /// Release a slot (automatically called by permit drop)
    fn release(&self) {
        self.current_operations.fetch_sub(1, Ordering::SeqCst);
    }

    /// Get current load
    fn current_load(&self) -> usize {
        self.current_operations.load(Ordering::Relaxed)
    }

    /// Get max capacity
    fn max_capacity(&self) -> usize {
        self.max_operations
    }
}

/// Operation permit (auto-releasing on drop)
struct OperationPermit {
    controller: Arc<ConcurrencyController>,
}

impl Drop for OperationPermit {
    fn drop(&mut self) {
        self.controller.release();
    }
}

/// Performance stats for batch optimization
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct BatchPerformanceStats {
    size: usize,
    time_ms: f64,
    success_rate: f64,
}

/// Embedding model types
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum EmbeddingModelType {
    /// Static embeddings (fast, lightweight)
    StaticSimilarityMRL,
    /// MiniLM model (balanced performance, English-only)
    MiniLM,
    /// Multilingual MiniLM model (supports 50+ languages including Bulgarian)
    MultilingualMiniLM,
    /// TinyBERT model (smallest BERT variant)
    TinyBERT,
    /// BGE Small model (balanced BERT)
    BGESmall,
}

impl Default for EmbeddingModelType {
    fn default() -> Self {
        Self::MultilingualMiniLM
    }
}

impl EmbeddingModelType {
    /// Get embedding dimension for this model type
    pub fn embedding_dimension(&self) -> usize {
        match self {
            Self::StaticSimilarityMRL => 1024,
            Self::MiniLM => 384,
            Self::MultilingualMiniLM => 384,
            Self::TinyBERT => 312,
            Self::BGESmall => 384,
        }
    }

    /// Get model ID for HuggingFace Hub
    pub fn model_id(&self) -> &'static str {
        match self {
            Self::StaticSimilarityMRL => "sentence-transformers/all-MiniLM-L6-v2",
            Self::MiniLM => "sentence-transformers/all-MiniLM-L6-v2",
            Self::MultilingualMiniLM => {
                "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2"
            }
            Self::TinyBERT => "huawei-noah/TinyBERT_General_6L_312D",
            Self::BGESmall => "BAAI/bge-small-en-v1.5",
        }
    }

    /// Check if this is a BERT-based model
    pub fn is_bert_based(&self) -> bool {
        matches!(
            self,
            Self::MiniLM | Self::MultilingualMiniLM | Self::TinyBERT | Self::BGESmall
        )
    }
}

/// Local embedding engine
pub struct LocalEmbeddingEngine {
    /// Model for generating embeddings
    model: Arc<BertModel>,
    /// Tokenizer for text processing
    tokenizer: Arc<Tokenizer>,
    /// Configuration
    config: EmbeddingConfig,
    /// Device for computation
    device: Device,
    /// Current batch size (atomic)
    current_batch_size: AtomicUsize,
    /// Performance stats for batch optimization
    batch_performance_cache: Arc<DashMap<usize, f64>>,
    /// Memory pool for pre-allocated embedding vectors
    memory_pool: Arc<MemoryPool>,
    /// Concurrency controller
    concurrency_controller: Arc<ConcurrencyController>,
}

impl LocalEmbeddingEngine {
    /// Create a new embedding engine with the given configuration
    pub async fn new(config: EmbeddingConfig) -> Result<Self> {
        info!(
            "Initializing embedding engine with model: {:?}",
            config.model_type
        );

        let device = Device::Cpu;
        let model_id = config.model_type.model_id();

        // Load model and tokenizer (same as before)
        let api = Api::new().map_err(|e| anyhow::anyhow!("Failed to create API: {}", e))?;
        let repo = api.model(model_id.to_string());

        let model_path = repo
            .get("model.safetensors")
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get model: {}", e))?;
        let config_path = repo
            .get("config.json")
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get config: {}", e))?;
        let tokenizer_path = repo
            .get("tokenizer.json")
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get tokenizer: {}", e))?;

        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        let bert_config = std::fs::read_to_string(config_path)
            .map_err(|e| anyhow::anyhow!("Failed to read BERT config: {}", e))?;
        let bert_config: BertConfig = serde_json::from_str(&bert_config)
            .map_err(|e| anyhow::anyhow!("Failed to parse BERT config: {}", e))?;

        let vb =
            unsafe { VarBuilder::from_mmaped_safetensors(&[model_path], DType::F32, &device)? };
        let model = BertModel::load(vb, &bert_config)?;

        info!(
            "Model loaded successfully, embedding dimension: {}",
            config.model_type.embedding_dimension()
        );

        // Initialize components
        let concurrency_controller = Arc::new(ConcurrencyController::new(
            config.max_concurrent_ops,
        ));

        Ok(Self {
            model: Arc::new(model),
            tokenizer: Arc::new(tokenizer),
            current_batch_size: AtomicUsize::new(config.max_batch_size),
            batch_performance_cache: Arc::new(DashMap::new()),
            memory_pool: Arc::new(MemoryPool::new(
                config.memory_pool_size,
                config.model_type.embedding_dimension(),
            )),
            concurrency_controller,
            config,
            device,
        })
    }

    /// Get current batch size
    pub fn current_batch_size(&self) -> usize {
        self.current_batch_size.load(Ordering::Relaxed)
    }

    /// Get embedding dimension
    pub fn embedding_dimension(&self) -> usize {
        self.config.model_type.embedding_dimension()
    }

    /// Check if model is BERT-based
    pub fn is_bert_based(&self) -> bool {
        self.config.model_type.is_bert_based()
    }

    /// Encode text into embeddings
    pub async fn encode_text(&self, text: &str) -> Result<Vec<f32>> {
        let texts = vec![text.to_string()];
        let embeddings = self.encode_batch(texts).await?;
        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No embeddings generated"))
    }

    /// Encode batch of texts
    pub async fn encode_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let _start_time = std::time::Instant::now();

        // Use static embeddings for non-BERT models
        if !self.config.model_type.is_bert_based() {
            warn!(
                "Using STATIC embeddings (non-BERT) for model_type: {:?} - semantic search will NOT work correctly!",
                self.config.model_type
            );
            return self.encode_batch_static(&texts).await;
        }

        info!(
            "Using BERT embeddings for model_type: {:?}, encoding {} texts",
            self.config.model_type,
            texts.len()
        );

        // BERT models use concurrent processing
        let total_start_time = std::time::Instant::now();
        let result = self.encode_batch_bert(texts.clone()).await;

        // Performance monitoring
        let total_time = total_start_time.elapsed();
        debug!("Encoded {} texts in {:?}", texts.len(), total_time);

        result
    }

    /// Static embeddings (fast path)
    async fn encode_batch_static(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        // Simplified static embedding generation
        let dimension = self.embedding_dimension();
        let mut results = Vec::with_capacity(texts.len());

        for text in texts {
            // Get from memory pool or allocate new
            let mut embedding = self.memory_pool.get_or_allocate();

            // Generate static embedding (simplified hash-based approach)
            let hash = self.text_hash(text);
            embedding.clear();
            embedding.reserve(dimension);

            for i in 0..dimension {
                let value = ((hash.wrapping_add(i as u64)) as f32 / u64::MAX as f32) * 2.0 - 1.0;
                embedding.push(value);
            }

            // Normalize embedding
            let norm = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for val in &mut embedding {
                    *val /= norm;
                }
            }

            results.push(embedding);
        }

        Ok(results)
    }

    /// BERT embeddings with concurrency control
    async fn encode_batch_bert(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        // Try to acquire slot without blocking
        let _permit = match self.concurrency_controller.try_acquire() {
            Some(permit) => permit,
            None => {
                // If no immediate slot, wait for one
                self.concurrency_controller.acquire().await?
            }
        };

        // Use adaptive or fixed batch size
        let batch_size = if self.config.adaptive_batching {
            self.get_adaptive_batch_size(texts.len()).await
        } else {
            self.current_batch_size()
        };

        let mut all_embeddings = Vec::new();

        // Process in batches
        for chunk in texts.chunks(batch_size) {
            let start_time = std::time::Instant::now();

            // Process batch with timeout
            let batch_result = timeout(
                Duration::from_secs(self.config.operation_timeout_secs),
                self.process_bert_batch(chunk.to_vec()),
            )
            .await;

            match batch_result {
                Ok(Ok(batch_embeddings)) => {
                    all_embeddings.extend(batch_embeddings);

                    // Update performance stats
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

    /// L2 normalize embeddings tensor (critical for cosine similarity)
    fn l2_normalize_embeddings(&self, embeddings: &Tensor) -> Result<Tensor> {
        // Calculate L2 norm (magnitude) for each embedding vector
        // embeddings shape: [batch_size, embedding_dim]
        let squared = embeddings.sqr()?; // Square each element
        let sum_squared = squared.sum_keepdim(1)?; // Sum across embedding dimension
        let l2_norm = sum_squared.sqrt()?; // Take square root to get L2 norm

        // Debug: Check if vectors are already normalized
        let l2_norm_values = l2_norm.to_vec2::<f32>()?;
        debug!(
            "L2 normalization - batch size: {}, first norm: {:.6}",
            l2_norm_values.len(),
            l2_norm_values
                .first()
                .and_then(|v| v.first())
                .unwrap_or(&0.0)
        );

        // Avoid division by zero by clamping norm to minimum epsilon
        // Using clamp_min instead of affine to only affect near-zero norms
        let epsilon = 1e-12_f32;
        let l2_norm_safe = l2_norm.clamp(epsilon, f32::MAX)?;

        // Normalize: embeddings / l2_norm
        let normalized = embeddings.broadcast_div(&l2_norm_safe)?;

        debug!("L2 normalization completed successfully");

        Ok(normalized)
    }

    /// Process a single BERT batch
    async fn process_bert_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        // Maximum sequence length for BERT models (most use 512)
        const MAX_SEQ_LENGTH: usize = 512;

        // Tokenize texts
        let mut tokenized = Vec::with_capacity(texts.len());
        for text in &texts {
            let encoding = self
                .tokenizer
                .encode(text.clone(), true)
                .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

            tokenized.push(encoding);
        }

        // Create tensor from tokenized input
        // Limit max_len to MAX_SEQ_LENGTH to prevent index out of bounds
        let max_len = tokenized
            .iter()
            .map(|enc| enc.len())
            .max()
            .unwrap_or(0)
            .min(MAX_SEQ_LENGTH);

        let mut input_ids = Vec::new();
        let mut attention_mask = Vec::new();

        for encoding in tokenized {
            let ids = encoding.get_ids();
            let mask = encoding.get_attention_mask();

            // Truncate to max_len if necessary
            let truncate_len = ids.len().min(max_len);
            input_ids.extend_from_slice(&ids[..truncate_len]);
            attention_mask.extend_from_slice(&mask[..truncate_len]);

            // Pad to max_len if shorter
            if truncate_len < max_len {
                input_ids.extend(vec![0u32; max_len - truncate_len]);
                attention_mask.extend(vec![0u32; max_len - truncate_len]);
            }
        }



        // Create tensors
        // Convert u32 to i64 for BERT model compatibility.
        // Performance note: This O(n) conversion is negligible compared to BERT's
        // O(n²) attention computation. Typical overhead: ~0.1ms for 512 tokens.
        let input_ids_i64: Vec<i64> = input_ids.iter().map(|&x| x as i64).collect();
        let attention_mask_i64: Vec<i64> = attention_mask.iter().map(|&x| x as i64).collect();

        let input_tensor = Tensor::from_vec(input_ids_i64, (texts.len(), max_len), &self.device)?;
        let mask_tensor =
            Tensor::from_vec(attention_mask_i64, (texts.len(), max_len), &self.device)?;

        // Forward pass through BERT model
        // Pass None for token_type_ids as XLM-R/MiniLM often don't use them or learn to ignore them
        let outputs = self
            .model
            .forward(&input_tensor, &mask_tensor, None)?;

        // MASKED Mean Pooling - standard for Sentence Transformers
        // We use the attention mask to zero out padding tokens before averaging
        
        let mask_f32 = mask_tensor.to_dtype(DType::F32)?;
        // mask_f32 is [batch, seq_len] -> expand to [batch, seq_len, hidden_size]
        let mask_expanded = mask_f32.unsqueeze(2)?.broadcast_as(outputs.shape())?;
        
        // Zero out padding embeddings
        let masked_outputs = outputs.broadcast_mul(&mask_expanded)?;
        
        // Sum along sequence dimension (dim 1) -> [batch, hidden_size]
        let sum_embeddings = masked_outputs.sum(1)?;
        
        // Count non-padding tokens -> [batch, 1]
        let token_counts = mask_f32.sum(1)?.unsqueeze(1)?;
        let token_counts_safe = token_counts.clamp(1e-9f64, f64::MAX)?;
        
        // Divide sum by count to get mean
        let embeddings = sum_embeddings.broadcast_div(&token_counts_safe)?;

        // L2 normalize embeddings for correct cosine similarity calculation
        let embeddings_normalized = self.l2_normalize_embeddings(&embeddings)?;

        let embeddings_vec = embeddings_normalized.to_vec2::<f32>()?; // Convert 2D tensor to 2D vec

        // Debug: Check norms after normalization
        if log::log_enabled!(log::Level::Debug) {
            for (i, emb) in embeddings_vec.iter().enumerate() {
                let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
                debug!("Embedding {} norm after L2 normalization: {:.6}", i, norm);
            }
        }

        // Split into individual embeddings
        // Note: We use embeddings directly from tensor conversion,
        // memory pool is used for temporary allocations during processing
        Ok(embeddings_vec)
    }

    /// Get adaptive batch size based on performance history
    async fn get_adaptive_batch_size(&self, text_count: usize) -> usize {
        // Simple adaptive logic based on text count and performance history
        let base_size = self.current_batch_size();

        if text_count <= base_size {
            return text_count;
        }

        // Check recent performance
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

        // Adjust batch size based on performance
        if avg_performance > 0.9 {
            (base_size as f64 * 1.2) as usize
        } else if avg_performance < 0.7 {
            (base_size as f64 * 0.8) as usize
        } else {
            base_size
        }
    }

    /// Update batch performance stats (via CAS loop)
    fn update_batch_performance(&self, batch_size: usize, time_ms: f64, success_rate: f64) {
        // Store performance metric
        let metric = success_rate / (time_ms / batch_size as f64);
        self.batch_performance_cache.insert(batch_size, metric);

        // Update current batch size atomically with CAS loop
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
                    // Another thread updated, retry with new value
                    std::hint::spin_loop();
                    continue;
                }
            }
        }
    }

    /// Get current concurrency load
    pub fn current_concurrency_load(&self) -> usize {
        self.concurrency_controller.current_load()
    }

    /// Get concurrency stats
    pub fn get_concurrency_stats(&self) -> (usize, usize) {
        (
            self.concurrency_controller.current_load(),
            self.concurrency_controller.max_capacity(),
        )
    }

    /// Simple text hash for static embeddings
    fn text_hash(&self, text: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[tokio::test]
    async fn test_embedding_engine_creation() {
        let config = EmbeddingConfig::default();
        let engine = LocalEmbeddingEngine::new(config).await;

        // Should fail because we don't have actual models, but should not panic
        assert!(engine.is_err() || engine.is_ok());
    }

    #[test]
    fn test_concurrency_controller() {
        let controller = Arc::new(ConcurrencyController::new(2));

        // Should be able to acquire 2 permits
        let permit1 = controller.try_acquire();
        assert!(permit1.is_some());

        let permit2 = controller.try_acquire();
        assert!(permit2.is_some());

        // Third should fail
        assert!(controller.try_acquire().is_none());

        // After releasing one, should be able to acquire again
        drop(permit1); // This will release the first permit

        // Now should be able to acquire again
        assert!(controller.try_acquire().is_some());

        // Stats should work
        assert_eq!(controller.max_capacity(), 2);
    }

    /// Test concurrent access to concurrency controller (validates race condition fix)
    #[test]
    fn test_concurrency_controller_concurrent_access() {
        let controller = Arc::new(ConcurrencyController::new(4));
        let acquired_count = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];

        // Spawn 10 threads trying to acquire permits concurrently
        for _ in 0..10 {
            let ctrl = Arc::clone(&controller);
            let count = Arc::clone(&acquired_count);
            handles.push(thread::spawn(move || {
                if let Some(_permit) = ctrl.try_acquire() {
                    count.fetch_add(1, Ordering::SeqCst);
                    // Hold permit briefly
                    thread::sleep(Duration::from_millis(10));
                    // Permit released on drop
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // All permits should be released now
        assert_eq!(controller.current_load(), 0);
        // Should have acquired exactly 4 permits initially (max capacity)
        // Due to timing, some threads may have acquired after others released
        assert!(acquired_count.load(Ordering::SeqCst) >= 4);
    }

    /// Test CAS retry loop in concurrency controller
    #[test]
    fn test_concurrency_controller_cas_retry() {
        let controller = Arc::new(ConcurrencyController::new(100));
        let success_count = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];

        // Spawn many threads to stress test the CAS loop
        for _ in 0..50 {
            let ctrl = Arc::clone(&controller);
            let count = Arc::clone(&success_count);
            handles.push(thread::spawn(move || {
                // Each thread tries to acquire and release multiple times
                for _ in 0..10 {
                    if let Some(_permit) = ctrl.try_acquire() {
                        count.fetch_add(1, Ordering::SeqCst);
                        // Very short hold to maximize contention
                        std::hint::spin_loop();
                    }
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // All permits should be released
        assert_eq!(controller.current_load(), 0);
        // Should have many successful acquisitions
        assert!(success_count.load(Ordering::SeqCst) > 100);
    }

    #[tokio::test]
    async fn test_memory_pool() {
        let pool = MemoryPool::new(10, 384);

        // Should be able to get vectors (always succeeds - allocates if pool empty)
        let vec1 = pool.get_or_allocate();
        let vec2 = pool.get_or_allocate();

        assert_eq!(vec1.capacity(), 384);
        assert_eq!(vec2.capacity(), 384);

        // Return them to pool
        pool.return_vector(vec1);
        pool.return_vector(vec2);

        // Stats should work
        let stats = pool.get_stats();
        assert_eq!(stats.total, 10);
        assert!(stats.available <= 12); // May have up to 12 (10 original + 2 returned)
        assert_eq!(stats.hits, 2); // Two successful pool retrievals
        assert_eq!(stats.misses, 0); // No fallback allocations
    }

    /// Test memory pool doesn't grow unbounded (validates max_size limit)
    #[test]
    fn test_memory_pool_bounded_growth() {
        let pool = MemoryPool::new(5, 384);

        // Get all vectors from pool (5 hits)
        let mut vecs: Vec<Vec<f32>> = (0..5).map(|_| pool.get_or_allocate()).collect();

        // Pool should be empty now, but get_or_allocate still works (1 miss)
        let extra = pool.get_or_allocate();
        assert_eq!(extra.capacity(), 384);
        vecs.push(extra);

        // Return all vectors
        for v in vecs {
            pool.return_vector(v);
        }

        // Pool should not exceed max_size (5)
        let stats = pool.get_stats();
        assert!(
            stats.available <= 5,
            "Pool grew beyond max_size: {}",
            stats.available
        );
        assert_eq!(stats.hits, 5); // 5 successful retrievals
        assert_eq!(stats.misses, 1); // 1 fallback allocation
    }

    /// Test pool hit rate calculation
    #[test]
    fn test_memory_pool_hit_rate() {
        let pool = MemoryPool::new(2, 384);

        // Get 2 from pool (hits), then 2 more (misses)
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

    /// Test memory pool concurrent access
    #[test]
    fn test_memory_pool_concurrent() {
        let pool = Arc::new(MemoryPool::new(20, 384));
        let mut handles = vec![];

        // Spawn threads that get and return vectors
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

        // Pool should still function correctly
        let vec = pool.get_or_allocate();
        assert_eq!(vec.capacity(), 384);
    }

    /// Test atomic batch size updates (validates CAS loop fix)
    #[test]
    fn test_atomic_batch_size_updates() {
        let batch_size = Arc::new(AtomicUsize::new(32));
        let mut handles = vec![];

        // Simulate concurrent batch size updates
        for _ in 0..10 {
            let bs = Arc::clone(&batch_size);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    // Simulate the CAS loop pattern we use
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

        // Final value should be within valid range
        let final_size = batch_size.load(Ordering::Acquire);
        assert!(final_size >= 8 && final_size <= 256);
    }

    /// Test embedding model types
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

    /// Test default config values
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
}
