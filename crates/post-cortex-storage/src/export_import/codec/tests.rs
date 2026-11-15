// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The streaming codec writes exactly the bytes `serde_json` gives the
//! whole archive, reads back what it wrote in every compression, and
//! refuses parts and sections out of order.

use std::io::Write;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use post_cortex_core::archive::{
    ArchivePart, CompressionType, ExportData, ExportType, ExportedEmbedding, ExportedSession,
    ExportedWorkspace,
};
use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_core::session::active_session::{ActiveSession, StoredSession};
use post_cortex_core::workspace::SessionRole;
use post_cortex_ports::{ArchiveCodec, PortError};
use uuid::Uuid;

use crate::export_import::{EXPORT_FORMAT_VERSION, EXPORT_SCHEMA_URL, JsonArchive};

/// A buffer the writer can own while the test reads it.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn archive() -> ExportData {
    let at = Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap();
    let mut data = ExportData::new(
        EXPORT_FORMAT_VERSION,
        EXPORT_SCHEMA_URL,
        ExportType::Full,
        CompressionType::None,
        at,
    );
    let ws = Uuid::from_u128(2);
    for n in 0..2u128 {
        let session = ActiveSession::new(Uuid::from_u128(10 + n), Some(format!("s{n}")), None, at);
        data.sessions.push(ExportedSession {
            session: StoredSession::new(session, vec![]),
            updates: vec![],
            messages: vec![],
        });
    }
    data.workspaces.push(ExportedWorkspace {
        id: ws,
        name: "w\nwith a line break".into(),
        slug: None,
        description: String::new(),
        sessions: vec![(Uuid::from_u128(10), SessionRole::Primary)],
        created_at: 1,
        projects: vec![],
        archived_at: None,
        remote_rerank: false,
        dismissals: vec![],
    });
    data.embeddings.push(ExportedEmbedding {
        content_id: "entity:x".into(),
        session_id: Uuid::from_u128(10).to_string(),
        vector: vec![0.5, 1.0],
        text: "x".into(),
        content_type: "entity".into(),
        timestamp: at.to_rfc3339(),
        metadata: Default::default(),
    });
    for version in 1..=2 {
        data.cards.push(Card {
            scope: CardScope::Workspace(ws),
            version,
            text: format!("# card\nv{version}\n"),
            updated_at: at,
            author: None,
        });
    }
    data.update_counts();
    data
}

fn write(parts: &[ArchivePart], compression: CompressionType, pretty: bool) -> Vec<u8> {
    let out = Shared::default();
    let mut writer = JsonArchive
        .writer(Box::new(out.clone()), compression, pretty)
        .unwrap();
    for part in parts {
        writer.write(part).unwrap();
    }
    let size = writer.finish().unwrap();
    let bytes = out.0.lock().unwrap().clone();
    assert_eq!(size.written_bytes, bytes.len() as u64);
    bytes
}

fn read(bytes: &[u8]) -> Result<Vec<ArchivePart>, PortError> {
    let mut parts = Vec::new();
    JsonArchive.read(&mut &bytes[..], &mut |part| {
        parts.push(part);
        ControlFlow::Continue(())
    })?;
    Ok(parts)
}

fn json(data: &ExportData) -> serde_json::Value {
    serde_json::to_value(data).unwrap()
}

#[test]
fn parts_are_written_as_serde_writes_the_whole_archive() {
    for data in [archive(), {
        let mut empty = archive();
        empty.sessions.clear();
        empty.workspaces.clear();
        empty.embeddings.clear();
        empty.cards.clear();
        empty.schema = None;
        empty
    }] {
        let parts = data.parts();
        assert_eq!(
            write(&parts, CompressionType::None, false),
            serde_json::to_vec(&data).unwrap()
        );
        assert_eq!(
            write(&parts, CompressionType::None, true),
            serde_json::to_vec_pretty(&data).unwrap()
        );
    }
}

#[test]
fn every_compression_reads_back() {
    let data = archive();
    for compression in [
        CompressionType::None,
        CompressionType::Gzip,
        CompressionType::Zstd,
    ] {
        let bytes = write(&data.parts(), compression, true);
        assert_eq!(crate::export_import::codec::sniff(&bytes), compression);
        let back = ExportData::from_parts(read(&bytes).unwrap());
        assert_eq!(json(&back), json(&data), "{compression:?}");
    }
}

#[test]
fn the_writer_refuses_parts_out_of_order() {
    let parts = archive().parts();
    let corrupt = |result: Result<(), PortError>| {
        assert!(
            matches!(result, Err(PortError::Corrupt { .. })),
            "{result:?}"
        );
    };
    let out = Box::new(Shared::default());
    let mut writer = JsonArchive
        .writer(out, CompressionType::None, false)
        .unwrap();
    corrupt(writer.write(&parts[1]));

    let out = Box::new(Shared::default());
    let mut writer = JsonArchive
        .writer(out, CompressionType::None, false)
        .unwrap();
    writer.write(&parts[0]).unwrap();
    writer.write(&parts[3]).unwrap();
    corrupt(writer.write(&parts[1]));
    corrupt(writer.write(&parts[0]));

    let out = Box::new(Shared::default());
    let writer = JsonArchive
        .writer(out, CompressionType::None, false)
        .unwrap();
    corrupt(writer.finish().map(drop));
}

#[test]
fn the_reader_refuses_a_header_after_the_sections_and_sections_out_of_order() {
    let text = String::from_utf8(serde_json::to_vec(&archive()).unwrap()).unwrap();
    let value = json(&archive());
    let field = |key: &str| format!("\"{key}\":{}", value[key]);
    let refused = [
        format!(
            "{{{},{},{},{}}}",
            field("sessions"),
            field("format_version"),
            field("metadata"),
            field("workspaces")
        ),
        format!(
            "{{{},{},{},{},{}}}",
            field("format_version"),
            field("metadata"),
            field("workspaces"),
            field("sessions"),
            field("checkpoints")
        ),
        format!(
            "{{{},{},{}}}",
            field("format_version"),
            field("metadata"),
            field("sessions")
        ),
        text[..text.len() - 1].to_string(),
        format!("{text} trailing"),
    ];
    for bytes in refused {
        assert!(
            matches!(read(bytes.as_bytes()), Err(PortError::Corrupt { .. })),
            "{bytes}"
        );
    }
    // Unknown fields are skipped; cards and vectors may be left out.
    let older = format!(
        "{{{},\"note\":[1,{{}}],{},{},{},{}}}",
        field("format_version"),
        field("metadata"),
        field("sessions"),
        field("workspaces"),
        field("checkpoints")
    );
    let back = ExportData::from_parts(read(older.as_bytes()).unwrap());
    assert_eq!(back.sessions.len(), 2);
    assert!(back.cards.is_empty());
}

#[test]
fn a_sink_that_breaks_off_ends_the_read() {
    let bytes = write(&archive().parts(), CompressionType::Gzip, false);
    let mut seen = 0;
    JsonArchive
        .read(&mut &bytes[..], &mut |_| {
            seen += 1;
            if seen == 2 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .unwrap();
    assert_eq!(seen, 2);
}
