// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use serde_json::json;

use super::*;

#[test]
fn the_ids_in_their_order() {
    // Z.AI's listing, as its endpoint answers.
    let listing = json!({"object": "list", "data": [
        {"id": "glm-5.3", "object": "model", "owned_by": "z-ai"},
        {"id": "glm-5.3-flash", "object": "model", "owned_by": "z-ai"},
        {"object": "model"}
    ]});
    let ids: Vec<String> = served(&listing).into_iter().map(|m| m.id).collect();
    assert_eq!(ids, ["glm-5.3", "glm-5.3-flash"]);
}

#[test]
fn a_listing_without_data_names_none() {
    assert!(served(&json!({"error": "no"})).is_empty());
}
