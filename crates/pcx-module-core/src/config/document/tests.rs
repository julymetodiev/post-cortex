// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::DocumentKind;

/// A message of real descriptors: `name` and `description` are strings,
/// `update_count` a number.
const TYPE: &str = "pcx.v1.SessionInfo";

fn kind(name: &str, r#type: &str, title_field: &str) -> DocumentKind {
    DocumentKind {
        name: name.to_string(),
        title_field: title_field.to_string(),
        searchable: None,
        r#type: r#type.to_string(),
    }
}

fn manifest(kinds: Vec<DocumentKind>) -> Manifest {
    Manifest {
        id: "board".to_string(),
        document_kinds: kinds,
        descriptors: post_cortex_proto::PCX_DESCRIPTOR_SET.to_vec(),
        ..Manifest::default()
    }
}

#[test]
fn declared_kinds_are_read() {
    let kinds = DeclaredKind::of(&manifest(vec![kind("task", TYPE, "name")])).unwrap();
    assert_eq!(
        kinds,
        [DeclaredKind {
            module: "board".to_string(),
            name: "task".to_string(),
            type_name: TYPE.to_string(),
            title_field: "name".to_string(),
        }]
    );
    let none = Manifest {
        descriptors: Vec::new(),
        ..manifest(vec![])
    };
    assert!(DeclaredKind::of(&none).unwrap().is_empty());
}

#[test]
fn malformed_kinds_are_refused() {
    let cases = [
        (vec![kind("Task", TYPE, "name")], "lowercase"),
        (vec![kind("", TYPE, "name")], "lowercase"),
        (vec![kind("ta/sk", TYPE, "name")], "lowercase"),
        (
            vec![kind("task", TYPE, "name"), kind("task", TYPE, "name")],
            "twice",
        ),
        (
            vec![kind("task", "pcx.v1.Nothing", "name")],
            "not a message",
        ),
        (vec![kind("task", TYPE, "missing")], "title field"),
        (vec![kind("task", TYPE, "update_count")], "title field"),
    ];
    for (kinds, reason) in cases {
        let error = DeclaredKind::of(&manifest(kinds)).unwrap_err();
        assert!(error.to_string().contains(reason), "{error}");
    }
    let unreadable = Manifest {
        descriptors: vec![0xff, 0xff],
        ..manifest(vec![kind("task", TYPE, "name")])
    };
    let error = DeclaredKind::of(&unreadable).unwrap_err();
    assert!(error.reason.contains("cannot be read"), "{error}");
}
