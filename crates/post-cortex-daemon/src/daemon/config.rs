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

//! Daemon configuration file support
//!
//! Loads configuration from TOML file with environment variable overrides.
//! Priority: Environment variables > Config file > Defaults

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Daemon configuration loaded from file and environment
///
/// Every field is optional in `daemon.toml`; missing keys take their
/// [`Default`] value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    /// Host to bind to (default: 127.0.0.1)
    pub host: String,

    /// Port to listen on (default: 3737)
    pub port: u16,

    /// gRPC port (default: 3738, 0 = disabled)
    #[serde(default = "defaults::grpc_port")]
    pub grpc_port: u16,

    /// Data directory for RocksDB (default: ~/.post-cortex/data)
    pub data_directory: PathBuf,

    /// Storage backend: "rocksdb" or "surrealdb" (default: rocksdb)
    #[serde(default = "defaults::storage_backend")]
    pub storage_backend: String,

    /// SurrealDB endpoint (required if storage_backend = "surrealdb")
    /// Example: "ws://localhost:8000"
    #[serde(default)]
    pub surrealdb_endpoint: Option<String>,

    /// SurrealDB username (optional)
    #[serde(default)]
    pub surrealdb_username: Option<String>,

    /// SurrealDB password (optional)
    #[serde(default)]
    pub surrealdb_password: Option<String>,

    /// SurrealDB namespace (default: "post_cortex")
    #[serde(default = "defaults::surrealdb_namespace")]
    pub surrealdb_namespace: String,

    /// SurrealDB database (default: "main")
    #[serde(default = "defaults::surrealdb_database")]
    pub surrealdb_database: String,

    /// Bearer token required on every HTTP (except `/health`) and gRPC call.
    /// Unset = no authentication, only safe on a loopback bind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,

    /// Hook scripts the module host runs (`[[hooks]]`), each pinned by the
    /// SHA-256 of its file (default: none).
    pub hooks: Vec<pcx_host::ScriptSpec>,

    /// Modules in their own processes (`[[modules]]`), each pinned by the
    /// SHA-256 of its executable (default: none).
    pub modules: Vec<pcx_host::ProcessSpec>,

    /// Built-in tool modules (`[[builtins]]`: `tools-fs`, `tools-shell`,
    /// `tools-git`) with what each may do and where (default: none; each is
    /// off until named).
    pub builtins: Vec<crate::compose::BuiltinSpec>,

    /// Redact secrets and personal data on every write (default: true)
    #[serde(default = "defaults::redact")]
    pub redact: bool,

    /// Extra redaction regexes; matches become `[REDACTED:custom]`
    #[serde(default)]
    pub redact_patterns: Vec<String>,

    /// Extra `Host` header values the HTTP endpoints accept (DNS rebinding
    /// protection). Each entry is `host` (any port) or `host:port`; `"*"`
    /// disables the check. Always added to the defaults: `localhost`,
    /// `127.0.0.1`, `::1` and `host` when it is not a wildcard address.
    /// See [`crate::daemon::host_allowlist`].
    #[serde(default)]
    pub allowed_hosts: Vec<String>,

    /// HuggingFace id of a cross-encoder that reranks search results, e.g.
    /// `cross-encoder/ms-marco-MiniLM-L4-v2` (default: none). Needs a build
    /// with the `rerank` feature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reranker_model: Option<String>,

    /// Path of the ONNX Runtime library the local reranker loads on Linux,
    /// from `ORT_DYLIB_PATH` (not a `daemon.toml` key; default: the library
    /// search path).
    #[serde(skip)]
    pub onnx_runtime: Option<String>,

    /// Voyage AI API key (or `VOYAGE_API_KEY`). When set, Voyage reranks
    /// search results, with `reranker_model` as fallback. Candidate text is
    /// sent to Voyage. Needs a build with the `rerank-voyage` feature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voyage_api_key: Option<String>,

    /// Voyage rerank model (default: `rerank-3-lite`).
    #[serde(default = "defaults::voyage_model")]
    pub voyage_model: String,

    /// Voyage rerank API (default: Voyage's own; a proxy or another region
    /// otherwise).
    #[serde(default = "defaults::voyage_endpoint")]
    pub voyage_endpoint: String,

    /// Search mode when a call does not name one: `fast` (no reranker),
    /// `local` (`reranker_model`) or `accurate` (Voyage). Default: the best
    /// the configured rerankers allow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_mode: Option<String>,

    /// Updates returned on each side of every search hit from the same
    /// session (default: 1; 0 = hits only).
    #[serde(default = "defaults::neighbour_radius")]
    pub neighbour_radius: usize,

    /// Days captured conversation messages are kept (`POST /api/messages`);
    /// older ones are deleted with their vectors once a day. Structured
    /// records are never expired. Default 90; 0 keeps messages forever.
    #[serde(default = "defaults::message_retention_days")]
    pub message_retention_days: u32,

    /// Largest archive `Import` takes, in bytes (spooled to a temporary
    /// file as it arrives). Default 1 GiB.
    #[serde(default = "defaults::max_import_bytes")]
    pub max_import_bytes: u64,

    /// Origins of web pages allowed to call (the app on the web):
    /// `https://host[:port]` entries, or `localhost` for any local port.
    /// Empty (the default): no browser routes (gRPC-Web, `/v1/import`,
    /// `/v1/export`) and no CORS answers.
    #[serde(default)]
    pub browser_origins: Vec<String>,

    /// Where memory keeps the images documents name (`[blobs]`; default:
    /// none, and a document that names an image is refused).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blobs: Option<blobs::BlobsConfig>,

    /// What the daemon lends a hosted tenant as a runner (`[runner]`, read
    /// at start; default: nothing, and no name is denied).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<RunnerConfig>,

    /// How the sandbox runs processes (`[sandbox]`; default: as this
    /// platform does, and on FreeBSD none run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxConfig>,

    /// Dynamic modules (`[dynamic]`; default: none run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic: Option<DynamicConfig>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3737,
            grpc_port: defaults::grpc_port(),
            data_directory: default_data_dir(),
            storage_backend: defaults::storage_backend(),
            surrealdb_endpoint: None,
            surrealdb_username: None,
            surrealdb_password: None,
            surrealdb_namespace: defaults::surrealdb_namespace(),
            surrealdb_database: defaults::surrealdb_database(),
            auth_token: None,
            redact: defaults::redact(),
            redact_patterns: Vec::new(),
            allowed_hosts: Vec::new(),
            reranker_model: None,
            onnx_runtime: None,
            voyage_api_key: None,
            voyage_model: defaults::voyage_model(),
            voyage_endpoint: defaults::voyage_endpoint(),
            search_mode: None,
            neighbour_radius: defaults::neighbour_radius(),
            message_retention_days: defaults::message_retention_days(),
            max_import_bytes: defaults::max_import_bytes(),
            browser_origins: Vec::new(),
            blobs: None,
            runner: None,
            sandbox: None,
            dynamic: None,
            hooks: Vec::new(),
            modules: Vec::new(),
            builtins: Vec::new(),
        }
    }
}

/// `value` of the variable `name` as a port number.
fn parse_port(name: &str, value: &str) -> Result<u16, ConfigError> {
    value
        .trim()
        .parse()
        .map_err(|e| ConfigError::Env(format!("{name} is not a port ({value:?}): {e}")))
}

impl DaemonConfig {
    /// The storage the daemon and every `pcx` command that opens storage
    /// use: the data directory, or the SurrealDB connection.
    ///
    /// Fails when `storage_backend` names no backend of this build (a typo,
    /// or `surrealdb` without the `surrealdb-storage` feature): the daemon
    /// never falls back to another backend than the one configured.
    pub fn storage_config(&self) -> Result<crate::compose::StorageConfig, String> {
        if self.storage_backend_type()? == post_cortex_storage::StorageBackendType::RocksDB {
            return Ok(crate::compose::StorageConfig::RocksDb {
                data_directory: self.data_directory.clone(),
            });
        }
        Ok(crate::compose::StorageConfig::SurrealDb {
            endpoint: self
                .surrealdb_endpoint
                .clone()
                .ok_or_else(|| "SurrealDB endpoint not configured".to_string())?,
            username: self.surrealdb_username.clone(),
            password: self.surrealdb_password.clone(),
            namespace: Some(self.surrealdb_namespace.clone()),
            database: Some(self.surrealdb_database.clone()),
        })
    }

    /// The configured `storage_backend`, parsed strictly.
    pub fn storage_backend_type(&self) -> Result<post_cortex_storage::StorageBackendType, String> {
        self.storage_backend
            .parse()
            .map_err(|e| format!("storage_backend: {e}"))
    }

    /// The models the daemon runs: the embedding model (`daemon.toml` has
    /// no model key: the default model, PotionMultilingual) and the
    /// rerankers.
    pub fn model_config(&self) -> crate::compose::ModelConfig {
        crate::compose::ModelConfig {
            embedding_model: post_cortex_embeddings::EmbeddingModelType::default(),
            reranker_model: self.reranker_model.clone(),
            onnx_runtime: self.onnx_runtime.clone(),
            voyage: self
                .voyage_api_key
                .clone()
                .map(|api_key| crate::compose::VoyageConfig {
                    endpoint: self.voyage_endpoint.clone(),
                    api_key,
                    model: self.voyage_model.clone(),
                }),
        }
    }

    /// Vector dimension of the daemon's embedding model.
    pub fn vector_dimension(&self) -> usize {
        self.model_config().embedding_model.embedding_dimension()
    }

    /// The daemon's embedding model as an archive records it: `pcx export`
    /// writes it, `pcx import` refuses an archive of another model.
    pub fn embedding_model(&self) -> post_cortex_core::archive::EmbeddingModelInfo {
        let model = self.model_config().embedding_model;
        post_cortex_core::archive::EmbeddingModelInfo {
            model: model.as_str().to_string(),
            dimension: model.embedding_dimension(),
        }
    }

    /// Where `daemon.toml` is (`~/.post-cortex/daemon.toml`).
    #[must_use]
    pub fn path() -> PathBuf {
        Self::dir().join("daemon.toml")
    }

    /// The directory of `daemon.toml` (`~/.post-cortex`), which also keeps
    /// the pairing of `pcx connect`.
    #[must_use]
    pub fn dir() -> PathBuf {
        default_config_dir()
    }

    /// The configuration: `daemon.toml` (defaults when there is none), then
    /// the `PC_*`, `VOYAGE_API_KEY` and `ORT_DYLIB_PATH` overrides,
    /// validated. Fails closed: a file that cannot be read or parsed, an
    /// override that is not valid, or a result that does not validate is an
    /// error, never a fallback to defaults.
    ///
    /// # Errors
    /// See [`ConfigError`].
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(&Self::path())
    }

    /// As [`DaemonConfig::load`], from the file at `config_path`.
    ///
    /// # Errors
    /// See [`ConfigError`].
    pub fn load_from(config_path: &Path) -> Result<Self, ConfigError> {
        let config_path = config_path.to_path_buf();
        let mut config = if config_path.exists() {
            let contents =
                fs::read_to_string(&config_path).map_err(|source| ConfigError::Read {
                    path: config_path.clone(),
                    source,
                })?;
            let parsed =
                toml::from_str::<DaemonConfig>(&contents).map_err(|source| ConfigError::Parse {
                    path: config_path.clone(),
                    source,
                })?;
            tracing::info!("Loaded configuration from {:?}", config_path);
            parsed
        } else {
            tracing::debug!("Config file {:?} not found, using defaults", config_path);
            Self::default()
        };
        config.apply_env()?;
        config.validate().map_err(ConfigError::Invalid)?;
        Ok(config)
    }

    /// Apply the environment overrides (they win over the file).
    fn apply_env(&mut self) -> Result<(), ConfigError> {
        let var = |name: &str| post_cortex_cli::env_text(name).map_err(ConfigError::Env);
        if let Some(key) = var("VOYAGE_API_KEY")?.filter(|k| !k.is_empty()) {
            self.voyage_api_key = Some(key);
        }
        if let Some(path) = var("ORT_DYLIB_PATH")?.filter(|p| !p.is_empty()) {
            self.onnx_runtime = Some(path);
        }
        if let Some(host) = var("PC_HOST")? {
            self.host = host;
        }
        if let Some(port) = var("PC_PORT")? {
            self.port = parse_port("PC_PORT", &port)?;
        }
        if let Some(port) = var("PC_GRPC_PORT")? {
            self.grpc_port = parse_port("PC_GRPC_PORT", &port)?;
        }
        if let Some(data_dir) = std::env::var_os("PC_DATA_DIR") {
            self.data_directory = data_dir.into();
        }
        if let Some(backend) = var("PC_STORAGE_BACKEND")? {
            self.storage_backend = backend;
        }
        if let Some(endpoint) = var("PC_SURREALDB_ENDPOINT")? {
            self.surrealdb_endpoint = Some(endpoint);
        }
        if let Some(username) = var("PC_SURREALDB_USER")? {
            self.surrealdb_username = Some(username);
        }
        if let Some(password) = var("PC_SURREALDB_PASS")? {
            self.surrealdb_password = Some(password);
        }
        if let Some(blobs) = &mut self.blobs {
            blobs.read_keys(var)?;
        }
        if let Some(token) = var("PC_AUTH_TOKEN")? {
            self.auth_token = (!token.is_empty()).then_some(token);
        }
        if let Some(hosts) = var("PC_ALLOWED_HOSTS")? {
            self.allowed_hosts = parse_host_list(&hosts);
        }
        if let Some(redact) = var("PC_REDACT")? {
            self.redact = match redact.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                other => {
                    return Err(ConfigError::Env(format!(
                        "PC_REDACT is {other:?}; expected on or off"
                    )));
                }
            };
        }
        Ok(())
    }

    /// The browser access `browser_origins` configures.
    ///
    /// # Errors
    /// An entry is not an origin.
    pub fn browser_access(&self) -> Result<crate::compose::BrowserAccess, String> {
        if self.browser_origins.is_empty() {
            return Ok(crate::compose::BrowserAccess::Off);
        }
        post_cortex_transport::browser::Origins::parse(&self.browser_origins)
            .map(crate::compose::BrowserAccess::Origins)
            .map_err(|e| e.to_string())
    }

    /// The local daemon as the client commands reach it.
    #[must_use]
    pub fn local(&self) -> post_cortex_cli::Local {
        post_cortex_cli::Local {
            host: self.host.clone(),
            port: self.port,
            grpc_port: self.grpc_port,
            auth_token: self.auth_token.clone(),
            data_directory: self.data_directory.clone(),
            config_dir: Self::dir(),
            message_retention_days: self.message_retention_days,
        }
    }

    /// Validate configuration values
    pub fn validate(&self) -> Result<(), String> {
        // Validate host
        if self.host.is_empty() {
            return Err("Host cannot be empty".to_string());
        }

        // Validate port range
        if self.port == 0 {
            return Err("Port cannot be 0".to_string());
        }

        // Validate data directory is not empty
        if self.data_directory.as_os_str().is_empty() {
            return Err("Data directory cannot be empty".to_string());
        }

        if self.max_import_bytes == 0 {
            return Err("max_import_bytes cannot be 0".to_string());
        }
        self.browser_access()?;

        self.storage_backend_type()?;

        crate::daemon::host_allowlist::HostPolicy::from_config(self)?;
        if let Some(runner) = &self.runner {
            runner.validate(&self.builtins)?;
        }

        Ok(())
    }
}

/// Splits a comma-separated host list, dropping empty items.
fn parse_host_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .collect()
}

/// Get default config file path: ~/.post-cortex/daemon.toml
fn default_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".post-cortex")
}

/// Get default data directory: ~/.post-cortex/data
fn default_data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".post-cortex/data")
}

mod blobs;
mod defaults;
mod error;
mod runner;
mod sandbox;

pub use error::ConfigError;
pub use runner::RunnerConfig;
pub use sandbox::{DynamicConfig, SandboxConfig};

#[cfg(test)]
mod tests;
