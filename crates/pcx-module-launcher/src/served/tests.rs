// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

/// A change of a request that refuses it.
type Change = fn(&mut LaunchRequest);

fn served() -> Served {
    served_with(Vec::new(), Vec::new())
}

fn served_with(programs: Vec<PathBuf>, network_programs: Vec<PathBuf>) -> Served {
    Served {
        stopping: CancellationToken::new(),
        followed: TaskTracker::new(),
        config: ServedConfig {
            runtimes: vec!["/opt/python".into()],
            programs,
            network_programs,
            always_read: vec!["/opt".into()],
            stores: vec!["/store".into()],
            sockets: "/run/pcx".into(),
            scratch: "/scratch".into(),
            host_group: None,
        },
        preparer: Preparer::new(
            crate::prepare::PrepareConfig {
                store: "/store".into(),
                python: "/opt/python".into(),
                read: Vec::new(),
                wheelhouse: None,
                protoc: None,
                protoc_gen_es: None,
                deno: None,
            },
            Launcher::Unsupported,
        ),
        launcher: Launcher::Unsupported,
        watcher: Arc::new(Watcher::default()),
    }
}

fn request() -> LaunchRequest {
    LaunchRequest {
        module: "greet".into(),
        owner: "org-1".into(),
        program: "/opt/python".into(),
        socket: "/run/pcx/launch-1/m.sock".into(),
        read: vec!["/store/abc".into()],
        cwd: "/store/abc".into(),
        memory_mb: 256,
        open_files: 256,
        ..LaunchRequest::default()
    }
}

#[tokio::test]
async fn it_starts_only_what_it_was_given() {
    let (launch, confinement) = served().confinement(&request()).unwrap();
    assert_eq!(launch.id, "greet");
    assert_eq!(confinement.scratch, PathBuf::from("/scratch/org-1/greet"));
    assert_eq!(
        confinement.read,
        vec![PathBuf::from("/opt"), PathBuf::from("/store/abc")]
    );
    assert!(!confinement.network);
    assert!(!confinement.spawn);
    let refused: [(&str, Change); 8] = [
        ("neither a runtime nor a program", |r| {
            r.program = "/bin/sh".into();
        }),
        ("a name", |r| r.module = "../x".into()),
        ("a name", |r| r.owner = "a/b".into()),
        ("not a launch", |r| r.socket = "/elsewhere/l/m.sock".into()),
        ("not a launch", |r| r.socket = "/run/pcx/../x/m.sock".into()),
        ("not under", |r| r.read = vec!["/etc".into()]),
        ("not under", |r| r.read = vec!["/store/../etc".into()]),
        ("limits", |r| r.memory_mb = 0),
    ];
    for (why, change) in refused {
        let mut request = request();
        change(&mut request);
        let status = served().confinement(&request).unwrap_err();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert!(status.message().contains(why), "{why}: {status:?}");
    }
}

/// A program of the catalog and its SHA-256, in a directory kept while
/// the test runs.
fn program() -> (tempfile::TempDir, PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail");
    std::fs::write(&path, b"a module").unwrap();
    let sha = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(b"a module"));
    (dir, path, sha)
}

fn program_request(path: &Path, sha256: &str) -> LaunchRequest {
    LaunchRequest {
        module: "mail".into(),
        owner: "org-1".into(),
        program: path.to_string_lossy().into_owned(),
        socket: "/run/pcx/launch-2/m.sock".into(),
        memory_mb: 256,
        open_files: 1024,
        sha256: sha256.into(),
        ..LaunchRequest::default()
    }
}

#[tokio::test]
async fn a_program_of_the_catalog_has_the_network_only_as_the_launcher_says() {
    let (_dir, path, sha) = program();
    let without = served_with(vec![path.clone()], Vec::new());
    let (launch, confinement) = without.confinement(&program_request(&path, &sha)).unwrap();
    assert_eq!(launch.command, path);
    assert!(!confinement.network);
    assert_eq!(confinement.read, vec![PathBuf::from("/opt")]);
    assert_eq!(confinement.cwd, PathBuf::from("/scratch/org-1/mail"));
    let with = served_with(Vec::new(), vec![path.clone()]);
    let (_, confinement) = with.confinement(&program_request(&path, &sha)).unwrap();
    assert!(confinement.network);
    assert!(!confinement.spawn);
    // A runtime never has it, whatever the launcher's programs.
    let (_, runtime) = with.confinement(&request()).unwrap();
    assert!(!runtime.network);
}

#[tokio::test]
async fn a_program_is_refused_unless_it_is_the_approved_one_and_reads_nothing() {
    let (_dir, path, sha) = program();
    let served = served_with(Vec::new(), vec![path.clone()]);
    let refused: [(&str, Change); 4] = [
        ("not the approved", |r| r.sha256 = "00".repeat(32)),
        ("not the approved", |r| r.sha256 = String::new()),
        ("only the system's files", |r| {
            r.read = vec!["/store/abc".into()]
        }),
        ("only the system's files", |r| r.cwd = "/store/abc".into()),
    ];
    for (why, change) in refused {
        let mut request = program_request(&path, &sha);
        change(&mut request);
        let status = served.confinement(&request).unwrap_err();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert!(status.message().contains(why), "{why}: {status:?}");
    }
}

/// With a host group, the hosts of that group connect to the launcher,
/// make their launches' directories, and reach each launch's sockets: a
/// cloud that does not run as root (docs/modules/dynamic.md).
#[tokio::test]
async fn a_host_group_reaches_the_socket_and_the_launches() {
    use std::os::unix::fs::MetadataExt;
    let base = tempfile::tempdir().unwrap();
    let sockets = base.path().join("s");
    std::fs::create_dir(&sockets).unwrap();
    let socket = base.path().join("launcher.sock");
    // A group this user may give: the one its files already have.
    let group = std::fs::metadata(base.path()).unwrap().gid();
    let mut served = served();
    served.config.sockets.clone_from(&sockets);
    served.config.host_group = Some(group);
    let mut asked = request();
    asked.socket = sockets.join("launch-1/m.sock").display().to_string();
    let (_, confinement) = served.confinement(&asked).unwrap();
    assert_eq!(
        confinement.sockets_group,
        Some(group),
        "each launch's directory"
    );
    let stop = CancellationToken::new();
    let serving = tokio::spawn({
        let (socket, stop) = (socket.clone(), stop.clone());
        async move { serve(served, &socket, stop).await }
    });
    while !socket.exists() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let rights = |path: &Path| {
        let meta = std::fs::metadata(path).unwrap();
        (meta.mode() & 0o777, meta.gid())
    };
    assert_eq!(rights(&socket), (0o660, group), "the launcher's socket");
    assert_eq!(rights(&sockets), (0o770, group), "the launches' directory");
    stop.cancel();
    serving.await.unwrap().unwrap();
}
