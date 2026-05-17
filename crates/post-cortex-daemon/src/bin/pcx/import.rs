// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx import` — load an exported JSON archive back into the active backend.

use std::path::Path;

use post_cortex_daemon::daemon::DaemonConfig;
#[cfg(feature = "surrealdb-storage")]
use post_cortex_storage::SurrealDBStorage;
use post_cortex_storage::{
    ImportOptions, RealRocksDBStorage, list_export_sessions, preview_export_file, read_export_file,
};
use uuid::Uuid;

pub async fn handle_import(
    input: String,
    session: Option<Vec<String>>,
    workspace: Option<String>,
    skip_existing: bool,
    overwrite: bool,
    list: bool,
) -> Result<(), String> {
    let path = Path::new(&input);

    if !path.exists() {
        return Err(format!("File not found: {}", input));
    }

    // List mode - just show contents
    if list {
        println!("Reading export file: {}", input);
        println!();

        let metadata =
            preview_export_file(path).map_err(|e| format!("Failed to read export: {}", e))?;

        println!("Export Metadata:");
        println!("  Format Version:  {}", metadata.post_cortex_version);
        println!("  Exported At:     {}", metadata.exported_at);
        println!("  Export Type:     {:?}", metadata.export_type);
        println!("  Sessions:        {}", metadata.session_count);
        println!("  Workspaces:      {}", metadata.workspace_count);
        println!("  Updates:         {}", metadata.update_count);
        println!("  Checkpoints:     {}", metadata.checkpoint_count);
        println!("  Embeddings:      {}", metadata.embedding_count);
        println!();

        let sessions =
            list_export_sessions(path).map_err(|e| format!("Failed to list sessions: {}", e))?;

        if !sessions.is_empty() {
            println!("Sessions in export:");
            println!("{:<38} {:<30} Updates", "ID", "Name");
            println!("{:-<38} {:-<30} {:-<10}", "", "", "");
            for (id, name, updates) in sessions {
                println!("{:<38} {:<30} {}", id, name, updates);
            }
        }

        return Ok(());
    }

    // Import mode
    println!("Reading export file: {}", input);

    let export_data =
        read_export_file(path).map_err(|e| format!("Failed to read export: {}", e))?;

    println!("  Format:      {}", export_data.format_version);
    println!("  Sessions:    {}", export_data.sessions.len());
    println!("  Workspaces:  {}", export_data.workspaces.len());
    println!();

    // Parse filters
    let session_filter = if let Some(ref ids) = session {
        let uuids: Result<Vec<Uuid>, _> = ids
            .iter()
            .map(|s| Uuid::parse_str(s).map_err(|e| format!("Invalid session UUID {}: {}", s, e)))
            .collect();
        Some(uuids?)
    } else {
        None
    };

    let workspace_filter = if let Some(ref id) = workspace {
        let uuid = Uuid::parse_str(id).map_err(|e| format!("Invalid workspace UUID: {}", e))?;
        Some(vec![uuid])
    } else {
        None
    };

    let options = ImportOptions {
        session_filter,
        workspace_filter,
        skip_existing,
        overwrite,
    };

    println!("Initializing import...");
    let daemon_config = DaemonConfig::load();

    // Check storage backend configuration
    #[cfg(feature = "surrealdb-storage")]
    let use_surrealdb = daemon_config.storage_backend == "surrealdb";
    #[cfg(not(feature = "surrealdb-storage"))]
    let use_surrealdb = false;

    #[cfg(feature = "surrealdb-storage")]
    if use_surrealdb {
        // Import to SurrealDB
        let endpoint = daemon_config
            .surrealdb_endpoint
            .as_ref()
            .ok_or("SurrealDB endpoint not configured in daemon.toml")?;

        println!("Target: SurrealDB at {}", endpoint);
        println!(
            "  Namespace: {}, Database: {}",
            daemon_config.surrealdb_namespace, daemon_config.surrealdb_database
        );

        let storage = SurrealDBStorage::new(
            endpoint,
            daemon_config.surrealdb_username.as_deref(),
            daemon_config.surrealdb_password.as_deref(),
            Some(&daemon_config.surrealdb_namespace),
            Some(&daemon_config.surrealdb_database),
        )
        .await
        .map_err(|e| format!("Failed to connect to SurrealDB: {}", e))?;

        println!("Importing data to SurrealDB...");
        let result = storage
            .import_data(export_data, &options)
            .await
            .map_err(|e| format!("Import failed: {}", e))?;

        println!();
        println!("Import complete!");
        println!("  Sessions imported:   {}", result.sessions_imported);
        println!("  Sessions skipped:    {}", result.sessions_skipped);
        println!("  Workspaces imported: {}", result.workspaces_imported);
        println!("  Workspaces skipped:  {}", result.workspaces_skipped);
        println!("  Updates imported:    {}", result.updates_imported);
        println!("  Checkpoints:         {}", result.checkpoints_imported);
        println!("  Embeddings:          {}", result.embeddings_imported);

        if !result.errors.is_empty() {
            println!();
            println!("Errors ({}):", result.errors.len());
            for err in &result.errors {
                println!("  - {}", err);
            }
        }

        return Ok(());
    }

    // Import to RocksDB (default)
    if use_surrealdb {
        return Err("SurrealDB storage backend requires surrealdb-storage feature".to_string());
    }

    let data_dir = daemon_config.data_directory;
    println!("Target: RocksDB at {}", data_dir);

    let storage = RealRocksDBStorage::new(&data_dir).await.map_err(|e| {
        let err_str = e.to_string();
        if err_str.contains("LOCK") || err_str.contains("Resource temporarily unavailable") {
            "Database is locked by another process (likely the daemon).\n\
                     \n\
                     Please stop the daemon first:\n\
                     \n\
                     Option 1: Stop via CLI\n\
                     $ pkill -f 'pcx start'\n\
                     \n\
                     Option 2: Stop via launchctl (macOS)\n\
                     $ launchctl unload ~/Library/LaunchAgents/com.juliusml.post-cortex.plist\n\
                     \n\
                     Then retry the import command."
                .to_string()
        } else {
            format!("Failed to open storage: {}", e)
        }
    })?;

    println!("Importing data to RocksDB...");
    let result = storage
        .import_data(export_data, &options)
        .await
        .map_err(|e| format!("Import failed: {}", e))?;

    println!();
    println!("Import complete!");
    println!("  Sessions imported:   {}", result.sessions_imported);
    println!("  Sessions skipped:    {}", result.sessions_skipped);
    println!("  Workspaces imported: {}", result.workspaces_imported);
    println!("  Workspaces skipped:  {}", result.workspaces_skipped);
    println!("  Updates imported:    {}", result.updates_imported);
    println!("  Checkpoints:         {}", result.checkpoints_imported);
    println!("  Embeddings:          {}", result.embeddings_imported);

    if !result.errors.is_empty() {
        println!();
        println!("Errors ({}):", result.errors.len());
        for err in &result.errors {
            println!("  - {}", err);
        }
    }

    Ok(())
}
