// Copyright (c) 2025 Julius ML
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
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Export/Import functionality for Post-Cortex data
//!
//! Supports:
//! - Full database export/import
//! - Selective export by session or workspace
//! - Compression (none, gzip, zstd)
//! - Versioned format for forward compatibility

use crate::core::context_update::ContextUpdate;
use crate::session::active_session::ActiveSession;
use crate::storage::rocksdb_storage::{RealRocksDBStorage, SessionCheckpoint, StoredWorkspace};
use crate::workspace::SessionRole;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression as GzCompression;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use tracing::info;
use uuid::Uuid;

/// Current export format version
/// v1.0.0 - Initial format (RelationType only in graph edges)
/// v1.1.0 - Added EdgeData with context field in graph edges
/// v1.2.0 - Added embeddings export/import
pub const EXPORT_FORMAT_VERSION: &str = "1.2.0";

/// Supported legacy versions for import
pub const SUPPORTED_IMPORT_VERSIONS: &[&str] = &["1.0.0", "1.1.0", "1.2.0"];

/// Schema URL for validation (GitHub raw URL)
pub const EXPORT_SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/JuliusDeGroot/post-cortex/main/schemas/export/v1.2.0.json";

/// Compression type for export
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompressionType {
    #[default]
    None,
    Gzip,
    Zstd,
}

impl CompressionType {
    /// Get file extension for this compression type
    pub fn extension(&self) -> &'static str {
        match self {
            CompressionType::None => "json",
            CompressionType::Gzip => "json.gz",
            CompressionType::Zstd => "json.zst",
        }
    }

    /// Detect compression type from file path
    pub fn from_path(path: &Path) -> Self {
        let path_str = path.to_string_lossy().to_lowercase();
        if path_str.ends_with(".json.gz") || path_str.ends_with(".gz") {
            CompressionType::Gzip
        } else if path_str.ends_with(".json.zst") || path_str.ends_with(".zst") {
            CompressionType::Zstd
        } else {
            CompressionType::None
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "none" | "json" => Some(CompressionType::None),
            "gzip" | "gz" => Some(CompressionType::Gzip),
            "zstd" | "zst" => Some(CompressionType::Zstd),
            _ => None,
        }
    }
}

/// Export type indicator
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportType {
    Full,
    SelectiveSessions { session_ids: Vec<Uuid> },
    SelectiveWorkspace { workspace_id: Uuid },
}

/// Metadata about the export
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportMetadata {
    /// When the export was created
    pub exported_at: DateTime<Utc>,
    /// Version of post-cortex that created this export
    pub post_cortex_version: String,
    /// Type of export (full or selective)
    pub export_type: ExportType,
    /// Compression used (for reference, actual compression is in the file)
    pub compression: CompressionType,
    /// Total number of sessions exported
    pub session_count: usize,
    /// Total number of workspaces exported
    pub workspace_count: usize,
    /// Total number of updates exported
    pub update_count: usize,
    /// Total number of checkpoints exported
    pub checkpoint_count: usize,
    /// Total number of embeddings exported (v1.2.0+)
    #[serde(default)]
    pub embedding_count: usize,
}

/// Exported session data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedSession {
    /// The full session data
    pub session: ActiveSession,
    /// Updates associated with this session (stored separately in RocksDB)
    pub updates: Vec<ContextUpdate>,
}

/// Exported workspace data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedWorkspace {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub sessions: Vec<(Uuid, SessionRole)>,
    pub created_at: u64,
}

/// Exported embedding data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedEmbedding {
    pub content_id: String,
    pub session_id: String,
    pub vector: Vec<f32>,
    pub text: String,
    pub content_type: String,
    pub timestamp: String,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
}

impl From<StoredWorkspace> for ExportedWorkspace {
    fn from(ws: StoredWorkspace) -> Self {
        Self {
            id: ws.id,
            name: ws.name,
            description: ws.description,
            sessions: ws.sessions,
            created_at: ws.created_at,
        }
    }
}

impl From<ExportedWorkspace> for StoredWorkspace {
    fn from(ws: ExportedWorkspace) -> Self {
        Self {
            id: ws.id,
            name: ws.name,
            description: ws.description,
            sessions: ws.sessions,
            created_at: ws.created_at,
        }
    }
}

/// Main export data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportData {
    /// Format version for compatibility checking
    pub format_version: String,
    /// Schema URL for validation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Export metadata
    pub metadata: ExportMetadata,
    /// Exported sessions with their updates
    pub sessions: Vec<ExportedSession>,
    /// Exported workspaces
    pub workspaces: Vec<ExportedWorkspace>,
    /// Exported checkpoints
    pub checkpoints: Vec<SessionCheckpoint>,
    /// Exported embeddings (v1.2.0+)
    #[serde(default)]
    pub embeddings: Vec<ExportedEmbedding>,
}

impl ExportData {
    /// Create a new empty export data structure
    pub fn new(export_type: ExportType, compression: CompressionType) -> Self {
        Self {
            format_version: EXPORT_FORMAT_VERSION.to_string(),
            schema: Some(EXPORT_SCHEMA_URL.to_string()),
            metadata: ExportMetadata {
                exported_at: Utc::now(),
                post_cortex_version: env!("CARGO_PKG_VERSION").to_string(),
                export_type,
                compression,
                session_count: 0,
                workspace_count: 0,
                update_count: 0,
                checkpoint_count: 0,
                embedding_count: 0,
            },
            sessions: Vec::new(),
            workspaces: Vec::new(),
            checkpoints: Vec::new(),
            embeddings: Vec::new(),
        }
    }

    /// Update metadata counts based on current data
    pub fn update_counts(&mut self) {
        self.metadata.session_count = self.sessions.len();
        self.metadata.workspace_count = self.workspaces.len();
        self.metadata.update_count = self.sessions.iter().map(|s| s.updates.len()).sum();
        self.metadata.checkpoint_count = self.checkpoints.len();
        self.metadata.embedding_count = self.embeddings.len();
    }

    /// Get list of session IDs in this export
    pub fn session_ids(&self) -> Vec<Uuid> {
        self.sessions.iter().map(|s| s.session.id()).collect()
    }

    /// Get list of workspace IDs in this export
    pub fn workspace_ids(&self) -> Vec<Uuid> {
        self.workspaces.iter().map(|w| w.id).collect()
    }
}

/// Options for export operation
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// Compression type
    pub compression: CompressionType,
    /// Include checkpoints in export
    pub include_checkpoints: bool,
    /// Pretty print JSON (only for uncompressed)
    pub pretty: bool,
}

/// Options for import operation
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Only import specific session IDs (None = import all)
    pub session_filter: Option<Vec<Uuid>>,
    /// Only import specific workspace IDs (None = import all)
    pub workspace_filter: Option<Vec<Uuid>>,
    /// Skip existing sessions instead of erroring
    pub skip_existing: bool,
    /// Overwrite existing sessions
    pub overwrite: bool,
}

/// Result of an import operation
#[derive(Debug, Clone, Default)]
pub struct ImportResult {
    pub sessions_imported: usize,
    pub sessions_skipped: usize,
    pub workspaces_imported: usize,
    pub workspaces_skipped: usize,
    pub updates_imported: usize,
    pub checkpoints_imported: usize,
    pub embeddings_imported: usize,
    pub errors: Vec<String>,
}

// ============================================================================
// Export Functions
// ============================================================================

impl RealRocksDBStorage {
    /// Export all data from the database
    pub async fn export_full(&self, options: &ExportOptions) -> Result<ExportData> {
        info!("Starting full database export");

        let mut export = ExportData::new(ExportType::Full, options.compression);

        // Export all sessions
        let session_ids = self.list_sessions().await?;
        info!("Exporting {} sessions", session_ids.len());

        for session_id in session_ids {
            match self.export_session_data(session_id).await {
                Ok(session_data) => export.sessions.push(session_data),
                Err(e) => {
                    info!("Warning: Failed to export session {}: {}", session_id, e);
                }
            }
        }

        // Export all workspaces
        let workspaces = self.list_workspaces().await?;
        info!("Exporting {} workspaces", workspaces.len());
        export.workspaces = workspaces.into_iter().map(ExportedWorkspace::from).collect();

        // Export checkpoints if requested
        if options.include_checkpoints {
            export.checkpoints = self.list_checkpoints().await?;
            info!("Exported {} checkpoints", export.checkpoints.len());
        }

        export.update_counts();
        info!(
            "Export complete: {} sessions, {} workspaces, {} updates",
            export.metadata.session_count,
            export.metadata.workspace_count,
            export.metadata.update_count
        );

        Ok(export)
    }

    /// Export specific sessions
    pub async fn export_sessions(
        &self,
        session_ids: Vec<Uuid>,
        options: &ExportOptions,
    ) -> Result<ExportData> {
        info!("Starting selective export of {} sessions", session_ids.len());

        let mut export = ExportData::new(
            ExportType::SelectiveSessions {
                session_ids: session_ids.clone(),
            },
            options.compression,
        );

        for session_id in session_ids {
            match self.export_session_data(session_id).await {
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

        export.update_counts();
        Ok(export)
    }

    /// Export a workspace and all its sessions
    pub async fn export_workspace(
        &self,
        workspace_id: Uuid,
        options: &ExportOptions,
    ) -> Result<ExportData> {
        info!("Starting workspace export for {}", workspace_id);

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
        export.workspaces.push(ExportedWorkspace::from(workspace.clone()));

        // Export all sessions in the workspace
        for (session_id, _role) in &workspace.sessions {
            match self.export_session_data(*session_id).await {
                Ok(session_data) => export.sessions.push(session_data),
                Err(e) => {
                    info!(
                        "Warning: Failed to export session {} from workspace: {}",
                        session_id, e
                    );
                }
            }
        }

        export.update_counts();
        Ok(export)
    }

    /// Export a single session with its updates
    async fn export_session_data(&self, session_id: Uuid) -> Result<ExportedSession> {
        let session = self.load_session(session_id).await?;
        let updates = self.load_session_updates(session_id).await?;

        Ok(ExportedSession { session, updates })
    }

    // ========================================================================
    // Import Functions
    // ========================================================================

    /// Import data from an ExportData structure
    pub async fn import_data(
        &self,
        data: ExportData,
        options: &ImportOptions,
    ) -> Result<ImportResult> {
        info!(
            "Starting import: {} sessions, {} workspaces",
            data.sessions.len(),
            data.workspaces.len()
        );

        // Check format version compatibility
        if !is_version_compatible(&data.format_version) {
            return Err(anyhow::anyhow!(
                "Incompatible export format version: {}. Expected: {}",
                data.format_version,
                EXPORT_FORMAT_VERSION
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
                            .add_session_to_workspace(workspace.id, *session_id, role.clone())
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

        info!(
            "Import complete: {} sessions, {} workspaces, {} updates, {} errors",
            result.sessions_imported,
            result.workspaces_imported,
            result.updates_imported,
            result.errors.len()
        );

        Ok(result)
    }
}

// ============================================================================
// File I/O Functions
// ============================================================================

/// Statistics from export operation
#[derive(Debug, Clone)]
pub struct ExportStats {
    /// Size of the file on disk (after compression)
    pub file_size: u64,
    /// Size of the uncompressed JSON data
    pub uncompressed_size: usize,
}

/// Write export data to a file with optional compression
pub fn write_export_file(
    data: &ExportData,
    path: &Path,
    options: &ExportOptions,
) -> Result<ExportStats> {
    let compression = if options.compression == CompressionType::None {
        CompressionType::from_path(path)
    } else {
        options.compression
    };

    info!("Writing export to {:?} with {:?} compression", path, compression);

    let json_data = if options.pretty && compression == CompressionType::None {
        serde_json::to_vec_pretty(data)?
    } else {
        serde_json::to_vec(data)?
    };

    let uncompressed_size = json_data.len();

    let file = File::create(path).context("Failed to create export file")?;
    let mut writer = BufWriter::new(file);

    match compression {
        CompressionType::None => {
            writer.write_all(&json_data)?;
        }
        CompressionType::Gzip => {
            let mut encoder = GzEncoder::new(writer, GzCompression::default());
            encoder.write_all(&json_data)?;
            encoder.finish()?;
        }
        CompressionType::Zstd => {
            let mut encoder = zstd::stream::Encoder::new(writer, 3)?;
            encoder.write_all(&json_data)?;
            encoder.finish()?;
        }
    };

    // Get actual file size after writing
    let file_size = std::fs::metadata(path)
        .context("Failed to get file metadata")?
        .len();

    info!(
        "Export written: {} bytes on disk, {} bytes uncompressed",
        file_size, uncompressed_size
    );

    Ok(ExportStats {
        file_size,
        uncompressed_size,
    })
}

/// Read export data from a file with automatic decompression
pub fn read_export_file(path: &Path) -> Result<ExportData> {
    let compression = CompressionType::from_path(path);
    info!("Reading export from {:?} with {:?} compression", path, compression);

    let file = File::open(path).context("Failed to open export file")?;
    let reader = BufReader::new(file);

    let json_data: Vec<u8> = match compression {
        CompressionType::None => {
            let mut data = Vec::new();
            let mut reader = reader;
            reader.read_to_end(&mut data)?;
            data
        }
        CompressionType::Gzip => {
            let mut decoder = GzDecoder::new(reader);
            let mut data = Vec::new();
            decoder.read_to_end(&mut data)?;
            data
        }
        CompressionType::Zstd => {
            let mut decoder = zstd::stream::Decoder::new(reader)?;
            let mut data = Vec::new();
            decoder.read_to_end(&mut data)?;
            data
        }
    };

    let export: ExportData = serde_json::from_slice(&json_data).context("Failed to parse export JSON")?;

    info!(
        "Export read: format_version={}, {} sessions, {} workspaces",
        export.format_version,
        export.sessions.len(),
        export.workspaces.len()
    );

    Ok(export)
}

/// Preview export file without loading full data
pub fn preview_export_file(path: &Path) -> Result<ExportMetadata> {
    let export = read_export_file(path)?;
    Ok(export.metadata)
}

/// List sessions available in an export file
pub fn list_export_sessions(path: &Path) -> Result<Vec<(Uuid, String, usize)>> {
    let export = read_export_file(path)?;
    Ok(export
        .sessions
        .iter()
        .map(|s| {
            let name = s
                .session
                .metadata
                .name
                .clone()
                .unwrap_or_else(|| "Unnamed".to_string());
            (s.session.id(), name, s.updates.len())
        })
        .collect())
}

// ============================================================================
// Version Compatibility
// ============================================================================

/// Check if an export format version is compatible with current version
fn is_version_compatible(version: &str) -> bool {
    // Parse versions
    let current_parts: Vec<u32> = EXPORT_FORMAT_VERSION
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();
    let import_parts: Vec<u32> = version
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();

    if current_parts.len() < 2 || import_parts.len() < 2 {
        return false;
    }

    // Major version must match
    if current_parts[0] != import_parts[0] {
        return false;
    }

    // Minor version of import must be <= current (we can read older formats)
    import_parts[1] <= current_parts[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_compression_type_from_path() {
        assert_eq!(
            CompressionType::from_path(Path::new("export.json")),
            CompressionType::None
        );
        assert_eq!(
            CompressionType::from_path(Path::new("export.json.gz")),
            CompressionType::Gzip
        );
        assert_eq!(
            CompressionType::from_path(Path::new("export.json.zst")),
            CompressionType::Zstd
        );
        assert_eq!(
            CompressionType::from_path(Path::new("backup.gz")),
            CompressionType::Gzip
        );
    }

    #[test]
    fn test_compression_type_from_str() {
        assert_eq!(
            CompressionType::from_str("none"),
            Some(CompressionType::None)
        );
        assert_eq!(
            CompressionType::from_str("gzip"),
            Some(CompressionType::Gzip)
        );
        assert_eq!(
            CompressionType::from_str("zstd"),
            Some(CompressionType::Zstd)
        );
        assert_eq!(CompressionType::from_str("invalid"), None);
    }

    #[test]
    fn test_version_compatibility() {
        assert!(is_version_compatible("1.0.0"));
        assert!(is_version_compatible("1.0.1")); // Patch version doesn't matter
        assert!(is_version_compatible("1.1.0")); // Past minor version
        assert!(!is_version_compatible("2.0.0")); // Major version mismatch
        assert!(!is_version_compatible("1.3.0")); // Future minor version
    }

    #[test]
    fn test_export_data_new() {
        let export = ExportData::new(ExportType::Full, CompressionType::Gzip);
        assert_eq!(export.format_version, EXPORT_FORMAT_VERSION);
        assert!(export.schema.is_some());
        assert_eq!(export.sessions.len(), 0);
        assert_eq!(export.workspaces.len(), 0);
    }

    #[tokio::test]
    async fn test_export_import_roundtrip() {
        let temp_dir = tempdir().expect("Failed to create temp directory");
        let storage = RealRocksDBStorage::new(temp_dir.path().join("db"))
            .await
            .expect("Failed to create storage");

        // Create a test session
        let session = ActiveSession::new(
            Uuid::new_v4(),
            Some("Test Session".to_string()),
            Some("Test description".to_string()),
        );
        storage.save_session(&session).await.expect("Failed to save session");

        // Export
        let options = ExportOptions::default();
        let export = storage.export_full(&options).await.expect("Failed to export");

        assert_eq!(export.sessions.len(), 1);
        assert_eq!(export.sessions[0].session.id(), session.id());

        // Write to file
        let export_path = temp_dir.path().join("export.json");
        write_export_file(&export, &export_path, &options).expect("Failed to write export");

        // Read back
        let imported = read_export_file(&export_path).expect("Failed to read export");
        assert_eq!(imported.sessions.len(), 1);
        assert_eq!(imported.format_version, EXPORT_FORMAT_VERSION);
    }

    #[tokio::test]
    async fn test_compressed_export() {
        let temp_dir = tempdir().expect("Failed to create temp directory");

        let export = ExportData::new(ExportType::Full, CompressionType::Gzip);

        // Test gzip
        let gzip_path = temp_dir.path().join("export.json.gz");
        let options = ExportOptions {
            compression: CompressionType::Gzip,
            ..Default::default()
        };
        write_export_file(&export, &gzip_path, &options).expect("Failed to write gzip export");
        let imported = read_export_file(&gzip_path).expect("Failed to read gzip export");
        assert_eq!(imported.format_version, EXPORT_FORMAT_VERSION);

        // Test zstd
        let zstd_path = temp_dir.path().join("export.json.zst");
        let options = ExportOptions {
            compression: CompressionType::Zstd,
            ..Default::default()
        };
        write_export_file(&export, &zstd_path, &options).expect("Failed to write zstd export");
        let imported = read_export_file(&zstd_path).expect("Failed to read zstd export");
        assert_eq!(imported.format_version, EXPORT_FORMAT_VERSION);
    }
}
