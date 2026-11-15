// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The archive codec port (ARCHITECTURE section 5): writes and reads the
//! export archive one part at a time ([`post_cortex_core::archive::ArchivePart`]),
//! so neither side holds a whole archive. The adapter owns the format
//! versions, the JSON schema and compression.

use std::io::{Read, Write};
use std::ops::ControlFlow;

use post_cortex_core::archive::{ArchivePart, ArchiveSize, CompressionType};

use crate::error::Result;

/// The export file format.
pub trait ArchiveCodec: Send + Sync {
    /// The format version this codec writes.
    fn format_version(&self) -> &'static str;

    /// The URL of the JSON schema of [`Self::format_version`].
    fn schema_url(&self) -> &'static str;

    /// Whether this codec reads an archive of format `version`.
    ///
    /// # Errors
    /// [`crate::PortError::Corrupt`] naming the version when it is not
    /// readable.
    fn check_version(&self, version: &str) -> Result<()>;

    /// A writer of one archive into `out`, compressed with `compression`
    /// (`pretty`: indented JSON, when uncompressed).
    ///
    /// # Errors
    /// The compressor could not start.
    fn writer(
        &self,
        out: Box<dyn Write + Send>,
        compression: CompressionType,
        pretty: bool,
    ) -> Result<Box<dyn ArchiveWriter>>;

    /// Read the archive in `input` (compression detected from its first
    /// bytes), handing its parts to `sink` in archive order, the header
    /// first; stops early when `sink` breaks.
    ///
    /// # Errors
    /// [`crate::PortError::Corrupt`] when the bytes are not an archive (or
    /// its header does not precede its sections);
    /// [`crate::PortError::Unavailable`] when reading failed.
    fn read(
        &self,
        input: &mut dyn Read,
        sink: &mut dyn FnMut(ArchivePart) -> ControlFlow<()>,
    ) -> Result<()>;
}

/// Writes one archive, part by part.
pub trait ArchiveWriter: Send {
    /// Append `part`. Parts come in archive order: the header first, then
    /// the sections in order (a section may be empty).
    ///
    /// # Errors
    /// [`crate::PortError::Corrupt`] for a part out of order (the archive
    /// would not be readable);
    /// [`crate::PortError::Unavailable`] when writing failed.
    fn write(&mut self, part: &ArchivePart) -> Result<()>;

    /// End the archive (sections not written are empty) and flush it.
    ///
    /// # Errors
    /// No header was written ([`crate::PortError::Corrupt`]), or writing
    /// failed.
    fn finish(self: Box<Self>) -> Result<ArchiveSize>;
}
