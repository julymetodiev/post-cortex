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

//! Model2Vec backend — static, multilingual embeddings via `model2vec-rs`.
//!
//! Loads `minishlab/potion-*` checkpoints from the HuggingFace Hub (or a
//! local path), encodes batches of text through the precomputed token-vector
//! table, and returns L2-normalised vectors. All computation is CPU-only and
//! orders of magnitude faster than the BERT path — typically ms/text on a
//! laptop CPU with no GPU.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use hf_hub::repository::{HFRepository, RepoTypeModel};
use hf_hub::{HFClient, HFError, split_id};
use model2vec_rs::model::StaticModel;
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::embeddings::backend::EmbeddingBackend;
use crate::embeddings::config::EmbeddingModelType;

/// Default token-length budget passed to `StaticModel::encode_with_args`.
/// Mirrors `model2vec-rs`' built-in default but kept explicit so callers
/// can audit the trade-off.
const DEFAULT_MAX_TOKENS: usize = 512;

/// Texts per batch: a static model costs a table lookup per token, so the
/// engine hands it large slices.
const BATCH_SIZE: usize = 1024;

/// The files of a Model2Vec model, the weights last.
const MODEL_FILES: [&str; 3] = ["config.json", "tokenizer.json", "model.safetensors"];

/// A failure to load `model_id`, for `reason`.
fn load_error(model_id: &str, reason: String) -> Error {
    Error::ModelLoad {
        model: model_id.to_string(),
        reason,
    }
}

/// The Hub's handle of the model `model_id` (`owner/name`) on `client`.
fn repository(client: &HFClient, model_id: &str) -> HFRepository<RepoTypeModel> {
    let (owner, name) = split_id(model_id);
    client.model(owner, name)
}

/// The snapshot directory of `model_id` in `client`'s cache when it holds
/// every file a Model2Vec model needs; nothing is fetched.
async fn cached_snapshot(client: &HFClient, model_id: &str) -> Result<Option<PathBuf>> {
    let weights = repository(client, model_id)
        .download_file()
        .filename("model.safetensors")
        .local_files_only(true)
        .send()
        .await;
    let weights = match weights {
        Ok(path) => path,
        Err(HFError::LocalEntryNotFound { .. }) => return Ok(None),
        Err(e) => return Err(load_error(model_id, format!("the cache: {e:#}"))),
    };
    Ok(weights
        .parent()
        .map(Path::to_path_buf)
        .filter(|dir| MODEL_FILES.iter().all(|file| dir.join(file).exists())))
}

/// Fetch the files of `model_id` from the Hugging Face Hub into `client`'s
/// cache (again, even when cached, with `refresh`) and return its snapshot
/// directory. The lookup and the fetch share the cache, so a model fetched
/// once is found at the next start (`HF_HOME` included).
async fn fetch_snapshot(client: &HFClient, model_id: &str, refresh: bool) -> Result<PathBuf> {
    let repo = repository(client, model_id);
    let mut snapshot = None;
    for file in MODEL_FILES {
        let path = repo
            .download_file()
            .filename(file)
            .force_download(refresh)
            .send()
            .await
            .map_err(|e| load_error(model_id, format!("{file}: {e:#}")))?;
        snapshot = path.parent().map(Path::to_path_buf);
    }
    snapshot.ok_or_else(|| load_error(model_id, "no snapshot directory".to_string()))
}

/// One fetch at a time in this process. The Hub's client gives up on a file
/// another fetch holds after a timeout (`HFError::CacheLockTimeout`), so of
/// two engines started together on a machine without the model the second
/// failed.
static FETCHING: Mutex<()> = Mutex::const_new(());

/// Load the model in the snapshot directory `dir`, off the reactor (it
/// reads and maps the weights).
async fn load_dir(model_id: &str, dir: PathBuf) -> Result<StaticModel> {
    let model =
        tokio::task::spawn_blocking(move || StaticModel::from_pretrained(dir, None, None, None))
            .await?;
    model.map_err(|e| load_error(model_id, format!("{e:#}")))
}

/// Load the Model2Vec model `model_id` from the Hugging Face cache
/// (`HF_HUB_CACHE`, else `$HF_HOME/hub`, else `~/.cache/huggingface/hub`),
/// fetching it into that cache first when it is not there; a cached
/// snapshot that does not load is fetched again. A load that waited for
/// another's fetch finds the model in the cache.
async fn load_static_model(model_id: &str) -> Result<StaticModel> {
    let client = HFClient::new().map_err(|e| load_error(model_id, format!("hub client: {e:#}")))?;
    if let Some(dir) = cached_snapshot(&client, model_id).await? {
        debug!("Model2Vec {model_id}: loading the cached snapshot {dir:?}");
        match load_dir(model_id, dir).await {
            Ok(model) => return Ok(model),
            Err(e) => debug!(
                "Model2Vec {model_id}: the cached snapshot does not load ({e}), fetching it again"
            ),
        }
        let _fetching = FETCHING.lock().await;
        return load_dir(model_id, fetch_snapshot(&client, model_id, true).await?).await;
    }
    let _fetching = FETCHING.lock().await;
    match cached_snapshot(&client, model_id).await? {
        Some(dir) => load_dir(model_id, dir).await,
        None => load_dir(model_id, fetch_snapshot(&client, model_id, false).await?).await,
    }
}

/// Static-embedding backend powered by `model2vec-rs`.
///
/// Construct via [`Self::load`] — the call is async and offloads the
/// (blocking) `StaticModel::from_pretrained` download/load onto
/// [`tokio::task::spawn_blocking`]. Returns the model's own (L2-normalised)
/// vectors.
pub struct Model2VecBackend {
    model: StaticModel,
    dimension: usize,
    model_type: EmbeddingModelType,
}

impl Model2VecBackend {
    /// Download (if needed) and load a Model2Vec model from the HuggingFace
    /// Hub. Verifies the runtime embedding dimension against the
    /// `EmbeddingModelType` constant and returns an error on mismatch —
    /// silent drift would corrupt the vector index.
    pub async fn load(model_type: EmbeddingModelType) -> Result<Self> {
        let model_id = model_type.model_id().to_string();
        info!("Loading Model2Vec backend for model: {}", model_id);

        // A model already in the Hugging Face cache loads from its snapshot
        // directory: going through the Hub revalidates every file online on
        // each start, which is slow and fails when the Hub is slow or
        // unreachable.
        let model = load_static_model(&model_id).await?;

        // Probe the actual dimension by encoding a sentinel — this is the
        // ground-truth source. If it disagrees with the enum constant the
        // index would be sized wrong, so fail loudly.
        let probe = model.encode_with_args(
            &["dimension probe".to_string()],
            Some(DEFAULT_MAX_TOKENS),
            BATCH_SIZE,
        );
        let runtime_dim = probe.first().map(|v| v.len()).unwrap_or(0);
        let declared_dim = model_type.embedding_dimension();
        if runtime_dim == 0 {
            return Err(Error::NoEmbedding(model_id.to_string()));
        }
        if runtime_dim != declared_dim {
            return Err(Error::Dimension {
                expected: declared_dim,
                actual: runtime_dim,
            });
        }

        info!(
            "Model2Vec model loaded — id: {}, dim: {}",
            model_id, runtime_dim
        );

        Ok(Self {
            model,
            dimension: runtime_dim,
            model_type,
        })
    }
}

impl EmbeddingBackend for Model2VecBackend {
    fn embedding_dimension(&self) -> usize {
        self.dimension
    }

    fn batch_size(&self) -> usize {
        BATCH_SIZE
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let embeddings = self
            .model
            .encode_with_args(texts, Some(DEFAULT_MAX_TOKENS), BATCH_SIZE);
        debug!(
            "Model2Vec encoded {} texts for {}",
            embeddings.len(),
            self.model_type
        );
        Ok(embeddings)
    }
}

#[cfg(test)]
mod tests;
