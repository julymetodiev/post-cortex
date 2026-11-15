// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The sandbox's helper for the bindings' tests: what `pcx sandbox-run` is
//! on Linux and FreeBSD, without building the `pcx` binary. macOS has none.

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mode, flag, policy, separator, command @ ..] = args.as_slice() else {
        return Err("usage: sandbox-helper sandbox-run --policy <json> -- program".to_string());
    };
    if mode != "sandbox-run" || flag != "--policy" || separator != "--" {
        return Err(format!("usage: not {mode} {flag} ... {separator}"));
    }
    run(policy, command)
}

#[cfg(target_os = "linux")]
fn run(policy: &str, command: &[String]) -> Result<(), String> {
    match pcx_sandbox::run_helper(policy, command) {
        Ok(never) => match never {},
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(target_os = "freebsd")]
fn run(policy: &str, command: &[String]) -> Result<(), String> {
    match pcx_sandbox::run_helper(policy, command) {
        Ok(code) => std::process::exit(code),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn main() {}
