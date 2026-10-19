// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! macOS + `rerank`: the static ONNX Runtime that `ort` links carries
//! CoreML code whose `@available` checks call `__isPlatformVersionAtLeast`.
//! That lives in clang's runtime (`libclang_rt.osx.a`), which rustc does not
//! link (`-nodefaultlibs`), so link it explicitly.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if !macos || std::env::var_os("CARGO_FEATURE_RERANK").is_none() {
        return;
    }
    let Ok(out) = Command::new("xcrun")
        .args(["clang", "--print-runtime-dir"])
        .output()
    else {
        println!("cargo:warning=xcrun not found; linking ONNX Runtime may fail");
        return;
    };
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    println!("cargo:rustc-link-search=native={dir}");
    println!("cargo:rustc-link-lib=static=clang_rt.osx");
}
