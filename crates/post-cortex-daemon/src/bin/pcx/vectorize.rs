// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx vectorize-all` — backfill embeddings for every session.

#[cfg(feature = "embeddings")]
use post_cortex_daemon::daemon::DaemonConfig;
#[cfg(feature = "embeddings")]
use post_cortex_memory::{ConversationMemorySystem, SystemConfig};
#[cfg(all(feature = "embeddings", feature = "surrealdb-storage"))]
use post_cortex_storage::StorageBackendType;
#[cfg(feature = "embeddings")]
use tracing::error;

#[cfg(feature = "embeddings")]
pub async fn vectorize_all() -> Result<(), String> {
    println!("Starting vectorization of all sessions...");

    let daemon_config = DaemonConfig::load();
    #[allow(unused_mut)]
    let mut config = SystemConfig {
        enable_embeddings: true,
        auto_vectorize_on_update: false,
        data_directory: daemon_config.data_directory.clone(),
        ..SystemConfig::default()
    };

    #[cfg(feature = "surrealdb-storage")]
    {
        config.storage_backend = match daemon_config.storage_backend.as_str() {
            "surrealdb" => StorageBackendType::SurrealDB,
            _ => StorageBackendType::RocksDB,
        };
        config.surrealdb_endpoint = daemon_config.surrealdb_endpoint.clone();
        config.surrealdb_username = daemon_config.surrealdb_username.clone();
        config.surrealdb_password = daemon_config.surrealdb_password.clone();
        config.surrealdb_namespace = Some(daemon_config.surrealdb_namespace.clone());
        config.surrealdb_database = Some(daemon_config.surrealdb_database.clone());

        if config.storage_backend == StorageBackendType::SurrealDB {
            println!(
                "Target: SurrealDB at {}",
                config.surrealdb_endpoint.as_deref().unwrap_or("default")
            );
        } else {
            println!("Target: RocksDB at {}", config.data_directory);
        }
    }

    #[cfg(not(feature = "surrealdb-storage"))]
    println!("Target: RocksDB at {}", config.data_directory);

    let system = ConversationMemorySystem::new(config)
        .await
        .map_err(|e| format!("Failed to initialize: {}", e))?;

    match system.vectorize_all_sessions().await {
        Ok((total, successful, failed)) => {
            println!("Vectorization complete!");
            println!("  Total items:  {}", total);
            println!("  Successful:   {}", successful);
            println!("  Failed:       {}", failed);
            Ok(())
        }
        Err(e) => {
            error!("Vectorization failed: {}", e);
            Err(e)
        }
    }
}

#[cfg(not(feature = "embeddings"))]
pub async fn vectorize_all() -> Result<(), String> {
    eprintln!("Vectorization requires the 'embeddings' feature");
    eprintln!("Rebuild with: cargo build --release --features embeddings");
    Err("Embeddings feature not enabled".to_string())
}
