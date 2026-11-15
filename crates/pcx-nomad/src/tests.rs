// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn a_name_or_an_id_never_leaves_its_path() {
    for good in ["pcx-cloud", "surrealdb-backup", "tikv_1", "pd.2"] {
        assert!(name(good), "{good}");
    }
    for bad in [
        "",
        "..",
        "../v1/acl/tokens",
        "a/b",
        "a b",
        "a?x=1",
        "-a",
        "surrealdb-backup/periodic-1794300000",
    ] {
        assert!(!name(bad), "{bad}");
    }
    assert!(allocation("7c4d5fa2"));
    assert!(allocation("7c4d5fa2-dc85-52f7-3a01-5c90f660cd2d"));
    for bad in [
        "7c4d5fa",
        "7c4d5fa2/stop",
        "7c4d5fa2-dc85-52f7-3a01-5c90f660cd2d0",
        "zzzzzzzz",
    ] {
        assert!(!allocation(bad), "{bad}");
    }
    assert_eq!(prefix("7c4d5fa2"), "7c4d5fa2");
    assert_eq!(prefix("7c4d5fa2-d"), "7c4d5fa2");
    assert_eq!(prefix("7c4d5fa2-"), "7c4d5fa2");
    assert_eq!(prefix("7c4d5fa2-dc8"), "7c4d5fa2-dc");
}

#[test]
fn settings_need_https_a_token_and_a_ca_for_a_name() {
    assert_eq!(settings("https://n:4646", "/c/t", "", ""), Ok(()));
    assert_eq!(
        settings("https://n:4646", "/c/t", "/c/ca.pem", "server.global.nomad"),
        Ok(())
    );
    assert_eq!(
        settings("http://n:4646", "/c/t", "", ""),
        Err(("nomad_api", "is no https URL"))
    );
    assert_eq!(
        settings("https://n:4646", " ", "", "").unwrap_err().0,
        "nomad_token_file"
    );
    assert_eq!(
        settings("https://n:4646", "/c/t", "", "server.global.nomad")
            .unwrap_err()
            .0,
        "nomad_tls_server_name"
    );
}
