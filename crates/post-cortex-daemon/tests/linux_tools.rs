// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Linux: the shell and git tools under Landlock and seccomp through the
//! `pcx` binary's `sandbox-run`, as the daemon runs them: bash and its
//! devices, tmux's server and its terminals, git and its hooks. Elsewhere
//! there is nothing to test (macOS runs them under Seatbelt in the tool
//! crates' tests).

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_host::Host;
use pcx_module_api::sandbox::{Policy, Root};
use pcx_module_api::{Module, Scope, ToolOutput};
use pcx_sandbox::{Launcher, SandboxModule};
use pcx_tools_git::GitModule;
use pcx_tools_shell::ShellModule;
use serde_json::{Value, json};
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
        deadline: Instant::now() + Duration::from_secs(60),
        cancel: CancellationToken::new(),
    }
}

async fn call(host: &Host, tool: &str, input: Value) -> ToolOutput {
    host.call_for_agent(&scope(), tool, input).await.unwrap()
}

#[tokio::test]
async fn shell_and_git_run_confined_through_the_pcx_helper() {
    let (repo, outside, scratch) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let init = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(init.success());
    let policy = Policy {
        roots: vec![Root {
            path: repo.path().to_path_buf(),
            write: true,
        }],
        network: false,
        spawn: true,
        deny: Vec::new(),
        local_only: false,
    };
    let launcher = Launcher::Landlock {
        helper: PathBuf::from(env!("CARGO_BIN_EXE_pcx")),
    };
    let modules: Vec<Arc<dyn Module>> = vec![
        Arc::new(ShellModule::new()),
        Arc::new(GitModule::new()),
        Arc::new(SandboxModule::new(launcher, scratch.path())),
    ];
    let configs = HashMap::from([(
        GitModule::ID.to_string(),
        json!({ "author_name": "Agent", "author_email": "agent@pcx.is" }),
    )]);
    let policies = HashMap::from([
        (ShellModule::ID.to_string(), policy.clone()),
        (GitModule::ID.to_string(), policy),
    ]);
    let host = Host::start(modules, &configs, &policies).await.unwrap();

    let devices = call(
        &host,
        "bash",
        json!({"command": "echo gone > /dev/null && echo kept"}),
    )
    .await;
    assert_eq!(devices.data.unwrap()["output"], json!("kept"));
    let escaped = outside.path().join("x");
    let out = call(
        &host,
        "bash",
        json!({"command": format!("touch {}", escaped.display())}),
    )
    .await;
    assert_ne!(out.data.unwrap()["exit"], json!({"code": 0}));
    assert!(!escaped.exists());

    let started = call(&host, "tmux_start", json!({"command": "echo ready; cat"})).await;
    let session = started.data.unwrap()["session"]
        .as_str()
        .unwrap()
        .to_string();
    let mut read = String::new();
    for _ in 0..50 {
        read = call(&host, "tmux_read", json!({"session": session}))
            .await
            .data
            .unwrap()["pane"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if read.contains("ready") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(read.contains("ready"), "{read}");
    call(&host, "tmux_kill", json!({"session": session})).await;

    std::fs::write(repo.path().join("a.txt"), "one\n").unwrap();
    let hook = repo.path().join(".git/hooks/pre-commit");
    std::fs::write(
        &hook,
        format!("#!/bin/sh\ntouch {}\nexit 0\n", escaped.display()),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let committed = call(&host, "git_commit", json!({"message": "feat: confined"})).await;
    assert_eq!(
        committed.data.unwrap()["conventional"]["kind"],
        json!("feat")
    );
    assert!(!escaped.exists());
    host.stop().await;
}
