// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The shared, strict parsers of every transport: domain enum names (on the
//! core `wire_names` codec), entities and relations, the writer's
//! validity statements, search sources and modes, UUIDs.
//!
//! Names are matched without regard to case and `_`, against the wire name
//! and the stored (serde) name, so `depends_on`, `DependsOn` and
//! `dependson` are one relation type. A name that matches no variant is an
//! invalid argument naming the field and the valid values: never a default.

use crate::content_vectorizer::{RerankSkipped, SearchMode};
use chrono::{DateTime, Utc};
use post_cortex_core::core::content::ContentKind;
use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType, UpdateMeta, UpdateType,
};
use post_cortex_core::core::dismissal::RadarItemKind;
use post_cortex_core::core::message::MessageRole;
use post_cortex_core::workspace::SessionRole;
use post_cortex_ports::Corpus;
use uuid::Uuid;

use crate::tools::Failure;

/// A domain enum with wire names (the core `wire_names` codec).
pub trait WireName: Sized + Clone + 'static {
    /// The enum, as a phrase (`relation type`).
    const KIND: &'static str;
    /// Every variant.
    fn all() -> &'static [Self];
    /// The wire name.
    fn wire(&self) -> &'static str;
    /// The stored (serde) name.
    fn stored(&self) -> &'static str;
}

macro_rules! wire_name {
    ($($ty:ty => $kind:literal),+ $(,)?) => {$(
        impl WireName for $ty {
            const KIND: &'static str = $kind;
            fn all() -> &'static [Self] {
                <$ty>::ALL
            }
            fn wire(&self) -> &'static str {
                self.as_str()
            }
            fn stored(&self) -> &'static str {
                self.stored_name()
            }
        }
    )+};
}

wire_name!(
    UpdateType => "interaction type",
    EntityType => "entity type",
    RelationType => "relation type",
    SessionRole => "session role",
    ContentKind => "content kind",
    MessageRole => "message role",
    RadarItemKind => "radar kind",
);

/// `s` lowercased without `_` and surrounding space.
fn folded(s: &str) -> String {
    s.trim()
        .chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// The variant `value` names, if any.
#[must_use]
pub fn lookup<T: WireName>(value: &str) -> Option<T> {
    let key = folded(value);
    T::all()
        .iter()
        .find(|v| folded(v.wire()) == key || folded(v.stored()) == key)
        .cloned()
}

/// The wire names of `T`, comma-separated.
#[must_use]
pub fn names<T: WireName>() -> String {
    T::all()
        .iter()
        .map(|v| v.wire())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The variant `value` names; `field` names the argument in the error.
///
/// # Errors
/// `value` names no variant of `T`.
pub fn parse<T: WireName>(field: &str, value: &str) -> Result<T, Failure> {
    lookup(value).ok_or_else(|| {
        Failure::invalid(format!(
            "{field}: unknown {} {value:?}; valid values are: {}",
            T::KIND,
            names::<T>()
        ))
        .parameter(field)
        .expected(format!("one of: {}", names::<T>()))
        .received(value)
    })
}

/// A UUID argument.
///
/// # Errors
/// `value` is not a UUID.
pub fn uuid(field: &str, value: &str) -> Result<Uuid, Failure> {
    Uuid::parse_str(value.trim()).map_err(|_| {
        Failure::invalid(format!("Invalid UUID: {value}"))
            .parameter(field)
            .expected("UUID string (36 chars with hyphens)")
            .received(value)
    })
}

/// The entities a write names, each first and last mentioned `now`.
/// `entity_type` is parsed strictly when `strict` (an empty one is unset:
/// `Concept`); otherwise an unknown type is `Concept`, as the MCP tool
/// documents.
///
/// # Errors
/// With `strict`, an entity type that names no variant.
pub fn entities<'a>(
    items: impl IntoIterator<Item = (&'a str, &'a str)>,
    strict: bool,
    now: DateTime<Utc>,
) -> Result<Vec<EntityData>, Failure> {
    items
        .into_iter()
        .enumerate()
        .map(|(i, (name, entity_type))| {
            let entity_type = match lookup(entity_type) {
                Some(t) => t,
                // Unset (an empty proto3 string) is `concept`, as everywhere.
                None if !strict || entity_type.trim().is_empty() => EntityType::Concept,
                None => parse(&format!("entities[{i}].entity_type"), entity_type)?,
            };
            Ok(EntityData {
                aliases: Vec::new(),
                name: name.to_string(),
                entity_type,
                first_mentioned: now,
                last_mentioned: now,
                mention_count: 1,
                importance_score: 1.0,
                description: None,
            })
        })
        .collect()
}

/// One relation of a write: `(from, to, relation_type, context)`.
pub type RelationItem<'a> = (&'a str, &'a str, &'a str, &'a str);

/// The relations a write states; relation types are strict.
///
/// # Errors
/// A relation type that names no variant.
pub fn relations<'a>(
    items: impl IntoIterator<Item = RelationItem<'a>>,
) -> Result<Vec<EntityRelationship>, Failure> {
    items
        .into_iter()
        .enumerate()
        .map(|(i, (from, to, relation_type, context))| {
            let relation_type = lookup(relation_type).ok_or_else(|| {
                Failure::invalid(format!(
                    "relation[{i}]: unknown relation_type {relation_type:?}; valid values are: \
                     required_by, leads_to, related_to, conflicts_with, depends_on, implements, caused_by, solves"
                ))
                .parameter(format!("relations[{i}].relation_type"))
                .received(relation_type)
            })?;
            Ok(EntityRelationship {
                from_entity: from.to_string(),
                to_entity: to.to_string(),
                relation_type,
                context: context.to_string(),
            })
        })
        .collect()
}

/// The writer's validity and trust statements; `prefix` names the update
/// in errors (`updates[2].`).
///
/// # Errors
/// `valid_from` is not a time or date, or a `supersedes` entry is not an
/// entry id.
pub fn update_meta(
    valid_from: Option<&str>,
    supersedes: &[String],
    confidence: Option<f32>,
    prefix: &str,
) -> Result<UpdateMeta, Failure> {
    let valid_from = valid_from
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| {
            post_cortex_core::time::parse_time(v).ok_or_else(|| {
                Failure::invalid(format!(
                    "valid_from: '{v}' is not an RFC 3339 time or YYYY-MM-DD date"
                ))
                .parameter(format!("{prefix}valid_from"))
            })
        })
        .transpose()?;
    let supersedes = supersedes
        .iter()
        .map(|id| {
            Uuid::parse_str(id.trim()).map_err(|_| {
                Failure::invalid(format!("supersedes: '{id}' is not an entry id"))
                    .parameter(format!("{prefix}supersedes"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(UpdateMeta {
        valid_from,
        supersedes,
        confidence,
        retired_at: None,
    })
}

/// The search scopes.
pub const SCOPES: &[&str] = &["session", "workspace", "global"];

/// A search scope name (`session`, `workspace`, `global`), lowercased.
///
/// # Errors
/// Another name.
pub fn scope(value: &str) -> Result<String, Failure> {
    let scope = value.to_lowercase();
    if SCOPES.contains(&scope.as_str()) {
        return Ok(scope);
    }
    Err(
        Failure::invalid(format!("Invalid scope: '{value}': Invalid scope"))
            .parameter("scope")
            .expected(format!("one of: {}", SCOPES.join(", ")))
            .received(value),
    )
}

/// A limit argument `field`: 1 to `max`, `default` when absent.
///
/// # Errors
/// 0 or over `max`.
pub fn limit(
    field: &str,
    value: Option<usize>,
    default: usize,
    max: usize,
) -> Result<usize, Failure> {
    let v = value.unwrap_or(default);
    let message = if v > max {
        format!("Limit {v} exceeds maximum of {max}: Limit too large")
    } else if v == 0 {
        "Limit must be at least 1: Limit too small".to_string()
    } else {
        return Ok(v);
    };
    Err(Failure::invalid(message)
        .parameter(field)
        .received(v)
        .expected(format!("number between 1 and {max}"))
        .hint(format!(
            "Use limit between 1 and {max}, or omit for default ({default})"
        )))
}

/// The `sources` argument: `records` (the default), `messages` or `all`.
///
/// # Errors
/// Another value.
pub fn sources(sources: Option<&str>) -> Result<Corpus, Failure> {
    match sources.map(str::trim) {
        None | Some("") => Ok(Corpus::default()),
        Some(s) => Corpus::parse(&s.to_lowercase()).ok_or_else(|| {
            Failure::invalid(format!(
                "Invalid sources '{s}': expected 'records', 'messages' or 'all'"
            ))
            .parameter("sources")
        }),
    }
}

/// The `mode` argument: `fast`, `local` or `accurate`.
///
/// # Errors
/// Another value.
pub fn mode(mode: Option<&str>) -> Result<Option<SearchMode>, Failure> {
    mode.map(|m| {
        m.parse()
            .map_err(|e: String| Failure::invalid(e).parameter("mode"))
    })
    .transpose()
}

/// Whether the cloud withheld a search's reranker (`rerank_withheld`, set
/// by its gateway): `plan_limit`, or unset.
///
/// # Errors
/// `InvalidArgument` for another value.
pub fn rerank_withheld(value: Option<&str>) -> Result<bool, Failure> {
    match value {
        None => Ok(false),
        Some(v) if v == RerankSkipped::PlanLimit.as_str() => Ok(true),
        Some(other) => Err(Failure::invalid(format!(
            "rerank_withheld is {}, not {other}",
            RerankSkipped::PlanLimit.as_str()
        ))
        .parameter("rerank_withheld")),
    }
}

/// The `(date_from, date_to)` pair as a UTC range: both RFC 3339, given
/// together or not at all.
///
/// # Errors
/// One of them is missing or not RFC 3339.
pub fn date_range(
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> Result<Option<post_cortex_core::time::TimeRange>, Failure> {
    let parse = |field: &str, v: &str, example: &str| {
        post_cortex_core::time::parse_time(v).ok_or_else(|| {
            Failure::invalid(format!(
                "Invalid {field} format: '{v}'. Expected RFC3339 format (e.g., {example}) \
                 or a date (YYYY-MM-DD)"
            ))
            .parameter(field)
        })
    };
    match (date_from, date_to) {
        (Some(from), Some(to)) => Ok(Some((
            parse("date_from", from, "2024-01-01T00:00:00Z")?,
            parse("date_to", to, "2024-12-31T23:59:59Z")?,
        ))),
        (Some(_), None) | (None, Some(_)) => Err(Failure::invalid(
            "Both date_from and date_to must be provided together",
        )
        .parameter("date_from,date_to")),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests;
