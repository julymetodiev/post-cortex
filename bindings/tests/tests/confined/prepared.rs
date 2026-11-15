// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A bundle prepared through the launcher: its lock file, its message code,
//! what a lock does not allow, a native package.

use super::*;

/// The Python a venv was made from, which has `pip`, and what it reads.
fn base_python() -> (PathBuf, Vec<PathBuf>) {
    let venv = runtime("PCX_BINDINGS_PYTHON");
    let out = Command::new(&venv)
        .args([
            "-c",
            "import sys; print(sys.base_prefix); print(sys.executable)",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let base = PathBuf::from(text.lines().next().unwrap());
    let found = std::fs::read_dir(base.join("bin"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("python3.15"))
                && !p.to_string_lossy().ends_with("config")
        })
        .expect("the base Python");
    (found, vec![base])
}

/// A tar of `files`, and its hash.
fn bundle(files: &[(&str, &str)]) -> (Vec<u8>, String) {
    use sha2::Digest;
    let mut builder = tar::Builder::new(Vec::new());
    for (name, text) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        builder
            .append_data(&mut header, name, text.as_bytes())
            .unwrap();
    }
    let bytes = builder.into_inner().unwrap();
    let digest = sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    (bytes, digest)
}

/// Dependencies through the launcher (docs/modules/dynamic.md, prepared): a
/// pure package from its lock, kept; a wrong hash, no hash and another
/// version of a native package refused; numpy from the wheelhouse where the
/// platform has one (`PCX_WHEELHOUSE`), importable in a module's confinement,
/// and refused without one, as nothing is ever compiled.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the built bindings: scripts/test-bindings.sh"]
async fn a_bundle_is_prepared_from_its_lock() {
    const SIX: &str = "six==1.17.0 \\\n    --hash=sha256:4721f391ed90541fddacab5acf947aa0d3dc7d27b2e1e8eda2be8970586c3274\n";
    const WRONG: &str = "six==1.17.0 \\\n    --hash=sha256:0000000000000000000000000000000000000000000000000000000000000000\n";
    let base = tempfile::tempdir_in("/tmp").unwrap();
    let (python, read) = base_python();
    let socket = base.path().join("launcher.sock");
    let stop = CancellationToken::new();
    let wheelhouse = std::env::var_os("PCX_WHEELHOUSE").map(PathBuf::from);
    let store = base.path().join("prepared");
    let preparer = Preparer::new(
        PrepareConfig {
            store: store.clone(),
            python: python.clone(),
            read: read.clone(),
            wheelhouse: wheelhouse.clone(),
            protoc: std::env::var_os("PROTOC").map(PathBuf::from),
            protoc_gen_es: None,
            deno: None,
        },
        sandbox(),
    );
    let served = Served::new(
        ServedConfig {
            runtimes: vec![python.clone()],
            programs: Vec::new(),
            network_programs: Vec::new(),
            always_read: read.clone(),
            stores: vec![store.clone()],
            sockets: base.path().to_path_buf(),
            scratch: base.path().join("scratch"),
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

    let (tar, sha) = bundle(&[("module.py", "print()\n"), ("requirements.lock", SIX)]);
    let prepared = prepare(&socket, tar.clone(), &sha).await.unwrap();
    assert!(Path::new(&prepared.bundle).join("module.py").exists());
    assert!(Path::new(&prepared.environment).join("six.py").exists());
    // Kept: the same answer.
    let again = prepare(&socket, tar, &sha).await.unwrap();
    assert_eq!(again.environment, prepared.environment);

    // Its message code, written by the compiler as the jail's user.
    if std::env::var_os("PROTOC").is_some() {
        let proto = "syntax = \"proto3\";\npackage x.v1;\nmessage M { string s = 1; }\n";
        let (tar, sha) = bundle(&[("module.py", "print()\n"), ("proto/x/v1/x.proto", proto)]);
        let generated = prepare(&socket, tar, &sha).await.unwrap();
        assert!(
            Path::new(&generated.bundle)
                .join("gen/x/v1/x_pb2.py")
                .exists()
        );
    }

    // What a lock does not allow.
    for (lock, says) in [
        (WRONG, "six"),
        ("six==1.17.0\n", "hashes"),
        ("numpy==2.0.0 --hash=sha256:00\n", "numpy 2.5.4"),
    ] {
        let (tar, sha) = bundle(&[("requirements.lock", lock)]);
        let refused = prepare(&socket, tar, &sha).await.unwrap_err();
        assert!(refused.contains(says), "{lock}: {refused}");
    }
    let (tar, _) = bundle(&[("module.py", "print()\n")]);
    let refused = prepare(&socket, tar, "00").await.unwrap_err();
    assert!(refused.contains("hash"), "{refused}");

    // A native package: from the wheelhouse, else refused, never compiled.
    let numpy = format!("numpy==2.5.4 --hash=sha256:{NUMPY_SOURCE}\n");
    let (tar, sha) = bundle(&[("requirements.lock", &numpy)]);
    let result = prepare(&socket, tar, &sha).await;
    if wheelhouse.is_some() {
        let built = result.unwrap();
        let environment = PathBuf::from(&built.environment);
        let scratch = base.path().join("scratch").join("numpy");
        let imported = Confined::new(
            sandbox(),
            Watcher::start(),
            Confinement {
                read: read.iter().cloned().chain([environment.clone()]).collect(),
                written: Vec::new(),
                scratch,
                cwd: environment.clone(),
                env: vec![("PYTHONPATH".to_string(), environment.display().to_string())],
                network: false,
                spawn: false,
                memory_mb: MEMORY_MB,
                open_files: 256,
                sockets_group: None,
            },
        )
        .run(
            &python,
            &[
                "-c".to_string(),
                "import numpy; print(numpy.__version__)".to_string(),
            ],
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        assert!(imported.success, "{imported:?}");
        assert!(imported.lines.iter().any(|l| l == "2.5.4"), "{imported:?}");
    } else {
        let refused = result.unwrap_err();
        assert!(refused.contains("numpy"), "{refused}");
    }

    stop.cancel();
    server.await.unwrap().unwrap();
}
