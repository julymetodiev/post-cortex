// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Session metadata.

use crate::session::active_session::UserPreferences;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What a session is besides its records: id, name, description, creation
/// time and preferences.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Unique session identifier
    pub id: Uuid,
    /// Human-readable session name
    pub name: Option<String>,
    /// Human-readable session description
    pub description: Option<String>,
    /// When the session was created
    pub created_at: DateTime<Utc>,
    /// User-configurable session preferences
    pub user_preferences: UserPreferences,
}

impl SessionMetadata {
    /// Metadata of a session created at `created_at`.
    pub fn new(
        id: Uuid,
        name: Option<String>,
        description: Option<String>,
        user_preferences: UserPreferences,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            name,
            description,
            created_at,
            user_preferences,
        }
    }
}
