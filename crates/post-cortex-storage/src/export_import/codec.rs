// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Archive files: JSON, optionally gzip or zstd compressed, written and
//! read one part at a time ([`write`], [`read`]); [`JsonArchive`] is the
//! codec.

use std::io::{Read, Write};
use std::ops::ControlFlow;

use post_cortex_core::archive::{ArchivePart, ArchiveSection, CompressionType};
use post_cortex_ports::{ArchiveCodec, ArchiveWriter, PortError};

mod read;
mod write;

#[cfg(test)]
mod tests;

/// Why an archive could not be written or read.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// The bytes are not an archive (or a part could not be encoded).
    #[error("archive JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Compressing, decompressing, reading or writing failed.
    #[error("{action}: {source}")]
    Io {
        /// What was being done.
        action: &'static str,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },
    /// An archive of a format this build does not read.
    #[error(
        "Incompatible export format version: {found}. This build reads 1.0.0 to {}",
        crate::export_import::format::EXPORT_FORMAT_VERSION
    )]
    Version {
        /// The archive's format version.
        found: String,
    },
    /// A part written out of archive order.
    #[error("archive part out of order: {0}")]
    Order(&'static str),
}

/// The result of an archive operation.
pub type Result<T> = std::result::Result<T, ArchiveError>;

/// An [`ArchiveError::Io`] while doing `action`.
fn io(action: &'static str) -> impl FnOnce(std::io::Error) -> ArchiveError {
    move |source| ArchiveError::Io { action, source }
}

/// The sections of an archive in order, with their JSON keys.
const SECTIONS: [(ArchiveSection, &str); 6] = [
    (ArchiveSection::Sessions, "sessions"),
    (ArchiveSection::Workspaces, "workspaces"),
    (ArchiveSection::Checkpoints, "checkpoints"),
    (ArchiveSection::Embeddings, "embeddings"),
    (ArchiveSection::Cards, "cards"),
    (ArchiveSection::Documents, "documents"),
];

/// The compression of export bytes, from their magic number (gzip
/// `1f 8b`, zstd `28 b5 2f fd`); anything else is read as plain JSON.
fn sniff(bytes: &[u8]) -> CompressionType {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        CompressionType::Gzip
    } else if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        CompressionType::Zstd
    } else {
        CompressionType::None
    }
}

/// The `ArchiveCodec` adapter: the export format of this build (version
/// [`crate::export_import::format::EXPORT_FORMAT_VERSION`], JSON with
/// optional gzip or zstd compression).
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonArchive;

impl ArchiveCodec for JsonArchive {
    fn format_version(&self) -> &'static str {
        crate::export_import::format::EXPORT_FORMAT_VERSION
    }

    fn schema_url(&self) -> &'static str {
        crate::export_import::format::EXPORT_SCHEMA_URL
    }

    fn check_version(&self, version: &str) -> post_cortex_ports::error::Result<()> {
        crate::export_import::format::check_format_version(version)
            .map_err(|e| post_cortex_ports::error::corrupt("archive", version, e))
    }

    fn writer(
        &self,
        out: Box<dyn Write + Send>,
        compression: CompressionType,
        pretty: bool,
    ) -> post_cortex_ports::error::Result<Box<dyn ArchiveWriter>> {
        write::JsonWriter::new(out, compression, pretty)
            .map(|w| Box::new(w) as Box<dyn ArchiveWriter>)
            .map_err(write_failed)
    }

    fn read(
        &self,
        input: &mut dyn Read,
        sink: &mut dyn FnMut(ArchivePart) -> ControlFlow<()>,
    ) -> post_cortex_ports::error::Result<()> {
        read::read(input, sink)
            .map_err(|e| post_cortex_ports::error::corrupt("archive", "bytes", format!("{e:#}")))
    }
}

/// The port error of a failed write: a part out of order would leave an
/// unreadable archive; anything else is the output's failure.
fn write_failed(e: ArchiveError) -> PortError {
    match e {
        ArchiveError::Order(_) => post_cortex_ports::error::corrupt("archive", "part", e),
        e => PortError::Unavailable(format!("{e:#}")),
    }
}
