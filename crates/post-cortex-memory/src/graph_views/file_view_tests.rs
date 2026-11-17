// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The file view on the cases of board task b22dfd96: records that name a
//! file without an entity named after it, files named in details, cut
//! titles, and file names several directories share.

use crate::graph_views::GraphScope;
use crate::graph_views::index::Index;
use crate::graph_views::record_files::files_of;
use crate::graph_views::test_support::{rec, scope};
use post_cortex_core::core::context_update::{ContextUpdate, EntityType, RelationType, UpdateType};
use post_cortex_core::session::active_session::normalised;
use uuid::Uuid;

use EntityType::{CodeComponent as Code, Concept};

const SCOPE_RS: &str = "crates/pcx-module-core/src/scope.rs";

/// The scope.rs records as they were written: a change names the file and
/// two other files' code (a method of it, a crate it was changed with); a
/// test depends on the method, Discord code on the crate.
fn scope_rs_records() -> Vec<ContextUpdate> {
    vec![
        rec(
            0,
            UpdateType::CodeChanged,
            "crates/pcx-module-core/src/scope.rs, crates/pcx-discord/src/lib.rs, crates/pcx-host/src/route.rs",
        )
        .about(&[
            ("Scope::working_in", Code),
            ("Scope.tenant", Code),
            ("pcx-discord", Code),
            ("pcx-host route", Code),
            ("ServiceRoute::target", Code),
            ("member reach", Concept),
        ])
        .update(),
        rec(1, UpdateType::DecisionMade, "the runner tests read the call's workspace")
            .about(&[("compose_runner_tests", Code), ("Scope::working_in", Code)])
            .rel(
                "compose_runner_tests",
                "Scope::working_in",
                RelationType::DependsOn,
            )
            .update(),
        rec(2, UpdateType::DecisionMade, "Discord announces through a module")
            .about(&[("DiscordModule", Code), ("pcx-discord", Code)])
            .rel("DiscordModule", "pcx-discord", RelationType::Implements)
            .update(),
        rec(3, UpdateType::DecisionMade, "the host stamps the tenant")
            .about(&[("ServiceRoute::target", Code), ("Scope.tenant", Code)])
            .rel(
                "ServiceRoute::target",
                "Scope.tenant",
                RelationType::Implements,
            )
            .update(),
    ]
}

fn dependents(s: &GraphScope, path: &str) -> Vec<String> {
    Index::of(s)
        .file_view(path, 2, 10)
        .dependents
        .into_iter()
        .map(|d| d.entity)
        .collect()
}

fn names(nodes: &[crate::graph_views::GraphNode]) -> Vec<&str> {
    nodes.iter().map(|n| n.name.as_str()).collect()
}

/// The scope.rs case: its records name its methods (`Scope::working_in`,
/// `Scope.tenant`), which are its own entities, and other files' code,
/// which was changed with it and whose dependents are not its own.
#[tokio::test]
async fn a_files_own_entities_give_its_dependents_and_other_code_was_changed_with_it() {
    let s = scope(scope_rs_records()).await;
    let view = Index::of(&s).file_view(SCOPE_RS, 2, 10);
    assert_eq!(names(&view.entities), ["Scope.tenant", "Scope::working_in"]);
    let mut found = dependents(&s, SCOPE_RS);
    found.sort();
    assert_eq!(found, ["ServiceRoute::target", "compose_runner_tests"]);
    // ServiceRoute::target was changed with it too, but is said as a dependent.
    assert_eq!(names(&view.changed_with), ["pcx-discord", "pcx-host route"]);

    // Another scope.rs: the records that name this one and not the other
    // still say which entities are its own.
    let mut records = scope_rs_records();
    records.push(
        rec(
            4,
            UpdateType::CodeChanged,
            "crates/pcx-module-api/src/scope.rs",
        )
        .about(&[("Scope::reach", Code)])
        .update(),
    );
    let s = scope(records).await;
    let view = Index::of(&s).file_view(SCOPE_RS, 2, 10);
    assert_eq!(names(&view.entities), ["Scope.tenant", "Scope::working_in"]);
    let other = Index::of(&s).file_view("crates/pcx-module-api/src/scope.rs", 2, 10);
    assert_eq!(names(&other.entities), ["Scope::reach"]);

    // A record that names both scope.rs files says nothing of whose its
    // entities are.
    let s = scope(vec![
        rec(
            0,
            UpdateType::CodeChanged,
            "crates/pcx-module-core/src/scope.rs, crates/pcx-module-api/src/scope.rs",
        )
        .about(&[("Scope::held_to", Code)])
        .update(),
    ])
    .await;
    let view = Index::of(&s).file_view(SCOPE_RS, 2, 10);
    assert!(view.entities.is_empty());
    assert_eq!(names(&view.changed_with), ["Scope::held_to"]);
}

/// An item of a module named after the file (`veil_core::limiter::Limiter`
/// for limiter.rs) is the file's own.
#[tokio::test]
async fn a_module_path_names_its_file() {
    let s = scope(vec![
        rec(
            0,
            UpdateType::CodeChanged,
            "crates/veil-core/src/limiter.rs (new), crates/veil-core/src/loudness.rs",
        )
        .about(&[("veil_core::limiter::Limiter", Code)])
        .update(),
        rec(
            1,
            UpdateType::DecisionMade,
            "the film's draft goes through the limiter",
        )
        .about(&[
            ("pcx promo film", Concept),
            ("veil_core::limiter::Limiter", Code),
        ])
        .rel(
            "pcx promo film",
            "veil_core::limiter::Limiter",
            RelationType::DependsOn,
        )
        .update(),
    ])
    .await;
    let view = Index::of(&s).file_view("crates/veil-core/src/limiter.rs", 2, 10);
    assert_eq!(names(&view.entities), ["veil_core::limiter::Limiter"]);
    assert_eq!(view.dependents[0].entity, "pcx promo film");
    let loudness = Index::of(&s).file_view("crates/veil-core/src/loudness.rs", 2, 10);
    assert!(loudness.entities.is_empty());
    assert_eq!(
        names(&loudness.changed_with),
        ["veil_core::limiter::Limiter"]
    );
}

/// A problem names its files in its details (`file: …`), with no entity
/// named after them: both files have it.
#[tokio::test]
async fn a_file_named_in_details_is_read_for_any_type() {
    let mut problem = rec(0, UpdateType::ProblemSolved, "the actor dropped a write")
        .about(&[("write path", Concept)])
        .update();
    problem.content.details = vec![
        "file: crates/post-cortex-memory/src/memory_system/storage_actor.rs:718-721, session_manager.rs:107-160"
            .into(),
    ];
    let s = scope(vec![problem.clone()]).await;
    for file in ["storage_actor.rs", "session_manager.rs"] {
        let path = format!("crates/post-cortex-memory/src/memory_system/{file}");
        let view = Index::of(&s).file_view(&path, 2, 10);
        let ids: Vec<Uuid> = view.problems.iter().map(|r| r.id).collect();
        assert_eq!(ids, [problem.id], "{file}");
    }
}

/// A code change's title over the limit keeps every path it lists; an old
/// cut title (no whole `file` kept) names the whole paths before the cut
/// and the paths of its description.
#[test]
fn a_cut_code_change_title_loses_no_path() {
    let paths: Vec<String> = (0..9)
        .map(|i| format!("crates/pcx-web/src/handlers/route_{i}.rs"))
        .collect();
    let mut change = rec(0, UpdateType::CodeChanged, "").update();
    change.content.title = paths.join(", ");
    assert!(change.content.title.len() > 200);
    assert_eq!(files_of(&normalised(change)), paths);

    let mut old = rec(0, UpdateType::CodeChanged, "").update();
    old.content.title = format!("{}...", &paths.join(", ")[..190]);
    old.content.description =
        "crates/pcx-web/src/handlers/route_8.rs (moved), docs/routes.md (listed)".into();
    let mut expected = paths[..4].to_vec();
    expected.extend([paths[8].clone(), "docs/routes.md".to_string()]);
    assert_eq!(files_of(&old), expected);
}

/// Two cards.rs: neither takes the records of the entity "Cards" by its
/// stem, and each keeps its own records; a stem no other file has still
/// matches.
#[tokio::test]
async fn a_shared_file_name_does_not_match_by_stem() {
    let s = scope(vec![
        rec(
            0,
            UpdateType::CodeChanged,
            "crates/rocksdb_storage/src/cards.rs",
        )
        .about(&[("rocks card rows", Code)])
        .update(),
        rec(
            1,
            UpdateType::CodeChanged,
            "crates/transport/src/rest/cards.rs",
        )
        .about(&[("rest card routes", Code)])
        .update(),
        rec(
            2,
            UpdateType::DecisionMade,
            "the Hermes rollout exports cards",
        )
        .about(&[("Cards", Concept)])
        .update(),
        rec(
            3,
            UpdateType::CodeChanged,
            "crates/memory/src/storage_actor.rs",
        )
        .about(&[("actor loop", Code)])
        .update(),
        rec(4, UpdateType::ProblemSolved, "the actor dropped a write")
            .about(&[("StorageActor", Code)])
            .update(),
    ])
    .await;
    let index = Index::of(&s);
    let rocks = index.file_view("crates/rocksdb_storage/src/cards.rs", 2, 10);
    let rest = index.file_view("crates/transport/src/rest/cards.rs", 2, 10);
    assert!(rocks.entities.is_empty() && rest.entities.is_empty());
    assert_eq!((rocks.records.len(), rest.records.len()), (1, 1));
    assert_ne!(rocks.records[0].id, rest.records[0].id);
    assert!(rocks.decisions.is_empty());

    let actor = index.file_view("crates/memory/src/storage_actor.rs", 2, 10);
    let names: Vec<&str> = actor.entities.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["StorageActor"]);
    assert_eq!(actor.problems.len(), 1);
}
