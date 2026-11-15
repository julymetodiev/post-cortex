// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use std::path::PathBuf;

use super::*;

/// A policy over a writable and a read-only root, and a directory outside
/// both.
struct Fixture {
    _dirs: [tempfile::TempDir; 3],
    policy: Policy,
    outside: PathBuf,
    /// No denied names.
    open: Deny,
}

fn fixture() -> Fixture {
    let dirs = [
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    ];
    std::fs::write(dirs[1].path().join("ro.txt"), b"read only").unwrap();
    std::fs::write(dirs[2].path().join("secret.txt"), b"secret").unwrap();
    let policy = Policy {
        roots: vec![
            Root {
                path: dirs[0].path().to_path_buf(),
                write: true,
            },
            Root {
                path: dirs[1].path().to_path_buf(),
                write: false,
            },
        ],
        network: false,
        spawn: false,
        deny: Vec::new(),
        local_only: false,
    };
    let outside = dirs[2].path().to_path_buf();
    Fixture {
        _dirs: dirs,
        policy,
        outside,
        open: Deny::new(&[]).unwrap(),
    }
}

fn at<'a>(f: &'a Fixture, path: &str) -> Result<Beneath<'a>, ModuleError> {
    beneath(&f.policy, &f.open, &SandboxPath::new(path))
}

#[test]
fn files_are_written_read_listed_and_moved_beneath_the_roots() {
    let f = fixture();
    write(&at(&f, "src/main.rs").unwrap(), b"fn main() {}", None).unwrap();
    assert_eq!(
        read(&at(&f, "src/main.rs").unwrap(), 100).unwrap(),
        b"fn main() {}"
    );
    assert_eq!(read(&at(&f, "src/main.rs").unwrap(), 2).unwrap(), b"fn");
    write(&at(&f, "src/main.rs").unwrap(), b"replaced", None).unwrap();
    let names: Vec<String> = list(&at(&f, "src").unwrap())
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, ["main.rs"], "no temporary file is left");
    rename(
        &at(&f, "src/main.rs").unwrap(),
        &at(&f, "src/lib.rs").unwrap(),
    )
    .unwrap();
    assert_eq!(
        metadata(&at(&f, "src/lib.rs").unwrap()).unwrap().kind,
        EntryKind::File
    );
    remove(&at(&f, "src/lib.rs").unwrap()).unwrap();
    assert_eq!(
        read(&at(&f, "src/lib.rs").unwrap(), 10).unwrap_err().kind,
        ModuleErrorKind::NotFound
    );

    // An absolute path under the second root, and the root by name.
    let ro = f.policy.roots[1].path.join("ro.txt");
    assert_eq!(
        read(&at(&f, ro.to_str().unwrap()).unwrap(), 100).unwrap(),
        b"read only"
    );
    let named = SandboxPath {
        root: Some(f.policy.roots[1].path.clone()),
        path: PathBuf::from("ro.txt"),
    };
    assert_eq!(
        read(&beneath(&f.policy, &f.open, &named).unwrap(), 100).unwrap(),
        b"read only"
    );
}

#[test]
fn nothing_escapes_its_root() {
    let f = fixture();
    let secret = f.outside.join("secret.txt");
    // An absolute path outside every root.
    assert_eq!(
        at(&f, secret.to_str().unwrap())
            .map(|_| ())
            .unwrap_err()
            .kind,
        ModuleErrorKind::Denied
    );
    // `..` above the root.
    let up = format!(
        "../{}/secret.txt",
        f.outside.file_name().unwrap().to_str().unwrap()
    );
    assert_eq!(
        read(&at(&f, &up).unwrap(), 100).unwrap_err().kind,
        ModuleErrorKind::Denied
    );
    // A symbolic link inside the root that points out.
    std::os::unix::fs::symlink(&f.outside, f.policy.roots[0].path.join("out")).unwrap();
    assert_eq!(
        read(&at(&f, "out/secret.txt").unwrap(), 100)
            .unwrap_err()
            .kind,
        ModuleErrorKind::Denied
    );
    let refused = write(&at(&f, "out/new.txt").unwrap(), b"x", None).unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied, "{refused:?}");
    assert!(!f.outside.join("new.txt").exists());
    // A root that is not one, and a write to a read-only root.
    let other = SandboxPath {
        root: Some(f.outside.clone()),
        path: PathBuf::from("secret.txt"),
    };
    assert_eq!(
        beneath(&f.policy, &f.open, &other)
            .map(|_| ())
            .unwrap_err()
            .kind,
        ModuleErrorKind::Denied
    );
    let ro = f.policy.roots[1].path.join("new.txt");
    assert_eq!(
        write(&at(&f, ro.to_str().unwrap()).unwrap(), b"x", None)
            .unwrap_err()
            .kind,
        ModuleErrorKind::Denied
    );
    // The root itself is never removed.
    assert_eq!(
        remove(&at(&f, "").unwrap()).unwrap_err().kind,
        ModuleErrorKind::Denied
    );
    // No roots: nothing at all.
    let none = Policy::default();
    assert_eq!(
        beneath(&none, &f.open, &SandboxPath::new("a"))
            .map(|_| ())
            .unwrap_err()
            .kind,
        ModuleErrorKind::Denied
    );
}

/// A runner's credentials: refused by name, through a link to one, through
/// a linked directory, and left out of a listing.
#[test]
fn a_denied_name_is_out_of_reach_however_it_is_named() {
    let f = fixture();
    let root = f.policy.roots[0].path.clone();
    let deny = Deny::new(&[".env".into(), ".ssh".into()]).unwrap();
    let at = |path: &str| beneath(&f.policy, &deny, &SandboxPath::new(path));
    std::fs::write(root.join(".env"), b"KEY=secret").unwrap();
    std::fs::create_dir(root.join(".ssh")).unwrap();
    std::fs::write(root.join(".ssh/id_ed25519"), b"key").unwrap();
    std::fs::write(root.join("main.rs"), b"fn main() {}").unwrap();
    std::os::unix::fs::symlink(".env", root.join("innocent")).unwrap();
    std::os::unix::fs::symlink(".ssh", root.join("keys")).unwrap();

    let named = at(".env").err().unwrap();
    assert_eq!(named.kind, ModuleErrorKind::Denied, "{named:?}");
    assert!(at(".ssh/id_ed25519").is_err());
    let linked = read(&at("innocent").unwrap(), 100).unwrap_err();
    assert_eq!(linked.kind, ModuleErrorKind::Denied, "{linked:?}");
    let through = write(&at("keys/authorized_keys").unwrap(), b"attacker", None).unwrap_err();
    assert_eq!(through.kind, ModuleErrorKind::Denied, "{through:?}");
    assert!(!root.join(".ssh/authorized_keys").exists());
    assert!(remove(&at("keys/id_ed25519").unwrap()).is_err());
    assert!(root.join(".ssh/id_ed25519").exists());
    let names: Vec<String> = list(&at("").unwrap())
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(!names.contains(&".env".to_string()), "{names:?}");
    assert!(names.contains(&"main.rs".to_string()), "{names:?}");
    assert_eq!(read(&at("main.rs").unwrap(), 100).unwrap(), b"fn main() {}");
}

/// What a write makes belongs to the owner it names: every directory it made
/// and the file. As root (a FreeBSD node) that is another user's; otherwise
/// this one's, which still goes through each call.
#[test]
fn a_write_gives_what_it_makes_to_its_owner() {
    use std::os::unix::fs::MetadataExt;

    let f = fixture();
    let root = f.policy.roots[0].path.clone();
    let mine = std::fs::metadata(&root).unwrap();
    let owner = if mine.uid() == 0 {
        (65534, 65534)
    } else {
        (mine.uid(), mine.gid())
    };
    write(&at(&f, "a/b/c.txt").unwrap(), b"made", Some(owner)).unwrap();
    for made in ["a", "a/b", "a/b/c.txt"] {
        let meta = std::fs::metadata(root.join(made)).unwrap();
        assert_eq!((meta.uid(), meta.gid()), owner, "{made}");
    }
    assert_eq!(std::fs::read(root.join("a/b/c.txt")).unwrap(), b"made");
}
