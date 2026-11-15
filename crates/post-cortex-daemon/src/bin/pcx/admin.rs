// Copyright (c) 2025 Julius ML
// MIT License

//! The offline mode of the admin commands (ARCHITECTURE section 5): when
//! the daemon does not run, they compose memory and the module host over
//! its storage in this process, without servers, and stop the host when
//! done (which writes the sessions).
//!
//! Every path starts from [`DaemonConfig::storage_config`] and
//! [`DaemonConfig::model_config`], the resolution the daemon itself uses,
//! so a CLI command never opens the daemon's database with another
//! embedding dimension.

use std::sync::Arc;

use pcx_host::Host;
use post_cortex_daemon::compose::{self, Access, Models, OpenedStorage, StorageConfig};
use post_cortex_daemon::daemon::DaemonConfig;
use post_cortex_memory::{Memory, SystemConfig};

use crate::daemon_ops;

/// Memory and its host, offline.
pub struct Offline {
    /// The capability services the command calls.
    pub memory: Arc<Memory>,
    host: Host,
}

impl Offline {
    /// Stop the host: the sessions are written.
    pub async fn finish(self) {
        self.host.stop().await;
    }
}

/// Open `storage` in this process for a command that runs without the
/// daemon, storing vectors of the daemon's dimension. A RocksDB database
/// is held by one writer only: a write open while the daemon is listening
/// is refused with how to stop it, instead of failing on the lock.
///
/// # Errors
/// The daemon holds a RocksDB database this command would write, or the
/// storage could not be opened.
pub async fn open_offline(
    config: &DaemonConfig,
    storage: &StorageConfig,
    access: Access,
) -> Result<OpenedStorage, String> {
    if access == Access::ReadWrite
        && matches!(storage, StorageConfig::RocksDb { .. })
        && daemon_ops::listening(config).await
    {
        return Err(format!(
            "The daemon at {}:{} holds the database; stop it first (`pcx stop`), \
             then retry.",
            config.host, config.port
        ));
    }
    compose::open_storage(storage, config.vector_dimension(), access).await
}

/// Memory over `storage` without embeddings (the index and the model, not
/// loaded, still give an export its vectors and its model), in the module
/// host as the daemon composes it, for the commands that run its
/// capabilities offline (Transfer for export, import and migrate).
///
/// # Errors
/// Memory could not be built over the storage, or the host refused it.
pub async fn offline_memory(
    config: &DaemonConfig,
    storage: OpenedStorage,
) -> Result<Offline, String> {
    let models = Models::new(&config.model_config());
    let system = SystemConfig {
        enable_embeddings: false,
        ..SystemConfig::default()
    };
    let memory = Arc::new(
        // Offline commands keep no blobs: none of them deletes a workspace.
        Memory::over_ports(compose::ports(storage, &models, None)?, system)
            .await
            .map_err(|e| format!("Failed to initialize: {e}"))?,
    );
    offline(memory).await
}

/// `memory` in the module host as the daemon composes it, without servers.
///
/// # Errors
/// The host refused the memory module.
pub async fn offline(memory: Arc<Memory>) -> Result<Offline, String> {
    let host = compose::host(&compose::Modules::memory(Arc::clone(&memory))?)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Offline { memory, host })
}

#[cfg(all(test, feature = "surrealdb-storage"))]
mod tests {
    use super::*;

    fn mem_storage() -> StorageConfig {
        StorageConfig::SurrealDb {
            endpoint: "mem://".into(),
            username: None,
            password: None,
            namespace: None,
            database: None,
        }
    }

    /// The write path (`pcx import`, `pcx migrate`) stores vectors of the
    /// daemon's dimension only (0.5.0 used a hard-coded 384).
    #[tokio::test]
    async fn write_open_uses_the_daemon_dimension() {
        let config = DaemonConfig::default();
        let storage = open_offline(&config, &mem_storage(), Access::ReadWrite)
            .await
            .unwrap();
        assert_eq!(config.vector_dimension(), 256);
        let meta = post_cortex_ports::VectorMetadata::new(
            "u".into(),
            "t",
            uuid::Uuid::new_v4().to_string(),
            "qa".into(),
        );
        assert!(
            storage
                .vectors
                .upsert_vectors(vec![(vec![0.5; 384], meta.clone())])
                .await
                .is_err()
        );
        storage
            .vectors
            .upsert_vectors(vec![(vec![0.5; 256], meta)])
            .await
            .unwrap();
    }

    /// The read path runs no schema statement: on a database without the
    /// post-cortex schema it refuses instead of creating one at some
    /// dimension.
    #[tokio::test]
    async fn read_only_open_changes_no_schema() {
        let err = open_offline(&DaemonConfig::default(), &mem_storage(), Access::ReadOnly)
            .await
            .err()
            .unwrap();
        assert!(err.contains("no post-cortex schema"), "{err}");
    }
}
