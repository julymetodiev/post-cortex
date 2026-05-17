// Copyright (c) 2025 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Core data types for context updates, entities, and relationships

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use uuid::Uuid;

/// The type of context update recorded during a session
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub enum UpdateType {
    /// A question-answer pair was completed
    QuestionAnswered,
    /// An issue or problem was resolved
    ProblemSolved,
    /// A source file was modified
    CodeChanged,
    /// An architectural or design decision was recorded
    DecisionMade,
    /// A new concept was explained or defined
    ConceptDefined,
    /// A new requirement was identified
    RequirementAdded,
}

/// A named entity with its semantic type, provided by the LLM
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TypedEntity {
    /// Human-readable entity name
    pub name: String,
    /// Semantic category of the entity
    pub entity_type: EntityType,
}

/// A single context update record capturing a meaningful event in the session
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ContextUpdate {
    /// Unique identifier for this update
    pub id: Uuid,
    /// When this update was created
    pub timestamp: DateTime<Utc>,
    /// Category of the update
    pub update_type: UpdateType,
    /// Structured content describing the update
    pub content: UpdateContent,
    /// Optional source code reference associated with this update
    pub related_code: Option<CodeReference>,
    /// Parent update ID for threaded updates
    pub parent_update: Option<Uuid>,
    /// Whether the user explicitly marked this update as important
    pub user_marked_important: bool,

    /// Entity names created by this update
    pub creates_entities: Vec<String>,
    /// Relationships created by this update
    pub creates_relationships: Vec<EntityRelationship>,
    /// Entity names referenced by this update
    pub references_entities: Vec<String>,

    /// LLM-provided typed entities; skips NER when non-empty
    #[serde(default)]
    pub typed_entities: Vec<TypedEntity>,
}

/// Structured content payload for a context update
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UpdateContent {
    /// Short title summarizing the update
    pub title: String,
    /// Longer description of what happened
    pub description: String,
    /// Supporting details or bullet points
    pub details: Vec<String>,
    /// Illustrative code or text examples
    pub examples: Vec<String>,
    /// Consequences or follow-up actions
    pub implications: Vec<String>,
}

/// Reference to a span of source code
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct CodeReference {
    /// Absolute or workspace-relative file path
    pub file_path: String,
    /// Starting line number (1-based)
    pub start_line: u32,
    /// Ending line number (inclusive)
    pub end_line: u32,
    /// The literal code text at this location
    pub code_snippet: String,
    /// Git commit hash, if available
    pub commit_hash: Option<String>,
    /// Git branch name, if available
    pub branch: Option<String>,
    /// Human-readable description of the change
    pub change_description: String,
}

/// A directed relationship between two named entities
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EntityRelationship {
    /// Name of the source entity
    pub from_entity: String,
    /// Name of the target entity
    pub to_entity: String,
    /// Kind of relationship
    pub relation_type: RelationType,
    /// Free-text context describing why this relationship exists
    pub context: String,
}

/// The kind of directed relationship between two entities
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelationType {
    /// A is required by B
    RequiredBy,
    /// A leads to B
    LeadsTo,
    /// A is generically related to B
    RelatedTo,
    /// A conflicts with B
    ConflictsWith,
    /// A depends on B
    DependsOn,
    /// A implements B
    Implements,
    /// A is caused by B
    CausedBy,
    /// A solves B
    Solves,
}

/// Parses a `RelationType` from its string representation
impl std::str::FromStr for RelationType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "RequiredBy" => Ok(RelationType::RequiredBy),
            "LeadsTo" => Ok(RelationType::LeadsTo),
            "RelatedTo" => Ok(RelationType::RelatedTo),
            "ConflictsWith" => Ok(RelationType::ConflictsWith),
            "DependsOn" => Ok(RelationType::DependsOn),
            "Implements" => Ok(RelationType::Implements),
            "CausedBy" => Ok(RelationType::CausedBy),
            "Solves" => Ok(RelationType::Solves),
            _ => Err(format!("Unknown RelationType: {}", s)),
        }
    }
}

/// Semantic category of a tracked entity
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub enum EntityType {
    /// A technology or tool (Rust, PostgreSQL, etc.)
    Technology,
    /// An abstract concept (Authentication, Performance, etc.)
    Concept,
    /// A problem or bug
    Problem,
    /// A solution or fix
    Solution,
    /// An architectural or design decision
    Decision,
    /// A code artifact (file, function, module, etc.)
    CodeComponent,
}

/// Stored metadata for a tracked entity
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EntityData {
    /// Unique human-readable name
    pub name: String,
    /// Semantic category
    pub entity_type: EntityType,
    /// Timestamp when first seen
    pub first_mentioned: DateTime<Utc>,
    /// Timestamp when last referenced
    pub last_mentioned: DateTime<Utc>,
    /// Number of times this entity has been mentioned
    pub mention_count: u32,
    /// Computed importance score (higher = more relevant)
    pub importance_score: f32,
    /// Optional human-readable description
    pub description: Option<String>,
}
