// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The configured modules across reloads: an unchanged spec keeps its
//! running module, a changed one is loaded anew, and the host swaps them.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use pcx_host::{Grant, ScriptEvent, ScriptPolicy, ScriptSpec};
use sha2::Digest;

use crate::compose::{Modules, host};

fn script(dir: &tempfile::TempDir, name: &str, text: &str) -> ScriptSpec {
    let path = dir.path().join(name);
    std::fs::write(&path, format!("#!/bin/sh\ncat >/dev/null\necho '{text}'\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sha256 = sha2::Sha256::digest(std::fs::read(&path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    ScriptSpec {
        id: name.to_string(),
        command: path,
        args: Vec::new(),
        sha256,
        events: vec![ScriptEvent {
            event: "SessionStart".to_string(),
            policy: ScriptPolicy::FailOpen,
            deadline_ms: 2_000,
        }],
        capabilities: vec![Grant::ReadConversation],
    }
}

#[tokio::test]
async fn a_reload_keeps_unchanged_modules_and_swaps_the_rest() {
    let data = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &data,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let dir = tempfile::tempdir().unwrap();
    let a = script(&dir, "a", "from a");
    let b = script(&dir, "b", "from b");

    let first = Modules::memory(memory)
        .unwrap()
        .configured(std::slice::from_ref(&a), &[], &[])
        .await
        .unwrap();
    assert!(first.are(std::slice::from_ref(&a), &[], &[]));
    let host = host(&first).await.unwrap();

    let second = first
        .configured(&[a.clone(), b.clone()], &[], &[])
        .await
        .unwrap();
    assert!(!first.are(&[a.clone(), b.clone()], &[], &[]));
    let (old, new) = (first.all(), second.all());
    assert!(Arc::ptr_eq(&old[0], &new[0]), "memory stays");
    assert!(Arc::ptr_eq(&old[1], &new[1]), "the unchanged script stays");
    assert_eq!(new.len(), 3);

    host.reload(second.all(), &second.config(), &second.policies())
        .await
        .unwrap();
    let ids: Vec<String> = host.health().into_iter().map(|h| h.module).collect();
    assert_eq!(ids, ["post-cortex.memory", "a", "b"]);
}

/// A built-in module runs under the sandbox with the roots its entry
/// grants, and nowhere else.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_builtin_works_beneath_the_roots_its_entry_grants() {
    use std::time::{Duration, Instant};

    use pcx_host::RootGrant;
    use pcx_module_api::Scope;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use crate::compose::{Builtin, BuiltinSpec};

    let data = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &data,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let (root, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let fs = BuiltinSpec {
        id: Builtin::ToolsFs,
        grants: vec![Grant::Filesystem],
        filesystem: vec![RootGrant {
            path: root.path().to_path_buf(),
            write: true,
        }],
        config: None,
    };
    let modules = Modules::memory(memory)
        .unwrap()
        .sandboxed(&data.path().join("scratch"), Vec::new(), None)
        .unwrap()
        .configured(&[], &[], std::slice::from_ref(&fs))
        .await
        .unwrap();
    let host = host(&modules).await.unwrap();
    let scope = Scope {
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
    };
    host.call_for_agent(&scope, "write", json!({"path": "a.txt", "content": "one"}))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
        "one"
    );
    let escaped = host
        .call_for_agent(
            &scope,
            "write",
            json!({"path": outside.path().join("b.txt"), "content": "two"}),
        )
        .await
        .unwrap_err();
    assert_eq!(escaped.kind, pcx_module_api::ModuleErrorKind::Denied);
    host.stop().await;
}
