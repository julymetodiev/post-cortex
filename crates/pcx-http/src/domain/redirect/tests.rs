// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_proto::http::{HttpConfig, HttpRequest};

use super::*;
use crate::domain::request::plan;
use crate::domain::settings;
use crate::domain::tests::github;

fn with_github() -> Settings {
    settings(
        HttpConfig {
            workspace_id: "6f1c2a3e-0000-4000-8000-000000000001".into(),
            connections: vec![github()],
            allow: vec!["objects.githubusercontent.com".into()],
            ..HttpConfig::default()
        },
        false,
    )
    .unwrap()
}

fn planned(s: &Settings, method: &str) -> Planned {
    let request = HttpRequest {
        connection: "github".into(),
        method: method.into(),
        path: "/repos/a/b/tarball".into(),
        body: (method == "POST")
            .then(|| post_cortex_proto::http::http_request::Body::Text("x".into())),
        ..HttpRequest::default()
    };
    plan(&request, s, false, None).unwrap()
}

#[test]
fn a_redirect_home_keeps_the_credentials() {
    let s = with_github();
    let next = follow(&planned(&s, "GET"), 302, "/repos/a/c/tarball", &s).unwrap();
    assert_eq!(
        next.url.as_str(),
        "https://api.github.com/repos/a/c/tarball"
    );
    assert!(next.headers.iter().any(|(n, _)| n == "authorization"));
    assert!(next.credentials.is_some());
}

#[test]
fn a_redirect_elsewhere_drops_them_and_needs_an_allowed_host() {
    let s = with_github();
    let next = follow(
        &planned(&s, "GET"),
        302,
        "https://objects.githubusercontent.com/x?token=t",
        &s,
    )
    .unwrap();
    assert!(!next.headers.iter().any(|(n, _)| n == "authorization"));
    assert!(next.credentials.is_none());
    let denied = follow(&planned(&s, "GET"), 302, "https://evil.example/x", &s).unwrap_err();
    assert_eq!(denied.kind, pcx_module_api::ModuleErrorKind::Denied);
    let plain = follow(&planned(&s, "GET"), 302, "http://api.github.com/x", &s).unwrap_err();
    assert_eq!(plain.field, "location");
}

#[test]
fn a_303_and_a_redirected_post_go_on_as_a_get() {
    let s = with_github();
    for status in [301, 302, 303] {
        let next = follow(&planned(&s, "POST"), status, "/done", &s).unwrap();
        assert_eq!(
            (next.method, next.body.is_none()),
            (Method::Get, true),
            "{status}"
        );
    }
    let kept = follow(&planned(&s, "POST"), 307, "/again", &s).unwrap();
    assert_eq!((kept.method, kept.body.is_some()), (Method::Post, true));
    assert!(redirects(308) && !redirects(304));
}
