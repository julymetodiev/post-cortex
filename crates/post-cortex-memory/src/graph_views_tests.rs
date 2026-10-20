// Copyright (c) 2026 Julius ML
// MIT License

use super::*;
use post_cortex_core::core::context_update::{
    EntityRelationship, TypedEntity, UpdateContent, UpdateMeta,
};

/// Records are written `minutes` after a fixed start.
fn at(minutes: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + Duration::days(20_000) + Duration::minutes(minutes)
}

struct Rec {
    minutes: i64,
    kind: UpdateType,
    title: &'static str,
    description: &'static str,
    entities: Vec<(&'static str, EntityType)>,
    relations: Vec<(&'static str, &'static str, RelationType)>,
    meta: UpdateMeta,
}

fn rec(minutes: i64, kind: UpdateType, title: &'static str) -> Rec {
    Rec {
        minutes,
        kind,
        title,
        description: "because",
        entities: Vec::new(),
        relations: Vec::new(),
        meta: UpdateMeta::default(),
    }
}

impl Rec {
    fn about(mut self, entities: &[(&'static str, EntityType)]) -> Self {
        self.entities = entities.to_vec();
        self
    }
    fn rel(mut self, a: &'static str, b: &'static str, r: RelationType) -> Self {
        self.relations.push((a, b, r));
        self
    }
    fn update(&self) -> ContextUpdate {
        ContextUpdate {
            id: Uuid::new_v4(),
            timestamp: at(self.minutes),
            update_type: self.kind.clone(),
            content: UpdateContent {
                title: self.title.to_string(),
                description: self.description.to_string(),
                details: Vec::new(),
                examples: Vec::new(),
                implications: Vec::new(),
            },
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: Vec::new(),
            creates_relationships: self
                .relations
                .iter()
                .map(|(a, b, r)| EntityRelationship {
                    from_entity: a.to_string(),
                    to_entity: b.to_string(),
                    relation_type: r.clone(),
                    context: format!("{a} {} {b}", r.as_str()),
                })
                .collect(),
            references_entities: Vec::new(),
            typed_entities: self
                .entities
                .iter()
                .map(|(n, t)| TypedEntity {
                    name: n.to_string(),
                    entity_type: t.clone(),
                })
                .collect(),
            author: Some("claude-code".into()),
            derived_from: Vec::new(),
            meta: self.meta.clone(),
        }
    }
}

async fn scope(records: Vec<ContextUpdate>) -> GraphScope {
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None);
    for update in records {
        session.add_incremental_update(update).await.unwrap();
    }
    GraphScope {
        graph: Arc::clone(&session.entity_graph),
        sessions: vec![Arc::new(session)],
    }
}

use EntityType::{CodeComponent as Code, Concept, Decision, Problem, Technology as Tech};
use RelationType::*;

#[tokio::test]
async fn graph_view_shows_aliases_validity_and_assertions() {
    let first = rec(0, UpdateType::DecisionMade, "storage")
        .about(&[("SurrealDB", Tech), ("pcx", Code)])
        .rel("pcx", "SurrealDB", DependsOn)
        .update();
    let again = rec(5, UpdateType::CodeChanged, "schema")
        .about(&[("surreal-db", Tech), ("pcx", Code)])
        .rel("pcx", "surreal-db", DependsOn)
        .update();
    let s = scope(vec![first.clone(), again.clone()]).await;

    let view = s.graph_view(10, None);
    assert_eq!(view.total_nodes, 2);
    let surreal = view.nodes.iter().find(|n| n.name == "SurrealDB").unwrap();
    assert_eq!(surreal.aliases, ["surreal-db"]);
    assert_eq!(surreal.records, 2);
    assert_eq!(view.edges.len(), 1, "both spellings are one edge");
    let edge = &view.edges[0];
    assert_eq!(edge.records, [first.id, again.id]);
    assert_eq!(edge.valid_from, Some(at(0)));
    assert_eq!(edge.valid_to, None);
    assert!(edge.weight > 0.0);

    // As it stood before the first record: nothing.
    let before = s.graph_view(10, Some(at(0) - Duration::days(1)));
    assert_eq!(before.total_edges, 0);

    let limited = s.graph_view(1, None);
    assert_eq!(limited.nodes.len(), 1);
    assert!(limited.edges.is_empty(), "edges need both ends shown");
}

#[tokio::test]
async fn entity_view_resolves_aliases_and_lists_decisions() {
    let decision = rec(0, UpdateType::DecisionMade, "Use RocksDB")
        .about(&[("RocksDB", Tech)])
        .update();
    let change = rec(3, UpdateType::CodeChanged, "rocks tuning")
        .about(&[("rocks_db", Tech)])
        .update();
    let s = scope(vec![decision.clone(), change.clone()]).await;
    let view = s.entity_view("ROCKS DB", 10).expect("alias resolves");
    assert_eq!(view.entity.name, "RocksDB");
    assert_eq!(view.total_records, 2);
    assert_eq!(view.records[0].id, change.id, "newest first");
    assert_eq!(view.decisions.len(), 1);
    assert_eq!(view.decisions[0].id, decision.id);
    assert_eq!(view.records[0].author.as_deref(), Some("claude-code"));
    assert!(s.entity_view("nothing", 10).is_none());
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
    let view = s.impact_view("core", 3, 10).unwrap();
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

    let shallow = s.impact_view("core", 1, 10).unwrap();
    assert_eq!(shallow.dependents.len(), 1);
}

#[tokio::test]
async fn timeline_shows_supersession_and_contradicted_relations() {
    let old = rec(0, UpdateType::DecisionMade, "cache depends on redis")
        .about(&[("cache", Code), ("redis", Tech)])
        .rel("cache", "redis", DependsOn)
        .update();
    let mut replace = rec(10, UpdateType::ProblemSolved, "redis removed")
        .about(&[("cache", Code), ("redis", Tech)])
        .rel("redis", "cache", ConflictsWith);
    replace.meta = UpdateMeta::default();
    let replace = replace.update();
    let s = scope(vec![old.clone(), replace.clone()]).await;

    let view = s.timeline(None, None, 100);
    let kinds: Vec<String> = view
        .events
        .iter()
        .map(|e| {
            serde_json::to_value(&e.kind)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "recorded",
            "relation_added",
            "ended",
            "relation_ended",
            "recorded",
            "relation_added"
        ]
    );
    let ended = &view.events[2];
    assert_eq!(ended.record, old.id);
    assert_eq!(ended.reason, Some(temporal::EndReason::Contradicted));
    assert_eq!(ended.at, at(10));

    let window = s.timeline(Some(at(5)), None, 100);
    assert_eq!(window.total, 4);
    let latest = s.timeline(None, None, 2);
    assert_eq!(latest.events.len(), 2);
    assert_eq!(latest.total, 6);
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
    let view = s.lineage_view("HNSW", 3).unwrap();
    let titles: Vec<&str> = view.steps.iter().map(|s| s.record.title.as_str()).collect();
    assert_eq!(
        titles,
        ["slow search", "add HNSW", "HNSW leads to more RAM"]
    );
    assert!(view.chain.contains(&"full scan".to_string()));
}

#[tokio::test]
async fn radar_finds_gaps_and_alias_candidates() {
    let mut bare = rec(1, UpdateType::DecisionMade, "go with surreal");
    bare.description = "";
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
    let view = s.radar_view(at(0) + Duration::days(200), 10);
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
    let view = s.topics_view(10, 3);
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
    let view = s.file_view(
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
    let other = s.file_view("/elsewhere/main.rs", 2, 10);
    assert!(other.records.is_empty() && other.entities.is_empty());
}
