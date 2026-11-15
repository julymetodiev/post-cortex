// Copyright (c) 2026 Julius ML
// MIT License

//! Whole-graph analytics over a [`SimpleEntityGraph`]: centrality-based
//! importance and topic communities.
//!
//! Both treat the graph as undirected with each pair's weight the sum of
//! its relations' [`RelationType::weight`](crate::core::context_update::RelationType::weight),
//! and both are deterministic: nodes are processed in name order.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};

use crate::graph::entity_graph::SimpleEntityGraph;

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
        let names: Vec<&str> = graph
            .entities
            .sorted()
            .into_iter()
            .map(|(name, _)| name.as_str())
            .collect();
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
        let linked = graph
            .entity_mentions
            .get(name)
            .map_or(0, crate::collections::SharedVec::len);
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
    let rank = personalised_pagerank(
        &g.adjacency,
        &strength,
        &teleport,
        PageRank {
            damping: DAMPING,
            max_iterations: PAGERANK_ITERATIONS,
            tolerance: PAGERANK_TOLERANCE,
        },
    );

    let top = rank.iter().copied().fold(f64::MIN, f64::max);
    let latest = graph
        .entities
        .values_unordered()
        .map(|e| e.last_mentioned)
        .max();
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

/// The bucket of a score of [`importance`], on its `(0, 1]` scale (the
/// most central, recently mentioned entity scores 1): critical from 0.8,
/// high from 0.5, medium from 0.25, low from 0.1, else minimal.
pub fn importance_level(score: f32) -> crate::summary::ImportanceLevel {
    use crate::summary::ImportanceLevel;
    match score {
        s if s >= 0.8 => ImportanceLevel::Critical,
        s if s >= 0.5 => ImportanceLevel::High,
        s if s >= 0.25 => ImportanceLevel::Medium,
        s if s >= 0.1 => ImportanceLevel::Low,
        _ => ImportanceLevel::Minimal,
    }
}

/// Parameters of [`personalised_pagerank`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct PageRank {
    /// Share of the mass that follows edges each step.
    pub damping: f64,
    /// Steps at most.
    pub max_iterations: usize,
    /// Stop once a step moves less than this (L1); 0 runs every step.
    pub tolerance: f64,
}

/// Personalised PageRank over weighted adjacency lists: the one PageRank
/// of the crate (entity importance and the retrieval graph). `strength` is
/// each node's total edge weight and `teleport` the restart distribution
/// (summing to 1). The mass of nodes without edges restarts through
/// `teleport`.
pub(crate) fn personalised_pagerank(
    adjacency: &[Vec<(usize, f64)>],
    strength: &[f64],
    teleport: &[f64],
    params: PageRank,
) -> Vec<f64> {
    let n = adjacency.len();
    let mut rank = teleport.to_vec();
    let mut next = vec![0.0; n];
    for _ in 0..params.max_iterations {
        let dangling: f64 = (0..n)
            .filter(|&i| strength[i] == 0.0)
            .map(|i| rank[i])
            .sum();
        let restart = 1.0 - params.damping + params.damping * dangling;
        for (x, t) in next.iter_mut().zip(teleport) {
            *x = restart * t;
        }
        for (i, neighbours) in adjacency.iter().enumerate() {
            if strength[i] == 0.0 {
                continue;
            }
            let share = params.damping * rank[i] / strength[i];
            for &(j, w) in neighbours {
                next[j] += share * w;
            }
        }
        let delta: f64 = next.iter().zip(&rank).map(|(a, b)| (a - b).abs()).sum();
        std::mem::swap(&mut rank, &mut next);
        if delta < params.tolerance {
            break;
        }
    }
    rank
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
mod tests;
