// Copyright (c) 2025 Julius ML
// MIT License

//! stdio-to-daemon proxy with auto-start capability
//!
//! This module provides a stdio interface that proxies MCP requests to a running daemon.
//! If no daemon is running, it automatically starts one in the background.

use crate::daemon::DaemonConfig;
use arc_swap::ArcSwap;
use std::io::{self, BufRead};
use std::process::{Command, Stdio};
use std::sync::Arc;
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
pub fn start_daemon_background(config: &DaemonConfig) -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Failed to get current executable: {}", e))?;

    info!("Starting daemon in background: {:?}", current_exe);

    let child = Command::new(&current_exe)
        .args(["start", "--daemon"])
        .env("PCX_PORT", config.port.to_string())
        .env("PCX_HOST", &config.host)
        .env("PCX_DATA_DIR", &config.data_directory)
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
pub async fn ensure_daemon_running(config: &DaemonConfig) -> Result<(), String> {
    if is_daemon_running(&config.host, config.port).await {
        info!("Daemon already running on {}:{}", config.host, config.port);
        return Ok(());
    }

    info!("Daemon not running, starting in background...");
    start_daemon_background(config)?;
    wait_for_daemon(&config.host, config.port).await
}

/// Run stdio proxy mode - bridges stdin/stdout to daemon via Streamable HTTP
pub async fn run_stdio_proxy(config: DaemonConfig) -> Result<(), String> {
    // Ensure daemon is running
    ensure_daemon_running(&config).await?;

    let mcp_url = format!("http://{}:{}/mcp", config.host, config.port);
    let client = reqwest::Client::new();

    info!("Connecting to daemon at {}", mcp_url);

    // Session ID storage (lock-free via ArcSwap)
    let session_id: Arc<ArcSwap<Option<String>>> = Arc::new(ArcSwap::from_pointee(None));

    // Stdin reader - send requests to daemon and write responses to stdout
    let stdin_handle = tokio::task::spawn_blocking({
        let client = client.clone();
        let mcp_url = mcp_url.clone();
        let session_id = session_id.clone();

        move || {
            let stdin = io::stdin();
            let reader = stdin.lock();
            let rt = tokio::runtime::Handle::current();

            for line in reader.lines() {
                match line {
                    Ok(line) if !line.trim().is_empty() => {
                        let client = client.clone();
                        let url = mcp_url.clone();
                        let session_id = session_id.clone();

                        rt.block_on(async {
                            // Build request with proper headers for Streamable HTTP
                            let mut request = client
                                .post(&url)
                                .header("Content-Type", "application/json")
                                .header("Accept", "application/json, text/event-stream");

                            // Add session ID header if we have one (lock-free load)
                            let current_sid = session_id.load();
                            if let Some(ref id) = **current_sid {
                                request = request.header("Mcp-Session-Id", id.clone());
                            }

                            match request.body(line).send().await {
                                Ok(resp) => {
                                    // Extract session ID from response header (lock-free store)
                                    if let Some(new_sid) = resp.headers().get("mcp-session-id") {
                                        if let Ok(sid_str) = new_sid.to_str() {
                                            if session_id.load().is_none() {
                                                info!("Got session ID: {}", sid_str);
                                                session_id
                                                    .store(Arc::new(Some(sid_str.to_string())));
                                            }
                                        }
                                    }

                                    if resp.status().is_success() {
                                        // Read response body and parse SSE events
                                        match resp.text().await {
                                            Ok(body) => {
                                                // Parse SSE format: "data: {...}\n\n"
                                                for event in body.split("\n\n") {
                                                    if let Some(data) = event.strip_prefix("data: ")
                                                    {
                                                        let data = data.trim();
                                                        if !data.is_empty() {
                                                            println!("{}", data);
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                error!("Failed to read response: {}", e);
                                            }
                                        }
                                    } else {
                                        error!("Request failed with status: {}", resp.status());
                                        if let Ok(body) = resp.text().await {
                                            error!("Response: {}", body);
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to send request: {}", e);
                                }
                            }
                        });
                    }
                    Ok(_) => {} // Empty line, skip
                    Err(e) => {
                        error!("Error reading stdin: {}", e);
                        break;
                    }
                }
            }
        }
    });

    // Wait for stdin to close (client disconnected)
    let _ = stdin_handle.await;

    Ok(())
}
