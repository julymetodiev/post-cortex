// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_proto::http::Param;

use super::*;

fn issue() -> Preset {
    let param = |name: &str, at: Place| Param {
        name: name.into(),
        r#in: at.into(),
        r#type: "string".into(),
        required: true,
        ..Param::default()
    };
    Preset {
        name: "my-issue-create".into(),
        service: "github".into(),
        connection: "github".into(),
        base_url: "https://api.github.com".into(),
        auth: Auth::Bearer.into(),
        method: "POST".into(),
        path: "/repos/{owner}/{repo}/issues".into(),
        params: vec![
            param("owner", Place::Path),
            param("repo", Place::Path),
            param("title", Place::Body),
            param("X-Trace", Place::Header),
        ],
        ..Preset::default()
    }
}

#[test]
fn a_preset_checks() {
    assert!(check(&issue(), &[]).is_ok());
}

#[test]
fn a_preset_the_rules_refuse_names_its_field() {
    let refused = |p: Preset| check(&p, &[]).unwrap_err().field;
    assert_eq!(
        check(&issue(), &["my-issue-create".into()])
            .unwrap_err()
            .field,
        "name"
    );
    let mut p = issue();
    p.params.remove(1);
    assert_eq!(refused(p), "path");
    let mut p = issue();
    p.params[2].name = "Title".into();
    assert_eq!(refused(p), "params[2].name");
    let mut p = issue();
    p.params[0].r#in = Place::Unspecified.into();
    assert_eq!(refused(p), "params[0].in");
    let mut p = issue();
    p.params[0].r#type = "strin".into();
    assert_eq!(refused(p), "params[0].type");
    let mut p = issue();
    p.params.push(p.params[0].clone());
    assert_eq!(refused(p), "params[4].name");
    let mut p = issue();
    p.method = "TRACE".into();
    assert_eq!(refused(p), "method");
    let mut p = issue();
    p.response_list = true;
    assert_eq!(refused(p), "response_list");
    let mut p = issue();
    p.base_url = "http://api.github.com".into();
    assert_eq!(refused(p), "base_url");
}
