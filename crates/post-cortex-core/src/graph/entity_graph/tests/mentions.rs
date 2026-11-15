// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn test_merge_from_dedupes_mentions() {
    let mut a = SimpleEntityGraph::new();
    let mut b = SimpleEntityGraph::new();
    let now = Utc::now();
    let shared = uuid::Uuid::new_v4();
    let only_b = uuid::Uuid::new_v4();
    a.add_or_update_entity("x".to_string(), EntityType::Concept, now, "");
    b.add_or_update_entity("x".to_string(), EntityType::Concept, now, "");
    a.entity_mentions
        .insert("x".to_string(), vec![shared].into());
    b.entity_mentions
        .insert("x".to_string(), vec![shared, only_b, only_b].into());

    a.merge_from(&b);
    assert_eq!(a.entity_mentions["x"].to_vec(), vec![shared, only_b]);
}

/// Records applied out of order still leave first = earliest and
/// last = latest mention.
#[test]
fn out_of_order_mentions_keep_first_and_last() {
    let mut g = SimpleEntityGraph::new();
    g.apply_update(&record(20, &["x"], &[]));
    g.apply_update(&record(10, &["x"], &[]));
    let x = &g.entities["x"];
    assert_eq!((x.first_mentioned, x.last_mentioned), (at(10), at(20)));
    assert_eq!(x.mention_count, 2);
}

/// Replaying the same records gives the same graph, including the
/// endpoints a relation creates (stamped with the record's time).
#[test]
fn replay_is_deterministic() {
    let records = [
        record(1, &["a"], &[("a", "only-in-relation")]),
        record(2, &["b"], &[("b", "a")]),
    ];
    let build = || {
        let mut g = SimpleEntityGraph::new();
        for r in &records {
            g.apply_update(r);
        }
        g
    };
    let first = build();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = build();
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
    let created = &first.entities["only-in-relation"];
    assert_eq!(
        (created.first_mentioned, created.last_mentioned),
        (at(1), at(1))
    );
}

/// Merging graphs sums mention counts and keeps the earliest first,
/// the latest last mention and the highest importance.
#[test]
fn merge_from_sums_counts_and_keeps_extremes() {
    let mut a = SimpleEntityGraph::new();
    for t in [10, 20, 30] {
        a.apply_update(&record(t, &["x"], &[]));
    }
    a.entities.get_mut("x").unwrap().importance_score = 0.4;
    let mut b = SimpleEntityGraph::new();
    for t in [5, 25] {
        b.apply_update(&record(t, &["x"], &[]));
    }
    b.entities.get_mut("x").unwrap().importance_score = 0.9;

    let mut merged = SimpleEntityGraph::new();
    merged.merge_from(&a);
    merged.merge_from(&b);
    let x = &merged.entities["x"];
    assert_eq!(x.mention_count, 5);
    assert_eq!((x.first_mentioned, x.last_mentioned), (at(5), at(30)));
    assert_eq!(x.importance_score, 0.9);
}

/// Applying a record changes exactly what `changed_by` names: every other
/// entity and edge is as before, so a save that writes those rows stores
/// the new graph. An alias resolves to its entity.
#[test]
fn a_record_changes_only_what_changed_by_names() {
    let mut g = SimpleEntityGraph::new();
    g.apply_update(&record(1, &["a", "b"], &[("a", "b")]));
    g.apply_update(&record(2, &["c", "d"], &[("c", "d")]));
    // "A" is another spelling of "a".
    assert_eq!(g.resolve("A"), Some("a"));
    let before = g.clone();
    let update = record(3, &["A", "e"], &[("A", "e"), ("c", "d")]);
    g.apply_update(&update);

    let (entities, edges) = g.changed_by(&update);
    let mut changed: Vec<&str> = entities.iter().map(|e| e.name.as_str()).collect();
    changed.sort();
    assert_eq!(changed, ["a", "c", "d", "e"]);
    for (name, entity) in g.entities.iter_unordered() {
        if !changed.contains(&name.as_str()) {
            let json = |e: &EntityData| serde_json::to_value(e).unwrap();
            assert_eq!(
                Some(json(entity)),
                before.entities.get(name).map(json),
                "{name}"
            );
        }
    }
    let mut stated: Vec<(String, String)> = edges
        .iter()
        .map(|e| (e.from_entity.clone(), e.to_entity.clone()))
        .collect();
    stated.sort();
    assert_eq!(stated, [("a".into(), "e".into()), ("c".into(), "d".into())]);
    let all = |g: &SimpleEntityGraph| {
        let mut v: Vec<String> = g
            .get_all_relationships()
            .iter()
            .map(|r| format!("{} {} {}", r.from_entity, r.to_entity, r.context))
            .collect();
        v.sort();
        v
    };
    let new: Vec<String> = all(&g)
        .into_iter()
        .filter(|e| !all(&before).contains(e))
        .collect();
    assert!(
        new.iter().all(|e| stated
            .iter()
            .any(|(f, t)| e.starts_with(&format!("{f} {t} ")))),
        "{new:?}"
    );
}
