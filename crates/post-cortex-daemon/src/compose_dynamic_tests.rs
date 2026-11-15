// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A dynamic module's whole way (docs/modules/dynamic.md) with the real memory,
//! board, launcher and OS sandbox: installed, waiting for a person, approved and
//! called; a new version waiting while the old one answers; removed.
//!
//! ```sh
//! PCX_BINDINGS_PYTHON=bindings/python/.venv/bin/python \
//! cargo test -p post-cortex-daemon --lib a_dynamic_module -- --ignored
//! ```

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_host::{Host, Origin};
use pcx_module_api::Scope;
use post_cortex_proto::board::boards_client::BoardsClient;
use post_cortex_proto::board::{DecisionRequest, GetTaskRequest};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::compose::{Builtin, BuiltinSpec, Modules, host, keep_dynamic};
use crate::compose_dynamic_stack_tests::{Stack, launcher_up, memory_up};

const PROTO: &str = r#"syntax = "proto3";
package hello.v1;
service Hello {
  // Says hello.
  rpc Say(Said) returns (Said);
}
message Said {
  string text = 1;
}
"#;

fn module_py(answer: &str) -> String {
    format!(
        r#"from pcx_module import Effect, Module
from hello.v1 import hello_pb2 as pb

module = Module(id="hello", version="1", proto="hello/v1/hello.proto", description="Says hello.")

@module.tool("/hello.v1.Hello/Say", name="hello_say", effect=Effect.READ)
async def say(ctx, request: pb.Said) -> pb.Said:
    return pb.Said(text="{answer} " + request.text)

if __name__ == "__main__":
    module.run()
"#
    )
}

/// The module in TS, with a package from its lock file.
const MODULE_TS: &str = r#"import { Effect, Module } from "@post-cortex/module";
import isOdd from "npm:is-odd@3.0.1";
import { file_hello_v1_hello, Hello } from "./gen/hello/v1/hello_pb.ts";

const module = new Module({
  id: "hello-ts",
  version: "1",
  proto: file_hello_v1_hello,
  description: "Says hello.",
});

module.tool(Hello.method.say, { name: "hello_ts_say", effect: Effect.READ }, async (_ctx, request) => ({
  text: `Hello ${request.text}, odd: ${isOdd(3)}`,
}));

await module.run();
"#;

const TEST_PY: &str = r"import unittest

class Passes(unittest.TestCase):
    def test_it(self):
        self.assertEqual(1, 1)
";

/// A call by `author`.
pub fn scope(author: &str) -> Scope {
    Scope::own(
        author,
        "",
        Duration::from_secs(120),
        CancellationToken::new(),
    )
}

/// The module `hello`, answering with `answer`.
pub fn install(answer: &str) -> Value {
    json!({
        "id": "hello",
        "version": answer,
        "language": "LANGUAGE_PYTHON",
        "files": [
            { "path": "module.py", "text": module_py(answer) },
            { "path": "proto/hello/v1/hello.proto", "text": PROTO },
            { "path": "tests/test_it.py", "text": TEST_PY },
        ],
        "grants": {},
    })
}

/// The lock file of [`MODULE_TS`]'s package, as an author makes it.
fn ts_lock(deno: &Path, run: &Path, repository: &Path) -> String {
    let dir = run.join("author");
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("m.ts");
    std::fs::write(&source, "import \"npm:is-odd@3.0.1\";\n").unwrap();
    let out = Command::new(deno)
        .current_dir(&dir)
        .args(["cache", "--lock=deno.lock", "m.ts"])
        .env("DENO_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(repository.is_dir());
    std::fs::read_to_string(dir.join("deno.lock")).unwrap()
}

/// The decision `asked` names approved by a person.
pub async fn approve(host: &Host, workspace: &str, asked: &Value) {
    let (task, decision) = (
        asked["task"].as_str().unwrap().to_string(),
        asked["decision"].as_str().unwrap().to_string(),
    );
    let mut boards = BoardsClient::new(host.channel(Origin::Client));
    let mut read = tonic::Request::new(GetTaskRequest {
        workspace_id: workspace.to_string(),
        task,
    });
    scope("ana").to_metadata(read.metadata_mut()).unwrap();
    let thread = boards.get_task(read).await.unwrap().into_inner();
    let version = thread
        .decisions
        .iter()
        .find(|d| d.id == decision)
        .expect("its decision is on its task")
        .version;
    let mut request = tonic::Request::new(DecisionRequest {
        workspace_id: workspace.to_string(),
        decision,
        version,
        ..Default::default()
    });
    scope("ana").to_metadata(request.metadata_mut()).unwrap();
    boards.approve_decision(request).await.unwrap();
}

/// What `hello_say` answers once `says` it, within a while.
async fn answers(host: &Host, says: &str) {
    let started = Instant::now();
    loop {
        let answer = host
            .call_for_agent(&scope("tester"), "hello_say", json!({ "text": "Ada" }))
            .await;
        if let Ok(output) = &answer
            && output
                .data
                .as_ref()
                .is_some_and(|d| d["text"] == json!(format!("{says} Ada")))
        {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "never answered {says}: {answer:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the built Python binding: set PCX_BINDINGS_PYTHON and pass --ignored"]
async fn a_dynamic_module_waits_for_a_person_then_runs_and_goes() {
    let Stack {
        launcher,
        stop,
        task: launcher_task,
        run,
        deno,
        repository,
    } = launcher_up();
    let (_data, memory, workspace) = memory_up().await;
    let board_spec = BuiltinSpec {
        id: Builtin::Board,
        grants: Vec::new(),
        filesystem: Vec::new(),
        config: None,
    };

    // A board first: the module's settings name it.
    let modules = Modules::memory(Arc::clone(&memory))
        .unwrap()
        .configured(&[], &[], std::slice::from_ref(&board_spec))
        .await
        .unwrap();
    let first = host(&modules).await.unwrap();
    let board = first
        .call_for_agent(
            &scope("tester"),
            "board_create",
            json!({ "workspace_id": workspace, "name": "Approvals", "columns": ["todo", "done"] }),
        )
        .await
        .unwrap()
        .data
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    first.stop().await;

    let modules = Modules::memory(Arc::clone(&memory))
        .unwrap()
        .configured(&[], &[], &[board_spec])
        .await
        .unwrap()
        .with_dynamic(
            launcher.clone(),
            json!({ "workspace_id": workspace, "board": board }),
        );
    let running = Arc::new(host(&modules).await.unwrap());
    let keeper = tokio::spawn(keep_dynamic(
        Arc::clone(&running),
        launcher,
        "org-1".to_string(),
        stop.clone(),
    ));

    let install_as = |answer: &'static str| {
        let running = Arc::clone(&running);
        async move {
            running
                .call_for_agent(&scope("tester"), "dynamic_install", install(answer))
                .await
                .unwrap()
                .data
                .unwrap()
        }
    };

    // Installed: checked, its tool named, and nothing runs until a person says.
    let installed = install_as("Hello").await;
    assert_eq!(installed["module"]["check"]["tools"], json!(["hello_say"]));
    assert_eq!(installed["module"]["check"]["tests_passed"], json!(true));
    assert!(installed["module"].get("approved").is_none(), "{installed}");
    assert!(running.tools().iter().all(|t| t.name != "hello_say"));

    // Approved: it loads in the running host and answers.
    approve(&running, &workspace, &installed["module"]["asked"]).await;
    answers(&running, "Hello").await;
    // Its RPC's comment is what its tool says, though generated code keeps none.
    let tool = running
        .tools()
        .into_iter()
        .find(|t| t.name == "hello_say")
        .unwrap();
    assert_eq!(tool.description, "Says hello.");

    // New code waits, and the approved version goes on.
    let changed = install_as("Bye").await;
    answers(&running, "Hello").await;
    let shown = running
        .call_for_agent(&scope("tester"), "dynamic_get", json!({ "id": "hello" }))
        .await
        .unwrap()
        .data
        .unwrap();
    assert!(
        shown["difference"].as_str().unwrap().contains("Bye"),
        "{shown}"
    );
    approve(&running, &workspace, &changed["module"]["asked"]).await;
    answers(&running, "Bye").await;

    // Removed: its tool goes.
    running
        .call_for_agent(&scope("tester"), "dynamic_remove", json!({ "id": "hello" }))
        .await
        .unwrap();
    let started = Instant::now();
    while running.tools().iter().any(|t| t.name == "hello_say") {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "its tool stayed"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // The same way in TS, a package from its lock file.
    let lock = ts_lock(&deno, run.path(), &repository);
    let installed = running
        .call_for_agent(
            &scope("tester"),
            "dynamic_install",
            json!({
                "id": "hello-ts",
                "version": "1",
                "language": "LANGUAGE_TS",
                "files": [
                    { "path": "module.ts", "text": MODULE_TS },
                    { "path": "proto/hello/v1/hello.proto", "text": PROTO },
                    { "path": "deno.lock", "text": lock },
                ],
                "grants": {},
            }),
        )
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(
        installed["module"]["check"]["tools"],
        json!(["hello_ts_say"])
    );
    approve(&running, &workspace, &installed["module"]["asked"]).await;
    let started = Instant::now();
    loop {
        let answer = running
            .call_for_agent(&scope("tester"), "hello_ts_say", json!({ "text": "Ada" }))
            .await;
        if let Ok(output) = &answer {
            assert_eq!(
                output.data.as_ref().unwrap()["text"],
                json!("Hello Ada, odd: true")
            );
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "{answer:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    stop.cancel();
    keeper.await.unwrap();
    running.stop().await;
    launcher_task.await.unwrap().unwrap();
}
