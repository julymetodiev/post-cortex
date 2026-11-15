// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_proto::http::{Auth as AuthKind, Connection as Wire, HttpConfig, Pair};
use post_cortex_proto::workflow::{Field as Wanted, Schema};

use super::*;
use crate::domain::settings;
use crate::domain::tests::github;

fn with(connections: Vec<Wire>) -> Settings {
    settings(
        HttpConfig {
            workspace_id: "6f1c2a3e-0000-4000-8000-000000000001".into(),
            connections,
            allow: vec!["api.example.com".into()],
            ..HttpConfig::default()
        },
        false,
    )
    .unwrap()
}

fn get(path: &str, params: &[(&str, &str)]) -> HttpRequest {
    HttpRequest {
        connection: "github".into(),
        method: "get".into(),
        path: path.into(),
        path_params: params
            .iter()
            .map(|(k, v)| Pair {
                name: (*k).to_string(),
                value: (*v).to_string(),
            })
            .collect(),
        ..HttpRequest::default()
    }
}

#[test]
fn a_request_through_a_connection_carries_its_credentials() {
    let s = with(vec![github()]);
    let mut request = get(
        "/repos/{owner}/{repo}/issues",
        &[("owner", "octo cat"), ("repo", "a/b")],
    );
    request.query = vec![Pair {
        name: "state".into(),
        value: "open & closed".into(),
    }];
    request.headers = vec![Pair {
        name: "Accept".into(),
        value: "text/plain".into(),
    }];
    let planned = plan(&request, &s, true, None).unwrap();
    assert_eq!(planned.method, Method::Get);
    assert_eq!(
        planned.url.as_str(),
        "https://api.github.com/repos/octo%20cat/a%2Fb/issues?state=open+%26+closed"
    );
    assert_eq!(
        planned.headers,
        [
            ("accept".to_string(), "text/plain".to_string()),
            ("authorization".to_string(), "Bearer ghp_secret".to_string()),
        ]
    );
    assert_eq!(planned.secrets, ["ghp_secret"]);
    let held = planned.credentials.unwrap();
    assert_eq!(held.headers, ["authorization"]);
    assert_eq!(planned.timeout, DEFAULT_TIMEOUT);
}

#[test]
fn a_key_in_the_query_and_the_idempotency_header_are_the_connections() {
    let mut stripe = github();
    stripe.name = "stripe".into();
    stripe.base_url = "https://api.stripe.com".into();
    stripe.auth = AuthKind::Query.into();
    stripe.auth_name = "api_key".into();
    stripe.idempotency_header = "Idempotency-Key".into();
    let s = with(vec![stripe]);
    let mut request = get("/v1/charges", &[]);
    request.connection = "stripe".into();
    request.method = "POST".into();
    let planned = plan(&request, &s, false, Some("run:block:1")).unwrap();
    assert_eq!(planned.url.query(), Some("api_key=ghp_secret"));
    assert!(
        planned
            .headers
            .contains(&("idempotency-key".to_string(), "run:block:1".to_string()))
    );
    assert_eq!(
        planned.credentials.unwrap().query.as_deref(),
        Some("api_key")
    );
    request.headers = vec![Pair {
        name: "Idempotency-Key".into(),
        value: "mine".into(),
    }];
    assert_eq!(
        plan(&request, &s, false, None).unwrap_err().field,
        "headers[0]"
    );
}

#[test]
fn a_path_never_leaves_its_connection() {
    let s = with(vec![github()]);
    let refused = |r: HttpRequest| plan(&r, &s, true, None).unwrap_err().field;
    assert_eq!(refused(get("/a/../b", &[])), "path");
    assert_eq!(refused(get("/a?x=1", &[])), "path");
    assert_eq!(refused(get("a", &[])), "path");
    assert_eq!(refused(get("/{x}", &[("x", "..")])), "path_params.x");
    assert_eq!(refused(get("/{x}", &[])), "path_params.x");
    assert_eq!(refused(get("/x", &[("y", "1")])), "path_params.y");
    assert_eq!(refused(get("/{x", &[])), "path");
    let mut absolute = get("/x", &[]);
    absolute.url = "https://evil.example/x".into();
    assert_eq!(refused(absolute), "url");
}

#[test]
fn without_a_connection_only_an_allowed_host_is_reached() {
    let s = with(Vec::new());
    let to = |url: &str| HttpRequest {
        method: "GET".into(),
        url: url.into(),
        ..HttpRequest::default()
    };
    let planned = plan(&to("https://api.example.com/v1?a=1"), &s, true, None).unwrap();
    assert!(planned.credentials.is_none() && planned.secrets.is_empty());
    let denied = plan(&to("https://other.example.com/"), &s, true, None).unwrap_err();
    assert_eq!(denied.kind, pcx_module_api::ModuleErrorKind::Denied);
    assert_eq!(
        plan(&to("http://api.example.com/"), &s, true, None)
            .unwrap_err()
            .field,
        "url"
    );
    assert_eq!(plan(&to(""), &s, true, None).unwrap_err().field, "url");
}

#[test]
fn a_whole_number_of_a_json_body_goes_as_an_integer() {
    let s = with(vec![github()]);
    let mut post = get("/x", &[]);
    post.method = "POST".into();
    let fields = [
        ("limit", pbjson_types::Value::from(1.0)),
        ("ratio", pbjson_types::Value::from(0.5)),
        ("ids", vec![pbjson_types::Value::from(-3.0)].into()),
    ];
    post.body = Some(Given::Json(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<std::collections::HashMap<_, _>>()
            .into(),
    ));
    let body = plan(&post, &s, false, None).unwrap().body.unwrap();
    let sent: serde_json::Value = serde_json::from_slice(&body.bytes).unwrap();
    assert_eq!(
        sent,
        serde_json::json!({"limit": 1, "ratio": 0.5, "ids": [-3]})
    );
    assert!(sent["limit"].is_u64() && sent["ids"][0].is_i64() && sent["ratio"].is_f64());
}

#[test]
fn http_get_takes_a_read_and_the_rules_hold_the_rest() {
    let s = with(vec![github()]);
    let mut post = get("/x", &[]);
    post.method = "POST".into();
    assert_eq!(plan(&post, &s, true, None).unwrap_err().field, "method");
    post.body = Some(Given::Json(pbjson_types::Value::from("hi".to_string())));
    let planned = plan(&post, &s, false, None).unwrap();
    let body = planned.body.unwrap();
    assert_eq!(
        (body.bytes.as_slice(), body.content_type.as_str()),
        (&b"\"hi\""[..], "application/json")
    );
    post.method = "TRACE".into();
    assert_eq!(plan(&post, &s, false, None).unwrap_err().field, "method");
    let mut slow = get("/x", &[]);
    slow.timeout = Some(prost_types::Duration {
        seconds: 301,
        nanos: 0,
    });
    assert_eq!(plan(&slow, &s, true, None).unwrap_err().field, "timeout");
    slow.timeout = Some(prost_types::Duration {
        seconds: 0,
        nanos: 0,
    });
    assert_eq!(plan(&slow, &s, true, None).unwrap_err().field, "timeout");
    let mut status = get("/x", &[]);
    status.ok = vec![404, 99];
    assert_eq!(plan(&status, &s, true, None).unwrap_err().field, "ok[1]");
    let mut cookie = get("/x", &[]);
    cookie.headers = vec![Pair {
        name: "Cookie".into(),
        value: "a=b".into(),
    }];
    assert_eq!(
        plan(&cookie, &s, true, None).unwrap_err().field,
        "headers[0]"
    );
    let mut split = get("/x", &[]);
    split.headers = vec![Pair {
        name: "X-A".into(),
        value: "a\r\nb: c".into(),
    }];
    assert_eq!(
        plan(&split, &s, true, None).unwrap_err().field,
        "headers[0]"
    );
    let mut huge = get("/x", &[]);
    huge.method = "PUT".into();
    huge.body = Some(Given::Raw(vec![0; MAX_BODY + 1]));
    assert_eq!(plan(&huge, &s, false, None).unwrap_err().field, "body");
}

#[test]
fn expect_is_read_as_a_schema() {
    let s = with(vec![github()]);
    let mut request = get("/x", &[]);
    request.expect = Some(Schema {
        fields: vec![Wanted {
            name: "full_name".into(),
            r#type: "string".into(),
            required: true,
            ..Wanted::default()
        }],
    });
    let planned = plan(&request, &s, true, None).unwrap();
    assert_eq!(planned.expect.unwrap().fields[0].name, "full_name");
    request.expect.as_mut().unwrap().fields[0].r#type = "strin".into();
    assert_eq!(
        plan(&request, &s, true, None).unwrap_err().field,
        "expect.fields[0].type"
    );
    request.expect = None;
    request.expect_list = true;
    assert_eq!(
        plan(&request, &s, true, None).unwrap_err().field,
        "expect_list"
    );
}

#[test]
fn a_path_expands_each_place_once() {
    let one = |name: &str, value: &str| Pair {
        name: name.into(),
        value: value.into(),
    };
    assert_eq!(
        path::expand("/p/{a}/q", &[one("a", "x~y.z")]).unwrap(),
        "/p/x~y.z/q"
    );
    let twice = path::expand("/p/{a}", &[one("a", "1"), one("a", "2")]).unwrap_err();
    assert_eq!(twice.field, "path_params[1].name");
}
