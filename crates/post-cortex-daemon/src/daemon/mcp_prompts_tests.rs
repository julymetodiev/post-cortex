// Copyright (c) 2026 Julius ML
// MIT License

//! MCP prompts end to end (ARCHITECTURE section 7, prompts): the daemon's
//! HTTP app with the skills library as a built-in offers the `prompts`
//! capability, lists each skill as a prompt and fills it; a prompt no
//! module offers is a JSON-RPC error.

use std::sync::Arc;

use post_cortex_memory::SystemConfig;
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::compose::{Builtin, BuiltinSpec, Modules};
use crate::daemon::rmcp_server::build_router;
use crate::daemon::test_client::{Client, policy};

#[tokio::test]
async fn skills_are_prompts_over_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &dir,
            SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let library = memory
        .workspaces
        .create(CreateWorkspace {
            name: "library".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id;
    let skills = BuiltinSpec {
        id: Builtin::Skills,
        grants: Vec::new(),
        filesystem: Vec::new(),
        config: Some(json!({ "workspace_id": library.to_string() })),
    };
    let modules = Modules::memory(Arc::clone(&memory))
        .unwrap()
        .configured(&[], &[], &[skills])
        .await
        .unwrap();
    let host = crate::compose::host(&modules).await.unwrap();
    let ct = CancellationToken::new();
    let router = build_router(Arc::new(host), memory, &policy(), None, &ct);
    let mut client = Client::connect(router).await;
    assert!(
        client.server["capabilities"]["prompts"].is_object(),
        "{}",
        client.server
    );

    client
        .call(
            "skill_add",
            json!({
                "name": "release",
                "description": "Cut a release",
                "instructions": "Bump the version, then tag it.",
                "tools": ["bash"]
            }),
        )
        .await;
    let listed = client.list_prompts().await;
    // The library's built-in skills are added at start, maybe already.
    let mine: Vec<&serde_json::Value> = listed["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["name"] != "workflow-builder" && p["name"] != "module-builder")
        .collect();
    assert_eq!(
        serde_json::Value::Array(mine.into_iter().cloned().collect()),
        json!([{ "name": "release", "description": "Cut a release" }])
    );
    let filled = client.get_prompt("release", json!({})).await;
    assert_eq!(filled["result"]["description"], json!("Cut a release"));
    assert_eq!(filled["result"]["messages"][0]["role"], json!("user"));
    assert_eq!(
        filled["result"]["messages"][0]["content"]["text"],
        json!("Bump the version, then tag it.\n\nTools this skill needs: bash.")
    );
    let missing = client.get_prompt("nope", json!({})).await;
    assert_eq!(missing["error"]["code"], json!(-32602), "{missing}");
    ct.cancel();
}
