// Copyright (c) 2025, 2026 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

//! SurrealDB-specific export/import paths.
//!
//! These methods live on [`SurrealDBStorage`] as inherent (non-trait) methods
//! because the RocksDB backend goes through `storage::export_import` instead —
//! see `pcx export` and `pcx migrate` in `bin/pcx.rs` for the entry points.

use anyhow::Result;
use tracing::info;
use uuid::Uuid;

use crate::traits::Storage;

use super::SurrealDBStorage;
use super::records::EmbeddingRecord;

impl SurrealDBStorage {
    /// Import data from an ExportData structure
    pub async fn import_data(
        &self,
        data: crate::export_import::ExportData,
        options: &crate::export_import::ImportOptions,
    ) -> Result<crate::export_import::ImportResult> {
        use crate::export_import::{ImportResult, SUPPORTED_IMPORT_VERSIONS};

        info!(
            "SurrealDBStorage: Starting import: {} sessions, {} workspaces",
            data.sessions.len(),
            data.workspaces.len()
        );

        // Check format version compatibility
        if !SUPPORTED_IMPORT_VERSIONS.contains(&data.format_version.as_str()) {
            return Err(anyhow::anyhow!(
                "Incompatible export format version: {}. Supported: {:?}",
                data.format_version,
                SUPPORTED_IMPORT_VERSIONS
            ));
        }

        let mut result = ImportResult::default();

        // Import sessions
        for exported_session in data.sessions {
            let session_id = exported_session.session.id();

            // Apply filter if specified
            if let Some(ref filter) = options.session_filter {
                if !filter.contains(&session_id) {
                    continue;
                }
            }

            // Check if session exists
            let exists = self.session_exists(session_id).await?;

            if exists {
                if options.skip_existing {
                    result.sessions_skipped += 1;
                    continue;
                } else if !options.overwrite {
                    result.errors.push(format!(
                        "Session {} already exists (use --overwrite or --skip-existing)",
                        session_id
                    ));
                    continue;
                }
            }

            // Import session
            match self.save_session(&exported_session.session).await {
                Ok(()) => {
                    result.sessions_imported += 1;

                    // Import updates
                    if !exported_session.updates.is_empty() {
                        match self
                            .batch_save_updates(session_id, exported_session.updates.clone())
                            .await
                        {
                            Ok(()) => {
                                result.updates_imported += exported_session.updates.len();
                            }
                            Err(e) => {
                                result.errors.push(format!(
                                    "Failed to import updates for session {}: {}",
                                    session_id, e
                                ));
                            }
                        }
                    }
                }
                Err(e) => {
                    result
                        .errors
                        .push(format!("Failed to import session {}: {}", session_id, e));
                }
            }
        }

        // Import workspaces
        for workspace in data.workspaces {
            // Apply filter if specified
            if let Some(ref filter) = options.workspace_filter {
                if !filter.contains(&workspace.id) {
                    continue;
                }
            }

            // Check if workspace exists
            let existing = self.list_workspaces().await?;
            let exists = existing.iter().any(|w| w.id == workspace.id);

            if exists {
                if options.skip_existing {
                    result.workspaces_skipped += 1;
                    continue;
                } else if !options.overwrite {
                    result.errors.push(format!(
                        "Workspace {} already exists (use --overwrite or --skip-existing)",
                        workspace.id
                    ));
                    continue;
                } else {
                    // Delete existing workspace before reimporting
                    self.delete_workspace(workspace.id).await?;
                }
            }

            // Import workspace
            let session_ids: Vec<Uuid> = workspace.sessions.iter().map(|(id, _)| *id).collect();
            match self
                .save_workspace_metadata(
                    workspace.id,
                    &workspace.name,
                    &workspace.description,
                    &session_ids,
                )
                .await
            {
                Ok(()) => {
                    // Add session associations with roles
                    for (session_id, role) in &workspace.sessions {
                        if let Err(e) = self
                            .add_session_to_workspace(workspace.id, *session_id, *role)
                            .await
                        {
                            result.errors.push(format!(
                                "Failed to add session {} to workspace {}: {}",
                                session_id, workspace.id, e
                            ));
                        }
                    }
                    result.workspaces_imported += 1;
                }
                Err(e) => {
                    result.errors.push(format!(
                        "Failed to import workspace {}: {}",
                        workspace.id, e
                    ));
                }
            }
        }

        // Import checkpoints
        for checkpoint in data.checkpoints {
            match self.save_checkpoint(&checkpoint).await {
                Ok(()) => result.checkpoints_imported += 1,
                Err(e) => {
                    result.errors.push(format!(
                        "Failed to import checkpoint {}: {}",
                        checkpoint.id, e
                    ));
                }
            }
        }

        // Import embeddings (v1.2.0+)
        if !data.embeddings.is_empty() {
            match self.import_embeddings(&data.embeddings).await {
                Ok(count) => result.embeddings_imported = count,
                Err(e) => {
                    result
                        .errors
                        .push(format!("Failed to import embeddings: {}", e));
                }
            }
        }

        info!(
            "SurrealDBStorage: Import complete: {} sessions, {} workspaces, {} updates, {} embeddings, {} errors",
            result.sessions_imported,
            result.workspaces_imported,
            result.updates_imported,
            result.embeddings_imported,
            result.errors.len()
        );

        Ok(result)
    }

    /// Export all data from the database
    pub async fn export_full(
        &self,
        options: &crate::export_import::ExportOptions,
    ) -> Result<crate::export_import::ExportData> {
        use crate::export_import::{ExportData, ExportType, ExportedWorkspace};

        info!("SurrealDBStorage: Starting full database export");

        let mut export = ExportData::new(ExportType::Full, options.compression);

        // Export all sessions
        let session_ids = self.list_sessions().await?;
        info!("SurrealDBStorage: Exporting {} sessions", session_ids.len());

        for session_id in session_ids {
            info!("SurrealDBStorage: Processing session {}...", session_id);
            match self.export_session_data(session_id).await {
                Ok(session_data) => export.sessions.push(session_data),
                Err(e) => {
                    info!(
                        "SurrealDBStorage: Warning: Failed to export session {}: {}",
                        session_id, e
                    );
                }
            }
        }

        // Export all workspaces
        let workspaces = self.list_workspaces().await?;
        info!(
            "SurrealDBStorage: Exporting {} workspaces",
            workspaces.len()
        );
        export.workspaces = workspaces
            .into_iter()
            .map(ExportedWorkspace::from)
            .collect();

        // Export checkpoints if requested
        if options.include_checkpoints {
            export.checkpoints = self.list_checkpoints().await?;
            info!(
                "SurrealDBStorage: Exported {} checkpoints",
                export.checkpoints.len()
            );
        }

        // Export all embeddings
        export.embeddings = self.export_all_embeddings().await?;
        info!(
            "SurrealDBStorage: Exported {} embeddings",
            export.embeddings.len()
        );

        export.update_counts();
        info!(
            "SurrealDBStorage: Export complete: {} sessions, {} workspaces, {} updates, {} embeddings",
            export.metadata.session_count,
            export.metadata.workspace_count,
            export.metadata.update_count,
            export.metadata.embedding_count
        );

        Ok(export)
    }

    /// Export specific sessions
    pub async fn export_sessions(
        &self,
        session_ids: Vec<Uuid>,
        options: &crate::export_import::ExportOptions,
    ) -> Result<crate::export_import::ExportData> {
        use crate::export_import::{ExportData, ExportType};

        info!(
            "SurrealDBStorage: Starting selective export of {} sessions",
            session_ids.len()
        );

        let mut export = ExportData::new(
            ExportType::SelectiveSessions {
                session_ids: session_ids.clone(),
            },
            options.compression,
        );

        for session_id in &session_ids {
            match self.export_session_data(*session_id).await {
                Ok(session_data) => export.sessions.push(session_data),
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Failed to export session {}: {}",
                        session_id,
                        e
                    ));
                }
            }
        }

        // Export embeddings for these sessions
        export.embeddings = self.export_session_embeddings(&session_ids).await?;

        export.update_counts();
        Ok(export)
    }

    /// Export a workspace and all its sessions
    pub async fn export_workspace(
        &self,
        workspace_id: Uuid,
        options: &crate::export_import::ExportOptions,
    ) -> Result<crate::export_import::ExportData> {
        use crate::export_import::{ExportData, ExportType, ExportedWorkspace};

        info!(
            "SurrealDBStorage: Starting workspace export for {}",
            workspace_id
        );

        let mut export = ExportData::new(
            ExportType::SelectiveWorkspace { workspace_id },
            options.compression,
        );

        // Find the workspace
        let workspaces = self.list_workspaces().await?;
        let workspace = workspaces
            .into_iter()
            .find(|w| w.id == workspace_id)
            .ok_or_else(|| anyhow::anyhow!("Workspace {} not found", workspace_id))?;

        // Export the workspace
        export
            .workspaces
            .push(ExportedWorkspace::from(workspace.clone()));

        // Export all sessions in the workspace
        let session_ids: Vec<Uuid> = workspace.sessions.iter().map(|(id, _)| *id).collect();
        for session_id in &session_ids {
            match self.export_session_data(*session_id).await {
                Ok(session_data) => export.sessions.push(session_data),
                Err(e) => {
                    info!(
                        "SurrealDBStorage: Warning: Failed to export session {} from workspace: {}",
                        session_id, e
                    );
                }
            }
        }

        // Export embeddings for workspace sessions
        export.embeddings = self.export_session_embeddings(&session_ids).await?;

        export.update_counts();
        Ok(export)
    }

    /// Export a single session with its updates
    async fn export_session_data(
        &self,
        session_id: Uuid,
    ) -> Result<crate::export_import::ExportedSession> {
        use crate::export_import::ExportedSession;

        let session = self.load_session(session_id).await?;
        let updates = self.load_session_updates(session_id).await?;

        Ok(ExportedSession { session, updates })
    }

    /// Export all embeddings from the database
    async fn export_all_embeddings(
        &self,
    ) -> Result<Vec<crate::export_import::ExportedEmbedding>> {
        use crate::export_import::ExportedEmbedding;

        let mut all_embeddings = Vec::new();
        let mut start = 0;
        let limit = 500; // Embeddings are large, use smaller batch size

        loop {
            let mut response = self
                .db
                .query("SELECT * FROM embedding LIMIT $limit START $start")
                .bind(("limit", limit))
                .bind(("start", start))
                .await?;

            let records: Vec<EmbeddingRecord> = response.take(0)?;
            let count = records.len();

            if count == 0 {
                break;
            }

            for r in records {
                all_embeddings.push(ExportedEmbedding {
                    content_id: r.content_id,
                    session_id: r.session_id,
                    vector: r.vector,
                    text: r.text,
                    content_type: r.content_type,
                    timestamp: r.timestamp,
                    metadata: r.metadata,
                });
            }

            if all_embeddings.len() % 500 == 0 {
                info!(
                    "SurrealDBStorage: Exported {} embeddings...",
                    all_embeddings.len()
                );
            }

            if count < limit {
                break;
            }

            start += count;
        }

        Ok(all_embeddings)
    }

    /// Export embeddings for specific sessions
    async fn export_session_embeddings(
        &self,
        session_ids: &[Uuid],
    ) -> Result<Vec<crate::export_import::ExportedEmbedding>> {
        use crate::export_import::ExportedEmbedding;

        let session_id_strs: Vec<String> = session_ids.iter().map(|id| id.to_string()).collect();
        let mut all_embeddings = Vec::new();
        let mut start = 0;
        let limit = 500;

        loop {
            let mut response = self
                .db
                .query("SELECT * FROM embedding WHERE session_id IN $session_ids LIMIT $limit START $start")
                .bind(("session_ids", session_id_strs.clone()))
                .bind(("limit", limit))
                .bind(("start", start))
                .await?;

            let records: Vec<EmbeddingRecord> = response.take(0)?;
            let count = records.len();

            if count == 0 {
                break;
            }

            for r in records {
                all_embeddings.push(ExportedEmbedding {
                    content_id: r.content_id,
                    session_id: r.session_id,
                    vector: r.vector,
                    text: r.text,
                    content_type: r.content_type,
                    timestamp: r.timestamp,
                    metadata: r.metadata,
                });
            }

            if all_embeddings.len() % 500 == 0 {
                info!(
                    "SurrealDBStorage: Exported {} session embeddings...",
                    all_embeddings.len()
                );
            }

            if count < limit {
                break;
            }

            start += count;
        }

        Ok(all_embeddings)
    }

    /// Import embeddings from export data
    async fn import_embeddings(
        &self,
        embeddings: &[crate::export_import::ExportedEmbedding],
    ) -> Result<usize> {
        let mut count = 0;
        for emb in embeddings {
            let record = EmbeddingRecord {
                content_id: emb.content_id.clone(),
                session_id: emb.session_id.clone(),
                vector: emb.vector.clone(),
                text: emb.text.clone(),
                content_type: emb.content_type.clone(),
                timestamp: emb.timestamp.clone(),
                metadata: emb.metadata.clone(),
            };

            let _: Option<EmbeddingRecord> = self
                .db
                .create(("embedding", emb.content_id.as_str()))
                .content(record)
                .await?;
            count += 1;
        }
        Ok(count)
    }
}
