// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::core::structured_context::StructuredContext;
use std::collections::HashMap;

/// Blob written by the pre-W2 serializer (every tier stored).
const FIXTURE_V1: &str = include_str!("../../../../tests/fixtures/active_session_v1.json");

/// The stored session as older readers parse it: every field required.
#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct LegacyActiveSessionData {
    id: Uuid,
    name: Option<String>,
    description: Option<String>,
    created_at: DateTime<Utc>,
    last_updated: DateTime<Utc>,
    user_preferences: UserPreferences,
    hot_context: Vec<ContextUpdate>,
    warm_context: Vec<serde_json::Value>,
    cold_context: Vec<serde_json::Value>,
    current_state: StructuredContext,
    incremental_updates: Vec<ContextUpdate>,
    code_references: HashMap<String, Vec<CodeReference>>,
    change_history: Vec<ChangeRecord>,
    entity_graph: SimpleEntityGraph,
    #[serde(default)]
    vectorized_update_ids: Vec<Uuid>,
}

/// Sort the vectorized-id set (DashSet order) and drop the fields that
/// are not the old blob's: the empty cold tier (old summaries are not
/// kept), derived change ids (they were random), the graph's record links
/// (old blobs stored them empty; they are derived on load), and the fields
/// records gained later (author, derived_from, meta, aliases), which read
/// back as defaults.
fn comparable(mut v: serde_json::Value) -> serde_json::Value {
    let obj = v.as_object_mut().unwrap();
    obj.remove("cold_context");
    obj["entity_graph"]
        .as_object_mut()
        .unwrap()
        .remove("entity_mentions");
    if let Some(serde_json::Value::Array(ids)) = obj.get_mut("vectorized_update_ids") {
        ids.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    }
    if let Some(serde_json::Value::Array(changes)) = obj.get_mut("change_history") {
        for c in changes {
            c.as_object_mut().unwrap().remove("id");
        }
    }
    without_new_update_fields(&mut v);
    v
}

fn without_new_update_fields(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(obj) => {
            if obj.get("author") == Some(&serde_json::Value::Null) {
                obj.remove("author");
            }
            if obj.get("derived_from") == Some(&serde_json::json!([])) {
                obj.remove("derived_from");
            }
            let no_meta =
                serde_json::to_value(crate::core::context_update::UpdateMeta::default()).unwrap();
            if obj.get("meta") == Some(&no_meta) {
                obj.remove("meta");
            }
            if obj.get("aliases") == Some(&serde_json::json!([])) {
                obj.remove("aliases");
            }
            obj.values_mut().for_each(without_new_update_fields);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(without_new_update_fields),
        _ => {}
    }
}

/// An old blob loads, and what this version writes for it is the old blob
/// again: the derived fields come out as the old writer stored them.
#[test]
fn fixture_v1_blob_loads_and_writes_the_same_views() {
    let stored: StoredSession = serde_json::from_str(FIXTURE_V1).unwrap();
    assert_eq!(stored.vectorized.len(), 2);
    let loaded = &stored.session;
    assert_eq!(loaded.records().len(), 7);
    assert_eq!(loaded.hot_records().len(), 4);
    assert_eq!(loaded.change_history().len(), 7);
    assert_eq!(loaded.entity_graph().get_graph_stats().edge_count, 7);
    assert_eq!(
        loaded.created_at(),
        Utc.with_ymd_and_hms(2024, 12, 31, 0, 0, 0).unwrap()
    );

    let original: serde_json::Value = serde_json::from_str(FIXTURE_V1).unwrap();
    let rewritten = serde_json::to_value(&stored).unwrap();
    assert_eq!(rewritten["cold_context"], serde_json::json!([]));
    assert_eq!(comparable(rewritten), comparable(original));

    // Older readers accept what this version writes.
    let json = serde_json::to_string(&stored).unwrap();
    let legacy: LegacyActiveSessionData = serde_json::from_str(&json).unwrap();
    assert_eq!(legacy.incremental_updates.len(), 7);
    assert_eq!(legacy.hot_context.len(), 4);
}

/// The stored derived fields are not read: a blob whose stored state
/// disagrees with its records (as different backends and older versions
/// wrote) loads with the state of its records.
#[test]
fn stored_views_are_ignored_on_load() {
    let s = populated_session();
    let mut v = serde_json::to_value(stored(&s)).unwrap();
    v["current_state"]["key_decisions"] = serde_json::json!([]);
    v["hot_context"] = serde_json::json!([]);
    v["code_references"] = serde_json::json!({});
    v["change_history"] = serde_json::json!([]);
    let loaded = serde_json::from_value::<StoredSession>(v.clone())
        .unwrap()
        .session;
    assert_eq!(loaded.current_state(), s.current_state());
    assert_eq!(loaded.hot_records().len(), 3);
    assert_eq!(loaded.code_references().len(), 1);

    // And they may be missing altogether.
    for field in [
        "hot_context",
        "warm_context",
        "cold_context",
        "current_state",
    ] {
        v.as_object_mut().unwrap().remove(field);
    }
    let loaded = serde_json::from_value::<StoredSession>(v).unwrap().session;
    assert_eq!(loaded.records().len(), 5);
}

#[test]
fn round_trip_keeps_records_graph_and_vectorized_ids() {
    let s = stored(&populated_session());
    let json = serde_json::to_string(&s).unwrap();
    let back: StoredSession = serde_json::from_str(&json).unwrap();
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
    assert_eq!(back.vectorized, s.vectorized);
    assert_eq!(
        back.session.entity_graph().entity_mentions,
        s.session.entity_graph().entity_mentions
    );
}

#[test]
fn view_without_incremental_updates_keeps_field() {
    let s = populated_session();
    let json = serde_json::to_string(&s.serializable_view().without_incremental_updates()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["incremental_updates"], serde_json::json!([]));
    assert_eq!(v["hot_context"].as_array().unwrap().len(), 3);
    let _: LegacyActiveSessionData = serde_json::from_str(&json).unwrap();
    let back: StoredSession = serde_json::from_str(&json).unwrap();
    assert!(back.session.records().is_empty());
}

/// `s` with its first record marked as embedded.
fn stored(s: &ActiveSession) -> StoredSession {
    let first = s.records()[0].id;
    StoredSession::new(s.clone(), vec![first])
}
