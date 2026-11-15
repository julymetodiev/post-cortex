// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The shell tools through a host with the sandbox module (Seatbelt): every
//! process under the module's policy, tmux included.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_host::Host;
use pcx_module_api::sandbox::{Policy, Root};
use pcx_module_api::{Module, ModuleErrorKind, Scope, ToolOutput};
use pcx_sandbox::{Launcher, SandboxModule};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::ShellModule;

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
        deadline: Instant::now() + Duration::from_secs(30),
        cancel: CancellationToken::new(),
    }
}

struct Fixture {
    host: Host,
    root: tempfile::TempDir,
    outside: tempfile::TempDir,
    scratch: tempfile::TempDir,
}

/// A test that fails before the host stops still ends the module's tmux
/// server: it outlives the test's scratch directory otherwise.
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(e) = module_tmux(self, &["kill-server"]).output() {
            eprintln!("end the module's tmux server: {e}");
        }
    }
}

async fn fixture() -> Fixture {
    configured(json!({})).await.unwrap()
}

/// The fixture with the module configured by `config`.
async fn configured(config: Value) -> Result<Fixture, pcx_host::HostError> {
    let (root, outside, scratch) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let policy = Policy {
        roots: vec![Root {
            path: root.path().to_path_buf(),
            write: true,
        }],
        network: false,
        spawn: true,
        deny: Vec::new(),
        local_only: false,
    };
    let modules: Vec<Arc<dyn Module>> = vec![
        Arc::new(ShellModule::new()),
        Arc::new(SandboxModule::new(
            Launcher::platform(None).unwrap(),
            scratch.path(),
        )),
    ];
    let policies = HashMap::from([(ShellModule::ID.to_string(), policy)]);
    let configs = HashMap::from([(ShellModule::ID.to_string(), config)]);
    let host = Host::start(modules, &configs, &policies).await?;
    Ok(Fixture {
        host,
        root,
        outside,
        scratch,
    })
}

async fn call(f: &Fixture, tool: &str, input: Value) -> ToolOutput {
    f.host.call_for_agent(&scope(), tool, input).await.unwrap()
}

#[tokio::test]
async fn bash_answers_the_output_and_how_the_command_ended() {
    let f = fixture().await;
    let ok = call(
        &f,
        "bash",
        json!({"command": "echo out; echo err >&2; pwd"}),
    )
    .await;
    let root = std::fs::canonicalize(f.root.path()).unwrap();
    let ok = ok.data.unwrap();
    assert_eq!(ok["output"], json!(format!("out\n{}\nerr", root.display())));
    assert_eq!(ok["exit"], json!({"code": 0}));

    let piped = call(&f, "bash", json!({"command": "false | tail -1"})).await;
    assert_eq!(piped.data.unwrap()["exit"], json!({"code": 1}));

    let failed = call(&f, "bash", json!({"command": "exit 3"})).await;
    assert_eq!(failed.data.unwrap()["exit"], json!({"code": 3}));

    let slow = call(&f, "bash", json!({"command": "sleep 5", "timeout": 1})).await;
    assert_eq!(slow.data.unwrap()["exit"], json!({"timed_out": true}));

    let refused = f
        .host
        .call_for_agent(&scope(), "bash", json!({"command": "true", "timeout": 601}))
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);
}

#[tokio::test]
async fn a_caller_that_leaves_takes_its_command_with_it() {
    let f = fixture().await;
    let left = f.root.path().join("left");
    let command = format!("sleep 2; touch {}", left.display());
    let call = call(&f, "bash", json!({ "command": command }));
    assert!(
        tokio::time::timeout(Duration::from_millis(500), call)
            .await
            .is_err(),
        "the command ended at once"
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!left.exists(), "the command ran on after its caller left");
}

#[tokio::test]
async fn bash_writes_only_its_roots_and_sees_only_its_environment() {
    let f = fixture().await;
    let outside = f.outside.path().join("x");
    let escaped = call(
        &f,
        "bash",
        json!({"command": format!("touch {}", outside.display())}),
    )
    .await;
    assert_ne!(escaped.data.unwrap()["exit"], json!({"code": 0}));
    assert!(!outside.exists());

    call(&f, "bash", json!({"command": "echo made > made.txt"})).await;
    assert!(f.root.path().join("made.txt").exists());

    let env = call(
        &f,
        "bash",
        json!({"command": "echo \"$HOME|${USER:-none}\""}),
    )
    .await
    .data
    .unwrap();
    let output = env["output"].as_str().unwrap().trim().to_string();
    let scratch = std::fs::canonicalize(f.scratch.path()).unwrap();
    let home = output.split('|').next().unwrap().to_string();
    assert!(
        std::fs::canonicalize(&home).unwrap().starts_with(&scratch),
        "{output}"
    );
    assert!(output.ends_with("|none"), "{output}");
}

#[tokio::test]
async fn tmux_sessions_run_in_the_sandbox_and_end_with_the_module() {
    let f = fixture().await;
    let started = call(&f, "tmux_start", json!({"command": "echo ready; cat"})).await;
    let session = started.data.unwrap()["session"]
        .as_str()
        .unwrap()
        .to_string();

    let mut read = pane(&f, &session).await;
    for _ in 0..50 {
        if read["pane"].as_str().unwrap().contains("ready") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        read = pane(&f, &session).await;
    }
    assert!(read["pane"].as_str().unwrap().contains("ready"), "{read}");
    assert_eq!(read["exited"], json!(false));

    call(
        &f,
        "tmux_send",
        json!({"session": session, "text": "echoed"}),
    )
    .await;
    let mut echoed = String::new();
    for _ in 0..50 {
        echoed = pane(&f, &session).await["pane"]
            .as_str()
            .unwrap()
            .to_string();
        if echoed.matches("echoed").count() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(echoed.matches("echoed").count() >= 2, "{echoed}");

    let listed = call(&f, "tmux_list", json!({})).await;
    assert_eq!(
        listed.data.unwrap()["sessions"][0]["session"],
        json!(session)
    );

    call(&f, "tmux_kill", json!({"session": session})).await;
    let gone = f
        .host
        .call_for_agent(&scope(), "tmux_read", json!({"session": session}))
        .await
        .unwrap_err();
    assert_eq!(gone.kind, ModuleErrorKind::NotFound);

    call(&f, "tmux_start", json!({"command": "sleep 60"})).await;
    f.host.stop().await;
    let after = fixture_list(&f).await;
    assert!(after.is_empty(), "{after:?}");
}

/// What `tmux_read` answers for `session` (an RPC's answer is its data).
async fn pane(f: &Fixture, session: &str) -> Value {
    call(f, "tmux_read", json!({"session": session}))
        .await
        .data
        .unwrap()
}

/// tmux with `args` on the module's server, outside the host.
fn module_tmux(f: &Fixture, args: &[&str]) -> std::process::Command {
    let mut tmux = std::process::Command::new("tmux");
    tmux.args(["-L", "pcx"])
        .args(args)
        .env_clear()
        .env("TMUX_TMPDIR", f.scratch.path().join(ShellModule::ID))
        .env("PATH", "/usr/bin:/bin:/opt/homebrew/bin");
    tmux
}

/// The sessions of the module's tmux server, asked outside the host: none
/// once the module stopped.
async fn fixture_list(f: &Fixture) -> Vec<String> {
    let out = module_tmux(f, &["list-sessions", "-F", "#{session_name}"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(String::from)
        .collect()
}

/// A toolchain's variables reach the commands; the module's and the
/// sandbox's own are not the configuration's.
#[tokio::test]
async fn the_configured_variables_reach_the_commands() {
    let f = configured(json!({ "env": [{ "name": "CARGO_HOME", "value": "/opt/cargo" }] }))
        .await
        .unwrap();
    let ran = call(&f, "bash", json!({"command": "echo $CARGO_HOME"})).await;
    assert_eq!(ran.data.unwrap()["output"], json!("/opt/cargo"));
    drop(f);
    for own in ["PATH", "HOME", "GIT_AUTHOR_NAME", "GIT_COMMITTER_EMAIL"] {
        let refused = configured(json!({ "env": [{ "name": own, "value": "/x" }] })).await;
        assert!(refused.is_err(), "{own} is refused");
    }
}

/// An agent's command commits in the agent's name, whatever `-c user.name`
/// it passes; a person's names no one.
#[tokio::test]
async fn an_agents_commands_name_the_agent_as_git_author() {
    let f = fixture().await;
    let ident = json!({"command": "printf %s\\|%s \"$GIT_AUTHOR_NAME\" \"$GIT_AUTHOR_EMAIL\""});
    let dev = Scope {
        author: Some("workflow:board-count".into()),
        ..scope()
    };
    let ran = f
        .host
        .call_for_agent(&dev, "bash", ident.clone())
        .await
        .unwrap();
    assert_eq!(
        ran.data.unwrap()["output"],
        json!("workflow:board-count|workflow-board-count@agents.pcx.is")
    );
    let person = Scope {
        author: Some("ana".into()),
        role: Some(pcx_module_api::PersonRole::Owner),
        ..scope()
    };
    let ran = as_person(&f, &person, "bash", ident).await;
    assert_eq!(ran.data.unwrap()["output"], json!("|"));
}

/// An agent's real `git commit` through bash, in a repository and a sandbox
/// with no git identity, is made, with the agent as its author and its
/// committer (the prod runner, 2026-11-08: with the author's variables only,
/// git refused the commit, "unable to auto-detect email address").
#[tokio::test]
async fn an_agents_commit_through_bash_is_made_in_its_name() {
    let f = fixture().await;
    let dev = Scope {
        author: Some("dev".into()),
        ..scope()
    };
    let commit = json!({"command": "git init -q r && cd r && echo one > a.txt && git add a.txt \
        && git commit -q -m 'fix: by dev' && git log -1 --format='%an <%ae>|%cn <%ce>'"});
    let ran = f
        .host
        .call_for_agent(&dev, "bash", commit)
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(ran["exit"]["code"], json!(0), "{ran}");
    assert_eq!(
        ran["output"],
        json!("dev <dev@agents.pcx.is>|dev <dev@agents.pcx.is>")
    );
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
