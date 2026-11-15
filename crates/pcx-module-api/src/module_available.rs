// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The module-available event (docs/connect.md section 7): modules that run
//! elsewhere (a runner's) can be called again, their place connected, as
//! `EVENT_KIND_MODULE_AVAILABLE` carries it; a module that waited for them
//! (the team's work on a task) goes on.

use serde_json::{Map, Value};

use crate::{Event, json};

/// The event's `hook_event_name`.
pub const MODULE_AVAILABLE: &str = "ModuleAvailable";

/// What a module-available event says: the ids of the modules available
/// again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleAvailable {
    /// The modules.
    pub modules: Vec<String>,
}

impl ModuleAvailable {
    /// The event: `hook_event_name` [`MODULE_AVAILABLE`] with the modules as
    /// its payload.
    #[must_use]
    pub fn to_event(&self) -> Event {
        let mut payload = Map::new();
        payload.insert(
            "modules".into(),
            Value::Array(self.modules.iter().cloned().map(Value::String).collect()),
        );
        Event {
            hook_event_name: MODULE_AVAILABLE.to_string(),
            payload: Some(json::object_to_wire(payload)),
            ..Event::default()
        }
    }

    /// The modules `event` says are available; none: it is another event.
    ///
    /// # Errors
    /// `InvalidArgument` for a module-available event without its modules.
    pub fn from_event(event: &Event) -> Result<Option<Self>, crate::ModuleError> {
        if event.hook_event_name != MODULE_AVAILABLE {
            return Ok(None);
        }
        let payload = event.payload.clone().map(json::object_from_wire);
        let modules = payload
            .as_ref()
            .and_then(|p| p.get("modules"))
            .and_then(Value::as_array)
            .and_then(|modules| {
                modules
                    .iter()
                    .map(|m| m.as_str().map(str::to_string))
                    .collect::<Option<Vec<_>>>()
            })
            .ok_or_else(|| {
                crate::ModuleError::new(
                    crate::ModuleErrorKind::InvalidArgument,
                    "a module-available event names no modules",
                )
            })?;
        Ok(Some(Self { modules }))
    }
}

#[cfg(test)]
mod tests;
