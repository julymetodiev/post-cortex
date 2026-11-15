// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use post_cortex_core::core::context_update::EntityType;

fn graph_with(name: &str) -> Arc<SimpleEntityGraph> {
    let mut g = SimpleEntityGraph::new();
    g.add_or_update_entity(
        name.to_string(),
        EntityType::Concept,
        chrono::Utc::now(),
        "test",
    );
    Arc::new(g)
}

#[test]
fn reuses_merge_while_session_graphs_are_unchanged() {
    let cache = WorkspaceGraphCache::default();
    let ws = Uuid::new_v4();
    let (s1, s2) = (Uuid::new_v4(), Uuid::new_v4());
    let (g1, g2) = (graph_with("a"), graph_with("b"));

    let first = cache.merged(
        MergeKey::Workspace(ws),
        vec![(s1, g1.clone()), (s2, g2.clone())],
    );
    assert_eq!(first.entity_count(), 2);

    // Same graphs in a different order: a cache hit.
    let second = cache.merged(
        MergeKey::Workspace(ws),
        vec![(s2, g2.clone()), (s1, g1.clone())],
    );
    assert!(Arc::ptr_eq(&first, &second));

    // One session's graph replaced: a rebuild.
    let third = cache.merged(
        MergeKey::Workspace(ws),
        vec![(s1, g1), (s2, graph_with("c"))],
    );
    assert!(!Arc::ptr_eq(&first, &third));
    assert!(third.has_entity("c"));
    assert!(!third.has_entity("b"));
}

#[test]
fn membership_change_rebuilds() {
    let cache = WorkspaceGraphCache::default();
    let ws = Uuid::new_v4();
    let g1 = graph_with("a");
    let first = cache.merged(MergeKey::Workspace(ws), vec![(Uuid::new_v4(), g1.clone())]);
    let second = cache.merged(MergeKey::Workspace(ws), vec![(Uuid::new_v4(), g1)]);
    assert!(!Arc::ptr_eq(&first, &second));
}

/// At most [`CAPACITY`] workspaces are kept, least recently used out first.
#[test]
fn the_cache_is_bounded() {
    let cache = WorkspaceGraphCache::default();
    let (old, used) = (Uuid::new_v4(), Uuid::new_v4());
    let graph = graph_with("a");
    cache.merged(MergeKey::Workspace(old), Vec::new());
    let first = cache.merged(
        MergeKey::Workspace(used),
        vec![(Uuid::new_v4(), Arc::clone(&graph))],
    );
    for _ in 0..CAPACITY - 2 {
        cache.merged(MergeKey::Workspace(Uuid::new_v4()), Vec::new());
    }
    // `used` is touched, so `old` is the least recently used.
    let hit = cache.merged(
        MergeKey::Workspace(used),
        vec![(first_source(&cache, used), graph)],
    );
    assert!(Arc::ptr_eq(&first, &hit));
    cache.merged(MergeKey::Workspace(Uuid::new_v4()), Vec::new());
    assert_eq!(cache.len(), CAPACITY);
    assert!(
        !cache.entries.contains_key(&MergeKey::Workspace(old)),
        "the oldest went first"
    );
    assert!(cache.entries.contains_key(&MergeKey::Workspace(used)));
}

fn first_source(cache: &WorkspaceGraphCache, ws: Uuid) -> Uuid {
    cache.entries.get(&MergeKey::Workspace(ws)).unwrap().sources[0].0
}
