// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use std::collections::HashSet;

/// Names, RPCs, routes and MCP routes are each listed once.
#[test]
fn every_entry_is_unique() {
    let mut names = HashSet::new();
    for op in OPERATIONS {
        assert!(names.insert(op.name), "operation {} twice", op.name);
        assert!(op.capability.contains("::"), "{}", op.name);
        assert!(
            !op.mcp.is_empty()
                || !op.grpc.is_empty()
                || !op.views.is_empty()
                || !op.blobs.is_empty()
                || !op.rest.is_empty(),
            "{} has no transport",
            op.name
        );
    }
    let grpc = grpc_methods();
    assert_eq!(
        grpc.len(),
        grpc.iter().collect::<HashSet<_>>().len(),
        "an RPC twice"
    );
    let views = views_methods();
    assert_eq!(
        views.len(),
        views.iter().collect::<HashSet<_>>().len(),
        "a view RPC twice"
    );
    let blobs = blobs_methods();
    assert_eq!(
        blobs.len(),
        blobs.iter().collect::<HashSet<_>>().len(),
        "a blob RPC twice"
    );
    let rest = rest_routes();
    assert_eq!(
        rest.len(),
        rest.iter().collect::<HashSet<_>>().len(),
        "a route twice"
    );
    for tool in mcp_tools() {
        let values = mcp_values(tool);
        let unique: HashSet<_> = values.iter().collect();
        assert_eq!(values.len(), unique.len(), "{tool}: a value twice");
        let defaults = OPERATIONS
            .iter()
            .flat_map(|op| op.mcp)
            .filter(|r| r.tool == tool && r.values.is_empty())
            .count();
        assert!(defaults <= 1, "{tool}: two default routes");
    }
}

#[test]
fn an_mcp_call_reaches_its_operation() {
    let op = |tool, value| mcp_operation(tool, value).map(|op| op.name);
    assert_eq!(op("session", Some("load")), Some("session.get"));
    assert_eq!(
        op("manage_workspace", Some("UNARCHIVE")),
        Some("workspace.archive")
    );
    assert_eq!(
        op("query_conversation_context", Some("radar")),
        Some("view")
    );
    assert_eq!(
        op("update_conversation_context", Some("updates")),
        Some("record.bulk_write")
    );
    assert_eq!(
        op("update_conversation_context", None),
        Some("record.write")
    );
    assert_eq!(op("semantic_search", None), Some("search"));
    assert_eq!(op("query_conversation_context", Some("nope")), None);
}

/// The graph views the table routes are the ones memory renders.
#[test]
fn the_view_route_lists_every_graph_view() {
    let mut routed: Vec<&str> = operation("view").unwrap().mcp[0].values.to_vec();
    let mut views: Vec<&str> = crate::graph_views::VIEW_NAMES.to_vec();
    routed.sort_unstable();
    views.sort_unstable();
    assert_eq!(routed, views);
}
