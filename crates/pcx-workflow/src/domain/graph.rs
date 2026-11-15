// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A graph's shape: its blocks' ids (unique in the whole workflow), its
//! edges (their ends in the graph, each port of a block at most once), its
//! starts (the triggers at the top, one block in a body or a branch), no
//! cycles, and the order its blocks are checked and scheduled in.

use std::collections::{BTreeMap, BTreeSet};

use post_cortex_proto::workflow::{Block, Graph, Point, block::Kind};

use crate::domain::Refusal;

/// Characters of a block's id at most.
const MAX_ID: usize = 64;

/// The port an edge leaves by when it names none.
pub const NEXT: &str = "next";

/// The port of a block's failure after its retries.
pub const ERROR: &str = "error";

/// The port of a `validate` whose value broke nothing.
pub const VALID: &str = "valid";

/// The port of a `validate` whose value broke a rule or a constraint;
/// without an edge, the block fails instead.
pub const INVALID: &str = "invalid";

/// A graph's blocks in order, with what reaches each and where it ends.
pub struct Shape<'a> {
    /// The blocks, each after every block with an edge to it.
    pub order: Vec<&'a Block>,
    /// The edges into each block: the block they leave and its port.
    pub incoming: BTreeMap<&'a str, Vec<(&'a str, &'a str)>>,
    /// The ports by which a run of the graph ends: a block's port, but
    /// `error` and `invalid` (which fail), with no edge.
    pub ends: Vec<(&'a str, String)>,
}

/// The ports a block of kind `kind` leaves by.
pub fn ports(kind: &Kind) -> Vec<String> {
    let mut ports = match kind {
        Kind::Trigger(_) => return vec![NEXT.to_string()],
        Kind::Branch(_) => vec!["then".to_string(), "else".to_string()],
        Kind::Validate(_) => vec![VALID.to_string(), INVALID.to_string()],
        Kind::Switch(switch) => (0..switch.cases.len())
            .map(|n| format!("case:{n}"))
            .chain(["default".to_string()])
            .collect(),
        _ => vec![NEXT.to_string()],
    };
    ports.push(ERROR.to_string());
    ports
}

/// Whether block `from`'s port `port` has an edge, in `graph` or a body or
/// a branch of it.
pub fn has_edge(graph: &Graph, from: &str, port: &str) -> bool {
    graph.edges.iter().any(|e| e.from == from && e.port == port)
        || graph.blocks.iter().any(|b| match &b.kind {
            Some(Kind::ForEach(each)) => {
                each.body.as_ref().is_some_and(|g| has_edge(g, from, port))
            }
            Some(Kind::Parallel(p)) => p.branches.iter().any(|g| has_edge(g, from, port)),
            _ => false,
        })
}

/// The shape of `graph`: `top` for a workflow's graph (its starts its
/// triggers, and every id of it and its bodies and branches unique), else
/// a body's or a branch's (one start, no trigger).
///
/// # Errors
/// The first rule it breaks.
pub fn shape(graph: &Graph, top: bool) -> Result<Shape<'_>, Refusal> {
    if top {
        unique(graph, &mut BTreeSet::new())?;
    }
    let mut blocks = BTreeMap::new();
    for block in &graph.blocks {
        let kind = block
            .kind
            .as_ref()
            .ok_or_else(|| Refusal::block(&block.id, "kind", "a block has a kind"))?;
        if !top && matches!(kind, Kind::Trigger(_)) {
            return Err(Refusal::block(
                &block.id,
                "kind",
                "a trigger starts a workflow, not a body or a branch",
            ));
        }
        blocks.insert(block.id.as_str(), (block, ports(kind)));
    }
    let mut incoming: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    let mut outgoing: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut taken = BTreeSet::new();
    for (i, edge) in graph.edges.iter().enumerate() {
        let at = format!("edges[{i}]");
        let port = if edge.port.is_empty() {
            NEXT
        } else {
            edge.port.as_str()
        };
        let (_, ports) = blocks.get(edge.from.as_str()).ok_or_else(|| {
            Refusal::workflow(
                &at,
                format!("leaves \"{}\", no block of its graph", edge.from),
            )
        })?;
        if !blocks.contains_key(edge.to.as_str()) {
            return Err(Refusal::workflow(
                &at,
                format!("reaches \"{}\", no block of its graph", edge.to),
            ));
        }
        if !ports.iter().any(|p| p == port) {
            return Err(Refusal::block(
                &edge.from,
                &at,
                format!("has no port \"{port}\" (its ports: {})", ports.join(", ")),
            ));
        }
        if !taken.insert((edge.from.as_str(), port)) {
            return Err(Refusal::block(
                &edge.from,
                &at,
                format!("its port {port} has an edge already"),
            ));
        }
        incoming
            .entry(edge.to.as_str())
            .or_default()
            .push((edge.from.as_str(), port));
        outgoing
            .entry(edge.from.as_str())
            .or_default()
            .push(edge.to.as_str());
    }
    let starts: Vec<&Block> = graph
        .blocks
        .iter()
        .filter(|b| !incoming.contains_key(b.id.as_str()))
        .collect();
    starts_hold(graph, top, &starts, &incoming, &outgoing)?;
    let order = ordered(graph, &starts, &incoming, &outgoing)?;
    let ends = order
        .iter()
        .flat_map(|block| {
            let (_, ports) = &blocks[block.id.as_str()];
            ports
                .iter()
                .filter(|p| *p != ERROR && *p != INVALID)
                .filter(|p| !taken.contains(&(block.id.as_str(), p.as_str())))
                .map(|p| (block.id.as_str(), p.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    Ok(Shape {
        order,
        incoming,
        ends,
    })
}

/// That the starts are what `top` asks: at the top its triggers, at
/// least one, none reached by an edge, each leading on; in a body or a
/// branch exactly one block.
fn starts_hold(
    graph: &Graph,
    top: bool,
    starts: &[&Block],
    incoming: &BTreeMap<&str, Vec<(&str, &str)>>,
    outgoing: &BTreeMap<&str, Vec<&str>>,
) -> Result<(), Refusal> {
    let trigger = |b: &Block| matches!(b.kind, Some(Kind::Trigger(_)));
    if !top {
        return match starts {
            [_] => Ok(()),
            [] if graph.blocks.is_empty() => {
                Err(Refusal::workflow("blocks", "a body or a branch has blocks"))
            }
            [] => Err(Refusal::workflow(
                "edges",
                "a body or a branch has one start, and a cycle has none",
            )),
            [first, ..] => Err(Refusal::block(
                &first.id,
                "edges",
                format!(
                    "a body or a branch has one start, this one {}: {}",
                    starts.len(),
                    starts
                        .iter()
                        .map(|b| b.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
        };
    }
    if !graph.blocks.iter().any(trigger) {
        return Err(Refusal::workflow("graph", "a workflow has a trigger"));
    }
    if let Some(start) = starts.iter().find(|b| !trigger(b)) {
        return Err(Refusal::block(
            &start.id,
            "edges",
            "no edge reaches it, and only a trigger starts a run",
        ));
    }
    if let Some(reached) = graph
        .blocks
        .iter()
        .find(|b| trigger(b) && incoming.contains_key(b.id.as_str()))
    {
        return Err(Refusal::block(
            &reached.id,
            "edges",
            "a trigger starts a run; no edge reaches it",
        ));
    }
    if let Some(idle) = starts
        .iter()
        .find(|b| !outgoing.contains_key(b.id.as_str()))
    {
        return Err(Refusal::block(
            &idle.id,
            "edges",
            "a trigger leads to a block",
        ));
    }
    Ok(())
}

/// The blocks in an order where each comes after every block with an edge
/// to it.
///
/// # Errors
/// A cycle, naming a block on it (a loop is only a `for_each`).
fn ordered<'a>(
    graph: &'a Graph,
    starts: &[&'a Block],
    incoming: &BTreeMap<&str, Vec<(&str, &str)>>,
    outgoing: &BTreeMap<&str, Vec<&str>>,
) -> Result<Vec<&'a Block>, Refusal> {
    let by_id: BTreeMap<&str, &Block> = graph.blocks.iter().map(|b| (b.id.as_str(), b)).collect();
    let mut waiting: BTreeMap<&str, usize> = incoming
        .iter()
        .map(|(id, from)| (*id, from.len()))
        .collect();
    let mut ready: Vec<&Block> = starts.to_vec();
    let mut order = Vec::with_capacity(graph.blocks.len());
    while let Some(block) = ready.pop() {
        order.push(block);
        for next in outgoing.get(block.id.as_str()).into_iter().flatten() {
            if let Some(left) = waiting.get_mut(next) {
                *left -= 1;
                if *left == 0 {
                    waiting.remove(next);
                    ready.push(by_id[next]);
                }
            }
        }
    }
    match waiting.keys().next() {
        Some(id) => Err(Refusal::block(
            id,
            "edges",
            "is on a cycle; a loop is only a for_each",
        )),
        None => Ok(order),
    }
}

/// That every position of `layout` (the editor's, by block id) names a
/// block of `graph`, its bodies or its branches, at a finite point.
///
/// # Errors
/// The first position, by id, that breaks it; as [`shape`]'s for the ids.
pub fn placed<'a>(
    graph: &Graph,
    layout: impl IntoIterator<Item = (&'a String, &'a Point)>,
) -> Result<(), Refusal> {
    let mut ids = BTreeSet::new();
    unique(graph, &mut ids)?;
    let mut layout: Vec<_> = layout.into_iter().collect();
    layout.sort_by_key(|(id, _)| *id);
    for (id, point) in layout {
        if !ids.contains(id.as_str()) {
            return Err(Refusal::workflow(
                format!("layout.{id}"),
                "names no block of the workflow",
            ));
        }
        if !point.x.is_finite() || !point.y.is_finite() {
            return Err(Refusal::workflow(
                format!("layout.{id}"),
                "is not a finite point",
            ));
        }
    }
    Ok(())
}

/// That every block's id in `graph`, its bodies and its branches is one
/// (a lowercase letter, then a-z, 0-9 and '_', at most 64) and unique.
fn unique<'a>(graph: &'a Graph, seen: &mut BTreeSet<&'a str>) -> Result<(), Refusal> {
    for block in &graph.blocks {
        let id = block.id.as_str();
        let mut bytes = id.bytes();
        let formed = id.len() <= MAX_ID
            && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if !formed {
            return Err(Refusal::workflow(
                "blocks",
                format!(
                    "\"{id}\" is no block id: a lowercase letter, then a-z, 0-9 and _, at most {MAX_ID}"
                ),
            ));
        }
        if !seen.insert(id) {
            return Err(Refusal::block(id, "id", "is the id of another block"));
        }
        match &block.kind {
            Some(Kind::ForEach(each)) => {
                if let Some(body) = &each.body {
                    unique(body, seen)?;
                }
            }
            Some(Kind::Parallel(parallel)) => {
                for branch in &parallel.branches {
                    unique(branch, seen)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}
