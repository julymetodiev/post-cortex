// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use crate::capabilities::transfer::tests::{decode, encode, export, import, of, preview_of};
use crate::capabilities::transfer::*;
use chrono;
use post_cortex_core::archive::*;
use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::session::active_session::{ActiveSession, StoredSession};
use post_cortex_core::workspace::SessionRole;
use post_cortex_ports::storage::SessionStore;
use post_cortex_storage::RealRocksDBStorage;
use post_cortex_storage::export_import::{
    EXPORT_FORMAT_VERSION, EXPORT_SCHEMA_URL, check_format_version,
};
use std::path::Path;
use tempfile::tempdir;
use uuid::Uuid;

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
        "none".parse::<CompressionType>().ok(),
        Some(CompressionType::None)
    );
    assert_eq!(
        "gzip".parse::<CompressionType>().ok(),
        Some(CompressionType::Gzip)
    );
    assert_eq!(
        "zstd".parse::<CompressionType>().ok(),
        Some(CompressionType::Zstd)
    );
    assert!("invalid".parse::<CompressionType>().is_err());
}

/// One gate for import and preview on every backend: `1.<minor>.<patch>`
/// up to the current minor; anything else (a newer minor or major, a
/// malformed version) is refused.
#[test]
fn format_versions_up_to_the_current_one_are_readable() {
    for readable in [
        "1.0.0", "1.0.1", "1.1.0", "1.2.0", "1.2.7", "1.3.0", "1.4.0", "1.4.9", "1.5.0", "1.6.0",
        "1.7.0", "1.8.0", "1.9.0", "1.10.0",
    ] {
        assert!(check_format_version(readable).is_ok(), "{readable}");
    }
    for refused in [
        "1.11.0", "2.0.0", "0.9.0", "1.4", "1", "", "1.x.0", "1.4.0.1",
    ] {
        assert!(check_format_version(refused).is_err(), "{refused}");
    }
}

#[test]
fn test_export_data_new() {
    let export = ExportData::new(
        EXPORT_FORMAT_VERSION,
        EXPORT_SCHEMA_URL,
        ExportType::Full,
        CompressionType::Gzip,
        chrono::Utc::now(),
    );
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
        chrono::Utc::now(),
    );
    storage
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .expect("Failed to save session");

    // Export
    let options = ExportOptions::default();
    let export = export(&of(&storage), &ExportScope::All, &options)
        .await
        .expect("Failed to export");

    assert_eq!(export.sessions.len(), 1);
    assert_eq!(export.sessions[0].session.session.id(), session.id());

    // Write to file
    let export_path = temp_dir.path().join("export.json");
    std::fs::write(
        &export_path,
        encode(&export, options.compression, options.pretty),
    )
    .expect("Failed to write export");

    // Read back
    let imported = decode(&std::fs::read(&export_path).unwrap()).expect("Failed to read export");
    assert_eq!(imported.sessions.len(), 1);
    assert_eq!(imported.format_version, EXPORT_FORMAT_VERSION);
}

/// Sessions store their updates as rows, not in the blob; an export
/// must still carry every update, and an import must restore them.
#[tokio::test]
async fn test_export_import_keeps_row_backed_updates() {
    use post_cortex_core::core::context_update::{UpdateContent, UpdateType};

    let temp_dir = tempdir().unwrap();
    let source = RealRocksDBStorage::new(temp_dir.path().join("src"))
        .await
        .unwrap();

    let updates: Vec<ContextUpdate> = (0..3)
        .map(|i| ContextUpdate {
            meta: Default::default(),
            id: Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
            update_type: UpdateType::DecisionMade,
            content: UpdateContent {
                title: format!("decision {i}"),
                description: String::new(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: vec![],
            creates_relationships: vec![],
            references_entities: vec![],
            typed_entities: vec![],
            author: None,
            derived_from: Vec::new(),
        })
        .collect();
    let mut session =
        ActiveSession::new(Uuid::new_v4(), Some("s".into()), None, chrono::Utc::now());
    session.replace_records(updates.clone());
    source
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .unwrap();

    let export = export(&of(&source), &ExportScope::All, &ExportOptions::default())
        .await
        .unwrap();
    let exported = &export.sessions[0];
    assert_eq!(exported.session.session.records().len(), 3);
    // Format 1.4.0 holds the records once, in the session.
    assert!(exported.updates.is_empty());
    assert_eq!(exported.records().len(), 3);

    let path = temp_dir.path().join("export.json");
    std::fs::write(&path, encode(&export, CompressionType::None, false)).unwrap();
    let data = decode(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(data.sessions[0].session.session.records().len(), 3);

    let target = RealRocksDBStorage::new(temp_dir.path().join("dst"))
        .await
        .unwrap();
    let result = import(&of(&target), data, &ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(result.sessions_imported, 1);

    let restored = target.load_session(session.id()).await.unwrap().session;
    let ids: Vec<Uuid> = restored.records().iter().map(|u| u.id).collect();
    assert_eq!(ids, updates.iter().map(|u| u.id).collect::<Vec<_>>());
    assert_eq!(
        target
            .load_session(session.id())
            .await
            .unwrap()
            .session
            .records()
            .len(),
        3
    );
}

/// The in-memory encoding is the file format: any compression decodes,
/// and the preview counts what the filters keep and flags existing ids.
#[tokio::test]
async fn encoded_exports_decode_and_preview() {
    let mut export = ExportData::new(
        EXPORT_FORMAT_VERSION,
        EXPORT_SCHEMA_URL,
        ExportType::Full,
        CompressionType::None,
        chrono::Utc::now(),
    );
    let session = ActiveSession::new(Uuid::new_v4(), Some("s".into()), None, chrono::Utc::now());
    let other = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    let ws = Uuid::new_v4();
    export.sessions = vec![
        ExportedSession {
            session: StoredSession::new(session.clone(), vec![]),
            updates: vec![],
            messages: vec![],
        },
        ExportedSession {
            session: StoredSession::new(other.clone(), vec![]),
            updates: vec![],
            messages: vec![],
        },
    ];
    export.workspaces.push(ExportedWorkspace {
        id: ws,
        name: "w".into(),
        slug: None,
        description: String::new(),
        sessions: vec![(session.id(), SessionRole::Primary)],
        created_at: 1,
        projects: vec![],
        archived_at: Some(5),
        remote_rerank: false,
        dismissals: vec![],
    });
    export.update_counts();
    for compression in [
        CompressionType::None,
        CompressionType::Gzip,
        CompressionType::Zstd,
    ] {
        let back = decode(&encode(&export, compression, true)).unwrap();
        assert_eq!(back.session_ids(), export.session_ids());
        assert_eq!(back.workspaces[0].archived_at, Some(5));
    }

    let dir = tempdir().unwrap();
    let target = RealRocksDBStorage::new(dir.path()).await.unwrap();
    target
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .unwrap();
    post_cortex_ports::storage::WorkspaceStore::save_workspace_metadata(
        &target,
        ws,
        "w",
        &ws.to_string(),
        "",
        &[],
    )
    .await
    .unwrap();
    let target = of(&target);
    let preview = preview_of(
        &target,
        &export,
        &ImportOptions {
            skip_existing: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!((preview.sessions, preview.workspaces), (2, 1));
    assert_eq!(preview.conflicts.len(), 2);
    assert!(
        preview
            .conflicts
            .iter()
            .all(|c| c.resolution == ConflictResolution::Skip)
    );
    let filtered = preview_of(
        &target,
        &export,
        &ImportOptions {
            session_filter: Some(vec![other.id()]),
            workspace_filter: Some(vec![]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!((filtered.sessions, filtered.workspaces), (1, 0));
    assert!(filtered.conflicts.is_empty());

    export.format_version = "9.0.0".into();
    assert!(
        preview_of(&target, &export, &ImportOptions::default())
            .await
            .is_err()
    );
}

/// A 1.2.0 file (no cards, no messages, no new counts) still reads,
/// and imports.
#[tokio::test]
async fn a_1_2_0_archive_still_imports() {
    let session = ActiveSession::new(Uuid::new_v4(), Some("old".into()), None, chrono::Utc::now());
    let mut current = ExportData::new(
        EXPORT_FORMAT_VERSION,
        EXPORT_SCHEMA_URL,
        ExportType::Full,
        CompressionType::None,
        chrono::Utc::now(),
    );
    current.sessions.push(ExportedSession {
        session: StoredSession::new(session.clone(), vec![]),
        updates: vec![],
        messages: vec![],
    });
    current.update_counts();
    let mut json = serde_json::to_value(&current).unwrap();
    json["format_version"] = "1.2.0".into();
    json.as_object_mut().unwrap().remove("cards");
    let meta = json["metadata"].as_object_mut().unwrap();
    meta.remove("message_count");
    meta.remove("card_count");
    json["sessions"][0]
        .as_object_mut()
        .unwrap()
        .remove("messages");
    // In the order the writer puts the fields (a `Value` sorts its keys).
    let old = format!(
        "{{\"format_version\":{},\"schema\":{},\"metadata\":{},\"sessions\":{},\"workspaces\":{},\"checkpoints\":{},\"embeddings\":{}}}",
        json["format_version"],
        json["schema"],
        json["metadata"],
        json["sessions"],
        json["workspaces"],
        json["checkpoints"],
        json["embeddings"]
    );
    let old = decode(old.as_bytes()).unwrap();
    assert_eq!(old.format_version, "1.2.0");
    assert!(old.cards.is_empty() && old.sessions[0].messages.is_empty());

    let dir = tempdir().unwrap();
    let target = RealRocksDBStorage::new(dir.path()).await.unwrap();
    let result = import(&of(&target), old.clone(), &ImportOptions::default())
        .await
        .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.sessions_imported, 1);
    assert_eq!((result.messages_imported, result.cards_imported), (0, 0));

    {
        let target = post_cortex_storage::SurrealDBStorage::new("mem://", None, None, None, None)
            .await
            .unwrap();
        let result = import(&of(&target), old, &ImportOptions::default())
            .await
            .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.sessions_imported, 1);
    }
}

#[tokio::test]
async fn test_compressed_export() {
    let temp_dir = tempdir().expect("Failed to create temp directory");

    let export = ExportData::new(
        EXPORT_FORMAT_VERSION,
        EXPORT_SCHEMA_URL,
        ExportType::Full,
        CompressionType::Gzip,
        chrono::Utc::now(),
    );

    // Test gzip
    let gzip_path = temp_dir.path().join("export.json.gz");
    let options = ExportOptions {
        compression: CompressionType::Gzip,
        ..Default::default()
    };
    std::fs::write(&gzip_path, encode(&export, options.compression, false)).unwrap();
    let imported = decode(&std::fs::read(&gzip_path).unwrap()).expect("Failed to read gzip export");
    assert_eq!(imported.format_version, EXPORT_FORMAT_VERSION);

    // Test zstd
    let zstd_path = temp_dir.path().join("export.json.zst");
    let options = ExportOptions {
        compression: CompressionType::Zstd,
        ..Default::default()
    };
    std::fs::write(&zstd_path, encode(&export, options.compression, false)).unwrap();
    let imported = decode(&std::fs::read(&zstd_path).unwrap()).expect("Failed to read zstd export");
    assert_eq!(imported.format_version, EXPORT_FORMAT_VERSION);
}
