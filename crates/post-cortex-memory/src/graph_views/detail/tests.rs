// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The detail views over records built without a memory system: the graph
//! and timeline, the record browser and its cursor, records in full, the
//! overview and files; the entity page, topics, radar and file view
//! (`explore`); pages of a stream and search hits (`streams`).

use super::*;
use crate::graph_views::Index;
use crate::graph_views::test_support::{at, rec, scope, workspace};
use chrono::Duration;
use post_cortex_core::core::context_update::{EntityType, RelationType, UpdateType};
use post_cortex_core::core::dismissal::{RadarDismissal, RadarItemKind};
use post_cortex_core::graph::temporal::EndReason;
use uuid::Uuid;

use EntityType::{CodeComponent as Code, Problem, Solution, Technology as Tech};
use RelationType::*;

#[tokio::test]
async fn graph_shows_aliases_validity_and_assertions() {
    let first = rec(0, UpdateType::DecisionMade, "storage")
        .about(&[("SurrealDB", Tech), ("pcx", Code)])
        .rel("pcx", "SurrealDB", DependsOn)
        .update();
    let again = rec(5, UpdateType::CodeChanged, "schema")
        .about(&[("surreal-db", Tech), ("pcx", Code)])
        .rel("pcx", "surreal-db", DependsOn)
        .update();
    let s = scope(vec![first.clone(), again.clone()]).await;
    let index = Index::of(&s);

    let g = index.graph_detail(10, None);
    assert_eq!(g.total_nodes, 2);
    let surreal = g.nodes.iter().find(|n| n.node.name == "SurrealDB").unwrap();
    assert_eq!(surreal.node.aliases, ["surreal-db"]);
    assert_eq!(surreal.node.records, 2);
    assert!(surreal.node.importance > 0.0);
    assert_eq!(g.edges.len(), 1, "both spellings are one edge");
    let edge = &g.edges[0].edge;
    assert_eq!(edge.records, [first.id, again.id]);
    assert_eq!(edge.valid_from, Some(at(0)));
    assert_eq!(edge.valid_to, None);
    assert_eq!(edge.relation_type, "depends_on");
    assert!(edge.weight > 0.0);

    // As it stood before the first record: nothing.
    let before = index.graph_detail(10, Some(at(0) - Duration::days(1)));
    assert_eq!((before.total_nodes, before.total_edges), (0, 0));

    let limited = index.graph_detail(1, None);
    assert_eq!(limited.nodes.len(), 1);
    assert!(limited.edges.is_empty(), "edges need both ends shown");
}

#[tokio::test]
async fn timeline_shows_supersession_and_contradicted_relations() {
    let old = rec(0, UpdateType::DecisionMade, "cache depends on redis")
        .about(&[("cache", Code), ("redis", Tech)])
        .rel("cache", "redis", DependsOn)
        .update();
    let replace = rec(10, UpdateType::ProblemSolved, "redis removed")
        .about(&[("cache", Code), ("redis", Tech)])
        .rel("redis", "cache", ConflictsWith)
        .update();
    let s = scope(vec![old.clone(), replace]).await;
    let index = Index::of(&s);

    let t = index.timeline(None, None, 100);
    let kinds: Vec<EventKind> = t.events.iter().map(|e| e.kind).collect();
    use EventKind::*;
    assert_eq!(
        kinds,
        [
            Recorded,
            RelationAdded,
            Ended,
            RelationEnded,
            Recorded,
            RelationAdded
        ]
    );
    let ended = &t.events[2];
    assert_eq!(ended.record, old.id);
    assert_eq!(ended.reason, Some(EndReason::Contradicted));
    assert_eq!(ended.at, at(10));
    assert_eq!(
        t.events[1].relation.as_ref().unwrap().relation_type,
        DependsOn
    );

    assert_eq!(index.timeline(Some(at(5)), None, 100).total, 4);
    let latest = index.timeline(None, None, 2);
    assert_eq!((latest.events.len(), latest.total), (2, 6));
}

#[tokio::test]
async fn graph_nodes_and_edges_carry_authors_and_sessions() {
    let mine = rec(0, UpdateType::DecisionMade, "storage")
        .about(&[("SurrealDB", Tech), ("pcx", Code)])
        .rel("pcx", "SurrealDB", DependsOn)
        .update();
    let theirs = rec(5, UpdateType::CodeChanged, "schema")
        .by("julius")
        .about(&[("SurrealDB", Tech), ("pcx", Code), ("api", Code)])
        .rel("pcx", "SurrealDB", DependsOn)
        .rel("api", "pcx", DependsOn)
        .update();
    let s = workspace(vec![vec![mine], vec![theirs]]).await;
    let second = s.sessions[1].id();
    let mut both: Vec<Uuid> = s.sessions.iter().map(|m| m.id()).collect();
    both.sort();

    let g = Index::of(&s).graph_detail(10, None);
    let node = |name: &str| g.nodes.iter().find(|n| n.node.name == name).unwrap();
    assert_eq!(
        node("SurrealDB").provenance.authors,
        ["claude-code", "julius"]
    );
    assert_eq!(node("SurrealDB").provenance.sessions, both);
    assert_eq!(node("api").provenance.authors, ["julius"]);
    assert_eq!(node("api").provenance.sessions, [second]);

    let edge = |from: &str| g.edges.iter().find(|e| e.edge.from == from).unwrap();
    assert_eq!(edge("pcx").provenance.authors, ["claude-code", "julius"]);
    assert_eq!(edge("pcx").provenance.sessions, both);
    assert_eq!(edge("api").provenance.authors, ["julius"]);
    assert_eq!(edge("api").provenance.sessions, [second]);
}

/// Four recent records and one from three days before; the second record
/// supersedes the first.
async fn records_seeded() -> (crate::graph_views::GraphScope, Vec<Uuid>) {
    let old = rec(-3 * 24 * 60, UpdateType::ConceptDefined, "memory card")
        .about(&[("card", Code), ("pcx", Code)])
        .rel("pcx", "card", Implements)
        .update();
    let r1 = rec(0, UpdateType::DecisionMade, "use RocksDB")
        .about(&[("pcx", Code), ("RocksDB", Tech)])
        .rel("pcx", "RocksDB", DependsOn)
        .file("src/store.rs")
        .update();
    let r2 = rec(10, UpdateType::DecisionMade, "move to rocks-db")
        .about(&[("pcx", Code), ("rocks-db", Tech)])
        .rel("pcx", "rocks-db", DependsOn)
        .file("crates/x/src/store.rs")
        .supersedes(&[r1.id])
        .by("julius")
        .update();
    let r3 = rec(20, UpdateType::ProblemSolved, "cache stampede")
        .about(&[("cache", Code), ("stampede", Problem)])
        .rel("cache", "stampede", CausedBy)
        .described("fixed with a redis LOCK")
        .by("hermes:research")
        .update();
    let r4 = rec(30, UpdateType::QuestionAnswered, "what is pcx")
        .about(&[("pcx", Code), ("card", Code)])
        .rel("pcx", "card", RelatedTo)
        .update();
    let ids = vec![old.id, r1.id, r2.id, r3.id, r4.id];
    (scope(vec![old, r1, r2, r3, r4]).await, ids)
}

fn ids(records: &[RecordDetail]) -> Vec<Uuid> {
    records.iter().map(|r| r.id).collect()
}

#[tokio::test]
async fn filters_select_records_newest_first() {
    let (sc, id) = records_seeded().await;
    let index = Index::of(&sc);
    let (old, r1, r2, r3, r4) = (id[0], id[1], id[2], id[3], id[4]);
    let list = |f: RecordFilter| index.record_page(&f, 100, None);

    let all = list(RecordFilter::default());
    assert_eq!(ids(&all.records), [r4, r3, r2, r1, old]);
    assert_eq!((all.total, all.next), (5, None));

    let decisions = list(RecordFilter {
        types: vec![UpdateType::DecisionMade],
        ..Default::default()
    });
    assert_eq!(ids(&decisions.records), [r2, r1]);
    let by = list(RecordFilter {
        author: Some("julius".into()),
        ..Default::default()
    });
    assert_eq!(ids(&by.records), [r2]);

    // By alias, by canonical name, case-insensitively; unknown: nothing.
    for name in ["rocks-db", "RocksDB", "rocksdb"] {
        let about = list(RecordFilter {
            entity: Some(name.into()),
            ..Default::default()
        });
        assert_eq!(ids(&about.records), [r2, r1], "{name}");
    }
    let none = list(RecordFilter {
        entity: Some("nothing".into()),
        ..Default::default()
    });
    assert_eq!(none.total, 0);

    let text = list(RecordFilter {
        text: "lock".into(),
        ..Default::default()
    });
    assert_eq!(ids(&text.records), [r3]);

    let current = list(RecordFilter {
        validity: ValidityFilter::Current,
        ..Default::default()
    });
    assert_eq!(current.total, 4);
    let ended = list(RecordFilter {
        validity: ValidityFilter::Ended,
        ..Default::default()
    });
    assert_eq!(ids(&ended.records), [r1]);

    let window = list(RecordFilter {
        from: Some(at(10)),
        to: Some(at(20)),
        ..Default::default()
    });
    assert_eq!(ids(&window.records), [r3, r2]);

    let other_session = list(RecordFilter {
        session: Some(Uuid::new_v4()),
        ..Default::default()
    });
    assert_eq!(other_session.total, 0);
}

#[tokio::test]
async fn pages_follow_the_cursor() {
    let (sc, id) = records_seeded().await;
    let index = Index::of(&sc);
    let f = RecordFilter::default();
    let first = index.record_page(&f, 2, None);
    assert_eq!(ids(&first.records), [id[4], id[3]]);
    assert_eq!(first.total, 5);
    let cursor = first.next.unwrap();
    assert_eq!(cursor.to_string().parse::<PageCursor<Uuid>>(), Ok(cursor));
    let second = index.record_page(&f, 2, Some(cursor));
    assert_eq!(ids(&second.records), [id[2], id[1]]);
    let third = index.record_page(&f, 2, second.next);
    assert_eq!(ids(&third.records), [id[0]]);
    assert_eq!(third.next, None);
    for bad in [
        "nope",
        "12_nope",
        "12.x_00000000-0000-0000-0000-000000000000",
        "",
    ] {
        assert_eq!(bad.parse::<PageCursor<Uuid>>(), Err(InvalidCursor), "{bad}");
    }
}

#[tokio::test]
async fn records_carry_validity_provenance_and_entities() {
    let (sc, id) = records_seeded().await;
    let index = Index::of(&sc);
    let (r1, r2) = (id[1], id[2]);
    let first = index.record_detail(r1).unwrap();
    assert_eq!(first.validity.valid_from, at(0));
    assert_eq!(first.validity.valid_to, Some(at(10)));
    assert_eq!(first.validity.reason, Some(EndReason::Superseded));
    assert_eq!(first.validity.ended_by, Some(r2));
    assert_eq!(first.session_name, "main");
    assert_eq!(first.author.as_deref(), Some("claude-code"));
    assert_eq!(first.code.as_ref().unwrap().file_path, "src/store.rs");

    let second = index.record_detail(r2).unwrap();
    assert_eq!(second.supersedes, [r1]);
    assert_eq!(second.ended, [r1]);
    assert!(second.validity.valid_to.is_none());
    // Alias spellings come back under the canonical name.
    let rocks = second
        .entities
        .iter()
        .find(|e| e.name == "RocksDB")
        .unwrap();
    assert_eq!(rocks.entity_type, Some(Tech));
    assert_eq!(second.relations[0].relation.to, "RocksDB");
    assert_eq!(second.relations[0].relation.relation_type, DependsOn);
    assert_eq!(second.update_type, UpdateType::DecisionMade);

    assert!(index.record_detail(Uuid::new_v4()).is_none());
}

#[tokio::test]
async fn overview_counts_and_recent() {
    let (sc, id) = records_seeded().await;
    let o = Index::of(&sc).overview(at(60), 3, 10, &[]);
    assert_eq!((o.records, o.records_day, o.records_week), (5, 4, 5));
    assert_eq!(o.current_records, 4);
    assert_eq!(o.entities, 5, "rocks-db is RocksDB");
    assert!(o.relations >= 4);
    assert_eq!((o.sessions, o.active_sessions), (1, 1));
    assert_eq!(o.authors[0].author, "claude-code");
    assert_eq!(o.authors[0].records, 3);
    assert_eq!(o.authors.len(), 3);
    assert_eq!(o.first_activity, Some(at(-3 * 24 * 60)));
    assert_eq!(o.last_activity, Some(at(30)));
    assert_eq!(o.top_entities.len(), 3);
    assert!(o.top_entities[0].node.importance >= o.top_entities[2].node.importance);
    assert_eq!(
        ids(&o.recent_decisions),
        [id[2]],
        "the superseded one is not recent"
    );
    assert_eq!(o.recent_records.len(), 5);
    assert_eq!(o.radar.unsolved_problems, 1, "stampede has no solution");
    let set_aside = RadarDismissal {
        kind: RadarItemKind::UnsolvedProblem,
        key: "stampede".into(),
        dismissed_at: at(60),
        author: None,
    };
    let without = Index::of(&sc).overview(at(60), 3, 10, &[set_aside]);
    assert_eq!(
        without.radar.unsolved_problems, 0,
        "what a person set aside is not counted, as the radar's list leaves it out"
    );
    assert_eq!(o.session_stats[0].records, 5);
    assert_eq!(o.session_stats[0].name, "main");
}

#[tokio::test]
async fn files_group_code_references() {
    let (sc, _) = records_seeded().await;
    let index = Index::of(&sc);
    let f = index.files("", 10, None);
    assert_eq!(f.total, 2);
    let paths: Vec<&str> = f.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["crates/x/src/store.rs", "src/store.rs"]);
    assert_eq!(f.files[0].records, 1);
    assert_eq!(f.files[0].last_update_type, UpdateType::DecisionMade);
    assert_eq!(index.files("", 1, None).files.len(), 1);
}

#[tokio::test]
async fn files_page_by_their_last_touch() {
    let (sc, _) = records_seeded().await;
    let index = Index::of(&sc);
    let first = index.files("", 1, None);
    assert_eq!(first.files[0].path, "crates/x/src/store.rs");
    assert_eq!(first.total, 2);
    let cursor = first.next.unwrap();
    let raw = cursor.to_string();
    assert_eq!(raw.parse::<PageCursor<String>>(), Ok(cursor.clone()));
    let second = index.files("", 1, Some(cursor));
    assert_eq!(second.files[0].path, "src/store.rs");
    assert_eq!((second.total, second.next), (2, None));
    let crates = index.files(" CRATES/", 10, None);
    assert_eq!(crates.total, 1, "the text filters before the page");
    assert_eq!(crates.files[0].path, "crates/x/src/store.rs");
    let underscored = PageCursor {
        at: at(5),
        key: "src/a_b/c_d.rs".to_string(),
    };
    assert_eq!(
        underscored.to_string().parse::<PageCursor<String>>(),
        Ok(underscored)
    );
}

/// A code change names its files in its title (the `file` of the write):
/// they are files of the scope too, and the file's view finds the record.
#[tokio::test]
async fn code_changes_name_their_files() {
    let change = rec(
        40,
        UpdateType::CodeChanged,
        "crates/x/src/{store.rs, pool.rs (new)}, docs/STORE.md",
    )
    .about(&[("pool", Code)])
    .update();
    let id = change.id;
    let sc = scope(vec![change]).await;
    let index = Index::of(&sc);
    let mut paths: Vec<String> = index
        .files("", 10, None)
        .files
        .into_iter()
        .map(|f| f.path)
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            "crates/x/src/pool.rs",
            "crates/x/src/store.rs",
            "docs/STORE.md"
        ]
    );
    let detail = index.file_detail("/repo/crates/x/src/pool.rs", 2, 10);
    assert!(detail.records.iter().any(|r| r.id == id));
    assert!(index.file_detail("src/other.rs", 2, 10).records.is_empty());
}

mod explore;
mod streams;
