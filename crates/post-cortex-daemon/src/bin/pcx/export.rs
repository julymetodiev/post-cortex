// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx export` — write sessions/workspaces to a (compressed) JSON file.

use std::io::{self, Write};
use std::path::Path;

use crate::admin::{offline_memory, open_offline};
use crate::remote;
use post_cortex_core::archive::{
    ArchiveHeader, ArchiveSize, CompressionType, ExportOptions, ExportScope,
};
use post_cortex_daemon::compose::{Access, OpenedStorage};
use post_cortex_daemon::daemon::DaemonConfig;
use uuid::Uuid;

/// Export what the flags name (a workspace, sessions, or everything) from
/// `storage` into the file at `path`, through the Transfer capability of a
/// memory instance over it (embeddings off: the index gives the archive its
/// vectors, nothing is embedded); the same for every backend. The archive
/// is written to `<path>.partial` and renamed when complete, so a failed
/// export leaves no file that looks whole.
async fn export_scope(
    daemon_config: &DaemonConfig,
    storage: OpenedStorage,
    options: &ExportOptions,
    workspace: Option<String>,
    session: Option<Vec<String>>,
    path: &Path,
) -> Result<(ArchiveHeader, ArchiveSize), String> {
    let offline = offline_memory(daemon_config, storage).await?;
    let scope = if let Some(workspace_id) = workspace {
        let uuid =
            Uuid::parse_str(&workspace_id).map_err(|e| format!("Invalid workspace UUID: {}", e))?;
        println!("Exporting workspace {}...", workspace_id);
        ExportScope::Workspace(uuid)
    } else if let Some(session_ids) = session {
        let uuids = session_ids
            .iter()
            .map(|s| Uuid::parse_str(s).map_err(|e| format!("Invalid session UUID {}: {}", s, e)))
            .collect::<Result<Vec<Uuid>, _>>()?;
        println!("Exporting {} session(s)...", uuids.len());
        ExportScope::Sessions(uuids)
    } else {
        println!("Exporting all data...");
        ExportScope::All
    };
    let exported = write_archive(&offline.memory, scope, options, path).await;
    offline.finish().await;
    exported
}

/// Plan and write the export of `scope` into `path` (through
/// `<path>.partial`).
async fn write_archive(
    memory: &post_cortex_memory::Memory,
    scope: ExportScope,
    options: &ExportOptions,
    path: &Path,
) -> Result<(ArchiveHeader, ArchiveSize), String> {
    let plan = memory
        .transfer
        .plan_export(scope, options.clone())
        .await
        .map_err(|e| format!("Export failed: {e:#}"))?;
    let header = plan.header.clone();
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = std::path::PathBuf::from(partial);
    let file = std::fs::File::create(&partial)
        .map_err(|e| format!("Cannot create {}: {e}", partial.display()))?;
    let written = plan
        .write(Box::new(io::BufWriter::new(file)))
        .await
        .map_err(|e| format!("Export failed: {e:#}"));
    let size = match written {
        Ok(size) => size,
        Err(e) => {
            if let Err(removed) = std::fs::remove_file(&partial) {
                eprintln!("Cannot remove {}: {removed}", partial.display());
            }
            return Err(e);
        }
    };
    std::fs::rename(&partial, path).map_err(|e| {
        format!(
            "Cannot rename {} to {}: {e}",
            partial.display(),
            path.display()
        )
    })?;
    Ok((header, size))
}

pub async fn handle_export(args: post_cortex_cli::cli::ExportArgs) -> Result<(), String> {
    let post_cortex_cli::cli::ExportArgs {
        output,
        compress,
        session,
        workspace,
        checkpoints,
        pretty,
        no_messages,
        force,
    } = args;
    // Generate default filename if not provided
    let output = output.unwrap_or_else(|| {
        let now = chrono::Local::now();
        format!("export-{}.json.gz", now.format("%Y-%m-%d_%H-%M-%S"))
    });

    let path = Path::new(&output);

    // Check if file exists and ask for confirmation
    if path.exists() && !force {
        print!("File '{}' already exists. Overwrite? [y/N] ", output);
        io::stdout()
            .flush()
            .map_err(|e| format!("Cannot write to stdout: {e}"))?;

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

    let daemon_config = DaemonConfig::load().map_err(|e| e.to_string())?;

    // Determine compression
    let compression = if let Some(ref c) = compress {
        // Any case on the command line; the codec name is exact.
        c.to_lowercase()
            .parse::<CompressionType>()
            .map_err(|_| format!("Invalid compression type: {}. Use: none, gzip, zstd", c))?
    } else {
        CompressionType::from_path(Path::new(&output))
    };

    let options = ExportOptions {
        compression,
        include_checkpoints: checkpoints,
        pretty,
        include_messages: !no_messages,
        embedding_model: Some(daemon_config.embedding_model()),
    };

    if let Some(daemon) = remote::daemon(&daemon_config).await? {
        if pretty {
            return Err(
                "--pretty is written offline: stop the daemon, or leave --pretty out".to_string(),
            );
        }
        let request = post_cortex_proto::pb::ExportRequest {
            scope: match (&workspace, &session) {
                (Some(_), _) => "workspace",
                (None, Some(_)) => "session",
                (None, None) => "all",
            }
            .to_string(),
            session_ids: session.unwrap_or_default(),
            workspace_id: workspace.unwrap_or_default(),
            include_checkpoints: checkpoints,
            compression: compression.name().to_string(),
            no_messages,
        };
        return export_through(daemon, request, path).await;
    }

    // Read-only: no lock, schema statement or migration.
    let storage = open_offline(
        &daemon_config,
        &daemon_config.storage_config()?,
        Access::ReadOnly,
    )
    .await?;
    let (header, size) =
        export_scope(&daemon_config, storage, &options, workspace, session, path).await?;
    let counts = &header.metadata;

    println!();
    println!("Export complete!");
    println!("  File:        {}", output);
    println!("  Format:      {}", header.format_version);
    println!("  Compression: {}", compression.name());
    println!("  Sessions:    {}", counts.session_count);
    println!("  Workspaces:  {}", counts.workspace_count);
    println!("  Updates:     {}", counts.update_count);
    println!("  Checkpoints: {}", counts.checkpoint_count);
    println!("  Embeddings:  {}", counts.embedding_count);
    println!("  Messages:    {}", counts.message_count);
    println!("  Cards:       {} versions", counts.card_count);
    println!("  Documents:   {} versions", counts.document_count);

    // Show size info based on compression
    if compression == CompressionType::None || size.json_bytes == 0 {
        println!("  Size:        {} bytes", size.written_bytes);
    } else {
        let compression_ratio =
            (1.0 - (size.written_bytes as f64 / size.json_bytes as f64)) * 100.0;
        println!(
            "  Size:        {} bytes ({:.0}% compression, from {} uncompressed)",
            size.written_bytes, compression_ratio, size.json_bytes
        );
    }

    Ok(())
}

/// Export through the running daemon (`Export`), its chunks written to
/// `path` as they come.
async fn export_through(
    mut daemon: remote::Daemon,
    request: post_cortex_proto::pb::ExportRequest,
    path: &Path,
) -> Result<(), String> {
    println!("Exporting through the running daemon...");
    let mut chunks = daemon
        .export(request)
        .await
        .map_err(|s| format!("Export failed: {}", s.message()))?
        .into_inner();
    let mut file = std::fs::File::create(path)
        .map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
    let mut summary = None;
    while let Some(chunk) = chunks
        .message()
        .await
        .map_err(|s| format!("Export failed: {}", s.message()))?
    {
        file.write_all(&chunk.data)
            .map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
        if chunk.last {
            summary = chunk.summary;
        }
    }
    let summary = summary.ok_or("the daemon ended the export without its summary")?;
    println!();
    println!("Export complete!");
    println!("  File:        {}", path.display());
    println!("  Format:      {}", summary.format_version);
    println!("  Compression: {}", summary.compression);
    println!("  Sessions:    {}", summary.sessions);
    println!("  Workspaces:  {}", summary.workspaces);
    println!("  Updates:     {}", summary.updates);
    println!("  Checkpoints: {}", summary.checkpoints);
    println!("  Embeddings:  {}", summary.embeddings);
    println!("  Messages:    {}", summary.messages);
    println!("  Cards:       {} versions", summary.cards);
    println!("  Size:        {} bytes", summary.total_bytes);
    Ok(())
}

#[cfg(all(test, feature = "surrealdb-storage"))]
mod tests {
    use super::*;
    use post_cortex_core::session::active_session::ActiveSession;
    use post_cortex_core::workspace::SessionRole;
    use post_cortex_ports::VectorMetadata;
    use post_cortex_ports::{SessionStore, VectorStore, WorkspaceStore};
    use post_cortex_storage::SurrealDBStorage;

    /// `pcx export --workspace` against a database the daemon initialised at
    /// its own dimension, holding vectors: the export carries the workspace,
    /// its session and their vectors, and leaves the stored vectors as they
    /// were (0.5.0 opened the database at 384 and dropped the production
    /// index).
    #[tokio::test]
    async fn workspace_export_carries_the_vectors() -> Result<(), String> {
        let daemon = DaemonConfig::default();
        let dim = daemon.vector_dimension();
        let storage = SurrealDBStorage::new_with_dimension("mem://", None, None, None, None, dim)
            .await
            .unwrap();

        let session = Uuid::new_v4();
        let workspace = Uuid::new_v4();
        storage
            .save_session(
                &ActiveSession::new(session, Some("s".into()), None, chrono::Utc::now()),
                post_cortex_ports::storage::SessionChanges::default(),
            )
            .await
            .unwrap();
        storage
            .save_workspace_metadata(workspace, "w", &workspace.to_string(), "", &[session])
            .await
            .unwrap();
        storage
            .add_session_to_workspace(workspace, session, SessionRole::Primary)
            .await
            .unwrap();
        let mut vector = vec![0.0; dim];
        vector[0] = 1.0;
        storage
            .upsert_vectors(vec![(
                vector,
                VectorMetadata {
                    id: "entity:x".into(),
                    text: "x".into(),
                    source: session.to_string(),
                    content_type: "entity".into(),
                    timestamp: chrono::Utc::now(),
                    metadata: Default::default(),
                },
            )])
            .await
            .unwrap();

        let options = ExportOptions {
            compression: CompressionType::None,
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.json");
        let (header, size) = export_scope(
            &daemon,
            OpenedStorage::surrealdb(storage.clone()),
            &options,
            Some(workspace.to_string()),
            None,
            &path,
        )
        .await?;
        assert_eq!(header.metadata.workspace_count, 1);
        assert_eq!(header.metadata.session_count, 1);
        assert_eq!(header.metadata.embedding_count, 1);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), size.written_bytes);
        assert_eq!(storage.total_count().await.unwrap(), 1);
        Ok(())
    }
}
