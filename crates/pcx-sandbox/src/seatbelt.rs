// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! macOS: a process runs under `sandbox-exec` with a Seatbelt profile built
//! from the policy. Everything is denied but starting and running programs,
//! reading the system paths a program needs, reading the roots, writing the
//! roots that allow it (Unix sockets there included: tmux's server), the
//! terminals a program opens (`/dev/ptmx` and its `/dev/ttys*`), and the
//! network with its grant; last, a denied name under a root is neither read
//! nor written (a later rule wins in a profile).

use std::path::{Path, PathBuf};

use pcx_module_api::sandbox::Policy;

use crate::deny::{Deny, REGEX_SPECIAL};
use crate::exec::CommandLine;
use crate::launcher::Limits;

/// The launcher of a Seatbelt-confined process.
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The shell that sets a module's limit of open files, then becomes it.
const SH: &str = "/bin/sh";
const LIMITED: &str = r#"ulimit -n "$1"; shift; exec "$@""#;

/// The system paths a program reads to start and run (its libraries, the
/// dynamic linker's cache, the developer tools behind `/usr/bin/git`,
/// Homebrew).
const SYSTEM_READ: [&str; 14] = [
    "/usr",
    "/bin",
    "/sbin",
    "/System",
    "/Library/Apple",
    "/Library/Developer/CommandLineTools",
    "/Library/Preferences",
    "/private/etc",
    "/private/var/db/dyld",
    "/private/var/db/timezone",
    "/private/var/select",
    "/opt/homebrew",
    "/usr/local",
    "/dev",
];

/// Where `xcode-select` keeps the active developer directory, which the
/// `/usr/bin` tools (`git`, `make`) run from: Xcode or the Command Line
/// Tools, wherever they are installed; it and its parent are read.
const XCODE_SELECT_LINK: &str = "/private/var/db/xcode_select_link";

/// The services a program asks of the system to start (user and group
/// lookup, logging); nothing that reaches files or the network.
const SERVICES: [&str; 4] = [
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.logger",
    "com.apple.system.notification_center",
    "com.apple.trustd.agent",
];

/// A path as a Seatbelt string literal.
fn literal(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The real path of a root (Seatbelt matches resolved paths: `/tmp` is
/// `/private/tmp`), or the path itself when it cannot be resolved.
fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `text` with every character a regular expression gives a meaning
/// escaped.
fn regex_literal(text: &str) -> String {
    text.chars()
        .map(|c| {
            if REGEX_SPECIAL.contains(c) || c == '*' || c == '?' {
                format!("\\{c}")
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// The Seatbelt profile of `policy`, `deny` its denied names; `forks`:
/// whether what it runs may start processes.
pub fn profile(policy: &Policy, deny: &Deny, forks: bool) -> String {
    let mut p = String::from(
        "(version 1)\n\
         (deny default)\n\
         (allow process-exec)\n\
         (allow signal (target same-sandbox))\n\
         (allow process-info* (target same-sandbox))\n\
         (allow sysctl-read)\n\
         (allow file-read-metadata)\n\
         (allow ipc-posix-shm-read-data)\n",
    );
    if forks {
        p.push_str("(allow process-fork)\n");
    }
    p.push_str("(allow mach-lookup");
    for service in SERVICES {
        p.push_str(&format!(" (global-name \"{service}\")"));
    }
    p.push_str(")\n(allow file-read* (literal \"/\")");
    for path in SYSTEM_READ {
        p.push_str(&format!(" (subpath {})", literal(Path::new(path))));
    }
    // The developer directory with what it loads beside it (Xcode's
    // `Contents/SharedFrameworks`); none selected: the `/usr/bin` tools
    // are not there.
    if let Ok(developer) = std::fs::canonicalize(XCODE_SELECT_LINK) {
        let tools = developer.parent().unwrap_or(&developer);
        p.push_str(&format!(" (subpath {})", literal(tools)));
    }
    for root in &policy.roots {
        p.push_str(&format!(" (subpath {})", literal(&real(&root.path))));
    }
    p.push_str(")\n(allow file-write* (literal \"/dev/null\") (literal \"/dev/tty\")");
    let writable: Vec<String> = policy
        .roots
        .iter()
        .filter(|r| r.write)
        .map(|r| format!("(subpath {})", literal(&real(&r.path))))
        .collect();
    for root in &writable {
        p.push_str(&format!(" {root}"));
    }
    p.push_str(")\n");
    p.push_str(
        "(allow pseudo-tty)\n\
         (allow file-read* file-write* file-ioctl (literal \"/dev/ptmx\") (regex #\"^/dev/ttys[0-9]+$\"))\n",
    );
    // One clause per root: a unix-socket filter takes one path.
    for root in &writable {
        p.push_str(&format!(
            "(allow network-bind network-outbound (local unix-socket {root}) (remote unix-socket {root}))\n"
        ));
    }
    if policy.network {
        p.push_str("(allow network*)\n");
    }
    for root in &policy.roots {
        let under = regex_literal(&real(&root.path).to_string_lossy());
        for name in deny.component_regexes() {
            p.push_str(&format!(
                "(deny file-read* file-write* (regex #\"^{under}(/.*)?/{name}(/.*)?$\"))\n"
            ));
        }
    }
    p
}

/// `program args` under `sandbox-exec` with the profile of `policy` and
/// `deny`, and under `limits` when they are a module's.
pub fn command(
    policy: &Policy,
    deny: &Deny,
    limits: Option<&Limits>,
    program: &str,
    args: &[String],
) -> CommandLine {
    let forks = limits.is_none_or(|l| !l.no_processes);
    let mut wrapped = vec!["-p".to_string(), profile(policy, deny, forks)];
    if let Some(limits) = limits {
        wrapped.extend([
            SH.to_string(),
            "-c".to_string(),
            LIMITED.to_string(),
            SH.to_string(),
            limits.open_files.to_string(),
        ]);
    }
    wrapped.push(program.to_string());
    wrapped.extend(args.iter().cloned());
    CommandLine {
        program: PathBuf::from(SANDBOX_EXEC),
        args: wrapped,
    }
}
