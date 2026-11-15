// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Reading an archive part by part: the JSON is parsed as it is
//! decompressed, and each session, workspace, checkpoint, vector, card and
//! document is handed on as soon as it is read. The header (format version, schema,
//! metadata) must precede the sections, and the sections come in archive
//! order; unknown fields are skipped, as the whole-archive reader did.

use std::fmt;
use std::io::{BufRead, BufReader, Read};
use std::ops::ControlFlow;

use flate2::read::GzDecoder;
use post_cortex_core::archive::{
    ArchiveHeader, ArchivePart, CompressionType, ExportMetadata, ExportedEmbedding,
    ExportedSession, ExportedWorkspace,
};
use post_cortex_core::core::card::Card;
use post_cortex_core::core::document::Document;
use post_cortex_core::session::checkpoint::SessionCheckpoint;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::export_import::codec::{Result, SECTIONS, io, sniff};

/// Sections an archive must have (the later ones default to empty).
const REQUIRED: usize = 3;

/// Read the archive in `input`, handing its parts to `sink`.
pub fn read(
    input: &mut dyn Read,
    sink: &mut dyn FnMut(ArchivePart) -> ControlFlow<()>,
) -> Result<()> {
    let mut buffered = BufReader::new(input);
    let compression = sniff(buffered.fill_buf().map_err(io("read the archive"))?);
    let json: Box<dyn Read + '_> = match compression {
        CompressionType::None => Box::new(buffered),
        CompressionType::Gzip => Box::new(BufReader::new(GzDecoder::new(buffered))),
        CompressionType::Zstd => Box::new(BufReader::new(
            zstd::stream::read::Decoder::with_buffer(buffered).map_err(io("unzstd"))?,
        )),
    };
    let mut reading = Reading {
        sink,
        stopped: false,
    };
    let mut de = serde_json::Deserializer::from_reader(json);
    match (Archive {
        reading: &mut reading,
    })
    .deserialize(&mut de)
    {
        Ok(()) => Ok(de.end()?),
        // The sink broke off: not an error of the archive.
        Err(_) if reading.stopped => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Where the parts go.
struct Reading<'s> {
    sink: &'s mut dyn FnMut(ArchivePart) -> ControlFlow<()>,
    /// The sink broke off; parsing ends with an error that is not one.
    stopped: bool,
}

impl Reading<'_> {
    fn give<E: de::Error>(&mut self, part: ArchivePart) -> std::result::Result<(), E> {
        match (self.sink)(part) {
            ControlFlow::Continue(()) => Ok(()),
            ControlFlow::Break(()) => {
                self.stopped = true;
                Err(E::custom("the reader stopped"))
            }
        }
    }
}

/// The whole archive: its top-level object.
struct Archive<'r, 's> {
    reading: &'r mut Reading<'s>,
}

impl<'de> DeserializeSeed<'de> for Archive<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> std::result::Result<(), D::Error> {
        de.deserialize_map(self)
    }
}

/// The header fields read so far.
#[derive(Default)]
struct Head {
    format_version: Option<String>,
    schema: Option<String>,
    metadata: Option<ExportMetadata>,
    /// Handed on (the first section started).
    sent: bool,
}

impl Head {
    fn header<E: de::Error>(&mut self) -> std::result::Result<ArchivePart, E> {
        let format_version = self
            .format_version
            .take()
            .ok_or_else(|| E::missing_field("format_version"))?;
        let metadata = self
            .metadata
            .take()
            .ok_or_else(|| E::missing_field("metadata"))?;
        self.sent = true;
        Ok(ArchivePart::Header(Box::new(ArchiveHeader {
            format_version,
            schema: self.schema.take(),
            metadata,
        })))
    }
}

impl<'de> Visitor<'de> for Archive<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a pcx export archive")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<(), A::Error> {
        let mut head = Head::default();
        // The index (in SECTIONS) of the next section allowed, and the
        // sections read.
        let mut next = 0;
        let mut seen = [false; SECTIONS.len()];
        while let Some(key) = map.next_key::<String>()? {
            let header_field = matches!(key.as_str(), "format_version" | "schema" | "metadata");
            if header_field && head.sent {
                return Err(de::Error::custom(format!(
                    "{key} must come before the archive's sections"
                )));
            }
            match key.as_str() {
                "format_version" => head.format_version = Some(map.next_value()?),
                "schema" => head.schema = map.next_value()?,
                "metadata" => head.metadata = Some(map.next_value()?),
                other => match SECTIONS.iter().position(|(_, k)| *k == other) {
                    Some(index) => {
                        if index < next {
                            return Err(de::Error::custom(format!(
                                "section {other} out of archive order"
                            )));
                        }
                        if !head.sent {
                            let header = head.header()?;
                            self.reading.give(header)?;
                        }
                        next = index + 1;
                        seen[index] = true;
                        section(&mut map, index, self.reading)?;
                    }
                    None => {
                        map.next_value::<IgnoredAny>()?;
                    }
                },
            }
        }
        match seen[..REQUIRED].iter().position(|seen| !seen) {
            Some(index) => Err(de::Error::missing_field(SECTIONS[index].1)),
            None => Ok(()),
        }
    }
}

/// Read the value of section `index` part by part.
fn section<'de, A: MapAccess<'de>>(
    map: &mut A,
    index: usize,
    reading: &mut Reading<'_>,
) -> std::result::Result<(), A::Error> {
    match index {
        0 => map.next_value_seed(Section::<ExportedSession>::new(reading, |s| {
            ArchivePart::Session(Box::new(s))
        })),
        1 => map.next_value_seed(Section::<ExportedWorkspace>::new(reading, |w| {
            ArchivePart::Workspace(Box::new(w))
        })),
        2 => map.next_value_seed(Section::<SessionCheckpoint>::new(reading, |c| {
            ArchivePart::Checkpoint(Box::new(c))
        })),
        3 => map.next_value_seed(Section::<ExportedEmbedding>::new(reading, |e| {
            ArchivePart::Embedding(Box::new(e))
        })),
        4 => map.next_value_seed(Section::<Card>::new(reading, |c| {
            ArchivePart::Card(Box::new(c))
        })),
        _ => map.next_value_seed(Section::<Document>::new(reading, |d| {
            ArchivePart::Document(Box::new(d))
        })),
    }
}

/// One section: a JSON array whose elements become parts.
struct Section<'r, 's, T> {
    reading: &'r mut Reading<'s>,
    part: fn(T) -> ArchivePart,
}

impl<'r, 's, T> Section<'r, 's, T> {
    fn new(reading: &'r mut Reading<'s>, part: fn(T) -> ArchivePart) -> Self {
        Self { reading, part }
    }
}

impl<'de, T: Deserialize<'de>> DeserializeSeed<'de> for Section<'_, '_, T> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> std::result::Result<(), D::Error> {
        de.deserialize_seq(self)
    }
}

impl<'de, T: Deserialize<'de>> Visitor<'de> for Section<'_, '_, T> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an array of archive parts")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<(), A::Error> {
        while let Some(item) = seq.next_element::<T>()? {
            self.reading.give((self.part)(item))?;
        }
        Ok(())
    }
}
