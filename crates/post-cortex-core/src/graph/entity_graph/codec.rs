// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Serialised form of the graph: entities and mentions as maps in key
//! order, the edges as name-sorted nodes and edges (the shape the graph
//! had over petgraph, so stored and exported bytes are unchanged); the
//! alias index is rebuilt on load.

use crate::collections::{SharedMap, SharedVec};
use crate::core::context_update::{EntityData, RelationType};
use crate::graph::entity_graph::{EdgeData, SimpleEntityGraph};
use serde::{Deserialize, Serialize};

/// Custom deserializer for EdgeData - handles both v1.0.0 (RelationType) and v1.1.0 (EdgeData) formats
impl<'de> serde::Deserialize<'de> for EdgeData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};

        struct EdgeDataVisitor;

        impl<'de> Visitor<'de> for EdgeDataVisitor {
            type Value = EdgeData;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("EdgeData object or RelationType string (v1.0.0 format)")
            }

            // v1.1.0 format: {"relation_type": "...", "context": "..."}
            fn visit_map<M>(self, mut map: M) -> Result<EdgeData, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut relation_type: Option<RelationType> = None;
                let mut context: Option<String> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "relation_type" => relation_type = Some(map.next_value()?),
                        "context" => context = Some(map.next_value()?),
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let relation_type =
                    relation_type.ok_or_else(|| de::Error::missing_field("relation_type"))?;

                Ok(EdgeData {
                    relation_type,
                    context: context.unwrap_or_default(),
                })
            }

            // v1.0.0 format: just "RelationType" string
            fn visit_str<E>(self, value: &str) -> Result<EdgeData, E>
            where
                E: de::Error,
            {
                let relation_type: RelationType = value.parse().map_err(|_| {
                    de::Error::unknown_variant(
                        value,
                        &[
                            "RequiredBy",
                            "LeadsTo",
                            "RelatedTo",
                            "ConflictsWith",
                            "DependsOn",
                            "Implements",
                            "CausedBy",
                            "Solves",
                        ],
                    )
                })?;
                Ok(EdgeData::new(relation_type, String::new()))
            }
        }

        deserializer.deserialize_any(EdgeDataVisitor)
    }
}

/// The graph's edges as stored: node names, sorted, and edges by
/// (source, target, relation type).
#[derive(Deserialize)]
struct GraphData {
    nodes: Vec<String>,
    edges: Vec<(String, String, EdgeData)>,
}

impl Serialize for SimpleEntityGraph {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct GraphRef<'a> {
            nodes: Vec<&'a String>,
            edges: Vec<(&'a str, &'a str, EdgeData)>,
        }
        #[derive(Serialize)]
        struct Ref<'a> {
            entities: &'a SharedMap<String, EntityData>,
            entity_mentions: &'a SharedMap<String, SharedVec<uuid::Uuid>>,
            graph: GraphRef<'a>,
        }
        let mut nodes: Vec<&String> = self.entities.keys_unordered().collect();
        nodes.sort();
        let edges = self
            .relationships()
            .map(|r| {
                (
                    r.from,
                    r.to,
                    EdgeData::new(*r.relation_type, r.context.to_string()),
                )
            })
            .collect();
        Ref {
            entities: &self.entities,
            entity_mentions: &self.entity_mentions,
            graph: GraphRef { nodes, edges },
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SimpleEntityGraph {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Data {
            entities: SharedMap<String, EntityData>,
            entity_mentions: SharedMap<String, SharedVec<uuid::Uuid>>,
            graph: GraphData,
        }

        let data = Data::deserialize(deserializer)?;
        let mut graph = SimpleEntityGraph {
            entities: data.entities,
            entity_mentions: data.entity_mentions,
            ..SimpleEntityGraph::new()
        };
        // An edge is kept when both its ends are nodes of the stored graph.
        let nodes: std::collections::HashSet<&String> = data.graph.nodes.iter().collect();
        for (from, to, edge) in data.graph.edges {
            if nodes.contains(&from) && nodes.contains(&to) {
                graph.put_edge(from, to, edge.relation_type, edge.context);
            }
        }
        graph.rebuild_alias_index();
        Ok(graph)
    }
}
