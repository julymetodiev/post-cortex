// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;

/// A Hugging Face cache in a fresh temporary directory, with `model_id` at
/// revision `rev` holding `files` (the Hub's layout: `refs/main` names the
/// revision, its snapshot directory holds the files).
fn hub_cache(name: &str, model_id: &str, rev: &str, files: &[&str]) -> (PathBuf, Cache) {
    let root = std::env::temp_dir().join(format!("pcx-model2vec-{name}-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let repo = root.join(format!("models--{}", model_id.replace('/', "--")));
    fs::create_dir_all(repo.join("refs")).unwrap();
    fs::write(repo.join("refs").join("main"), rev).unwrap();
    let snapshot = repo.join("snapshots").join(rev);
    fs::create_dir_all(&snapshot).unwrap();
    for file in files {
        fs::write(snapshot.join(file), b"x").unwrap();
    }
    (root.clone(), Cache::new(root))
}

fn snapshot_dir(root: &Path, model_id: &str, rev: &str) -> PathBuf {
    root.join(format!("models--{}", model_id.replace('/', "--")))
        .join("snapshots")
        .join(rev)
}

#[test]
fn a_complete_snapshot_is_found_in_the_given_cache() {
    let model = "minishlab/potion-test";
    let (root, cache) = hub_cache("complete", model, "abc123", &MODEL_FILES);

    assert_eq!(
        cached_snapshot(&cache, model),
        Some(snapshot_dir(&root, model, "abc123"))
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_snapshot_missing_a_file_is_not_used() {
    let model = "minishlab/potion-test";
    let (root, cache) = hub_cache(
        "partial",
        model,
        "abc123",
        &["config.json", "model.safetensors"],
    );

    assert_eq!(cached_snapshot(&cache, model), None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_model_not_in_the_cache_is_not_found() {
    let (root, cache) = hub_cache("other", "minishlab/potion-other", "abc123", &MODEL_FILES);

    assert_eq!(cached_snapshot(&cache, "minishlab/potion-test"), None);
    fs::remove_dir_all(root).unwrap();
}
