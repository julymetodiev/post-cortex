// Copyright (c) 2025 Julius ML
// MIT License

//! stdio-to-daemon proxy with auto-start capability
//!
//! This module provides a stdio interface that proxies MCP requests to a running daemon.
//! If no daemon is running, it automatically starts one in the background.

use crate::local::Local;
use std::io::{self, BufRead};
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::sleep;
use tracing::{error, info};

const MAX_STARTUP_WAIT_SECS: u64 = 30;
const STARTUP_CHECK_INTERVAL_MS: u64 = 100;

/// Check if daemon is running by attempting TCP connection
pub async fn is_daemon_running(host: &str, port: u16) -> bool {
    let addr = format!("{}:{}", host, port);
    TcpStream::connect(&addr).await.is_ok()
}

/// Start daemon in background process
pub fn start_daemon_background(local: &Local) -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Failed to get current executable: {}", e))?;

    info!("Starting daemon in background: {:?}", current_exe);

    let child = Command::new(&current_exe)
        .args(["start", "--daemon"])
        .env("PC_PORT", local.port.to_string())
        .env("PC_HOST", &local.host)
        .env("PC_DATA_DIR", &local.data_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn daemon: {}", e))?;

    info!("Daemon process spawned with PID: {}", child.id());
    Ok(())
}

/// Wait for daemon to become ready
pub async fn wait_for_daemon(host: &str, port: u16) -> Result<(), String> {
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(MAX_STARTUP_WAIT_SECS);

    while start.elapsed() < timeout {
        if is_daemon_running(host, port).await {
            info!("Daemon is ready on {}:{}", host, port);
            return Ok(());
        }
        sleep(Duration::from_millis(STARTUP_CHECK_INTERVAL_MS)).await;
    }

    Err(format!(
        "Daemon failed to start within {} seconds",
        MAX_STARTUP_WAIT_SECS
    ))
}

/// Ensure daemon is running, starting it if necessary
pub async fn ensure_daemon_running(local: &Local) -> Result<(), String> {
    if is_daemon_running(&local.host, local.port).await {
        info!("Daemon already running on {}:{}", local.host, local.port);
        return Ok(());
    }

    info!("Daemon not running, starting in background...");
    start_daemon_background(local)?;
    wait_for_daemon(&local.host, local.port).await
}

/// Run stdio proxy mode - bridges stdin/stdout to daemon via Streamable HTTP
pub async fn run_stdio_proxy(local: &Local) -> Result<(), String> {
    // Ensure daemon is running
    ensure_daemon_running(local).await?;

    let mcp_url = format!("{}/mcp", local.url());
    let client = local.http_client().map_err(|e| e.to_string())?;
    proxy_stdio(mcp_url, client).await
}

/// Bridge stdin/stdout to the Streamable HTTP MCP endpoint at `mcp_url`
/// (the local daemon, a self-hosted one or the hosted cloud), sending every
/// request with `client` (which carries the bearer token and author). Starts
/// nothing: [`run_stdio_proxy`] starts the local daemon first.
///
/// Every request read from stdin gets exactly one answer on stdout: the
/// daemon's, or a JSON-RPC error when the HTTP call failed. A session the
/// daemon no longer knows (it restarted) is initialised again with the
/// client's own `initialize`, then the request is resent.
pub async fn proxy_stdio(mcp_url: String, client: reqwest::Client) -> Result<(), String> {
    info!("Connecting to daemon at {}", mcp_url);
    let mut bridge = Bridge::new(mcp_url, client);

    let stdin_handle = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Handle::current();
        for line in io::stdin().lock().lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => {
                    for message in rt.block_on(bridge.handle(&line)) {
                        println!("{message}");
                    }
                }
                Ok(_) => {} // Empty line, skip
                Err(e) => {
                    error!("Error reading stdin: {}", e);
                    break;
                }
            }
        }
    });

    // Wait for stdin to close (client disconnected)
    stdin_handle
        .await
        .map_err(|e| format!("stdio proxy task failed: {e}"))
}

/// JSON-RPC code of an answer the proxy makes up because the daemon could
/// not be reached or answered with an HTTP error.
const PROXY_ERROR_CODE: i64 = -32603;

/// One client's connection to the Streamable HTTP endpoint: its MCP
/// session and what it takes to open the session again.
struct Bridge {
    url: String,
    client: reqwest::Client,
    session_id: Option<String>,
    /// The client's `initialize` request, replayed when the daemon lost the
    /// session.
    initialize: Option<String>,
    /// Whether the client sent `notifications/initialized` (replayed too).
    initialized: bool,
}

/// What one HTTP exchange produced.
enum Exchange {
    /// The JSON-RPC messages of the response.
    Messages(Vec<String>),
    /// The daemon does not know the session (404).
    UnknownSession,
    /// The exchange failed; the reason.
    Failed(String),
}

impl Bridge {
    fn new(url: String, client: reqwest::Client) -> Self {
        Self {
            url,
            client,
            session_id: None,
            initialize: None,
            initialized: false,
        }
    }

    /// Forward one line from the client; returns the lines to write back.
    /// A request (a message with an `id`) always gets one answer.
    async fn handle(&mut self, line: &str) -> Vec<String> {
        // A line that is not JSON has no id to answer; the daemon reports it.
        let parsed: Option<serde_json::Value> = match serde_json::from_str(line) {
            Ok(value) => Some(value),
            Err(e) => {
                tracing::debug!("stdio proxy: client line is not JSON: {e}");
                None
            }
        };
        let id = parsed
            .as_ref()
            .and_then(|v| v.get("id"))
            .filter(|id| !id.is_null())
            .cloned();
        let method = parsed
            .as_ref()
            .and_then(|v| v.get("method"))
            .and_then(|m| m.as_str());
        match method {
            Some("initialize") => {
                // A new handshake: a new session.
                self.initialize = Some(line.to_string());
                self.initialized = false;
                self.session_id = None;
            }
            Some("notifications/initialized") => self.initialized = true,
            _ => {}
        }

        let mut outcome = self.exchange(line).await;
        if matches!(outcome, Exchange::UnknownSession) && self.reinitialize().await {
            outcome = self.exchange(line).await;
        }
        match (outcome, id) {
            (Exchange::Messages(messages), Some(id)) if !answers(&messages, &id) => {
                let mut messages = messages;
                messages.push(error_answer(&id, "the daemon sent no response"));
                messages
            }
            (Exchange::Messages(messages), _) => messages,
            (Exchange::UnknownSession, Some(id)) => {
                vec![error_answer(
                    &id,
                    "the daemon no longer knows this MCP session",
                )]
            }
            (Exchange::Failed(reason), Some(id)) => {
                error!("{}", reason);
                vec![error_answer(&id, &reason)]
            }
            (Exchange::UnknownSession | Exchange::Failed(_), None) => Vec::new(),
        }
    }

    /// Open the session again after a daemon restart: replay the client's
    /// `initialize` (its answer was already given) and `initialized`.
    async fn reinitialize(&mut self) -> bool {
        let Some(initialize) = self.initialize.clone() else {
            return false;
        };
        info!("Daemon lost the MCP session (restart?); initialising again");
        self.session_id = None;
        if !matches!(self.exchange(&initialize).await, Exchange::Messages(_))
            || self.session_id.is_none()
        {
            return false;
        }
        if self.initialized {
            let notification =
                serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
            if !matches!(
                self.exchange(&notification.to_string()).await,
                Exchange::Messages(_)
            ) {
                return false;
            }
        }
        true
    }

    /// POST `body` to the MCP endpoint in the current session.
    async fn send(&self, body: &str) -> reqwest::Result<reqwest::Response> {
        let mut request = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        if let Some(id) = &self.session_id {
            request = request.header("Mcp-Session-Id", id.clone());
        }
        request.body(body.to_string()).send().await
    }

    async fn exchange(&mut self, body: &str) -> Exchange {
        // A pooled connection the daemon closed (it restarted) fails the
        // send; the request goes again once, on a new connection.
        let resp = match self.send(body).await {
            Ok(resp) => resp,
            Err(first) => match self.send(body).await {
                Ok(resp) => {
                    info!("Daemon connection lost ({first}); request sent again");
                    resp
                }
                Err(e) => return Exchange::Failed(format!("Failed to reach the daemon: {e}")),
            },
        };
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND && self.session_id.is_some() {
            return Exchange::UnknownSession;
        }
        if let Some(sid) = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            if self.session_id.as_deref() != Some(sid) {
                info!("Got session ID: {}", sid);
            }
            self.session_id = Some(sid.to_string());
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let text = match resp.text().await {
            Ok(text) => text,
            Err(e) => {
                return Exchange::Failed(format!("Failed to read the daemon's response: {e}"));
            }
        };
        if !status.is_success() {
            return Exchange::Failed(format!("The daemon answered {status}: {}", text.trim()));
        }
        Exchange::Messages(messages_of(&content_type, &text))
    }
}

/// The JSON-RPC messages of a response body: one JSON body, or the `data`
/// of each server-sent event (multi-line data joined; `id:`, `event:` and
/// `retry:` fields and comments ignored).
fn messages_of(content_type: &str, body: &str) -> Vec<String> {
    if !content_type.starts_with("text/event-stream") {
        let body = body.trim();
        return if body.is_empty() {
            Vec::new()
        } else {
            vec![body.to_string()]
        };
    }
    let mut messages = Vec::new();
    let mut data: Vec<&str> = Vec::new();
    let flush = |data: &mut Vec<&str>, messages: &mut Vec<String>| {
        let joined = data.join("\n");
        if !joined.trim().is_empty() {
            messages.push(joined.trim().to_string());
        }
        data.clear();
    };
    for line in body.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            flush(&mut data, &mut messages);
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    flush(&mut data, &mut messages);
    messages
}

/// Whether one of `messages` is the response to request `id`.
fn answers(messages: &[String], id: &serde_json::Value) -> bool {
    messages.iter().any(|m| {
        serde_json::from_str::<serde_json::Value>(m)
            .ok()
            .is_some_and(|v| {
                v.get("id") == Some(id) && (v.get("result").is_some() || v.get("error").is_some())
            })
    })
}

/// A JSON-RPC error answer to request `id`.
fn error_answer(id: &serde_json::Value, message: &str) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": PROXY_ERROR_CODE, "message": message},
    })
    .to_string()
}

#[cfg(test)]
mod tests;
