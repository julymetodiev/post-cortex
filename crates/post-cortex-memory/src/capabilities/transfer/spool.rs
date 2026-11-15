// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An archive on disk for an import: one that arrives in pieces is spooled
//! to a temporary file (at most [`SystemConfig::max_import_bytes`](crate::SystemConfig::max_import_bytes)),
//! so an import reads it twice, a preview pass and then the write pass,
//! without holding it in memory.

use std::path::PathBuf;

use tempfile::TempPath;
use tokio::io::AsyncWriteExt;

use crate::capabilities::transfer::TransferError;

/// An archive file an import reads.
pub struct ArchiveFile {
    path: PathBuf,
    /// The spooled file, deleted with it.
    _spooled: Option<TempPath>,
}

impl ArchiveFile {
    /// The archive at `path`, a file the caller keeps.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            _spooled: None,
        }
    }

    /// The file, read from the start.
    ///
    /// # Errors
    /// It cannot be opened.
    pub fn open(&self) -> Result<std::fs::File, TransferError> {
        Ok(std::fs::File::open(&self.path)?)
    }
}

/// An archive arriving in pieces, written to a temporary file.
pub struct ArchiveSpool {
    file: tokio::fs::File,
    path: TempPath,
    len: u64,
    limit: u64,
}

impl ArchiveSpool {
    /// A spool of at most `limit` bytes.
    ///
    /// # Errors
    /// The temporary file cannot be created.
    pub fn new(limit: u64) -> Result<Self, TransferError> {
        let (file, path) = tempfile::NamedTempFile::new()?.into_parts();
        Ok(Self {
            file: tokio::fs::File::from_std(file),
            path,
            len: 0,
            limit,
        })
    }

    /// Append `bytes`.
    ///
    /// # Errors
    /// The archive would be larger than the limit
    /// ([`TransferError::TooLarge`]), or writing failed.
    pub async fn append(&mut self, bytes: &[u8]) -> Result<(), TransferError> {
        let len = self.len + bytes.len() as u64;
        if len > self.limit {
            return Err(TransferError::TooLarge(self.limit));
        }
        self.file.write_all(bytes).await?;
        self.len = len;
        Ok(())
    }

    /// The archive received.
    ///
    /// # Errors
    /// Nothing was received ([`TransferError::Empty`]), or writing failed.
    pub async fn finish(mut self) -> Result<ArchiveFile, TransferError> {
        if self.len == 0 {
            return Err(TransferError::Empty);
        }
        self.file.flush().await?;
        Ok(ArchiveFile {
            path: self.path.to_path_buf(),
            _spooled: Some(self.path),
        })
    }
}
