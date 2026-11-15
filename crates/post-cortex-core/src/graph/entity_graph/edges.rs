// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The edges of the entity graph: what an edge stores and a borrowed view
//! of one.

use crate::core::context_update::{EntityRelationship, RelationType};
use serde::Serialize;

/// Edge data containing relation type and context
/// Supports backwards-compatible deserialization from v1.0.0 format (RelationType only)
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct EdgeData {
    /// Kind of relationship between two entities
    pub relation_type: RelationType,
    /// Free-text context describing this relationship
    #[serde(default)]
    pub context: String,
}

impl EdgeData {
    /// Create new EdgeData with context
    pub fn new(relation_type: RelationType, context: String) -> Self {
        Self {
            relation_type,
            context,
        }
    }
}

/// Borrowed view of one directed relationship edge in a
/// [`SimpleEntityGraph`](crate::graph::entity_graph::SimpleEntityGraph).
///
/// Returned by the O(degree) lookups [`relationships_of`](crate::graph::entity_graph::SimpleEntityGraph::relationships_of)
/// and [`relation_between`](crate::graph::entity_graph::SimpleEntityGraph::relation_between); nothing is cloned until
/// [`RelationRef::to_relationship`] is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelationRef<'a> {
    /// Source entity name
    pub from: &'a str,
    /// Target entity name
    pub to: &'a str,
    /// Kind of relationship
    pub relation_type: &'a RelationType,
    /// Free-text context stored on the edge
    pub context: &'a str,
}

impl RelationRef<'_> {
    /// Owned `EntityRelationship` copy of this edge.
    pub fn to_relationship(&self) -> EntityRelationship {
        EntityRelationship {
            from_entity: self.from.to_string(),
            to_entity: self.to.to_string(),
            relation_type: *self.relation_type,
            context: self.context.to_string(),
        }
    }
}
