// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The entity page, topics, radar and file view.

use super::*;
use post_cortex_core::core::dismissal::{RadarDismissal, RadarItemKind};

async fn explore_seeded() -> (crate::graph_views::GraphScope, Vec<Uuid>) {
    let e1 = rec(0, UpdateType::ProblemSolved, "stampede under load")
        .about(&[("stampede", Problem), ("cache", Code), ("lock", Solution)])
        .rel("lock", "stampede", Solves)
        .rel("stampede", "cache", RelatedTo)
        .update();
    let e2 = rec(5, UpdateType::DecisionMade, "cache depends on redis")
        .about(&[("cache", Code), ("redis", Tech)])
        .rel("cache", "redis", DependsOn)
        .file("src/cache.rs")
        .update();
    let e3 = rec(10, UpdateType::DecisionMade, "api depends on cache")
        .about(&[("api", Code), ("cache", Code)])
        .rel("api", "cache", DependsOn)
        .update();
    let e4 = rec(15, UpdateType::RequirementAdded, "cache p99 under 5 ms")
        .about(&[("cache", Code), ("leak", Problem)])
        .rel("leak", "cache", CausedBy)
        .update();
    let e5 = rec(20, UpdateType::DecisionMade, "use an api gateway")
        .described("")
        .about(&[("api", Code), ("gateway", Tech)])
        .rel("api", "gateway", DependsOn)
        .update();
    let ids = [&e1, &e2, &e3, &e4, &e5].map(|u| u.id).to_vec();
    (scope(vec![e1, e2, e3, e4, e5]).await, ids)
}

#[tokio::test]
async fn entity_page_has_records_impact_lineage_and_files() {
    let (sc, id) = explore_seeded().await;
    let index = Index::of(&sc);
    let cache = index.entity_detail("Cache", 100, 3, 3).unwrap();
    let node = &cache.entity;
    assert_eq!(node.node.name, "cache");
    assert_eq!(node.node.entity_type, "code_component");
    assert_eq!(cache.ranked, 7);
    assert!((1..=7).contains(&cache.rank));
    assert_eq!(cache.total_records, 4);
    assert_eq!(ids(&cache.records), [id[3], id[2], id[1], id[0]]);
    assert_eq!(ids(&cache.decisions), [id[2], id[1]]);
    assert!(
        cache
            .relations
            .iter()
            .any(|e| e.edge.from == "cache" && e.edge.to == "redis")
    );
    assert_eq!(cache.files.len(), 1);
    assert_eq!(cache.files[0].path, "src/cache.rs");
    // The records come in full.
    assert_eq!(cache.records[0].title, "cache p99 under 5 ms");
    // Nodes and relations carry who wrote them and where.
    let session = sc.sessions[0].id();
    assert_eq!(node.provenance.authors, ["claude-code"]);
    assert_eq!(node.provenance.sessions, [session]);
    assert!(cache.relations.iter().all(|e| {
        e.provenance.authors == ["claude-code"] && e.provenance.sessions == [session]
    }));

    let redis = index.entity_detail("redis", 100, 2, 3).unwrap();
    let deps: Vec<(&str, usize)> = redis
        .impact
        .dependents
        .iter()
        .map(|d| (d.entity.as_str(), d.depth))
        .collect();
    assert!(deps.contains(&("cache", 1)), "{deps:?}");
    assert!(deps.contains(&("api", 2)), "{deps:?}");
    assert_eq!(redis.impact.depth, 2);
    assert!(!redis.impact.risks.is_empty());

    let stampede = index.entity_detail("stampede", 100, 1, 3).unwrap();
    let lineage = stampede.lineage;
    assert_eq!(lineage.chain[0], "stampede");
    assert!(lineage.chain.contains(&"lock".to_string()));
    let step = &lineage.steps[0];
    assert_eq!(step.record.id, id[0]);
    assert_eq!(step.relation.as_ref().unwrap().relation_type, Solves);

    assert!(index.entity_detail("nothing", 100, 3, 3).is_none());
}

#[tokio::test]
async fn topics_cover_every_entity() {
    let (sc, _) = explore_seeded().await;
    let t = Index::of(&sc).topics_detail(100, 5);
    let clustered: usize = t.topics.iter().map(|t| t.size).sum();
    assert_eq!(clustered + t.unclustered, 7);
    for topic in &t.topics {
        assert!(!topic.central.is_empty() && topic.central.len() <= 5);
        assert!(topic.first.is_some() && topic.records > 0);
    }
}

#[tokio::test]
async fn radar_lists_what_needs_attention() {
    let (sc, id) = explore_seeded().await;
    let index = Index::of(&sc);
    let r = index.radar_detail(at(0) + Duration::days(200), 100, &[], false);
    let unsolved: Vec<&str> = r
        .unsolved_problems
        .iter()
        .map(|n| n.node.name.as_str())
        .collect();
    assert_eq!(unsolved, ["leak"], "stampede is solved by lock");
    assert_eq!(ids(&r.decisions_without_rationale), [id[4]]);
    assert_eq!(r.decisions_without_rationale[0].title, "use an api gateway");
    assert_eq!(r.stale_entities.len(), 7);
    assert!(
        index
            .radar_detail(at(30), 100, &[], false)
            .stale_entities
            .is_empty()
    );
}

#[tokio::test]
async fn radar_leaves_dismissed_items_out_before_the_limit() {
    let (sc, _) = explore_seeded().await;
    let index = Index::of(&sc);
    let now = at(0) + Duration::days(200);
    let dismissal = |kind, key: &str| RadarDismissal {
        kind,
        key: key.into(),
        dismissed_at: at(1),
        author: Some("ana".into()),
    };
    let dismissals = [
        dismissal(RadarItemKind::StaleEntity, "cache"),
        dismissal(RadarItemKind::UnsolvedProblem, "leak"),
        // Matches nothing: not listed as dismissed.
        dismissal(RadarItemKind::OpenQuestion, "nobody asked"),
    ];
    let r = index.radar_detail(now, 6, &dismissals, false);
    let stale: Vec<&str> = r
        .stale_entities
        .iter()
        .map(|n| n.node.name.as_str())
        .collect();
    assert_eq!(
        stale.len(),
        6,
        "6 of the 7 stale entities are left: {stale:?}"
    );
    assert!(!stale.contains(&"cache"));
    assert!(r.unsolved_problems.is_empty());
    let dismissed: Vec<(&str, &str)> = r
        .dismissed
        .iter()
        .map(|d| (d.kind.as_str(), d.key.as_str()))
        .collect();
    assert_eq!(
        dismissed,
        [("unsolved_problem", "leak"), ("stale_entity", "cache")]
    );
    assert_eq!(r.dismissed[0].author.as_deref(), Some("ana"));
    assert_eq!(r.dismissed[0].dismissed_at, at(1));

    let all = index.radar_detail(now, 100, &dismissals, true);
    assert_eq!(all.stale_entities.len(), 7);
    assert_eq!(all.unsolved_problems[0].node.name, "leak");
    assert_eq!(all.dismissed.len(), 2);
}

#[tokio::test]
async fn file_view_matches_by_trailing_components() {
    let (sc, id) = explore_seeded().await;
    let index = Index::of(&sc);
    let f = index.file_detail("/home/me/repo/src/cache.rs", 2, 20);
    assert_eq!(f.path, "/home/me/repo/src/cache.rs");
    assert!(ids(&f.records).contains(&id[1]));
    assert!(ids(&f.decisions).contains(&id[1]));
    let found = f.records.iter().find(|r| r.id == id[1]).unwrap();
    assert_eq!(found.code.as_ref().unwrap().file_path, "src/cache.rs");
    assert!(index.file_detail("other.rs", 2, 20).records.is_empty());
}
