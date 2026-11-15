// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Archive export and import over `pcx.v1` (`Export`, `Import`), the one
//! mapping both the gRPC service and the browser transfer
//! ([`crate::browser::transfer`]) use: an export's chunks are sent as the
//! archive is written, an import's are spooled as they arrive (the Transfer
//! capability reads the spooled archive twice). The work is the
//! capability's; these translate the wire types.

use std::io::Write;
use std::pin::Pin;

use futures::{Stream, StreamExt};
use post_cortex_core::archive::{
    ArchiveHeader, ArchiveSize, CompressionType, ExportOptions, ExportScope, ImportOptions,
};
use post_cortex_memory::Memory;
use post_cortex_memory::capabilities::{CardHead, ImportPreview};
use post_cortex_proto::pb;
use tokio::sync::mpsc;
use tonic::Status;
use uuid::Uuid;

use crate::grpc::status;
use crate::views::request::uuid;

/// Bytes of archive data per [`pb::ExportChunk`].
const EXPORT_CHUNK_BYTES: usize = 1 << 20;

/// Chunks of an export waiting for the client.
const CHUNKS_BUFFERED: usize = 4;

/// The chunks of an export.
pub type ExportStream = Pin<Box<dyn Stream<Item = Result<pb::ExportChunk, Status>> + Send>>;

/// The archive's bytes as [`pb::ExportChunk`]s: written on the codec's
/// thread, sent when a chunk is full and at the end.
struct ChunkWriter {
    chunks: mpsc::Sender<Result<pb::ExportChunk, Status>>,
    data: Vec<u8>,
}

impl ChunkWriter {
    fn send(&mut self) -> std::io::Result<()> {
        if self.data.is_empty() {
            return Ok(());
        }
        let chunk = pb::ExportChunk {
            data: std::mem::replace(&mut self.data, Vec::with_capacity(EXPORT_CHUNK_BYTES)),
            last: false,
            summary: None,
        };
        self.chunks.blocking_send(Ok(chunk)).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the export's client left")
        })
    }
}

impl Write for ChunkWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.data.extend_from_slice(buf);
        if self.data.len() >= EXPORT_CHUNK_BYTES {
            self.send()?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.send()
    }
}

/// The scope and options of an export request (the instance's own model).
///
/// # Errors
/// `InvalidArgument` for an unknown scope or compression, a bad id, or a
/// session scope without sessions.
pub fn export_parts(req: &pb::ExportRequest) -> Result<(ExportScope, ExportOptions), Status> {
    let scope = match req.scope.trim().to_ascii_lowercase().as_str() {
        "" | "all" => ExportScope::All,
        "session" | "sessions" => {
            if req.session_ids.is_empty() {
                return Err(Status::invalid_argument(
                    "scope 'session' needs session_ids",
                ));
            }
            ExportScope::Sessions(
                req.session_ids
                    .iter()
                    .map(|id| uuid(id, "session_ids"))
                    .collect::<Result<_, _>>()?,
            )
        }
        "workspace" => ExportScope::Workspace(uuid(&req.workspace_id, "workspace_id")?),
        other => {
            return Err(Status::invalid_argument(format!(
                "scope must be all, session or workspace, got '{other}'"
            )));
        }
    };
    let compression = match req.compression.trim() {
        "" => CompressionType::None,
        raw => raw.to_lowercase().parse::<CompressionType>().map_err(|_| {
            Status::invalid_argument(format!(
                "compression must be none, gzip or zstd, got '{raw}'"
            ))
        })?,
    };
    let options = ExportOptions {
        compression,
        include_checkpoints: req.include_checkpoints,
        pretty: false,
        include_messages: !req.no_messages,
        embedding_model: None,
    };
    Ok((scope, options))
}

/// The options of an import's settings (the instance's own model).
///
/// # Errors
/// `InvalidArgument` for a bad id, or both `skip_existing` and `overwrite`.
pub fn import_options(settings: &pb::ImportSettings) -> Result<ImportOptions, Status> {
    if settings.skip_existing && settings.overwrite {
        return Err(Status::invalid_argument(
            "skip_existing and overwrite are exclusive",
        ));
    }
    let ids = |raw: &[String], what: &str| -> Result<Option<Vec<Uuid>>, Status> {
        if raw.is_empty() {
            return Ok(None);
        }
        raw.iter()
            .map(|id| uuid(id, what))
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    };
    Ok(ImportOptions {
        session_filter: ids(&settings.session_ids, "session_ids")?,
        workspace_filter: ids(&settings.workspace_ids, "workspace_ids")?,
        skip_existing: settings.skip_existing,
        overwrite: settings.overwrite,
        cards_newer: settings.cards_newer,
        embedding_model: None,
    })
}

/// The summary the last chunk carries.
fn summary(
    header: &ArchiveHeader,
    size: ArchiveSize,
    compression: CompressionType,
) -> pb::ExportSummary {
    let counts = &header.metadata;
    pb::ExportSummary {
        format_version: header.format_version.clone(),
        sessions: counts.session_count as u32,
        workspaces: counts.workspace_count as u32,
        updates: counts.update_count as u32,
        checkpoints: counts.checkpoint_count as u32,
        embeddings: counts.embedding_count as u32,
        total_bytes: size.written_bytes,
        compression: compression.name().to_string(),
        messages: counts.message_count as u32,
        cards: counts.card_count as u32,
        documents: counts.document_count as u32,
    }
}

/// The export of `req`: planned (an unknown scope fails here, before any
/// chunk), then written as the client reads; the last chunk carries the
/// summary and no data.
///
/// # Errors
/// The request is invalid, or the export cannot be planned.
pub async fn export(memory: &Memory, req: &pb::ExportRequest) -> Result<ExportStream, Status> {
    let (scope, options) = export_parts(req)?;
    let compression = options.compression;
    let plan = memory
        .transfer
        .plan_export(scope, options)
        .await
        .map_err(status)?;
    let header = plan.header.clone();
    let (chunks, rx) = mpsc::channel(CHUNKS_BUFFERED);
    let out = ChunkWriter {
        chunks: chunks.clone(),
        data: Vec::with_capacity(EXPORT_CHUNK_BYTES),
    };
    tokio::spawn(async move {
        let last = match plan.write(Box::new(out)).await {
            Ok(size) => Ok(pb::ExportChunk {
                data: Vec::new(),
                last: true,
                summary: Some(summary(&header, size, compression)),
            }),
            Err(e) => Err(status(e)),
        };
        if chunks.send(last).await.is_err() {
            tracing::debug!("the export's client left before its end");
        }
    });
    Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|chunk| (chunk, rx))
    })))
}

fn preview_response(preview: ImportPreview, committed: bool) -> pb::ImportResponse {
    let head = |h: &CardHead| pb::ImportCardHead {
        version: h.version,
        updated_at_unix: h.updated_at.timestamp(),
        first_line: h.first_line.clone(),
    };
    pb::ImportResponse {
        committed,
        format_version: preview.format_version,
        sessions: preview.sessions as u32,
        workspaces: preview.workspaces as u32,
        updates: preview.updates as u32,
        checkpoints: preview.checkpoints as u32,
        embeddings: preview.embeddings as u32,
        messages: preview.messages as u32,
        messages_new: preview.messages_new as u32,
        cards: preview.cards as u32,
        documents: preview.documents as u32,
        warnings: preview.warnings,
        conflicts: preview
            .conflicts
            .into_iter()
            .map(|c| pb::ImportConflict {
                kind: c.kind.to_string(),
                id: c.id.to_string(),
                name: c.name,
                resolution: c.resolution.as_str().to_string(),
                target: c.card.as_ref().map(|h| head(&h.target)),
                archive: c.card.as_ref().map(|h| head(&h.archive)),
            })
            .collect(),
        ..Default::default()
    }
}

/// Spool the `chunks` of an import (the settings on the first), then
/// preview it or (with `settings.commit`) write it.
///
/// # Errors
/// The settings are invalid, the archive is too large, empty or not
/// readable, or the import failed.
pub async fn import<S>(memory: &Memory, mut chunks: S) -> Result<pb::ImportResponse, Status>
where
    S: Stream<Item = Result<pb::ImportChunk, Status>> + Unpin,
{
    let mut settings: Option<pb::ImportSettings> = None;
    let mut spool = memory.transfer.spool().map_err(status)?;
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk?;
        if settings.is_none() {
            settings = Some(chunk.settings.unwrap_or_default());
        }
        spool.append(&chunk.data).await.map_err(status)?;
    }
    let settings = settings.unwrap_or_default();
    let options = import_options(&settings)?;
    let archive = spool.finish().await.map_err(status)?;
    if !settings.commit {
        let preview = memory
            .transfer
            .preview_import(&archive, &options)
            .await
            .map_err(status)?;
        return Ok(preview_response(preview, false));
    }
    let (preview, result) = memory
        .transfer
        .import(&archive, options)
        .await
        .map_err(status)?;
    Ok(pb::ImportResponse {
        sessions_imported: result.sessions_imported as u32,
        sessions_skipped: result.sessions_skipped as u32,
        workspaces_imported: result.workspaces_imported as u32,
        workspaces_skipped: result.workspaces_skipped as u32,
        updates_imported: result.updates_imported as u32,
        checkpoints_imported: result.checkpoints_imported as u32,
        embeddings_imported: result.embeddings_imported as u32,
        messages_imported: result.messages_imported as u32,
        messages_skipped: result.messages_skipped as u32,
        cards_imported: result.cards_imported as u32,
        cards_skipped: result.cards_skipped as u32,
        documents_imported: result.documents_imported as u32,
        documents_skipped: result.documents_skipped as u32,
        errors: result.errors,
        ..preview_response(preview, true)
    })
}
