// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Port conformance suites (feature `conformance`): one generic case per
//! port behaviour, run against every adapter from the adapter crate's
//! tests with [`conformance_suite!`]. A case is a `pub async fn(&Stores)`
//! in the module of its port and reaches the adapter only through the
//! ports it names.
//!
//! Adapter-specific behaviour (on-disk layouts, migrations, caches) is
//! tested next to the adapter.

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use uuid::Uuid;

use crate::storage::{Stores, VectorStore};

use post_cortex_core::core::context_update::{
    ContextUpdate, EntityRelationship, EntityType, RelationType, TypedEntity, UpdateContent,
    UpdateMeta, UpdateType,
};

pub mod blobs;
pub mod cards;
pub mod checkpoints;
pub mod dismissals;
pub mod documents;
pub mod events;
pub mod freshness;
pub mod graph;
pub mod index;
pub mod message_stats;
pub mod messages;
pub mod rerank;
pub mod sessions;
pub mod vectors;
pub mod workspaces;

/// One `#[tokio::test]` per case of every port suite, run against the
/// adapter that `$open` (an async expression with `crate::` paths,
/// evaluating to `(Adapter, keep_alive)`) opens afresh for each case. The
/// adapter crate invokes it inside a module named after the adapter, e.g.
/// `mod rocksdb { post_cortex_ports::conformance_suite!(crate::conformance::rocksdb()); }`,
/// giving `rocksdb::<port>::<case>`. The adapter crate needs `tokio` with
/// the `macros` and multi-thread runtime features as a dev-dependency.
#[macro_export]
macro_rules! conformance_suite {
    ($open:expr) => {
        $crate::conformance_cases!(
            $open;
            sessions: round_trip_keeps_the_session, summary_reports_the_stored_session, replace_replaces_the_session;
            checkpoints: checkpoint_round_trip, deleting_a_session_deletes_its_checkpoints;
            messages: append_assigns_positions_and_skips_known_turns, retention_deletes_old_messages_and_keeps_positions, deleting_the_session_deletes_its_stream, stats_and_pages_read_the_stream_and_skip_expired_heads, messages_are_deleted_without_reusing_positions, an_agents_turn_keeps_its_parent_and_blocks;
            message_stats: stats_of_many_streams_keep_their_authors;
            cards: versions_and_conflicts, one_of_two_racing_writers_wins, deleting_the_workspace_deletes_its_card;
            documents: versions_and_conflicts, another_kind_is_a_conflict, one_of_two_racing_writers_wins, latest_documents_by_workspace_and_kind, delete_removes_every_version, deleting_the_workspace_deletes_its_documents, restore_keeps_the_versions, pages_by_kind_and_key_newest_first, unkeyed_documents_are_listed_until_keyed;
            dismissals: dismissals_are_per_workspace;
            events: positions_are_ordered_by_name_and_never_reused, racing_appends_get_distinct_positions, old_events_are_dropped, cursors_only_move_forward;
            graph: a_record_save_writes_what_the_records_name, a_whole_graph_save_replaces_the_stored_graph, a_save_drops_what_the_snapshot_lost, vectorized_ids_follow_the_changes;
            freshness: every_scope_round_trips, invalidated_references_are_stale_until_registered_again, cascade_marks_stale, invalidation_is_scoped_to_the_session, batch_check_matches_single_checks;
            vectors: upsert_keeps_one_vector_per_content, batched_delete_is_scoped_to_the_session;
            workspaces: saves_keep_the_creation_time, save_adds_members, membership_needs_the_workspace, delete_racing_updates_leaves_nothing, updates_keep_the_rest_of_the_row;
        );
    };
}

/// The expansion of [`conformance_suite!`]: one module per port, one test
/// per case.
#[doc(hidden)]
#[macro_export]
macro_rules! conformance_cases {
    ($open:expr; $($port:ident: $($case:ident),+;)+) => {
        $(
            mod $port {
                $(
                    #[tokio::test(flavor = "multi_thread")]
                    async fn $case() {
                        let (stores, _keep) = $open.await;
                        $crate::conformance::$port::$case(&stores).await;
                    }
                )+
            }
        )+
    };
}

/// One storage adapter under test: its capability ports, and the
/// persisted embeddings it hands a `SearchIndex` adapter. Derefs to the [`Stores`], which most cases take.
#[derive(Clone)]
pub struct Adapter {
    /// The capability ports.
    pub stores: Stores,
    /// The persisted embeddings.
    pub vectors: Arc<dyn VectorStore>,
}

impl Adapter {
    /// Every port of `backend`.
    pub fn of<B>(backend: Arc<B>) -> Self
    where
        B: crate::storage::SessionStore
            + crate::storage::CheckpointStore
            + crate::storage::WorkspaceStore
            + VectorStore
            + crate::storage::MessageStore
            + crate::storage::CardStore
            + crate::storage::DocumentStore
            + crate::storage::DismissalStore
            + crate::storage::FreshnessStore
            + crate::storage::EventLog
            + crate::storage::EventCursors
            + 'static,
    {
        Self {
            stores: Stores::of(Arc::clone(&backend)),
            vectors: backend,
        }
    }
}

impl std::ops::Deref for Adapter {
    type Target = Stores;

    fn deref(&self) -> &Stores {
        &self.stores
    }
}

/// A fixed instant, whole seconds after 2026-10-01 12:00 UTC.
pub fn at(seconds: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap() + chrono::Duration::seconds(seconds)
}

/// A decision record at `at(seconds)` naming `entities`; the first two get
/// a `DependsOn` edge.
pub fn record(seconds: i64, title: &str, entities: &[&str]) -> ContextUpdate {
    ContextUpdate {
        id: Uuid::new_v4(),
        timestamp: at(seconds),
        update_type: UpdateType::DecisionMade,
        content: UpdateContent {
            title: title.to_string(),
            description: format!("about {title}"),
            details: vec!["detail".into()],
            examples: vec![],
            implications: vec![],
        },
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: entities.iter().map(|e| e.to_string()).collect(),
        creates_relationships: match entities {
            [a, b, ..] => vec![EntityRelationship {
                from_entity: a.to_string(),
                to_entity: b.to_string(),
                relation_type: RelationType::DependsOn,
                context: "uses".into(),
            }],
            _ => vec![],
        },
        references_entities: vec![],
        typed_entities: entities
            .iter()
            .map(|e| TypedEntity {
                name: e.to_string(),
                entity_type: EntityType::Technology,
            })
            .collect(),
        author: Some("claude-code".into()),
        derived_from: Vec::new(),
        meta: UpdateMeta::default(),
    }
}
