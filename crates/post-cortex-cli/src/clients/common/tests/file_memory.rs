// Copyright (c) 2026 Julius ML
// MIT License

use super::*;

/// A file's memory as the memory module answers it: a held edit (its
/// reason), a read's context, or nothing; the session and the moment go
/// with each file.
#[tokio::test]
async fn file_memory_maps_the_memory_module_s_answer() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let recorded = seen.clone();
    let app = axum::Router::new().route(
        "/api/events",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let recorded = recorded.clone();
            async move {
                recorded.lock().unwrap().push(body["event"].clone());
                let path = body["event"]["tool_input"]["file_path"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                axum::Json(
                    match (body["event"]["hook_event_name"].as_str(), path.as_str()) {
                        (Some("PreToolUse"), "a.rs") => {
                            json!({ "decision": "block", "reason": "memory of a.rs" })
                        }
                        (Some("PostToolUse"), "a.rs") => json!({ "hookSpecificOutput": {
                        "hookEventName": "PostToolUse", "additionalContext": "memory of a.rs" } }),
                        _ => json!({}),
                    },
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    let env = HookEnv::new(&url, reqwest::Client::new(), "S1", "W1", "codex");
    let files = vec!["a.rs".to_string(), "b.rs".to_string()];

    let held = env
        .file_memory(&files, FileMoment::Edit, Some("agent-1"))
        .await;
    assert_eq!(held, Some(FileMemory::Held("memory of a.rs".into())));
    let out: Value =
        serde_json::from_str(&file_memory_output("PreToolUse", held.unwrap())).unwrap();
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(
        out["hookSpecificOutput"]["permissionDecisionReason"],
        "memory of a.rs"
    );

    let read = env.file_memory(&files, FileMoment::Read, None).await;
    assert_eq!(read, Some(FileMemory::Added("memory of a.rs".into())));
    let nothing = env
        .file_memory(&["b.rs".to_string()], FileMoment::Edit, None)
        .await;
    assert_eq!(nothing, None);

    let events = seen.lock().unwrap().clone();
    assert_eq!(events[0]["session_id"], "agent-1");
    assert_eq!(events[2]["hook_event_name"], "PostToolUse");
    assert_eq!(events[2]["session_id"], "");
}
