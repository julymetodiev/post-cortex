// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The memory-write event (ARCHITECTURE section 7, modules together): what
//! changed in memory, as `EVENT_KIND_MEMORY_WRITE` carries it, and which
//! subscriptions follow it (`Subscription.memory`), so every host and the
//! cloud deliver the same events.

use serde_json::{Map, Value};

use crate::{Event, json};
use post_cortex_proto::module::MemoryFilter;

/// The event's `hook_event_name`.
pub const MEMORY_WRITE: &str = "MemoryWrite";

/// What a memory-write event says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryWrite {
    /// A document version was written, or a document deleted.
    Document {
        /// Its workspace.
        workspace_id: String,
        /// The document.
        id: String,
        /// Its kind, `<module>/<name>`.
        kind: String,
        /// Its version now (0 once deleted).
        version: u64,
        /// Whether it was deleted.
        deleted: bool,
    },
    /// A record was written, forgotten or removed.
    Record {
        /// A workspace its session belongs to (empty: none).
        workspace_id: String,
        /// The record.
        id: String,
        /// Its type by wire name; none for a removed record, whose type is
        /// gone.
        record_type: Option<String>,
        /// Whether it was removed.
        deleted: bool,
    },
    /// The feed fell behind: what a subscriber follows may have changed
    /// unannounced; it re-reads it.
    FellBehind,
}

impl MemoryWrite {
    /// The event: `hook_event_name` [`MEMORY_WRITE`] with the change as its
    /// payload.
    #[must_use]
    pub fn to_event(&self) -> Event {
        let text = |s: &str| Value::String(s.to_string());
        let mut payload = Map::new();
        match self {
            Self::Document {
                workspace_id,
                id,
                kind,
                version,
                deleted,
            } => {
                payload.insert("change".into(), text("document"));
                payload.insert("workspace_id".into(), text(workspace_id));
                payload.insert("id".into(), text(id));
                payload.insert("kind".into(), text(kind));
                payload.insert("version".into(), Value::from(*version));
                payload.insert("deleted".into(), Value::Bool(*deleted));
            }
            Self::Record {
                workspace_id,
                id,
                record_type,
                deleted,
            } => {
                payload.insert("change".into(), text("record"));
                payload.insert("workspace_id".into(), text(workspace_id));
                payload.insert("id".into(), text(id));
                payload.insert(
                    "type".into(),
                    record_type.as_deref().map_or(Value::Null, text),
                );
                payload.insert("version".into(), Value::from(0));
                payload.insert("deleted".into(), Value::Bool(*deleted));
            }
            Self::FellBehind => {
                payload.insert("fell_behind".into(), Value::Bool(true));
            }
        }
        Event {
            hook_event_name: MEMORY_WRITE.to_string(),
            payload: Some(json::object_to_wire(payload)),
            ..Event::default()
        }
    }

    /// The change `event` (of `hook_event_name` [`MEMORY_WRITE`]) carries.
    ///
    /// # Errors
    /// `InvalidArgument` for another event, or a payload out of shape.
    pub fn from_event(event: &Event) -> Result<Self, crate::ModuleError> {
        let invalid = |reason: &str| {
            crate::ModuleError::new(
                crate::ModuleErrorKind::InvalidArgument,
                format!("a memory write event: {reason}"),
            )
        };
        if event.hook_event_name != MEMORY_WRITE {
            return Err(invalid("another event"));
        }
        let payload = event
            .payload
            .clone()
            .map(json::object_from_wire)
            .ok_or_else(|| invalid("no payload"))?;
        if payload.get("fell_behind") == Some(&Value::Bool(true)) {
            return Ok(Self::FellBehind);
        }
        let text = |field: &str| {
            payload
                .get(field)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| invalid(&format!("no {field}")))
        };
        let deleted = payload
            .get("deleted")
            .and_then(Value::as_bool)
            .ok_or_else(|| invalid("no deleted"))?;
        match text("change")?.as_str() {
            "document" => Ok(Self::Document {
                workspace_id: text("workspace_id")?,
                id: text("id")?,
                kind: text("kind")?,
                version: payload
                    .get("version")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| invalid("no version"))?,
                deleted,
            }),
            "record" => Ok(Self::Record {
                workspace_id: text("workspace_id")?,
                id: text("id")?,
                record_type: match payload.get("type") {
                    Some(Value::String(t)) => Some(t.clone()),
                    Some(Value::Null) => None,
                    _ => return Err(invalid("no type")),
                },
                deleted,
            }),
            other => Err(invalid(&format!("an unknown change {other}"))),
        }
    }

    /// Whether a subscription with `filter` follows this change: every
    /// change without a filter; else a document whose kind it lists, a
    /// record whose type it lists (a removed one when it lists any type),
    /// in a workspace it lists when it lists any; falling behind always.
    #[must_use]
    pub fn followed_by(&self, filter: Option<&MemoryFilter>) -> bool {
        let Some(filter) = filter else {
            return true;
        };
        let in_workspace = |workspace_id: &String| {
            filter.workspaces.is_empty() || filter.workspaces.contains(workspace_id)
        };
        match self {
            Self::Document {
                workspace_id, kind, ..
            } => filter.document_kinds.contains(kind) && in_workspace(workspace_id),
            Self::Record {
                workspace_id,
                record_type,
                ..
            } => {
                let typed = match record_type {
                    Some(t) => filter.record_types.contains(t),
                    None => !filter.record_types.is_empty(),
                };
                typed && in_workspace(workspace_id)
            }
            Self::FellBehind => true,
        }
    }
}

#[cfg(test)]
mod tests;
