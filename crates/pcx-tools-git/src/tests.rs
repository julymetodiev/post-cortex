// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The git tools through a host with the sandbox module (Seatbelt): git and
//! its hooks under the module's policy, commits by the configured author.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_host::Host;
use pcx_module_api::sandbox::{Policy, Root};
use pcx_module_api::{Module, ModuleErrorKind, Scope, ToolOutput};
use pcx_sandbox::{Launcher, SandboxModule};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::GitModule;

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

struct Fixture {
    host: Host,
    repo: tempfile::TempDir,
    outside: tempfile::TempDir,
    _scratch: tempfile::TempDir,
}

/// A host whose `tools-git` may use a new repository (on `main`), with
/// `config`.
async fn fixture(config: Value) -> Fixture {
    fixture_with(config, false, None).await
}

/// [`fixture`], with the network grant when `network` and `extra` as a
/// second root.
async fn fixture_with(config: Value, network: bool, extra: Option<&std::path::Path>) -> Fixture {
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
    let mut roots = vec![Root {
        path: repo.path().to_path_buf(),
        write: true,
    }];
    roots.extend(extra.map(|path| Root {
        path: path.to_path_buf(),
        write: true,
    }));
    let policy = Policy {
        roots,
        network,
        spawn: true,
        deny: Vec::new(),
        local_only: false,
    };
    let modules: Vec<Arc<dyn Module>> = vec![
        Arc::new(GitModule::new()),
        Arc::new(SandboxModule::new(
            Launcher::platform(None).unwrap(),
            scratch.path(),
        )),
    ];
    let configs = HashMap::from([(GitModule::ID.to_string(), config)]);
    let policies = HashMap::from([(GitModule::ID.to_string(), policy)]);
    let host = Host::start(modules, &configs, &policies).await.unwrap();
    Fixture {
        host,
        repo,
        outside,
        _scratch: scratch,
    }
}

fn author() -> Value {
    json!({ "author_name": "Agent", "author_email": "agent@pcx.is" })
}

async fn call(f: &Fixture, tool: &str, input: Value) -> ToolOutput {
    f.host.call_for_agent(&scope(), tool, input).await.unwrap()
}

#[tokio::test]
async fn work_is_committed_on_a_branch_by_the_configured_author() {
    let f = fixture(author()).await;
    std::fs::write(f.repo.path().join("a.txt"), "one\n").unwrap();
    let status = call(&f, "git_status", json!({})).await;
    assert_eq!(
        status.data.unwrap()["changes"][0]["kind"],
        json!("CHANGE_KIND_UNTRACKED")
    );

    let first = call(&f, "git_commit", json!({"message": "feat(board): first"})).await;
    let data = first.data.unwrap();
    assert_eq!(data["conventional"]["kind"], json!("feat"));
    assert_eq!(data["conventional"]["scope"], json!("board"));
    let log = call(&f, "git_log", json!({})).await.data.unwrap();
    assert_eq!(log["commits"][0]["sha"], data["sha"]);
    assert_eq!(log["commits"][0]["author"], json!("Agent"));

    std::fs::write(f.repo.path().join("a.txt"), "two\n").unwrap();
    let diff = call(&f, "git_diff", json!({})).await.data.unwrap();
    assert!(diff["patch"].as_str().unwrap().contains("+two"), "{diff}");

    call(&f, "git_branch", json!({"name": "task-1", "from": "main"})).await;
    call(
        &f,
        "git_commit",
        json!({"message": "fix: second", "paths": ["a.txt"]}),
    )
    .await;
    let branches = call(&f, "git_branches", json!({})).await.data.unwrap();
    let current: Vec<&Value> = branches["branches"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["current"] == json!(true))
        .collect();
    assert_eq!(current[0]["name"], json!("task-1"));

    call(&f, "git_switch", json!({"name": "main"})).await;
    let work = call(&f, "git_branch_commits", json!({"branch": "task-1"}))
        .await
        .data
        .unwrap();
    assert_eq!(work["commits"].as_array().unwrap().len(), 1);
    assert_eq!(work["commits"][0]["subject"], json!("fix: second"));
}

/// Who made a commit and who committed it, as git keeps them.
fn signed(repo: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["log", "-1", "--format=%an <%ae> / %cn <%ce>"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// `tool` called with `input` in `scope` as a person calls it (the app): its
/// RPC through the host's channel of a client, the one path that keeps a
/// person's role ([`Host::call_for_agent`] drops it).
async fn as_person(f: &Fixture, scope: &Scope, tool: &str, input: Value) -> ToolOutput {
    let rpc = f
        .host
        .modules()
        .iter()
        .flat_map(|m| pcx_module_api::rpc_tool::tools(&m.manifest).unwrap())
        .find(|t| t.spec.name == tool)
        .unwrap();
    rpc.call(f.host.channel(pcx_host::Origin::Client), scope, input)
        .await
        .unwrap()
}

/// An agent's commit names the agent as its author, under a domain that
/// receives no mail; the configured identity commits it. A person's commit
/// (a call with a person's role) is the configured author's.
#[tokio::test]
async fn an_agents_commit_names_the_agent() {
    let f = fixture(author()).await;
    std::fs::write(f.repo.path().join("a.txt"), "one\n").unwrap();
    let dev = Scope {
        author: Some("dev".into()),
        ..scope()
    };
    f.host
        .call_for_agent(&dev, "git_commit", json!({"message": "fix: by dev"}))
        .await
        .unwrap();
    assert_eq!(
        signed(f.repo.path()),
        "dev <dev@agents.pcx.is> / Agent <agent@pcx.is>"
    );

    std::fs::write(f.repo.path().join("a.txt"), "two\n").unwrap();
    let person = Scope {
        author: Some("ana".into()),
        role: Some(pcx_module_api::PersonRole::Owner),
        ..scope()
    };
    as_person(&f, &person, "git_commit", json!({"message": "fix: by ana"})).await;
    assert_eq!(
        signed(f.repo.path()),
        "Agent <agent@pcx.is> / Agent <agent@pcx.is>"
    );
}

#[tokio::test]
async fn commits_need_an_author_and_revisions_are_never_options() {
    let f = fixture(Value::Null).await;
    std::fs::write(f.repo.path().join("a.txt"), "one\n").unwrap();
    let unsigned = f
        .host
        .call_for_agent(&scope(), "git_commit", json!({"message": "feat: x"}))
        .await
        .unwrap_err();
    assert_eq!(unsigned.kind, ModuleErrorKind::FailedPrecondition);
    let option = f
        .host
        .call_for_agent(&scope(), "git_log", json!({"rev": "--output=/tmp/x"}))
        .await
        .unwrap_err();
    assert_eq!(option.kind, ModuleErrorKind::InvalidArgument);
}

#[tokio::test]
async fn a_field_put_in_the_wrong_message_is_named_where_it_is() {
    let f = fixture(author()).await;
    let misplaced = f
        .host
        .call_for_agent(
            &scope(),
            "git_branch",
            json!({"name": "task/1-a", "at": {"from": "main"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(misplaced.kind, ModuleErrorKind::InvalidArgument);
    assert!(
        misplaced
            .message
            .ends_with("at.from: pcx.tools.git.v1.Repo has no field from (its fields: repo, root)"),
        "{}",
        misplaced.message
    );
}

#[tokio::test]
async fn hooks_run_inside_the_sandbox() {
    let f = fixture(author()).await;
    let escaped = f.outside.path().join("escaped");
    let hook = f.repo.path().join(".git/hooks/pre-commit");
    std::fs::write(
        &hook,
        format!("#!/bin/sh\ntouch {}\nexit 0\n", escaped.display()),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(f.repo.path().join("a.txt"), "one\n").unwrap();
    call(&f, "git_commit", json!({"message": "chore: hooked"})).await;
    assert!(!escaped.exists());
}

/// W32: a task's worktree on its branch, made once; a path that leaves the
/// repository is refused.
#[tokio::test]
async fn a_worktree_is_made_on_its_branch_once() {
    let f = fixture(author()).await;
    std::fs::write(f.repo.path().join("a.txt"), "one\n").unwrap();
    call(&f, "git_commit", json!({"message": "feat: first"})).await;
    let made = call(
        &f,
        "git_worktree_add",
        json!({"path": ".worktrees/t1", "branch": "task/t1-ship"}),
    )
    .await
    .data
    .unwrap();
    assert_eq!(made["existing"], json!(false));
    let dir = made["path"].as_str().unwrap().to_string();
    assert!(dir.ends_with("/.worktrees/t1"), "{dir}");
    assert!(std::path::Path::new(&dir).join("a.txt").exists());
    let again = call(
        &f,
        "git_worktree_add",
        json!({"path": ".worktrees/t1", "branch": "task/t1-ship"}),
    )
    .await
    .data
    .unwrap();
    assert_eq!(
        (again["existing"].clone(), again["path"].clone()),
        (json!(true), json!(dir))
    );
    for bad in ["../elsewhere", "/tmp/x", "-x"] {
        let refused = f
            .host
            .call_for_agent(
                &scope(),
                "git_worktree_add",
                json!({"path": bad, "branch": "b"}),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument, "{bad}");
    }
}

/// A task's branch goes to the configured remote as it is; `main` and an
/// unconfigured module do not push.
#[tokio::test]
async fn a_tasks_branch_is_pushed_and_nothing_else() {
    let unconfigured = fixture(author()).await;
    let refused = unconfigured
        .host
        .call_for_agent(&scope(), "git_push", json!({"branch": "task/1-a"}))
        .await
        .unwrap_err();
    assert_eq!(
        refused.kind,
        ModuleErrorKind::FailedPrecondition,
        "{refused:?}"
    );

    // The remote: a bare repository under a second root, beside the
    // agent's socket (a local remote needs no key).
    let extra = tempfile::tempdir().unwrap();
    let root = extra.path().canonicalize().unwrap();
    let remote = root.join("remote.git");
    let bare = std::process::Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&remote)
        .status()
        .unwrap();
    assert!(bare.success());
    let mut config = author();
    config["push"] = json!({
        "remote": remote.to_str().unwrap(),
        "ssh_auth_sock": root.join("agent.sock").to_str().unwrap(),
    });
    let f = fixture_with(config, true, Some(&root)).await;
    std::fs::write(f.repo.path().join("a.txt"), "one\n").unwrap();
    call(&f, "git_commit", json!({"message": "feat: first"})).await;
    call(&f, "git_branch", json!({"name": "task/1-a"})).await;
    let pushed = call(&f, "git_push", json!({"branch": "task/1-a"}))
        .await
        .data
        .unwrap();
    let head = std::process::Command::new("git")
        .args(["rev-parse", "refs/heads/task/1-a"])
        .current_dir(&remote)
        .output()
        .unwrap();
    assert_eq!(
        pushed["sha"],
        json!(String::from_utf8_lossy(&head.stdout).trim())
    );
    let main = f
        .host
        .call_for_agent(&scope(), "git_push", json!({"branch": "main"}))
        .await
        .unwrap_err();
    assert_eq!(main.kind, ModuleErrorKind::InvalidArgument, "{main:?}");
}

/// `git` with `args` in `dir`, as the test (not the module) runs it.
fn run_git(dir: &std::path::Path, args: &[&str]) {
    let done = std::process::Command::new("git")
        .args(["-c", "user.name=T", "-c", "user.email=t@pcx.is"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(done.success(), "git {args:?}");
}

/// A new task's worktree starts from the remote's copy of the checked-out
/// branch, fetched first, not from a clone gone stale (the prod runner,
/// 2026-11-08: its clone stood a week behind, so a task ran on old code).
#[tokio::test]
async fn a_new_tasks_worktree_starts_from_the_remotes_tip() {
    let extra = tempfile::tempdir().unwrap();
    let root = extra.path().canonicalize().unwrap();
    let remote = root.join("remote.git");
    run_git(&root, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
    let mut config = author();
    config["push"] = json!({
        "remote": remote.to_str().unwrap(),
        "ssh_auth_sock": root.join("agent.sock").to_str().unwrap(),
    });
    let f = fixture_with(config, true, Some(&root)).await;
    std::fs::write(f.repo.path().join("a.txt"), "one\n").unwrap();
    call(&f, "git_commit", json!({"message": "feat: first"})).await;
    run_git(
        f.repo.path(),
        &["push", "-q", remote.to_str().unwrap(), "main"],
    );
    // Someone else moves the remote's main on.
    let other = tempfile::tempdir().unwrap();
    run_git(
        other.path(),
        &["clone", "-q", remote.to_str().unwrap(), "c"],
    );
    let clone = other.path().join("c");
    std::fs::write(clone.join("a.txt"), "two\n").unwrap();
    run_git(&clone, &["commit", "-q", "-am", "feat: second"]);
    run_git(&clone, &["push", "-q", "origin", "main"]);

    let made = call(
        &f,
        "git_worktree_add",
        json!({"path": ".worktrees/t2", "branch": "task/t2-new"}),
    )
    .await
    .data
    .unwrap();
    let dir = made["path"].as_str().unwrap();
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(dir).join("a.txt")).unwrap(),
        "two\n"
    );
    let subject = |dir: &std::path::Path| {
        let out = std::process::Command::new("git")
            .args(["log", "-1", "--format=%s"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    assert_eq!(subject(std::path::Path::new(dir)), "feat: second");
    // The checkout itself is left as it was.
    assert_eq!(subject(f.repo.path()), "feat: first");
}
