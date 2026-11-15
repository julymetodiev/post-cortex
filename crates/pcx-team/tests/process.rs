// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `pcx-team` binary as a module process, as the cloud runs it: the
//! host launches it, reads its manifest (its document kinds and view, the
//! grants and modules it needs) and stops it.

use std::path::PathBuf;

use pcx_host::{Grant, ProcessModule, ProcessSpec};
use sha2::Digest;

#[tokio::test]
async fn the_binary_serves_the_team() {
    let command = PathBuf::from(env!("CARGO_BIN_EXE_pcx-team"));
    let sha256 = sha2::Sha256::digest(std::fs::read(&command).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let spec = ProcessSpec {
        id: "team".to_string(),
        command,
        args: Vec::new(),
        sha256,
        config: None,
        grants: vec![Grant::WriteMemory],
        uses: [
            "post-cortex.memory",
            "board",
            "skills",
            "agent",
            "tools-fs",
            "tools-shell",
            "tools-git",
            "observe",
            "ops",
            "scheduler",
            "workflow",
            "mail",
            "http",
            "dynamic",
        ]
        .iter()
        .map(|m| (*m).to_string())
        .collect(),
        filesystem: Vec::new(),
    };
    let sockets = tempfile::tempdir().unwrap();
    let manifest = ProcessModule::describe(&spec, sockets.path())
        .await
        .unwrap();
    assert_eq!(manifest.id, "team");
    let kinds: Vec<&str> = manifest
        .document_kinds
        .iter()
        .map(|k| k.name.as_str())
        .collect();
    assert_eq!(kinds, ["team", "agent", "work", "autonomy"]);
    assert_eq!(manifest.views.len(), 1);

    let ungranted = ProcessSpec {
        uses: vec!["post-cortex.memory".to_string()],
        ..spec
    };
    assert!(
        ProcessModule::describe(&ungranted, sockets.path())
            .await
            .is_err()
    );
}
