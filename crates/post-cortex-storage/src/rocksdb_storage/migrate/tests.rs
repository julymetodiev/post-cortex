// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Migration at open, from rows written in the 0.5.0 layouts.
//!
//! `tests/fixtures/rocksdb-0.5.0.kv` holds every key and value (hex, one
//! pair per line) of a database written by the code at 2ce045e, the last
//! with the 0.5.0 layouts: a row-backed session with a graph, a session
//! whose blob embeds its updates (no index, no listing record), a
//! checkpoint, an archived workspace with a project and a `Related`
//! member, source references in both protobuf layouts (state-prefixed and
//! bare) with a symbol dependency, the entity and edge rows nothing read,
//! an embedding and a card.

use std::path::Path;

use tempfile::{TempDir, tempdir};
use uuid::Uuid;

use super::{DATA_VERSION, DATA_VERSION_KEY, data_version};
use crate::rocksdb_storage::RealRocksDBStorage;
use crate::rocksdb_storage::rows::{
    CARD_V1, CHECKPOINT_V4, DISMISSAL_V1, EDGE_V1, EMBEDDING_V1, ENTITY_V1, MEMBERSHIP_V1,
    MESSAGE_V1, META_V1, SESSION_V2, UPDATE_V4, WORKSPACE_V6,
};
use crate::rocksdb_storage::source_rows::SOURCE_V2;
use crate::rocksdb_storage::v2_layout;
use post_cortex_core::core::card::CardScope;
use post_cortex_core::core::freshness::{FreshnessStatus, SourceScope, SymbolId};
use post_cortex_core::session::active_session::{ActiveSession, StoredSession};
use post_cortex_core::workspace::SessionRole;
use post_cortex_ports::storage::{
    CardStore, CheckpointStore, FreshnessStore, SessionStore, VectorStore, WorkspaceStore,
};

const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/rocksdb-0.5.0.kv"
));

mod version_2;
mod version_3;
mod version_4;
mod version_5;

/// Ids the fixture generator used.
fn uid(n: u128) -> Uuid {
    Uuid::from_u128(0x0005_0000_0000_4000_8000_0000_0000_0000 | n)
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

pub(crate) fn fixture_rows() -> Vec<(Vec<u8>, Vec<u8>)> {
    FIXTURE
        .lines()
        .map(|line| {
            let (k, v) = line.split_once(' ').unwrap_or((line, ""));
            (unhex(k), unhex(v))
        })
        .collect()
}

/// The value of `key` in the fixture.
pub(crate) fn fixture_row(key: &str) -> Vec<u8> {
    fixture_rows()
        .into_iter()
        .find(|(k, _)| k == key.as_bytes())
        .unwrap_or_else(|| panic!("{key} not in the fixture"))
        .1
}

/// A database holding exactly `rows` and no data version, as 0.5.0 left it.
async fn legacy_db(rows: &[(Vec<u8>, Vec<u8>)]) -> TempDir {
    let dir = tempdir().unwrap();
    {
        let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
        storage.db.delete(DATA_VERSION_KEY).unwrap();
        for (k, v) in rows {
            storage.db.put(k, v).unwrap();
        }
    }
    dir
}

async fn open(dir: &Path) -> RealRocksDBStorage {
    RealRocksDBStorage::new(dir).await.unwrap()
}

fn dump(storage: &RealRocksDBStorage) -> Vec<(Vec<u8>, Vec<u8>)> {
    storage
        .db
        .iterator(rocksdb::IteratorMode::Start)
        .map(|item| {
            let (k, v) = item.unwrap();
            (k.to_vec(), v.to_vec())
        })
        .collect()
}

fn titles(session: &ActiveSession) -> Vec<String> {
    session
        .records()
        .iter()
        .map(|u| u.content.title.clone())
        .collect()
}

/// Every row of a family carries its marker after the migration.
fn assert_all_marked(storage: &RealRocksDBStorage) {
    for (key, value) in dump(storage) {
        let key = String::from_utf8_lossy(&key).into_owned();
        assert!(
            !(key.starts_with("session:") && key.contains(":update:"))
                && !key.starts_with("session_updates:"),
            "version 2 row {key} left"
        );
        let marker: &[u8] = if key.starts_with("session_meta:") {
            META_V1
        } else if key.starts_with("session:") && key.contains(":rec:") {
            UPDATE_V4
        } else if key.starts_with("session:") && key.contains(":ent:") {
            ENTITY_V1
        } else if key.starts_with("session:") && key.contains(":edge:") {
            EDGE_V1
        } else if key.starts_with("session:")
            && (key.contains(":recid:") || key.contains(":vec:") || key.ends_with(":next"))
        {
            continue;
        } else if key.starts_with("session:") {
            SESSION_V2
        } else if key.starts_with("checkpoint:") {
            CHECKPOINT_V4
        } else if key.starts_with("workspace:") {
            WORKSPACE_V6
        } else if key.starts_with("ws_session:") {
            MEMBERSHIP_V1
        } else if key.starts_with("source_ref:") {
            SOURCE_V2
        } else if key.starts_with("embedding:") {
            EMBEDDING_V1
        } else if key.starts_with("message:") {
            MESSAGE_V1
        } else if key.starts_with("card:") {
            CARD_V1
        } else if key.starts_with("radar_dismissal:") {
            DISMISSAL_V1
        } else {
            assert!(
                !key.starts_with("entity:") && !key.starts_with("relationship:"),
                "graph row {key} left"
            );
            assert!(!key.starts_with("index_version:"), "old marker {key} left");
            continue;
        };
        assert!(value.starts_with(marker), "{key} not migrated");
    }
}

#[tokio::test]
async fn the_0_5_0_fixture_migrates_and_reads_back() {
    let (s1, legacy, checkpoint, ws) = (uid(1), uid(2), uid(3), uid(4));
    // Also an old database whose checkpoint index was never built.
    let rows: Vec<_> = fixture_rows()
        .into_iter()
        .filter(|(k, _)| !k.starts_with(b"checkpoint_by_session:"))
        .collect();
    let old_workspace =
        crate::rocksdb_storage::legacy::decode_workspace(&fixture_row(&format!("workspace:{ws}")))
            .unwrap();
    let dir = legacy_db(&rows).await;
    let storage = open(dir.path()).await;

    assert_eq!(data_version(&storage.db).unwrap(), DATA_VERSION);
    assert_all_marked(&storage);

    // Sessions: rows, graph, the embedded list, listings.
    let session = storage.load_session(s1).await.unwrap().session;
    assert_eq!(titles(&session), ["storage", "schema", "plain"]);
    assert_eq!(session.name().as_deref(), Some("rows"));
    for name in ["pcx", "RocksDB", "SurrealDB"] {
        assert!(session.entity_graph().has_entity(name), "{name}");
    }
    let old = storage.load_session(legacy).await.unwrap().session;
    assert_eq!(titles(&old), ["old one", "old two"]);
    assert!(old.entity_graph().has_entity("Old"));
    let mut summaries = storage.list_session_summaries().await.unwrap();
    summaries.sort_by_key(|s| s.id);
    let counts: Vec<_> = summaries.iter().map(|s| (s.id, s.update_count)).collect();
    assert_eq!(counts, [(s1, 3), (legacy, 2)]);
    assert_eq!(summaries[1].name.as_deref(), Some("legacy"));

    // Workspace and member.
    let workspaces = storage.list_workspaces().await.unwrap();
    assert_eq!(workspaces.len(), 1);
    let w = &workspaces[0];
    assert_eq!(
        (w.id, w.name.as_str(), w.description.as_str()),
        (ws, "ws", "workspace")
    );
    assert_eq!(w.created_at, old_workspace.created_at);
    assert_eq!(w.archived_at, Some(1_790_000_000));
    assert_eq!(w.projects[0].key, "github.com/o/r");
    assert_eq!(w.sessions, [(s1, SessionRole::Related)]);

    // Freshness: both protobuf layouts, the status, the function scope.
    let status = |entry: &'static str| {
        let storage = &storage;
        async move {
            storage
                .check_freshness(entry, b"file-hash")
                .await
                .unwrap()
                .status
        }
    };
    assert_eq!(status("fresh-fn").await, FreshnessStatus::Fresh);
    assert_eq!(status("stale-file").await, FreshnessStatus::Stale);
    assert_eq!(status("bare").await, FreshnessStatus::Fresh);
    let refs = storage.get_entries_by_source("src/a.rs").await.unwrap();
    let function = refs.iter().find(|r| r.entry_id == "fresh-fn").unwrap();
    match &function.scope {
        SourceScope::Function(f) => {
            assert_eq!(
                (f.name.as_str(), f.imports.as_slice()),
                ("foo", &["Bar".to_string()][..])
            );
        }
        other => panic!("scope {other:?}"),
    }
    assert_eq!(function.captured_at_unix, 1_790_000_000);
    let report = storage
        .cascade_invalidate(
            SymbolId {
                file_path: "src/b.rs".into(),
                symbol_name: "Bar".into(),
                symbol_type: "struct".into(),
            },
            vec![],
            3,
        )
        .await
        .unwrap();
    assert_eq!(report.cascade_invalidations, 1);
    assert_eq!(status("fresh-fn").await, FreshnessStatus::Stale);

    // Embedding and card untouched; the checkpoint is indexed again.
    assert_eq!(
        storage
            .get_session_vectors(&s1.to_string())
            .await
            .unwrap()
            .len(),
        1
    );
    let card = storage.get_card(CardScope::Workspace(ws)).await.unwrap();
    assert_eq!(card.unwrap().text, "goals");
    let listed = storage.list_checkpoints(s1).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|c| (c.id, c.recent_updates.len()))
            .collect::<Vec<_>>(),
        [(checkpoint, 3)]
    );
    storage.delete_session(s1).await.unwrap();
    assert!(storage.list_checkpoints(s1).await.unwrap().is_empty());
}

/// A second open changes nothing.
#[tokio::test]
async fn migration_runs_once() {
    let dir = legacy_db(&fixture_rows()).await;
    let first = dump(&open(dir.path()).await);
    let second = dump(&open(dir.path()).await);
    assert_eq!(first, second);
}

/// A database of a newer build is refused, not misread.
#[tokio::test]
async fn a_newer_data_version_is_refused() {
    let dir = tempdir().unwrap();
    open(dir.path())
        .await
        .db
        .put(DATA_VERSION_KEY, (DATA_VERSION + 1).to_string())
        .unwrap();
    let err = RealRocksDBStorage::new(dir.path()).await.err().unwrap();
    assert!(format!("{err:#}").contains("newer"), "{err:#}");
}

/// A blob that embeds its updates wins over a stale index (a downgraded
/// binary rewrote it): index entries it lacks lose their rows. Rows of
/// only some updates (written by `batch_save_updates` for such a session)
/// are completed.
#[tokio::test]
async fn an_embedded_update_list_is_authoritative() {
    let fixture_session = |id: Uuid, titles: &[&str]| {
        let mut session = ActiveSession::new(
            id,
            Some("s".into()),
            None,
            post_cortex_ports::conformance::at(0),
        );
        for (i, title) in titles.iter().enumerate() {
            session.apply_record(post_cortex_ports::conformance::record(i as i64, title, &[]));
        }
        session
    };
    let id = Uuid::new_v4();
    let old = fixture_session(id, &["u1", "u2"]);
    let dropped = old.records()[1].id;
    let mut newer = fixture_session(id, &["u3"]);
    newer.apply_record((*old.records()[0]).clone());

    let dir = tempdir().unwrap();
    {
        let storage = open(dir.path()).await;
        write_version_2(&storage, &old, &[]);
        storage.db.delete(DATA_VERSION_KEY).unwrap();
        storage
            .db
            .put(
                format!("session:{id}"),
                serde_json::to_vec(&StoredSession::new(newer, vec![])).unwrap(),
            )
            .unwrap();
    }
    let storage = open(dir.path()).await;
    assert_eq!(
        titles(&storage.load_session(id).await.unwrap().session),
        ["u3", "u1"]
    );
    let row = storage
        .db
        .get(format!("session:{id}:update:{dropped}"))
        .unwrap();
    assert!(row.is_none(), "the dropped update's row stays");
}

/// A row the migration cannot read is left, and reading its session is
/// `Corrupt` (not a session without the update).
#[tokio::test]
async fn an_unreadable_row_stays_corrupt() {
    let s1 = uid(1);
    let mut rows = fixture_rows();
    let bad = format!("session:{s1}:update:{}", uid(0x102));
    for (k, v) in &mut rows {
        if k == bad.as_bytes() {
            *v = b"\x01\x02 not an update".to_vec();
        }
    }
    let dir = legacy_db(&rows).await;
    let storage = open(dir.path()).await;
    let err = storage.load_session(s1).await.unwrap_err();
    assert!(err.is_corrupt(), "{err:#}");
    assert!(
        storage.load_session(uid(2)).await.is_ok(),
        "other sessions load"
    );
}

/// Data version 1 stored embedding, message, card and radar dismissal rows
/// as unmarked bincode; opening marks them (step 2) and they read back.
/// Before step 2 these reads decoded any bytes as the type (no layout check).
#[tokio::test]
async fn version_1_rows_get_their_markers() {
    use crate::rocksdb_storage::StoredEmbedding;
    use post_cortex_core::core::card::Card;
    use post_cortex_core::core::dismissal::{RadarDismissal, RadarItemKind};
    use post_cortex_core::core::message::{Message, MessageRole};
    use post_cortex_ports::storage::{DismissalStore, MessageStore};

    let (sid, ws) = (uid(901), uid(902));
    let at = chrono::DateTime::<chrono::Utc>::from_timestamp(1_760_000_000, 0).unwrap();
    let embedding = StoredEmbedding {
        content_id: "u1".into(),
        session_id: sid.to_string(),
        vector: vec![0.5; 4],
        text: "t".into(),
        content_type: "UpdateContent".into(),
        timestamp: at,
        metadata: Default::default(),
    };
    let message = Message {
        session_id: sid,
        seq: 0,
        turn_id: "t0".into(),
        role: MessageRole::User,
        author: None,
        source_session: None,
        timestamp: at,
        text: "hello".into(),
        tools: vec![],
        files: vec![],
        parent: None,
        blocks: Vec::new(),
    };
    let card = Card {
        scope: CardScope::Workspace(ws),
        version: 1,
        text: "goals".into(),
        updated_at: at,
        author: None,
    };
    let dismissal = RadarDismissal {
        kind: RadarItemKind::ALL[0],
        key: "k".into(),
        dismissed_at: at,
        author: None,
    };
    let rows = vec![
        (DATA_VERSION_KEY.to_vec(), b"1".to_vec()),
        (format!("embedding:{sid}:u1").into_bytes(), bin(&embedding)),
        (
            format!("message:{sid}:{:020}", 0).into_bytes(),
            // A row of version 1, as it was stored unmarked.
            bin(&crate::rocksdb_storage::message_rows::MessageRowV1::of(
                &message,
            )),
        ),
        (
            format!("message_next:{sid}").into_bytes(),
            1u64.to_be_bytes().to_vec(),
        ),
        (
            format!("message_count:{sid}").into_bytes(),
            1u64.to_be_bytes().to_vec(),
        ),
        (
            format!("card:{}:{:020}", CardScope::Workspace(ws).key(), 1).into_bytes(),
            bin(&card),
        ),
        (
            format!("card_head:{}", CardScope::Workspace(ws).key()).into_bytes(),
            1u64.to_be_bytes().to_vec(),
        ),
        (
            format!("radar_dismissal:{ws}:{}:k", dismissal.kind.as_str()).into_bytes(),
            bin(&dismissal),
        ),
    ];
    let dir = legacy_db(&rows).await;
    let storage = open(dir.path()).await;
    assert_eq!(data_version(&storage.db).unwrap(), DATA_VERSION);
    assert_all_marked(&storage);
    let vectors = storage.get_session_vectors(&sid.to_string()).await.unwrap();
    assert_eq!(vectors[0].1.id, "u1");
    assert_eq!(storage.load_messages(sid, 0..10).await.unwrap(), [message]);
    assert_eq!(
        storage.get_card(CardScope::Workspace(ws)).await.unwrap(),
        Some(card)
    );
    assert_eq!(storage.list_dismissals(ws).await.unwrap(), [dismissal]);
}

/// `session` in the layout of data version 2: blob (with the embedded
/// records `vectorized`), rows by update id and the update index.
fn write_version_2(storage: &RealRocksDBStorage, session: &ActiveSession, vectorized: &[Uuid]) {
    let id = session.id();
    let db = &storage.db;
    for update in session.records() {
        db.put(
            v2_layout::update_key(id, update.id),
            crate::rocksdb_storage::rows::encode_update(update).unwrap(),
        )
        .unwrap();
    }
    db.put(
        v2_layout::update_index_key(id),
        v2_layout::encode_ids(session.records().iter().map(|u| &u.id)),
    )
    .unwrap();
    db.put(
        format!("session:{id}"),
        v2_layout::encode_session(session, vectorized).unwrap(),
    )
    .unwrap();
}

/// Bincode of a value, as data version 1 wrote these rows.
fn bin<T: serde::Serialize>(value: &T) -> Vec<u8> {
    bincode::serde::encode_to_vec(value, bincode::config::standard()).unwrap()
}
