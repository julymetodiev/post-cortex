// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module under the OS sandbox and its limits (docs/modules/dynamic.md),
//! by the real host, directly and through the launcher of `pcx-modules`:
//! it serves and calls through its host, and everything else is refused.

use std::process::Command;
use std::time::Duration;

use nix::sys::signal::kill;
use nix::unistd::Pid;
use pcx_host::ModuleLauncher;
use pcx_module_launcher::{
    Confined, Confinement, PrepareConfig, Preparer, Remote, Requested, Served, ServedConfig,
    Watcher, prepare, serve,
};
use pcx_sandbox::Launcher;

use super::*;

/// The memory limit of these modules, in MB, and what goes past it.
const MEMORY_MB: u32 = 256;
const PAST_IT_MB: u32 = 700;

/// The limit of open files.
const OPEN_FILES: u32 = 64;

/// The SHA-256 of numpy 2.5.4's source, which no wheel has: a lock naming only
/// it installs only from a wheelhouse.
const NUMPY_SOURCE: &str = "9a94cf751c9ad8ebaa835bcd3d40dacf8534ad086b88c38029b65123c7999d2a";

/// This platform's sandbox, the test's helper as its helper.
fn sandbox() -> Launcher {
    #[cfg(target_os = "macos")]
    {
        Launcher::Seatbelt
    }
    #[cfg(target_os = "linux")]
    {
        Launcher::Landlock {
            helper: env!("CARGO_BIN_EXE_sandbox-helper").into(),
        }
    }
    #[cfg(target_os = "freebsd")]
    {
        Launcher::Jail {
            helper: env!("CARGO_BIN_EXE_sandbox-helper").into(),
            user: std::env::var("PCX_JAIL_USER").ok(),
        }
    }
}

/// What the runtime of `command` reads: the repository, its directory and,
/// of a Python environment, the Python it was made from.
fn reads(language: Language, command: &Path) -> Vec<PathBuf> {
    let mut read = vec![root().canonicalize().unwrap()];
    match language {
        Language::Python => {
            let prefix = |code: &str| {
                let out = Command::new(command).args(["-c", code]).output().unwrap();
                PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
            };
            read.push(prefix("import sys; print(sys.prefix)"));
            read.push(prefix("import sys; print(sys.base_prefix)"));
        }
        Language::Deno | Language::Node => {
            read.push(command.canonicalize().unwrap().parent().unwrap().into());
        }
    }
    read.sort();
    read.dedup();
    read
}

/// The environment the runtimes need.
fn environment(scratch: &Path) -> Vec<(String, String)> {
    vec![
        ("PYTHONDONTWRITEBYTECODE".to_string(), "1".to_string()),
        ("DENO_NO_UPDATE_CHECK".to_string(), "1".to_string()),
        ("DENO_DIR".to_string(), scratch.display().to_string()),
    ]
}

/// A module's sandboxed behaviour, launched by `launcher`.
async fn behaves(
    language: Language,
    launcher: Arc<dyn ModuleLauncher>,
    sockets: &Path,
    outside: &Path,
) {
    let spec = language.spec("relay", "bindings/tests/fixtures/relay_module", &["helper"]);
    let module = ProcessModule::launch_by(&spec, sockets, launcher)
        .await
        .unwrap_or_else(|e| panic!("launch: {e}"));
    let host = Host::start(
        vec![Arc::new(Helper::new()), Arc::new(module)],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let probe = |what: String| {
        let channel = host.channel(Origin::Client);
        async move {
            RelayClient::new(channel)
                .probe(request(
                    Text {
                        text: what,
                        wait_ms: 0,
                    },
                    Duration::from_secs(10),
                ))
                .await
                .map(|answer| answer.into_inner().text)
        }
    };

    // It serves, and calls a module through its host from its sandbox.
    let answer = RelayClient::new(host.channel(Origin::Client))
        .upper(request(
            Text {
                text: "abc".to_string(),
                wait_ms: 0,
            },
            Duration::from_secs(10),
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(answer.text, "ABC from relay for tester");

    // Nothing else.
    for what in [
        "spawn".to_string(),
        "connect".to_string(),
        format!("read:{}", outside.join("secret").display()),
        format!("write:{}", outside.join("written").display()),
        format!(
            "write:{}",
            root().canonicalize().unwrap().join("written").display()
        ),
    ] {
        let answer = probe(what.clone()).await.unwrap();
        assert!(answer.starts_with("refused"), "{what}: {answer}");
    }
    assert!(!outside.join("written").exists());

    // Its limit of open files holds.
    let files: u32 = probe("files".to_string()).await.unwrap().parse().unwrap();
    assert!(files > 0 && files < OPEN_FILES, "{files} files");

    // Past its memory limit it is killed and says so; it comes back.
    let past = probe(format!("alloc:{PAST_IT_MB}")).await;
    assert!(past.is_err(), "{past:?}");
    let log = |host: &Host| {
        host.module_log("relay")
            .unwrap()
            .into_iter()
            .map(|l| l.text)
            .collect::<Vec<_>>()
            .join("\n")
    };
    // Asked only once it started again, as a call meanwhile counts against
    // its breaker.
    for _ in 0..150 {
        if log(&host).contains("the process started again") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let answered = probe("pid".to_string())
        .await
        .unwrap_or_else(|e| panic!("it did not come back: {e:?}\n{}", log(&host)));
    assert!(answered.parse::<u32>().is_ok(), "{answered}");
    assert!(
        log(&host).contains(&format!("memory limit of {MEMORY_MB} MB")),
        "{}",
        log(&host)
    );
    host.stop().await;
}

async fn directly(language: Language) {
    let sockets = tempfile::tempdir_in("/tmp").unwrap();
    let outside = tempfile::tempdir_in("/tmp").unwrap();
    std::fs::write(outside.path().join("secret"), "secret").unwrap();
    let scratch = sockets.path().join("scratch");
    let spec = language.spec("relay", "bindings/tests/fixtures/relay_module", &[]);
    let launcher = Confined::new(
        sandbox(),
        Watcher::start(),
        Confinement {
            read: reads(language, &spec.command),
            written: Vec::new(),
            env: environment(&scratch),
            scratch,
            cwd: root().canonicalize().unwrap(),
            network: false,
            spawn: false,
            memory_mb: MEMORY_MB,
            open_files: u64::from(OPEN_FILES),
            sockets_group: None,
        },
    );
    behaves(language, Arc::new(launcher), sockets.path(), outside.path()).await;
}

async fn through_the_launcher(language: Language) {
    let base = tempfile::tempdir_in("/tmp").unwrap();
    let outside = tempfile::tempdir_in("/tmp").unwrap();
    std::fs::write(outside.path().join("secret"), "secret").unwrap();
    let sockets = base.path().join("sockets");
    std::fs::create_dir(&sockets).unwrap();
    let scratch = base.path().join("scratch");
    let spec = language.spec("relay", "bindings/tests/fixtures/relay_module", &["helper"]);
    let read = reads(language, &spec.command);
    let repository = root().canonicalize().unwrap();
    let socket = base.path().join("launcher.sock");
    let stop = CancellationToken::new();
    let preparer = Preparer::new(
        PrepareConfig {
            store: base.path().join("prepared"),
            python: spec.command.clone(),
            read: Vec::new(),
            wheelhouse: None,
            protoc: None,
            protoc_gen_es: None,
            deno: None,
        },
        sandbox(),
    );
    let served = Served::new(
        ServedConfig {
            runtimes: vec![spec.command.clone()],
            programs: Vec::new(),
            network_programs: Vec::new(),
            always_read: read.iter().filter(|r| **r != repository).cloned().collect(),
            stores: vec![repository.clone()],
            sockets: sockets.clone(),
            scratch: scratch.clone(),
            host_group: None,
        },
        preparer,
        sandbox(),
    );
    let server = tokio::spawn({
        let (socket, stop) = (socket.clone(), stop.clone());
        async move { serve(served, &socket, stop).await }
    });
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let launcher = Remote::new(
        socket,
        Requested {
            owner: "org-1".to_string(),
            read: vec![repository.clone()],
            cwd: repository,
            env: environment(&scratch.join("org-1").join("relay"))
                .into_iter()
                .collect(),
            memory_mb: MEMORY_MB,
            open_files: OPEN_FILES,
            sha256: String::new(),
        },
    );
    let launcher: Arc<dyn ModuleLauncher> = Arc::new(launcher);
    behaves(language, Arc::clone(&launcher), &sockets, outside.path()).await;

    // A host that goes takes its module along: what it holds is dropped,
    // unstopped.
    let module = ProcessModule::launch_by(&spec, &sockets, launcher)
        .await
        .unwrap_or_else(|e| panic!("launch: {e}"));
    let host = Host::start(
        vec![Arc::new(Helper::new()), Arc::new(module)],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let answer = RelayClient::new(host.channel(Origin::Client))
        .probe(request(
            Text {
                text: "pid".to_string(),
                wait_ms: 0,
            },
            Duration::from_secs(10),
        ))
        .await
        .unwrap()
        .into_inner();
    let pid = Pid::from_raw(answer.text.parse().unwrap());
    drop(host);
    let mut gone = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if kill(pid, None).is_err() {
            gone = true;
            break;
        }
    }
    assert!(gone, "process {pid} outlived its host");

    stop.cancel();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the built bindings: scripts/test-bindings.sh"]
async fn python_confined() {
    directly(Language::Python).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the built bindings: scripts/test-bindings.sh"]
async fn deno_confined() {
    directly(Language::Deno).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the built bindings: scripts/test-bindings.sh"]
async fn python_through_the_launcher() {
    through_the_launcher(Language::Python).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the built bindings: scripts/test-bindings.sh"]
async fn deno_through_the_launcher() {
    through_the_launcher(Language::Deno).await;
}

mod prepared;
