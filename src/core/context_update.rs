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
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub enum UpdateType {
    QuestionAnswered, // Q/A pair completed
    ProblemSolved,    // Issue resolved
    CodeChanged,      // File modification
    DecisionMade,     // Decision recorded
    ConceptDefined,   // New concept explained
    RequirementAdded, // New requirement identified
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TypedEntity {
    pub name: String,
    pub entity_type: EntityType,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ContextUpdate {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub update_type: UpdateType,
    pub content: UpdateContent,
    pub related_code: Option<CodeReference>,
    pub parent_update: Option<Uuid>,
    pub user_marked_important: bool,

    // Graph components
    pub creates_entities: Vec<String>,
    pub creates_relationships: Vec<EntityRelationship>,
    pub references_entities: Vec<String>,

    // Claude-provided typed entities (skip NER when non-empty)
    #[serde(default)]
    pub typed_entities: Vec<TypedEntity>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UpdateContent {
    pub title: String,
    pub description: String,
    pub details: Vec<String>,
    pub examples: Vec<String>,
    pub implications: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct CodeReference {
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub code_snippet: String,
    pub commit_hash: Option<String>,
    pub branch: Option<String>,
    pub change_description: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EntityRelationship {
    pub from_entity: String,
    pub to_entity: String,
    pub relation_type: RelationType,
    pub context: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelationType {
    RequiredBy,    // A is required by B
    LeadsTo,       // A leads to B
    RelatedTo,     // A is related to B
    ConflictsWith, // A conflicts with B
    DependsOn,     // A depends on B
    Implements,    // A implements B
    CausedBy,      // A is caused by B
    Solves,        // A solves B
}

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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub enum EntityType {
    Technology,    // Rust, PostgreSQL, etc.
    Concept,       // Authentication, Performance, etc.
    Problem,       // Bug, Issue, etc.
    Solution,      // Fix, Implementation, etc.
    Decision,      // Architecture choice, etc.
    CodeComponent, // File, Function, Module, etc.
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EntityData {
    pub name: String,
    pub entity_type: EntityType,
    pub first_mentioned: DateTime<Utc>,
    pub last_mentioned: DateTime<Utc>,
    pub mention_count: u32,
    pub importance_score: f32,
    pub description: Option<String>,
}
