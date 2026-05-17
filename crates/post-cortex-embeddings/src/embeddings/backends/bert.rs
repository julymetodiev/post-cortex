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

//! BERT backend: HuggingFace Hub model load, tokenize, forward pass,
//! masked mean pooling, and L2 normalization.

use anyhow::Result;
use async_trait::async_trait;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use hf_hub::api::tokio::Api;
use std::sync::Arc;
use tokenizers::Tokenizer;
use tracing::{debug, info};

use crate::embeddings::backend::EmbeddingBackend;
use crate::embeddings::config::EmbeddingModelType;

/// Maximum sequence length for BERT models (most variants cap at 512).
const MAX_SEQ_LENGTH: usize = 512;

/// BERT-based embedding backend backed by `candle_transformers::BertModel`.
pub struct BertBackend {
    model: Arc<BertModel>,
    tokenizer: Arc<Tokenizer>,
    device: Device,
    model_type: EmbeddingModelType,
}

impl BertBackend {
    /// Download (if needed) and load a BERT model + tokenizer from HuggingFace Hub.
    pub async fn load(model_type: EmbeddingModelType) -> Result<Self> {
        info!("Loading BERT backend for model: {:?}", model_type);

        let device = Device::Cpu;
        let model_id = model_type.model_id();

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

        let bert_config_text = std::fs::read_to_string(config_path)
            .map_err(|e| anyhow::anyhow!("Failed to read BERT config: {}", e))?;
        let bert_config: BertConfig = serde_json::from_str(&bert_config_text)
            .map_err(|e| anyhow::anyhow!("Failed to parse BERT config: {}", e))?;

        // SAFETY: `from_mmaped_safetensors` is `unsafe fn` because mapping
        // an external file means the kernel can change the bytes under us
        // (the safetensors file could be truncated or replaced mid-read).
        // In our pipeline the file is downloaded by `hf-hub`, lives in
        // the user-local model cache, and is never modified after load —
        // standard candle convention.
        #[allow(unsafe_code)]
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[model_path], DType::F32, &device)?
        };
        let model = BertModel::load(vb, &bert_config)?;

        info!(
            "BERT model loaded successfully, embedding dimension: {}",
            model_type.embedding_dimension()
        );

        Ok(Self {
            model: Arc::new(model),
            tokenizer: Arc::new(tokenizer),
            device,
            model_type,
        })
    }

    /// L2-normalize a batch of embeddings (critical for cosine similarity).
    fn l2_normalize_embeddings(embeddings: &Tensor) -> Result<Tensor> {
        // embeddings shape: [batch_size, embedding_dim]
        let squared = embeddings.sqr()?;
        let sum_squared = squared.sum_keepdim(1)?;
        let l2_norm = sum_squared.sqrt()?;

        if tracing::enabled!(tracing::Level::DEBUG) {
            let l2_norm_values = l2_norm.to_vec2::<f32>()?;
            debug!(
                "L2 normalization - batch size: {}, first norm: {:.6}",
                l2_norm_values.len(),
                l2_norm_values
                    .first()
                    .and_then(|v| v.first())
                    .unwrap_or(&0.0)
            );
        }

        // Clamp norm to a small epsilon to avoid division-by-zero
        let epsilon = 1e-12_f32;
        let l2_norm_safe = l2_norm.clamp(epsilon, f32::MAX)?;
        let normalized = embeddings.broadcast_div(&l2_norm_safe)?;

        debug!("L2 normalization completed successfully");
        Ok(normalized)
    }
}

#[async_trait]
impl EmbeddingBackend for BertBackend {
    fn embedding_dimension(&self) -> usize {
        self.model_type.embedding_dimension()
    }

    fn is_bert_based(&self) -> bool {
        true
    }

    async fn process_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Tokenize
        let mut tokenized = Vec::with_capacity(texts.len());
        for text in &texts {
            let encoding = self
                .tokenizer
                .encode(text.clone(), true)
                .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;
            tokenized.push(encoding);
        }

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

            let truncate_len = ids.len().min(max_len);
            input_ids.extend_from_slice(&ids[..truncate_len]);
            attention_mask.extend_from_slice(&mask[..truncate_len]);

            if truncate_len < max_len {
                input_ids.extend(vec![0u32; max_len - truncate_len]);
                attention_mask.extend(vec![0u32; max_len - truncate_len]);
            }
        }

        // Convert u32 → i64 for BERT compatibility. The O(n) conversion is
        // negligible compared to BERT's O(n²) attention (~0.1ms / 512 tokens).
        let input_ids_i64: Vec<i64> = input_ids.iter().map(|&x| x as i64).collect();
        let attention_mask_i64: Vec<i64> = attention_mask.iter().map(|&x| x as i64).collect();

        let input_tensor = Tensor::from_vec(input_ids_i64, (texts.len(), max_len), &self.device)?;
        let mask_tensor =
            Tensor::from_vec(attention_mask_i64, (texts.len(), max_len), &self.device)?;

        // Pass None for token_type_ids — XLM-R/MiniLM ignore them.
        let outputs = self.model.forward(&input_tensor, &mask_tensor, None)?;

        // Masked mean pooling — standard for Sentence Transformers.
        let mask_f32 = mask_tensor.to_dtype(DType::F32)?;
        // mask_f32 is [batch, seq_len] → expand to [batch, seq_len, hidden_size]
        let mask_expanded = mask_f32.unsqueeze(2)?.broadcast_as(outputs.shape())?;

        let masked_outputs = outputs.broadcast_mul(&mask_expanded)?;
        let sum_embeddings = masked_outputs.sum(1)?;
        let token_counts = mask_f32.sum(1)?.unsqueeze(1)?;
        let token_counts_safe = token_counts.clamp(1e-9f64, f64::MAX)?;

        let embeddings = sum_embeddings.broadcast_div(&token_counts_safe)?;
        let embeddings_normalized = Self::l2_normalize_embeddings(&embeddings)?;
        let embeddings_vec = embeddings_normalized.to_vec2::<f32>()?;

        if tracing::enabled!(tracing::Level::DEBUG) {
            for (i, emb) in embeddings_vec.iter().enumerate() {
                let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
                debug!("Embedding {} norm after L2 normalization: {:.6}", i, norm);
            }
        }

        Ok(embeddings_vec)
    }
}
