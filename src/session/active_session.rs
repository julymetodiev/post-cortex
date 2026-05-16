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
use crate::core::context_update::{ContextUpdate, EntityType, UpdateType};
use crate::core::structured_context::StructuredContext;
use crate::graph::entity_graph::SimpleEntityGraph;
use crate::session::session_components::{HotContext, SessionMetadata};

use chrono::DateTime;
use chrono::Utc;
use dashmap::DashSet;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

/// ActiveSession with lock-free granular components
/// Uses Arc-wrapped lock-free structures for concurrent access and cheap cloning
///
/// **Copy-on-Write (CoW) Semantics:**
/// Heavy fields are wrapped in Arc for efficient cloning. When the session needs
/// to be modified, use `Arc::make_mut()` which will:
/// - Return a mutable reference if this is the only owner
/// - Clone the data only if there are other owners
///
/// This dramatically reduces cloning overhead when sessions are frequently updated.
#[derive(Clone, Debug)]
pub struct ActiveSession {
    // Metadata (immutable or rare updates)
    pub metadata: Arc<SessionMetadata>,
    pub last_updated: DateTime<Utc>,

    // Lock-free tiered context storage
    pub hot_context: Arc<HotContext>, // Lock-free hot updates (DashMap-based)
    pub warm_context: Arc<Vec<CompressedUpdate>>, // CoW: Compressed updates (storage)
    pub cold_context: Arc<Vec<StructuredSummary>>, // CoW: Periodic summaries (storage)

    // Structured context - CoW wrapped for efficient updates
    pub current_state: Arc<StructuredContext>, // CoW: Current queryable state
    pub incremental_updates: Arc<Vec<ContextUpdate>>, // CoW: All incremental updates (biggest!)

    // Code integration - CoW wrapped
    pub code_references: Arc<HashMap<String, Vec<CodeReference>>>, // CoW: By file path
    pub change_history: Arc<Vec<ChangeRecord>>,                    // CoW: Change history

    // Entity graph - CoW wrapped for efficient graph updates
    pub entity_graph: Arc<SimpleEntityGraph>,

    // Vectorization tracking (lock-free set for concurrent access)
    pub vectorized_update_ids: Arc<DashSet<Uuid>>,
}

// Serialization helper - contains data in serializable form
#[derive(Serialize, Deserialize)]
struct ActiveSessionData {
    id: Uuid,
    name: Option<String>,
    description: Option<String>,
    created_at: DateTime<Utc>,
    last_updated: DateTime<Utc>,
    user_preferences: UserPreferences,
    hot_context: VecDeque<ContextUpdate>,
    warm_context: Vec<CompressedUpdate>,
    cold_context: Vec<StructuredSummary>,
    current_state: StructuredContext,
    incremental_updates: Vec<ContextUpdate>,
    code_references: HashMap<String, Vec<CodeReference>>,
    change_history: Vec<ChangeRecord>,
    entity_graph: SimpleEntityGraph,
    #[serde(default)]
    vectorized_update_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CompressedUpdate {
    pub update: ContextUpdate,
    pub compression_ratio: f32,
    pub compressed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StructuredSummary {
    pub summary_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub context_snapshot: StructuredContext,
    pub referenced_updates: Vec<Uuid>,
    pub summary_quality: f32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CodeReference {
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub code_snippet: String,
    pub commit_hash: Option<String>,
    pub branch: Option<String>,
    pub change_description: String,
}

// Remove duplicate CodeReference definition since we're using the one from core
// Removed duplicate field declarations - using CodeReference from core::context_update
// No extra closing brace needed here

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ChangeRecord {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub change_type: String,
    pub description: String,
    pub related_update_id: Option<Uuid>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UserPreferences {
    pub auto_save_enabled: bool,
    pub context_retention_days: u32,
    pub max_hot_context_size: usize,
    pub auto_summary_threshold: usize,
    pub important_keywords: Vec<String>,
}

// Custom Serialize implementation - extract data from Arc-wrapped components
impl Serialize for ActiveSession {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Dereference Arc before cloning for serialization
        let data = ActiveSessionData {
            id: self.metadata.id,
            name: self.metadata.name.clone(),
            description: self.metadata.description.clone(),
            created_at: self.metadata.created_at,
            last_updated: self.last_updated,
            user_preferences: self.metadata.user_preferences.clone(),
            hot_context: VecDeque::from(self.hot_context.snapshot()),
            warm_context: (*self.warm_context).clone(),
            cold_context: (*self.cold_context).clone(),
            current_state: (*self.current_state).clone(),
            incremental_updates: (*self.incremental_updates).clone(),
            code_references: (*self.code_references).clone(),
            change_history: (*self.change_history).clone(),
            entity_graph: (*self.entity_graph).clone(),
            vectorized_update_ids: self.vectorized_update_ids.iter().map(|id| *id).collect(),
        };
        data.serialize(serializer)
    }
}

// Custom Deserialize implementation - reconstruct Arc-wrapped components
impl<'de> Deserialize<'de> for ActiveSession {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let data = ActiveSessionData::deserialize(deserializer)?;

        let max_hot_size = data.user_preferences.max_hot_context_size;

        let metadata = Arc::new(SessionMetadata::new(
            data.id,
            data.name,
            data.description,
            data.user_preferences,
        ));

        let hot_context = Arc::new(HotContext::from_deque(data.hot_context, max_hot_size));

        // Reconstruct vectorized_update_ids DashSet from Vec
        let vectorized_ids = Arc::new(DashSet::new());
        for id in data.vectorized_update_ids {
            vectorized_ids.insert(id);
        }

        // Wrap deserialized data in Arc for CoW semantics
        Ok(ActiveSession {
            metadata,
            last_updated: data.last_updated,
            hot_context,
            warm_context: Arc::new(data.warm_context),
            cold_context: Arc::new(data.cold_context),
            current_state: Arc::new(data.current_state),
            incremental_updates: Arc::new(data.incremental_updates),
            code_references: Arc::new(data.code_references),
            change_history: Arc::new(data.change_history),
            entity_graph: Arc::new(data.entity_graph),
            vectorized_update_ids: vectorized_ids,
        })
    }
}

impl Default for ActiveSession {
    fn default() -> Self {
        Self::new(Uuid::new_v4(), None, None)
    }
}

/// Truncate a string to at most `max_bytes` without splitting a multi-byte
/// UTF-8 character. Finds the largest char boundary ≤ `max_bytes`.
fn truncate_safe(s: &mut String, max_bytes: usize) {
    if s.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

impl ActiveSession {
    pub fn new(id: Uuid, name: Option<String>, description: Option<String>) -> Self {
        let user_preferences = UserPreferences {
            auto_save_enabled: true,
            context_retention_days: 30,
            max_hot_context_size: 50,
            auto_summary_threshold: 100,
            important_keywords: vec![],
        };

        let metadata = Arc::new(SessionMetadata::new(
            id,
            name,
            description,
            user_preferences.clone(),
        ));

        Self {
            metadata,
            last_updated: Utc::now(),
            hot_context: Arc::new(HotContext::new(50)),
            warm_context: Arc::new(Vec::new()),
            cold_context: Arc::new(Vec::new()),
            current_state: Arc::new(StructuredContext::new()),
            incremental_updates: Arc::new(Vec::new()),
            code_references: Arc::new(HashMap::new()),
            change_history: Arc::new(Vec::new()),
            entity_graph: Arc::new(SimpleEntityGraph::new()),
            vectorized_update_ids: Arc::new(DashSet::new()),
        }
    }

    /// Reconstruct an ActiveSession from individual components (used for SurrealDB native storage)
    #[allow(clippy::too_many_arguments)]
    pub fn from_components(
        id: Uuid,
        name: Option<String>,
        description: Option<String>,
        created_at: DateTime<Utc>,
        last_updated: DateTime<Utc>,
        user_preferences: UserPreferences,
        hot_context_vec: Vec<ContextUpdate>,
        warm_context: Vec<CompressedUpdate>,
        cold_context: Vec<StructuredSummary>,
        current_state: StructuredContext,
        incremental_updates: Vec<ContextUpdate>,
        code_references: HashMap<String, Vec<CodeReference>>,
        change_history: Vec<ChangeRecord>,
        entity_graph: SimpleEntityGraph,
        vectorized_update_ids: Vec<Uuid>,
    ) -> Self {
        let max_hot_size = user_preferences.max_hot_context_size;

        let mut metadata = SessionMetadata::new(id, name, description, user_preferences);
        metadata.created_at = created_at;

        let hot_context = HotContext::from_deque(VecDeque::from(hot_context_vec), max_hot_size);

        let vectorized_ids = Arc::new(DashSet::new());
        for vid in vectorized_update_ids {
            vectorized_ids.insert(vid);
        }

        Self {
            metadata: Arc::new(metadata),
            last_updated,
            hot_context: Arc::new(hot_context),
            warm_context: Arc::new(warm_context),
            cold_context: Arc::new(cold_context),
            current_state: Arc::new(current_state),
            incremental_updates: Arc::new(incremental_updates),
            code_references: Arc::new(code_references),
            change_history: Arc::new(change_history),
            entity_graph: Arc::new(entity_graph),
            vectorized_update_ids: vectorized_ids,
        }
    }

    // Convenience getters for metadata fields
    pub fn id(&self) -> Uuid {
        self.metadata.id
    }

    pub fn name(&self) -> Option<String> {
        self.metadata.name.clone()
    }

    pub fn description(&self) -> Option<String> {
        self.metadata.description.clone()
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        self.metadata.created_at
    }

    pub fn user_preferences(&self) -> &UserPreferences {
        &self.metadata.user_preferences
    }

    #[instrument(skip(self, update), fields(session_id = %self.id()))]
    pub async fn add_incremental_update(&mut self, update: ContextUpdate) -> anyhow::Result<()> {
        info!(
            "ActiveSession: Starting add_incremental_update for update ID: {}",
            update.id
        );
        info!("Update type: {:?}", update.update_type);
        info!("Content title: '{}'", update.content.title);
        info!("Content description: '{}'", update.content.description);

        // Limit content size to prevent processing issues
        let mut limited_update = update.clone();
        if limited_update.content.description.len() > 2000 {
            truncate_safe(&mut limited_update.content.description, 1800);
            limited_update
                .content
                .description
                .push_str("... (truncated)");
            warn!("ActiveSession: Content description truncated to prevent timeout");
        }
        if limited_update.content.title.len() > 200 {
            truncate_safe(&mut limited_update.content.title, 190);
            limited_update.content.title.push_str("...");
        }

        self.hot_context.push(limited_update.clone());

        match timeout(
            Duration::from_secs(3),
            self.update_current_state(&limited_update),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                warn!("ActiveSession: update_current_state timed out");
                return Err(anyhow::anyhow!("Current state update timeout"));
            }
        }

        match timeout(
            Duration::from_secs(5),
            self.update_entity_graph(&limited_update),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                warn!("ActiveSession: update_entity_graph timed out");
                return Err(anyhow::anyhow!("Entity graph update timeout"));
            }
        }

        if let Some(code_ref) = &limited_update.related_code {
            debug!("ActiveSession: Code reference found, processing...");
            let code_ref_clone = CodeReference {
                file_path: code_ref.file_path.clone(),
                start_line: code_ref.start_line,
                end_line: code_ref.end_line,
                code_snippet: code_ref.code_snippet.clone(),
                commit_hash: code_ref.commit_hash.clone(),
                branch: code_ref.branch.clone(),
                change_description: code_ref.change_description.clone(),
            };
            debug!("ActiveSession: Calling add_code_reference");
            match timeout(
                Duration::from_secs(2),
                self.add_code_reference(&code_ref_clone),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => {
                    warn!("ActiveSession: add_code_reference timed out");
                    // Continue without failing the entire operation
                }
            }
            debug!("ActiveSession: add_code_reference completed");
        } else {
            debug!("ActiveSession: No code reference in update");
        }

        self.record_change(&limited_update)?;
        self.maintain_context()?;
        self.last_updated = Utc::now();

        // Add to incremental updates (use original update for storage)
        // Use Arc::make_mut for CoW semantics - only clones if there are other owners
        Arc::make_mut(&mut self.incremental_updates).push(limited_update.clone());

        info!("ActiveSession: add_incremental_update completed successfully");

        Ok(())
    }

    /// Fast path: same as add_incremental_update but skips update_entity_graph.
    /// Entity graph update should be applied separately via apply_entity_graph_update.
    pub async fn add_incremental_update_fast(
        &mut self,
        update: ContextUpdate,
    ) -> anyhow::Result<()> {
        debug!(
            "ActiveSession: Starting add_incremental_update_fast for update ID: {}",
            update.id
        );

        // Limit content size to prevent processing issues
        let mut limited_update = update.clone();
        if limited_update.content.description.len() > 2000 {
            truncate_safe(&mut limited_update.content.description, 1800);
            limited_update
                .content
                .description
                .push_str("... (truncated)");
            warn!("ActiveSession: Content description truncated to prevent timeout");
        }
        if limited_update.content.title.len() > 200 {
            truncate_safe(&mut limited_update.content.title, 190);
            limited_update.content.title.push_str("...");
        }

        // Add to hot context (lock-free)
        self.hot_context.push(limited_update.clone());

        // Update structured state with timeout
        match timeout(
            Duration::from_secs(3),
            self.update_current_state(&limited_update),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                warn!("ActiveSession: update_current_state timed out");
                return Err(anyhow::anyhow!("Current state update timeout"));
            }
        }

        // Add code reference if present with timeout
        if let Some(code_ref) = &limited_update.related_code {
            let code_ref_clone = CodeReference {
                file_path: code_ref.file_path.clone(),
                start_line: code_ref.start_line,
                end_line: code_ref.end_line,
                code_snippet: code_ref.code_snippet.clone(),
                commit_hash: code_ref.commit_hash.clone(),
                branch: code_ref.branch.clone(),
                change_description: code_ref.change_description.clone(),
            };
            match timeout(
                Duration::from_secs(2),
                self.add_code_reference(&code_ref_clone),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => {
                    warn!("ActiveSession: add_code_reference timed out");
                }
            }
        }

        // Record change + maintain context (sync, cheap)
        self.record_change(&limited_update)?;
        self.maintain_context()?;
        self.last_updated = Utc::now();

        // Add to incremental updates (CoW)
        Arc::make_mut(&mut self.incremental_updates).push(limited_update);

        debug!("ActiveSession: add_incremental_update_fast completed successfully");

        Ok(())
    }

    /// Apply entity graph update only. Used as background task after CAS success.
    pub async fn apply_entity_graph_update(
        &mut self,
        update: &ContextUpdate,
    ) -> anyhow::Result<()> {
        match timeout(Duration::from_secs(5), self.update_entity_graph(update)).await {
            Ok(result) => result?,
            Err(_) => {
                warn!("ActiveSession: background update_entity_graph timed out");
            }
        }
        Ok(())
    }

    /// Remove a single incremental update by its ContextUpdate.id (entry_id).
    ///
    /// Cleans up the hot context cache, the warm-compressed cache, and the
    /// canonical `incremental_updates` vector. Returns true if the entry existed.
    /// Caller is responsible for persisting the session and (optionally) rebuilding
    /// the entity graph if relations from this update should be revoked.
    pub fn remove_update_by_id(&mut self, entry_id: &Uuid) -> bool {
        let hot_removed = self.hot_context.remove_by_id(entry_id);

        let warm = Arc::make_mut(&mut self.warm_context);
        let before = warm.len();
        warm.retain(|c| c.update.id != *entry_id);
        let warm_removed = before != warm.len();

        let incr = Arc::make_mut(&mut self.incremental_updates);
        let before = incr.len();
        incr.retain(|u| u.id != *entry_id);
        let incr_removed = before != incr.len();

        hot_removed || warm_removed || incr_removed
    }

    /// Remove incremental updates whose `related_code.file_path` matches `file_path`.
    /// Also removes the corresponding code_references entry.
    /// Returns the number of updates removed.
    pub fn remove_updates_for_file(&mut self, file_path: &str) -> usize {
        let before = self.incremental_updates.len();
        let updates = Arc::make_mut(&mut self.incremental_updates);
        updates.retain(|u| {
            u.related_code
                .as_ref()
                .map_or(true, |cr| cr.file_path != file_path)
        });
        let removed = before - updates.len();
        if removed > 0 {
            // Also clean up the code_references index
            let code_refs = Arc::make_mut(&mut self.code_references);
            code_refs.remove(file_path);
            info!(
                "Removed {} updates referencing file: {}",
                removed, file_path
            );
        }
        removed
    }

    /// Rebuild the entity graph by clearing it and replaying all updates through NER extraction.
    /// Returns (entities_before, entities_after) counts.
    pub async fn rebuild_entity_graph_from_updates(&mut self) -> anyhow::Result<(usize, usize)> {
        let entity_graph = Arc::make_mut(&mut self.entity_graph);
        let entities_before = entity_graph.entity_count();
        entity_graph.clear();

        let updates: Vec<ContextUpdate> = self.incremental_updates.as_ref().clone();
        let total = updates.len();
        info!(
            "Rebuilding entity graph: {} updates to process, {} entities cleared",
            total, entities_before
        );

        for (i, update) in updates.iter().enumerate() {
            if (i + 1) % 10 == 0 || i + 1 == total {
                info!(
                    "Rebuilding entity graph: {}/{} updates processed",
                    i + 1,
                    total
                );
            }
            self.update_entity_graph(update).await?;
        }

        let entities_after = self.entity_graph.entity_count();
        info!(
            "Entity graph rebuild complete: {} -> {} entities",
            entities_before, entities_after
        );
        Ok((entities_before, entities_after))
    }

    async fn update_current_state(&mut self, update: &ContextUpdate) -> anyhow::Result<()> {
        // Use Arc::make_mut for CoW semantics on current_state
        // This will only clone if there are other Arc references
        let current_state = Arc::make_mut(&mut self.current_state);

        // Update structured context based on update type
        match &update.update_type {
            UpdateType::QuestionAnswered => {
                // Add question to open questions (since Q&A implies there was a question)
                current_state
                    .open_questions
                    .push(crate::core::structured_context::QuestionItem {
                        question: update.content.title.clone(),
                        context: update.content.description.clone(),
                        status: crate::core::structured_context::QuestionStatus::Answered,
                        timestamp: update.timestamp,
                        last_updated: update.timestamp,
                    });

                // Add to conversation flow
                current_state
                    .conversation_flow
                    .push(crate::core::structured_context::FlowItem {
                        step_description: format!("Q&A: {}", update.content.title),
                        timestamp: update.timestamp,
                        related_updates: vec![update.id],
                        outcome: Some(update.content.description.clone()),
                    });
            }
            UpdateType::ProblemSolved => {
                // Add to conversation flow
                current_state
                    .conversation_flow
                    .push(crate::core::structured_context::FlowItem {
                        step_description: format!("Problem Solved: {}", update.content.title),
                        timestamp: update.timestamp,
                        related_updates: vec![update.id],
                        outcome: Some(update.content.description.clone()),
                    });
            }
            UpdateType::CodeChanged => {
                // Add to conversation flow
                current_state
                    .conversation_flow
                    .push(crate::core::structured_context::FlowItem {
                        step_description: format!("Code Change: {}", update.content.title),
                        timestamp: update.timestamp,
                        related_updates: vec![update.id],
                        outcome: Some(update.content.description.clone()),
                    });
            }
            UpdateType::DecisionMade => {
                // Add to key decisions
                current_state
                    .key_decisions
                    .push(crate::core::structured_context::DecisionItem {
                        description: update.content.title.clone(),
                        context: update.content.description.clone(),
                        alternatives: update.content.details.clone(),
                        confidence: 1.0,
                        timestamp: update.timestamp,
                    });

                // Add to conversation flow
                current_state
                    .conversation_flow
                    .push(crate::core::structured_context::FlowItem {
                        step_description: format!("Decision Made: {}", update.content.title),
                        timestamp: update.timestamp,
                        related_updates: vec![update.id],
                        outcome: Some(update.content.description.clone()),
                    });
            }
            UpdateType::ConceptDefined => {
                // Add to key concepts
                current_state
                    .key_concepts
                    .push(crate::core::structured_context::ConceptItem {
                        name: update.content.title.clone(),
                        definition: update.content.description.clone(),
                        examples: update.content.examples.clone(),
                        related_concepts: update.content.details.clone(),
                        timestamp: update.timestamp,
                    });

                // Add to conversation flow
                current_state
                    .conversation_flow
                    .push(crate::core::structured_context::FlowItem {
                        step_description: format!("Concept Defined: {}", update.content.title),
                        timestamp: update.timestamp,
                        related_updates: vec![update.id],
                        outcome: Some(update.content.description.clone()),
                    });
            }
            UpdateType::RequirementAdded => {
                // Add to technical specifications
                current_state.technical_specifications.push(
                    crate::core::structured_context::SpecItem {
                        title: update.content.title.clone(),
                        description: update.content.description.clone(),
                        requirements: update.content.details.clone(),
                        constraints: update.content.implications.clone(),
                        timestamp: update.timestamp,
                    },
                );

                // Add to conversation flow
                current_state
                    .conversation_flow
                    .push(crate::core::structured_context::FlowItem {
                        step_description: format!("Requirement Added: {}", update.content.title),
                        timestamp: update.timestamp,
                        related_updates: vec![update.id],
                        outcome: Some(update.content.description.clone()),
                    });
            }
        }

        Ok(())
    }

    async fn update_entity_graph(&mut self, update: &ContextUpdate) -> anyhow::Result<()> {
        info!(
            "update_entity_graph: Starting entity graph update for update {}",
            update.id
        );

        // When typed entities are provided (from Claude), use them directly — skip NER entirely.
        if !update.typed_entities.is_empty() {
            let entity_graph = Arc::make_mut(&mut self.entity_graph);

            for typed_entity in &update.typed_entities {
                entity_graph.add_or_update_entity(
                    typed_entity.name.clone(),
                    typed_entity.entity_type.clone(),
                    update.timestamp,
                    &format!("Provided by caller: {}", update.content.title),
                );
            }

            for rel in &update.creates_relationships {
                entity_graph.add_relationship(rel.clone());
            }

            info!(
                "update_entity_graph: Used {} caller-provided entities, {} relationships (NER skipped)",
                update.typed_entities.len(),
                update.creates_relationships.len(),
            );
            return Ok(());
        }

        // Untyped entity strings (legacy path — no extraction, just add what's given)
        let entity_graph = Arc::make_mut(&mut self.entity_graph);
        for name in &update.creates_entities {
            entity_graph.add_or_update_entity(
                name.clone(),
                EntityType::Concept,
                update.timestamp,
                &format!("From update: {}", update.content.title),
            );
        }
        for rel in &update.creates_relationships {
            entity_graph.add_relationship(rel.clone());
        }

        debug!("update_entity_graph: completed successfully");
        Ok(())
    }

    async fn add_code_reference(&mut self, code_ref: &CodeReference) -> anyhow::Result<()> {
        // Use Arc::make_mut for CoW semantics on code_references
        let code_references = Arc::make_mut(&mut self.code_references);
        let code_refs = code_references
            .entry(code_ref.file_path.clone())
            .or_default();
        code_refs.push(code_ref.clone());
        Ok(())
    }

    fn record_change(&mut self, update: &ContextUpdate) -> anyhow::Result<()> {
        // Use Arc::make_mut for CoW semantics on change_history
        Arc::make_mut(&mut self.change_history).push(ChangeRecord {
            id: Uuid::new_v4(),
            timestamp: update.timestamp,
            change_type: format!("{:?}", update.update_type),
            description: update.content.description.clone(),
            related_update_id: Some(update.id),
        });
        Ok(())
    }

    fn maintain_context(&mut self) -> anyhow::Result<()> {
        // Note: HotContext now manages its own capacity automatically
        // No need to manually move to warm - capacity is enforced on push

        // Create summary if needed
        if self.should_create_summary() {
            self.create_periodic_summary()?;
        }

        Ok(())
    }

    fn should_create_summary(&self) -> bool {
        let threshold = self.metadata.user_preferences.auto_summary_threshold;
        let len = self.incremental_updates.len();
        // Guard against threshold=0 (is_multiple_of(0) returns true for any number)
        threshold > 0 && len > 0 && len % threshold == 0
    }

    fn create_periodic_summary(&mut self) -> anyhow::Result<()> {
        // Create a summary from current state
        // Note: Arc clone is cheap (just ref count increment), and we need immutable access
        let summary = StructuredSummary {
            summary_id: Uuid::new_v4(),
            created_at: Utc::now(),
            context_snapshot: (*self.current_state).clone(),
            referenced_updates: self.incremental_updates.iter().map(|u| u.id).collect(),
            summary_quality: 1.0, // Placeholder - would calculate actual quality
        };

        // Use Arc::make_mut for CoW semantics on cold_context
        Arc::make_mut(&mut self.cold_context).push(summary);
        Ok(())
    }

    /// Update the session name (preserves `created_at`).
    pub fn set_name(&mut self, name: Option<String>) {
        let new_metadata = Arc::new(SessionMetadata::with_created_at(
            self.metadata.id,
            name,
            self.metadata.description.clone(),
            self.metadata.user_preferences.clone(),
            self.metadata.created_at,
        ));
        self.metadata = new_metadata;
        self.last_updated = Utc::now();
    }

    /// Update the session description (preserves `created_at`).
    pub fn set_description(&mut self, description: Option<String>) {
        let new_metadata = Arc::new(SessionMetadata::with_created_at(
            self.metadata.id,
            self.metadata.name.clone(),
            description,
            self.metadata.user_preferences.clone(),
            self.metadata.created_at,
        ));
        self.metadata = new_metadata;
        self.last_updated = Utc::now();
    }

    /// Update both name and description (preserves existing values if None
    /// provided, and always preserves the original `created_at`).
    pub fn update_metadata(&mut self, name: Option<String>, description: Option<String>) {
        let final_name = if name.is_some() {
            name
        } else {
            self.metadata.name.clone()
        };
        let final_description = if description.is_some() {
            description
        } else {
            self.metadata.description.clone()
        };

        let new_metadata = Arc::new(SessionMetadata::with_created_at(
            self.metadata.id,
            final_name,
            final_description,
            self.metadata.user_preferences.clone(),
            self.metadata.created_at,
        ));
        self.metadata = new_metadata;
        self.last_updated = Utc::now();
    }

    /// Get the current name and description
    pub fn get_metadata(&self) -> (Option<String>, Option<String>) {
        (
            self.metadata.name.clone(),
            self.metadata.description.clone(),
        )
    }

    /// Build a `ContextUpdate` from a description (or deserialize one from caller metadata)
    /// and append it via `add_incremental_update_fast`. Returns `(update_id, update)`.
    pub async fn add_context_update(
        &mut self,
        description: String,
        metadata: Option<serde_json::Value>,
    ) -> Result<(String, ContextUpdate), String> {
        use crate::core::context_update::{UpdateContent, UpdateType};

        let update = if let Some(metadata) = metadata {
            serde_json::from_value::<ContextUpdate>(metadata)
                .map_err(|e| format!("Invalid ContextUpdate metadata: {e}"))?
        } else {
            ContextUpdate {
                id: Uuid::new_v4(),
                update_type: UpdateType::ConceptDefined,
                content: UpdateContent {
                    title: "Incremental Update".to_string(),
                    description,
                    details: Vec::new(),
                    examples: Vec::new(),
                    implications: Vec::new(),
                },
                timestamp: chrono::Utc::now(),
                related_code: None,
                parent_update: None,
                user_marked_important: false,
                creates_entities: Vec::new(),
                creates_relationships: Vec::new(),
                references_entities: Vec::new(),
                typed_entities: Vec::new(),
            }
        };

        let update_id = update.id;
        let update_clone = update.clone();
        self.add_incremental_update_fast(update)
            .await
            .map_err(|e| format!("Failed to add update: {e}"))?;
        Ok((update_id.to_string(), update_clone))
    }

    /// Recent updates from hot context as a human-readable bullet list.
    pub fn context_summary(&self) -> String {
        let mut summary = Vec::new();
        for update in self.hot_context.iter().iter().rev().take(10) {
            summary.push(format!("- {}", update.content.description));
        }
        if summary.is_empty() {
            "No context available".to_string()
        } else {
            summary.join("\n")
        }
    }
}
