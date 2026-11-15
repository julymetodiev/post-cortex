// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! One real turn against Z.AI, only when asked: the key is read from the
//! JSON file (`{"api_key": ...}`) `PCX_ZAI_KEY_FILE` names.
//!
//! ```sh
//! PCX_ZAI_KEY_FILE=~/.axon/zai_auth.json cargo test -p pcx-provider-zai --test live -- --ignored
//! ```

use std::time::{Duration, Instant};

use pcx_module_api::Scope;
use pcx_module_api::model::{
    Block, KeyedProvider, ModelMessage, ModelProvider, ModelRequest, ModelTool, Role, StopReason,
    ToolChoice,
};
use pcx_provider_zai as zai;
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: None,
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(120),
        cancel: CancellationToken::new(),
    }
}

#[tokio::test]
#[ignore = "calls Z.AI: set PCX_ZAI_KEY_FILE and pass --ignored"]
async fn glm_answers_and_calls_a_tool() {
    let file = std::env::var("PCX_ZAI_KEY_FILE").expect("PCX_ZAI_KEY_FILE names the key file");
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let provider = zai::provider();
    provider
        .start(zai::ENDPOINT, stored["api_key"].as_str().unwrap())
        .await
        .unwrap();
    // Its listing, begun at start: glm-5.3 as its table knows it, and the
    // others Z.AI serves.
    let models = provider.listed().await.unwrap();
    eprintln!(
        "listed {:?}",
        models.iter().map(|m| &m.id).collect::<Vec<_>>()
    );
    let glm = models.iter().find(|m| m.id == "glm-5.3").unwrap();
    assert!(glm.context_tokens == 1_048_576);
    assert!(models.len() > 1);

    let request = ModelRequest {
        model: "glm-5.3".into(),
        system: "Use the tool to read files.".into(),
        messages: vec![ModelMessage {
            role: Role::User,
            blocks: vec![Block::Text {
                text: "What is in README.md?".into(),
            }],
        }],
        tools: vec![ModelTool {
            name: "fs_read".into(),
            description: "Read a file".into(),
            input_schema: json!({ "type": "object",
                "properties": { "path": { "type": "string" } }, "required": ["path"] })
            .as_object()
            .cloned()
            .unwrap(),
        }],
        max_tokens: 2000,
        tool_choice: ToolChoice::Auto,
    };
    let (blocks, usage, stop) = provider
        .generate(&scope(), request)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    eprintln!("stop {stop:?}, usage {usage:?}, blocks {blocks:?}");
    assert_eq!(stop, StopReason::ToolUse);
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b, Block::ToolUse { name, .. } if name == "fs_read"))
    );
}
