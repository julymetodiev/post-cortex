// Copyright (c) 2026 Julius ML
// MIT License

//! Validity intervals of records (ROADMAP Phase 3 items 3 and 7).
//!
//! Records are never rewritten to say they stopped holding; their validity
//! is derived from the records themselves, in time order:
//!
//! - **explicit**: a record names the records it supersedes
//!   ([`UpdateMeta::supersedes`](crate::core::context_update::UpdateMeta));
//! - **contradiction**: a record asserts `A conflicts_with B`, which ends
//!   the older records that asserted another relation between A and B;
//! - **same fact restated**: a newer record of the same kind about exactly
//!   the same entities with a similar title (word overlap at least
//!   [`RESTATE_TITLE_OVERLAP`]) replaces the older one; with near-identical
//!   text (overlap at least [`DUPLICATE_OVERLAP`]) it is a duplicate;
//! - **forgotten**: `retired_at` ends the record then.
//!
//! An ended record keeps its history: it stays searchable and in the
//! timeline, marked with when and why it ended. No LLM is involved, and the
//! same records always give the same intervals.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::context_update::{ContextUpdate, RelationType, UpdateType};

use super::entity_graph::{SimpleEntityGraph, entity_key, mentioned_names};

/// Title word overlap (Jaccard) from which a newer record about the same
/// entities restates an older one.
pub const RESTATE_TITLE_OVERLAP: f32 = 0.5;
/// Word overlap of title and description from which two such records are
/// duplicates.
pub const DUPLICATE_OVERLAP: f32 = 0.8;

/// Why a record stopped being valid.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// A later record said it replaces this one.
    Superseded,
    /// A later record contradicts a relation this one asserted.
    Contradicted,
    /// A later record states the same fact anew.
    Restated,
    /// A later record says the same thing.
    Duplicate,
    /// Retired with `forget`.
    Forgotten,
}

/// When a record held.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Validity {
    /// Start of validity: the writer's `valid_from`, else the timestamp.
    pub valid_from: DateTime<Utc>,
    /// End of validity; `None` while it holds.
    pub valid_to: Option<DateTime<Utc>>,
    /// The record that ended it (none when forgotten).
    pub ended_by: Option<Uuid>,
    /// Why it ended.
    pub reason: Option<EndReason>,
}

impl Validity {
    /// Whether the record held at `t`.
    pub fn holds_at(&self, t: DateTime<Utc>) -> bool {
        self.valid_from <= t && self.valid_to.is_none_or(|end| t < end)
    }

    /// Whether the record still holds.
    pub fn is_current(&self) -> bool {
        self.valid_to.is_none()
    }
}

/// Start of a record's validity.
pub fn valid_from(update: &ContextUpdate) -> DateTime<Utc> {
    update.meta.valid_from.unwrap_or(update.timestamp)
}

/// Kinds of records that state facts which a later record can restate.
fn restatable(kind: &UpdateType) -> bool {
    matches!(
        kind,
        UpdateType::DecisionMade | UpdateType::RequirementAdded | UpdateType::ConceptDefined
    )
}

/// Lowercase words of three or more letters or digits, any script.
pub fn words(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect()
}

/// Jaccard overlap of two word sets (0 when both are empty).
pub fn overlap(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(b).count() as f32 / union as f32
}

/// Canonical keys of the entities a record mentions.
fn subject(update: &ContextUpdate, graph: &SimpleEntityGraph) -> Vec<String> {
    let mut keys: Vec<String> = mentioned_names(update)
        .map(|n| entity_key(graph.resolve(n).unwrap_or(n)))
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Canonical key of an unordered entity pair.
fn pair(a: &str, b: &str, graph: &SimpleEntityGraph) -> (String, String) {
    let a = entity_key(graph.resolve(a).unwrap_or(a));
    let b = entity_key(graph.resolve(b).unwrap_or(b));
    if a <= b { (a, b) } else { (b, a) }
}

/// Validity of every record in `updates` (any order; ties in time are
/// broken by id). `graph` resolves entity names to their canonical entity.
pub fn validity<'a>(
    updates: impl IntoIterator<Item = &'a ContextUpdate>,
    graph: &SimpleEntityGraph,
) -> HashMap<Uuid, Validity> {
    let mut ordered: Vec<&ContextUpdate> = updates.into_iter().collect();
    ordered.sort_by_key(|u| (u.timestamp, u.id));

    let mut out: HashMap<Uuid, Validity> = ordered
        .iter()
        .map(|u| {
            (
                u.id,
                Validity {
                    valid_from: valid_from(u),
                    valid_to: None,
                    ended_by: None,
                    reason: None,
                },
            )
        })
        .collect();
    let end =
        |out: &mut HashMap<Uuid, Validity>, old: Uuid, by: &ContextUpdate, reason: EndReason| {
            let at = valid_from(by);
            if let Some(v) = out.get_mut(&old)
                && v.valid_to.is_none()
                && old != by.id
            {
                v.valid_to = Some(at.max(v.valid_from));
                v.ended_by = Some(by.id);
                v.reason = Some(reason);
            }
        };

    // Records still holding, per (kind, subject), oldest first.
    let mut by_subject: BTreeMap<(String, Vec<String>), Vec<usize>> = BTreeMap::new();
    // Records that asserted a relation, per entity pair.
    let mut by_pair: HashMap<(String, String), Vec<(Uuid, RelationType)>> = HashMap::new();
    let titles: Vec<HashSet<String>> = ordered.iter().map(|u| words(&u.content.title)).collect();

    for (i, update) in ordered.iter().enumerate() {
        for old in &update.meta.supersedes {
            end(&mut out, *old, update, EndReason::Superseded);
        }

        for rel in &update.creates_relationships {
            let key = pair(&rel.from_entity, &rel.to_entity, graph);
            if rel.relation_type == RelationType::ConflictsWith
                && let Some(asserted) = by_pair.get(&key)
            {
                let ended: Vec<Uuid> = asserted
                    .iter()
                    .filter(|(_, r)| *r != RelationType::ConflictsWith)
                    .map(|(id, _)| *id)
                    .collect();
                for old in ended {
                    end(&mut out, old, update, EndReason::Contradicted);
                }
            }
            by_pair
                .entry(key)
                .or_default()
                .push((update.id, rel.relation_type.clone()));
        }

        let subject = subject(update, graph);
        if restatable(&update.update_type) && !subject.is_empty() {
            let slot = by_subject
                .entry((update.update_type.as_str().to_string(), subject))
                .or_default();
            let mut text: Option<HashSet<String>> = None;
            for &j in slot.iter() {
                let older = ordered[j];
                if !out[&older.id].is_current() {
                    continue;
                }
                if overlap(&titles[i], &titles[j]) < RESTATE_TITLE_OVERLAP {
                    continue;
                }
                let mine = text.get_or_insert_with(|| {
                    words(&format!(
                        "{} {}",
                        update.content.title, update.content.description
                    ))
                });
                let theirs = words(&format!(
                    "{} {}",
                    older.content.title, older.content.description
                ));
                let reason = if overlap(mine, &theirs) >= DUPLICATE_OVERLAP {
                    EndReason::Duplicate
                } else {
                    EndReason::Restated
                };
                end(&mut out, older.id, update, reason);
            }
            slot.push(i);
        }
    }

    for update in &ordered {
        if let Some(at) = update.meta.retired_at
            && let Some(v) = out.get_mut(&update.id)
            && v.valid_to.is_none_or(|end| at < end)
        {
            v.valid_to = Some(at.max(v.valid_from));
            v.ended_by = None;
            v.reason = Some(EndReason::Forgotten);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::context_update::{
        EntityRelationship, EntityType, TypedEntity, UpdateContent, UpdateMeta,
    };
    use chrono::Duration;

    fn record(
        minutes: i64,
        kind: UpdateType,
        title: &str,
        entities: &[&str],
        relations: Vec<EntityRelationship>,
    ) -> ContextUpdate {
        ContextUpdate {
            id: Uuid::new_v4(),
            timestamp: DateTime::<Utc>::UNIX_EPOCH + Duration::minutes(minutes),
            update_type: kind,
            content: UpdateContent {
                title: title.to_string(),
                description: String::new(),
                details: Vec::new(),
                examples: Vec::new(),
                implications: Vec::new(),
            },
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: Vec::new(),
            creates_relationships: relations,
            references_entities: Vec::new(),
            typed_entities: entities
                .iter()
                .map(|n| TypedEntity {
                    name: n.to_string(),
                    entity_type: EntityType::Concept,
                })
                .collect(),
            author: None,
            derived_from: Vec::new(),
            meta: UpdateMeta::default(),
        }
    }

    fn rel(a: &str, b: &str, r: RelationType) -> EntityRelationship {
        EntityRelationship {
            from_entity: a.into(),
            to_entity: b.into(),
            relation_type: r,
            context: "c".into(),
        }
    }

    #[test]
    fn explicit_supersession_ends_at_the_new_valid_from() {
        let old = record(
            0,
            UpdateType::DecisionMade,
            "use rocksdb",
            &["storage"],
            vec![],
        );
        let mut new = record(10, UpdateType::DecisionMade, "move", &["other"], vec![]);
        new.meta.supersedes = vec![old.id];
        new.meta.valid_from = Some(old.timestamp + Duration::minutes(5));
        let v = validity([&old, &new], &SimpleEntityGraph::new());
        let ended = &v[&old.id];
        assert_eq!(ended.valid_to, Some(old.timestamp + Duration::minutes(5)));
        assert_eq!(ended.ended_by, Some(new.id));
        assert_eq!(ended.reason, Some(EndReason::Superseded));
        assert!(ended.holds_at(old.timestamp + Duration::minutes(1)));
        assert!(!ended.holds_at(new.timestamp));
        assert!(v[&new.id].is_current());
    }

    #[test]
    fn restated_fact_replaces_the_older_one() {
        let g = SimpleEntityGraph::new();
        let a = record(
            0,
            UpdateType::DecisionMade,
            "Default search mode is local",
            &["search mode"],
            vec![],
        );
        let b = record(
            5,
            UpdateType::DecisionMade,
            "Default search mode is fast",
            &["Search-Mode"],
            vec![],
        );
        // Different subject: untouched.
        let c = record(
            6,
            UpdateType::DecisionMade,
            "Default search mode is fast",
            &["reranker"],
            vec![],
        );
        // Not a restatable kind.
        let d = record(
            1,
            UpdateType::ProblemSolved,
            "search mode slow",
            &["search mode"],
            vec![],
        );
        let e = record(
            7,
            UpdateType::ProblemSolved,
            "search mode slow again",
            &["search mode"],
            vec![],
        );
        let v = validity([&a, &b, &c, &d, &e], &g);
        assert_eq!(v[&a.id].reason, Some(EndReason::Restated));
        assert_eq!(v[&a.id].ended_by, Some(b.id));
        assert!(v[&b.id].is_current() && v[&c.id].is_current());
        assert!(v[&d.id].is_current() && v[&e.id].is_current());
    }

    #[test]
    fn identical_text_is_a_duplicate_and_unrelated_titles_stay() {
        let g = SimpleEntityGraph::new();
        let a = record(
            0,
            UpdateType::ConceptDefined,
            "Agent as extractor",
            &["pcx"],
            vec![],
        );
        let b = record(
            1,
            UpdateType::ConceptDefined,
            "Agent as extractor",
            &["pcx"],
            vec![],
        );
        let c = record(
            2,
            UpdateType::ConceptDefined,
            "Cards are short notes",
            &["pcx"],
            vec![],
        );
        let v = validity([&c, &b, &a], &g);
        assert_eq!(v[&a.id].reason, Some(EndReason::Duplicate));
        assert!(v[&b.id].is_current() && v[&c.id].is_current());
    }

    #[test]
    fn conflicts_with_ends_records_asserting_the_pair() {
        let g = SimpleEntityGraph::new();
        let a = record(
            0,
            UpdateType::DecisionMade,
            "x",
            &["A", "B"],
            vec![rel("A", "B", RelationType::DependsOn)],
        );
        let other = record(
            1,
            UpdateType::DecisionMade,
            "y",
            &["A", "C"],
            vec![rel("A", "C", RelationType::DependsOn)],
        );
        let b = record(
            2,
            UpdateType::ProblemSolved,
            "z",
            &["A", "B"],
            vec![rel("B", "A", RelationType::ConflictsWith)],
        );
        let v = validity([&a, &other, &b], &g);
        assert_eq!(v[&a.id].reason, Some(EndReason::Contradicted));
        assert!(v[&other.id].is_current() && v[&b.id].is_current());
    }

    #[test]
    fn forgotten_records_end_when_retired() {
        let mut a = record(0, UpdateType::QuestionAnswered, "q", &["x"], vec![]);
        let at = a.timestamp + Duration::minutes(3);
        a.meta.retired_at = Some(at);
        let v = validity([&a], &SimpleEntityGraph::new());
        assert_eq!(v[&a.id].valid_to, Some(at));
        assert_eq!(v[&a.id].reason, Some(EndReason::Forgotten));
    }

    #[test]
    fn aliases_resolve_to_one_subject() {
        let mut g = SimpleEntityGraph::new();
        g.add_or_update_entity("SurrealDB".into(), EntityType::Technology, Utc::now(), "");
        g.add_or_update_entity("Surreal".into(), EntityType::Technology, Utc::now(), "");
        assert!(g.merge_entities("Surreal", "SurrealDB"));
        let a = record(
            0,
            UpdateType::DecisionMade,
            "storage backend choice",
            &["Surreal"],
            vec![],
        );
        let b = record(
            1,
            UpdateType::DecisionMade,
            "storage backend choice made",
            &["SurrealDB"],
            vec![],
        );
        let v = validity([&a, &b], &g);
        assert_eq!(v[&a.id].ended_by, Some(b.id));
    }
}
