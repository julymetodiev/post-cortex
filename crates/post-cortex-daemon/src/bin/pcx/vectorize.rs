// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx vectorize-all`: backfill embeddings for every session and repair
//! the message vectors (`--messages`: only those). While the daemon runs,
//! the sessions are embedded through it (`VectorizeSession`); the message
//! vectors are repaired offline only.

use std::sync::Arc;

use post_cortex_daemon::compose::StorageConfig;
use post_cortex_daemon::daemon::DaemonConfig;
use post_cortex_memory::SystemConfig;
use tracing::error;

use crate::admin::offline;
use crate::remote;

pub async fn vectorize_all(messages_only: bool) -> Result<(), String> {
    let daemon = DaemonConfig::load().map_err(|e| e.to_string())?;
    if let Some(client) = remote::daemon(&daemon).await? {
        if messages_only {
            return Err(
                "message vectors are repaired offline: stop the daemon, then run \
                 `pcx vectorize-all --messages`"
                    .to_string(),
            );
        }
        return vectorize_through(client).await;
    }
    if messages_only {
        println!("Repairing the message vectors of all sessions...");
    } else {
        println!("Starting vectorization of all sessions...");
    }

    // Model, dimension and storage exactly as the daemon resolves them.
    let config = SystemConfig {
        enable_embeddings: true,
        auto_vectorize_on_update: false,
        ..SystemConfig::default()
    };
    let storage = daemon.storage_config()?;
    match &storage {
        StorageConfig::RocksDb { data_directory } => {
            println!("Target: RocksDB at {}", data_directory.display());
        }
        StorageConfig::SurrealDb { endpoint, .. } => println!("Target: SurrealDB at {endpoint}"),
    }
    let model = daemon.model_config();
    println!(
        "  Model: {} ({} dims)",
        model.embedding_model,
        model.embedding_model.embedding_dimension()
    );

    let models = post_cortex_daemon::compose::Models::new(&model);
    let memory = post_cortex_daemon::compose::open_memory(
        &storage,
        &models,
        // Embedding reads no blob.
        None,
        config,
        post_cortex_memory::PipelineConfig::default(),
    )
    .await
    .map_err(|e| format!("Failed to initialize: {}", e))?;
    let offline = offline(Arc::new(memory)).await?;
    let outcome = vectorize_offline(&offline.memory, messages_only).await;
    offline.finish().await;
    outcome
}

/// Embed every session's records, then repair the message vectors (only
/// those with `messages_only`), in this process.
async fn vectorize_offline(
    system: &post_cortex_memory::Memory,
    messages_only: bool,
) -> Result<(), String> {
    if !messages_only {
        match system.admin.vectorize_all_sessions().await {
            Ok((total, successful, failed)) => {
                println!("Vectorization complete!");
                println!("  Total items:  {}", total);
                println!("  Successful:   {}", successful);
                println!("  Failed:       {}", failed);
            }
            Err(e) => {
                error!("Vectorization failed: {}", e);
                return Err(e.to_string());
            }
        }
    }
    match system.admin.vectorize_all_messages().await {
        Ok((added, successful, failed)) => {
            println!("Message vectors repaired!");
            println!("  Vectors added: {}", added);
            println!("  Sessions:      {}", successful);
            println!("  Failed:        {}", failed);
            if failed > 0 {
                return Err(format!("{failed} session(s) not repaired, see the log"));
            }
            Ok(())
        }
        Err(e) => {
            error!("Message vector repair failed: {}", e);
            Err(e.to_string())
        }
    }
}

/// Embed every session's records through the running daemon.
async fn vectorize_through(mut daemon: remote::Daemon) -> Result<(), String> {
    println!("Vectorizing every session through the running daemon...");
    let sessions = daemon
        .list_sessions(post_cortex_proto::pb::ListSessionsRequest {
            name_filter: String::new(),
            // Every session (0 would mean the first 100).
            limit: u32::MAX,
        })
        .await
        .map_err(|s| format!("Cannot list sessions: {}", s.message()))?
        .into_inner()
        .sessions;
    let (mut vectors, mut failed) = (0u64, 0usize);
    for session in &sessions {
        let request = post_cortex_proto::pb::VectorizeSessionRequest {
            session_id: session.session_id.clone(),
        };
        match daemon.vectorize_session(request).await {
            Ok(done) => vectors += u64::from(done.into_inner().vectors_created),
            Err(status) => {
                failed += 1;
                error!(
                    "Session {} not vectorized: {}",
                    session.session_id,
                    status.message()
                );
            }
        }
    }
    println!("Vectorization complete!");
    println!("  Sessions:     {}", sessions.len());
    println!("  Vectors:      {}", vectors);
    println!("  Failed:       {}", failed);
    println!(
        "Message vectors are repaired offline: stop the daemon and run \
         `pcx vectorize-all --messages`."
    );
    if failed > 0 {
        return Err(format!("{failed} session(s) not vectorized, see the log"));
    }
    Ok(())
}
