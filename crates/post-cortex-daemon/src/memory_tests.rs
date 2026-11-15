// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Memory instances for the daemon's tests, composed as the daemon composes
//! them (`crate::compose`): RocksDB, the in-memory index, the default model
//! (loaded on first use) and the JSON archive codec.

use post_cortex_memory::{Memory, PipelineConfig, SystemConfig};
use post_cortex_storage::RealRocksDBStorage;

use crate::compose::{ModelConfig, Models, OpenedStorage, StorageConfig};

/// Memory with `config` over RocksDB in `dir`.
pub async fn rocksdb_memory(
    dir: &tempfile::TempDir,
    config: SystemConfig,
) -> Result<Memory, String> {
    let storage = StorageConfig::RocksDb {
        data_directory: dir.path().to_path_buf(),
    };
    let models = Models::new(&ModelConfig::default());
    crate::compose::open_memory(&storage, &models, None, config, PipelineConfig::default()).await
}

/// [`rocksdb_memory`] keeping its blobs in `blobs`.
pub async fn rocksdb_memory_with_blobs(
    dir: &tempfile::TempDir,
    blobs: std::sync::Arc<dyn post_cortex_ports::BlobStore>,
    config: SystemConfig,
) -> Result<Memory, String> {
    let storage = StorageConfig::RocksDb {
        data_directory: dir.path().to_path_buf(),
    };
    let models = Models::new(&ModelConfig::default());
    crate::compose::open_memory(
        &storage,
        &models,
        Some(blobs),
        config,
        PipelineConfig::default(),
    )
    .await
}

/// Memory with `config` over the RocksDB `storage` the caller opened.
pub async fn over_rocksdb(
    storage: RealRocksDBStorage,
    config: SystemConfig,
) -> Result<Memory, String> {
    let models = Models::new(&ModelConfig::default());
    let ports = crate::compose::ports(OpenedStorage::rocksdb(storage), &models, None)?;
    Memory::over_ports(ports, config)
        .await
        .map_err(|e| e.to_string())
}

/// The memory module over `memory` as the daemon composes it, serving
/// `pcx.v1` to the other modules (W23), and its document kinds.
pub fn memory_module(
    memory: &std::sync::Arc<Memory>,
) -> (
    std::sync::Arc<dyn pcx_module_api::Module>,
    std::sync::Arc<dyn post_cortex_ports::DocumentKinds>,
) {
    let modules = crate::compose::Modules::memory(std::sync::Arc::clone(memory)).unwrap();
    let kinds = modules.kinds();
    (modules.all().remove(0), kinds)
}

/// The data of tool `tool` called with `input` in `scope` as a person
/// calls it (the app): its RPC by path through the host's channel of a
/// client, where a tool only a person calls is not refused; one of
/// memory's MCP tools as an MCP client calls it.
pub async fn as_person(
    host: &pcx_host::Host,
    scope: &pcx_module_api::Scope,
    tool: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, pcx_module_api::ModuleError> {
    let found = host
        .modules()
        .iter()
        .flat_map(|m| pcx_module_api::rpc_tool::tools(&m.manifest).unwrap())
        .find(|t| t.spec.name == tool);
    let output = match found {
        Some(rpc) => {
            rpc.call(host.channel(pcx_host::Origin::Client), scope, input)
                .await?
        }
        None => host.call_for_agent(scope, tool, input).await?,
    };
    Ok(output.data.unwrap())
}
