// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The stored form of a session (blob, export): [`StoredSession`], the
//! session with the ids of its embedded records, which the memory runtime
//! keeps beside it (`vectorized_update_ids`). Its fields and their order
//! are the format older versions read and write; the views a session
//! derives from its records (hot window, current state, code references,
//! change history) are written from the records, the retired tiers (warm,
//! cold) as empty lists, and on read every derived field is skipped and
//! derived again.

use crate::core::context_update::ContextUpdate;
use crate::graph::entity_graph::SimpleEntityGraph;
use crate::session::active_session::{ActiveSession, UserPreferences};
use crate::session::session_components::SessionMetadata;

use chrono::{DateTime, Utc};
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The stored form as read. The derived fields are skipped (and may be
/// missing).
#[derive(Deserialize)]
struct ActiveSessionData {
    id: Uuid,
    name: Option<String>,
    description: Option<String>,
    created_at: DateTime<Utc>,
    last_updated: DateTime<Utc>,
    user_preferences: UserPreferences,
    #[serde(default, rename = "hot_context")]
    _hot_context: IgnoredAny,
    #[serde(default, rename = "warm_context")]
    _warm_context: IgnoredAny,
    #[serde(default, rename = "cold_context")]
    _cold_context: IgnoredAny,
    #[serde(default, rename = "current_state")]
    _current_state: IgnoredAny,
    incremental_updates: Vec<ContextUpdate>,
    #[serde(default, rename = "code_references")]
    _code_references: IgnoredAny,
    #[serde(default, rename = "change_history")]
    _change_history: IgnoredAny,
    entity_graph: SimpleEntityGraph,
    #[serde(default)]
    vectorized_update_ids: Vec<Uuid>,
}

/// A session as stored and exported: the session and the ids of its records
/// whose embedding is in the vector index (kept by the memory runtime,
/// stored in the blob's `vectorized_update_ids` field).
#[derive(Clone, Debug)]
pub struct StoredSession {
    /// The session.
    pub session: ActiveSession,
    /// Ids of the session's embedded records, in no particular order.
    pub vectorized: Vec<Uuid>,
}

impl StoredSession {
    /// `session` with the embedded record ids `vectorized`.
    pub fn new(session: ActiveSession, vectorized: Vec<Uuid>) -> Self {
        Self {
            session,
            vectorized,
        }
    }

    /// Borrowed serialisation view; see [`ActiveSessionView`].
    pub fn view(&self) -> ActiveSessionView<'_> {
        self.session
            .serializable_view()
            .with_vectorized(&self.vectorized)
    }
}

/// Borrowed serialisation view of an [`ActiveSession`]; a
/// [`StoredSession`]'s `Serialize` delegates here. Obtain it with
/// [`ActiveSession::serializable_view`] or [`StoredSession::view`].
///
/// [`without_incremental_updates`](Self::without_incremental_updates)
/// writes `"incremental_updates": []` instead of the full list, for storage
/// backends that persist records as separate rows; the result still
/// deserialises with every version of the reader.
pub struct ActiveSessionView<'a> {
    session: &'a ActiveSession,
    vectorized: &'a [Uuid],
    include_incremental_updates: bool,
    include_derived: bool,
}

impl<'a> ActiveSessionView<'a> {
    /// Write `ids` as the session's embedded records
    /// (`vectorized_update_ids`; empty unless set).
    pub fn with_vectorized(mut self, ids: &'a [Uuid]) -> Self {
        self.vectorized = ids;
        self
    }

    /// Write the views derived from the records (`hot_context`,
    /// `current_state`, `code_references`, `change_history`) empty. The
    /// reader rebuilds them from `incremental_updates` and ignores what is
    /// stored, so the result reads back the same; an archive then holds each
    /// record once.
    pub fn without_derived(mut self) -> Self {
        self.include_derived = false;
        self
    }

    /// Omit the (potentially large) `incremental_updates` list; it is
    /// serialised as an empty array so the field stays present.
    pub fn without_incremental_updates(mut self) -> Self {
        self.include_incremental_updates = false;
        self
    }
}

impl Serialize for ActiveSessionView<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let s = self.session;
        let meta = &*s.metadata;
        let none_kept = crate::collections::SharedVec::new();
        let records = if self.include_incremental_updates {
            s.records()
        } else {
            &none_kept
        };
        let none: [(); 0] = [];

        let mut st = serializer.serialize_struct("ActiveSessionData", 15)?;
        st.serialize_field("id", &meta.id)?;
        st.serialize_field("name", &meta.name)?;
        st.serialize_field("description", &meta.description)?;
        st.serialize_field("created_at", &meta.created_at)?;
        st.serialize_field("last_updated", &s.last_updated)?;
        st.serialize_field("user_preferences", &meta.user_preferences)?;
        if self.include_derived {
            st.serialize_field("hot_context", &s.hot_records().collect::<Vec<_>>())?;
        } else {
            st.serialize_field("hot_context", &none)?;
        }
        st.serialize_field("warm_context", &none)?;
        st.serialize_field("cold_context", &none)?;
        if self.include_derived {
            st.serialize_field("current_state", &s.current_state())?;
        } else {
            st.skip_field("current_state")?;
        }
        st.serialize_field("incremental_updates", records)?;
        if self.include_derived {
            st.serialize_field("code_references", &s.code_references())?;
            st.serialize_field("change_history", &s.change_history())?;
        } else {
            st.skip_field("code_references")?;
            st.serialize_field("change_history", &none)?;
        }
        st.serialize_field("entity_graph", &*s.entity_graph)?;
        st.serialize_field("vectorized_update_ids", self.vectorized)?;
        st.end()
    }
}

impl ActiveSession {
    /// Borrowed serialisation view; see [`ActiveSessionView`].
    pub fn serializable_view(&self) -> ActiveSessionView<'_> {
        ActiveSessionView {
            session: self,
            vectorized: &[],
            include_incremental_updates: true,
            include_derived: true,
        }
    }
}

impl Serialize for StoredSession {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.view().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StoredSession {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let data = ActiveSessionData::deserialize(deserializer)?;
        let metadata = SessionMetadata::new(
            data.id,
            data.name,
            data.description,
            data.user_preferences,
            data.created_at,
        );
        let session = ActiveSession::from_records(
            metadata,
            data.last_updated,
            data.incremental_updates,
            data.entity_graph,
        );
        Ok(StoredSession::new(session, data.vectorized_update_ids))
    }
}
