// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `pcx-skills` binary as a module process, as the cloud runs it: the
//! host launches it, reads its manifest (its document kind, configuration,
//! prompts and the module it uses) and stops it.

use std::path::PathBuf;

use pcx_host::{Grant, ProcessModule, ProcessSpec};
use sha2::Digest;

#[tokio::test]
async fn the_binary_serves_the_library() {
    let command = PathBuf::from(env!("CARGO_BIN_EXE_pcx-skills"));
    let sha256 = sha2::Sha256::digest(std::fs::read(&command).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let spec = ProcessSpec {
        id: "skills".to_string(),
        command,
        args: Vec::new(),
        sha256,
        config: None,
        grants: vec![Grant::WriteMemory],
        uses: vec!["post-cortex.memory".to_string()],
        filesystem: Vec::new(),
    };
    let sockets = tempfile::tempdir().unwrap();
    let manifest = ProcessModule::describe(&spec, sockets.path())
        .await
        .unwrap();
    assert_eq!(manifest.id, "skills");
    assert!(manifest.prompts, "it offers prompts");
    let kinds: Vec<&str> = manifest
        .document_kinds
        .iter()
        .map(|k| k.name.as_str())
        .collect();
    assert_eq!(kinds, ["skill"]);
    assert_eq!(manifest.config_type, "pcx.skills.v1.SkillsConfig");

    let ungranted = ProcessSpec {
        uses: Vec::new(),
        ..spec
    };
    assert!(
        ProcessModule::describe(&ungranted, sockets.path())
            .await
            .is_err()
    );
}
