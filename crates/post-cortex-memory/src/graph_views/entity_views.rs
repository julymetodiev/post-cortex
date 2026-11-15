// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Views of an entity: the entity with its records, what depends on it
//! (impact), its causal chain (lineage), and topics.

use crate::context_assembly::analyze_impact;
use crate::graph_views::index::Index;
use crate::graph_views::index::sort_by_importance;
use crate::graph_views::types::{
    EntityView, GraphNode, ImpactItem, ImpactView, LineageLink, LineageLinks, LineageStep,
    LineageView, RecordView, RelationRef, TopicView, TopicsView,
};
use chrono::{DateTime, Utc};
use post_cortex_core::core::context_update::{RelationType, UpdateType};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use uuid::Uuid;

impl Index<'_> {
    /// One entity (by name or alias): its relations, the records that
    /// mention it (newest first, at most `limit`) and its decisions.
    pub fn entity_view(&self, name: &str, limit: usize) -> Option<EntityView> {
        let index = self;
        let entity = index.entity(name)?;
        let relations = index.relations_of(&entity.name);
        let mut records = index.mentioning(&entity.name);
        records.sort_by_key(|r| std::cmp::Reverse(r.timestamp));
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
        let index = self;
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
        risks.sort_by_key(|r| std::cmp::Reverse(r.timestamp));
        risks.truncate(limit);
        Some(ImpactView {
            entity,
            dependents,
            risks,
        })
    }

    /// Topic communities with their central entities and key decisions.
    pub fn topics_view(&self, limit: usize, per_topic: usize) -> TopicsView {
        let index = self;
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

    /// "Why did we choose X": the causal chain around `name`, up to `depth`
    /// hops each way, and the records that built it, in time order. Each
    /// causal relation points from cause to effect (`a leads_to b`: a to b;
    /// `a caused_by b`: b to a; `a solves b`: the problem b to its solution
    /// a). The chain walks back from `name` along them to its causes and
    /// forward to its consequences, never turning: from a problem it reaches
    /// what caused it and what solved it, not the other problems that
    /// solution also solved.
    pub fn lineage_links(&self, name: &str, depth: usize) -> Option<LineageLinks> {
        let index = self;
        let entity = index.entity(name)?.name.clone();
        let mut chain = vec![entity.clone()];
        let mut seen: HashSet<String> = HashSet::from([entity.clone()]);
        let mut edges: BTreeSet<(String, String, RelationType)> = BTreeSet::new();
        for forward in [false, true] {
            let mut queue = VecDeque::from([(entity.clone(), 0usize)]);
            while let Some((current, hops)) = queue.pop_front() {
                if hops >= depth.max(1) {
                    continue;
                }
                for rel in index.graph.relationships_of(&current) {
                    let (cause, effect) = match rel.relation_type {
                        RelationType::LeadsTo => (rel.from, rel.to),
                        RelationType::CausedBy | RelationType::Solves => (rel.to, rel.from),
                        _ => continue,
                    };
                    let (here, other) = if forward {
                        (cause, effect)
                    } else {
                        (effect, cause)
                    };
                    if here != current || other == current {
                        continue;
                    }
                    edges.insert((rel.from.to_string(), rel.to.to_string(), *rel.relation_type));
                    if seen.insert(other.to_string()) {
                        chain.push(other.to_string());
                        queue.push_back((other.to_string(), hops + 1));
                    }
                }
            }
        }

        // (time, record, relation) of every record on the chain, once.
        let mut found: Vec<(DateTime<Utc>, LineageLink)> = Vec::new();
        let mut listed = HashSet::new();
        for (from, to, rel) in &edges {
            let key = (from.clone(), to.clone(), *rel);
            for id in index.edge_records.get(&key).into_iter().flatten() {
                if let Some((_, u)) = index.records.get(id)
                    && listed.insert(*id)
                {
                    found.push((
                        u.timestamp,
                        LineageLink {
                            record: *id,
                            relation: Some(RelationRef {
                                from: from.clone(),
                                to: to.clone(),
                                relation_type: *rel,
                            }),
                        },
                    ));
                }
            }
        }
        let decision_kinds = [UpdateType::ProblemSolved, UpdateType::DecisionMade];
        for id in index
            .graph
            .entity_mentions
            .get(&entity)
            .into_iter()
            .flatten()
        {
            if let Some((_, u)) = index.records.get(id)
                && decision_kinds.contains(&u.update_type)
                && listed.insert(*id)
            {
                found.push((
                    u.timestamp,
                    LineageLink {
                        record: *id,
                        relation: None,
                    },
                ));
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.record.cmp(&b.1.record)));
        Some(LineageLinks {
            entity,
            chain,
            links: found.into_iter().map(|(_, link)| link).collect(),
        })
    }

    /// [`Self::lineage_links`] as agents read it.
    pub fn lineage_view(&self, name: &str, depth: usize) -> Option<LineageView> {
        let links = self.lineage_links(name, depth)?;
        Some(LineageView {
            entity: links.entity,
            chain: links.chain,
            steps: links
                .links
                .into_iter()
                .filter_map(|link| {
                    Some(LineageStep {
                        record: self.record(link.record)?,
                        relation: link
                            .relation
                            .map(|r| (r.from, r.to, r.relation_type.as_str().to_string())),
                    })
                })
                .collect(),
        })
    }
}
