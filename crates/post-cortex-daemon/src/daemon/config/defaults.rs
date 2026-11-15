// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The defaults of [`DaemonConfig`](crate::daemon::DaemonConfig) fields
//! (`#[serde(default = ...)]` and `Default`).

pub fn neighbour_radius() -> usize {
    1
}

pub fn message_retention_days() -> u32 {
    90
}

pub fn max_import_bytes() -> u64 {
    post_cortex_memory::DEFAULT_MAX_IMPORT_BYTES
}

pub fn voyage_endpoint() -> String {
    post_cortex_embeddings::rerank::VOYAGE_ENDPOINT.to_string()
}

pub fn voyage_model() -> String {
    "rerank-3-lite".to_string()
}

pub fn redact() -> bool {
    true
}

pub fn grpc_port() -> u16 {
    3738
}

pub fn storage_backend() -> String {
    "rocksdb".to_string()
}

pub fn surrealdb_namespace() -> String {
    "post_cortex".to_string()
}

pub fn surrealdb_database() -> String {
    "main".to_string()
}
