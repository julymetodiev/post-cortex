// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx import` — load an exported JSON archive back into the active backend.

use std::ops::ControlFlow;
use std::path::Path;

use crate::admin::{offline_memory, open_offline};
use crate::remote;
use post_cortex_core::archive::{ArchiveHeader, ArchivePart, ImportOptions, ImportResult};
use post_cortex_daemon::compose::{Access, OpenedStorage};
use post_cortex_daemon::daemon::DaemonConfig;
use post_cortex_memory::capabilities::{ArchiveFile, ImportPreview};
use post_cortex_ports::ArchiveCodec;
use post_cortex_storage::JsonArchive;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

/// How an import treats what the target has already.
pub struct ImportFlags {
    pub skip_existing: bool,
    pub overwrite: bool,
    pub cards_newer: bool,
}

/// Print the cards whose history diverged from the file's, with both latest
/// versions and what the import did with them.
fn print_card_conflicts(preview: &ImportPreview, options: &ImportOptions) {
    let cards: Vec<_> = preview
        .conflicts
        .iter()
        .filter(|c| c.kind == "card")
        .collect();
    if cards.is_empty() {
        return;
    }
    println!("Cards that differ from the file's ({}):", cards.len());
    for conflict in cards {
        println!("  - {}", conflict.describe());
    }
    if !options.overwrite && !options.cards_newer {
        println!(
            "  (kept here; --cards-newer takes a file card written later, --overwrite takes the file's)"
        );
    }
    println!();
}

/// A session of an archive, as `--list` shows it.
struct Listed {
    id: Uuid,
    name: String,
    records: usize,
}

/// The header of the archive at `path` and its sessions, read part by part.
fn list_archive(path: &Path) -> Result<(ArchiveHeader, Vec<Listed>), String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut header = None;
    let mut sessions = Vec::new();
    JsonArchive
        .read(&mut file, &mut |part| match part {
            ArchivePart::Header(h) => {
                header = Some(*h);
                ControlFlow::Continue(())
            }
            ArchivePart::Session(s) => {
                let name = s
                    .session
                    .session
                    .metadata()
                    .name
                    .clone()
                    .unwrap_or_else(|| "Unnamed".to_string());
                sessions.push(Listed {
                    id: s.session.session.id(),
                    name,
                    records: s.record_count(),
                });
                ControlFlow::Continue(())
            }
            // The sessions are all that is listed.
            _ => ControlFlow::Break(()),
        })
        .map_err(|e| format!("Failed to read export: {e}"))?;
    let header = header.ok_or("Failed to read export: no header")?;
    Ok((header, sessions))
}

pub async fn handle_import(
    input: String,
    session: Option<Vec<String>>,
    workspace: Option<String>,
    flags: ImportFlags,
    list: bool,
) -> Result<(), String> {
    let ImportFlags {
        skip_existing,
        overwrite,
        cards_newer,
    } = flags;
    let path = Path::new(&input);

    if !path.exists() {
        return Err(format!("File not found: {}", input));
    }

    // List mode - just show contents
    if list {
        println!("Reading export file: {}", input);
        println!();

        let (header, sessions) = list_archive(path)?;
        let metadata = header.metadata;

        println!("Export Metadata:");
        println!("  Format Version:  {}", metadata.post_cortex_version);
        println!("  Exported At:     {}", metadata.exported_at);
        println!("  Export Type:     {:?}", metadata.export_type);
        println!("  Sessions:        {}", metadata.session_count);
        println!("  Workspaces:      {}", metadata.workspace_count);
        println!("  Updates:         {}", metadata.update_count);
        println!("  Checkpoints:     {}", metadata.checkpoint_count);
        println!("  Embeddings:      {}", metadata.embedding_count);
        println!("  Messages:        {}", metadata.message_count);
        println!("  Card versions:   {}", metadata.card_count);
        println!("  Doc versions:    {}", metadata.document_count);
        println!();

        if !sessions.is_empty() {
            println!("Sessions in export:");
            println!("{:<38} {:<30} Updates", "ID", "Name");
            println!("{:-<38} {:-<30} {:-<10}", "", "", "");
            for session in sessions {
                println!(
                    "{:<38} {:<30} {}",
                    session.id, session.name, session.records
                );
            }
        }

        return Ok(());
    }

    // Import mode
    let daemon_config = DaemonConfig::load().map_err(|e| e.to_string())?;
    if let Some(daemon) = remote::daemon(&daemon_config).await? {
        let settings = post_cortex_proto::pb::ImportSettings {
            commit: true,
            skip_existing,
            overwrite,
            session_ids: session.unwrap_or_default(),
            workspace_ids: workspace.into_iter().collect(),
            cards_newer,
        };
        return import_through(daemon, path, settings).await;
    }
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

    println!("Initializing import...");
    let options = ImportOptions {
        session_filter,
        workspace_filter,
        skip_existing,
        overwrite,
        cards_newer,
        embedding_model: Some(daemon_config.embedding_model()),
    };

    let storage = open_offline(
        &daemon_config,
        &daemon_config.storage_config()?,
        Access::ReadWrite,
    )
    .await?;
    println!("Importing {}...", input);
    let (preview, result) =
        import_into(&daemon_config, storage, &ArchiveFile::at(path), &options).await?;
    println!("  Format:      {}", preview.format_version);
    println!("  Sessions:    {}", preview.sessions);
    println!("  Workspaces:  {}", preview.workspaces);
    println!();
    print_card_conflicts(&preview, &options);
    print_result(&result);
    Ok(())
}

/// Import `archive` into `storage` through the Transfer capability of a
/// memory instance over it (embeddings off: the archive's vectors go into
/// the index, nothing is embedded).
async fn import_into(
    daemon_config: &DaemonConfig,
    storage: OpenedStorage,
    archive: &ArchiveFile,
    options: &ImportOptions,
) -> Result<(ImportPreview, ImportResult), String> {
    let offline = offline_memory(daemon_config, storage).await?;
    let imported = offline
        .memory
        .transfer
        .import(archive, options.clone())
        .await
        .map_err(|e| format!("Import failed: {}", e));
    offline.finish().await;
    imported
}

/// Bytes of the archive sent per chunk.
const CHUNK: usize = 1 << 20;

/// Import through the running daemon (`Import`): the file in chunks, the
/// settings with the first.
async fn import_through(
    mut daemon: remote::Daemon,
    path: &Path,
    settings: post_cortex_proto::pb::ImportSettings,
) -> Result<(), String> {
    println!("Importing {} through the running daemon...", path.display());
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    // The file in chunks as it is read, the settings with the first (sent
    // even for an empty file, which the daemon refuses). A read error ends
    // the stream early: the daemon then refuses the cut archive before
    // writing anything.
    let chunks =
        futures::stream::unfold((file, Some(settings)), |(mut file, settings)| async move {
            let mut data = vec![0; CHUNK];
            let n = match file.read(&mut data).await {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("Cannot read the archive: {e}");
                    0
                }
            };
            if n == 0 && settings.is_none() {
                return None;
            }
            data.truncate(n);
            let chunk = post_cortex_proto::pb::ImportChunk { data, settings };
            Some((chunk, (file, None)))
        });
    let response = daemon
        .import(chunks)
        .await
        .map_err(|s| format!("Import failed: {}", s.message()))?
        .into_inner();
    let cards: Vec<_> = response
        .conflicts
        .iter()
        .filter(|c| c.kind == "card")
        .collect();
    if !cards.is_empty() {
        println!("Cards that differ from the file's ({}):", cards.len());
        for card in cards {
            println!("  - {} {}: {}", card.kind, card.id, card.resolution);
        }
        println!();
    }
    for warning in &response.warnings {
        println!("Warning: {warning}");
    }
    println!();
    println!("Import complete!");
    println!("  Sessions imported:   {}", response.sessions_imported);
    println!("  Sessions skipped:    {}", response.sessions_skipped);
    println!("  Workspaces imported: {}", response.workspaces_imported);
    println!("  Workspaces skipped:  {}", response.workspaces_skipped);
    println!("  Updates imported:    {}", response.updates_imported);
    println!("  Checkpoints:         {}", response.checkpoints_imported);
    println!("  Embeddings:          {}", response.embeddings_imported);
    println!(
        "  Messages imported:   {} ({} already there)",
        response.messages_imported, response.messages_skipped
    );
    println!(
        "  Cards imported:      {} ({} left as they were)",
        response.cards_imported, response.cards_skipped
    );
    println!(
        "  Documents imported:  {} ({} left as they were)",
        response.documents_imported, response.documents_skipped
    );
    if !response.errors.is_empty() {
        println!();
        println!("Errors ({}):", response.errors.len());
        for err in &response.errors {
            println!("  - {}", err);
        }
    }
    Ok(())
}

/// Print what an import did.
fn print_result(result: &ImportResult) {
    println!();
    println!("Import complete!");
    println!("  Sessions imported:   {}", result.sessions_imported);
    println!("  Sessions skipped:    {}", result.sessions_skipped);
    println!("  Workspaces imported: {}", result.workspaces_imported);
    println!("  Workspaces skipped:  {}", result.workspaces_skipped);
    println!("  Updates imported:    {}", result.updates_imported);
    println!("  Checkpoints:         {}", result.checkpoints_imported);
    println!("  Embeddings:          {}", result.embeddings_imported);
    println!(
        "  Messages imported:   {} ({} already there)",
        result.messages_imported, result.messages_skipped
    );
    println!(
        "  Cards imported:      {} ({} left as they were)",
        result.cards_imported, result.cards_skipped
    );
    println!(
        "  Documents imported:  {} ({} left as they were)",
        result.documents_imported, result.documents_skipped
    );

    if !result.errors.is_empty() {
        println!();
        println!("Errors ({}):", result.errors.len());
        for err in &result.errors {
            println!("  - {}", err);
        }
    }
}
