// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx export` — write sessions/workspaces to a (compressed) JSON file.

use std::io::{self, Write};
use std::path::Path;

use post_cortex_daemon::daemon::DaemonConfig;
#[cfg(feature = "surrealdb-storage")]
use post_cortex_storage::SurrealDBStorage;
use post_cortex_storage::{CompressionType, ExportOptions, RealRocksDBStorage, write_export_file};
use uuid::Uuid;

async fn export_from_rocksdb(
    daemon_config: &DaemonConfig,
    options: &ExportOptions,
    workspace: Option<String>,
    session: Option<Vec<String>>,
) -> Result<post_cortex_storage::export_import::ExportData, String> {
    let data_dir = &daemon_config.data_directory;
    println!("Source: RocksDB at {}", data_dir);

    let storage = RealRocksDBStorage::new(data_dir).await.map_err(|e| {
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
                     Then retry the export command."
                .to_string()
        } else {
            format!("Failed to open storage: {}", e)
        }
    })?;

    if let Some(workspace_id) = workspace {
        let uuid =
            Uuid::parse_str(&workspace_id).map_err(|e| format!("Invalid workspace UUID: {}", e))?;
        println!("Exporting workspace {}...", workspace_id);
        storage
            .export_workspace(uuid, options)
            .await
            .map_err(|e| format!("Export failed: {}", e))
    } else if let Some(session_ids) = session {
        let uuids: Result<Vec<Uuid>, _> = session_ids
            .iter()
            .map(|s| Uuid::parse_str(s).map_err(|e| format!("Invalid session UUID {}: {}", s, e)))
            .collect();
        let uuids = uuids?;
        println!("Exporting {} session(s)...", uuids.len());
        storage
            .export_sessions(uuids, options)
            .await
            .map_err(|e| format!("Export failed: {}", e))
    } else {
        println!("Exporting all data...");
        storage
            .export_full(options)
            .await
            .map_err(|e| format!("Export failed: {}", e))
    }
}

pub async fn handle_export(
    output: Option<String>,
    compress: Option<String>,
    session: Option<Vec<String>>,
    workspace: Option<String>,
    checkpoints: bool,
    pretty: bool,
    force: bool,
) -> Result<(), String> {
    // Generate default filename if not provided
    let output = output.unwrap_or_else(|| {
        let now = chrono::Local::now();
        format!("export-{}.json.gz", now.format("%Y-%m-%d_%H-%M-%S"))
    });

    let path = Path::new(&output);

    // Check if file exists and ask for confirmation
    if path.exists() && !force {
        print!("File '{}' already exists. Overwrite? [y/N] ", output);
        io::stdout().flush().ok();

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| format!("Failed to read input: {}", e))?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Export cancelled.");
            return Ok(());
        }
    }

    println!("Initializing export...");

    let daemon_config = DaemonConfig::load();

    // Determine compression
    let compression = if let Some(ref c) = compress {
        CompressionType::from_str(c)
            .ok_or_else(|| format!("Invalid compression type: {}. Use: none, gzip, zstd", c))?
    } else {
        CompressionType::from_path(Path::new(&output))
    };

    let options = ExportOptions {
        compression,
        include_checkpoints: checkpoints,
        pretty,
    };

    // Check storage backend configuration
    #[cfg(feature = "surrealdb-storage")]
    let use_surrealdb = daemon_config.storage_backend == "surrealdb";

    // Perform export based on storage backend
    #[cfg(feature = "surrealdb-storage")]
    let export_data = if use_surrealdb {
        let endpoint = daemon_config
            .surrealdb_endpoint
            .as_ref()
            .ok_or("SurrealDB endpoint not configured in daemon.toml")?;

        println!("Source: SurrealDB at {}", endpoint);

        let storage = SurrealDBStorage::new(
            endpoint,
            daemon_config.surrealdb_username.as_deref(),
            daemon_config.surrealdb_password.as_deref(),
            Some(&daemon_config.surrealdb_namespace),
            Some(&daemon_config.surrealdb_database),
        )
        .await
        .map_err(|e| format!("Failed to connect to SurrealDB: {}", e))?;

        if let Some(workspace_id) = workspace {
            let uuid = Uuid::parse_str(&workspace_id)
                .map_err(|e| format!("Invalid workspace UUID: {}", e))?;
            println!("Exporting workspace {}...", workspace_id);
            storage
                .export_workspace(uuid, &options)
                .await
                .map_err(|e| format!("Export failed: {}", e))?
        } else if let Some(session_ids) = session {
            let uuids: Result<Vec<Uuid>, _> = session_ids
                .iter()
                .map(|s| {
                    Uuid::parse_str(s).map_err(|e| format!("Invalid session UUID {}: {}", s, e))
                })
                .collect();
            let uuids = uuids?;
            println!("Exporting {} session(s)...", uuids.len());
            storage
                .export_sessions(uuids, &options)
                .await
                .map_err(|e| format!("Export failed: {}", e))?
        } else {
            println!("Exporting all data...");
            storage
                .export_full(&options)
                .await
                .map_err(|e| format!("Export failed: {}", e))?
        }
    } else {
        export_from_rocksdb(&daemon_config, &options, workspace, session).await?
    };

    #[cfg(not(feature = "surrealdb-storage"))]
    let export_data = export_from_rocksdb(&daemon_config, &options, workspace, session).await?;

    // Write to file
    let path = Path::new(&output);
    let stats = write_export_file(&export_data, path, &options)
        .map_err(|e| format!("Failed to write export file: {}", e))?;

    println!();
    println!("Export complete!");
    println!("  File:        {}", output);
    println!("  Format:      {}", export_data.format_version);
    println!("  Compression: {:?}", compression);
    println!("  Sessions:    {}", export_data.metadata.session_count);
    println!("  Workspaces:  {}", export_data.metadata.workspace_count);
    println!("  Updates:     {}", export_data.metadata.update_count);
    println!("  Checkpoints: {}", export_data.metadata.checkpoint_count);
    println!("  Embeddings:  {}", export_data.metadata.embedding_count);

    // Show size info based on compression
    if compression == CompressionType::None {
        println!("  Size:        {} bytes", stats.file_size);
    } else {
        let compression_ratio =
            (1.0 - (stats.file_size as f64 / stats.uncompressed_size as f64)) * 100.0;
        println!(
            "  Size:        {} bytes ({:.0}% compression, from {} uncompressed)",
            stats.file_size, compression_ratio, stats.uncompressed_size
        );
    }

    Ok(())
}
