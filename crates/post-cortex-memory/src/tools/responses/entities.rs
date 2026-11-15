// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Entity responses: delete and merge.

use serde::Serialize;

/// An entity delete.
#[derive(Debug, Clone, Serialize)]
pub struct EntityDeleted {
    /// It was in the session.
    pub existed: bool,
    /// The entity.
    pub entity_name: String,
    /// The session.
    pub session_id: String,
}

/// An entity merge.
#[derive(Debug, Clone, Serialize)]
pub struct EntitiesMerged {
    /// Anything was merged.
    pub merged: bool,
    /// The entity merged away (now an alias).
    pub entity_name: String,
    /// The entity it was merged into.
    pub into: String,
    /// The session.
    pub session_id: String,
}
