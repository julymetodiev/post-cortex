// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! FreeBSD: processes in a jail through the helper. This binary is the
//! helper too: started as `<self> sandbox-run ...` it builds the jail and
//! runs the program, as the `pcx` binary does. It runs as root where nested
//! jails are allowed (docs/host.md, sandbox and tools), with the user a
//! jailed process runs as in `PCX_JAIL_USER`. Elsewhere it has nothing to
//! test.

#[cfg(target_os = "freebsd")]
mod freebsd {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use pcx_module_api::Module;
    use pcx_module_api::sandbox::{ExecExit, ExecOutput, ExecSpec, Policy, Root, Sandbox};
    use pcx_sandbox::{Launcher, SandboxModule};
    use tokio_util::sync::CancellationToken;

    /// Where the helpers build their jails.
    const BASE: &str = "/var/run/pcx-sandbox";

    fn spec(program: &str, args: &[&str]) -> ExecSpec {
        ExecSpec {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: None,
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            stdin: None,
            timeout: Duration::from_secs(20),
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

    /// A root the jailed user owns.
    fn owned(dir: &Path, user: &str) {
        let user = nix::unistd::User::from_name(user).unwrap().unwrap();
        std::os::unix::fs::chown(dir, Some(user.uid.as_raw()), Some(user.gid.as_raw())).unwrap();
    }

    fn sandbox(user: &str, roots: Vec<Root>, network: bool, deny: Vec<String>) -> Arc<dyn Sandbox> {
        let scratch = tempfile::tempdir().unwrap().keep();
        let launcher = Launcher::Jail {
            helper: std::env::current_exe().unwrap(),
            user: Some(user.to_string()),
        };
        SandboxModule::new(launcher, &scratch)
            .sandbox_provider()
            .unwrap()
            .bind(
                "tester",
                Policy {
                    roots,
                    network,
                    spawn: true,
                    deny,
                    local_only: false,
                },
            )
            .unwrap()
    }

    /// Nothing of a jail is left: no record, no mount, no root.
    fn nothing_left() {
        let left: Vec<String> = std::fs::read_dir(BASE)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "empty" && name != "empty.d")
            .collect();
        assert!(left.is_empty(), "left in {BASE}: {left:?}");
        let mounts = std::process::Command::new("/sbin/mount")
            .arg("-p")
            .output()
            .unwrap();
        let mounts = String::from_utf8_lossy(&mounts.stdout);
        assert!(!mounts.contains(BASE), "mounts left: {mounts}");
    }

    pub async fn confined(user: &str, network: bool) {
        let (root, read_only, outside) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        owned(root.path(), user);
        std::fs::write(root.path().join("in.txt"), b"inside").unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
        let sandbox = sandbox(
            user,
            vec![
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
            Vec::new(),
        );
        let s = sandbox.as_ref();
        let inside = run(s, spec("/bin/cat", &["in.txt"])).await;
        assert_eq!(inside.stdout, b"inside", "{inside:?}");
        let who = run(s, spec("/usr/bin/id", &["-un"])).await;
        assert_eq!(String::from_utf8_lossy(&who.stdout).trim(), user, "{who:?}");
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
        assert!(made.exists());
        for refused in [
            outside.path().join("made.txt"),
            read_only.path().join("made.txt"),
        ] {
            let out = run(s, spec("/usr/bin/touch", &[&arg(&refused)])).await;
            assert_ne!(out.exit, ExecExit::Code(0), "{out:?}");
            assert!(!refused.exists());
        }
        // Its scratch directory (TMPDIR, HOME) is the jail user's to write.
        let scratch = run(s, spec("/bin/sh", &["-c", "echo x > \"$TMPDIR/made\""])).await;
        assert_eq!(scratch.exit, ExecExit::Code(0), "{scratch:?}");
        let devices = run(s, spec("/bin/sh", &["-c", "echo x > /dev/null"])).await;
        assert_eq!(devices.exit, ExecExit::Code(0), "{devices:?}");
        let exit = run(s, spec("/bin/sh", &["-c", "exit 7"])).await;
        assert_eq!(exit.exit, ExecExit::Code(7), "{exit:?}");
        // No address of the internet without the grant.
        let probe = run(
            s,
            spec("/usr/bin/fetch", &["-qo", "/dev/null", "http://1.1.1.1/"]),
        )
        .await;
        assert_eq!(probe.exit == ExecExit::Code(0), network, "{probe:?}");
        nothing_left();
        println!("freebsd helper (network {network}): ok");
    }

    /// A runner's credentials: hidden from a process by name and through a
    /// link, a denied directory read-only and empty, the rest as before.
    pub async fn denied_names_are_hidden(user: &str) {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".env"), b"KEY=secret").unwrap();
        std::fs::create_dir_all(root.path().join("app/.ssh")).unwrap();
        std::fs::write(root.path().join("app/.ssh/id_ed25519"), b"key").unwrap();
        std::fs::write(root.path().join("main.rs"), b"fn main() {}").unwrap();
        std::os::unix::fs::symlink(".env", root.path().join("innocent")).unwrap();
        owned(root.path(), user);
        owned(&root.path().join("app"), user);
        owned(&root.path().join("app/.ssh"), user);
        let sandbox = sandbox(
            user,
            vec![Root {
                path: root.path().to_path_buf(),
                write: true,
            }],
            false,
            vec![".env".into(), ".ssh".into()],
        );
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
        assert_eq!(
            std::fs::read(root.path().join(".env")).unwrap(),
            b"KEY=secret"
        );
        nothing_left();
        println!("freebsd helper (denied names): ok");
    }

    /// A root under a root that allows as much needs no mount of its own
    /// (nullfs refuses one over its own tree), and a jail that could not
    /// be built leaves nothing, as no next helper may come for its name.
    pub async fn roots_inside_roots_and_a_failed_build(user: &str) {
        let root = tempfile::tempdir().unwrap();
        let inner = root.path().join("inner");
        std::fs::create_dir(&inner).unwrap();
        owned(root.path(), user);
        owned(&inner, user);
        let writes = |path: &Path| Root {
            path: path.to_path_buf(),
            write: true,
        };
        let nested = sandbox(
            user,
            vec![writes(root.path()), writes(&inner)],
            false,
            Vec::new(),
        );
        let out = run(
            nested.as_ref(),
            spec("/bin/sh", &["-c", "echo x > inner/made"]),
        )
        .await;
        assert_eq!(out.exit, ExecExit::Code(0), "{out:?}");
        assert!(inner.join("made").exists());
        nothing_left();

        // The first root is the working directory; the second cannot mount.
        let elsewhere = tempfile::tempdir().unwrap();
        let broken = sandbox(
            user,
            vec![
                writes(root.path()),
                writes(&elsewhere.path().join("not-there")),
            ],
            false,
            Vec::new(),
        );
        let out = run(broken.as_ref(), spec("/usr/bin/true", &[])).await;
        assert_ne!(out.exit, ExecExit::Code(0), "{out:?}");
        nothing_left();
        println!("freebsd helper (roots in roots, a failed build): ok");
    }

    /// A helper killed past its deadline leaves its jail's root; the next
    /// helper undoes it before it builds its own.
    pub async fn a_killed_helper_is_swept(user: &str) {
        let root = tempfile::tempdir().unwrap();
        owned(root.path(), user);
        let sandbox = sandbox(
            user,
            vec![Root {
                path: root.path().to_path_buf(),
                write: true,
            }],
            false,
            Vec::new(),
        );
        let s = sandbox.as_ref();
        let mut sleeping = spec("/bin/sleep", &["30"]);
        sleeping.timeout = Duration::from_secs(2);
        assert_eq!(run(s, sleeping).await.exit, ExecExit::TimedOut);
        let next = run(s, spec("/usr/bin/true", &[])).await;
        assert_eq!(next.exit, ExecExit::Code(0), "{next:?}");
        nothing_left();
        assert!(!jails().contains("pcx-"), "jails left: {}", jails());
        println!("freebsd helper (a killed helper swept): ok");
    }

    /// /tmp in the jail is the module's scratch directory: a program that
    /// writes there by name works, and the file is the module's alone.
    pub async fn tmp_is_the_scratch_directory(user: &str) {
        let root = tempfile::tempdir().unwrap();
        owned(root.path(), user);
        let sandbox = sandbox(
            user,
            vec![Root {
                path: root.path().to_path_buf(),
                write: true,
            }],
            false,
            Vec::new(),
        );
        let s = sandbox.as_ref();
        let wrote = run(
            s,
            spec(
                "/bin/sh",
                &["-c", "echo kept > /tmp/probe && cat \"$TMPDIR/probe\""],
            ),
        )
        .await;
        assert_eq!(wrote.exit, ExecExit::Code(0), "{wrote:?}");
        assert_eq!(wrote.stdout, b"kept\n", "{wrote:?}");
        nothing_left();
        println!("freebsd helper (/tmp is the scratch directory): ok");
    }

    /// The jails named `pcx-...` there are now.
    fn jails() -> String {
        let listed = std::process::Command::new("/usr/sbin/jls")
            .arg("name")
            .output()
            .unwrap();
        String::from_utf8_lossy(&listed.stdout).into_owned()
    }

    /// A process a program leaves lives on in the module's jail, where the
    /// module's next program finds it, as on Linux and macOS; once none
    /// lives there, the jail goes.
    pub async fn a_process_left_lives_on_in_the_modules_jail(user: &str) {
        let root = tempfile::tempdir().unwrap();
        owned(root.path(), user);
        let sandbox = sandbox(
            user,
            vec![Root {
                path: root.path().to_path_buf(),
                write: true,
            }],
            false,
            Vec::new(),
        );
        let s = sandbox.as_ref();
        let left = run(
            s,
            // Off the command's output (-f): a process that writes it is
            // the command's, and the command ends when its output does.
            spec("/bin/sh", &["-c", "/usr/sbin/daemon -f /bin/sleep 60"]),
        )
        .await;
        assert_eq!(left.exit, ExecExit::Code(0), "{left:?}");
        assert!(jails().contains("pcx-"), "the jail stays: {}", jails());
        let found = run(s, spec("/bin/pgrep", &["-x", "sleep"])).await;
        assert_eq!(found.exit, ExecExit::Code(0), "{found:?}");
        let killed = run(s, spec("/bin/pkill", &["-x", "sleep"])).await;
        assert_eq!(killed.exit, ExecExit::Code(0), "{killed:?}");
        // pkill's own run ends with the sleep gone or going: the next one
        // ends with none.
        let after = run(s, spec("/usr/bin/true", &[])).await;
        assert_eq!(after.exit, ExecExit::Code(0), "{after:?}");
        nothing_left();
        assert!(!jails().contains("pcx-"), "jails left: {}", jails());
        println!("freebsd helper (a process left lives on): ok");
    }

    /// A tmux session started by one program is read by the next, as the
    /// shell module's tmux tools do (the jail's user has nologin: the
    /// default shell is named).
    pub async fn a_tmux_session_outlives_its_start(user: &str) {
        let root = tempfile::tempdir().unwrap();
        owned(root.path(), user);
        let sandbox = sandbox(
            user,
            vec![Root {
                path: root.path().to_path_buf(),
                write: true,
            }],
            false,
            Vec::new(),
        );
        let s = sandbox.as_ref();
        let tmux = |args: &[&str]| {
            let mut all = vec!["-L", "pcx"];
            all.extend_from_slice(args);
            let mut spec = spec("tmux", &all);
            spec.env = vec![("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into())];
            spec
        };
        let started = run(
            s,
            tmux(&[
                "start-server",
                ";",
                "set-option",
                "-g",
                "default-shell",
                "/bin/sh",
                ";",
                "new-session",
                "-d",
                "-s",
                "probe",
                "--",
                "/bin/sh",
                "-c",
                "echo ok; sleep 30",
            ]),
        )
        .await;
        assert_eq!(started.exit, ExecExit::Code(0), "{started:?}");
        let there = run(s, tmux(&["has-session", "-t", "probe"])).await;
        assert_eq!(there.exit, ExecExit::Code(0), "{there:?}");
        let pane = run(s, tmux(&["capture-pane", "-p", "-t", "probe"])).await;
        assert!(
            String::from_utf8_lossy(&pane.stdout).contains("ok"),
            "{pane:?}"
        );
        let stopped = run(s, tmux(&["kill-server"])).await;
        assert_eq!(stopped.exit, ExecExit::Code(0), "{stopped:?}");
        let after = run(s, spec("/usr/bin/true", &[])).await;
        assert_eq!(after.exit, ExecExit::Code(0), "{after:?}");
        nothing_left();
        println!("freebsd helper (a tmux session outlives its start): ok");
    }
}

fn main() {
    #[cfg(target_os = "freebsd")]
    {
        // As `pcx sandbox-run --policy <json> -- program args` parses it.
        let args: Vec<String> = std::env::args().collect();
        if let [_, run, flag, policy, dashes, command @ ..] = args.as_slice()
            && run == "sandbox-run"
            && flag == "--policy"
            && dashes == "--"
        {
            match pcx_sandbox::run_helper(policy, command) {
                Ok(code) => std::process::exit(code),
                Err(e) => {
                    eprintln!("sandbox-run: {e}");
                    std::process::exit(126);
                }
            }
        }
        let user = match std::env::var("PCX_JAIL_USER") {
            Ok(user) if nix::unistd::geteuid().is_root() => user,
            _ => {
                println!(
                    "test result: ok. 0 passed (freebsd helper: needs root, nested jails \
                     and PCX_JAIL_USER)"
                );
                return;
            }
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            freebsd::confined(&user, false).await;
            freebsd::confined(&user, true).await;
            freebsd::denied_names_are_hidden(&user).await;
            freebsd::a_killed_helper_is_swept(&user).await;
            freebsd::a_process_left_lives_on_in_the_modules_jail(&user).await;
            freebsd::a_tmux_session_outlives_its_start(&user).await;
            freebsd::tmp_is_the_scratch_directory(&user).await;
            freebsd::roots_inside_roots_and_a_failed_build(&user).await;
        });
        println!("test result: ok. 8 passed (freebsd helper)");
    }
    #[cfg(not(target_os = "freebsd"))]
    println!("test result: ok. 0 passed (freebsd helper: not FreeBSD)");
}
