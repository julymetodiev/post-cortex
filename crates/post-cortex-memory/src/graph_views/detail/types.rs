// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the detail views return: records in full, nodes and edges with the
//! provenance of the records behind them, and one type per view.

use chrono::{DateTime, Utc};
use post_cortex_core::core::context_update::{CodeReference, EntityType, UpdateType};
use post_cortex_core::core::dismissal::RadarDismissal;
use post_cortex_core::graph::temporal::{EndReason, Validity};
use uuid::Uuid;

use crate::graph_views::types::{AliasCandidate, GraphEdge, GraphNode, ImpactItem, RelationRef};

/// Who wrote the records a node or edge rests on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Provenance {
    /// Distinct authors, sorted.
    pub authors: Vec<String>,
    /// Distinct sessions, sorted.
    pub sessions: Vec<Uuid>,
}

/// A node with the provenance of the records that mention it.
#[derive(Clone, Debug)]
pub struct NodeDetail {
    /// The node.
    pub node: GraphNode,
    /// Of the records that mention it.
    pub provenance: Provenance,
}

/// An edge with the provenance of the records asserting it.
#[derive(Clone, Debug)]
pub struct EdgeDetail {
    /// The edge.
    pub edge: GraphEdge,
    /// Of the records asserting it.
    pub provenance: Provenance,
}

/// An entity a record names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityRef {
    /// Canonical name.
    pub name: String,
    /// `None` when the entity is not in the scope's graph.
    pub entity_type: Option<EntityType>,
}

/// A relation as a record asserts it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordRelation {
    /// The relation (canonical names).
    pub relation: RelationRef,
    /// Why it exists, as the record says.
    pub context: String,
}

/// One record in full, with its validity and provenance.
#[derive(Clone, Debug)]
pub struct RecordDetail {
    /// Entry id.
    pub id: Uuid,
    /// Its session.
    pub session_id: Uuid,
    /// The session's display name (empty when unnamed).
    pub session_name: String,
    /// Decision, problem solved, code change, ...
    pub update_type: UpdateType,
    /// Title.
    pub title: String,
    /// Description, whole.
    pub description: String,
    /// Details.
    pub details: Vec<String>,
    /// Examples.
    pub examples: Vec<String>,
    /// Implications.
    pub implications: Vec<String>,
    /// Who wrote it, when known.
    pub author: Option<String>,
    /// When it was written.
    pub created_at: DateTime<Utc>,
    /// When it held.
    pub validity: Validity,
    /// The writer's confidence in [0, 1], when stated.
    pub confidence: Option<f32>,
    /// Entities it names, in the order it names them.
    pub entities: Vec<EntityRef>,
    /// Relations it asserts.
    pub relations: Vec<RecordRelation>,
    /// The code it points at.
    pub code: Option<CodeReference>,
    /// Captured messages it was drawn from (`message:<seq>`).
    pub derived_from: Vec<String>,
    /// Records the writer said it replaces.
    pub supersedes: Vec<Uuid>,
    /// Records whose validity it ended, oldest first.
    pub ended: Vec<Uuid>,
    /// Marked important by the user.
    pub important: bool,
}

/// The graph as nodes and edges, the most important nodes first.
#[derive(Clone, Debug)]
pub struct GraphDetail {
    /// Most important first.
    pub nodes: Vec<NodeDetail>,
    /// Edges between the nodes returned, sorted.
    pub edges: Vec<EdgeDetail>,
    /// Entities in the scope (as of `at`).
    pub total_nodes: usize,
    /// Relations in the scope (as of `at`).
    pub total_edges: usize,
    /// The time the graph is shown as of, if one was asked for.
    pub at: Option<DateTime<Utc>>,
}

/// What a timeline event is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventKind {
    /// A record stopped being valid (listed first among events at one
    /// instant: endings before beginnings).
    Ended,
    /// The last record asserting a relation stopped being valid.
    RelationEnded,
    /// A record was written.
    Recorded,
    /// A relation was first asserted.
    RelationAdded,
}

/// One event of a timeline.
#[derive(Clone, Debug)]
pub struct TimelineEvent {
    /// When.
    pub at: DateTime<Utc>,
    /// What.
    pub kind: EventKind,
    /// The record written or ended, or the one asserting the relation.
    pub record: Uuid,
    /// Its session.
    pub session_id: Uuid,
    /// Its title.
    pub title: String,
    /// Its type.
    pub update_type: UpdateType,
    /// For relation events.
    pub relation: Option<RelationRef>,
    /// For endings: why.
    pub reason: Option<EndReason>,
}

/// Events in a window, in time order.
#[derive(Clone, Debug)]
pub struct Timeline {
    /// The latest events of the window, in time order.
    pub events: Vec<TimelineEvent>,
    /// Events in the window before the limit.
    pub total: usize,
}

/// A file and the records pointing at it.
#[derive(Clone, Debug, PartialEq)]
pub struct FileTouch {
    /// As the records name it.
    pub path: String,
    /// Records pointing at it.
    pub records: usize,
    /// Type of the latest of them.
    pub last_update_type: UpdateType,
    /// When the latest was written.
    pub last_at: DateTime<Utc>,
}

/// A page of the files a scope's records name, most recently touched first
/// (then by path, descending).
#[derive(Clone, Debug)]
pub struct Files {
    /// At most the limit.
    pub files: Vec<FileTouch>,
    /// Files over all pages.
    pub total: usize,
    /// Where the next page starts; `None` on the last.
    pub next: Option<PageCursor<String>>,
}

/// What depends on an entity, with the records at risk.
#[derive(Clone, Debug)]
pub struct ImpactDetail {
    /// Canonical name.
    pub entity: String,
    /// Hops followed.
    pub depth: usize,
    /// Dependents, nearest first.
    pub dependents: Vec<ImpactItem>,
    /// Current decisions, requirements and problems about it and its
    /// dependents, most relevant first (as `ImpactView::risks`).
    pub risks: Vec<RiskDetail>,
    /// All such records.
    pub total_risks: usize,
}

/// A record at risk, with what it is about.
#[derive(Clone, Debug)]
pub struct RiskDetail {
    /// The record.
    pub record: RecordDetail,
    /// The nearest entity of the report it names.
    pub about: String,
    /// Hops of `about` from the entity asked for (0: that entity).
    pub depth: usize,
}

/// A record on an entity's causal chain.
#[derive(Clone, Debug)]
pub struct LineageStepDetail {
    /// The record.
    pub record: RecordDetail,
    /// The causal relation it asserts; `None` for decisions about the entity.
    pub relation: Option<RelationRef>,
}

/// An entity's causal chain.
#[derive(Clone, Debug)]
pub struct LineageDetail {
    /// Canonical name.
    pub entity: String,
    /// Entities on the chain, the entity first.
    pub chain: Vec<String>,
    /// In time order.
    pub steps: Vec<LineageStepDetail>,
}

/// An entity with everything the scope says about it.
#[derive(Clone, Debug)]
pub struct EntityDetail {
    /// The entity.
    pub entity: NodeDetail,
    /// Place by importance (1 = most important) among `ranked` entities.
    pub rank: usize,
    /// Entities ranked.
    pub ranked: usize,
    /// Entities in its topic (0 when it has none).
    pub topic_size: usize,
    /// In and out, sorted.
    pub relations: Vec<EdgeDetail>,
    /// Records mentioning it, newest first, at most the limit.
    pub records: Vec<RecordDetail>,
    /// Records mentioning it.
    pub total_records: usize,
    /// Its decisions, current first.
    pub decisions: Vec<RecordDetail>,
    /// What depends on it.
    pub impact: ImpactDetail,
    /// Its causal chain.
    pub lineage: LineageDetail,
    /// Files of the records mentioning it, most recent first.
    pub files: Vec<FileTouch>,
}

/// A topic community.
#[derive(Clone, Debug)]
pub struct TopicDetail {
    /// Community id.
    pub id: usize,
    /// Entities in it.
    pub size: usize,
    /// Most important first.
    pub central: Vec<NodeDetail>,
    /// Current decisions naming most of its entities.
    pub key_decisions: Vec<RecordDetail>,
    /// Records mentioning it.
    pub records: usize,
    /// First record mentioning it.
    pub first: Option<DateTime<Utc>>,
    /// Latest record mentioning it.
    pub last: Option<DateTime<Utc>>,
}

/// The topics of a scope.
#[derive(Clone, Debug)]
pub struct TopicsDetail {
    /// At most the limit.
    pub topics: Vec<TopicDetail>,
    /// Entities in no topic.
    pub unclustered: usize,
}

/// What needs attention.
#[derive(Clone, Debug)]
pub struct RadarDetail {
    /// Problems no record solves.
    pub unsolved_problems: Vec<NodeDetail>,
    /// Current decisions without a rationale.
    pub decisions_without_rationale: Vec<RecordDetail>,
    /// Questions still open.
    pub open_questions: Vec<String>,
    /// Entities untouched for 90 days.
    pub stale_entities: Vec<NodeDetail>,
    /// Names that may be aliases of other entities.
    pub alias_candidates: Vec<AliasCandidate>,
    /// The dismissals that matched an item.
    pub dismissed: Vec<RadarDismissal>,
}

/// What the records say about a source file.
#[derive(Clone, Debug)]
pub struct FileDetail {
    /// The path asked for.
    pub path: String,
    /// Newest first.
    pub records: Vec<RecordDetail>,
    /// Problems about it.
    pub problems: Vec<RecordDetail>,
    /// Current decisions and requirements about it.
    pub decisions: Vec<RecordDetail>,
    /// Entities named after it.
    pub entities: Vec<NodeDetail>,
    /// What depends on those entities.
    pub dependents: Vec<ImpactItem>,
    /// The code its records changed with it.
    pub changed_with: Vec<NodeDetail>,
}

/// Which records a page of the browser holds.
#[derive(Clone, Debug, Default)]
pub struct RecordFilter {
    /// Only these types (empty: all).
    pub types: Vec<UpdateType>,
    /// Only by this author.
    pub author: Option<String>,
    /// Only those mentioning this entity (name or alias).
    pub entity: Option<String>,
    /// Case-insensitive text in the title, description or details.
    pub text: String,
    /// Current, ended or both.
    pub validity: ValidityFilter,
    /// Written at or after this.
    pub from: Option<DateTime<Utc>>,
    /// Written at or before this.
    pub to: Option<DateTime<Utc>>,
    /// Only this session of the scope.
    pub session: Option<Uuid>,
}

/// Records by whether they still hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ValidityFilter {
    /// Both.
    #[default]
    Any,
    /// Only records that still hold.
    Current,
    /// Only records that ended.
    Ended,
}

/// Where the next page of a newest-first list starts: after the item at
/// `at` with `key`. The list is ordered by both, descending, so a page
/// never repeats or skips an item when newer ones arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PageCursor<K> {
    /// The item's time.
    pub at: DateTime<Utc>,
    /// What tells items of the same time apart (a record, a path, a
    /// session).
    pub key: K,
}

/// A page of the browser.
#[derive(Clone, Debug)]
pub struct RecordPage {
    /// Newest first.
    pub records: Vec<RecordDetail>,
    /// Records matching the filter over all pages.
    pub total: usize,
    /// Where the next page starts; `None` on the last.
    pub next: Option<PageCursor<Uuid>>,
}

/// Records of one author.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorCount {
    /// The author.
    pub author: String,
    /// Their records.
    pub records: usize,
}

/// One session of a scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionStat {
    /// The session.
    pub session_id: Uuid,
    /// Its display name (empty when unnamed).
    pub name: String,
    /// Its records.
    pub records: usize,
    /// Its latest record.
    pub last_record: Option<DateTime<Utc>>,
}

/// How many items each radar list has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RadarCounts {
    /// Problems no record solves.
    pub unsolved_problems: usize,
    /// Current decisions without a rationale.
    pub decisions_without_rationale: usize,
    /// Questions still open.
    pub open_questions: usize,
    /// Entities untouched for 90 days.
    pub stale_entities: usize,
    /// Possible aliases.
    pub alias_candidates: usize,
}

/// The numbers and lists of a scope's overview.
#[derive(Clone, Debug)]
pub struct Overview {
    /// Records.
    pub records: usize,
    /// Written in the last 24 hours.
    pub records_day: usize,
    /// Written in the last 7 days.
    pub records_week: usize,
    /// Records that still hold.
    pub current_records: usize,
    /// Entities.
    pub entities: usize,
    /// Relations.
    pub relations: usize,
    /// Topic communities.
    pub topics: usize,
    /// Captured messages over every session of the scope.
    pub messages: u64,
    /// Sessions.
    pub sessions: usize,
    /// Sessions with a record in the last 24 hours.
    pub active_sessions: usize,
    /// Authors by records, most first.
    pub authors: Vec<AuthorCount>,
    /// First record.
    pub first_activity: Option<DateTime<Utc>>,
    /// Latest record or message.
    pub last_activity: Option<DateTime<Utc>>,
    /// The most important entities.
    pub top_entities: Vec<NodeDetail>,
    /// Current decisions, newest first.
    pub recent_decisions: Vec<RecordDetail>,
    /// Newest records.
    pub recent_records: Vec<RecordDetail>,
    /// How many items each radar list has.
    pub radar: RadarCounts,
    /// Records waiting for an embedding.
    pub embedding_queue: usize,
    /// Sessions, most recently written first.
    pub session_stats: Vec<SessionStat>,
}
