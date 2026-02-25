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

//! Lock-free Named Entity Recognition (NER) Engine using DistilBERT
//!
//! This module provides fast, accurate entity extraction for technical text
//! using DistilBERT-base-NER via Candle. Optimized for low latency and
//! lock-free concurrent access.

#[cfg(feature = "embeddings")]
use anyhow::Result;
#[cfg(feature = "embeddings")]
use candle_core::{DType, Device, Tensor};
#[cfg(feature = "embeddings")]
use candle_nn::VarBuilder;
#[cfg(feature = "embeddings")]
use candle_transformers::models::distilbert::{Config as DistilBertConfig, DistilBertModel};
#[cfg(feature = "embeddings")]
use dashmap::DashMap;
#[cfg(feature = "embeddings")]
use hf_hub::api::tokio::Api;
#[cfg(feature = "embeddings")]
use std::sync::Arc;
#[cfg(feature = "embeddings")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "embeddings")]
use tokenizers::Tokenizer;
#[cfg(feature = "embeddings")]
use tracing::{debug, info};

#[cfg(feature = "embeddings")]
/// Entity types recognized by the NER model
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntityType {
    /// Person names
    Person,
    /// Organizations
    Organization,
    /// Locations
    Location,
    /// Miscellaneous (technical terms, tools, etc.)
    Miscellaneous,
}

#[cfg(feature = "embeddings")]
impl EntityType {
    /// Convert BIO tag to entity type
    fn from_bio_tag(tag: &str) -> Option<Self> {
        match tag {
            t if t.contains("PER") => Some(Self::Person),
            t if t.contains("ORG") => Some(Self::Organization),
            t if t.contains("LOC") => Some(Self::Location),
            t if t.contains("MISC") => Some(Self::Miscellaneous),
            _ => None,
        }
    }
}

#[cfg(feature = "embeddings")]
/// Recognized entity with position and confidence
#[derive(Debug, Clone)]
pub struct RecognizedEntity {
    /// Entity text
    pub text: String,
    /// Entity type
    pub entity_type: EntityType,
    /// Confidence score (0.0-1.0)
    pub confidence: f32,
    /// Start position in original text
    pub start: usize,
    /// End position in original text
    pub end: usize,
}

#[cfg(feature = "embeddings")]
/// Maximum cache entries before eviction
const NER_CACHE_MAX_SIZE: usize = 1000;

#[cfg(feature = "embeddings")]
/// Lock-free NER engine using DistilBERT
pub struct NEREngine {
    /// DistilBERT model (loaded lazily)
    model: Option<Arc<DistilBertModel>>,
    /// Classification layer weights (loaded lazily)
    classifier_weights: Option<Arc<Tensor>>,
    /// Classification layer bias (loaded lazily)
    classifier_bias: Option<Arc<Tensor>>,
    /// Tokenizer (loaded lazily)
    tokenizer: Option<Arc<Tokenizer>>,
    /// Device (CPU or CUDA)
    device: Device,
    /// Model loaded flag
    is_loaded: Arc<AtomicBool>,
    /// Cache for recent extractions (full text -> entities to avoid hash collisions)
    cache: Arc<DashMap<String, Vec<RecognizedEntity>>>,
    /// BIO tag labels
    labels: Arc<Vec<String>>,
}

#[cfg(feature = "embeddings")]
impl NEREngine {
    /// Create new NER engine (lazy loading)
    pub fn new() -> Self {
        Self {
            model: None,
            classifier_weights: None,
            classifier_bias: None,
            tokenizer: None,
            device: Device::Cpu,
            is_loaded: Arc::new(AtomicBool::new(false)),
            cache: Arc::new(DashMap::new()),
            labels: Arc::new(vec![
                "O".to_string(),
                "B-PER".to_string(),
                "I-PER".to_string(),
                "B-ORG".to_string(),
                "I-ORG".to_string(),
                "B-LOC".to_string(),
                "I-LOC".to_string(),
                "B-MISC".to_string(),
                "I-MISC".to_string(),
            ]),
        }
    }

    /// Load DistilBERT-NER model from HuggingFace Hub
    pub async fn load_model(&mut self) -> Result<()> {
        // Use Acquire ordering to synchronize with Release store below
        // This ensures we see all writes to model/tokenizer if is_loaded is true
        if self.is_loaded.load(Ordering::Acquire) {
            debug!("NER model already loaded");
            return Ok(());
        }

        info!("Loading DistilBERT-NER model from HuggingFace Hub...");

        let api = Api::new()?;
        let repo = api.model("dslim/distilbert-NER".to_string());

        // Download model files
        let config_path = repo.get("config.json").await?;
        let weights_path = repo.get("model.safetensors").await?;
        let tokenizer_path = repo.get("tokenizer.json").await?;

        info!("Downloaded model files, loading into memory...");

        // Load config
        let config_content = std::fs::read_to_string(&config_path)?;
        let config: DistilBertConfig = serde_json::from_str(&config_content)?;

        // Load tokenizer
        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        // Load model weights
        let device = Device::Cpu; // Use CPU for maximum compatibility


        // Create VarBuilder for loading model and classifier weights
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(std::slice::from_ref(&weights_path), DType::F32, &device)? };

        // Load DistilBERT base model with "distilbert" prefix (like DistilBertForMaskedLM does)
        let model = DistilBertModel::load(vb.pp("distilbert"), &config)?;

        // Load classification head using VarBuilder (at root level, no prefix!)
        // This is the KEY fix - use vb.clone() not vb.pp(), just like in DistilBertForMaskedLM
        debug!("Loading classifier weights with VarBuilder...");
        let classifier_vb = vb.clone(); // Root level VarBuilder

        // Load weights: [num_labels=9, hidden_dim=768]
        let classifier_weights = classifier_vb.get((9, config.dim), "classifier.weight")?;
        let classifier_bias = classifier_vb.get(9, "classifier.bias")?;

        debug!("Classifier weights shape: {:?}, bias shape: {:?}",
               classifier_weights.shape(), classifier_bias.shape());

        // Update engine
        self.model = Some(Arc::new(model));
        self.classifier_weights = Some(Arc::new(classifier_weights));
        self.classifier_bias = Some(Arc::new(classifier_bias));
        self.tokenizer = Some(Arc::new(tokenizer));
        self.device = device;
        self.is_loaded.store(true, Ordering::Release);

        info!("DistilBERT-NER model loaded successfully");
        Ok(())
    }

    /// Extract entities from text (lock-free)
    pub fn extract_entities(&self, text: &str) -> Result<Vec<RecognizedEntity>> {
        // Check if model is loaded
        let model = self.model.as_ref().ok_or_else(|| anyhow::anyhow!("NER model not loaded"))?;
        let tokenizer = self.tokenizer.as_ref().ok_or_else(|| anyhow::anyhow!("Tokenizer not loaded"))?;
        let classifier_weights = self.classifier_weights.as_ref().ok_or_else(|| anyhow::anyhow!("Classifier weights not loaded"))?;
        let classifier_bias = self.classifier_bias.as_ref().ok_or_else(|| anyhow::anyhow!("Classifier bias not loaded"))?;

        // Check cache (use full text as key to avoid hash collisions)
        if let Some(cached) = self.cache.get(text) {
            debug!("NER cache hit for text: {}...", &text[..text.len().min(50)]);
            return Ok(cached.clone());
        }

        // Clone tokenizer and disable padding/truncation (like BERT example)
        let mut tokenizer_clone = (**tokenizer).clone();
        tokenizer_clone.with_padding(None).with_truncation(None)
            .map_err(|e| anyhow::anyhow!("Failed to configure tokenizer: {}", e))?;

        // Tokenize with special tokens ([CLS] and [SEP]) - CRITICAL for BERT models!
        let encoding = tokenizer_clone.encode(text, true)
            .map_err(|e| anyhow::anyhow!("Tokenization error: {}", e))?;

        let tokens = encoding.get_ids().to_vec();
        let token_count = tokens.len();

        if token_count == 0 {
            return Ok(vec![]);
        }

        // Convert to tensors (following Candle BERT example pattern)
        let token_ids = Tensor::new(&tokens[..], &self.device)?.unsqueeze(0)?;

        // Get attention mask from tokenizer encoding and INVERT it!
        // DistilBERT expects: 0 = valid token, 1 = padding (opposite of standard BERT mask!)
        let attention_mask_vec = encoding.get_attention_mask().to_vec();
        let inverted_mask: Vec<u32> = attention_mask_vec.iter().map(|&x| 1 - x).collect();
        let attention_mask = Tensor::new(&inverted_mask[..], &self.device)?.unsqueeze(0)?;

        // Run model inference
        let hidden_states = model.forward(&token_ids, &attention_mask)?;

        // Apply classification head
        // hidden_states shape: [batch=1, seq_len, hidden_dim=768]
        // weights shape: [num_labels=9, hidden_dim=768]
        // We need to compute: hidden_states @ weights.T + bias

        // Flatten batch dimension for matmul: [batch * seq_len, hidden_dim]
        let batch_size = hidden_states.dim(0)?;
        let seq_len = hidden_states.dim(1)?;
        let hidden_dim = hidden_states.dim(2)?;

        let hidden_flat = hidden_states.reshape((batch_size * seq_len, hidden_dim))?;

        // Matmul: [batch*seq_len, hidden_dim] @ [hidden_dim, num_labels]
        let logits_flat = hidden_flat.matmul(&classifier_weights.t()?)?;
        let logits_flat = logits_flat.broadcast_add(classifier_bias)?;

        // Reshape back: [batch, seq_len, num_labels]
        let logits = logits_flat.reshape((batch_size, seq_len, 9))?;

        // Get predictions (argmax) and confidence scores (softmax)
        let predictions = logits.argmax(2)?;
        let pred_ids = predictions.to_vec2::<u32>()?;

        // Calculate softmax for confidence scores
        let softmax = candle_nn::ops::softmax(&logits, 2)?;
        let confidences = softmax.to_vec3::<f32>()?;

        // Decode entities using BIO tagging
        let mut entities = self.decode_bio_tags(&pred_ids[0], &confidences[0], &encoding, text)?;

        // Merge consecutive entities of same type (fixes subword tokenization issues)
        entities = self.merge_consecutive_entities(entities, text);

        // Filter low-confidence and very short entities
        entities.retain(|e| e.confidence >= 0.7 && e.text.len() >= 2);

        // Cache results with bounded size (simple eviction when full)
        if self.cache.len() >= NER_CACHE_MAX_SIZE {
            // Clear half the cache when limit reached (simple but lock-free)
            let keys_to_remove: Vec<String> = self.cache
                .iter()
                .take(NER_CACHE_MAX_SIZE / 2)
                .map(|entry| entry.key().clone())
                .collect();
            for key in keys_to_remove {
                self.cache.remove(&key);
            }
            debug!("NER cache evicted {} entries", NER_CACHE_MAX_SIZE / 2);
        }
        self.cache.insert(text.to_string(), entities.clone());

        Ok(entities)
    }

    /// Decode BIO tags to entities
    fn decode_bio_tags(
        &self,
        predictions: &[u32],
        confidences: &[Vec<f32>],
        encoding: &tokenizers::Encoding,
        original_text: &str,
    ) -> Result<Vec<RecognizedEntity>> {
        let mut entities = Vec::new();
        let mut current_entity: Option<(String, EntityType, f32, usize, usize)> = None;

        for (idx, &pred_id) in predictions.iter().enumerate() {
            // Skip special tokens ([CLS], [SEP], [PAD])
            let offsets = encoding.get_offsets();
            if idx >= offsets.len() {
                continue; // Safety: skip if index out of bounds
            }
            let (char_start, char_end) = offsets[idx];
            if char_start == char_end {
                // Special token - skip it
                continue;
            }

            // Bounds check for label index to prevent panic
            let pred_idx = pred_id as usize;
            let label = match self.labels.get(pred_idx) {
                Some(l) => l,
                None => {
                    debug!("Unexpected prediction index {}, skipping token", pred_idx);
                    continue;
                }
            };

            // Bounds check for confidence scores
            let confidence = confidences
                .get(idx)
                .and_then(|row| row.get(pred_idx))
                .copied()
                .unwrap_or(0.0);

            if label.starts_with("B-") {
                // Begin new entity
                if let Some((text, etype, conf, start, end)) = current_entity.take() {
                    entities.push(RecognizedEntity {
                        text,
                        entity_type: etype,
                        confidence: conf,
                        start,
                        end,
                    });
                }

                if let Some(entity_type) = EntityType::from_bio_tag(label) {
                    if let Some(text) = Self::safe_slice(original_text, char_start, char_end) {
                        current_entity = Some((
                            text,
                            entity_type,
                            confidence,
                            char_start,
                            char_end,
                        ));
                    }
                }
            } else if label.starts_with("I-") {
                // Continue current entity
                if let Some((ref mut text, ref etype, ref mut conf, start, ref mut end)) = current_entity
                    && let Some(entity_type) = EntityType::from_bio_tag(label)
                        && entity_type == *etype {
                            if let Some(new_text) = Self::safe_slice(original_text, start, char_end) {
                                *text = new_text;
                                *end = char_end;
                                // Update confidence to minimum across all tokens (conservative)
                                *conf = conf.min(confidence);
                            }
                        }
            } else {
                // "O" tag - outside entity
                if let Some((text, etype, conf, start, end)) = current_entity.take() {
                    entities.push(RecognizedEntity {
                        text,
                        entity_type: etype,
                        confidence: conf,
                        start,
                        end,
                    });
                }
            }
        }

        // Push last entity if exists
        if let Some((text, etype, conf, start, end)) = current_entity {
            entities.push(RecognizedEntity {
                text,
                entity_type: etype,
                confidence: conf,
                start,
                end,
            });
        }

        Ok(entities)
    }

    /// Merge consecutive entities of the same type (fixes subword splitting)
    fn merge_consecutive_entities(
        &self,
        entities: Vec<RecognizedEntity>,
        original_text: &str,
    ) -> Vec<RecognizedEntity> {
        if entities.is_empty() {
            return entities;
        }

        let mut merged = Vec::new();
        let mut current: Option<RecognizedEntity> = None;

        for entity in entities {
            match current.take() {
                None => {
                    current = Some(entity);
                }
                Some(prev) => {
                    // Merge if same type and adjacent (end == start)
                    if prev.entity_type == entity.entity_type && prev.end == entity.start {
                        // Use safe slicing to handle UTF-8 boundaries
                        let merged_text = Self::safe_slice(original_text, prev.start, entity.end)
                            .unwrap_or_else(|| format!("{}{}", prev.text, entity.text));
                        current = Some(RecognizedEntity {
                            text: merged_text,
                            entity_type: prev.entity_type,
                            confidence: prev.confidence.min(entity.confidence),
                            start: prev.start,
                            end: entity.end,
                        });
                    } else {
                        // Different type or not adjacent - push previous and keep current
                        merged.push(prev);
                        current = Some(entity);
                    }
                }
            }
        }

        // Push last entity
        if let Some(entity) = current {
            merged.push(entity);
        }

        merged
    }

    /// Safe string slicing that handles UTF-8 boundaries
    /// Returns None if the slice boundaries are invalid
    fn safe_slice(text: &str, start: usize, end: usize) -> Option<String> {
        if start > end || end > text.len() {
            return None;
        }
        // Check if boundaries are valid UTF-8 char boundaries
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            // Find nearest valid boundaries
            let safe_start = text.char_indices()
                .find(|(i, _)| *i >= start)
                .map(|(i, _)| i)
                .unwrap_or(start);
            let safe_end = text.char_indices()
                .find(|(i, _)| *i >= end)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            if safe_start <= safe_end && safe_end <= text.len() {
                return Some(text[safe_start..safe_end].to_string());
            }
            return None;
        }
        Some(text[start..end].to_string())
    }

    /// Clear cache
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Get cache size
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }
}

#[cfg(feature = "embeddings")]
impl Default for NEREngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[cfg(feature = "embeddings")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ner_engine_creation() {
        let engine = NEREngine::new();
        assert!(!engine.is_loaded.load(Ordering::Relaxed));
        assert_eq!(engine.cache_size(), 0);
    }

    #[tokio::test]
    #[ignore] // Requires downloading model from HuggingFace
    async fn test_ner_model_loading() {
        let mut engine = NEREngine::new();
        let result = engine.load_model().await;
        assert!(result.is_ok());
        assert!(engine.is_loaded.load(Ordering::Relaxed));
    }

    #[tokio::test]
    #[ignore] // Requires model to be loaded
    async fn test_entity_extraction() {
        let mut engine = NEREngine::new();

        println!("Loading DistilBERT-NER model...");
        engine.load_model().await.unwrap();
        println!("Model loaded successfully!");

        let test_cases = vec![
            // Classic NER examples that should definitely work
            "John lives in New York City.",
            "Apple Inc. was founded by Steve Jobs.",
            "Microsoft is headquartered in Redmond, Washington.",
            // Technical terms (may or may not be recognized as MISC)
            "Rust is a programming language developed by Mozilla.",
            "PostgreSQL and Redis are popular databases.",
        ];

        for text in test_cases {
            println!("\n--- Testing: {} ---", text);
            let entities = engine.extract_entities(text).unwrap();

            println!("Found {} entities", entities.len());
            for entity in &entities {
                println!(
                    "Entity: '{}' | Type: {:?} | Confidence: {:.3} | Position: {}..{}",
                    entity.text,
                    entity.entity_type,
                    entity.confidence,
                    entity.start,
                    entity.end
                );
            }

            // For now, just log if no entities found (model might not recognize technical terms)
            if entities.is_empty() {
                println!("WARNING: No entities extracted from: {}", text);
            }

            // Verify confidence scores are valid (0.0 to 1.0)
            for entity in &entities {
                assert!(entity.confidence >= 0.0 && entity.confidence <= 1.0,
                    "Confidence should be between 0 and 1, got {}", entity.confidence);
            }
        }

        // Test cache functionality
        println!("\n--- Testing cache ---");
        let text = "Test caching with Rust and Mozilla.";
        let _ = engine.extract_entities(text).unwrap();
        assert_eq!(engine.cache_size(), 6, "Cache should have 6 entries (5 test cases + 1 cache test)");

        println!("\nCache hit test...");
        let entities_cached = engine.extract_entities(text).unwrap();
        println!("Successfully retrieved {} entities from cache", entities_cached.len());
    }

    #[test]
    fn test_entity_type_from_bio_tag() {
        assert_eq!(EntityType::from_bio_tag("B-PER"), Some(EntityType::Person));
        assert_eq!(EntityType::from_bio_tag("I-ORG"), Some(EntityType::Organization));
        assert_eq!(EntityType::from_bio_tag("B-LOC"), Some(EntityType::Location));
        assert_eq!(EntityType::from_bio_tag("I-MISC"), Some(EntityType::Miscellaneous));
        assert_eq!(EntityType::from_bio_tag("O"), None);
    }
}
