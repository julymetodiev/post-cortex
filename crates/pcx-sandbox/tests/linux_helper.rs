// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Linux: processes under Landlock and seccomp through the helper. This
//! binary is the helper too: started as `<self> sandbox-run ...` it
//! confines itself and runs the program, as the `pcx` binary does.
//! Elsewhere it has nothing to test.

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;
    use std::time::Duration;

    use pcx_module_api::Module;
    use pcx_module_api::sandbox::{ExecExit, ExecOutput, ExecSpec, Policy, Root, Sandbox};
    use pcx_sandbox::{Launcher, SandboxModule};
    use tokio_util::sync::CancellationToken;

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

    fn arg(path: &Path) -> String {
        path.to_str().unwrap().to_string()
    }

    pub async fn confined(network: bool) {
        let (root, read_only, outside, scratch) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        std::fs::write(root.path().join("in.txt"), b"inside").unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
        let launcher = Launcher::Landlock {
            helper: std::env::current_exe().unwrap(),
        };
        let sandbox = SandboxModule::new(launcher, scratch.path())
            .sandbox_provider()
            .unwrap()
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
                    network,
                    spawn: true,
                    deny: Vec::new(),
                    local_only: false,
                },
            )
            .unwrap();
        let s = sandbox.as_ref();
        let inside = run(s, spec("/bin/cat", &["in.txt"])).await;
        assert_eq!(inside.stdout, b"inside", "{inside:?}");
        let secret = run(
            s,
            spec("/bin/cat", &[&arg(&outside.path().join("secret.txt"))]),
        )
        .await;
        assert_ne!(secret.exit, ExecExit::Code(0), "{secret:?}");
        assert!(secret.stdout.is_empty());
        let made = root.path().join("made.txt");
        assert_eq!(
            run(s, spec("/usr/bin/touch", &[&arg(&made)])).await.exit,
            ExecExit::Code(0)
        );
        for refused in [
            outside.path().join("made.txt"),
            read_only.path().join("made.txt"),
        ] {
            let out = run(s, spec("/usr/bin/touch", &[&arg(&refused)])).await;
            assert_ne!(out.exit, ExecExit::Code(0), "{out:?}");
            assert!(!refused.exists());
        }
        // A socket of the internet family, refused without the grant.
        let probe = run(
            s,
            spec(
                "/usr/bin/python3",
                &["-c", "import socket; socket.socket()"],
            ),
        )
        .await;
        assert_eq!(probe.exit == ExecExit::Code(0), network, "{probe:?}");
        // With the network, names resolve: the resolver's configuration is
        // read where its link leads (`/run/systemd/resolve` here).
        let resolver = run(s, spec("/bin/cat", &["/etc/resolv.conf"])).await;
        assert_eq!(resolver.exit == ExecExit::Code(0), network, "{resolver:?}");
        println!("linux helper (network {network}): ok");
    }

    /// A runner's credentials: hidden from a process by name and through a
    /// link, a denied directory read-only and empty, the rest as before.
    pub async fn denied_names_are_hidden() {
        let (root, scratch) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(root.path().join(".env"), b"KEY=secret").unwrap();
        std::fs::create_dir_all(root.path().join("app/.ssh")).unwrap();
        std::fs::write(root.path().join("app/.ssh/id_ed25519"), b"key").unwrap();
        std::fs::write(root.path().join("main.rs"), b"fn main() {}").unwrap();
        std::os::unix::fs::symlink(".env", root.path().join("innocent")).unwrap();
        let launcher = Launcher::Landlock {
            helper: std::env::current_exe().unwrap(),
        };
        let sandbox = SandboxModule::new(launcher, scratch.path())
            .sandbox_provider()
            .unwrap()
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
        let s = sandbox.as_ref();
        for hidden in [".env", "innocent", "app/.ssh/id_ed25519"] {
            let out = run(s, spec("/bin/cat", &[hidden])).await;
            assert!(out.stdout.is_empty(), "{hidden}: {out:?}");
        }
        let planted = run(
            s,
            spec("/bin/sh", &["-c", "echo x > app/.ssh/authorized_keys"]),
        )
        .await;
        assert_ne!(planted.exit, ExecExit::Code(0), "{planted:?}");
        assert!(!root.path().join("app/.ssh/authorized_keys").exists());
        let main = run(s, spec("/bin/cat", &["main.rs"])).await;
        assert_eq!(main.stdout, b"fn main() {}", "{main:?}");
        // The files themselves are untouched outside the process.
        assert_eq!(
            std::fs::read(root.path().join(".env")).unwrap(),
            b"KEY=secret"
        );
        println!("linux helper (denied names): ok");
    }

    /// /tmp is the module's scratch directory: a program that writes there
    /// by name works, and the file is the module's alone.
    pub async fn tmp_is_the_scratch_directory() {
        let (root, scratch) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let launcher = Launcher::Landlock {
            helper: std::env::current_exe().unwrap(),
        };
        let sandbox = SandboxModule::new(launcher, scratch.path())
            .sandbox_provider()
            .unwrap()
            .bind(
                "tester",
                Policy {
                    roots: vec![Root {
                        path: root.path().to_path_buf(),
                        write: true,
                    }],
                    network: false,
                    spawn: true,
                    deny: Vec::new(),
                    local_only: false,
                },
            )
            .unwrap();
        let wrote = run(
            sandbox.as_ref(),
            spec(
                "/bin/sh",
                &["-c", "echo kept > /tmp/probe && cat \"$TMPDIR/probe\""],
            ),
        )
        .await;
        assert_eq!(wrote.exit, ExecExit::Code(0), "{wrote:?}");
        assert_eq!(wrote.stdout, b"kept\n", "{wrote:?}");
        assert!(
            !std::path::Path::new("/tmp/probe").exists(),
            "the system's /tmp is untouched"
        );
        println!("linux helper (/tmp is the scratch directory): ok");
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        // As `pcx sandbox-run --policy <json> -- program args` parses it.
        let args: Vec<String> = std::env::args().collect();
        if let [_, run, flag, policy, dashes, command @ ..] = args.as_slice()
            && run == "sandbox-run"
            && flag == "--policy"
            && dashes == "--"
        {
            match pcx_sandbox::run_helper(policy, command) {
                Ok(never) => match never {},
                Err(e) => {
                    eprintln!("sandbox-run: {e}");
                    std::process::exit(126);
                }
            }
        }
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            linux::confined(false).await;
            linux::confined(true).await;
            linux::denied_names_are_hidden().await;
            linux::tmp_is_the_scratch_directory().await;
        });
        println!("test result: ok. 4 passed (linux helper)");
    }
    #[cfg(not(target_os = "linux"))]
    println!("test result: ok. 0 passed (linux helper: not Linux)");
}
