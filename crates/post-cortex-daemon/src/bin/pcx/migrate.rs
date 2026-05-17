// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx migrate --from rocksdb --to surrealdb` — one-shot RocksDB → SurrealDB
//! copy with progress reporting. Native graph data (entities, relationships)
//! goes through SurrealDB's typed APIs; embeddings/checkpoints are migrated
//! row by row.

#![cfg(feature = "surrealdb-storage")]

use std::path::PathBuf;

use post_cortex_daemon::daemon::DaemonConfig;
use post_cortex_storage::{
    GraphStorage, RealRocksDBStorage, Storage, StorageBackendType, SurrealDBStorage,
};

pub async fn handle_migrate(
    from: String,
    to: String,
    source_path: Option<String>,
    target_path: Option<String>,
    remote_endpoint: Option<String>,
    username: Option<String>,
    password: Option<String>,
    dry_run: bool,
) -> Result<(), String> {
    // Validate backends
    let from_backend = StorageBackendType::from_str(&from)
        .ok_or_else(|| format!("Invalid source backend: {}. Use: rocksdb", from))?;
    let to_backend = StorageBackendType::from_str(&to)
        .ok_or_else(|| format!("Invalid target backend: {}. Use: surrealdb", to))?;

    if from_backend == to_backend {
        return Err("Source and target backends must be different".to_string());
    }

    // Only rocksdb -> surrealdb is supported for now
    if from_backend != StorageBackendType::RocksDB || to_backend != StorageBackendType::SurrealDB {
        return Err("Only migration from RocksDB to SurrealDB is currently supported".to_string());
    }

    // Determine source path
    let daemon_config = DaemonConfig::load();
    let source = source_path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&daemon_config.data_directory));

    // Determine target (local path or remote endpoint)
    let is_remote = remote_endpoint.is_some();
    let target = target_path.map(PathBuf::from).unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".post-cortex/surrealdb")
    });

    println!("Migration: {} -> {}", from, to);
    println!("  Source path: {}", source.display());
    if is_remote {
        println!(
            "  Target: remote SurrealDB at {}",
            remote_endpoint.as_ref().unwrap()
        );
    } else {
        println!("  Target path: {}", target.display());
    }
    println!();

    // Open source storage
    println!("Opening source storage (RocksDB)...");
    let source_storage = RealRocksDBStorage::new(&source).await.map_err(|e| {
        let err_str = e.to_string();
        if err_str.contains("LOCK") || err_str.contains("Resource temporarily unavailable") {
            "Source database is locked by another process (likely the daemon).\n\
                     Please stop the daemon first: pkill -f 'pcx start'"
                .to_string()
        } else {
            format!("Failed to open source storage: {}", e)
        }
    })?;

    // Get source statistics
    let sessions = source_storage
        .list_sessions()
        .await
        .map_err(|e| format!("Failed to list sessions: {}", e))?;
    let workspaces = source_storage
        .list_workspaces()
        .await
        .map_err(|e| format!("Failed to list workspaces: {}", e))?;
    let checkpoints = source_storage
        .list_checkpoints()
        .await
        .map_err(|e| format!("Failed to list checkpoints: {}", e))?;

    println!("Source statistics:");
    println!("  Sessions:    {}", sessions.len());
    println!("  Workspaces:  {}", workspaces.len());
    println!("  Checkpoints: {}", checkpoints.len());
    println!();

    if dry_run {
        println!("DRY RUN - No data will be migrated.");
        println!();
        println!("Sessions to migrate:");
        for session_id in &sessions {
            if let Ok(session) = source_storage.load_session(*session_id).await {
                let name = session
                    .name()
                    .clone()
                    .unwrap_or_else(|| "Unnamed".to_string());
                let update_count = source_storage
                    .load_session_updates(*session_id)
                    .await
                    .map(|u| u.len())
                    .unwrap_or(0);
                println!("  {} - {} ({} updates)", session_id, name, update_count);
            }
        }
        return Ok(());
    }

    // Open target storage (local or remote)
    let target_storage = if let Some(endpoint) = &remote_endpoint {
        println!("Connecting to remote SurrealDB at {}...", endpoint);
        SurrealDBStorage::new(
            endpoint,
            username.as_deref(),
            password.as_deref(),
            None,
            None,
        )
        .await
        .map_err(|e| format!("Failed to connect to remote SurrealDB: {}", e))?
    } else {
        println!("Opening local SurrealDB at {}...", target.display());
        SurrealDBStorage::new(target.to_str().unwrap_or_default(), None, None, None, None)
            .await
            .map_err(|e| format!("Failed to open target storage: {}", e))?
    };

    // Migrate sessions
    println!();
    println!("Migrating sessions...");
    let mut migrated_sessions = 0;
    let mut migrated_updates = 0;
    let mut migrated_entities = 0;
    let mut migrated_relationships = 0;

    for session_id in &sessions {
        match source_storage.load_session(*session_id).await {
            Ok(session) => {
                let name = session
                    .name()
                    .clone()
                    .unwrap_or_else(|| "Unnamed".to_string());

                // Save session to target
                if let Err(e) = target_storage.save_session(&session).await {
                    println!("  [ERROR] {} - {}: {}", session_id, name, e);
                    continue;
                }

                // Get and save updates
                let update_count = match source_storage.load_session_updates(*session_id).await {
                    Ok(updates) => {
                        let count = updates.len();
                        if !updates.is_empty() {
                            if let Err(e) = target_storage
                                .batch_save_updates(*session_id, updates)
                                .await
                            {
                                println!(
                                    "  [WARN] {} - {}: Failed to save updates: {}",
                                    session_id, name, e
                                );
                            } else {
                                migrated_updates += count;
                            }
                        }
                        count
                    }
                    Err(e) => {
                        println!(
                            "  [WARN] {} - {}: Failed to load updates: {}",
                            session_id, name, e
                        );
                        0
                    }
                };

                // Extract and save entities from entity_graph to native SurrealDB graph
                let entities = session.entity_graph.get_all_entities();
                let entity_count = entities.len();
                for entity in &entities {
                    if let Err(e) = target_storage.upsert_entity(*session_id, entity).await {
                        println!(
                            "  [WARN] {} - {}: Failed to save entity '{}': {}",
                            session_id, name, entity.name, e
                        );
                    } else {
                        migrated_entities += 1;
                    }
                }

                // Extract and save relationships to native SurrealDB RELATE
                let relationships = session.entity_graph.get_all_relationships();
                let rel_count = relationships.len();
                for rel in &relationships {
                    if let Err(e) = target_storage.create_relationship(*session_id, rel).await {
                        println!(
                            "  [WARN] {} - {}: Failed to save relationship '{}'->'{}': {}",
                            session_id, name, rel.from_entity, rel.to_entity, e
                        );
                    } else {
                        migrated_relationships += 1;
                    }
                }

                println!(
                    "  [OK] {} - {} ({} updates, {} entities, {} relations)",
                    session_id, name, update_count, entity_count, rel_count
                );
                migrated_sessions += 1;
            }
            Err(e) => {
                println!("  [ERROR] {} - Failed to load: {}", session_id, e);
            }
        }
    }

    // Migrate workspaces
    println!();
    println!("Migrating workspaces...");
    let mut migrated_workspaces = 0;

    for ws in &workspaces {
        let session_ids: Vec<uuid::Uuid> = ws.sessions.iter().map(|(id, _)| *id).collect();
        if let Err(e) = target_storage
            .save_workspace_metadata(ws.id, &ws.name, &ws.description, &session_ids)
            .await
        {
            println!("  [ERROR] {} - {}: {}", ws.id, ws.name, e);
            continue;
        }
        println!(
            "  [OK] {} - {} ({} sessions)",
            ws.id,
            ws.name,
            ws.sessions.len()
        );
        migrated_workspaces += 1;
    }

    // Migrate checkpoints
    println!();
    println!("Migrating checkpoints...");
    let mut migrated_checkpoints = 0;

    for checkpoint in &checkpoints {
        if let Err(e) = target_storage.save_checkpoint(checkpoint).await {
            println!("  [ERROR] {} - {}", checkpoint.id, e);
            continue;
        }
        println!(
            "  [OK] {} (session: {})",
            checkpoint.id, checkpoint.session_id
        );
        migrated_checkpoints += 1;
    }

    // Migrate embeddings
    println!();
    println!("Migrating embeddings...");
    let mut migrated_embeddings = 0;

    use post_cortex_storage::traits::VectorStorage;
    let embeddings = source_storage
        .load_all_embeddings()
        .await
        .map_err(|e| format!("Failed to load embeddings: {}", e))?;

    let total_embeddings = embeddings.len();
    println!("  Found {} embeddings to migrate", total_embeddings);

    for (i, embedding) in embeddings.into_iter().enumerate() {
        let metadata = embedding.to_metadata();
        if let Err(e) = target_storage
            .add_vector(embedding.vector, metadata.clone())
            .await
        {
            println!(
                "  [WARN] Failed to migrate embedding {}: {}",
                metadata.id, e
            );
        } else {
            migrated_embeddings += 1;
        }

        // Progress every 100 embeddings
        if (i + 1) % 100 == 0 {
            println!(
                "  Progress: {}/{} embeddings migrated",
                i + 1,
                total_embeddings
            );
        }
    }
    println!("  [OK] {} embeddings migrated", migrated_embeddings);

    // Summary
    println!();
    println!("Migration complete!");
    println!(
        "  Sessions migrated:      {}/{}",
        migrated_sessions,
        sessions.len()
    );
    println!("  Updates migrated:       {}", migrated_updates);
    println!("  Entities migrated:      {}", migrated_entities);
    println!("  Relationships migrated: {}", migrated_relationships);
    println!(
        "  Workspaces migrated:    {}/{}",
        migrated_workspaces,
        workspaces.len()
    );
    println!(
        "  Checkpoints migrated:   {}/{}",
        migrated_checkpoints,
        checkpoints.len()
    );
    println!(
        "  Embeddings migrated:    {}/{}",
        migrated_embeddings, total_embeddings
    );

    Ok(())
}
