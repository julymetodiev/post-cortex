// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `pcx-board` binary as a module process, as the cloud runs it: the
//! host launches it, reads its manifest (its document kinds and view, the
//! grants and modules it needs) and stops it.

use std::path::PathBuf;

use pcx_host::{Grant, ProcessModule, ProcessSpec};
use sha2::Digest;

#[tokio::test]
async fn the_binary_serves_the_board() {
    let command = PathBuf::from(env!("CARGO_BIN_EXE_pcx-board"));
    let sha256 = sha2::Sha256::digest(std::fs::read(&command).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let spec = ProcessSpec {
        id: "board".to_string(),
        command,
        args: Vec::new(),
        sha256,
        config: None,
        grants: vec![Grant::WriteMemory],
        uses: vec!["post-cortex.memory".to_string(), "tools-git".to_string()],
        filesystem: Vec::new(),
    };
    let sockets = tempfile::tempdir().unwrap();
    let manifest = ProcessModule::describe(&spec, sockets.path())
        .await
        .unwrap();
    assert_eq!(manifest.id, "board");
    let kinds: Vec<&str> = manifest
        .document_kinds
        .iter()
        .map(|k| k.name.as_str())
        .collect();
    assert_eq!(kinds, ["board", "task", "comment", "decision"]);
    assert_eq!(manifest.views.len(), 2, "the board and the approvals");

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
