// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! macOS: processes under Seatbelt, what they may read, write and reach,
//! their timeout and their cancel. On Linux the helper is the `pcx` binary,
//! so the cases run in `tests/linux_helper.rs`, whose binary is its own
//! helper.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_module_api::sandbox::{
    ExecExit, ExecOutput, ExecSpec, Policy, Root, Sandbox, SandboxPath, SandboxProvider,
};
use pcx_module_api::{Module, ModuleErrorKind};
use tokio_util::sync::CancellationToken;

use crate::{Launcher, SandboxModule};

fn provider(scratch: &Path) -> Arc<dyn SandboxProvider> {
    SandboxModule::new(Launcher::platform(None).unwrap(), scratch)
        .sandbox_provider()
        .unwrap()
}

fn spec(program: &str, args: &[&str]) -> ExecSpec {
    ExecSpec {
        program: program.to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd: None,
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        stdin: None,
        timeout: Duration::from_secs(10),
        output_limit: 64 * 1024,
    }
}

async fn run(sandbox: &dyn Sandbox, spec: ExecSpec) -> ExecOutput {
    sandbox
        .exec(spec, CancellationToken::new())
        .await
        .unwrap()
        .collect(64 * 1024)
        .await
        .unwrap()
}

/// A writable root, a read-only root and a directory outside both.
struct Fixture {
    sandbox: Arc<dyn Sandbox>,
    root: tempfile::TempDir,
    read_only: tempfile::TempDir,
    outside: tempfile::TempDir,
    scratch: tempfile::TempDir,
}

fn fixture(spawn: bool) -> Fixture {
    let (root, read_only, outside, scratch) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    std::fs::write(root.path().join("in.txt"), b"inside").unwrap();
    std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
    let sandbox = provider(scratch.path())
        .bind(
            "tester",
            Policy {
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
                spawn,
                deny: Vec::new(),
                local_only: false,
            },
        )
        .unwrap();
    Fixture {
        sandbox,
        root,
        read_only,
        outside,
        scratch,
    }
}

fn arg(path: &Path) -> String {
    path.to_str().unwrap().to_string()
}

#[tokio::test]
async fn a_process_reads_and_writes_only_its_roots() {
    let f = fixture(true);
    let sandbox = f.sandbox.as_ref();
    let echo = run(sandbox, spec("/bin/echo", &["hello"])).await;
    assert_eq!(
        (echo.stdout.as_slice(), echo.exit),
        (&b"hello\n"[..], ExecExit::Code(0))
    );

    let inside = run(sandbox, spec("/bin/cat", &["in.txt"])).await;
    assert_eq!(inside.stdout, b"inside", "{inside:?}");
    let secret = arg(&f.outside.path().join("secret.txt"));
    let outside = run(sandbox, spec("/bin/cat", &[&secret])).await;
    assert_ne!(outside.exit, ExecExit::Code(0), "{outside:?}");
    assert!(outside.stdout.is_empty());
    let why = String::from_utf8_lossy(&outside.stderr);
    assert!(
        why.contains("Operation not permitted") || why.contains("Permission denied"),
        "{why}"
    );

    let made = arg(&f.root.path().join("made.txt"));
    assert_eq!(
        run(sandbox, spec("/usr/bin/touch", &[&made])).await.exit,
        ExecExit::Code(0)
    );
    assert!(f.root.path().join("made.txt").exists());
    for refused in [
        f.outside.path().join("made.txt"),
        f.read_only.path().join("made.txt"),
    ] {
        let out = run(sandbox, spec("/usr/bin/touch", &[&arg(&refused)])).await;
        assert_ne!(out.exit, ExecExit::Code(0), "{out:?}");
        assert!(!refused.exists(), "{} was written", refused.display());
    }
}

#[tokio::test]
async fn a_process_runs_in_its_root_with_its_input() {
    let f = fixture(true);
    let mut cat = spec("/bin/cat", &[]);
    cat.stdin = Some(b"piped".to_vec());
    assert_eq!(run(f.sandbox.as_ref(), cat).await.stdout, b"piped");
    std::fs::create_dir(f.root.path().join("sub")).unwrap();
    let mut pwd = spec("/bin/pwd", &["-P"]);
    pwd.cwd = Some(SandboxPath::new("sub"));
    let out = run(f.sandbox.as_ref(), pwd).await;
    let real = std::fs::canonicalize(f.root.path().join("sub")).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        real.to_str().unwrap()
    );
    // A working directory outside the roots.
    let mut away = spec("/bin/pwd", &[]);
    away.cwd = Some(SandboxPath::new(arg(f.outside.path())));
    let refused = f
        .sandbox
        .exec(away, CancellationToken::new())
        .await
        .err()
        .unwrap();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
}

#[tokio::test]
async fn a_process_is_killed_past_its_timeout_or_on_a_cancel() {
    let f = fixture(true);
    let mut slow = spec("/bin/sleep", &["30"]);
    slow.timeout = Duration::from_millis(200);
    let started = Instant::now();
    assert_eq!(run(f.sandbox.as_ref(), slow).await.exit, ExecExit::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(5));

    let cancel = CancellationToken::new();
    let run = f
        .sandbox
        .exec(spec("/bin/sleep", &["30"]), cancel.clone())
        .await
        .unwrap();
    cancel.cancel();
    let out = run.collect(10).await.unwrap();
    assert_eq!(out.exit, ExecExit::Cancelled);
}

#[tokio::test]
async fn processes_need_the_spawn_grant() {
    let f = fixture(false);
    let refused = f
        .sandbox
        .exec(spec("/bin/echo", &["x"]), CancellationToken::new())
        .await
        .err()
        .unwrap();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
}

#[tokio::test]
async fn a_process_has_its_scratch_directory_and_no_more_of_the_host() {
    let f = fixture(true);
    let mut env = spec("/usr/bin/env", &[]);
    env.env.push(("HOME".into(), "/Users/someone".into()));
    let out = run(f.sandbox.as_ref(), env).await;
    let vars = String::from_utf8_lossy(&out.stdout).into_owned();
    let scratch = f.scratch.path().join("tester");
    for name in ["HOME", "TMPDIR", "TMUX_TMPDIR"] {
        assert!(
            vars.contains(&format!("{name}={}", scratch.display())),
            "{vars}"
        );
    }
    assert!(!vars.contains("/Users/someone"), "{vars}");
    assert!(!vars.contains("USER="), "nothing of the host: {vars}");
    let temp = scratch.join("t.txt");
    let touched = run(f.sandbox.as_ref(), spec("/usr/bin/touch", &[&arg(&temp)])).await;
    assert_eq!(touched.exit, ExecExit::Code(0), "{touched:?}");
}

/// A runner's credentials: a process reads and writes none of the denied
/// names under its roots, by name or through a link, and the rest as before.
#[tokio::test]
async fn a_process_reaches_no_denied_name() {
    let (root, scratch) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(root.path().join(".env"), b"KEY=secret").unwrap();
    std::fs::create_dir(root.path().join(".ssh")).unwrap();
    std::fs::write(root.path().join(".ssh/id_ed25519"), b"key").unwrap();
    std::fs::write(root.path().join("main.rs"), b"fn main() {}").unwrap();
    std::os::unix::fs::symlink(".env", root.path().join("innocent")).unwrap();
    let sandbox = provider(scratch.path())
        .bind(
            "tester",
            Policy {
                roots: vec![Root {
                    path: root.path().to_path_buf(),
                    write: true,
                }],
                network: false,
                spawn: true,
                deny: vec![".env".into(), ".ssh".into()],
                local_only: false,
            },
        )
        .unwrap();
    let sandbox = sandbox.as_ref();
    for refused in [".env", "innocent", ".ssh/id_ed25519"] {
        let out = run(sandbox, spec("/bin/cat", &[refused])).await;
        assert_ne!(out.exit, ExecExit::Code(0), "{refused}: {out:?}");
        assert!(out.stdout.is_empty(), "{refused}: {out:?}");
    }
    let planted = run(
        sandbox,
        spec("/bin/sh", &["-c", "echo x > .ssh/authorized_keys"]),
    )
    .await;
    assert_ne!(planted.exit, ExecExit::Code(0), "{planted:?}");
    assert!(!root.path().join(".ssh/authorized_keys").exists());
    let main = run(sandbox, spec("/bin/cat", &["main.rs"])).await;
    assert_eq!(main.stdout, b"fn main() {}", "{main:?}");
}

#[tokio::test]
async fn a_write_keeps_the_mode_of_what_it_replaces() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture(false);
    let script = f.root.path().join("run.sh");
    std::fs::write(&script, b"old").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o750)).unwrap();
    f.sandbox
        .write(&SandboxPath::new("run.sh"), b"new".to_vec())
        .await
        .unwrap();
    let meta = std::fs::metadata(&script).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o750);
    assert_eq!(std::fs::read(&script).unwrap(), b"new");
    f.sandbox
        .write(&SandboxPath::new("a/b/new.txt"), b"made".to_vec())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(f.root.path().join("a/b/new.txt")).unwrap(),
        b"made"
    );
}
