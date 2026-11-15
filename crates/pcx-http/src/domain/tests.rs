// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_proto::http::{Connection as Given, Pair};

use super::*;

const WORKSPACE: &str = "6f1c2a3e-0000-4000-8000-000000000001";

pub fn github() -> Given {
    Given {
        name: "github".into(),
        base_url: "https://api.github.com/".into(),
        auth: AuthKind::Bearer.into(),
        secret: "ghp_secret".into(),
        headers: vec![Pair {
            name: "Accept".into(),
            value: "application/vnd.github+json".into(),
        }],
        ..Given::default()
    }
}

fn config(connections: Vec<Given>) -> HttpConfig {
    HttpConfig {
        workspace_id: WORKSPACE.into(),
        connections,
        allow: vec!["api.example.com".into(), "*.example.org".into()],
        ..HttpConfig::default()
    }
}

#[test]
fn settings_are_read_with_their_defaults() {
    let s = settings(config(vec![github()]), true).unwrap();
    assert_eq!(s.daily_limit, 5_000);
    let c = s.connection("github").unwrap();
    assert_eq!(c.base.as_str(), "https://api.github.com/");
    assert_eq!(c.auth, Auth::Bearer("ghp_secret".into()));
    assert_eq!(
        c.headers,
        [(
            "accept".to_string(),
            "application/vnd.github+json".to_string()
        )]
    );
    assert_eq!((c.timeout, c.test_path.as_str()), (DEFAULT_TIMEOUT, "/"));
    assert!(s.connection("nope").is_err());
}

#[test]
fn allow_names_hosts_and_subdomains() {
    let s = settings(config(Vec::new()), true).unwrap();
    assert!(s.allows("api.example.com"));
    assert!(s.allows("API.Example.com"));
    assert!(s.allows("x.example.org"));
    assert!(!s.allows("example.org"));
    assert!(!s.allows("evil-example.org"));
    assert!(!s.allows("example.com"));
}

#[test]
fn settings_the_rules_refuse_name_their_field() {
    let refused = |c: HttpConfig, public: bool| settings(c, public).unwrap_err().field;
    let mut private = config(Vec::new());
    private.private_networks = true;
    assert_eq!(refused(private.clone(), true), "private_networks");
    assert!(settings(private, false).is_ok());
    let mut limit = config(Vec::new());
    limit.daily_limit = 100_001;
    assert_eq!(refused(limit, false), "daily_limit");
    let mut plain = github();
    plain.base_url = "http://api.github.com".into();
    assert_eq!(
        refused(config(vec![plain]), false),
        "connections[0].base_url"
    );
    let mut no_secret = github();
    no_secret.secret = String::new();
    assert_eq!(
        refused(config(vec![no_secret]), false),
        "connections[0].secret"
    );
    let mut header = github();
    header.auth = AuthKind::Header.into();
    assert_eq!(
        refused(config(vec![header.clone()]), false),
        "connections[0].auth_name"
    );
    header.auth_name = "Authorization".into();
    assert_eq!(
        refused(config(vec![header]), false),
        "connections[0].auth_name"
    );
    assert_eq!(
        refused(config(vec![github(), github()]), false),
        "connections[1].name"
    );
    let mut with_query = github();
    with_query.base_url = "https://api.github.com/?x=1".into();
    assert_eq!(
        refused(config(vec![with_query]), false),
        "connections[0].base_url"
    );
    let mut wide = config(Vec::new());
    wide.allow = vec!["https://example.com".into()];
    assert_eq!(refused(wide, false), "allow[0]");
}

#[test]
fn http_reaches_only_a_loopback_host_with_private_networks() {
    assert!(base_url("http://127.0.0.1:8080/api/", true).is_ok());
    assert!(base_url("http://localhost:8080", true).is_ok());
    assert!(base_url("http://127.0.0.1:8080", false).is_err());
    assert!(base_url("http://10.0.0.1", true).is_err());
    assert!(base_url("ftp://example.com", true).is_err());
    assert!(base_url("https://user:pw@example.com", false).is_err());
    assert_eq!(
        base_url("https://example.com/api/", false).unwrap().path(),
        "/api"
    );
}
