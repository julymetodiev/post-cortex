// Copyright (c) 2025, 2026 Julius ML
// MIT License

use super::*;
use tokio_util::sync::CancellationToken;

fn request(id: u64, method: &str) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {}}).to_string()
}

fn initialize(id: u64) -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                   "clientInfo": {"name": "proxy-test", "version": "0"}},
    })
    .to_string()
}

fn only(messages: Vec<String>) -> serde_json::Value {
    assert_eq!(messages.len(), 1, "{messages:?}");
    serde_json::from_str(&messages[0]).unwrap()
}

/// A minimal Streamable HTTP MCP server on `port` until the returned
/// token is cancelled, as the bridge sees a daemon: `initialize` opens a
/// session named after `generation`, a request in a session it does not
/// know is a 404, a notification is accepted with nothing to answer.
async fn daemon(
    port: u16,
    generation: &'static str,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    async fn mcp(
        axum::extract::State(generation): axum::extract::State<&'static str>,
        headers: axum::http::HeaderMap,
        body: String,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        let message: serde_json::Value = serde_json::from_str(&body).unwrap();
        let Some(id) = message.get("id").cloned() else {
            return axum::http::StatusCode::ACCEPTED.into_response();
        };
        let session = format!("session-{generation}");
        let result = if message["method"] == "initialize" {
            serde_json::json!({"protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {"name": "fake", "version": "0"}})
        } else {
            let known = headers.get("mcp-session-id").and_then(|v| v.to_str().ok())
                == Some(session.as_str());
            if !known {
                return axum::http::StatusCode::NOT_FOUND.into_response();
            }
            serde_json::json!({"tools": []})
        };
        let answer = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result});
        (
            [
                ("mcp-session-id", session),
                ("content-type", "application/json".to_string()),
            ],
            answer.to_string(),
        )
            .into_response()
    }
    let ct = CancellationToken::new();
    let router = axum::Router::new()
        .route("/mcp", axum::routing::post(mcp))
        .with_state(generation);
    // A restart binds the address the last server left in TIME_WAIT.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_reuseaddr(true).unwrap();
    socket
        .bind(std::net::SocketAddr::from(([127, 0, 0, 1], port)))
        .unwrap();
    let listener = socket.listen(1024).unwrap();
    let stop = ct.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { stop.cancelled().await })
            .await
            .unwrap();
    });
    (ct, task)
}

/// An HTTP failure is answered with a JSON-RPC error for the request's
/// id (the client would otherwise wait forever); a notification gets
/// nothing.
#[tokio::test]
async fn a_request_the_daemon_cannot_answer_gets_an_error() {
    let port = portpicker::pick_unused_port().unwrap();
    let mut bridge = Bridge::new(
        format!("http://127.0.0.1:{port}/mcp"),
        reqwest::Client::new(),
    );

    let answer = only(bridge.handle(&request(7, "tools/list")).await);
    assert_eq!(answer["id"], 7);
    assert_eq!(answer["error"]["code"], PROXY_ERROR_CODE);

    let notification = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    assert!(bridge.handle(notification).await.is_empty());
}

/// After a daemon restart the old session id is unknown (404): the
/// bridge initialises again and the request is answered.
#[tokio::test]
async fn a_restarted_daemon_gets_a_new_session() {
    let port = portpicker::pick_unused_port().unwrap();
    let mut bridge = Bridge::new(
        format!("http://127.0.0.1:{port}/mcp"),
        reqwest::Client::new(),
    );

    let (ct, task) = daemon(port, "first").await;
    let init = only(bridge.handle(&initialize(0)).await);
    assert!(init.get("result").is_some(), "{init}");
    let notification = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    assert!(bridge.handle(notification).await.is_empty());
    let listed = only(bridge.handle(&request(1, "tools/list")).await);
    assert!(listed["result"]["tools"].is_array(), "{listed}");
    let first_session = bridge.session_id.clone().unwrap();

    // Restart: a new daemon on the same address knows no sessions.
    ct.cancel();
    task.await.unwrap();
    let (ct, _task) = daemon(port, "second").await;

    let listed = only(bridge.handle(&request(2, "tools/list")).await);
    assert_eq!(listed["id"], 2);
    assert!(listed["result"]["tools"].is_array(), "{listed}");
    assert_ne!(bridge.session_id.as_deref(), Some(first_session.as_str()));
    ct.cancel();
}

#[test]
fn server_sent_events_are_parsed_by_field() {
    let body =
        "id: 0\r\ndata: {\"a\":1}\r\n\r\n: comment\nevent: message\ndata: {\"b\":\ndata: 2}\n\n";
    assert_eq!(
        messages_of("text/event-stream", body),
        ["{\"a\":1}", "{\"b\":\n2}"]
    );
    assert_eq!(
        messages_of("application/json", " {\"c\":3}\n"),
        ["{\"c\":3}"]
    );
    assert!(messages_of("application/json", "").is_empty());
}
