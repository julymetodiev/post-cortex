// Copyright (c) 2025, 2026 Julius ML
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

//! Entity/relationship persistence and the [`GraphStorage`] trait
//! implementation (BFS-based graph traversal on bincode-encoded records).

use anyhow::Result;
use async_trait::async_trait;
use rocksdb::WriteBatch;
use std::collections::{BTreeMap, HashMap};
use tracing::debug;
use uuid::Uuid;

use crate::core::context_update::{EntityData, EntityRelationship, RelationType};
use crate::graph::entity_graph::EntityNetwork;
use crate::storage::traits::GraphStorage;

use super::RealRocksDBStorage;
use super::types::{StoredEntity, StoredRelationship};

impl RealRocksDBStorage {
    /// Generate key for entity storage
    fn entity_key(session_id: Uuid, entity_name: &str) -> String {
        format!("entity:{}:{}", session_id, entity_name)
    }

    /// Generate key prefix for all entities in a session
    fn entity_prefix(session_id: Uuid) -> String {
        format!("entity:{}:", session_id)
    }

    /// Generate key for relationship storage
    fn relationship_key(
        session_id: Uuid,
        from_entity: &str,
        to_entity: &str,
        relation_type: &RelationType,
    ) -> String {
        format!(
            "relationship:{}:{}:{}:{:?}",
            session_id, from_entity, to_entity, relation_type
        )
    }

    /// Generate key prefix for all relationships in a session
    fn relationship_prefix(session_id: Uuid) -> String {
        format!("relationship:{}:", session_id)
    }

    /// Save an entity to RocksDB
    async fn save_entity(&self, entity: &StoredEntity) -> Result<()> {
        let db = self.db.clone();
        let entity = entity.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let key = Self::entity_key(entity.session_id, &entity.name);
            let data = bincode::serde::encode_to_vec(&entity, bincode::config::standard())
                .map_err(|e| anyhow::anyhow!("Failed to serialize entity: {}", e))?;
            db.put(key.as_bytes(), &data)?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Load an entity from RocksDB
    #[allow(dead_code)]
    async fn load_entity(&self, session_id: Uuid, name: &str) -> Result<Option<StoredEntity>> {
        let db = self.db.clone();
        let key = Self::entity_key(session_id, name);

        tokio::task::spawn_blocking(move || -> Result<Option<StoredEntity>> {
            if let Some(data) = db.get(key.as_bytes())? {
                let (entity, _) = bincode::serde::decode_from_slice::<StoredEntity, _>(
                    &data,
                    bincode::config::standard(),
                )
                .map_err(|e| anyhow::anyhow!("Failed to deserialize entity: {}", e))?;
                Ok(Some(entity))
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Load all entities for a session
    pub(super) async fn load_session_entities(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<StoredEntity>> {
        let db = self.db.clone();
        let prefix = Self::entity_prefix(session_id);

        tokio::task::spawn_blocking(move || -> Result<Vec<StoredEntity>> {
            let mut entities = Vec::new();
            let iter = db.iterator(rocksdb::IteratorMode::From(
                prefix.as_bytes(),
                rocksdb::Direction::Forward,
            ));

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with(&prefix) {
                    break;
                }

                if let Ok((entity, _)) = bincode::serde::decode_from_slice::<StoredEntity, _>(
                    &value,
                    bincode::config::standard(),
                ) {
                    entities.push(entity);
                }
            }

            Ok(entities)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Delete an entity from RocksDB
    pub(super) async fn delete_stored_entity(&self, session_id: Uuid, name: &str) -> Result<()> {
        let db = self.db.clone();
        let key = Self::entity_key(session_id, name);

        tokio::task::spawn_blocking(move || -> Result<()> {
            db.delete(key.as_bytes())?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Save a relationship to RocksDB
    async fn save_relationship(&self, relationship: &StoredRelationship) -> Result<()> {
        let db = self.db.clone();
        let relationship = relationship.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let rel_type: RelationType = relationship
                .relation_type
                .parse()
                .unwrap_or(RelationType::RelatedTo);
            let key = Self::relationship_key(
                relationship.session_id,
                &relationship.from_entity,
                &relationship.to_entity,
                &rel_type,
            );
            let data = bincode::serde::encode_to_vec(&relationship, bincode::config::standard())
                .map_err(|e| anyhow::anyhow!("Failed to serialize relationship: {}", e))?;
            db.put(key.as_bytes(), &data)?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Batch save entities and relationships in a single RocksDB WriteBatch.
    /// This is significantly more efficient than individual saves when updating
    /// multiple entities/relationships at once (1 spawn_blocking vs N).
    pub async fn batch_save_graph_data(
        &self,
        entities: &[StoredEntity],
        relationships: &[StoredRelationship],
    ) -> Result<()> {
        if entities.is_empty() && relationships.is_empty() {
            return Ok(());
        }

        let db = self.db.clone();
        let entities = entities.to_vec();
        let relationships = relationships.to_vec();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut batch = WriteBatch::default();

            for entity in &entities {
                let key = Self::entity_key(entity.session_id, &entity.name);
                let data = bincode::serde::encode_to_vec(entity, bincode::config::standard())
                    .map_err(|e| anyhow::anyhow!("Failed to serialize entity: {}", e))?;
                batch.put(key.as_bytes(), &data);
            }

            for rel in &relationships {
                let rel_type: RelationType = rel
                    .relation_type
                    .parse()
                    .unwrap_or(RelationType::RelatedTo);
                let key = Self::relationship_key(
                    rel.session_id,
                    &rel.from_entity,
                    &rel.to_entity,
                    &rel_type,
                );
                let data = bincode::serde::encode_to_vec(rel, bincode::config::standard())
                    .map_err(|e| anyhow::anyhow!("Failed to serialize relationship: {}", e))?;
                batch.put(key.as_bytes(), &data);
            }

            db.write(batch)?;
            debug!(
                "Batch saved {} entities + {} relationships",
                entities.len(),
                relationships.len()
            );
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;

        Ok(())
    }

    /// Load all relationships for a session
    async fn load_session_relationships(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<StoredRelationship>> {
        let db = self.db.clone();
        let prefix = Self::relationship_prefix(session_id);

        tokio::task::spawn_blocking(move || -> Result<Vec<StoredRelationship>> {
            let mut relationships = Vec::new();
            let iter = db.iterator(rocksdb::IteratorMode::From(
                prefix.as_bytes(),
                rocksdb::Direction::Forward,
            ));

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with(&prefix) {
                    break;
                }

                if let Ok((rel, _)) = bincode::serde::decode_from_slice::<StoredRelationship, _>(
                    &value,
                    bincode::config::standard(),
                ) {
                    relationships.push(rel);
                }
            }

            Ok(relationships)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }
}

#[async_trait]
impl GraphStorage for RealRocksDBStorage {
    async fn upsert_entity(&self, session_id: Uuid, entity: &EntityData) -> Result<()> {
        let stored = StoredEntity::from_entity_data(session_id, entity);
        self.save_entity(&stored).await
    }

    async fn get_entity(&self, session_id: Uuid, name: &str) -> Result<Option<EntityData>> {
        let stored = self.load_entity(session_id, name).await?;
        Ok(stored.map(|s| s.to_entity_data()))
    }

    async fn list_entities(&self, session_id: Uuid) -> Result<Vec<EntityData>> {
        let stored = self.load_session_entities(session_id).await?;
        Ok(stored.into_iter().map(|s| s.to_entity_data()).collect())
    }

    async fn delete_entity(&self, session_id: Uuid, name: &str) -> Result<()> {
        self.delete_stored_entity(session_id, name).await
    }

    async fn create_relationship(
        &self,
        session_id: Uuid,
        relationship: &EntityRelationship,
    ) -> Result<()> {
        let stored = StoredRelationship::from_relationship(session_id, relationship);
        self.save_relationship(&stored).await
    }

    async fn find_related_entities(
        &self,
        session_id: Uuid,
        entity_name: &str,
    ) -> Result<Vec<String>> {
        let relationships = self.load_session_relationships(session_id).await?;
        let mut related: Vec<String> = relationships
            .into_iter()
            .filter_map(|r| {
                if r.from_entity == entity_name {
                    Some(r.to_entity)
                } else if r.to_entity == entity_name {
                    Some(r.from_entity)
                } else {
                    None
                }
            })
            .collect();

        // Deduplicate
        related.sort();
        related.dedup();
        Ok(related)
    }

    async fn find_related_by_type(
        &self,
        session_id: Uuid,
        entity_name: &str,
        relation_type: &RelationType,
    ) -> Result<Vec<String>> {
        let type_str = format!("{:?}", relation_type);
        let relationships = self.load_session_relationships(session_id).await?;
        let mut related: Vec<String> = relationships
            .into_iter()
            .filter(|r| r.relation_type == type_str)
            .filter_map(|r| {
                if r.from_entity == entity_name {
                    Some(r.to_entity)
                } else if r.to_entity == entity_name {
                    Some(r.from_entity)
                } else {
                    None
                }
            })
            .collect();

        related.sort();
        related.dedup();
        Ok(related)
    }

    async fn find_shortest_path(
        &self,
        session_id: Uuid,
        from: &str,
        to: &str,
    ) -> Result<Option<Vec<String>>> {
        use std::collections::{HashSet, VecDeque};

        if from == to {
            return Ok(Some(vec![from.to_string()]));
        }

        let relationships = self.load_session_relationships(session_id).await?;

        // Build adjacency list
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for rel in &relationships {
            adjacency
                .entry(rel.from_entity.clone())
                .or_default()
                .push(rel.to_entity.clone());
            adjacency
                .entry(rel.to_entity.clone())
                .or_default()
                .push(rel.from_entity.clone());
        }

        // BFS for shortest path
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, Vec<String>)> = VecDeque::new();

        visited.insert(from.to_string());
        queue.push_back((from.to_string(), vec![from.to_string()]));

        while let Some((current, path)) = queue.pop_front() {
            if let Some(neighbors) = adjacency.get(&current) {
                for neighbor in neighbors {
                    if neighbor == to {
                        let mut final_path = path.clone();
                        final_path.push(neighbor.clone());
                        return Ok(Some(final_path));
                    }

                    if !visited.contains(neighbor) {
                        visited.insert(neighbor.clone());
                        let mut new_path = path.clone();
                        new_path.push(neighbor.clone());
                        queue.push_back((neighbor.clone(), new_path));
                    }
                }
            }
        }

        Ok(None)
    }

    async fn get_entity_network(
        &self,
        session_id: Uuid,
        center: &str,
        max_depth: usize,
    ) -> Result<EntityNetwork> {
        use std::collections::HashSet;

        let all_entities = self.load_session_entities(session_id).await?;
        let all_relationships = self.load_session_relationships(session_id).await?;

        // Build adjacency map
        let mut adjacency: HashMap<String, Vec<(String, &StoredRelationship)>> = HashMap::new();
        for rel in &all_relationships {
            adjacency
                .entry(rel.from_entity.clone())
                .or_default()
                .push((rel.to_entity.clone(), rel));
            adjacency
                .entry(rel.to_entity.clone())
                .or_default()
                .push((rel.from_entity.clone(), rel));
        }

        // BFS to collect entities within max_depth
        let mut visited: HashSet<String> = HashSet::new();
        let mut current_level: Vec<String> = vec![center.to_string()];
        visited.insert(center.to_string());

        for _ in 0..max_depth {
            let mut next_level = Vec::new();
            for entity in &current_level {
                if let Some(neighbors) = adjacency.get(entity) {
                    for (neighbor, _) in neighbors {
                        if !visited.contains(neighbor) {
                            visited.insert(neighbor.clone());
                            next_level.push(neighbor.clone());
                        }
                    }
                }
            }
            if next_level.is_empty() {
                break;
            }
            current_level = next_level;
        }

        // Collect entities in the network
        let entities: BTreeMap<String, EntityData> = all_entities
            .into_iter()
            .filter(|e| visited.contains(&e.name))
            .map(|e| (e.name.clone(), e.to_entity_data()))
            .collect();

        // Collect relationships where both endpoints are in the network
        let relationships: Vec<EntityRelationship> = all_relationships
            .into_iter()
            .filter(|r| visited.contains(&r.from_entity) && visited.contains(&r.to_entity))
            .map(|r| r.to_relationship())
            .collect();

        Ok(EntityNetwork {
            center: center.to_string(),
            entities,
            relationships,
        })
    }
}
