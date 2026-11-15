// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx migrate --from rocksdb --to surrealdb` — one-shot RocksDB → SurrealDB
//! copy: an export of everything from the source imported into the target
//! (the Transfer capability of a memory instance over each, embeddings
//! off).

#![cfg(feature = "surrealdb-storage")]

use std::path::PathBuf;

use crate::admin::{offline_memory, open_offline};
use post_cortex_core::archive::{CompressionType, ExportOptions, ExportScope, ImportOptions};
use post_cortex_daemon::compose::{Access, StorageConfig};
use post_cortex_daemon::daemon::DaemonConfig;
use post_cortex_memory::capabilities::ArchiveFile;
use post_cortex_storage::StorageBackendType;

pub async fn handle_migrate(args: post_cortex_cli::cli::MigrateArgs) -> Result<(), String> {
    let post_cortex_cli::cli::MigrateArgs {
        from,
        to,
        source_path,
        target_path,
        remote_endpoint,
        username,
        password,
        dry_run,
    } = args;
    // Validate backends
    let from_backend: StorageBackendType = from
        .parse()
        .map_err(|e| format!("Invalid source backend: {e}"))?;
    let to_backend: StorageBackendType = to
        .parse()
        .map_err(|e| format!("Invalid target backend: {e}"))?;

    if from_backend == to_backend {
        return Err("Source and target backends must be different".to_string());
    }

    // Only rocksdb -> surrealdb is supported for now
    if from_backend != StorageBackendType::RocksDB || to_backend != StorageBackendType::SurrealDB {
        return Err("Only migration from RocksDB to SurrealDB is currently supported".to_string());
    }

    // Determine source path
    let daemon_config = DaemonConfig::load().map_err(|e| e.to_string())?;
    let source = source_path
        .map(PathBuf::from)
        .unwrap_or_else(|| daemon_config.data_directory.clone());

    // Determine target (local path or remote endpoint)
    let target = match target_path {
        Some(path) => PathBuf::from(path),
        None => dirs::home_dir()
            .ok_or("No home directory: give --target-path")?
            .join(".post-cortex/surrealdb"),
    };

    println!("Migration: {} -> {}", from, to);
    println!("  Source path: {}", source.display());
    match &remote_endpoint {
        Some(endpoint) => println!("  Target: remote SurrealDB at {endpoint}"),
        None => println!("  Target path: {}", target.display()),
    }
    println!();

    // The source is opened for writing: a database written by an older
    // version is migrated to this one's layout first.
    println!("Opening source storage (RocksDB)...");
    let source_storage = open_offline(
        &daemon_config,
        &StorageConfig::RocksDb {
            data_directory: source.clone(),
        },
        Access::ReadWrite,
    )
    .await?;
    let stores = &source_storage.stores;

    // Get source statistics
    let sessions = stores
        .sessions
        .list_sessions()
        .await
        .map_err(|e| format!("Failed to list sessions: {e}"))?;
    let workspaces = stores
        .workspaces
        .list_workspaces()
        .await
        .map_err(|e| format!("Failed to list workspaces: {e}"))?;
    let mut checkpoints = 0;
    for session_id in &sessions {
        checkpoints += stores
            .checkpoints
            .list_checkpoints(*session_id)
            .await
            .map_err(|e| format!("Failed to list checkpoints of {session_id}: {e}"))?
            .len();
    }

    println!("Source statistics:");
    println!("  Sessions:    {}", sessions.len());
    println!("  Workspaces:  {}", workspaces.len());
    println!("  Checkpoints: {checkpoints}");
    println!();

    if dry_run {
        println!("DRY RUN - No data will be migrated.");
        println!();
        println!("Sessions to migrate:");
        for session_id in &sessions {
            let stored = stores
                .sessions
                .load_session(*session_id)
                .await
                .map_err(|e| format!("Session {session_id} could not be read: {e}"))?;
            let name = stored
                .session
                .name()
                .clone()
                .unwrap_or_else(|| "Unnamed".to_string());
            println!(
                "  {} - {} ({} updates)",
                session_id,
                name,
                stored.session.records().len()
            );
        }
        return Ok(());
    }

    // Open target storage (local or remote) storing vectors of the daemon's
    // dimension, as the daemon will write them.
    let target_config = match &remote_endpoint {
        Some(endpoint) => {
            println!("Connecting to remote SurrealDB at {endpoint}...");
            StorageConfig::SurrealDb {
                endpoint: endpoint.clone(),
                username,
                password,
                namespace: None,
                database: None,
            }
        }
        None => {
            println!("Opening local SurrealDB at {}...", target.display());
            StorageConfig::SurrealDb {
                endpoint: target
                    .to_str()
                    .ok_or_else(|| format!("Target path {} is not UTF-8", target.display()))?
                    .to_string(),
                username: None,
                password: None,
                namespace: None,
                database: None,
            }
        }
    };
    let target_storage = open_offline(&daemon_config, &target_config, Access::ReadWrite).await?;

    // One Transfer for every backend: everything the source holds
    // (sessions with their records, graph, vectors and messages, workspaces
    // with dismissals, checkpoints, cards) in the export format, then
    // imported into the target.
    println!();
    println!("Reading the source...");
    // The archive goes through a temporary file (zstd), never memory.
    let archive = tempfile::NamedTempFile::new()
        .map_err(|e| format!("Cannot create a temporary file: {e}"))?;
    let out = archive
        .reopen()
        .map_err(|e| format!("Cannot write the temporary file: {e}"))?;
    let source = offline_memory(&daemon_config, source_storage).await?;
    let options = ExportOptions {
        include_checkpoints: true,
        compression: CompressionType::Zstd,
        ..Default::default()
    };
    let exported = async {
        let plan = source
            .memory
            .transfer
            .plan_export(ExportScope::All, options)
            .await?;
        let header = plan.header.clone();
        plan.write(Box::new(std::io::BufWriter::new(out))).await?;
        Ok::<_, post_cortex_memory::capabilities::TransferError>(header)
    }
    .await
    .map_err(|e| format!("Failed to read the source: {e:#}"));
    source.finish().await;
    let counts = exported?.metadata;
    println!(
        "  {} sessions, {} records, {} workspaces, {} checkpoints, {} vectors, {} messages, {} card versions",
        counts.session_count,
        counts.update_count,
        counts.workspace_count,
        counts.checkpoint_count,
        counts.embedding_count,
        counts.message_count,
        counts.card_count
    );
    println!("Writing the target...");
    let target = offline_memory(&daemon_config, target_storage).await?;
    let result = target
        .memory
        .transfer
        .import(&ArchiveFile::at(archive.path()), ImportOptions::default())
        .await
        .map(|(_, result)| result)
        .map_err(|e| format!("Migration failed: {e:#}"));
    target.finish().await;
    let result = result?;

    println!();
    println!("Migration complete!");
    println!("  Sessions migrated:      {}", result.sessions_imported);
    println!("  Updates migrated:       {}", result.updates_imported);
    println!("  Workspaces migrated:    {}", result.workspaces_imported);
    println!("  Checkpoints migrated:   {}", result.checkpoints_imported);
    println!("  Embeddings migrated:    {}", result.embeddings_imported);
    println!("  Messages migrated:      {}", result.messages_imported);
    println!("  Cards migrated:         {}", result.cards_imported);
    if !result.errors.is_empty() {
        println!();
        println!("Errors ({}):", result.errors.len());
        for err in &result.errors {
            println!("  - {err}");
        }
        return Err(format!("{} item(s) not migrated", result.errors.len()));
    }

    Ok(())
}
