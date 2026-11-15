// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The commit the daemon is built from, for `Health.commit` (`PCX_COMMIT`):
//! `git rev-parse` in the checkout the crate is built in; empty outside one
//! (a published crate), never a guessed value. The build reruns when the
//! checkout's HEAD moves.

use std::path::Path;
use std::process::Command;

/// The trimmed output of `git <args>` in `dir`, when git answers.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let commit = git(dir, &["rev-parse", "--short=12", "HEAD"]);
    if commit.is_some() {
        for path in [
            Some("HEAD".to_string()),
            git(dir, &["symbolic-ref", "-q", "HEAD"]),
            Some("packed-refs".to_string()),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(file) = git(dir, &["rev-parse", "--git-path", &path]) {
                println!("cargo:rerun-if-changed={}", dir.join(file).display());
            }
        }
    }
    println!("cargo:rustc-env=PCX_COMMIT={}", commit.unwrap_or_default());
}
