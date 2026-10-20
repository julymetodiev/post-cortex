// Copyright (c) 2026 Julius ML
// MIT License

//! Read-only views over a scope's knowledge graph and records: the data a
//! UI or an agent needs to show what post-cortex knows (ROADMAP Phase 4,
//! and the views of Phase 3 items 4-6 and 12).
//!
//! Every transport renders the same views: REST (`/api/graph`,
//! `/api/entities/{name}/...`, `/api/timeline`, ...), the MCP
//! `query_conversation_context` tool and the gRPC `GetGraphView` RPC.
//!
//! A [`GraphScope`] is one session, or the sessions of a workspace with
//! their merged graph. Views read all of the scope's records, not only the
//! hot tier, and derive everything else from them: which records mention
//! an entity, which records assert an edge, and when each record and edge
//! held ([`temporal`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use post_cortex_core::core::context_update::{
    ContextUpdate, EntityData, EntityType, RelationType, UpdateType,
};
use post_cortex_core::graph::analytics;
use post_cortex_core::graph::entity_graph::{SimpleEntityGraph, entity_key};
use post_cortex_core::graph::temporal::{self, Validity};
use post_cortex_core::session::active_session::ActiveSession;

use crate::context_assembly::analyze_impact;

/// Characters of a record's description a view carries.
const DESCRIPTION_CHARS: usize = 400;
/// Entities untouched this long are reported as stale by the radar.
const STALE_AFTER_DAYS: i64 = 90;

/// The sessions a view reads and the graph over them.
pub struct GraphScope {
    /// Sessions in the scope (one, or a workspace's members).
    pub sessions: Vec<Arc<ActiveSession>>,
    /// The session's graph, or the members' graphs merged.
    pub graph: Arc<SimpleEntityGraph>,
}

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
    /// Centrality-based importance in (0, 1], see [`analytics::importance`].
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

/// `GET /api/graph`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GraphView {
    /// The most important entities, at most the limit.
    pub nodes: Vec<GraphNode>,
    /// Relations between those entities.
    pub edges: Vec<GraphEdge>,
    /// Entities in the scope (at `at`, if given).
    pub total_nodes: usize,
    /// Relations in the scope (at `at`, if given).
    pub total_edges: usize,
    /// The point in time shown, if not now.
    pub at: Option<DateTime<Utc>>,
}

/// `GET /api/entities/{name}/context`.
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

/// `GET /api/entities/{name}/impact`: who depends on an entity, and what
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

/// What happened at one point of the timeline.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TimelineEventKind {
    /// A record was written.
    Recorded,
    /// A record stopped being valid.
    Ended,
    /// A relation was first asserted.
    RelationAdded,
    /// The last record asserting a relation stopped being valid.
    RelationEnded,
}

/// One timeline entry.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TimelineEvent {
    /// When.
    pub at: DateTime<Utc>,
    /// What.
    pub kind: TimelineEventKind,
    /// The record written or ended, or the one asserting the relation.
    pub record: Uuid,
    /// The record's session.
    pub session_id: Uuid,
    /// The record's title.
    pub title: String,
    /// The record's kind.
    pub interaction_type: String,
    /// The relation, for relation events.
    pub relation: Option<(String, String, String)>,
    /// For `ended`: why.
    pub reason: Option<temporal::EndReason>,
}

/// `GET /api/timeline`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TimelineView {
    /// Events in time order (at most the limit, the latest ones).
    pub events: Vec<TimelineEvent>,
    /// Events in the requested window.
    pub total: usize,
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

/// `GET /api/topics`: an extractive summary of the scope, by topic.
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

/// `GET /api/entities/{name}/lineage`: problem -> decision -> consequences
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

/// `GET /api/file`: what the records say about a source file, for a hook
/// before it is edited (ROADMAP Phase 3 item 4).
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

/// `GET /api/radar`: what needs attention.
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
}

/// What a scope's records say, indexed once per view.
struct Index<'a> {
    graph: &'a SimpleEntityGraph,
    records: HashMap<Uuid, (Uuid, &'a ContextUpdate)>,
    validity: HashMap<Uuid, Validity>,
    /// (from, to, relation) -> records asserting it, oldest first.
    edge_records: HashMap<(String, String, RelationType), Vec<Uuid>>,
    importance: BTreeMap<String, f32>,
    communities: BTreeMap<String, usize>,
}

impl GraphScope {
    fn index(&self) -> Index<'_> {
        let graph: &SimpleEntityGraph = &self.graph;
        let mut records = HashMap::new();
        for session in &self.sessions {
            for update in session.incremental_updates.iter() {
                records.insert(update.id, (session.id(), &**update));
            }
        }
        let validity = temporal::validity(records.values().map(|(_, u)| *u), graph);
        let mut ordered: Vec<&ContextUpdate> = records.values().map(|(_, u)| *u).collect();
        ordered.sort_by_key(|u| (u.timestamp, u.id));
        let mut edge_records: HashMap<_, Vec<Uuid>> = HashMap::new();
        for update in ordered {
            for rel in &update.creates_relationships {
                let resolve = |n: &str| graph.resolve(n).unwrap_or(n).to_string();
                let key = (
                    resolve(&rel.from_entity),
                    resolve(&rel.to_entity),
                    rel.relation_type.clone(),
                );
                let ids = edge_records.entry(key).or_default();
                if ids.last() != Some(&update.id) {
                    ids.push(update.id);
                }
            }
        }
        Index {
            graph,
            records,
            validity,
            edge_records,
            importance: analytics::importance(graph),
            communities: analytics::communities(graph),
        }
    }

    /// The graph's most important entities (at most `limit`) and the
    /// relations between them. With `at`, as it stood then: entities first
    /// seen by then, relations asserted by then.
    pub fn graph_view(&self, limit: usize, at: Option<DateTime<Utc>>) -> GraphView {
        let index = self.index();
        let graph = index.graph;
        let mut nodes: Vec<GraphNode> = graph
            .entities
            .values()
            .filter(|e| at.is_none_or(|t| e.first_mentioned <= t))
            .map(|e| index.node(e))
            .collect();
        let total_nodes = nodes.len();
        sort_by_importance(&mut nodes);
        nodes.truncate(limit);
        let shown: HashSet<&str> = nodes.iter().map(|n| n.name.as_str()).collect();

        let mut total_edges = 0;
        let mut edges = Vec::new();
        for rel in graph.relationships() {
            let edge = index.edge(rel.from, rel.to, rel.relation_type, rel.context);
            if let Some(t) = at
                && edge.valid_from.is_some_and(|from| from > t)
            {
                continue;
            }
            total_edges += 1;
            if shown.contains(rel.from) && shown.contains(rel.to) {
                edges.push(edge);
            }
        }
        sort_edges(&mut edges);
        GraphView {
            nodes,
            edges,
            total_nodes,
            total_edges,
            at,
        }
    }

    /// One entity (by name or alias): its relations, the records that
    /// mention it (newest first, at most `limit`) and its decisions.
    pub fn entity_view(&self, name: &str, limit: usize) -> Option<EntityView> {
        let index = self.index();
        let entity = index.entity(name)?;
        let relations = index.relations_of(&entity.name);
        let mut records = index.mentioning(&entity.name);
        records.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        let total_records = records.len();
        let mut decisions: Vec<RecordView> = records
            .iter()
            .filter(|r| r.interaction_type == UpdateType::DecisionMade.as_str())
            .cloned()
            .collect();
        decisions.sort_by_key(|r| !r.validity.is_current());
        decisions.truncate(limit);
        records.truncate(limit);
        Some(EntityView {
            entity: index.node(entity),
            relations,
            records,
            decisions,
            total_records,
        })
    }

    /// Everything that depends on `name`, up to `depth` hops (impact
    /// analysis of `assemble_context`, repeated), and the valid decisions,
    /// requirements and problems about it and its dependents.
    pub fn impact_view(&self, name: &str, depth: usize, limit: usize) -> Option<ImpactView> {
        let index = self.index();
        let entity = index.entity(name)?.name.clone();
        let mut seen: HashSet<String> = HashSet::from([entity.clone()]);
        let mut frontier = vec![entity.clone()];
        let mut dependents = Vec::new();
        for hop in 1..=depth.max(1) {
            let mut next = Vec::new();
            for impact in analyze_impact(&frontier, index.graph) {
                if seen.insert(impact.entity.clone()) {
                    next.push(impact.entity.clone());
                    dependents.push(ImpactItem {
                        entity_type: index
                            .graph
                            .entities
                            .get(&impact.entity)
                            .map(|e| e.entity_type.as_str().to_string()),
                        entity: impact.entity,
                        via: impact.depends_on,
                        relation_type: impact.relation_type.as_str().to_string(),
                        context: impact.context,
                        depth: hop,
                    });
                }
            }
            if next.is_empty() {
                break;
            }
            next.sort();
            frontier = next;
        }

        let risky = [
            UpdateType::DecisionMade.as_str(),
            UpdateType::RequirementAdded.as_str(),
            UpdateType::ProblemSolved.as_str(),
        ];
        let mut risks: Vec<RecordView> = Vec::new();
        let mut listed = HashSet::new();
        for name in std::iter::once(&entity).chain(dependents.iter().map(|d| &d.entity)) {
            for record in index.mentioning(name) {
                if record.validity.is_current()
                    && risky.contains(&record.interaction_type.as_str())
                    && listed.insert(record.id)
                {
                    risks.push(record);
                }
            }
        }
        risks.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        risks.truncate(limit);
        Some(ImpactView {
            entity,
            dependents,
            risks,
        })
    }

    /// Records written and ended, and relations added and ended, between
    /// `from` and `to` (inclusive), in time order; the latest `limit`.
    pub fn timeline(
        &self,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
        limit: usize,
    ) -> TimelineView {
        let index = self.index();
        let mut events = Vec::new();
        for (id, (session_id, update)) in &index.records {
            let v = &index.validity[id];
            let event = |at, kind, reason| TimelineEvent {
                at,
                kind,
                record: *id,
                session_id: *session_id,
                title: update.content.title.clone(),
                interaction_type: update.update_type.as_str().to_string(),
                relation: None,
                reason,
            };
            events.push(event(update.timestamp, TimelineEventKind::Recorded, None));
            if let Some(end) = v.valid_to {
                events.push(event(end, TimelineEventKind::Ended, v.reason));
            }
        }
        for ((from_e, to_e, rel), ids) in &index.edge_records {
            let relation = Some((from_e.clone(), to_e.clone(), rel.as_str().to_string()));
            let (start, end) = index.edge_validity(ids);
            let first = ids[0];
            let (session_id, update) = index.records[&first];
            if let Some(start) = start {
                events.push(TimelineEvent {
                    at: start,
                    kind: TimelineEventKind::RelationAdded,
                    record: first,
                    session_id,
                    title: update.content.title.clone(),
                    interaction_type: update.update_type.as_str().to_string(),
                    relation: relation.clone(),
                    reason: None,
                });
            }
            if let Some(end) = end {
                let last = *ids.last().unwrap_or(&first);
                let (session_id, update) = index.records[&last];
                events.push(TimelineEvent {
                    at: end,
                    kind: TimelineEventKind::RelationEnded,
                    record: last,
                    session_id,
                    title: update.content.title.clone(),
                    interaction_type: update.update_type.as_str().to_string(),
                    relation,
                    reason: index.validity[&last].reason,
                });
            }
        }
        events.retain(|e| from.is_none_or(|f| e.at >= f) && to.is_none_or(|t| e.at <= t));
        events.sort_by(|a, b| {
            a.at.cmp(&b.at)
                .then_with(|| kind_order(&a.kind).cmp(&kind_order(&b.kind)))
                .then_with(|| a.record.cmp(&b.record))
        });
        let total = events.len();
        if events.len() > limit {
            events.drain(..events.len() - limit);
        }
        TimelineView { events, total }
    }

    /// Topic communities with their central entities and key decisions.
    pub fn topics_view(&self, limit: usize, per_topic: usize) -> TopicsView {
        let index = self.index();
        let mut members: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
        for (name, c) in &index.communities {
            members.entry(*c).or_default().push(name);
        }
        let unclustered = index.graph.entities.len() - index.communities.len();
        let topics = members
            .into_iter()
            .take(limit)
            .map(|(id, names)| {
                let mut central: Vec<GraphNode> = names
                    .iter()
                    .filter_map(|n| index.graph.entities.get(*n))
                    .map(|e| index.node(e))
                    .collect();
                sort_by_importance(&mut central);
                central.truncate(per_topic);

                // Records mentioning the topic, with how many members each names.
                let mut hits: HashMap<Uuid, usize> = HashMap::new();
                for name in &names {
                    for id in index.graph.entity_mentions.get(*name).into_iter().flatten() {
                        if index.records.contains_key(id) {
                            *hits.entry(*id).or_default() += 1;
                        }
                    }
                }
                let times: Vec<DateTime<Utc>> = hits
                    .keys()
                    .map(|id| index.records[id].1.timestamp)
                    .collect();
                let mut decisions: Vec<(usize, RecordView)> = hits
                    .iter()
                    .filter_map(|(id, n)| {
                        let record = index.record(*id)?;
                        (record.interaction_type == UpdateType::DecisionMade.as_str()
                            && record.validity.is_current())
                        .then_some((*n, record))
                    })
                    .collect();
                decisions.sort_by(|a, b| {
                    b.0.cmp(&a.0)
                        .then_with(|| b.1.timestamp.cmp(&a.1.timestamp))
                });
                TopicView {
                    id,
                    size: names.len(),
                    central,
                    key_decisions: decisions
                        .into_iter()
                        .take(per_topic)
                        .map(|(_, r)| r)
                        .collect(),
                    records: hits.len(),
                    first: times.iter().min().copied(),
                    last: times.iter().max().copied(),
                }
            })
            .collect();
        TopicsView {
            topics,
            unclustered,
        }
    }

    /// "Why did we choose X": the causal chain around `name` (`caused_by`,
    /// `leads_to`, `solves`, up to `depth` hops either way) and the
    /// records that built it, in time order.
    pub fn lineage_view(&self, name: &str, depth: usize) -> Option<LineageView> {
        let index = self.index();
        let entity = index.entity(name)?.name.clone();
        let causal = [
            RelationType::CausedBy,
            RelationType::LeadsTo,
            RelationType::Solves,
        ];
        let mut chain = vec![entity.clone()];
        let mut seen: HashSet<String> = HashSet::from([entity.clone()]);
        let mut queue = VecDeque::from([(entity.clone(), 0usize)]);
        let mut edges: BTreeSet<(String, String, RelationType)> = BTreeSet::new();
        while let Some((current, hops)) = queue.pop_front() {
            if hops >= depth.max(1) {
                continue;
            }
            for rel in index.graph.relationships_of(&current) {
                if !causal.contains(rel.relation_type) {
                    continue;
                }
                edges.insert((
                    rel.from.to_string(),
                    rel.to.to_string(),
                    rel.relation_type.clone(),
                ));
                let other = if rel.from == current {
                    rel.to
                } else {
                    rel.from
                };
                if seen.insert(other.to_string()) {
                    chain.push(other.to_string());
                    queue.push_back((other.to_string(), hops + 1));
                }
            }
        }

        let mut steps: Vec<LineageStep> = Vec::new();
        let mut listed = HashSet::new();
        for (from, to, rel) in &edges {
            let key = (from.clone(), to.clone(), rel.clone());
            for id in index.edge_records.get(&key).into_iter().flatten() {
                if listed.insert(*id)
                    && let Some(record) = index.record(*id)
                {
                    steps.push(LineageStep {
                        record,
                        relation: Some((from.clone(), to.clone(), rel.as_str().to_string())),
                    });
                }
            }
        }
        let decision_kinds = [
            UpdateType::ProblemSolved.as_str(),
            UpdateType::DecisionMade.as_str(),
        ];
        for record in index.mentioning(&entity) {
            if decision_kinds.contains(&record.interaction_type.as_str())
                && listed.insert(record.id)
            {
                steps.push(LineageStep {
                    record,
                    relation: None,
                });
            }
        }
        steps.sort_by(|a, b| {
            a.record
                .timestamp
                .cmp(&b.record.timestamp)
                .then_with(|| a.record.id.cmp(&b.record.id))
        });
        Some(LineageView {
            entity,
            chain,
            steps,
        })
    }

    /// Records, problems, decisions, entities and dependents of the source
    /// file `path` (matched by trailing path components, so an absolute
    /// path finds records written with a repository-relative one).
    pub fn file_view(&self, path: &str, depth: usize, limit: usize) -> FileView {
        let index = self.index();
        let wanted = path_components(path);

        // Entities naming the file.
        let file_name = wanted.last().copied().unwrap_or_default();
        let stem = file_name
            .rsplit_once('.')
            .map_or(file_name, |(stem, _)| stem);
        let stem_ok = stem.chars().count() >= 4 && !GENERIC_STEMS.contains(&stem);
        let mut entities: Vec<&EntityData> = index
            .graph
            .entities
            .values()
            .filter(|e| {
                std::iter::once(&e.name).chain(&e.aliases).any(|n| {
                    same_path(&path_components(n), &wanted)
                        || (!file_name.is_empty() && n == file_name)
                        || (stem_ok && entity_key(n) == entity_key(stem))
                })
            })
            .collect();
        entities.sort_by(|a, b| a.name.cmp(&b.name));

        let mut ids: HashSet<Uuid> = index
            .records
            .values()
            .filter(|(_, u)| {
                u.related_code
                    .as_ref()
                    .is_some_and(|c| same_path(&path_components(&c.file_path), &wanted))
            })
            .map(|(_, u)| u.id)
            .collect();
        for e in &entities {
            ids.extend(
                index
                    .graph
                    .entity_mentions
                    .get(&e.name)
                    .into_iter()
                    .flatten(),
            );
        }
        let mut records: Vec<RecordView> =
            ids.into_iter().filter_map(|id| index.record(id)).collect();
        records.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then_with(|| a.id.cmp(&b.id)));

        let of_kind = |kinds: &[&str], current_only: bool| -> Vec<RecordView> {
            let mut out: Vec<RecordView> = records
                .iter()
                .filter(|r| kinds.contains(&r.interaction_type.as_str()))
                .filter(|r| !current_only || r.validity.is_current())
                .cloned()
                .collect();
            out.sort_by_key(|r| !r.validity.is_current());
            out.truncate(limit);
            out
        };
        let problems = of_kind(&[UpdateType::ProblemSolved.as_str()], false);
        let decisions = of_kind(
            &[
                UpdateType::DecisionMade.as_str(),
                UpdateType::RequirementAdded.as_str(),
            ],
            true,
        );

        let names: Vec<String> = entities.iter().map(|e| e.name.clone()).collect();
        let mut dependents = Vec::new();
        let mut seen: HashSet<String> = names.iter().cloned().collect();
        let mut frontier = names;
        for hop in 1..=depth {
            if frontier.is_empty() {
                break;
            }
            let mut next = Vec::new();
            for impact in analyze_impact(&frontier, index.graph) {
                if seen.insert(impact.entity.clone()) {
                    next.push(impact.entity.clone());
                    dependents.push(ImpactItem {
                        entity_type: index
                            .graph
                            .entities
                            .get(&impact.entity)
                            .map(|e| e.entity_type.as_str().to_string()),
                        entity: impact.entity,
                        via: impact.depends_on,
                        relation_type: impact.relation_type.as_str().to_string(),
                        context: impact.context,
                        depth: hop,
                    });
                }
            }
            next.sort();
            frontier = next;
        }

        records.truncate(limit);
        FileView {
            path: path.to_string(),
            records,
            problems,
            decisions,
            entities: entities.into_iter().map(|e| index.node(e)).collect(),
            dependents,
        }
    }

    /// Unsolved problems, decisions without rationale, open questions,
    /// entities untouched since `now - 90 days`, and possible aliases.
    pub fn radar_view(&self, now: DateTime<Utc>, limit: usize) -> RadarView {
        let index = self.index();
        let graph = index.graph;

        let mut unsolved_problems: Vec<GraphNode> = graph
            .entities
            .values()
            .filter(|e| e.entity_type == EntityType::Problem)
            .filter(|e| {
                !graph
                    .relationships_of(&e.name)
                    .iter()
                    .any(|r| *r.relation_type == RelationType::Solves && r.to == e.name)
            })
            .map(|e| index.node(e))
            .collect();
        sort_by_importance(&mut unsolved_problems);
        unsolved_problems.truncate(limit);

        let mut decisions_without_rationale: Vec<RecordView> = index
            .records
            .values()
            .filter(|(_, u)| {
                u.update_type == UpdateType::DecisionMade
                    && u.content.description.trim().is_empty()
                    && u.content.details.is_empty()
            })
            .filter_map(|(_, u)| index.record(u.id))
            .filter(|r| r.validity.is_current())
            .collect();
        decisions_without_rationale.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        decisions_without_rationale.truncate(limit);

        let mut open_questions: Vec<String> = self
            .sessions
            .iter()
            .flat_map(|s| s.current_state.open_questions.iter())
            .filter(|q| {
                matches!(
                    q.status,
                    post_cortex_core::core::structured_context::QuestionStatus::Open
                )
            })
            .map(|q| q.question.clone())
            .collect();
        open_questions.truncate(limit);

        let cutoff = now - Duration::days(STALE_AFTER_DAYS);
        let mut stale_entities: Vec<GraphNode> = graph
            .entities
            .values()
            .filter(|e| e.last_mentioned < cutoff)
            .map(|e| index.node(e))
            .collect();
        sort_by_importance(&mut stale_entities);
        stale_entities.truncate(limit);

        RadarView {
            unsolved_problems,
            decisions_without_rationale,
            open_questions,
            stale_entities,
            alias_candidates: alias_candidates(graph, limit),
        }
    }
}

/// Default number of nodes, records or events a view returns.
pub const DEFAULT_LIMIT: usize = 100;
/// Most a caller may ask for.
pub const MAX_LIMIT: usize = 5000;

/// One view with its parameters, parsed from string pairs so every
/// transport (REST query string, MCP `parameters`, gRPC map) shares one
/// parser.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewRequest {
    /// [`GraphScope::graph_view`].
    Graph {
        /// Most nodes.
        limit: usize,
        /// Point in time.
        at: Option<DateTime<Utc>>,
    },
    /// [`GraphScope::entity_view`].
    Entity {
        /// Entity name or alias.
        name: String,
        /// Most records.
        limit: usize,
    },
    /// [`GraphScope::impact_view`].
    Impact {
        /// Entity name or alias.
        name: String,
        /// Hops.
        depth: usize,
        /// Most risk records.
        limit: usize,
    },
    /// [`GraphScope::timeline`].
    Timeline {
        /// Window start.
        from: Option<DateTime<Utc>>,
        /// Window end.
        to: Option<DateTime<Utc>>,
        /// Most events.
        limit: usize,
    },
    /// [`GraphScope::topics_view`].
    Topics {
        /// Most topics.
        limit: usize,
        /// Entities and decisions per topic.
        per_topic: usize,
    },
    /// [`GraphScope::lineage_view`].
    Lineage {
        /// Entity name or alias.
        name: String,
        /// Hops.
        depth: usize,
    },
    /// [`GraphScope::radar_view`].
    Radar {
        /// Most items per list.
        limit: usize,
    },
    /// [`GraphScope::file_view`].
    File {
        /// Source file path.
        path: String,
        /// Hops of impact analysis.
        depth: usize,
        /// Most records per list.
        limit: usize,
    },
}

/// View names [`ViewRequest::parse`] accepts.
pub const VIEW_NAMES: &[&str] = &[
    "graph", "entity", "impact", "timeline", "topics", "lineage", "radar", "file",
];

impl ViewRequest {
    /// Parse view `view` from `params`: `limit`, `at`, `from`, `to`
    /// (RFC 3339 or a date), `depth`, `per_topic`, `entity` (or
    /// `entity_name`) for the entity views and `path` for `file`.
    pub fn parse(view: &str, params: &HashMap<String, String>) -> Result<Self, String> {
        let get = |k: &str| params.get(k).map(|v| v.trim()).filter(|v| !v.is_empty());
        let number = |k: &str, default: usize, max: usize| -> Result<usize, String> {
            match get(k) {
                None => Ok(default),
                Some(v) => v
                    .parse::<usize>()
                    .map(|n| n.clamp(1, max))
                    .map_err(|_| format!("{k} must be a positive integer, got '{v}'")),
            }
        };
        let time = |k: &str| -> Result<Option<DateTime<Utc>>, String> {
            get(k)
                .map(|v| {
                    parse_time(v).ok_or_else(|| format!("{k}: not a date or RFC 3339 time: '{v}'"))
                })
                .transpose()
        };
        let name = || -> Result<String, String> {
            get("entity")
                .or_else(|| get("entity_name"))
                .map(str::to_string)
                .ok_or_else(|| format!("view '{view}' needs an 'entity' parameter"))
        };
        let limit = number("limit", DEFAULT_LIMIT, MAX_LIMIT)?;
        Ok(match view {
            "graph" => Self::Graph {
                limit,
                at: time("at")?,
            },
            "entity" => Self::Entity {
                name: name()?,
                limit,
            },
            "impact" => Self::Impact {
                name: name()?,
                depth: number("depth", 3, 10)?,
                limit,
            },
            "timeline" => Self::Timeline {
                from: time("from")?,
                to: time("to")?,
                limit,
            },
            "topics" => Self::Topics {
                limit,
                per_topic: number("per_topic", 5, 50)?,
            },
            "lineage" => Self::Lineage {
                name: name()?,
                depth: number("depth", 3, 10)?,
            },
            "radar" => Self::Radar { limit },
            "file" => Self::File {
                path: get("path")
                    .map(str::to_string)
                    .ok_or_else(|| "view 'file' needs a 'path' parameter".to_string())?,
                depth: number("depth", 2, 10)?,
                limit: number("limit", 20, MAX_LIMIT)?,
            },
            other => {
                return Err(format!(
                    "unknown view '{other}'; expected one of {}",
                    VIEW_NAMES.join(", ")
                ));
            }
        })
    }
}

/// RFC 3339, or a bare date (midnight UTC).
pub fn parse_time(v: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v)
        .map(|t| t.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|t| t.and_utc())
        })
}

/// Why a view could not be rendered.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewError {
    /// The entity is not in the scope.
    EntityNotFound(String),
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewError::EntityNotFound(name) => write!(f, "entity '{name}' not found in scope"),
        }
    }
}

impl GraphScope {
    /// Render `request` as JSON.
    pub fn render(&self, request: &ViewRequest) -> Result<serde_json::Value, ViewError> {
        let missing = |name: &str| ViewError::EntityNotFound(name.to_string());
        let value = match request {
            ViewRequest::Graph { limit, at } => serde_json::to_value(self.graph_view(*limit, *at)),
            ViewRequest::Entity { name, limit } => serde_json::to_value(
                self.entity_view(name, *limit)
                    .ok_or_else(|| missing(name))?,
            ),
            ViewRequest::Impact { name, depth, limit } => serde_json::to_value(
                self.impact_view(name, *depth, *limit)
                    .ok_or_else(|| missing(name))?,
            ),
            ViewRequest::Timeline { from, to, limit } => {
                serde_json::to_value(self.timeline(*from, *to, *limit))
            }
            ViewRequest::Topics { limit, per_topic } => {
                serde_json::to_value(self.topics_view(*limit, *per_topic))
            }
            ViewRequest::Lineage { name, depth } => serde_json::to_value(
                self.lineage_view(name, *depth)
                    .ok_or_else(|| missing(name))?,
            ),
            ViewRequest::Radar { limit } => {
                serde_json::to_value(self.radar_view(Utc::now(), *limit))
            }
            ViewRequest::File { path, depth, limit } => {
                serde_json::to_value(self.file_view(path, *depth, *limit))
            }
        };
        // The views are plain data; serialising them cannot fail.
        Ok(value.unwrap_or(serde_json::Value::Null))
    }
}

/// File stems too common to name an entity on their own.
const GENERIC_STEMS: &[&str] = &[
    "main", "index", "init", "__init__", "test", "tests", "utils", "types", "README",
];

/// Path components, without `.` and empty ones; backslashes count as
/// separators.
fn path_components(path: &str) -> Vec<&str> {
    path.split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != ".")
        .collect()
}

/// Whether two paths name the same file: the shorter one is a
/// component-wise suffix of the longer, and has at least two components
/// unless both are bare file names.
fn same_path(a: &[&str], b: &[&str]) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    !short.is_empty() && long.ends_with(short) && (short.len() > 1 || a.len() == b.len())
}

/// Entity pairs where one name's key is a prefix of the other's (at least
/// four characters, as "surreal" / "surrealdb"), most shared records first.
fn alias_candidates(graph: &SimpleEntityGraph, limit: usize) -> Vec<AliasCandidate> {
    let mut keyed: Vec<(String, &str)> = graph
        .entities
        .keys()
        .map(|n| (entity_key(n), n.as_str()))
        .collect();
    keyed.sort();
    let mut out = Vec::new();
    for (i, (short, name)) in keyed.iter().enumerate() {
        if short.chars().count() < 4 {
            continue;
        }
        // Keys sharing the prefix follow `short` in sorted order.
        for (long, other) in keyed[i + 1..]
            .iter()
            .take_while(|(k, _)| k.starts_with(short))
        {
            if long == short {
                continue;
            }
            let mine: HashSet<&Uuid> = graph
                .entity_mentions
                .get(*name)
                .into_iter()
                .flatten()
                .collect();
            let shared = graph
                .entity_mentions
                .get(*other)
                .into_iter()
                .flatten()
                .filter(|id| mine.contains(id))
                .count();
            out.push(AliasCandidate {
                name: name.to_string(),
                of: other.to_string(),
                shared_records: shared,
            });
        }
    }
    out.sort_by(|a, b| {
        b.shared_records
            .cmp(&a.shared_records)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.of.cmp(&b.of))
    });
    out.truncate(limit);
    out
}

impl Index<'_> {
    fn entity(&self, name: &str) -> Option<&EntityData> {
        self.graph
            .resolve(name)
            .or_else(|| {
                // Fall back to a case-insensitive match on exact names.
                let lower = name.to_lowercase();
                self.graph
                    .entities
                    .keys()
                    .find(|k| k.to_lowercase() == lower)
                    .map(String::as_str)
            })
            .and_then(|n| self.graph.entities.get(n))
    }

    fn node(&self, e: &EntityData) -> GraphNode {
        GraphNode {
            name: e.name.clone(),
            entity_type: e.entity_type.as_str().to_string(),
            aliases: e.aliases.clone(),
            description: e.description.clone(),
            importance: self.importance.get(&e.name).copied().unwrap_or(0.0),
            mention_count: e.mention_count,
            records: self.graph.entity_mentions.get(&e.name).map_or(0, |ids| {
                ids.iter()
                    .filter(|id| self.records.contains_key(id))
                    .count()
            }),
            first_seen: e.first_mentioned,
            last_seen: e.last_mentioned,
            community: self.communities.get(&e.name).copied(),
        }
    }

    /// Validity of an edge asserted by `ids`: from the earliest start, until
    /// the last of them ends (open while any holds).
    fn edge_validity(&self, ids: &[Uuid]) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
        let intervals: Vec<&Validity> = ids.iter().filter_map(|id| self.validity.get(id)).collect();
        let start = intervals.iter().map(|v| v.valid_from).min();
        let end = if intervals.iter().any(|v| v.is_current()) {
            None
        } else {
            intervals.iter().filter_map(|v| v.valid_to).max()
        };
        (start, end)
    }

    fn edge(&self, from: &str, to: &str, rel: &RelationType, context: &str) -> GraphEdge {
        let records = self
            .edge_records
            .get(&(from.to_string(), to.to_string(), rel.clone()))
            .cloned()
            .unwrap_or_default();
        let (valid_from, valid_to) = self.edge_validity(&records);
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            relation_type: rel.as_str().to_string(),
            context: context.to_string(),
            weight: rel.weight(),
            valid_from,
            valid_to,
            records,
        }
    }

    fn relations_of(&self, name: &str) -> Vec<GraphEdge> {
        let mut edges: Vec<GraphEdge> = self
            .graph
            .relationships_of(name)
            .iter()
            .map(|r| self.edge(r.from, r.to, r.relation_type, r.context))
            .collect();
        sort_edges(&mut edges);
        edges
    }

    fn record(&self, id: Uuid) -> Option<RecordView> {
        let (session_id, update) = self.records.get(&id)?;
        let mut description: String = update
            .content
            .description
            .chars()
            .take(DESCRIPTION_CHARS)
            .collect();
        if description.len() < update.content.description.len() {
            description.push('…');
        }
        Some(RecordView {
            id,
            session_id: *session_id,
            interaction_type: update.update_type.as_str().to_string(),
            title: update.content.title.clone(),
            description,
            author: update.author.clone(),
            timestamp: update.timestamp,
            confidence: update.meta.confidence,
            derived_from: update.derived_from.clone(),
            file: update.related_code.as_ref().map(|c| c.file_path.clone()),
            validity: self.validity.get(&id).cloned()?,
        })
    }

    /// Records of the scope that mention `name` (canonical).
    fn mentioning(&self, name: &str) -> Vec<RecordView> {
        self.graph
            .entity_mentions
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|id| self.record(*id))
            .collect()
    }
}

fn kind_order(kind: &TimelineEventKind) -> u8 {
    match kind {
        TimelineEventKind::Ended => 0,
        TimelineEventKind::RelationEnded => 1,
        TimelineEventKind::Recorded => 2,
        TimelineEventKind::RelationAdded => 3,
    }
}

fn sort_by_importance(nodes: &mut [GraphNode]) {
    nodes.sort_by(|a, b| {
        b.importance
            .total_cmp(&a.importance)
            .then_with(|| a.name.cmp(&b.name))
    });
}

fn sort_edges(edges: &mut [GraphEdge]) {
    edges.sort_by(|a, b| {
        (&a.from, &a.to, &a.relation_type).cmp(&(&b.from, &b.to, &b.relation_type))
    });
}

#[cfg(test)]
#[path = "graph_views_tests.rs"]
mod tests;
