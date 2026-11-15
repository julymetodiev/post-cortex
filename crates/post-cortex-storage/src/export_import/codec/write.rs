// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Writing an archive part by part, with exactly the bytes `serde_json`
//! gives the whole archive (compact, or its pretty form, indented by two
//! spaces per level), through the compressor.

use std::io::Write;

use flate2::Compression as GzCompression;
use flate2::write::GzEncoder;
use post_cortex_core::archive::{ArchiveHeader, ArchivePart, ArchiveSize, CompressionType};
use post_cortex_ports::{ArchiveWriter, PortError};
use serde::Serialize;

use crate::export_import::codec::{ArchiveError, Result, SECTIONS, io, write_failed};

/// Counts the bytes that pass into `inner`.
struct Counting<W> {
    inner: W,
    bytes: u64,
}

impl<W: Write> Write for Counting<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// The output, counting the bytes written.
type Out = Counting<Box<dyn Write + Send>>;

/// The archive's compression, over the output.
enum Compressor {
    Plain(Out),
    Gzip(GzEncoder<Out>),
    Zstd(zstd::stream::write::Encoder<'static, Out>),
}

impl Write for Compressor {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(w) => w.write(buf),
            Self::Gzip(w) => w.write(buf),
            Self::Zstd(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(w) => w.flush(),
            Self::Gzip(w) => w.flush(),
            Self::Zstd(w) => w.flush(),
        }
    }
}

impl Compressor {
    /// End the compressed stream; the output.
    fn finish(self) -> std::io::Result<Out> {
        match self {
            Self::Plain(w) => Ok(w),
            Self::Gzip(w) => w.finish(),
            Self::Zstd(w) => w.finish(),
        }
    }
}

/// The JSON archive writer.
pub struct JsonWriter {
    /// The JSON, counted before compression.
    json: Counting<Compressor>,
    /// Indented JSON.
    pretty: bool,
    /// Whether the header was written.
    header: bool,
    /// The index (in [`SECTIONS`]) of the first section not opened yet.
    next: usize,
    /// The parts in the open section (`next - 1`), if one is open.
    open: Option<usize>,
}

impl JsonWriter {
    /// A writer into `out` (`pretty` only applies uncompressed).
    pub fn new(
        out: Box<dyn Write + Send>,
        compression: CompressionType,
        pretty: bool,
    ) -> Result<Self> {
        let out = Counting {
            inner: out,
            bytes: 0,
        };
        let compressor = match compression {
            CompressionType::None => Compressor::Plain(out),
            CompressionType::Gzip => {
                Compressor::Gzip(GzEncoder::new(out, GzCompression::default()))
            }
            CompressionType::Zstd => {
                Compressor::Zstd(zstd::stream::write::Encoder::new(out, 3).map_err(io("zstd"))?)
            }
        };
        Ok(Self {
            json: Counting {
                inner: compressor,
                bytes: 0,
            },
            pretty: pretty && compression == CompressionType::None,
            header: false,
            next: 0,
            open: None,
        })
    }

    fn raw(&mut self, text: &str) -> Result<()> {
        self.json
            .write_all(text.as_bytes())
            .map_err(io("write the archive"))
    }

    /// `"key": ` after the separator of a top-level field.
    fn field(&mut self, first: bool, key: &str) -> Result<()> {
        let text = match (self.pretty, first) {
            (true, true) => format!("{{\n  \"{key}\": "),
            (true, false) => format!(",\n  \"{key}\": "),
            (false, true) => format!("{{\"{key}\":"),
            (false, false) => format!(",\"{key}\":"),
        };
        self.raw(&text)
    }

    /// `value` at the nesting where `indent` starts its lines.
    fn value<T: Serialize>(&mut self, value: &T, indent: &str) -> Result<()> {
        if self.pretty {
            let text = serde_json::to_string_pretty(value)?;
            self.raw(&text.replace('\n', &format!("\n{indent}")))
        } else {
            serde_json::to_writer(&mut self.json, value)?;
            Ok(())
        }
    }

    fn header(&mut self, header: &ArchiveHeader) -> Result<()> {
        if self.header {
            return Err(ArchiveError::Order("a second header"));
        }
        self.field(true, "format_version")?;
        self.value(&header.format_version, "  ")?;
        if let Some(schema) = &header.schema {
            self.field(false, "schema")?;
            self.value(schema, "  ")?;
        }
        self.field(false, "metadata")?;
        self.value(&header.metadata, "  ")?;
        self.header = true;
        Ok(())
    }

    /// Close the open section, if any.
    fn close(&mut self) -> Result<()> {
        match self.open.take() {
            Some(parts) if self.pretty && parts > 0 => self.raw("\n  ]"),
            Some(_) => self.raw("]"),
            None => Ok(()),
        }
    }

    /// Make section `index` the open one: the sections before it that were
    /// not written are written empty.
    fn enter(&mut self, index: usize) -> Result<()> {
        if !self.header {
            return Err(ArchiveError::Order("a part before the header"));
        }
        if self.open.is_some() && self.next == index + 1 {
            return Ok(());
        }
        if index < self.next {
            return Err(ArchiveError::Order("a part after a later section"));
        }
        while self.next <= index {
            self.close()?;
            self.field(false, SECTIONS[self.next].1)?;
            self.raw("[")?;
            self.open = Some(0);
            self.next += 1;
        }
        Ok(())
    }

    fn part<T: Serialize>(&mut self, index: usize, value: &T) -> Result<()> {
        self.enter(index)?;
        let parts = self.open.unwrap_or_default();
        match (self.pretty, parts) {
            (true, 0) => self.raw("\n    ")?,
            (true, _) => self.raw(",\n    ")?,
            (false, 0) => {}
            (false, _) => self.raw(",")?,
        }
        self.value(value, "    ")?;
        self.open = Some(parts + 1);
        Ok(())
    }

    fn write_part(&mut self, part: &ArchivePart) -> Result<()> {
        match part {
            ArchivePart::Header(h) => self.header(h),
            ArchivePart::Session(s) => self.part(0, s.as_ref()),
            ArchivePart::Workspace(w) => self.part(1, w.as_ref()),
            ArchivePart::Checkpoint(c) => self.part(2, c.as_ref()),
            ArchivePart::Embedding(e) => self.part(3, e.as_ref()),
            ArchivePart::Card(c) => self.part(4, c.as_ref()),
            ArchivePart::Document(d) => self.part(5, d.as_ref()),
        }
    }

    fn end(mut self) -> Result<ArchiveSize> {
        self.enter(SECTIONS.len() - 1)?;
        self.close()?;
        self.raw(if self.pretty { "\n}" } else { "}" })?;
        let json_bytes = self.json.bytes;
        let mut out = self.json.inner.finish().map_err(io("compress"))?;
        out.flush().map_err(io("write the archive"))?;
        Ok(ArchiveSize {
            json_bytes,
            written_bytes: out.bytes,
        })
    }
}

impl ArchiveWriter for JsonWriter {
    fn write(&mut self, part: &ArchivePart) -> std::result::Result<(), PortError> {
        self.write_part(part).map_err(write_failed)
    }

    fn finish(self: Box<Self>) -> std::result::Result<ArchiveSize, PortError> {
        self.end().map_err(write_failed)
    }
}
