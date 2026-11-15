// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! BERT cross-encoder on ONNX Runtime with int8 weights: the query and a
//! document go through the encoder together as one `[CLS] query [SEP]
//! document [SEP]` sequence and the model outputs one relevance logit. The
//! `cross-encoder/ms-marco-*` repositories ship dynamically quantised
//! exports under `onnx/`, one per instruction set.
//!
//! The encoder owns a small pool of sessions: [`SESSIONS`] scores run at
//! once on the blocking pool, each on its share of the physical cores, and
//! one that waits and runs longer than [`TIMEOUT`] fails (the search then
//! keeps its fused order).

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::blocking::{self, Abandoned};
use crate::error::{Error, Result};
use async_trait::async_trait;
use hf_hub::api::tokio::Api;
use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::{AutoDevicePolicy, GraphOptimizationLevel};
use ort::value::Tensor;
use parking_lot::Mutex;
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams, TruncationStrategy};
use tokio::sync::Semaphore;
use tracing::info;

use post_cortex_ports::{PortError, Reranker};

/// Pairs per run. ONNX Runtime parallelises inside one run, so one batch
/// per call would do; batches bound the padding to the longest pair in each.
const BATCH_SIZE: usize = 64;

/// Sessions of one encoder: scores that run at once.
const SESSIONS: usize = 2;

/// Longest one score may wait for a session and run.
const TIMEOUT: Duration = Duration::from_secs(5);

struct Model {
    /// The sessions not running a score now.
    idle: Mutex<Vec<Session>>,
    tokenizer: Tokenizer,
    /// Whether the graph takes `token_type_ids` (BERT does, some exports not).
    token_types: bool,
}

/// Local int8 cross-encoder reranker on ONNX Runtime.
pub struct OnnxCrossEncoder {
    model: Arc<Model>,
    /// One permit per idle session.
    slots: Arc<Semaphore>,
}

/// ONNX Runtime allows one environment per process; every session shares it.
/// The first load sets it up, from its `runtime` library path.
fn environment(runtime: Option<&str>) -> Result<&'static Environment> {
    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| init(runtime).map_err(|e| format!("{e:#}")))
        .as_ref()
        .map_err(|e| runtime_error(format!("ONNX Runtime init failed: {e}")))
}

/// Load `libonnxruntime.so` from `runtime` (the daemon passes
/// `ORT_DYLIB_PATH`; default: the library search path). Done explicitly so a
/// missing library is an error, not the panic `ort` raises when it loads the
/// library implicitly.
#[cfg(target_os = "linux")]
fn init(runtime: Option<&str>) -> Result<Environment> {
    let path = runtime
        .filter(|p| !p.is_empty())
        .unwrap_or("libonnxruntime.so");
    let builder =
        ort::init_from(path).map_err(|e| runtime_error(format!("loading {path}: {e}")))?;
    builder.build().map_err(runtime_error)
}

/// Elsewhere ONNX Runtime is linked statically; `runtime` is not used.
#[cfg(not(target_os = "linux"))]
fn init(_runtime: Option<&str>) -> Result<Environment> {
    ort::init().build().map_err(runtime_error)
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
    /// tokens, document side first. `runtime` is the path of
    /// `libonnxruntime.so` on Linux (`None`: the library search path).
    pub async fn load(model_id: &str, max_length: usize, runtime: Option<&str>) -> Result<Self> {
        let file = quantized_file();
        info!("Loading ONNX cross-encoder {model_id} ({file}, max_length {max_length})");
        let load = |e: hf_hub::api::tokio::ApiError| Error::ModelLoad {
            model: model_id.to_string(),
            reason: e.to_string(),
        };
        let repo = Api::new().map_err(load)?.model(model_id.to_string());
        let weights = repo.get(file).await.map_err(load)?;
        let tokenizer = repo.get("tokenizer.json").await.map_err(load)?;
        Self::from_files(&weights, &tokenizer, max_length, runtime)
    }

    /// Load from a local `.onnx` graph and `tokenizer.json`, with ONNX
    /// Runtime from `runtime` (see [`Self::load`]).
    pub fn from_files(
        weights: &Path,
        tokenizer: &Path,
        max_length: usize,
        runtime: Option<&str>,
    ) -> Result<Self> {
        let mut tokenizer =
            Tokenizer::from_file(tokenizer).map_err(|e| Error::Tokenization(e.to_string()))?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length,
                strategy: TruncationStrategy::OnlySecond,
                ..Default::default()
            }))
            .map_err(|e| Error::Tokenization(e.to_string()))?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..Default::default()
        }));

        let threads = (num_cpus::get_physical() / SESSIONS).max(1);
        let idle = (0..SESSIONS)
            .map(|_| session(environment(runtime)?, weights, threads))
            .collect::<Result<Vec<Session>>>()?;
        let token_types = idle.first().is_some_and(|session| {
            session
                .inputs()
                .iter()
                .any(|i| i.name() == "token_type_ids")
        });

        Ok(Self {
            model: Arc::new(Model {
                idle: Mutex::new(idle),
                tokenizer,
                token_types,
            }),
            slots: Arc::new(Semaphore::new(SESSIONS)),
        })
    }
}

/// A session of the graph `weights` running on `threads` threads.
fn session(environment: &Environment, weights: &Path, threads: usize) -> Result<Session> {
    // `ort` defaults sessions to the MaxEfficiency device policy, which
    // ran this model ~5x slower than ONNX Runtime's own defaults (2.1 s
    // vs 0.43 s for 300 pairs on an M1 Pro); ask for the CPU at full speed.
    Session::builder(environment)
        .map_err(runtime_error)?
        .with_auto_device(AutoDevicePolicy::PreferCPU)
        .map_err(runtime_error)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(runtime_error)?
        .with_intra_threads(threads)
        .map_err(runtime_error)?
        .commit_from_file(weights)
        .map_err(runtime_error)
}

impl Model {
    fn score_batch(
        &self,
        session: &mut Session,
        query: &str,
        documents: &[Arc<str>],
    ) -> Result<Vec<f32>> {
        let pairs: Vec<(&str, &str)> = documents.iter().map(|d| (query, &**d)).collect();
        let encodings = self
            .tokenizer
            .encode_batch(pairs, true)
            .map_err(|e| Error::Tokenization(e.to_string()))?;
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

        let outputs = session.run(inputs)?;
        let (_, logits) = outputs[0].try_extract_tensor::<f32>()?;
        // Relevance in [0, 1]: the sigmoid of the logit.
        // Logits are [rows, 1].
        Ok(logits.iter().map(|&l| 1.0 / (1.0 + (-l).exp())).collect())
    }

    /// The scores of `documents` on an idle session (there is one for
    /// every permit of the encoder's slots), which goes back to the pool
    /// afterwards.
    fn score(
        &self,
        query: &str,
        documents: &[Arc<str>],
        abandoned: &Abandoned,
    ) -> Result<Vec<f32>> {
        let mut session = self
            .idle
            .lock()
            .pop()
            .ok_or_else(|| Error::Inference("no idle reranker session".to_string()))?;
        let scores = self.score_all(&mut session, query, documents, abandoned);
        self.idle.lock().push(session);
        scores
    }

    fn score_all(
        &self,
        session: &mut Session,
        query: &str,
        documents: &[Arc<str>],
        abandoned: &Abandoned,
    ) -> Result<Vec<f32>> {
        let mut scores = Vec::with_capacity(documents.len());
        for chunk in documents.chunks(BATCH_SIZE) {
            abandoned.check()?;
            scores.extend(self.score_batch(session, query, chunk)?);
        }
        Ok(scores)
    }
}

#[async_trait]
impl Reranker for OnnxCrossEncoder {
    async fn score(
        &self,
        query: &str,
        documents: &[Arc<str>],
    ) -> post_cortex_ports::error::Result<Vec<f32>> {
        self.rerank(query, documents).await.map_err(PortError::from)
    }
}

impl OnnxCrossEncoder {
    /// One relevance per document, in input order.
    ///
    /// # Errors
    /// `Timeout` past [`TIMEOUT`]; the session's error.
    async fn rerank(&self, query: &str, documents: &[Arc<str>]) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let model = Arc::clone(&self.model);
        let query = query.to_string();
        let documents = documents.to_vec();
        let run = blocking::run(&self.slots, move |abandoned| {
            model.score(&query, &documents, abandoned)
        });
        tokio::time::timeout(TIMEOUT, run)
            .await
            .map_err(|_| Error::Timeout(TIMEOUT.as_secs()))?
    }
}

/// [`Error::ModelLoad`] for an ONNX Runtime or session set-up failure.
fn runtime_error(e: impl std::fmt::Display) -> Error {
    Error::ModelLoad {
        model: "onnx runtime".to_string(),
        reason: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    const MODEL: &str = "cross-encoder/ms-marco-MiniLM-L4-v2";

    /// The smallest cross-encoder (~25 MB, cached by the HuggingFace Hub
    /// client and in CI), on the ONNX Runtime `ORT_DYLIB_PATH` names (Linux).
    pub async fn model() -> OnnxCrossEncoder {
        let runtime = std::env::var("ORT_DYLIB_PATH").ok();
        OnnxCrossEncoder::load(MODEL, 256, runtime.as_deref())
            .await
            .unwrap()
    }

    /// The model, for the port's conformance suite.
    pub async fn loaded() -> (Arc<dyn Reranker>, ()) {
        (Arc::new(model().await), ())
    }

    mod conformance {
        post_cortex_ports::rerank_conformance_suite!(crate::rerank::onnx::tests::loaded());
    }

    #[tokio::test]
    async fn ranks_the_answer_first() {
        let ce = model().await;
        let docs: Vec<Arc<str>> = vec![
            "Melanie: I went camping with the kids last weekend.".into(),
            "Caroline: I researched adoption agencies yesterday.".into(),
            "Melanie: The weather has been great lately.".into(),
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
        let ce = model().await;
        let docs: Vec<Arc<str>> = (0..300)
            .map(|i| {
                Arc::from(format!(
                    "8 May 2023 · Melanie Melanie: turn {i}, we talked about painting a lake \
                     sunrise and the pottery class I signed up for with the kids last week."
                ))
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
