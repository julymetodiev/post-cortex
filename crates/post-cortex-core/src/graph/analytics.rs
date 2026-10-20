// Copyright (c) 2026 Julius ML
// MIT License

//! Whole-graph analytics over a [`SimpleEntityGraph`]: centrality-based
//! importance (ROADMAP Phase 3 item 11) and topic communities (item 5).
//!
//! Both treat the graph as undirected with each pair's weight the sum of
//! its relations' [`RelationType::weight`](crate::core::context_update::RelationType::weight),
//! and both are deterministic: nodes are processed in name order.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};

use super::entity_graph::SimpleEntityGraph;

/// PageRank damping factor.
const DAMPING: f64 = 0.85;
/// PageRank stops after this many iterations or once it moves less than
/// [`PAGERANK_TOLERANCE`] (L1).
const PAGERANK_ITERATIONS: usize = 100;
const PAGERANK_TOLERANCE: f64 = 1e-9;
/// Days without a mention after which an entity's recency factor halves.
const RECENCY_HALF_LIFE_DAYS: f64 = 90.0;
/// Louvain passes over the nodes per level, at most.
const LOUVAIN_PASSES: usize = 32;

/// The graph as undirected weighted adjacency over name-sorted node indices.
struct Undirected<'a> {
    names: Vec<&'a str>,
    /// Per node: (neighbour, weight), one entry per neighbour.
    adjacency: Vec<Vec<(usize, f64)>>,
}

impl<'a> Undirected<'a> {
    fn new(graph: &'a SimpleEntityGraph) -> Self {
        let names: Vec<&str> = graph.entities.keys().map(String::as_str).collect();
        let index: HashMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let mut pairs: BTreeMap<(usize, usize), f64> = BTreeMap::new();
        for rel in graph.relationships() {
            let (Some(&a), Some(&b)) = (index.get(rel.from), index.get(rel.to)) else {
                continue;
            };
            if a != b {
                *pairs.entry((a.min(b), a.max(b))).or_default() +=
                    f64::from(rel.relation_type.weight());
            }
        }
        let mut adjacency = vec![Vec::new(); names.len()];
        for ((a, b), w) in pairs {
            adjacency[a].push((b, w));
            adjacency[b].push((a, w));
        }
        Self { names, adjacency }
    }

    fn strength(&self, node: usize) -> f64 {
        self.adjacency[node].iter().map(|(_, w)| w).sum()
    }
}

/// Importance of every entity in `(0, 1]`: weighted PageRank, personalised
/// by how many records mention each entity, scaled to the most central one,
/// then faded by how long ago the entity was last mentioned (relative to
/// the graph's latest mention, so a dormant project does not fade as a
/// whole): an entity untouched for one half-life keeps 75%, for ever 50%.
pub fn importance(graph: &SimpleEntityGraph) -> BTreeMap<String, f32> {
    let g = Undirected::new(graph);
    let n = g.names.len();
    if n == 0 {
        return BTreeMap::new();
    }

    let mentions = |name: &str| -> f64 {
        let linked = graph.entity_mentions.get(name).map_or(0, Vec::len);
        let counted = graph
            .entities
            .get(name)
            .map_or(0, |e| e.mention_count as usize);
        linked.max(counted) as f64
    };
    let mut teleport: Vec<f64> = g.names.iter().map(|n| 1.0 + mentions(n).ln_1p()).collect();
    let total: f64 = teleport.iter().sum();
    teleport.iter_mut().for_each(|t| *t /= total);

    let strength: Vec<f64> = (0..n).map(|i| g.strength(i)).collect();
    let mut rank = teleport.clone();
    for _ in 0..PAGERANK_ITERATIONS {
        // Mass of nodes without edges goes back through the teleport.
        let dangling: f64 = (0..n)
            .filter(|&i| strength[i] == 0.0)
            .map(|i| rank[i])
            .sum();
        let mut next: Vec<f64> = teleport
            .iter()
            .map(|t| (1.0 - DAMPING + DAMPING * dangling) * t)
            .collect();
        for (i, neighbours) in g.adjacency.iter().enumerate() {
            if strength[i] == 0.0 {
                continue;
            }
            let share = DAMPING * rank[i] / strength[i];
            for &(j, w) in neighbours {
                next[j] += share * w;
            }
        }
        let delta: f64 = next.iter().zip(&rank).map(|(a, b)| (a - b).abs()).sum();
        rank = next;
        if delta < PAGERANK_TOLERANCE {
            break;
        }
    }

    let top = rank.iter().copied().fold(f64::MIN, f64::max);
    let latest = graph.entities.values().map(|e| e.last_mentioned).max();
    g.names
        .iter()
        .zip(&rank)
        .map(|(name, r)| {
            let fade = match (latest, graph.entities.get(*name)) {
                (Some(latest), Some(e)) => recency(e.last_mentioned, latest),
                _ => 1.0,
            };
            (name.to_string(), ((r / top) * (0.5 + 0.5 * fade)) as f32)
        })
        .collect()
}

/// `0.5^(days since last / half-life)`.
fn recency(last: DateTime<Utc>, latest: DateTime<Utc>) -> f64 {
    let days = (latest - last).num_seconds().max(0) as f64 / 86_400.0;
    0.5f64.powf(days / RECENCY_HALF_LIFE_DAYS)
}

/// Topic communities (Louvain modularity optimisation). Returns each
/// entity's community index; entities alone in their community are left
/// out. Communities are numbered by size, largest first (ties: by their
/// first member's name).
pub fn communities(graph: &SimpleEntityGraph) -> BTreeMap<String, usize> {
    let g = Undirected::new(graph);
    let n = g.names.len();
    // membership[node] = community at the current level, for original nodes.
    let mut membership: Vec<usize> = (0..n).collect();
    let mut level: Vec<Vec<(usize, f64)>> = g.adjacency.clone();
    loop {
        let assignment = louvain_level(&level);
        let groups = renumber(&assignment);
        let count = groups.iter().copied().max().map_or(0, |m| m + 1);
        if count == level.len() {
            break;
        }
        for c in membership.iter_mut() {
            *c = groups[*c];
        }
        // Aggregate: one node per community, edges summed (self loops kept
        // as internal weight).
        let mut pairs: BTreeMap<(usize, usize), f64> = BTreeMap::new();
        for (i, neighbours) in level.iter().enumerate() {
            for &(j, w) in neighbours {
                if i <= j {
                    let (a, b) = (groups[i], groups[j]);
                    *pairs.entry((a.min(b), a.max(b))).or_default() += w;
                }
            }
        }
        let mut next = vec![Vec::new(); count];
        for ((a, b), w) in pairs {
            next[a].push((b, w));
            if a != b {
                next[b].push((a, w));
            }
        }
        level = next;
    }

    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (node, c) in membership.iter().enumerate() {
        members.entry(*c).or_default().push(node);
    }
    let mut groups: Vec<Vec<usize>> = members.into_values().filter(|m| m.len() > 1).collect();
    groups.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a[0].cmp(&b[0])));
    let mut out = BTreeMap::new();
    for (index, group) in groups.into_iter().enumerate() {
        for node in group {
            out.insert(g.names[node].to_string(), index);
        }
    }
    out
}

/// Community indices 0.. in order of first appearance.
fn renumber(assignment: &[usize]) -> Vec<usize> {
    let mut map = HashMap::new();
    assignment
        .iter()
        .map(|c| {
            let next = map.len();
            *map.entry(*c).or_insert(next)
        })
        .collect()
}

/// One Louvain level: move nodes between communities while modularity
/// improves. `adjacency` may hold self loops (internal weight of
/// aggregated nodes, listed once).
fn louvain_level(adjacency: &[Vec<(usize, f64)>]) -> Vec<usize> {
    let n = adjacency.len();
    let degree: Vec<f64> = adjacency
        .iter()
        .enumerate()
        .map(|(i, nb)| {
            nb.iter()
                .map(|&(j, w)| if j == i { 2.0 * w } else { w })
                .sum()
        })
        .collect();
    let m2: f64 = degree.iter().sum();
    let mut community: Vec<usize> = (0..n).collect();
    if m2 == 0.0 {
        return community;
    }
    let mut total: Vec<f64> = degree.clone();

    for _ in 0..LOUVAIN_PASSES {
        let mut moved = false;
        for i in 0..n {
            let own = community[i];
            total[own] -= degree[i];
            // Weight from i to each neighbouring community.
            let mut links: BTreeMap<usize, f64> = BTreeMap::new();
            for &(j, w) in &adjacency[i] {
                if j != i {
                    *links.entry(community[j]).or_default() += w;
                }
            }
            let gain = |c: usize, k_in: f64| k_in - total[c] * degree[i] / m2;
            let mut best = (own, gain(own, links.get(&own).copied().unwrap_or(0.0)));
            for (&c, &k_in) in &links {
                let g = gain(c, k_in);
                if g > best.1 + 1e-12 {
                    best = (c, g);
                }
            }
            community[i] = best.0;
            total[best.0] += degree[i];
            moved |= best.0 != own;
        }
        if !moved {
            break;
        }
    }
    community
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::context_update::{EntityRelationship, EntityType, RelationType};

    fn graph(edges: &[(&str, &str, RelationType)]) -> SimpleEntityGraph {
        let mut g = SimpleEntityGraph::new();
        let now = Utc::now();
        for (a, b, _) in edges {
            g.add_or_update_entity(a.to_string(), EntityType::Concept, now, "");
            g.add_or_update_entity(b.to_string(), EntityType::Concept, now, "");
        }
        for (a, b, r) in edges {
            g.add_relationship(EntityRelationship {
                from_entity: a.to_string(),
                to_entity: b.to_string(),
                relation_type: r.clone(),
                context: String::new(),
            });
        }
        g
    }

    #[test]
    fn hub_is_most_important() {
        let g = graph(&[
            ("hub", "a", RelationType::DependsOn),
            ("hub", "b", RelationType::DependsOn),
            ("hub", "c", RelationType::DependsOn),
            ("c", "d", RelationType::RelatedTo),
        ]);
        let scores = importance(&g);
        assert_eq!(scores["hub"], 1.0);
        assert!(scores.values().all(|s| *s > 0.0 && *s <= 1.0));
        assert!(scores["a"] < scores["hub"]);
        assert!(scores["d"] < scores["c"]);
    }

    #[test]
    fn stale_entities_fade() {
        let mut g = graph(&[("x", "y", RelationType::DependsOn)]);
        g.entities.get_mut("y").unwrap().last_mentioned -= chrono::Duration::days(90);
        let scores = importance(&g);
        // Symmetric pair: same centrality, y faded to 75%.
        assert!(
            (scores["y"] / scores["x"] - 0.75).abs() < 1e-3,
            "{scores:?}"
        );
    }

    #[test]
    fn two_cliques_are_two_communities() {
        use RelationType::DependsOn as D;
        let g = graph(&[
            ("a1", "a2", D),
            ("a2", "a3", D),
            ("a1", "a3", D),
            ("b1", "b2", D),
            ("b2", "b3", D),
            ("b1", "b3", D),
            ("a3", "b1", RelationType::RelatedTo),
        ]);
        let c = communities(&g);
        assert_eq!(c["a1"], c["a2"]);
        assert_eq!(c["a1"], c["a3"]);
        assert_eq!(c["b1"], c["b2"]);
        assert_eq!(c["b1"], c["b3"]);
        assert_ne!(c["a1"], c["b1"]);
        assert_eq!(communities(&g), c, "deterministic");
    }

    #[test]
    fn isolated_entities_have_no_community() {
        let mut g = graph(&[("a", "b", RelationType::DependsOn)]);
        g.add_or_update_entity("lonely".into(), EntityType::Concept, Utc::now(), "");
        let c = communities(&g);
        assert!(!c.contains_key("lonely"));
        assert_eq!(c.len(), 2);
    }
}
