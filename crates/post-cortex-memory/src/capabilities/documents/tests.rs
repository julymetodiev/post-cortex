// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use serde_json::json;

use super::*;
use crate::capabilities::CreateWorkspace;
use crate::capabilities::test_support::rocksdb_memory;
use crate::memory_system::Change;

/// The kinds a running module declares, fixed.
struct Fixed(Vec<KindDeclaration>);

#[async_trait::async_trait]
impl DocumentKinds for Fixed {
    async fn kinds(&self) -> Result<Vec<KindDeclaration>, KindsError> {
        Ok(self.0.clone())
    }
}

/// The kinds of a `board` module, over real descriptors: `task` is a
/// `pcx.v1.SessionInfo` (its title `name`, its column `description`) and
/// `comment` a `pcx.v1.ImpactView` (its title `entity`, strings nested in
/// its `dependents`).
fn board_kinds() -> Fixed {
    let kind = |name: &str, type_name: &str, title_field: &str| KindDeclaration {
        kind: DocumentKind::new("board", name).unwrap(),
        type_name: type_name.to_string(),
        title_field: title_field.to_string(),
        searchable: true,
        descriptors: post_cortex_proto::PCX_DESCRIPTOR_SET.to_vec(),
    };
    Fixed(vec![
        kind("task", "pcx.v1.SessionInfo", "name"),
        kind("comment", "pcx.v1.ImpactView", "entity"),
    ])
}

fn task() -> DocumentKind {
    "board/task".parse().unwrap()
}

/// A task's data as the MCP tool gives it: JSON of its message.
fn data(value: Value) -> DocumentData {
    DocumentData::Json(value.as_object().cloned().unwrap())
}

/// A task's data as its message, `name` its title and `description` its
/// column.
fn task_data(title: &str, column: &str) -> DocumentData {
    use prost::Message;
    let info = post_cortex_proto::pb::SessionInfo {
        name: title.to_string(),
        description: column.to_string(),
        ..Default::default()
    };
    DocumentData::Message {
        type_name: "pcx.v1.SessionInfo".to_string(),
        bytes: info.encode_to_vec(),
    }
}

/// The JSON of `document`'s data.
async fn json_of(memory: &crate::capabilities::Memory, document: &Document) -> Value {
    let data = memory
        .documents
        .json_data(std::slice::from_ref(document), &board_kinds())
        .await
        .unwrap();
    Value::Object(data.into_iter().next().unwrap())
}

fn put(workspace_id: Uuid, id: Option<Uuid>, value: Value, expected_version: u64) -> PutDocument {
    PutDocument {
        workspace_id,
        id,
        kind: task(),
        data: data(value),
        text: "the task".to_string(),
        author: Some("claude".to_string()),
        expected_version,
    }
}

async fn workspace(memory: &crate::capabilities::Memory) -> Uuid {
    memory
        .workspaces
        .create(CreateWorkspace {
            name: "docs".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn the_owning_module_writes_versions_and_a_stale_one_conflicts() {
    let (memory, _dir) = rocksdb_memory().await;
    let docs = &memory.documents;
    let kinds = board_kinds();
    let board = Writer {
        module: Some("board"),
        kinds: &kinds,
        reach: None,
    };
    let ws = workspace(&memory).await;
    let mut feed = memory.changes.subscribe();

    let first = docs
        .put(
            put(ws, None, json!({"name": "Ship", "description": "todo"}), 0),
            board,
        )
        .await
        .unwrap();
    assert!(first.written);
    let first = first.document.unwrap();
    assert_eq!(
        (first.version, first.kind.to_string()),
        (1, "board/task".to_string())
    );
    match feed.recv().await.unwrap() {
        Change::Document(change) => {
            assert_eq!((change.id, change.version), (first.id, Some(1)));
        }
        other => panic!("expected a document change, got {other:?}"),
    }

    let moved = put(
        ws,
        Some(first.id),
        json!({"name": "Ship", "description": "doing"}),
        1,
    );
    let second = docs.put(moved.clone(), board).await.unwrap();
    assert_eq!(second.document.as_ref().map(|d| d.version), Some(2));
    // A second writer that read version 1 too gets the latest back.
    let late = docs.put(moved, board).await.unwrap();
    assert!(!late.written);
    assert_eq!(late.document, second.document);

    assert_eq!(docs.get(ws, first.id, Some(1)).await.unwrap(), first);
    let history = docs.history(ws, first.id, 10).await.unwrap();
    assert_eq!(
        history.iter().map(|d| d.version).collect::<Vec<_>>(),
        [2, 1]
    );
}

#[tokio::test]
async fn writes_are_refused_unless_they_fit() {
    let (memory, _dir) = rocksdb_memory().await;
    let docs = &memory.documents;
    let kinds = board_kinds();
    let board = Writer {
        module: Some("board"),
        kinds: &kinds,
        reach: None,
    };
    let ws = workspace(&memory).await;
    let ok = || put(ws, None, json!({"name": "Ship"}), 0);

    // Another module, or no module, does not write the board's kinds.
    for module in [Some("skills"), None] {
        let writer = Writer {
            module,
            kinds: &kinds,
            reach: None,
        };
        assert!(matches!(
            docs.put(ok(), writer).await,
            Err(DocumentError::Denied(_))
        ));
    }
    // A kind no running module declares.
    let undeclared = Writer {
        module: Some("board"),
        kinds: &Fixed(Vec::new()),
        reach: None,
    };
    assert!(matches!(
        docs.put(ok(), undeclared).await,
        Err(DocumentError::InvalidArgument(_))
    ));
    // Data that is not the kind's message: a missing title, a wrong type,
    // an unknown field, another message, bytes that do not decode.
    let mut refusals: Vec<DocumentData> = [
        json!({"description": "todo"}),
        json!({"name": 7}),
        json!({"name": "x", "owner": "me"}),
    ]
    .into_iter()
    .map(data)
    .collect();
    refusals.push(DocumentData::Message {
        type_name: "pcx.v1.ImpactView".to_string(),
        bytes: Vec::new(),
    });
    refusals.push(DocumentData::Message {
        type_name: "pcx.v1.SessionInfo".to_string(),
        bytes: vec![0xff, 0xff, 0xff],
    });
    for bad in refusals {
        let mut req = ok();
        req.data = bad;
        let refused = docs.put(req, board).await;
        assert!(
            matches!(refused, Err(DocumentError::InvalidArgument(_))),
            "{refused:?}"
        );
    }
    // Ids: memory gives a new document its id; a change names one.
    let named_new = put(ws, Some(Uuid::new_v4()), json!({"name": "x"}), 0);
    assert!(matches!(
        docs.put(named_new, board).await,
        Err(DocumentError::InvalidArgument(_))
    ));
    let unnamed_change = put(ws, None, json!({"name": "x"}), 1);
    assert!(matches!(
        docs.put(unnamed_change, board).await,
        Err(DocumentError::InvalidArgument(_))
    ));
    // Limits.
    let mut long = ok();
    long.text = "x".repeat(MAX_DOCUMENT_TEXT_BYTES + 1);
    assert!(matches!(
        docs.put(long, board).await,
        Err(DocumentError::InvalidArgument(_))
    ));
    let big = put(
        ws,
        None,
        json!({"name": "x".repeat(MAX_DOCUMENT_DATA_BYTES)}),
        0,
    );
    assert!(matches!(
        docs.put(big, board).await,
        Err(DocumentError::InvalidArgument(_))
    ));
    // An unknown workspace.
    let elsewhere = put(Uuid::new_v4(), None, json!({"name": "x"}), 0);
    assert!(matches!(
        docs.put(elsewhere, board).await,
        Err(DocumentError::WorkspaceNotFound(_))
    ));
}

#[tokio::test]
async fn secrets_are_redacted_in_text_and_data() {
    let (memory, _dir) = rocksdb_memory().await;
    let kinds = board_kinds();
    let board = Writer {
        module: Some("board"),
        kinds: &kinds,
        reach: None,
    };
    let ws = workspace(&memory).await;
    let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH";
    let mut req = put(ws, None, json!({"name": format!("key {key}")}), 0);
    req.text = format!("use {key}");
    let written = memory
        .documents
        .put(req, board)
        .await
        .unwrap()
        .document
        .unwrap();
    assert!(!written.text.contains(key), "{}", written.text);
    assert!(!written.title.contains(key), "{}", written.title);
    let stored = json_of(&memory, &written).await;
    assert!(!stored["name"].as_str().unwrap().contains(key), "{stored}");
    // A string nested at any depth of the message.
    let comment = PutDocument {
        kind: "board/comment".parse().unwrap(),
        data: data(json!({
            "entity": "cards",
            "dependents": [{ "entity": "sessions", "context": format!("token {key}") }]
        })),
        ..put(ws, None, json!({}), 0)
    };
    let written = memory
        .documents
        .put(comment, board)
        .await
        .unwrap()
        .document
        .unwrap();
    let stored = json_of(&memory, &written).await;
    let context = stored["dependents"][0]["context"].as_str().unwrap();
    assert!(!context.contains(key), "{context}");
}

#[tokio::test]
async fn only_the_owning_module_deletes() {
    let (memory, _dir) = rocksdb_memory().await;
    let docs = &memory.documents;
    let kinds = board_kinds();
    let board = Writer {
        module: Some("board"),
        kinds: &kinds,
        reach: None,
    };
    let ws = workspace(&memory).await;
    let id = docs
        .put(put(ws, None, json!({"name": "x"}), 0), board)
        .await
        .unwrap()
        .document
        .unwrap()
        .id;
    let other = Writer {
        module: Some("skills"),
        kinds: &kinds,
        reach: None,
    };
    assert!(matches!(
        docs.delete(ws, id, other).await,
        Err(DocumentError::Denied(_))
    ));
    docs.delete(ws, id, board).await.unwrap();
    assert!(matches!(
        docs.get(ws, id, None).await,
        Err(DocumentError::NotFound(_))
    ));
    assert!(matches!(
        docs.delete(ws, id, board).await,
        Err(DocumentError::NotFound(_))
    ));
}

#[tokio::test]
async fn search_finds_the_latest_version_under_its_workspace() {
    use crate::capabilities::SearchScope;
    use crate::content_vectorizer::{ContentKind, HitSource, SearchOptions};
    use post_cortex_core::workspace::SessionRole;
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let memory = crate::memory_system::backends_tests::rocksdb_memory_in(
        &dir,
        crate::SystemConfig::default(),
    )
    .await
    .unwrap();
    let kinds = board_kinds();
    let board = Writer {
        module: Some("board"),
        kinds: &kinds,
        reach: None,
    };
    let ws = workspace(&memory).await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    memory
        .workspaces
        .add_session(ws, sid, SessionRole::Primary)
        .await
        .unwrap();
    let mut req = put(ws, None, json!({"name": "Paint the lighthouse"}), 0);
    req.text = "The lighthouse keeper repaints the tower before winter".to_string();
    let first = memory
        .documents
        .put(req, board)
        .await
        .unwrap()
        .document
        .unwrap();
    let mut moved = put(
        ws,
        Some(first.id),
        json!({"name": "Paint the lighthouse", "description": "doing"}),
        1,
    );
    moved.text = "The lighthouse keeper repaints the tower before winter storms".to_string();
    memory.documents.put(moved, board).await.unwrap();
    assert!(memory.records.wait_idle(Duration::from_secs(120)).await);

    let search = |scope| {
        memory.search.search(
            scope,
            "lighthouse keeper repaints",
            SearchOptions::default(),
        )
    };
    let documents = |hits: Vec<crate::content_vectorizer::SemanticSearchResult>| {
        hits.into_iter()
            .filter(|h| h.content_type == ContentKind::Document)
            .collect::<Vec<_>>()
    };
    let found = documents(search(SearchScope::Workspace(ws)).await.unwrap());
    assert_eq!(
        found.len(),
        1,
        "one entry for the latest version: {found:?}"
    );
    let hit = &found[0];
    assert_eq!(hit.source, HitSource::Workspace(ws));
    assert_eq!(hit.title.as_deref(), Some("Paint the lighthouse"));
    assert!(
        hit.text_content.ends_with("winter storms"),
        "{}",
        hit.text_content
    );
    assert_eq!(
        hit.document.as_ref().map(|d| (d.kind.as_str(), d.version)),
        Some(("board/task", 2))
    );
    assert_eq!(
        documents(search(SearchScope::Global).await.unwrap()).len(),
        1
    );
    assert!(documents(search(SearchScope::Session(sid)).await.unwrap()).is_empty());

    memory.documents.delete(ws, first.id, board).await.unwrap();
    assert!(documents(search(SearchScope::Workspace(ws)).await.unwrap()).is_empty());
}

mod list;
mod migrate;
mod searchable;
