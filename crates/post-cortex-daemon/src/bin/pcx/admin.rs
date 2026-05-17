// Copyright (c) 2025 Julius ML
// MIT License

//! Local (in-process) `ConversationMemorySystem` bootstrap shared by CLI handlers
//! that need to talk to storage directly when no daemon is running.

use post_cortex_daemon::daemon::DaemonConfig;
#[cfg(feature = "surrealdb-storage")]
use post_cortex_storage::StorageBackendType;
use post_cortex_memory::{ConversationMemorySystem, SystemConfig};

pub async fn init_admin_system() -> Result<ConversationMemorySystem, String> {
    let daemon_config = DaemonConfig::load();
    #[allow(unused_mut)]
    let mut config = SystemConfig {
        enable_embeddings: false,
        data_directory: daemon_config.data_directory,
        ..SystemConfig::default()
    };

    #[cfg(feature = "surrealdb-storage")]
    {
        config.storage_backend = match daemon_config.storage_backend.as_str() {
            "surrealdb" => StorageBackendType::SurrealDB,
            _ => StorageBackendType::RocksDB,
        };
        config.surrealdb_endpoint = daemon_config.surrealdb_endpoint;
        config.surrealdb_username = daemon_config.surrealdb_username;
        config.surrealdb_password = daemon_config.surrealdb_password;
        config.surrealdb_namespace = Some(daemon_config.surrealdb_namespace);
        config.surrealdb_database = Some(daemon_config.surrealdb_database);
    }

    ConversationMemorySystem::new(config).await
}
