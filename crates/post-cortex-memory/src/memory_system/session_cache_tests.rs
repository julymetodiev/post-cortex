// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Tests of the session cache.

use std::sync::Arc;

use uuid::Uuid;

use post_cortex_core::session::active_session::ActiveSession;

use crate::memory_system::session_cache::{CachedSession, SessionCache, SessionHandle};

fn handle(id: Uuid) -> SessionHandle {
    Arc::new(CachedSession::new(
        ActiveSession::new(id, None, None, chrono::Utc::now()),
        [],
    ))
}

fn filled(cache: &SessionCache, n: usize) -> Vec<Uuid> {
    (0..n)
        .map(|_| {
            let id = Uuid::new_v4();
            cache.insert(id, handle(id));
            id
        })
        .collect()
}

#[test]
fn evicts_the_least_recently_used_over_capacity() {
    let cache = SessionCache::new(2).unwrap();
    let ids = filled(&cache, 4);
    // Touch the oldest: it becomes the most recent.
    assert!(cache.get(&ids[0]).is_some());
    let evicted = cache.evict_over_capacity(|_| false);
    assert_eq!(evicted, [ids[1], ids[2]]);
    assert_eq!(cache.len(), 2);
    assert!(cache.contains(&ids[0]) && cache.contains(&ids[3]));
    assert!(cache.evict_over_capacity(|_| false).is_empty());
    assert_eq!(cache.stats().evictions, 2);
}

/// Pinned sessions and sessions someone still holds stay, even over the
/// capacity; they go once released.
#[test]
fn pinned_and_held_sessions_stay() {
    let cache = SessionCache::new(1).unwrap();
    let ids = filled(&cache, 3);
    let held = cache.get(&ids[1]).unwrap();
    let evicted = cache.evict_over_capacity(|id| id == ids[0]);
    assert_eq!(evicted, [ids[2]], "only the unpinned, unheld one");
    assert_eq!(cache.len(), 2);

    drop(held);
    let evicted = cache.evict_over_capacity(|_| false);
    assert_eq!(evicted, [ids[0]]);
    assert!(cache.contains(&ids[1]), "the most recently used stays");
}

#[test]
fn counts_hits_and_misses() {
    let cache = SessionCache::new(4).unwrap();
    let ids = filled(&cache, 1);
    assert!(cache.get(&ids[0]).is_some());
    assert!(cache.get(&Uuid::new_v4()).is_none());
    assert!(cache.peek(&ids[0]).is_some());
    let stats = cache.stats();
    assert_eq!((stats.hits, stats.misses, stats.size), (1, 1, 1));
    assert_eq!(stats.hit_rate(), 0.5);
    assert!(cache.remove(&ids[0]).is_some());
    assert!(cache.is_empty());
    assert!(SessionCache::new(0).is_err());
}
