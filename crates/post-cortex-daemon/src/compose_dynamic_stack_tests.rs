// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the tests of dynamic modules start: the launcher, as `pcx
//! module-launcher` serves it, with the OS sandbox, and memory with blobs.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use pcx_module_launcher::{PrepareConfig, Preparer, Runtimes, Served, ServedConfig, serve};
use pcx_sandbox::Launcher;
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::compose::LauncherConfig;

/// What `python`'s own prefix and its base's are.
fn prefixes(python: &Path) -> (PathBuf, PathBuf) {
    let out = Command::new(python)
        .args([
            "-c",
            "import sys; print(sys.prefix); print(sys.base_prefix)",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let mut lines = text.lines().map(PathBuf::from);
    (lines.next().unwrap(), lines.next().unwrap())
}

/// The launcher, as `pcx module-launcher` serves it, and what it was made of.
pub struct Stack {
    /// How a host reaches it.
    pub launcher: LauncherConfig,
    /// Stops it.
    pub stop: CancellationToken,
    /// It, serving.
    pub task: tokio::task::JoinHandle<Result<(), std::io::Error>>,
    /// Its directory.
    pub run: tempfile::TempDir,
    /// Deno.
    pub deno: PathBuf,
    /// The repository.
    pub repository: PathBuf,
}

/// The launcher, serving. Call within a runtime.
pub fn launcher_up() -> Stack {
    let venv_python = std::env::var("PCX_BINDINGS_PYTHON")
        .map(PathBuf::from)
        .expect("PCX_BINDINGS_PYTHON names the runtime");
    let venv_python = std::path::absolute(venv_python).unwrap();
    let (prefix, base) = prefixes(&venv_python);
    let protoc = std::env::var_os("PROTOC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/homebrew/bin/protoc"));
    let base_python = std::fs::read_dir(base.join("bin"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            name.starts_with("python3.15") && !name.ends_with("config")
        })
        .unwrap();

    // The launcher, as `pcx module-launcher` serves it.
    let run = tempfile::tempdir_in("/tmp").unwrap();
    let store = run.path().join("store");
    let sockets = run.path().join("sockets");
    std::fs::create_dir_all(&sockets).unwrap();
    let socket = run.path().join("launcher.sock");
    let sandbox = Launcher::Seatbelt;
    // `maturin develop` leaves the binding where it was built: a link into the
    // repository, which an installed one is not; the same for the TS binding.
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let deno =
        PathBuf::from(std::env::var("PCX_BINDINGS_DENO").expect("PCX_BINDINGS_DENO names Deno"));
    let node_modules = repository.join("bindings/ts/node_modules");
    let imports = run.path().join("imports.json");
    std::fs::write(
        &imports,
        json!({ "imports": {
            "@post-cortex/module": repository.join("bindings/ts/index.js"),
            "@bufbuild/protobuf": node_modules.join("@bufbuild/protobuf/dist/esm/index.js"),
            "@bufbuild/protobuf/wkt": node_modules.join("@bufbuild/protobuf/dist/esm/wkt/index.js"),
            "@bufbuild/protobuf/codegenv2": node_modules.join("@bufbuild/protobuf/dist/esm/codegenv2/index.js"),
        } })
        .to_string(),
    )
    .unwrap();
    // The plugin runs on Deno: a host has no Node.
    let plugin = run.path().join("protoc-gen-es");
    std::fs::write(
        &plugin,
        format!(
            "#!/bin/sh\nexec {} run --no-prompt --allow-all {} \"$@\"\n",
            deno.display(),
            node_modules
                .join("@bufbuild/protoc-gen-es/bin/protoc-gen-es")
                .display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&plugin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let read = vec![
        plugin.clone(),
        prefix,
        base,
        repository.join("bindings"),
        deno.parent().unwrap().to_path_buf(),
        run.path().join("imports.json"),
    ];
    let served = Served::new(
        ServedConfig {
            runtimes: vec![venv_python.clone(), deno.clone()],
            programs: Vec::new(),
            network_programs: Vec::new(),
            always_read: read.clone(),
            stores: vec![store.clone()],
            sockets: sockets.clone(),
            scratch: run.path().join("scratch"),
            host_group: None,
        },
        Preparer::new(
            PrepareConfig {
                store,
                python: base_python,
                read,
                wheelhouse: None,
                protoc: Some(protoc),
                protoc_gen_es: Some(plugin),
                deno: Some((deno.clone(), imports.clone())),
            },
            sandbox.clone(),
        ),
        sandbox,
    );
    let stop = CancellationToken::new();
    let launcher_task = tokio::spawn({
        let (socket, stop) = (socket.clone(), stop.clone());
        async move { serve(served, &socket, stop).await }
    });

    let launcher = LauncherConfig {
        socket,
        sockets,
        runtimes: Runtimes {
            python: venv_python,
            deno: deno.clone(),
            imports,
        },
    };
    Stack {
        launcher,
        stop,
        task: launcher_task,
        run,
        deno,
        repository,
    }
}

/// Memory with blobs, its directory and a workspace of it.
pub async fn memory_up() -> (tempfile::TempDir, Arc<post_cortex_memory::Memory>, String) {
    let data = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn post_cortex_ports::BlobStore> =
        Arc::new(post_cortex_blobs::ObjectBlobs::in_memory());
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory_with_blobs(
            &data,
            blobs,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let workspace = memory
        .workspaces
        .create(CreateWorkspace {
            name: "demo".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
        .to_string();
    (data, memory, workspace)
}
