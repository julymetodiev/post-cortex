// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::graph::entity_graph::matching::name_tokens;

fn graph_of(names: &[&str]) -> SimpleEntityGraph {
    let mut graph = SimpleEntityGraph::new();
    for name in names {
        graph.add_or_update_entity(name.to_string(), EntityType::Concept, at(0), "");
    }
    graph
}

#[test]
fn names_split_on_case_before_lowercasing() {
    assert_eq!(
        name_tokens("ChatRepositoryImpl"),
        ["chat", "repository", "impl"]
    );
    assert_eq!(name_tokens("HTTPServer"), ["http", "server"]);
    assert_eq!(name_tokens("PG LISTEN/NOTIFY"), ["pg", "listen", "notify"]);
    assert_eq!(name_tokens("svc-social"), ["svc", "social"]);
    assert_eq!(name_tokens("snake_case_name"), ["snake", "case", "name"]);
    assert_eq!(name_tokens("gRPC"), ["g", "rpc"]);
    assert_eq!(name_tokens("v2Api"), ["v2", "api"]);
}

/// A camelCase name is found from its words: before, it was lowercased
/// into one token ("chatrepositoryimpl") first, so only a text starting
/// that token ("chat...") found it.
#[test]
fn camel_case_names_are_found_from_their_words() {
    let graph = graph_of(&["ChatRepositoryImpl", "Billing"]);
    assert_eq!(
        entities_in("the repository implementation", &graph),
        ["ChatRepositoryImpl"]
    );
    assert!(entities_in("the repository", &graph).is_empty());
}

#[test]
fn names_in_the_text_rank_first() {
    let graph = graph_of(&["gRPC", "Axon", "tonic", "ServiceMesh"]);
    let found = entities_in("I'm working on the gRPC service mesh in Axon", &graph);
    // Same name length, so ties stay in name order.
    assert_eq!(found[..2], ["Axon".to_string(), "gRPC".to_string()]);
    assert!(found.contains(&"ServiceMesh".to_string()), "{found:?}");
    assert!(!found.contains(&"tonic".to_string()));
}
