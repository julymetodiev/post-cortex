// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Starting the daemon: compose memory from the configuration, serve HTTP
//! (MCP and REST) and gRPC, supervise them, and shut down in order (servers
//! drained, then every session written).

use crate::daemon::DaemonConfig;
use crate::daemon::host_allowlist::HostPolicy;
use post_cortex_memory::Memory;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Start RMCP-based SSE server
pub async fn start_rmcp_daemon(config: DaemonConfig) -> Result<(), String> {
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| format!("Invalid address: {}", e))?;

    let host_policy = HostPolicy::from_config(&config)?;
    crate::daemon::proxies::warm();

    info!("Initializing Post-Cortex daemon with RMCP SSE transport");
    info!("  Host: {}", config.host);
    info!("  Port: {}", config.port);
    info!("  Data Directory: {}", config.data_directory.display());
    log_host_policy(&host_policy, &addr, config.allowed_hosts.is_empty());

    // The adapters come from the daemon's configuration (the composition
    // root, `crate::compose`); the memory configuration holds capability
    // settings only.
    let mut system_config = post_cortex_memory::SystemConfig {
        redaction: post_cortex_core::redact::RedactionConfig {
            enabled: config.redact,
            extra_patterns: config.redact_patterns.clone(),
        },
        ..Default::default()
    };
    info!(
        "  Redaction: {} ({} extra patterns)",
        if config.redact { "on" } else { "OFF" },
        config.redact_patterns.len()
    );
    let model_config = config.model_config();
    {
        system_config.enable_embeddings = true;
        system_config.auto_vectorize_on_update = true;
        system_config.cross_session_search_enabled = true;
        system_config.neighbour_radius = config.neighbour_radius;
        system_config.max_import_bytes = config.max_import_bytes;
        system_config.search_mode = config.search_mode.clone();
        info!(
            "Embeddings enabled in daemon config (model={}, dim={})",
            model_config.embedding_model,
            model_config.embedding_model.embedding_dimension(),
        );
    }

    // Every capability service over one runtime, shared by the MCP tools,
    // the REST API and gRPC.
    let models = crate::compose::Models::new(&model_config);
    let blobs = match &config.blobs {
        Some(blobs) => Some(crate::compose::open_blobs(&blobs.s3()?)?),
        None => {
            info!("  Blobs: none ([blobs] is not configured; documents name no images)");
            None
        }
    };
    let memory = Arc::new(
        crate::compose::open_memory(
            &config.storage_config()?,
            &models,
            blobs,
            system_config,
            post_cortex_memory::PipelineConfig::default(),
        )
        .await?,
    );

    info!("Memory system initialized successfully");

    // Background work that runs until the shutdown (retention); aborted at
    // shutdown if still running. The memory module's start warms the
    // embeddings up.
    let ct = CancellationToken::new();
    let mut background = JoinSet::new();

    // Clear query cache to prevent stale vector IDs from previous runs
    memory.admin.clear_query_cache();

    if config.message_retention_days == 0 {
        info!("  Message retention: forever");
    } else {
        info!(
            "  Message retention: {} days",
            config.message_retention_days
        );
        background.spawn(expire_messages(
            memory.clone(),
            config.message_retention_days,
            ct.child_token(),
        ));
    }

    // The host runs the memory module, the sandbox (the modules' scratch
    // directories under the data directory) and the configured hook
    // scripts, module processes and built-in modules, reloaded when
    // daemon.toml changes; MCP serves the host's tools.
    let modules = crate::compose::Modules::memory(memory.clone())
        .map_err(|e| format!("Failed to build the memory module: {e}"))?
        // A runner's denied names hold for every module (none without
        // `[runner]`).
        .sandboxed(
            &config.data_directory.join(SCRATCH),
            config
                .runner
                .as_ref()
                .map(|runner| runner.deny.clone())
                .unwrap_or_default(),
            config.sandbox.as_ref().map(|sandbox| sandbox.user.clone()),
        )?
        .configured(&config.hooks, &config.modules, &config.builtins)
        .await
        .map_err(|e| format!("Failed to load the configured modules: {e}"))?;
    // Dynamic modules run through a launcher served here, the one path a
    // pot's does too (docs/modules/dynamic.md).
    let dynamic = config
        .dynamic
        .as_ref()
        .map(|dynamic| dynamic_launcher(dynamic, &config, &mut background, &ct))
        .transpose()?;
    let modules = match (&dynamic, &config.dynamic) {
        (Some(launcher), Some(table)) => modules.with_dynamic(
            launcher.clone(),
            serde_json::json!({ "workspace_id": table.workspace_id, "board": table.board }),
        ),
        _ => modules,
    };
    let kinds = modules.kinds();
    let host = Arc::new(
        crate::compose::host(&modules)
            .await
            .map_err(|e| format!("Failed to start the modules: {e}"))?,
    );
    // Blobs no document names are deleted daily.
    background.spawn(sweep_blobs(
        memory.clone(),
        Arc::clone(&kinds),
        ct.child_token(),
    ));
    // Events older than the bus keeps are dropped daily.
    background.spawn(crate::compose::drop_old_events(
        memory.clone(),
        ct.child_token(),
    ));
    if let Some(launcher) = dynamic {
        background.spawn(crate::compose::keep_dynamic(
            host.clone(),
            launcher,
            "daemon".to_string(),
            ct.child_token(),
        ));
    }
    // The modules that follow memory's changes hear of them.
    background.spawn(crate::compose::feed_memory_writes(
        host.clone(),
        memory.clone(),
        ct.child_token(),
    ));
    // One reloader for the file watcher and the module settings changed
    // through pcx.module.v1.Host (written into daemon.toml).
    let reloader = Arc::new(crate::daemon::reload::Reloader::new(
        host.clone(),
        modules,
        crate::daemon::DaemonConfig::path(),
    ));
    background.spawn(crate::daemon::reload::watch(
        reloader.clone(),
        ct.child_token(),
    ));
    let settings: Arc<dyn pcx_host::Settings> = reloader;
    let browser = config.browser_access()?;
    if let crate::compose::BrowserAccess::Origins(_) = &browser {
        info!(
            "  Browser routes (gRPC-Web, /v1/import, /v1/export) for {:?}",
            config.browser_origins
        );
    }
    // Paired with an organisation: its people reach the daemon over the
    // tunnel.
    crate::daemon::connect::serve(
        &crate::daemon::DaemonConfig::dir(),
        host.clone(),
        memory.clone(),
        Arc::clone(&kinds),
        config
            .runner
            .as_ref()
            .map_or(&[][..], |runner| &runner.modules),
        &mut background,
        &ct,
    )?;
    let router = crate::compose::routes(
        host.clone(),
        memory.clone(),
        Arc::clone(&kinds),
        Some(settings.clone()),
        crate::compose::RouteConfig {
            host_policy: host_policy.clone(),
            auth_token: config.auth_token.clone(),
            browser,
        },
        &ct,
    );
    info!("MCP endpoint configured: http://{}/mcp", addr);
    match &config.auth_token {
        Some(_) => info!("  Auth: bearer token required (HTTP except /health, gRPC)"),
        None if !addr.ip().is_loopback() => {
            warn!(
                "  Auth: NONE on non-loopback bind {} — set auth_token / PC_AUTH_TOKEN",
                addr
            );
        }
        None => info!("  Auth: none (loopback bind)"),
    }

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("Failed to bind to {}: {}", addr, e))?;
    info!("TCP listener bound to {}", addr);
    // Written by the daemon that holds the address, for `pcx stop`.
    let pid_file = crate::daemon::pidfile::PidFile::write(&config.data_directory)
        .map_err(|e| format!("Cannot write the pid file: {e}"))?;

    // The servers: each runs until the shutdown; one that stops before it
    // stops the daemon (with its error), so a dead server is never left
    // behind a daemon that looks ready.
    let mut servers = JoinSet::new();
    let http_shutdown = ct.clone();
    servers.spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { http_shutdown.cancelled().await })
            .await
            .map_err(|e| format!("HTTP server: {e}"))
    });
    if config.grpc_port > 0 {
        let grpc_addr: SocketAddr = format!("{}:{}", config.host, config.grpc_port)
            .parse()
            .map_err(|e| format!("Invalid gRPC address: {}", e))?;
        servers.spawn(crate::daemon::start_grpc_server(
            memory.clone(),
            kinds,
            host.clone(),
            Some(settings.clone()),
            grpc_addr,
            config.auth_token.clone(),
            ct.child_token(),
        ));
        info!("gRPC endpoint: {}:{}", config.host, config.grpc_port);
    }

    info!("Daemon is ready to accept connections");
    info!("Press Ctrl+C to shutdown");

    let stopped_early = tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, initiating shutdown");
            None
        }
        _ = terminate_signal() => {
            info!("Received SIGTERM, initiating shutdown");
            None
        }
        Some(done) = servers.join_next() => Some(server_end(done)),
    };

    ct.cancel();
    // In-flight requests finish before the sessions are written.
    let drained = tokio::time::timeout(SHUTDOWN_DRAIN, async {
        let mut failures = Vec::new();
        while let Some(done) = servers.join_next().await {
            if let Err(e) = server_end(done) {
                failures.push(e);
            }
        }
        failures
    })
    .await;
    background.shutdown().await;
    // Stopping the modules writes the sessions (their persists run in the
    // background), or the last updates would be lost with the process.
    host.stop().await;
    drop(pid_file);
    info!("Post-Cortex daemon stopped");

    match (stopped_early, drained) {
        (Some(Err(e)), _) => Err(format!("{e}; the daemon stopped")),
        (Some(Ok(())), _) => Err("a server stopped before the shutdown".to_string()),
        (None, Ok(failures)) if !failures.is_empty() => Err(failures.join("; ")),
        (None, Ok(_)) => Ok(()),
        (None, Err(_)) => Err(format!(
            "servers did not stop within {SHUTDOWN_DRAIN:?}; sessions were written"
        )),
    }
}

/// How long the servers get to finish their requests, and the sessions to
/// be written, at shutdown.
/// The modules' scratch directories, under the data directory.
const SCRATCH: &str = "scratch";

const SHUTDOWN_DRAIN: std::time::Duration = std::time::Duration::from_secs(30);

/// The result of a server task: its own error, or its panic.
fn server_end(done: Result<Result<(), String>, tokio::task::JoinError>) -> Result<(), String> {
    match done {
        Ok(result) => result,
        Err(e) => Err(format!("server task failed: {e}")),
    }
}

/// How often expired messages are deleted.
const MESSAGE_RETENTION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Delete captured messages older than `retention_days` at startup and then
/// daily, until `shutdown` is cancelled.
async fn expire_messages(memory: Arc<Memory>, retention_days: u32, shutdown: CancellationToken) {
    let mut interval = tokio::time::interval(MESSAGE_RETENTION_INTERVAL);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = interval.tick() => {}
        }
        let cutoff = chrono::Utc::now() - chrono::Duration::days(i64::from(retention_days));
        if let Err(e) = memory.messages.expire(cutoff).await {
            error!("Message retention failed: {e}");
        }
    }
}

/// How often blobs no document names are deleted, and how long one put
/// and not yet named is kept.
const BLOB_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Delete the blobs no version of a document names, put more than a day
/// ago, at startup and then daily, until `shutdown` is cancelled.
async fn sweep_blobs(
    memory: Arc<Memory>,
    kinds: Arc<dyn post_cortex_ports::DocumentKinds>,
    shutdown: CancellationToken,
) {
    let mut interval = tokio::time::interval(BLOB_SWEEP_INTERVAL);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = interval.tick() => {}
        }
        let put_before = chrono::Utc::now() - chrono::Duration::days(1);
        match memory.blobs.sweep(put_before, kinds.as_ref()).await {
            Ok(swept) => info!(
                "Blob sweep: {} deleted, {} workspaces left as they are",
                swept.deleted, swept.skipped
            ),
            Err(e) => error!("Blob sweep failed: {e}"),
        }
    }
}

/// Logs the effective `Host` allowlist, warning when it will turn remote
/// clients away or is switched off.
fn log_host_policy(policy: &HostPolicy, addr: &SocketAddr, using_defaults: bool) {
    match policy {
        HostPolicy::Any => warn!(
            "  Allowed hosts: ANY (allowed_hosts = [\"*\"]) — Host validation is off, \
             the daemon is exposed to DNS rebinding from any web page"
        ),
        HostPolicy::Only(hosts) => {
            info!("  Allowed hosts: {}", hosts.join(", "));
            if addr.ip().is_unspecified() && using_defaults {
                warn!(
                    "  Bound to {} but allowed_hosts only lists loopback names: remote \
                     clients get 403 on /mcp and /api/* until allowed_hosts (or \
                     PC_ALLOWED_HOSTS) lists the host names they connect with, e.g. a \
                     tailnet hostname or IP",
                    addr
                );
            }
        }
    }
}

/// The launcher of `dynamic`, served until `ct` under the data directory.
///
/// # Errors
/// This platform has no sandbox, or a directory cannot be made.
fn dynamic_launcher(
    dynamic: &crate::daemon::config::DynamicConfig,
    config: &DaemonConfig,
    background: &mut tokio::task::JoinSet<()>,
    ct: &CancellationToken,
) -> Result<crate::compose::LauncherConfig, String> {
    use pcx_module_launcher::{PrepareConfig, Preparer, Runtimes, Served, ServedConfig, serve};

    let base = config.data_directory.join("dynamic");
    let (store, sockets, scratch) = (base.join("store"), base.join("s"), base.join("scratch"));
    for dir in [&store, &sockets, &scratch] {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let sandbox = pcx_sandbox::Launcher::platform(
        config.sandbox.as_ref().map(|sandbox| sandbox.user.clone()),
    )
    .map_err(|e| format!("the sandbox does not start: {e}"))?;
    let served = Served::new(
        ServedConfig {
            runtimes: vec![dynamic.python.clone(), dynamic.deno.clone()],
            programs: Vec::new(),
            network_programs: Vec::new(),
            always_read: dynamic.read.clone(),
            stores: vec![store.clone()],
            sockets: sockets.clone(),
            scratch,
            // The daemon is its launcher's only host, as its own user.
            host_group: None,
        },
        Preparer::new(
            PrepareConfig {
                store,
                python: dynamic.python.clone(),
                read: dynamic.read.clone(),
                wheelhouse: None,
                protoc: Some(dynamic.protoc.clone()),
                protoc_gen_es: Some(dynamic.protoc_gen_es.clone()),
                deno: Some((dynamic.deno.clone(), dynamic.imports.clone())),
            },
            sandbox.clone(),
        ),
        sandbox,
    );
    let socket = base.join("launcher.sock");
    let (at, stop) = (socket.clone(), ct.child_token());
    background.spawn(async move {
        if let Err(e) = serve(served, &at, stop).await {
            error!("the dynamic modules' launcher ended: {e}");
        }
    });
    Ok(crate::compose::LauncherConfig {
        socket,
        sockets,
        runtimes: Runtimes {
            python: dynamic.python.clone(),
            deno: dynamic.deno.clone(),
            imports: dynamic.imports.clone(),
        },
    })
}

/// Resolves on Ctrl-C or SIGTERM: a process with nothing else to stop it.
pub async fn terminated() {
    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            if let Err(e) = result {
                warn!("Cannot listen for Ctrl-C: {}", e);
                std::future::pending::<()>().await;
            }
        }
        () = terminate_signal() => {}
    }
}

/// Resolves on SIGTERM (what launchd, systemd and `kill` send); never on
/// platforms without it.
async fn terminate_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut term) => {
                term.recv().await;
            }
            Err(e) => {
                warn!("Cannot listen for SIGTERM: {}", e);
                std::future::pending::<()>().await;
            }
        }
    }
    #[cfg(not(unix))]
    std::future::pending::<()>().await;
}
