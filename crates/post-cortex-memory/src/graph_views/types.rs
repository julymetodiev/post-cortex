// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the views return: nodes, edges, records and one type per view.

use chrono::{DateTime, Utc};
use post_cortex_core::core::context_update::RelationType;
use post_cortex_core::core::dismissal::RadarDismissal;
use post_cortex_core::graph::temporal::Validity;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An entity as views show it.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GraphNode {
    /// Canonical name.
    pub name: String,
    /// `technology`, `concept`, `problem`, `solution`, `decision` or
    /// `code_component`.
    pub entity_type: String,
    /// Other names it was written under.
    pub aliases: Vec<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Centrality-based importance in (0, 1], see [`post_cortex_core::graph::analytics::importance`].
    pub importance: f32,
    /// How often it was mentioned.
    pub mention_count: u32,
    /// Records of the scope that mention it.
    pub records: usize,
    /// First mention.
    pub first_seen: DateTime<Utc>,
    /// Latest mention.
    pub last_seen: DateTime<Utc>,
    /// Topic community, if it shares one with other entities.
    pub community: Option<usize>,
}

/// A relation as views show it.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GraphEdge {
    /// Source entity.
    pub from: String,
    /// Target entity.
    pub to: String,
    /// `depends_on`, `solves`, ...
    pub relation_type: String,
    /// Why it exists (latest context written).
    pub context: String,
    /// Strength of the relation type, see [`RelationType::weight`].
    pub weight: f32,
    /// When the first record asserting it became valid (none if no record
    /// of the scope asserts it).
    pub valid_from: Option<DateTime<Utc>>,
    /// When the last record asserting it stopped being valid; `None` while
    /// one still holds.
    pub valid_to: Option<DateTime<Utc>>,
    /// Records asserting it, oldest first.
    pub records: Vec<Uuid>,
}

/// A record as views show it.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RecordView {
    /// Entry id.
    pub id: Uuid,
    /// Session it belongs to.
    pub session_id: Uuid,
    /// `decision_made`, `problem_solved`, ...
    pub interaction_type: String,
    /// Title.
    pub title: String,
    /// Description, cut to a few hundred characters.
    pub description: String,
    /// Who wrote it.
    pub author: Option<String>,
    /// When it was written (known time).
    pub timestamp: DateTime<Utc>,
    /// How sure the writer was.
    pub confidence: Option<f32>,
    /// Captured messages it was drawn from.
    pub derived_from: Vec<String>,
    /// File of its code reference.
    pub file: Option<String>,
    /// When it held.
    pub validity: Validity,
}

/// The `entity` view.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EntityView {
    /// The entity.
    pub entity: GraphNode,
    /// Its relations.
    pub relations: Vec<GraphEdge>,
    /// Records that mention it, newest first (at most the limit).
    pub records: Vec<RecordView>,
    /// Decisions that mention it, still valid ones first.
    pub decisions: Vec<RecordView>,
    /// All records that mention it.
    pub total_records: usize,
}

/// An entity reached by impact analysis.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImpactItem {
    /// The dependent entity.
    pub entity: String,
    /// Its type.
    pub entity_type: Option<String>,
    /// The entity it depends on (one hop closer to the changed one).
    pub via: String,
    /// The relation found.
    pub relation_type: String,
    /// The relation's context.
    pub context: String,
    /// Hops from the changed entity.
    pub depth: usize,
}

/// The `impact` view: who depends on an entity, and what
/// the records say about them (a pre-change risk report).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImpactView {
    /// The entity that changes.
    pub entity: String,
    /// Everything that depends on it, nearest first.
    pub dependents: Vec<ImpactItem>,
    /// Valid decisions, requirements and problems about the entity or its
    /// dependents, newest first.
    pub risks: Vec<RecordView>,
}

/// One topic: a community of entities with what the records say about it.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TopicView {
    /// Community index.
    pub id: usize,
    /// Entities in it.
    pub size: usize,
    /// Its most central entities.
    pub central: Vec<GraphNode>,
    /// Valid decisions mentioning its entities, by how many they mention.
    pub key_decisions: Vec<RecordView>,
    /// Records mentioning its entities.
    pub records: usize,
    /// First and latest of those records.
    pub first: Option<DateTime<Utc>>,
    /// Latest of those records.
    pub last: Option<DateTime<Utc>>,
}

/// The `topics` view: an extractive summary of the scope, by topic.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TopicsView {
    /// Topics, largest first.
    pub topics: Vec<TopicView>,
    /// Entities in no topic.
    pub unclustered: usize,
}

/// One step of a decision lineage.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LineageStep {
    /// The record.
    pub record: RecordView,
    /// The causal relation it asserted, if that is how it joined.
    pub relation: Option<(String, String, String)>,
}

/// The `lineage` view: problem -> decision -> consequences
/// around an entity, in time order.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LineageView {
    /// The entity asked about.
    pub entity: String,
    /// Entities on the causal chain.
    pub chain: Vec<String>,
    /// Records in time order.
    pub steps: Vec<LineageStep>,
}

/// The `file` view (`GET /api/file`): what the records say about a source file, for a hook
/// before it is edited.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileView {
    /// The path asked about.
    pub path: String,
    /// Records with a code reference to the file or about an entity naming
    /// it, newest first.
    pub records: Vec<RecordView>,
    /// Problems solved there: "this broke before, here is why".
    pub problems: Vec<RecordView>,
    /// Valid decisions and requirements about it.
    pub decisions: Vec<RecordView>,
    /// Entities naming the file (its path, file name or stem).
    pub entities: Vec<GraphNode>,
    /// What depends on those entities (impact analysis).
    pub dependents: Vec<ImpactItem>,
}

/// A pair of entities that may be one.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AliasCandidate {
    /// Shorter name.
    pub name: String,
    /// Longer name it is a prefix of.
    pub of: String,
    /// Records mentioning both.
    pub shared_records: usize,
}

/// The `radar` view: what needs attention.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RadarView {
    /// Problem entities nothing `solves`.
    pub unsolved_problems: Vec<GraphNode>,
    /// Valid decisions recorded without a rationale.
    pub decisions_without_rationale: Vec<RecordView>,
    /// Questions still open in the sessions' structured context.
    pub open_questions: Vec<String>,
    /// Entities untouched for months, most important first.
    pub stale_entities: Vec<GraphNode>,
    /// Entities that may be one (merge with `manage_entity` `merge`).
    pub alias_candidates: Vec<AliasCandidate>,
    /// Dismissals of the scope's workspace(s) that match items on the
    /// radar. Those items are left out of the lists above unless the view
    /// was asked to include dismissed items.
    #[serde(default)]
    pub dismissed: Vec<RadarDismissal>,
}

/// A relation between two entities, by canonical names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationRef {
    /// Source entity.
    pub from: String,
    /// Target entity.
    pub to: String,
    /// Its type.
    pub relation_type: RelationType,
}

/// A record on an entity's causal chain: the causal relation it asserts,
/// or `None` for a decision or solved problem about the entity itself.
#[derive(Clone, Debug)]
pub struct LineageLink {
    /// The record.
    pub record: Uuid,
    /// The relation it asserts.
    pub relation: Option<RelationRef>,
}

/// An entity's causal chain, computed once and rendered by
/// [`crate::graph_views::Index::lineage_view`] (for agents) and the detail
/// view (in full).
#[derive(Clone, Debug)]
pub struct LineageLinks {
    /// Canonical name.
    pub entity: String,
    /// Entities on the chain, the entity first.
    pub chain: Vec<String>,
    /// In time order (then by record id).
    pub links: Vec<LineageLink>,
}
