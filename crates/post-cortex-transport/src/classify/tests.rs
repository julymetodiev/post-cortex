// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use post_cortex_memory::tools::operations::Counter;
use post_cortex_proto::pb;
use post_cortex_proto::views as v;
use prost::Message;
use serde_json::json;
use uuid::Uuid;

fn id() -> Uuid {
    Uuid::new_v4()
}

#[test]
fn mcp_calls_reach_their_operation_and_scope() {
    let s = id();
    let c = mcp(
        "session",
        &json!({ "action": "LOAD", "session_id": s.to_string() }),
    )
    .unwrap();
    assert_eq!(
        (c.operation.name, c.sessions.clone()),
        ("session.get", vec![s])
    );

    // The search scope selector names the session or workspace.
    let w = id();
    let c = mcp(
        "semantic_search",
        &json!({ "query": "q", "scope": "workspace", "scope_id": w.to_string(), "mode": "accurate" }),
    )
    .unwrap();
    assert_eq!(
        (c.operation.name, c.workspaces.clone(), c.rerank),
        ("search", vec![w], true)
    );
    let c = mcp("semantic_search", &json!({ "query": "q" })).unwrap();
    assert!(c.instance_wide() && !c.rerank);

    // Routed by argument: a bulk write counts its items, a dry run nothing.
    let write = json!({ "session_id": s.to_string(), "interaction_type": "qa", "content": {} });
    let c = mcp("update_conversation_context", &write).unwrap();
    assert_eq!(c.operation.name, "record.write");
    assert_eq!(
        c.counts,
        vec![Amount {
            counter: Counter::Records,
            by: By::Known(1)
        }]
    );
    let bulk = json!({ "session_id": s.to_string(), "updates": [{}, {}, {}] });
    let c = mcp("update_conversation_context", &bulk).unwrap();
    assert_eq!(c.operation.name, "record.bulk_write");
    assert_eq!(c.counts[0].by, By::Known(3));
    let dry = json!({ "session_id": s.to_string(), "updates": [{}], "dry_run": true });
    assert_eq!(
        mcp("update_conversation_context", &dry)
            .unwrap()
            .operation
            .name,
        "record.validate"
    );
    let not_dry = json!({ "session_id": s.to_string(), "dry_run": false });
    assert_eq!(
        mcp("update_conversation_context", &not_dry)
            .unwrap()
            .operation
            .name,
        "record.write"
    );

    // Deletes give back; answers say how many.
    let c = mcp(
        "session",
        &json!({ "action": "delete_messages", "session_id": s.to_string() }),
    )
    .unwrap();
    assert_eq!(c.counts[0].by, By::Answer("deleted"));

    // Projects name a key and a workspace by name, not an id.
    let c = mcp(
        "session",
        &json!({ "action": "resolve_project", "key": "github.com/a/b", "workspace": " acme " }),
    )
    .unwrap();
    assert!(c.instance_wide());
    assert_eq!(
        c.project,
        Some(ProjectKey {
            key: "github.com/a/b".into(),
            workspace: Some("acme".into()),
            name: None,
        })
    );
}

#[test]
fn mcp_answers_carry_their_counts() {
    let output = pcx_module_api::ToolOutput {
        text: "Deleted 4 messages".into(),
        data: Some(json!({ "session_id": "s", "deleted": 4, "remaining": 0 })),
        failed: false,
        meta: None,
    };
    let result = serde_json::to_value(crate::mcp::call_result(Ok(output))).unwrap();
    assert_eq!(mcp_answer_count(&result, "deleted").unwrap(), 4);
    let bare = serde_json::to_value(crate::mcp::call_result(Ok(pcx_module_api::ToolOutput {
        text: "done".into(),
        data: None,
        failed: false,
        meta: None,
    })))
    .unwrap();
    assert!(mcp_answer_count(&bare, "deleted").is_err());
}

/// Whether a search was reranked is read from the result's `_meta`, which a
/// `select` of its data keeps; a flag only in the data is not the answer.
#[test]
fn mcp_answers_carry_their_flags_in_their_meta() {
    let result = |data: serde_json::Value, meta: Option<serde_json::Value>| {
        serde_json::to_value(crate::mcp::call_result(Ok(pcx_module_api::ToolOutput {
            text: "Found 1 results".into(),
            data: Some(data),
            failed: false,
            meta: meta.and_then(|m| m.as_object().cloned()),
        })))
        .unwrap()
    };
    let selected = result(json!(["id"]), Some(json!({ "reranked": true })));
    assert_eq!(selected["_meta"], json!({ "reranked": true }));
    assert_eq!(mcp_answer_flag(&selected, "reranked"), Some(true));
    let unranked = result(json!(["id"]), Some(json!({ "reranked": false })));
    assert_eq!(mcp_answer_flag(&unranked, "reranked"), Some(false));
    let in_data_only = result(json!({ "reranked": true }), None);
    assert_eq!(mcp_answer_flag(&in_data_only, "reranked"), None);
}

#[test]
fn bad_mcp_calls_are_errors_not_defaults() {
    assert!(matches!(
        mcp("nope", &json!({})),
        Err(ClassifyError::UnknownTool(_))
    ));
    assert!(matches!(
        mcp("session", &json!({ "action": "explode" })),
        Err(ClassifyError::UnknownAction { .. })
    ));
    assert!(matches!(
        mcp(
            "session",
            &json!({ "action": "load", "session_id": "not-an-id" })
        ),
        Err(ClassifyError::BadId { .. })
    ));
    assert!(matches!(
        mcp(
            "update_conversation_context",
            &json!({ "session_id": id().to_string(), "updates": 3 })
        ),
        Err(ClassifyError::BadCount { .. })
    ));
}

#[test]
fn grpc_calls_read_their_message_through_the_descriptor() {
    let s = id();
    let message = pb::BulkUpdateContextRequest {
        session_id: s.to_string(),
        updates: vec![Default::default(), Default::default()],
        ..Default::default()
    }
    .encode_to_vec();
    let c = grpc("/pcx.v1.PostCortex/BulkUpdateContext", &message).unwrap();
    assert_eq!(
        (c.operation.name, c.sessions.clone()),
        ("record.bulk_write", vec![s])
    );
    assert_eq!(c.counts[0].by, By::Known(2));

    let search = pb::SemanticSearchRequest {
        query: "q".into(),
        mode: Some("local".into()),
        ..Default::default()
    }
    .encode_to_vec();
    let c = grpc("/pcx.v1.PostCortex/SemanticSearch", &search).unwrap();
    assert!(c.rerank && c.instance_wide());
    let fast = grpc_without_rerank("/pcx.v1.PostCortex/SemanticSearch", &search).unwrap();
    assert_eq!(
        pb::SemanticSearchRequest::decode(&fast[..])
            .unwrap()
            .mode
            .as_deref(),
        Some("fast")
    );
    assert_eq!(
        pb::SemanticSearchRequest::decode(&fast[..])
            .unwrap()
            .rerank_withheld
            .as_deref(),
        Some("plan_limit")
    );
    assert!(
        !grpc("/pcx.v1.PostCortex/SemanticSearch", &fast)
            .unwrap()
            .rerank
    );

    // An import preview writes nothing.
    let preview = pb::ImportChunk {
        data: Vec::new(),
        settings: Some(pb::ImportSettings {
            commit: false,
            ..Default::default()
        }),
    }
    .encode_to_vec();
    let c = grpc("/pcx.v1.PostCortex/Import", &preview).unwrap();
    assert!(c.preview && c.counts.is_empty());

    // Answers are read the same way.
    let answer = pb::DeleteMessagesResponse {
        deleted: 7,
        ..Default::default()
    }
    .encode_to_vec();
    assert_eq!(
        grpc_answer_count("/pcx.v1.PostCortex/DeleteMessages", &answer, "deleted").unwrap(),
        7
    );
    assert!(matches!(
        grpc("/pcx.v1.PostCortex/Nope", &[]),
        Err(ClassifyError::UnknownRpc(_))
    ));
    assert!(matches!(
        grpc("/pcx.v1.PostCortex/LoadSession", &[0xff, 0xff]),
        Err(ClassifyError::Malformed(_))
    ));
}

#[test]
fn view_calls_name_their_scope_message() {
    let w = id();
    let message = v::SearchRequest {
        scope: Some(v::Scope {
            scope: Some(v::scope::Scope::WorkspaceId(w.to_string())),
        }),
        query: "q".into(),
        mode: v::SearchMode::Accurate as i32,
        ..Default::default()
    }
    .encode_to_vec();
    let c = grpc("/pcx.views.v1.Views/Search", &message).unwrap();
    assert_eq!(
        (c.operation.name, c.workspaces.clone(), c.rerank),
        ("views.search", vec![w], true)
    );
    let fast = grpc_without_rerank("/pcx.views.v1.Views/Search", &message).unwrap();
    assert_eq!(
        v::SearchRequest::decode(&fast[..]).unwrap().mode,
        v::SearchMode::Fast as i32
    );
    assert_eq!(
        v::SearchRequest::decode(&fast[..])
            .unwrap()
            .rerank_withheld
            .as_deref(),
        Some("plan_limit")
    );

    let global = v::ListConversationsRequest {
        scope: Some(v::Scope {
            scope: Some(v::scope::Scope::Global(true)),
        }),
        ..Default::default()
    }
    .encode_to_vec();
    assert!(
        grpc("/pcx.views.v1.Views/ListConversations", &global)
            .unwrap()
            .instance_wide()
    );
    let s = id();
    let one = v::GetConversationRequest {
        session_id: s.to_string(),
        ..Default::default()
    }
    .encode_to_vec();
    assert_eq!(
        grpc("/pcx.views.v1.Views/GetConversation", &one)
            .unwrap()
            .sessions,
        vec![s]
    );
}

#[test]
fn rest_routes_fill_their_scope_from_path_query_and_body() {
    let s = id();
    let c = rest("DELETE", &format!("/api/sessions/{s}"), None, &Value::Null).unwrap();
    assert_eq!(
        (c.operation.name, c.sessions.clone()),
        ("session.delete", vec![s])
    );
    let w = id();
    let c = rest(
        "POST",
        &format!("/api/workspaces/{w}/sessions/{s}"),
        None,
        &Value::Null,
    )
    .unwrap();
    assert_eq!(
        (c.sessions.clone(), c.workspaces.clone()),
        (vec![s], vec![w])
    );
    let c = rest(
        "GET",
        "/api/cards",
        Some(&format!("workspace_id={w}")),
        &Value::Null,
    )
    .unwrap();
    assert_eq!(c.workspaces, vec![w]);
    let c = rest(
        "GET",
        "/api/file",
        Some(&format!("scope=session&id={s}&path=a.rs")),
        &Value::Null,
    )
    .unwrap();
    assert_eq!(c.sessions, vec![s]);
    let c = rest(
        "POST",
        "/api/messages",
        None,
        &json!({ "session_id": s.to_string(), "messages": [{}, {}] }),
    )
    .unwrap();
    assert_eq!(c.counts[0].by, By::Known(2));
    let c = rest(
        "POST",
        "/api/search",
        None,
        &json!({ "query": "q", "scope": "session", "id": s.to_string(), "mode": "local" }),
    )
    .unwrap();
    assert_eq!((c.sessions.clone(), c.rerank), (vec![s], true));
    let mut body = json!({ "query": "q", "mode": "accurate" });
    without_rerank(c.operation, &mut body);
    assert_eq!(body["mode"], "fast");
    assert_eq!(body["rerank_withheld"], "plan_limit");

    // A hook event names the scope of its envelope, not the agent's own
    // session id.
    let envelope = json!({
        "scope": { "session_id": s.to_string() },
        "event": { "hook_event_name": "Stop", "session_id": "agent-session-1" },
    });
    let c = rest("POST", "/api/events", None, &envelope).unwrap();
    assert_eq!(
        (c.operation.name, c.sessions.clone()),
        ("hook.event", vec![s])
    );

    assert!(matches!(
        rest("GET", "/api/nope", None, &Value::Null),
        Err(ClassifyError::UnknownRoute(_))
    ));
    assert!(matches!(
        rest("DELETE", "/api/workspaces/zzz", None, &Value::Null),
        Err(ClassifyError::BadId { .. })
    ));
    assert!(matches!(
        rest("POST", "/api/messages", None, &json!([1])),
        Err(ClassifyError::Malformed(_))
    ));
}

#[test]
fn every_operation_of_every_transport_classifies() {
    // Every MCP route, RPC and REST route reaches its own operation with an
    // empty request (no names: the whole instance).
    for op in post_cortex_memory::tools::operations::OPERATIONS {
        for route in op.mcp {
            let mut args = serde_json::Map::new();
            if let Some(selector) = post_cortex_memory::tools::operations::selector(route.tool)
                && let Some(value) = route.values.first()
            {
                args.insert(selector.into(), Value::from(*value));
            } else if let Some(value) = route.values.first() {
                // A routing argument: a flag, or a list.
                let given = if *value == "dry_run" {
                    Value::Bool(true)
                } else {
                    json!([])
                };
                args.insert((*value).into(), given);
            }
            let c = mcp(route.tool, &Value::Object(args)).unwrap();
            assert_eq!(c.operation.name, op.name, "mcp {}", route.tool);
        }
        for rpc in op.grpc {
            let c = grpc(&format!("/pcx.v1.PostCortex/{rpc}"), &[]).unwrap();
            assert_eq!(c.operation.name, op.name, "{rpc}");
        }
        for rpc in op.views {
            let c = grpc(&format!("/pcx.views.v1.Views/{rpc}"), &[]).unwrap();
            assert_eq!(c.operation.name, op.name, "views {rpc}");
        }
        for rpc in op.blobs {
            let c = grpc(&format!("/pcx.v1.Blobs/{rpc}"), &[]).unwrap();
            assert_eq!(c.operation.name, op.name, "blobs {rpc}");
        }
        for route in op.rest {
            let (method, template) = route.split_once(' ').unwrap();
            let path = template
                .split('/')
                .map(|p| {
                    if p.starts_with('{') {
                        id().to_string()
                    } else {
                        p.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("/");
            let c = rest(method, &path, None, &Value::Null).unwrap();
            assert_eq!(c.operation.name, op.name, "{route}");
        }
    }
}

#[test]
fn browser_transfer_is_the_rpc_it_maps_to() {
    let w = id();
    let c = browser(
        "POST",
        "/v1/import",
        Some(&format!("commit=true&workspace_id={w}")),
        b"",
    )
    .unwrap();
    assert_eq!(c.operation.name, "import");
    assert!(!c.preview);
    assert_eq!(c.counts.len(), 3);
    let preview = browser("POST", "/v1/import", None, b"archive bytes are not read").unwrap();
    assert!(preview.preview && preview.counts.is_empty());
    let c = browser(
        "POST",
        "/v1/export",
        None,
        format!(r#"{{"scope":"workspace","workspace_id":"{w}"}}"#).as_bytes(),
    )
    .unwrap();
    assert_eq!(
        (c.operation.name, c.workspaces.clone()),
        ("export", vec![w])
    );
    assert!(matches!(
        browser("POST", "/v1/import", Some("commit=perhaps"), b""),
        Err(ClassifyError::Malformed(_))
    ));
    assert!(matches!(
        browser("GET", "/v1/export/ticket", None, b""),
        Err(ClassifyError::UnknownRoute(_))
    ));
}
