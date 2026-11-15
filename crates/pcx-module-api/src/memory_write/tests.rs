// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use serde_json::json;

use super::*;

fn task(workspace_id: &str) -> MemoryWrite {
    MemoryWrite::Document {
        workspace_id: workspace_id.to_string(),
        id: "d1".to_string(),
        kind: "board/task".to_string(),
        version: 2,
        deleted: false,
    }
}

fn record(record_type: Option<&str>) -> MemoryWrite {
    MemoryWrite::Record {
        workspace_id: "w1".to_string(),
        id: "r1".to_string(),
        record_type: record_type.map(str::to_string),
        deleted: record_type.is_none(),
    }
}

fn filter(kinds: &[&str], types: &[&str], workspaces: &[&str]) -> MemoryFilter {
    let owned = |items: &[&str]| items.iter().map(|s| s.to_string()).collect();
    MemoryFilter {
        document_kinds: owned(kinds),
        record_types: owned(types),
        workspaces: owned(workspaces),
    }
}

#[test]
fn the_event_carries_the_change_as_its_payload() {
    let event = task("w1").to_event();
    assert_eq!(event.hook_event_name, MEMORY_WRITE);
    let payload = Value::Object(json::object_from_wire(event.payload.unwrap()));
    assert_eq!(
        payload,
        json!({"change": "document", "workspace_id": "w1", "id": "d1", "kind": "board/task", "version": 2, "deleted": false})
    );
    let behind = MemoryWrite::FellBehind.to_event();
    let payload = Value::Object(json::object_from_wire(behind.payload.unwrap()));
    assert_eq!(payload, json!({"fell_behind": true}));
}

#[test]
fn a_filter_follows_what_it_lists() {
    assert!(task("w1").followed_by(None));
    let tasks = filter(&["board/task"], &[], &[]);
    assert!(task("w1").followed_by(Some(&tasks)));
    assert!(!record(Some("decision_made")).followed_by(Some(&tasks)));
    let in_w2 = filter(&["board/task"], &[], &["w2"]);
    assert!(!task("w1").followed_by(Some(&in_w2)));
    assert!(task("w2").followed_by(Some(&in_w2)));

    let decisions = filter(&[], &["decision_made"], &[]);
    assert!(record(Some("decision_made")).followed_by(Some(&decisions)));
    assert!(!record(Some("qa")).followed_by(Some(&decisions)));
    // A removed record's type is gone: any type filter hears of it.
    assert!(record(None).followed_by(Some(&decisions)));
    assert!(!record(None).followed_by(Some(&tasks)));
    assert!(MemoryWrite::FellBehind.followed_by(Some(&tasks)));
}

#[test]
fn a_change_reads_back_from_its_event() {
    for change in [
        MemoryWrite::Document {
            workspace_id: "w".into(),
            id: "d".into(),
            kind: "board/task".into(),
            version: 3,
            deleted: false,
        },
        MemoryWrite::Record {
            workspace_id: String::new(),
            id: "r".into(),
            record_type: None,
            deleted: true,
        },
        MemoryWrite::FellBehind,
    ] {
        assert_eq!(MemoryWrite::from_event(&change.to_event()).unwrap(), change);
    }
    assert!(MemoryWrite::from_event(&crate::Event::default()).is_err());
}
