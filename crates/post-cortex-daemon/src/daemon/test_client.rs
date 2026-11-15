// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! A minimal MCP client over one `rmcp_server::build_router` router, for
//! tests that drive tools end to end (initialize, then `tools/call` over the
//! streamable HTTP transport).

use crate::daemon::DaemonConfig;
use crate::daemon::host_allowlist::HostPolicy;
use crate::daemon::rmcp_server::build_router;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, header};
use futures::StreamExt;
use post_cortex_memory::Memory;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const HOST: &str = "localhost:3737";

/// A minimal MCP client over one router: initialize, then tool calls.
pub(crate) struct Client {
    router: Router,
    session: String,
    next_id: u64,
    /// The server's `initialize` result.
    pub(crate) server: serde_json::Value,
}

impl Client {
    pub(crate) async fn connect(router: Router) -> Self {
        let init = serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                       "clientInfo": {"name": "test-client", "version": "0"}},
        });
        let resp = router.clone().oneshot(request(&init, None)).await.unwrap();
        assert!(resp.status().is_success(), "initialize: {}", resp.status());
        let session = resp
            .headers()
            .get("mcp-session-id")
            .expect("initialize returns an MCP session id")
            .to_str()
            .unwrap()
            .to_string();
        let server = read_response(resp.into_body(), 0).await["result"].clone();
        let initialized =
            serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        let resp = router
            .clone()
            .oneshot(request(&initialized, Some(&session)))
            .await
            .unwrap();
        assert!(resp.status().is_success(), "initialized: {}", resp.status());
        Self {
            router,
            session,
            next_id: 1,
            server,
        }
    }

    /// Sends one JSON-RPC request and returns the whole response message.
    async fn send(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        });
        let resp = self
            .router
            .clone()
            .oneshot(request(&body, Some(&self.session)))
            .await
            .unwrap();
        assert!(resp.status().is_success(), "{method}: {}", resp.status());
        read_response(resp.into_body(), id).await
    }

    /// Calls `tool` and returns the whole JSON-RPC response message.
    pub(crate) async fn call_raw(
        &mut self,
        tool: &str,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        self.send(
            "tools/call",
            serde_json::json!({"name": tool, "arguments": arguments}),
        )
        .await
    }

    /// The `tools/list` result.
    pub(crate) async fn list_tools(&mut self) -> serde_json::Value {
        self.send("tools/list", serde_json::json!({})).await["result"].clone()
    }

    /// The `prompts/list` result.
    pub(crate) async fn list_prompts(&mut self) -> serde_json::Value {
        self.send("prompts/list", serde_json::json!({})).await["result"].clone()
    }

    /// Gets prompt `name` with `arguments`; the whole JSON-RPC response
    /// message.
    pub(crate) async fn get_prompt(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        self.send(
            "prompts/get",
            serde_json::json!({"name": name, "arguments": arguments}),
        )
        .await
    }

    /// Calls `tool` and returns its text blocks joined; panics on a
    /// JSON-RPC error or a tool error.
    pub(crate) async fn call(&mut self, tool: &str, arguments: serde_json::Value) -> String {
        let message = self.call_raw(tool, arguments).await;
        assert!(message.get("error").is_none(), "{tool}: {message}");
        assert_ne!(
            message["result"]["isError"],
            serde_json::Value::Bool(true),
            "{tool}: {message}"
        );
        text_of(&message)
    }
}

/// The text blocks of a tool result message, joined by newlines.
pub(crate) fn text_of(message: &serde_json::Value) -> String {
    message["result"]["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|c| c["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn request(body: &serde_json::Value, session: Option<&str>) -> Request<Body> {
    let mut req = Request::post("/mcp")
        .header(header::HOST, HOST)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(session) = session {
        req = req.header("mcp-session-id", session);
    }
    req.body(Body::from(body.to_string())).unwrap()
}

/// Reads the SSE stream until the JSON-RPC response with `id` arrives.
async fn read_response(body: Body, id: u64) -> serde_json::Value {
    let mut stream = body.into_data_stream();
    let mut buf = String::new();
    let found = tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(chunk) = stream.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
            for line in buf.lines() {
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(data.trim())
                    && value["id"] == serde_json::json!(id)
                {
                    return Some(value);
                }
            }
        }
        None
    })
    .await;
    found
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("no response for id {id}: {buf}"))
}

/// The host policy of a loopback daemon.
pub(crate) fn policy() -> HostPolicy {
    HostPolicy::from_config(&DaemonConfig {
        host: "127.0.0.1".to_string(),
        ..Default::default()
    })
    .unwrap()
}

/// The daemon's HTTP app over `memory`, composed as the daemon composes it
/// (the host with the memory module).
pub(crate) async fn router(
    memory: Arc<Memory>,
    policy: &HostPolicy,
    auth_token: Option<String>,
    ct: &CancellationToken,
) -> Router {
    let modules = crate::compose::Modules::memory(memory.clone()).expect("the memory tools build");
    let host = crate::compose::host(&modules)
        .await
        .expect("the memory module starts");
    build_router(Arc::new(host), memory, policy, auth_token, ct)
}
