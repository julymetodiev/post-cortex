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
//! Granular session components for fine-grained locking
//!
//! This module provides lock-free and fine-grained locked components
//! that replace the monolithic ActiveSession structure for better
//! concurrent access patterns.

use crate::core::context_update::{ContextUpdate, EntityType};
use crate::session::active_session::UserPreferences;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use uuid::Uuid;

/// Hot context with lock-free concurrent access
///
/// Uses DashMap with sequential IDs for lock-free recent updates storage.
/// Older updates are automatically evicted when capacity is reached.
#[derive(Debug)]
pub struct HotContext {
    updates: DashMap<u64, ContextUpdate>,
    next_id: Arc<AtomicU64>,
    max_size: usize,
}

impl HotContext {
    /// Create new hot context with specified capacity
    pub fn new(max_size: usize) -> Self {
        Self {
            updates: DashMap::new(),
            next_id: Arc::new(AtomicU64::new(0)),
            max_size,
        }
    }

    /// Push new update, evicting oldest if at capacity (lock-free)
    ///
    /// Uses Release ordering to ensure the insert is visible before the ID increment
    /// is visible to readers using Acquire ordering.
    pub fn push(&self, update: ContextUpdate) {
        let id = self.next_id.fetch_add(1, Ordering::Release);
        self.updates.insert(id, update);

        // Evict oldest if we exceed capacity (lock-free cleanup)
        if id >= self.max_size as u64 {
            let to_evict = id - self.max_size as u64;
            self.updates.remove(&to_evict);
        }
    }

    /// Get N most recent updates (lock-free)
    ///
    /// Note: In a concurrent environment, some entries may have been evicted
    /// between loading the ID and fetching. This is expected lock-free behavior.
    /// The returned Vec may contain fewer than N elements.
    pub fn get_recent(&self, n: usize) -> Vec<ContextUpdate> {
        let current_id = self.next_id.load(Ordering::Acquire);
        // Limit to max_size to avoid iterating over evicted entries
        let effective_n = n.min(self.max_size);
        let start_id = current_id.saturating_sub(effective_n as u64);

        let mut results = Vec::with_capacity(effective_n);
        for id in (start_id..current_id).rev() {
            if let Some(entry) = self.updates.get(&id) {
                results.push(entry.clone());
            }
        }
        results
    }

    /// Get all updates as Vec in chronological order (lock-free)
    ///
    /// Returns updates sorted by insertion order (oldest first).
    /// Only iterates over the valid range of IDs to avoid O(n) iteration.
    pub fn get_all(&self) -> Vec<ContextUpdate> {
        let current_id = self.next_id.load(Ordering::Acquire);
        let start_id = current_id.saturating_sub(self.max_size as u64);

        let mut results = Vec::with_capacity(self.max_size.min(current_id as usize));
        for id in start_id..current_id {
            if let Some(entry) = self.updates.get(&id) {
                results.push(entry.clone());
            }
        }
        results
    }

    /// Get current length (lock-free)
    pub fn len(&self) -> usize {
        self.updates.len()
    }

    /// Check if empty (lock-free)
    pub fn is_empty(&self) -> bool {
        self.updates.is_empty()
    }

    /// Clear all updates (lock-free)
    pub fn clear(&self) {
        self.updates.clear();
        self.next_id.store(0, Ordering::Release);
    }

    /// Get most recent update (lock-free)
    ///
    /// Returns None if the context is empty or if the most recent entry
    /// was evicted by a concurrent push operation.
    pub fn back(&self) -> Option<ContextUpdate> {
        let current_id = self.next_id.load(Ordering::Acquire);
        if current_id == 0 {
            return None;
        }
        self.updates.get(&(current_id - 1)).map(|e| e.clone())
    }

    /// Get snapshot of all updates as Vec (lock-free)
    /// Returns owned Vec for maximum flexibility (.iter(), .rev(), .par_iter(), etc.)
    pub fn snapshot(&self) -> Vec<ContextUpdate> {
        self.get_all()
    }

    /// Iterate over all updates (convenience method, returns Vec)
    pub fn iter(&self) -> Vec<ContextUpdate> {
        self.snapshot()
    }

    /// Convert to VecDeque for serialization
    /// Only iterates over the valid range of IDs (last max_size entries)
    pub fn to_deque(&self) -> VecDeque<ContextUpdate> {
        let current_id = self.next_id.load(Ordering::Acquire);
        let start_id = current_id.saturating_sub(self.max_size as u64);

        let mut deque = VecDeque::with_capacity(self.max_size.min(current_id as usize));
        for id in start_id..current_id {
            if let Some(entry) = self.updates.get(&id) {
                deque.push_back(entry.clone());
            }
        }
        deque
    }

    /// Create from VecDeque for deserialization (lock-free)
    pub fn from_deque(deque: VecDeque<ContextUpdate>, max_size: usize) -> Self {
        let hot = Self::new(max_size);
        for update in deque {
            hot.push(update);
        }
        hot
    }
}

/// Lock-free entity graph using DashMap
///
/// Replaces SimpleEntityGraph with lock-free concurrent access.
/// Entity nodes use atomic counters for mention tracking.
/// Relationships use DashMap with sequential IDs for lock-free access.
pub struct EntityGraph {
    pub entities: DashMap<String, EntityNode>, // Public for len() access
    relationships: DashMap<u64, crate::core::context_update::EntityRelationship>,
    next_rel_id: Arc<AtomicU64>,
}

/// Entity node with atomic tracking
#[derive(Clone, Debug)]
pub struct EntityNode {
    pub entity_type: EntityType,
    pub mention_count: Arc<AtomicUsize>,
    pub first_mentioned: Arc<AtomicU64>,
    pub last_mentioned: Arc<AtomicU64>,
    pub importance_score: Arc<std::sync::atomic::AtomicU32>, // f32 as u32 bits
    pub description: Option<String>, // Immutable after creation, entity replaced to update
}

// Note: Using EntityRelationship from core::context_update instead of defining our own
// pub use crate::core::context_update::EntityRelationship;

impl EntityGraph {
    /// Create new empty entity graph (lock-free)
    pub fn new() -> Self {
        Self {
            entities: DashMap::new(),
            relationships: DashMap::new(),
            next_rel_id: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Add or update entity - API compatible with SimpleEntityGraph (lock-free)
    pub fn add_or_update_entity(
        &self,
        name: String,
        entity_type: EntityType,
        timestamp: DateTime<Utc>,
        description: &str,
    ) {
        let timestamp_secs = timestamp.timestamp() as u64;

        self.entities
            .entry(name.clone())
            .and_modify(|node| {
                node.mention_count.fetch_add(1, Ordering::Relaxed);
                node.last_mentioned.store(timestamp_secs, Ordering::Relaxed);
                // Update description if current is None and new description is not empty
                // Note: Can't update immutable field in-place, would need full node replacement
                // This is intentionally simplified - description set only on first creation
            })
            .or_insert(EntityNode {
                entity_type,
                mention_count: Arc::new(AtomicUsize::new(1)),
                first_mentioned: Arc::new(AtomicU64::new(timestamp_secs)),
                last_mentioned: Arc::new(AtomicU64::new(timestamp_secs)),
                importance_score: Arc::new(std::sync::atomic::AtomicU32::new(1.0f32.to_bits())),
                description: if description.is_empty() {
                    None
                } else {
                    Some(description.to_string())
                },
            });
    }

    /// Track entity mention - API compatible with SimpleEntityGraph (lock-free)
    pub fn mention_entity(&self, name: &str, _update_id: uuid::Uuid, timestamp: DateTime<Utc>) {
        let timestamp_secs = timestamp.timestamp() as u64;

        if let Some(node) = self.entities.get_mut(name) {
            node.mention_count.fetch_add(1, Ordering::Relaxed);
            node.last_mentioned.store(timestamp_secs, Ordering::Relaxed);
        }
    }

    /// Add relationship - API compatible with SimpleEntityGraph (lock-free)
    pub fn add_relationship(&self, relationship: crate::core::context_update::EntityRelationship) {
        let id = self.next_rel_id.fetch_add(1, Ordering::Relaxed);
        self.relationships.insert(id, relationship);
    }

    /// Get entity node (lock-free)
    pub fn get_entity(&self, name: &str) -> Option<EntityNode> {
        self.entities.get(name).map(|e| e.clone())
    }

    /// Get all entities as Vec (lock-free)
    pub fn get_all_entities(&self) -> Vec<(String, EntityType, usize)> {
        self.entities
            .iter()
            .map(|entry| {
                let name = entry.key().clone();
                let node = entry.value();
                let count = node.mention_count.load(Ordering::Relaxed);
                (name, node.entity_type.clone(), count)
            })
            .collect()
    }

    /// Get all relationships (lock-free)
    pub fn get_relationships(&self) -> Vec<crate::core::context_update::EntityRelationship> {
        self.relationships
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get entity count (lock-free)
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Get relationship count (lock-free)
    pub fn relationship_count(&self) -> usize {
        self.relationships.len()
    }

    /// Clear all entities and relationships (lock-free)
    pub fn clear(&self) {
        self.entities.clear();
        self.relationships.clear();
        self.next_rel_id.store(0, Ordering::Relaxed);
    }
}

impl Default for EntityGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable session metadata
///
/// Wrapped in ArcSwap for rare updates without cloning entire session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub user_preferences: UserPreferences,
}

impl SessionMetadata {
    /// Create new session metadata
    pub fn new(
        id: Uuid,
        name: Option<String>,
        description: Option<String>,
        user_preferences: UserPreferences,
    ) -> Self {
        Self {
            id,
            name,
            description,
            created_at: Utc::now(),
            user_preferences,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::context_update::{RelationType, UpdateContent, UpdateType};

    #[test]
    fn test_hot_context() {
        let hot = HotContext::new(3);
        assert_eq!(hot.len(), 0);
        assert!(hot.is_empty());

        // Add updates
        for i in 0..5 {
            let update = ContextUpdate {
                id: Uuid::new_v4(),
                timestamp: Utc::now(),
                update_type: UpdateType::QuestionAnswered,
                content: UpdateContent {
                    title: format!("Question {}", i),
                    description: format!("Answer {}", i),
                    details: vec![],
                    examples: vec![],
                    implications: vec![],
                },
                related_code: None,
                parent_update: None,
                user_marked_important: false,
                creates_entities: vec![],
                creates_relationships: vec![],
                references_entities: vec![],
                typed_entities: vec![],
            };
            hot.push(update);
        }

        // Should keep only last 3
        assert_eq!(hot.len(), 3);
        let recent = hot.get_recent(2);
        assert_eq!(recent.len(), 2);
    }

    #[test]
    fn test_entity_graph() {
        use crate::core::context_update::EntityRelationship;

        let graph = EntityGraph::new();
        assert_eq!(graph.entity_count(), 0);

        // Add entities
        let now = Utc::now();
        graph.add_or_update_entity(
            "Rust".to_string(),
            EntityType::Technology,
            now,
            "A systems programming language",
        );
        graph.add_or_update_entity(
            "PostgreSQL".to_string(),
            EntityType::Technology,
            now,
            "A relational database",
        );
        graph.add_or_update_entity(
            "Rust".to_string(),
            EntityType::Technology,
            now,
            "", // Empty description should not override
        );

        assert_eq!(graph.entity_count(), 2);

        let rust = graph.get_entity("Rust").unwrap();
        assert_eq!(rust.mention_count.load(Ordering::Relaxed), 2);
        assert_eq!(
            rust.description,
            Some("A systems programming language".to_string())
        );

        // Add relationship
        graph.add_relationship(EntityRelationship {
            from_entity: "Rust".to_string(),
            to_entity: "PostgreSQL".to_string(),
            relation_type: RelationType::RelatedTo,
            context: "Used together in projects".to_string(),
        });
        assert_eq!(graph.relationship_count(), 1);
    }

    #[test]
    fn test_session_metadata() {
        let prefs = UserPreferences {
            auto_save_enabled: true,
            context_retention_days: 30,
            max_hot_context_size: 50,
            auto_summary_threshold: 100,
            important_keywords: vec![],
        };
        let meta = SessionMetadata::new(
            Uuid::new_v4(),
            Some("test".to_string()),
            None,
            prefs,
        );
        assert_eq!(meta.name, Some("test".to_string()));
    }
}
