// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The file tools through a host with the sandbox module: every call
//! beneath the module's roots.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_host::Host;
use pcx_module_api::sandbox::{Policy, Root};
use pcx_module_api::{Module, ModuleErrorKind, Scope, ToolOutput};
use pcx_sandbox::{Launcher, SandboxModule};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::FsModule;

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
        deadline: Instant::now() + Duration::from_secs(10),
        cancel: CancellationToken::new(),
    }
}

struct Fixture {
    host: Host,
    root: tempfile::TempDir,
    read_only: tempfile::TempDir,
    _outside: tempfile::TempDir,
    _scratch: tempfile::TempDir,
}

async fn fixture(with_sandbox: bool) -> Fixture {
    let (root, read_only, outside) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
    std::fs::write(read_only.path().join("notes.md"), b"# notes\n").unwrap();
    let policy = Policy {
        roots: vec![
            Root {
                path: root.path().to_path_buf(),
                write: true,
            },
            Root {
                path: read_only.path().to_path_buf(),
                write: false,
            },
        ],
        network: false,
        spawn: false,
        deny: Vec::new(),
        local_only: false,
    };
    let mut modules: Vec<Arc<dyn Module>> = vec![Arc::new(FsModule::new())];
    let scratch = tempfile::tempdir().unwrap();
    if with_sandbox {
        modules.push(Arc::new(SandboxModule::new(
            Launcher::platform(None).unwrap(),
            scratch.path(),
        )));
    }
    let policies = HashMap::from([(FsModule::ID.to_string(), policy)]);
    let host = Host::start(modules, &HashMap::new(), &policies)
        .await
        .unwrap();
    Fixture {
        host,
        root,
        read_only,
        _outside: outside,
        _scratch: scratch,
    }
}

async fn call(f: &Fixture, tool: &str, input: Value) -> ToolOutput {
    f.host.call_for_agent(&scope(), tool, input).await.unwrap()
}

/// What a tool answered: its RPC's response in the proto JSON mapping.
fn data(output: ToolOutput) -> Value {
    output.data.unwrap()
}

#[tokio::test]
async fn files_are_written_read_edited_listed_and_found() {
    let f = fixture(true).await;
    call(
        &f,
        "write",
        json!({"path": "src/lib.rs", "content": "pub fn a() {}\npub fn b() {}\n"}),
    )
    .await;
    let read = data(call(&f, "read", json!({"path": "src/lib.rs"})).await);
    assert_eq!(
        (&read["first_line"], &read["lines"], &read["total_lines"]),
        (
            &json!("1"),
            &json!(["pub fn a() {}", "pub fn b() {}"]),
            &json!("2")
        )
    );
    let part = data(
        call(
            &f,
            "read",
            json!({"path": "src/lib.rs", "offset": 2, "limit": 1}),
        )
        .await,
    );
    assert_eq!(
        (&part["first_line"], &part["lines"]),
        (&json!("2"), &json!(["pub fn b() {}"]))
    );

    let edited = call(
        &f,
        "edit",
        json!({"path": "src/lib.rs", "old_text": "pub fn b() {}", "new_text": "pub fn c() {}"}),
    )
    .await;
    assert_eq!(data(edited)["line"], json!("2"));
    let twice = f
        .host
        .call_for_agent(
            &scope(),
            "edit",
            json!({"path": "src/lib.rs", "old_text": "pub fn", "new_text": "fn"}),
        )
        .await;
    assert_eq!(twice.unwrap_err().kind, ModuleErrorKind::InvalidArgument);

    std::fs::write(f.root.path().join(".gitignore"), "target/\n").unwrap();
    std::fs::create_dir_all(f.root.path().join("target")).unwrap();
    std::fs::write(f.root.path().join("target/gen.rs"), "pub fn c() {}\n").unwrap();
    let listed = data(call(&f, "list_dir", json!({})).await);
    let entries: Vec<(&Value, &Value)> = listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (&e["name"], &e["kind"]))
        .collect();
    assert_eq!(
        entries[..2],
        [
            (&json!("src"), &json!("ENTRY_KIND_DIR")),
            (&json!("target"), &json!("ENTRY_KIND_DIR"))
        ]
    );
    let found = data(call(&f, "glob", json!({"pattern": "**/*.rs"})).await);
    assert_eq!(
        found["files"],
        json!(["src/lib.rs"]),
        "ignored and hidden files are left out"
    );
    let hits = data(call(&f, "grep", json!({"pattern": "fn c"})).await);
    let hit = &hits["lines"][0];
    assert_eq!(
        (&hit["file"], &hit["number"], &hit["text"]),
        (&json!("src/lib.rs"), &json!("2"), &json!("pub fn c() {}"))
    );
    let none = data(call(&f, "grep", json!({"pattern": "fn z", "literal": true})).await);
    assert_eq!(none["matches"], json!("0"));
    let other_root = f.read_only.path().to_str().unwrap().to_string();
    let notes = data(call(&f, "glob", json!({"pattern": "*.md", "root": other_root})).await);
    assert_eq!(notes["files"], json!(["notes.md"]));
}

#[tokio::test]
async fn a_search_by_an_absolute_path_answers_paths_a_read_finds() {
    let f = fixture(true).await;
    let dir = f.root.path().join("src");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.rs"), "pub fn c() {}\n").unwrap();
    let dir = dir.to_str().unwrap();
    let found = data(call(&f, "glob", json!({"pattern": "**/*", "path": dir})).await);
    let first = found["files"][0].as_str().unwrap().to_string();
    assert_eq!(first, format!("{dir}/lib.rs"), "it names its root");
    let read = data(call(&f, "read", json!({"path": first})).await);
    assert!(read["lines"][0].is_string(), "{read}");

    let relative = data(call(&f, "glob", json!({"pattern": "**/*", "path": "src"})).await);
    assert_eq!(
        relative["files"],
        json!(["src/lib.rs"]),
        "relative stays relative"
    );
}

#[tokio::test]
async fn the_tools_never_leave_the_roots() {
    let f = fixture(true).await;
    for (tool, input) in [
        ("read", json!({"path": "../secret.txt"})),
        ("read", json!({"path": "/etc/hosts"})),
        (
            "write",
            json!({"path": "/tmp/pcx-escape.txt", "content": "x"}),
        ),
        ("list_dir", json!({"path": ".."})),
    ] {
        let refused = f
            .host
            .call_for_agent(&scope(), tool, input.clone())
            .await
            .unwrap_err();
        assert_eq!(
            refused.kind,
            ModuleErrorKind::Denied,
            "{tool} {input}: {refused:?}"
        );
    }
    let ro = f.read_only.path().join("new.md");
    let refused = f
        .host
        .call_for_agent(
            &scope(),
            "write",
            json!({"path": ro.to_str().unwrap(), "content": "x"}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
    assert!(!ro.exists());
}

#[tokio::test]
async fn without_a_sandbox_every_call_is_refused() {
    let f = fixture(false).await;
    let refused = f
        .host
        .call_for_agent(&scope(), "list_dir", json!({}))
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::FailedPrecondition);
}
