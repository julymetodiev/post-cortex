// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

fn deny() -> Deny {
    Deny::new(&[
        ".env".into(),
        ".env.*".into(),
        "*.pem".into(),
        ".ssh".into(),
    ])
    .unwrap()
}

#[test]
fn a_component_of_a_path_is_denied_by_its_name() {
    let deny = deny();
    assert_eq!(deny.refused(Path::new(".env")).as_deref(), Some(".env"));
    assert_eq!(
        deny.refused(Path::new("app/.env.local")).as_deref(),
        Some(".env.local")
    );
    assert_eq!(
        deny.refused(Path::new("certs/server.pem")).as_deref(),
        Some("server.pem")
    );
    assert_eq!(
        deny.refused(Path::new(".ssh/config")).as_deref(),
        Some(".ssh")
    );
    assert_eq!(deny.refused(Path::new("src/env.rs")), None);
    assert_eq!(deny.refused(Path::new("environment/.envrc")), None);
}

#[test]
fn a_path_is_no_denied_name() {
    assert!(matches!(
        Deny::new(&["secrets/.env".into()]),
        Err(DenyError::Path(_))
    ));
    assert!(matches!(
        Deny::new(&["[ab].pem".into()]),
        Err(DenyError::Syntax(_))
    ));
}

#[test]
fn a_link_to_a_denied_name_is_seen_through() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".env"), b"KEY=1").unwrap();
    std::os::unix::fs::symlink(".env", root.path().join("innocent")).unwrap();
    let file = std::fs::File::open(root.path().join("innocent")).unwrap();
    assert_eq!(
        deny().refused_at(root.path(), &file).unwrap().as_deref(),
        Some(".env")
    );
    let plain = std::fs::File::open(root.path()).unwrap();
    assert_eq!(deny().refused_at(root.path(), &plain).unwrap(), None);
}

#[test]
fn a_denied_name_is_one_regex_component() {
    let deny = Deny::new(&[".env.*".into(), "id_rsa?".into()]).unwrap();
    assert_eq!(
        deny.component_regexes(),
        vec![r"\.env\.[^/]*".to_string(), r"id_rsa[^/]".to_string()]
    );
}
