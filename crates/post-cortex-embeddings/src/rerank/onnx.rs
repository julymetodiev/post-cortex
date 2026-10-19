// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! BERT cross-encoder on ONNX Runtime with int8 weights: the query and a
//! document go through the encoder together as one `[CLS] query [SEP]
//! document [SEP]` sequence and the model outputs one relevance logit. The
//! `cross-encoder/ms-marco-*` repositories ship dynamically quantised
//! exports under `onnx/`, one per instruction set.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use hf_hub::api::tokio::Api;
use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::{AutoDevicePolicy, GraphOptimizationLevel};
use ort::value::Tensor;
use parking_lot::Mutex;
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams, TruncationStrategy};
use tracing::info;

use super::Reranker;

/// Pairs per run. ONNX Runtime parallelises inside one run, so one batch
/// per call would do; batches bound the padding to the longest pair in each.
const BATCH_SIZE: usize = 64;

struct Model {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    /// Whether the graph takes `token_type_ids` (BERT does, some exports not).
    token_types: bool,
}

/// Local int8 cross-encoder reranker on ONNX Runtime.
pub struct OnnxCrossEncoder {
    model: Arc<Model>,
}

/// ONNX Runtime allows one environment per process; every session shares it.
fn environment() -> Result<&'static Environment> {
    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| init().map_err(|e| format!("{e:#}")))
        .as_ref()
        .map_err(|e| anyhow!("ONNX Runtime init failed: {e}"))
}

/// Load `libonnxruntime.so` from `ORT_DYLIB_PATH` (default: the library
/// search path). Done explicitly so a missing library is an error, not the
/// panic `ort` raises when it loads the library implicitly.
#[cfg(target_os = "linux")]
fn init() -> Result<Environment> {
    let path = std::env::var("ORT_DYLIB_PATH")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "libonnxruntime.so".to_string());
    let builder = ort::init_from(&path).map_err(|e| anyhow!("loading {path}: {e}"))?;
    builder.build().map_err(|e| anyhow!("{e}"))
}

#[cfg(not(target_os = "linux"))]
fn init() -> Result<Environment> {
    ort::init().build().map_err(|e| anyhow!("{e}"))
}

/// The int8 export in the model repository that fits this CPU.
fn quantized_file() -> &'static str {
    #[cfg(target_arch = "aarch64")]
    {
        "onnx/model_qint8_arm64.onnx"
    }
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx512vnni") {
            "onnx/model_qint8_avx512_vnni.onnx"
        } else if std::arch::is_x86_feature_detected!("avx512f") {
            "onnx/model_qint8_avx512.onnx"
        } else {
            "onnx/model_quint8_avx2.onnx"
        }
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        "onnx/model.onnx"
    }
}

impl OnnxCrossEncoder {
    /// Download (if needed) the int8 export for this CPU and the tokenizer
    /// from the HuggingFace Hub. Each pair is truncated to `max_length`
    /// tokens, document side first.
    pub async fn load(model_id: &str, max_length: usize) -> Result<Self> {
        let file = quantized_file();
        info!("Loading ONNX cross-encoder {model_id} ({file}, max_length {max_length})");
        let repo = Api::new()?.model(model_id.to_string());
        let weights = repo.get(file).await?;
        let tokenizer = repo.get("tokenizer.json").await?;
        Self::from_files(&weights, &tokenizer, max_length)
    }

    /// Load from a local `.onnx` graph and `tokenizer.json`.
    pub fn from_files(weights: &Path, tokenizer: &Path, max_length: usize) -> Result<Self> {
        let mut tokenizer = Tokenizer::from_file(tokenizer).map_err(|e| anyhow!(e))?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length,
                strategy: TruncationStrategy::OnlySecond,
                ..Default::default()
            }))
            .map_err(|e| anyhow!(e))?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..Default::default()
        }));

        // `ort` defaults sessions to the MaxEfficiency device policy, which
        // ran this model ~5x slower than ONNX Runtime's own defaults (2.1 s
        // vs 0.43 s for 300 pairs on an M1 Pro); ask for the CPU at full speed.
        let session = Session::builder(environment()?)
            .map_err(|e| anyhow!("{e}"))?
            .with_auto_device(AutoDevicePolicy::PreferCPU)
            .map_err(|e| anyhow!("{e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow!("{e}"))?
            .with_intra_threads(num_cpus::get_physical())
            .map_err(|e| anyhow!("{e}"))?
            .commit_from_file(weights)
            .map_err(|e| anyhow!("{e}"))?;
        let token_types = session
            .inputs()
            .iter()
            .any(|i| i.name() == "token_type_ids");

        Ok(Self {
            model: Arc::new(Model {
                session: Mutex::new(session),
                tokenizer,
                token_types,
            }),
        })
    }
}

impl Model {
    fn score_batch(&self, query: &str, documents: &[String]) -> Result<Vec<f32>> {
        let pairs: Vec<(String, String)> = documents
            .iter()
            .map(|d| (query.to_string(), d.clone()))
            .collect();
        let encodings = self
            .tokenizer
            .encode_batch(pairs, true)
            .map_err(|e| anyhow!(e))?;
        let rows = encodings.len();
        let cols = encodings.first().map_or(0, |e| e.len());

        let mut ids = Vec::with_capacity(rows * cols);
        let mut types = Vec::with_capacity(rows * cols);
        let mut mask = Vec::with_capacity(rows * cols);
        for e in &encodings {
            ids.extend(e.get_ids().iter().map(|&x| i64::from(x)));
            types.extend(e.get_type_ids().iter().map(|&x| i64::from(x)));
            mask.extend(e.get_attention_mask().iter().map(|&x| i64::from(x)));
        }
        let shape = [rows, cols];
        let mut inputs = vec![
            ("input_ids", Tensor::from_array((shape, ids))?.into_dyn()),
            (
                "attention_mask",
                Tensor::from_array((shape, mask))?.into_dyn(),
            ),
        ];
        if self.token_types {
            inputs.push((
                "token_type_ids",
                Tensor::from_array((shape, types))?.into_dyn(),
            ));
        }

        let mut session = self.session.lock();
        let outputs = session.run(inputs)?;
        let (_, logits) = outputs[0].try_extract_tensor::<f32>()?;
        // Relevance in [0, 1]: the sigmoid of the logit.
        // Logits are [rows, 1].
        Ok(logits.iter().map(|&l| 1.0 / (1.0 + (-l).exp())).collect())
    }

    fn score(&self, query: &str, documents: &[String]) -> Result<Vec<f32>> {
        let mut scores = Vec::with_capacity(documents.len());
        for chunk in documents.chunks(BATCH_SIZE) {
            scores.extend(self.score_batch(query, chunk)?);
        }
        Ok(scores)
    }
}

#[async_trait]
impl Reranker for OnnxCrossEncoder {
    async fn score(&self, query: &str, documents: &[String]) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let model = Arc::clone(&self.model);
        let query = query.to_string();
        let documents = documents.to_vec();
        tokio::task::spawn_blocking(move || model.score(&query, &documents)).await?
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    const MODEL: &str = "cross-encoder/ms-marco-MiniLM-L4-v2";

    #[tokio::test]
    #[ignore = "requires network + HF Hub download (~25MB)"]
    async fn ranks_the_answer_first() {
        let ce = OnnxCrossEncoder::load(MODEL, 256).await.unwrap();
        let docs = vec![
            "Melanie: I went camping with the kids last weekend.".to_string(),
            "Caroline: I researched adoption agencies yesterday.".to_string(),
            "Melanie: The weather has been great lately.".to_string(),
        ];
        let scores = ce
            .score("What did Caroline research?", &docs)
            .await
            .unwrap();
        assert_eq!(scores.len(), 3);
        assert!(scores[1] > scores[0] && scores[1] > scores[2], "{scores:?}");
    }

    /// Latency for a 300-candidate pool of LoCoMo-length turns. Run with
    /// `cargo test --release --features rerank -- --ignored --nocapture onnx_pool_latency`.
    #[tokio::test]
    #[ignore = "benchmark; requires network + HF Hub download"]
    async fn onnx_pool_latency() {
        let ce = OnnxCrossEncoder::load(MODEL, 256).await.unwrap();
        let docs: Vec<String> = (0..300)
            .map(|i| {
                format!(
                    "8 May 2023 · Melanie Melanie: turn {i}, we talked about painting a lake \
                     sunrise and the pottery class I signed up for with the kids last week."
                )
            })
            .collect();
        let query = "When did Melanie paint a sunrise?";
        ce.score(query, &docs).await.unwrap();
        let mut times = Vec::new();
        for _ in 0..5 {
            let t = Instant::now();
            ce.score(query, &docs).await.unwrap();
            times.push(t.elapsed().as_millis());
        }
        times.sort_unstable();
        println!("onnx int8, 300 pairs: p50 {} ms, all {times:?}", times[2]);
    }
}
