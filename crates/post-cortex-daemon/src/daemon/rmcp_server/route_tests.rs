// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The REST routes: file memory, session attach across a restart, and
//! search over records and messages.

use super::tests::{get, policy, post_json, status_and_body};
use super::*;
use axum::http::StatusCode;
use uuid::Uuid;

/// `GET /api/file` for the editor hook: session and workspace scope,
/// path matching, superseded decisions left out, error statuses; the
/// graph data API is not served.
#[tokio::test]
async fn file_api_serves_the_file_view() {
    use post_cortex_core::core::context_update::{
        EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateMeta,
        UpdateType,
    };
    use post_cortex_core::services::UpdateContextRequest;

    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &dir,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let service = memory.clone();
    let ct = CancellationToken::new();
    let router =
        crate::daemon::test_client::router(memory.clone(), &policy("127.0.0.1", &[]), None, &ct)
            .await;
    let session = memory
        .sessions
        .create(Some("graph".into()), None)
        .await
        .unwrap();

    let entity = |name: &str| EntityData {
        name: name.into(),
        entity_type: EntityType::Technology,
        first_mentioned: chrono::Utc::now(),
        last_mentioned: chrono::Utc::now(),
        mention_count: 1,
        importance_score: 1.0,
        description: None,
        aliases: Vec::new(),
    };
    let code = post_cortex_core::core::context_update::CodeReference {
        file_path: "crates/store/src/rocks.rs".into(),
        start_line: 1,
        end_line: 9,
        code_snippet: String::new(),
        commit_hash: None,
        branch: None,
        change_description: String::new(),
    };
    let write = |title: &str, store: &str, meta: UpdateMeta| UpdateContextRequest {
        session_id: session,
        interaction_type: UpdateType::DecisionMade,
        content: UpdateContent {
            title: title.into(),
            description: format!("{title} because"),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        entities: vec![entity("pcx"), entity(store)],
        relations: vec![EntityRelationship {
            from_entity: "pcx".into(),
            to_entity: store.into(),
            relation_type: RelationType::DependsOn,
            context: "stores data".into(),
        }],
        code_reference: Some(code.clone()),
        author: Some("claude-code".into()),
        derived_from: vec![],
        meta,
    };
    let first = service
        .records
        .write(write("use RocksDB", "RocksDB", UpdateMeta::default()))
        .await
        .unwrap();
    let bad = service
        .records
        .write(write(
            "x",
            "rocks-db",
            UpdateMeta {
                supersedes: vec![Uuid::new_v4()],
                ..Default::default()
            },
        ))
        .await;
    assert!(bad.is_err(), "superseding an unknown record is rejected");
    service
        .records
        .write(write(
            "move to rocks-db v2",
            "rocks-db",
            UpdateMeta {
                supersedes: vec![first.entry_id],
                confidence: Some(0.75),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    // The graph merge runs on the background pipeline.
    for _ in 0..200 {
        let s = memory.sessions.get(session).await.unwrap();
        if s.entity_graph()
            .entity_mentions
            .get("RocksDB")
            .map_or(0, post_cortex_core::collections::SharedVec::len)
            == 2
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let json = |text: &str| -> serde_json::Value { serde_json::from_str(text).unwrap() };
    let host = "localhost:3737";
    let path = "/home/someone/repo/crates/store/src/rocks.rs";
    let (status, body) = status_and_body(
        &router,
        get(&format!("/api/file?session_id={session}&path={path}"), host),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let view = json(&body);
    assert_eq!(view["records"].as_array().unwrap().len(), 2);
    let decisions = view["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 1, "the superseded decision is left out");
    assert_eq!(decisions[0]["confidence"], 0.75);

    // Workspace scope.
    let (_, body) = status_and_body(
        &router,
        post_json("/api/workspaces", serde_json::json!({"name": "ws"})),
    )
    .await;
    let ws = json(&body)["id"].as_str().unwrap().to_string();
    let (status, _) = status_and_body(
        &router,
        post_json(
            &format!("/api/workspaces/{ws}/sessions/{session}"),
            serde_json::json!({}),
        ),
    )
    .await;
    assert!(status.is_success());
    let (status, body) = status_and_body(
        &router,
        get(
            &format!("/api/file?scope=workspace&id={ws}&path={path}"),
            host,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["records"].as_array().unwrap().len(), 2);

    // Errors, and the graph data API is not part of the daemon.
    for (path, expected) in [
        ("/api/file?path=x.rs".to_string(), StatusCode::BAD_REQUEST),
        (
            format!("/api/file?session_id={session}"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/api/file?session_id={session}&path=x.rs&limit=x"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/api/file?scope=team&id={session}&path=x.rs"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/api/file?session_id={}&path=x.rs", Uuid::new_v4()),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/api/graph?session_id={session}"),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/api/timeline?session_id={session}"),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (status, body) = status_and_body(&router, get(&path, host)).await;
        assert_eq!(status, expected, "{path}: {body}");
    }
    ct.cancel();
}

/// `POST /api/workspaces/{ws}/sessions/{sid}` stores the membership
/// with its role (a restart finds it); an unknown workspace is 404.
#[tokio::test]
async fn api_attach_session_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let rocks = post_cortex_storage::RealRocksDBStorage::new(dir.path())
        .await
        .unwrap();
    let start = || async {
        Arc::new(
            crate::memory_tests::over_rocksdb(
                rocks.clone(),
                post_cortex_memory::SystemConfig {
                    enable_embeddings: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
        )
    };
    let memory = start().await;
    let ct = CancellationToken::new();
    let router =
        crate::daemon::test_client::router(memory.clone(), &policy("127.0.0.1", &[]), None, &ct)
            .await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    let attach = |ws: &str| {
        post_json(
            &format!("/api/workspaces/{ws}/sessions/{sid}"),
            serde_json::json!({"role": "shared"}),
        )
    };

    let (status, _) = status_and_body(&router, attach(&Uuid::new_v4().to_string())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, text) = status_and_body(
        &router,
        post_json("/api/workspaces", serde_json::json!({"name": "ws"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let ws: serde_json::Value = serde_json::from_str(&text).unwrap();
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, text) = status_and_body(&router, attach(&ws)).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    ct.cancel();
    drop(router);
    drop(memory);

    let restarted = start().await;
    let sessions = restarted
        .workspaces
        .get(ws.parse().unwrap())
        .unwrap()
        .get_all_sessions();
    assert_eq!(
        sessions,
        vec![(sid, post_cortex_core::workspace::SessionRole::Shared)]
    );
}

/// `POST /api/search`: records and captured turns in session,
/// workspace and global scope, the author filter, and 400 on bad input.
#[tokio::test]
async fn api_search_finds_records_and_messages() {
    use post_cortex_core::core::context_update::{
        EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
    };
    use post_cortex_core::services::UpdateContextRequest;

    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &dir,
            post_cortex_memory::SystemConfig {
                enable_embeddings: true,
                // Vectorized below, not by the background pipeline.
                auto_vectorize_on_update: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let service = memory.clone();
    let ct = CancellationToken::new();
    let router =
        crate::daemon::test_client::router(memory.clone(), &policy("127.0.0.1", &[]), None, &ct)
            .await;
    let session = memory
        .sessions
        .create(Some("search".into()), None)
        .await
        .unwrap();
    let entity = |name: &str| EntityData {
        name: name.into(),
        entity_type: EntityType::Technology,
        first_mentioned: chrono::Utc::now(),
        last_mentioned: chrono::Utc::now(),
        mention_count: 1,
        importance_score: 1.0,
        description: None,
        aliases: Vec::new(),
    };
    let written = service
        .records
        .write(UpdateContextRequest {
            session_id: session,
            interaction_type: UpdateType::DecisionMade,
            content: UpdateContent {
                title: "Store sessions in RocksDB".into(),
                description: "RocksDB keeps the session database embedded, with no server to run."
                    .into(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            entities: vec![entity("pcx"), entity("RocksDB")],
            relations: vec![EntityRelationship {
                from_entity: "pcx".into(),
                to_entity: "RocksDB".into(),
                relation_type: RelationType::DependsOn,
                context: "stores sessions".into(),
            }],
            code_reference: None,
            author: Some("claude-code".into()),
            derived_from: vec![],
            meta: Default::default(),
        })
        .await
        .unwrap();
    let (status, text) = status_and_body(
        &router,
        post_json(
            "/api/messages",
            serde_json::json!({"session_id": session, "messages": [
                {"turn_id": "u1", "role": "user", "author": "julius",
                 "text": "Can we deploy the billing service to Kubernetes on Friday afternoon?"}
            ]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    memory.admin.vectorize_session(session).await.unwrap();

    let search = |body: serde_json::Value| {
        let router = router.clone();
        async move {
            let (status, text) = status_and_body(&router, post_json("/api/search", body)).await;
            let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
            (status, value, text)
        }
    };
    let ids = |v: &serde_json::Value| -> Vec<String> {
        v["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect()
    };

    // A record: title, snippet without it, author, score, session.
    let (status, v, text) = search(serde_json::json!({
        "query": "which database stores the sessions", "scope": "session", "id": session,
        "mode": "fast"
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let hit = &v["results"][0];
    assert_eq!(hit["id"], written.entry_id.to_string(), "{text}");
    assert_eq!(hit["title"], "Store sessions in RocksDB");
    assert!(
        hit["snippet"]
            .as_str()
            .unwrap()
            .starts_with("RocksDB keeps"),
        "{text}"
    );
    assert_eq!(hit["author"], "claude-code");
    assert_eq!(hit["session_id"], session.to_string());
    assert_eq!(hit["content_type"], "DecisionPoint");
    assert!(hit["score"].as_f64().unwrap() > 0.0);
    assert!(hit["timestamp"].is_string());
    assert!(
        !ids(&v).iter().any(|id| id.starts_with("message:")),
        "records only by default"
    );

    // A captured turn, in workspace scope.
    // Stored: a membership needs its workspace.
    let workspace = memory
        .workspaces
        .create(post_cortex_memory::capabilities::CreateWorkspace {
            name: "ws".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id;
    let (status, v, _) = search(serde_json::json!({
        "query": "deploy billing to kubernetes", "scope": "workspace", "id": workspace,
        "sources": "messages"
    }))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ids(&v).is_empty(), "an empty workspace has no results");
    memory
        .workspaces
        .add_session(
            workspace,
            session,
            post_cortex_core::workspace::SessionRole::Primary,
        )
        .await
        .unwrap();
    let (status, v, text) = search(serde_json::json!({
        "query": "deploy billing to kubernetes", "scope": "workspace", "id": workspace,
        "sources": "messages", "limit": 1
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(ids(&v), ["message:0"], "{text}");
    assert_eq!(v["results"][0]["content_type"], "Message");
    assert_eq!(v["results"][0]["author"], "julius");

    // Global scope with both kinds; the author filter.
    let (status, v, text) = search(serde_json::json!({
        "query": "RocksDB sessions and the Kubernetes deploy", "scope": "global",
        "sources": "all"
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let found = ids(&v);
    assert!(found.contains(&written.entry_id.to_string()), "{text}");
    assert!(found.contains(&"message:0".to_string()), "{text}");
    let (_, v, text) = search(serde_json::json!({
        "query": "RocksDB sessions and the Kubernetes deploy", "scope": "global",
        "sources": "all", "author": "julius"
    }))
    .await;
    assert_eq!(ids(&v), ["message:0"], "{text}");

    // Bad input.
    for body in [
        serde_json::json!({"query": "  ", "scope": "global"}),
        serde_json::json!({"query": "x", "scope": "team"}),
        serde_json::json!({"query": "x"}),
        serde_json::json!({"query": "x", "scope": "session"}),
        serde_json::json!({"query": "x", "scope": "session", "id": "not-a-uuid"}),
        serde_json::json!({"query": "x", "scope": "global", "mode": "slow"}),
        serde_json::json!({"query": "x", "scope": "global", "sources": "files"}),
        serde_json::json!({"query": "x", "scope": "global", "limit": 0}),
        serde_json::json!({"query": "x", "scope": "global", "limit": 51}),
    ] {
        let (status, _, text) = search(body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} -> {text}");
    }
    let (status, _, _) =
        search(serde_json::json!({"query": "x", "scope": "workspace", "id": Uuid::new_v4()})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    ct.cancel();
}
