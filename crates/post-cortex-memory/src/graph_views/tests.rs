// Copyright (c) 2026 Julius ML
// MIT License

use super::*;
use crate::graph_views::test_support::{at, rec, scope};
use chrono::{Duration, Utc};
use post_cortex_core::core::context_update::{EntityType, RelationType, UpdateType};
use post_cortex_core::core::dismissal::{RadarDismissal, RadarItemKind, alias_key};
use std::collections::HashMap;

use EntityType::{CodeComponent as Code, Concept, Decision, Problem, Technology as Tech};
use RelationType::*;

#[tokio::test]
async fn entity_view_resolves_aliases_and_lists_decisions() {
    let decision = rec(0, UpdateType::DecisionMade, "Use RocksDB")
        .about(&[("RocksDB", Tech)])
        .update();
    let change = rec(3, UpdateType::CodeChanged, "rocks tuning")
        .about(&[("rocks_db", Tech)])
        .update();
    let s = scope(vec![decision.clone(), change.clone()]).await;
    let view = Index::of(&s)
        .entity_view("ROCKS DB", 10)
        .expect("alias resolves");
    assert_eq!(view.entity.name, "RocksDB");
    assert_eq!(view.total_records, 2);
    assert_eq!(view.records[0].id, change.id, "newest first");
    assert_eq!(view.decisions.len(), 1);
    assert_eq!(view.decisions[0].id, decision.id);
    assert_eq!(view.records[0].author.as_deref(), Some("claude-code"));
    assert!(Index::of(&s).entity_view("nothing", 10).is_none());
}

#[tokio::test]
async fn impact_view_walks_dependents_and_reports_risks() {
    let s = scope(vec![
        rec(0, UpdateType::DecisionMade, "api uses core")
            .about(&[("api", Code), ("core", Code)])
            .rel("api", "core", DependsOn)
            .update(),
        rec(1, UpdateType::RequirementAdded, "ui needs api")
            .about(&[("ui", Code), ("api", Code)])
            .rel("ui", "api", DependsOn)
            .update(),
        rec(2, UpdateType::QuestionAnswered, "what is core")
            .about(&[("core", Code)])
            .update(),
    ])
    .await;
    let view = Index::of(&s).impact_view("core", 3, 10).unwrap();
    let found: Vec<(&str, usize)> = view
        .dependents
        .iter()
        .map(|d| (d.entity.as_str(), d.depth))
        .collect();
    assert_eq!(found, [("api", 1), ("ui", 2)]);
    assert_eq!(view.dependents[1].via, "api");
    let kinds: Vec<&str> = view
        .risks
        .iter()
        .map(|r| r.interaction_type.as_str())
        .collect();
    assert_eq!(
        kinds,
        ["requirement_added", "decision_made"],
        "no QA, newest first"
    );

    let shallow = Index::of(&s).impact_view("core", 1, 10).unwrap();
    assert_eq!(shallow.dependents.len(), 1);
}

#[tokio::test]
async fn lineage_orders_problem_decision_consequence() {
    let s = scope(vec![
        rec(0, UpdateType::ProblemSolved, "slow search")
            .about(&[("slow search", Problem), ("full scan", Concept)])
            .rel("slow search", "full scan", CausedBy)
            .update(),
        rec(5, UpdateType::DecisionMade, "add HNSW")
            .about(&[("HNSW", Decision), ("slow search", Problem)])
            .rel("HNSW", "slow search", Solves)
            .update(),
        rec(9, UpdateType::CodeChanged, "HNSW leads to more RAM")
            .about(&[("HNSW", Decision), ("RAM use", Concept)])
            .rel("HNSW", "RAM use", LeadsTo)
            .update(),
        rec(12, UpdateType::DecisionMade, "unrelated")
            .about(&[("logging", Concept)])
            .update(),
    ])
    .await;
    let view = Index::of(&s).lineage_view("HNSW", 3).unwrap();
    let titles: Vec<&str> = view.steps.iter().map(|s| s.record.title.as_str()).collect();
    assert_eq!(
        titles,
        ["slow search", "add HNSW", "HNSW leads to more RAM"]
    );
    assert!(view.chain.contains(&"full scan".to_string()));
}

/// From a problem the chain reaches what caused it and what solved it, not
/// the other problems that solution also solved, nor what led to those
/// (the drift seen on "duplicate derived work on update", 2026-11-07).
#[tokio::test]
async fn lineage_does_not_turn_at_a_solution() {
    let s = scope(vec![
        rec(0, UpdateType::ProblemSolved, "duplicate work")
            .about(&[("duplicate work", Problem), ("service", Code)])
            .rel("duplicate work", "service", CausedBy)
            .update(),
        rec(2, UpdateType::CodeChanged, "persister coalesces")
            .about(&[("persister", Code), ("duplicate work", Problem)])
            .rel("persister", "duplicate work", Solves)
            .update(),
        rec(4, UpdateType::CodeChanged, "persister also fixes the audit")
            .about(&[("persister", Code), ("audit", Problem)])
            .rel("persister", "audit", Solves)
            .update(),
        rec(6, UpdateType::DecisionMade, "the audit leads to the law")
            .about(&[("audit", Problem), ("law", Concept)])
            .rel("audit", "law", LeadsTo)
            .update(),
    ])
    .await;
    let view = Index::of(&s).lineage_view("duplicate work", 3).unwrap();
    assert_eq!(view.chain, ["duplicate work", "service", "persister"]);
    let titles: Vec<&str> = view.steps.iter().map(|s| s.record.title.as_str()).collect();
    assert_eq!(titles, ["duplicate work", "persister coalesces"]);
}

#[tokio::test]
async fn radar_finds_gaps_and_alias_candidates() {
    let bare = rec(1, UpdateType::DecisionMade, "go with surreal").described("");
    let s = scope(vec![
        rec(0, UpdateType::ProblemSolved, "leak")
            .about(&[
                ("memory leak", Problem),
                ("Surreal", Tech),
                ("SurrealDB", Tech),
            ])
            .update(),
        bare.about(&[("Surreal", Tech)]).update(),
        rec(2, UpdateType::ProblemSolved, "fixed")
            .about(&[("timeout", Problem), ("retry", Concept)])
            .rel("retry", "timeout", Solves)
            .update(),
    ])
    .await;
    let view = Index::of(&s).radar_view(at(0) + Duration::days(200), 10);
    let unsolved: Vec<&str> = view
        .unsolved_problems
        .iter()
        .map(|n| n.name.as_str())
        .collect();
    assert_eq!(unsolved, ["memory leak"]);
    assert_eq!(view.decisions_without_rationale.len(), 1);
    assert_eq!(view.stale_entities.len(), 5);
    assert_eq!(view.alias_candidates.len(), 1);
    let candidate = &view.alias_candidates[0];
    assert_eq!(
        (candidate.name.as_str(), candidate.of.as_str()),
        ("Surreal", "SurrealDB")
    );
    assert_eq!(candidate.shared_records, 1);

    // Dismissed items leave the lists (and are listed as dismissed) unless
    // asked for; a dismissal that matches nothing is not listed.
    let dismiss = |kind, key: String| RadarDismissal {
        kind,
        key,
        dismissed_at: at(5),
        author: None,
    };
    let decision = view.decisions_without_rationale[0].id;
    let dismissals = [
        dismiss(RadarItemKind::UnsolvedProblem, " memory leak ".into()),
        dismiss(
            RadarItemKind::DecisionWithoutRationale,
            decision.to_string(),
        ),
        dismiss(RadarItemKind::StaleEntity, "retry".into()),
        dismiss(
            RadarItemKind::AliasCandidate,
            alias_key("Surreal", "SurrealDB"),
        ),
        dismiss(RadarItemKind::StaleEntity, "gone".into()),
    ];
    let now = at(0) + Duration::days(200);
    let filtered = Index::of(&s).radar_view_with(now, 10, &dismissals, false);
    assert!(filtered.unsolved_problems.is_empty());
    assert!(filtered.decisions_without_rationale.is_empty());
    assert!(filtered.alias_candidates.is_empty());
    assert_eq!(filtered.stale_entities.len(), 4);
    assert_eq!(filtered.dismissed.len(), 4);
    let all = Index::of(&s).radar_view_with(now, 10, &dismissals, true);
    assert_eq!(all.unsolved_problems.len(), 1);
    assert_eq!(all.stale_entities.len(), 5);
    assert_eq!(all.dismissed.len(), 4);

    let request = ViewRequest::parse(
        "radar",
        &[("include_dismissed".to_string(), "true".to_string())].into(),
    )
    .unwrap();
    assert_eq!(
        request,
        ViewRequest::Radar {
            limit: DEFAULT_LIMIT,
            include_dismissed: true
        }
    );
    assert!(
        ViewRequest::parse(
            "radar",
            &[("include_dismissed".to_string(), "maybe".to_string())].into(),
        )
        .is_err()
    );
    let rendered = s.render_with(
        &ViewRequest::parse("radar", &HashMap::new()).unwrap(),
        &dismissals,
        Utc::now(),
    );
    let rendered = serde_json::to_value(rendered.unwrap()).unwrap();
    assert_eq!(rendered["dismissed"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn topics_group_entities_with_their_decisions() {
    let s = scope(vec![
        rec(0, UpdateType::DecisionMade, "search stack")
            .about(&[("bm25", Concept), ("hnsw", Concept), ("rrf", Concept)])
            .rel("bm25", "rrf", RelatedTo)
            .rel("hnsw", "rrf", DependsOn)
            .rel("bm25", "hnsw", RelatedTo)
            .update(),
        rec(1, UpdateType::DecisionMade, "deploy")
            .about(&[("systemd", Tech), ("ml host", Concept), ("unit file", Code)])
            .rel("systemd", "ml host", DependsOn)
            .rel("unit file", "systemd", Implements)
            .rel("unit file", "ml host", RelatedTo)
            .update(),
    ])
    .await;
    let view = Index::of(&s).topics_view(10, 3);
    assert_eq!(view.topics.len(), 2);
    assert_eq!(view.unclustered, 0);
    for topic in &view.topics {
        assert_eq!(topic.size, 3);
        assert_eq!(topic.key_decisions.len(), 1);
        assert_eq!(topic.records, 1);
    }
}

#[tokio::test]
async fn file_view_finds_past_problems_and_dependents() {
    let mut bug = rec(0, UpdateType::ProblemSolved, "panic on empty scope")
        .about(&[("scope resolver", Concept)])
        .update();
    bug.related_code = Some(post_cortex_core::core::context_update::CodeReference {
        file_path: "crates/post-cortex-memory/src/graph_views.rs".into(),
        start_line: 1,
        end_line: 2,
        code_snippet: String::new(),
        commit_hash: None,
        branch: None,
        change_description: String::new(),
    });
    let s = scope(vec![
        bug.clone(),
        rec(1, UpdateType::DecisionMade, "REST renders graph_views")
            .about(&[("rmcp_server", Code), ("graph_views", Code)])
            .rel("rmcp_server", "graph_views", DependsOn)
            .update(),
        rec(2, UpdateType::DecisionMade, "unrelated main")
            .about(&[("main", Code)])
            .update(),
    ])
    .await;
    let view = Index::of(&s).file_view(
        "/Users/someone/repo/crates/post-cortex-memory/src/graph_views.rs",
        2,
        10,
    );
    assert_eq!(view.problems.len(), 1);
    assert_eq!(view.problems[0].id, bug.id);
    assert_eq!(view.decisions.len(), 1);
    let names: Vec<&str> = view.entities.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["graph_views"]);
    assert_eq!(view.dependents.len(), 1);
    assert_eq!(view.dependents[0].entity, "rmcp_server");
    assert_eq!(view.records.len(), 2);

    // A bare file name of another directory does not match the path.
    let other = Index::of(&s).file_view("/elsewhere/main.rs", 2, 10);
    assert!(other.records.is_empty() && other.entities.is_empty());
}
