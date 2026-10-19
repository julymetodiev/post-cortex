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
//! Core active session type with lock-free, Arc-wrapped components.
//!
//! `ActiveSession` is the primary mutable session object used throughout
//! the crate. It stores tiered context (hot / warm / cold), a structured
//! state snapshot, code references, an entity graph, and vectorization
//! tracking — all wrapped in `Arc` for cheap clone-on-write semantics.
use crate::core::context_update::{ContextUpdate, EntityType, UpdateType};
use crate::core::structured_context::StructuredContext;
use crate::graph::entity_graph::SimpleEntityGraph;
use crate::session::session_components::{HotContext, SessionMetadata};

use chrono::DateTime;
use chrono::Utc;
use dashmap::DashSet;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    /// Immutable session metadata (id, name, preferences)
    pub metadata: Arc<SessionMetadata>,
    /// Timestamp of the last modification
    pub last_updated: DateTime<Utc>,

    // Lock-free tiered context storage
    /// Lock-free hot updates (DashMap-based)
    pub hot_context: Arc<HotContext>,
    /// CoW compressed updates (storage tier)
    pub warm_context: Arc<Vec<CompressedUpdate>>,
    /// CoW periodic summaries (storage tier)
    pub cold_context: Arc<Vec<StructuredSummary>>,

    // Structured context - CoW wrapped for efficient updates
    /// Current queryable structured state
    pub current_state: Arc<StructuredContext>,
    /// All incremental updates (biggest CoW vector).
    ///
    /// Elements are `Arc`-shared (also with `hot_context`), so the
    /// copy-on-write clone `Arc::make_mut` performs while an older session
    /// version is still alive copies pointers, not update payloads.
    /// Serialises exactly like `Vec<ContextUpdate>`.
    pub incremental_updates: Arc<Vec<Arc<ContextUpdate>>>,

    // Code integration - CoW wrapped
    /// Code references indexed by file path
    pub code_references: Arc<HashMap<String, Vec<CodeReference>>>,
    /// Recorded change history (`Arc` elements for cheap CoW, serialises
    /// exactly like `Vec<ChangeRecord>`)
    pub change_history: Arc<Vec<Arc<ChangeRecord>>>,

    // Entity graph - CoW wrapped for efficient graph updates
    /// Named-entity graph with relationships
    pub entity_graph: Arc<SimpleEntityGraph>,

    // Vectorization tracking (lock-free set for concurrent access)
    /// IDs of updates that have been vectorized
    pub vectorized_update_ids: Arc<DashSet<Uuid>>,
}

/// Maximum number of periodic summaries retained in `cold_context`.
/// Older summaries are dropped first.
pub const MAX_COLD_SUMMARIES: usize = 100;

// Deserialization helper - the on-disk shape of an `ActiveSession`.
// Field names and order are the wire format; `ActiveSessionView` serialises
// the same fields in the same order from borrowed data.
#[derive(Deserialize)]
struct ActiveSessionData {
    id: Uuid,
    name: Option<String>,
    description: Option<String>,
    created_at: DateTime<Utc>,
    last_updated: DateTime<Utc>,
    user_preferences: UserPreferences,
    hot_context: Vec<ContextUpdate>,
    warm_context: Vec<CompressedUpdate>,
    cold_context: Vec<StructuredSummary>,
    current_state: StructuredContext,
    incremental_updates: Vec<Arc<ContextUpdate>>,
    code_references: HashMap<String, Vec<CodeReference>>,
    change_history: Vec<Arc<ChangeRecord>>,
    entity_graph: SimpleEntityGraph,
    #[serde(default)]
    vectorized_update_ids: Vec<Uuid>,
}

/// Borrowed, zero-copy serialisation view of an [`ActiveSession`].
///
/// Produces exactly the JSON `ActiveSession`'s `Serialize` impl produces
/// (that impl delegates here), without deep-cloning any component. Obtain
/// it with [`ActiveSession::serializable_view`].
///
/// [`without_incremental_updates`](Self::without_incremental_updates)
/// writes `"incremental_updates": []` instead of the full list, for storage
/// backends that persist updates as separate rows; the result still
/// deserialises with every version of the `ActiveSession` deserializer.
pub struct ActiveSessionView<'a> {
    session: &'a ActiveSession,
    include_incremental_updates: bool,
}

impl ActiveSessionView<'_> {
    /// Omit the (potentially large) `incremental_updates` list; it is
    /// serialised as an empty array so the field stays present.
    pub fn without_incremental_updates(mut self) -> Self {
        self.include_incremental_updates = false;
        self
    }
}

/// Serialises a `DashSet` as a sequence without collecting it first.
struct DashSetSeq<'a>(&'a DashSet<Uuid>);

impl Serialize for DashSetSeq<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(self.0.iter().map(|id| *id))
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
        let incremental_updates: &[Arc<ContextUpdate>] = if self.include_incremental_updates {
            s.incremental_updates.as_slice()
        } else {
            &[]
        };

        let mut st = serializer.serialize_struct("ActiveSessionData", 15)?;
        st.serialize_field("id", &meta.id)?;
        st.serialize_field("name", &meta.name)?;
        st.serialize_field("description", &meta.description)?;
        st.serialize_field("created_at", &meta.created_at)?;
        st.serialize_field("last_updated", &s.last_updated)?;
        st.serialize_field("user_preferences", &meta.user_preferences)?;
        // Arc handles only - the hot tier holds at most `max_hot_context_size`.
        st.serialize_field("hot_context", &s.hot_context.snapshot_arcs())?;
        st.serialize_field("warm_context", s.warm_context.as_slice())?;
        st.serialize_field("cold_context", s.cold_context.as_slice())?;
        st.serialize_field("current_state", &*s.current_state)?;
        st.serialize_field("incremental_updates", incremental_updates)?;
        st.serialize_field("code_references", &*s.code_references)?;
        st.serialize_field("change_history", s.change_history.as_slice())?;
        st.serialize_field("entity_graph", &*s.entity_graph)?;
        st.serialize_field(
            "vectorized_update_ids",
            &DashSetSeq(&s.vectorized_update_ids),
        )?;
        st.end()
    }
}

/// A compressed (summarised) context update stored in the warm tier.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CompressedUpdate {
    /// The original context update
    pub update: ContextUpdate,
    /// Compression ratio achieved (0.0–1.0)
    pub compression_ratio: f32,
    /// When the compression was performed
    pub compressed_at: DateTime<Utc>,
}

/// A periodic summary marker stored in the cold tier.
///
/// Older versions stored a full copy of `current_state` plus the id of every
/// update so far in each summary, which grew without bound. Summaries are
/// now compact: `context_snapshot` is an empty `StructuredContext` (the field
/// is kept so older readers still parse the blob) and `referenced_updates`
/// holds only the `[first, last]` update ids of the covered range. Old blobs
/// are compacted on load (see [`StructuredSummary::compact`]).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StructuredSummary {
    /// Unique identifier for this summary
    pub summary_id: Uuid,
    /// When this summary was created
    pub created_at: DateTime<Utc>,
    /// Snapshot of the structured context at summary time (empty since the
    /// cold tier was compacted; kept for wire compatibility)
    pub context_snapshot: StructuredContext,
    /// `[first, last]` ids of the updates covered by this summary
    pub referenced_updates: Vec<Uuid>,
    /// Quality score of the summary (0.0–1.0)
    pub summary_quality: f32,
}

impl StructuredSummary {
    /// Drop the snapshot payload and reduce `referenced_updates` to its
    /// `[first, last]` range.
    pub fn compact(&mut self) {
        self.context_snapshot = StructuredContext::new();
        if self.referenced_updates.len() > 2 {
            let first = self.referenced_updates[0];
            let last = self.referenced_updates[self.referenced_updates.len() - 1];
            self.referenced_updates = vec![first, last];
        }
    }
}

/// Compact every summary and keep only the newest [`MAX_COLD_SUMMARIES`].
fn compact_cold_context(mut cold: Vec<StructuredSummary>) -> Vec<StructuredSummary> {
    if cold.len() > MAX_COLD_SUMMARIES {
        cold.drain(..cold.len() - MAX_COLD_SUMMARIES);
    }
    for summary in &mut cold {
        summary.compact();
    }
    cold
}

/// Build the hot tier, reusing the `Arc` allocations of `incremental_updates`
/// for entries present in both (the common case), so a loaded session keeps
/// one copy of each recent update.
fn hot_context_sharing(
    hot: Vec<ContextUpdate>,
    incremental: &[Arc<ContextUpdate>],
    max_size: usize,
) -> HotContext {
    // Hot entries are the most recent updates, so only the tail can match.
    let tail_len = incremental.len().min(hot.len().max(max_size));
    let by_id: HashMap<Uuid, &Arc<ContextUpdate>> = incremental[incremental.len() - tail_len..]
        .iter()
        .map(|u| (u.id, u))
        .collect();
    HotContext::from_updates(
        hot.into_iter().map(|u| match by_id.get(&u.id) {
            Some(shared) => Arc::clone(shared),
            None => Arc::new(u),
        }),
        max_size,
    )
}

/// A reference to a code region associated with a context update.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CodeReference {
    /// File path of the referenced code
    pub file_path: String,
    /// Start line number (1-based)
    pub start_line: u32,
    /// End line number (inclusive)
    pub end_line: u32,
    /// The actual code snippet
    pub code_snippet: String,
    /// Git commit hash, if available
    pub commit_hash: Option<String>,
    /// Git branch name, if available
    pub branch: Option<String>,
    /// Human-readable description of the change
    pub change_description: String,
}

// Remove duplicate CodeReference definition since we're using the one from core
// Removed duplicate field declarations - using CodeReference from core::context_update
// No extra closing brace needed here

/// A record of a change event within the session.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ChangeRecord {
    /// Unique identifier for this change record
    pub id: Uuid,
    /// When the change occurred
    pub timestamp: DateTime<Utc>,
    /// Categorisation of the change type
    pub change_type: String,
    /// Human-readable description of the change
    pub description: String,
    /// ID of the originating context update, if any
    pub related_update_id: Option<Uuid>,
}

/// User-configurable session preferences.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UserPreferences {
    /// Whether automatic saving is enabled
    pub auto_save_enabled: bool,
    /// How many days to retain context before pruning
    pub context_retention_days: u32,
    /// Maximum number of entries in the hot context tier
    pub max_hot_context_size: usize,
    /// Number of updates before auto-generating a summary
    pub auto_summary_threshold: usize,
    /// Keywords the user considers important for prioritisation
    pub important_keywords: Vec<String>,
}

// Serialize through the borrowed view - no component is cloned.
impl Serialize for ActiveSession {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.serializable_view().serialize(serializer)
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

        let metadata = SessionMetadata::with_created_at(
            data.id,
            data.name,
            data.description,
            data.user_preferences,
            data.created_at,
        );

        let hot_context = Arc::new(hot_context_sharing(
            data.hot_context,
            &data.incremental_updates,
            max_hot_size,
        ));

        // Reconstruct vectorized_update_ids DashSet from Vec
        let vectorized_ids = Arc::new(DashSet::new());
        for id in data.vectorized_update_ids {
            vectorized_ids.insert(id);
        }

        // Wrap deserialized data in Arc for CoW semantics
        Ok(ActiveSession {
            metadata: Arc::new(metadata),
            last_updated: data.last_updated,
            hot_context,
            warm_context: Arc::new(data.warm_context),
            cold_context: Arc::new(compact_cold_context(data.cold_context)),
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

/// Largest update description kept, in bytes; longer ones are cut. Long
/// enough for whole answers, code and logs: search embeds a summary of long
/// records and returns them in full.
pub const MAX_DESCRIPTION_BYTES: usize = 64 * 1024;

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
    /// Create a new empty session with default preferences.
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

        let incremental_updates: Vec<Arc<ContextUpdate>> =
            incremental_updates.into_iter().map(Arc::new).collect();
        let hot_context = hot_context_sharing(hot_context_vec, &incremental_updates, max_hot_size);

        let vectorized_ids = Arc::new(DashSet::new());
        for vid in vectorized_update_ids {
            vectorized_ids.insert(vid);
        }

        Self {
            metadata: Arc::new(metadata),
            last_updated,
            hot_context: Arc::new(hot_context),
            warm_context: Arc::new(warm_context),
            cold_context: Arc::new(compact_cold_context(cold_context)),
            current_state: Arc::new(current_state),
            incremental_updates: Arc::new(incremental_updates),
            code_references: Arc::new(code_references),
            change_history: Arc::new(change_history.into_iter().map(Arc::new).collect()),
            entity_graph: Arc::new(entity_graph),
            vectorized_update_ids: vectorized_ids,
        }
    }

    /// Borrowed serialisation view; see [`ActiveSessionView`].
    pub fn serializable_view(&self) -> ActiveSessionView<'_> {
        ActiveSessionView {
            session: self,
            include_incremental_updates: true,
        }
    }

    /// Replace `incremental_updates` with owned updates (e.g. from a checkpoint).
    pub fn set_incremental_updates(&mut self, updates: Vec<ContextUpdate>) {
        self.incremental_updates = Arc::new(updates.into_iter().map(Arc::new).collect());
    }

    /// Deep-cloned copy of `incremental_updates` (for checkpoints / exports).
    pub fn incremental_updates_owned(&self) -> Vec<ContextUpdate> {
        self.incremental_updates
            .iter()
            .map(|u| ContextUpdate::clone(u))
            .collect()
    }

    /// Replace `change_history` with owned records (e.g. from a checkpoint).
    pub fn set_change_history(&mut self, history: Vec<ChangeRecord>) {
        self.change_history = Arc::new(history.into_iter().map(Arc::new).collect());
    }

    /// Deep-cloned copy of `change_history` (for checkpoints / exports).
    pub fn change_history_owned(&self) -> Vec<ChangeRecord> {
        self.change_history
            .iter()
            .map(|c| ChangeRecord::clone(c))
            .collect()
    }

    /// Mutable access to the hot tier with copy-on-write semantics.
    ///
    /// `HotContext` mutates through `&self` (DashMap), so pushing into a
    /// shared `Arc<HotContext>` would also change every other session
    /// version holding it - e.g. the version still published in the
    /// caller's ArcSwap while a CAS loop retries. This forks the hot tier
    /// (refcount bumps only) whenever it is shared.
    fn hot_context_mut(&mut self) -> &HotContext {
        Arc::make_mut(&mut self.hot_context)
    }

    // Convenience getters for metadata fields
    /// Returns the session's unique identifier.
    pub fn id(&self) -> Uuid {
        self.metadata.id
    }

    // ---- Vectorisation pending-state (TODO.md item #4) ------------------
    //
    // The write path is non-blocking: `update_context` returns once the
    // entry is durably persisted, and the background pipeline computes its
    // embedding shortly after. Between persist and vector-index insertion
    // the entry exists in `hot_context` / `incremental_updates` but is not
    // yet searchable via HNSW — i.e. it's *pending vectorisation*. These
    // helpers expose that distinction without duplicating state: pending =
    // (entry exists in context) AND (id not in `vectorized_update_ids`).

    /// Returns `true` if `entry_id` is queued for vectorisation but the
    /// embedding has not yet landed in the vector index.
    pub fn is_vectorization_pending(&self, entry_id: Uuid) -> bool {
        if self.vectorized_update_ids.contains(&entry_id) {
            return false;
        }
        // Hot tier first (small, most likely to hold a fresh entry), then
        // the incremental sweep, newest first. Neither clones an update.
        self.hot_context.contains_id(&entry_id)
            || self
                .incremental_updates
                .iter()
                .rev()
                .any(|u| u.id == entry_id)
    }

    /// Returns the number of distinct context entries currently waiting for
    /// the embedding pipeline. Useful as a per-session counterpart to
    /// `Pipeline::backlog()` (which is process-wide).
    ///
    /// Hot entries are normally also in `incremental_updates`; each id is
    /// counted once.
    pub fn pending_vectorization_count(&self) -> usize {
        let pending: std::collections::HashSet<Uuid> = self
            .incremental_updates
            .iter()
            .map(|u| u.id)
            .filter(|id| !self.vectorized_update_ids.contains(id))
            .collect();
        let mut count = pending.len();
        // Hot entries missing from `incremental_updates` (e.g. after a
        // checkpoint restore replaced it) still count. An unvectorised hot
        // entry that is not in `pending` cannot be in `incremental_updates`.
        self.hot_context.for_each(|u| {
            if !pending.contains(&u.id) && !self.vectorized_update_ids.contains(&u.id) {
                count += 1;
            }
        });
        count
    }

    /// Returns the session's display name, if set.
    pub fn name(&self) -> Option<String> {
        self.metadata.name.clone()
    }

    /// Returns the session's description, if set.
    pub fn description(&self) -> Option<String> {
        self.metadata.description.clone()
    }

    /// Returns the session creation timestamp.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.metadata.created_at
    }

    /// Returns a reference to the session's user preferences.
    pub fn user_preferences(&self) -> &UserPreferences {
        &self.metadata.user_preferences
    }

    /// Add an incremental update, processing entity graph, code refs, and state.
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
        let mut limited_update = update;
        if limited_update.content.description.len() > MAX_DESCRIPTION_BYTES {
            truncate_safe(
                &mut limited_update.content.description,
                MAX_DESCRIPTION_BYTES,
            );
            limited_update
                .content
                .description
                .push_str("... (truncated)");
            warn!(
                "ActiveSession: description over {} bytes truncated",
                MAX_DESCRIPTION_BYTES
            );
        }
        if limited_update.content.title.len() > 200 {
            truncate_safe(&mut limited_update.content.title, 190);
            limited_update.content.title.push_str("...");
        }

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

        // Publish to the hot tier and the canonical list only once every
        // step succeeded, so an error leaves no half-applied entry. Both
        // share one allocation; the hot tier is copy-on-write (see
        // `hot_context_mut`), so other session versions are untouched.
        let shared = Arc::new(limited_update);
        self.hot_context_mut().push(Arc::clone(&shared));
        Arc::make_mut(&mut self.incremental_updates).push(shared);

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
        let mut limited_update = update;
        if limited_update.content.description.len() > MAX_DESCRIPTION_BYTES {
            truncate_safe(
                &mut limited_update.content.description,
                MAX_DESCRIPTION_BYTES,
            );
            limited_update
                .content
                .description
                .push_str("... (truncated)");
            warn!(
                "ActiveSession: description over {} bytes truncated",
                MAX_DESCRIPTION_BYTES
            );
        }
        if limited_update.content.title.len() > 200 {
            truncate_safe(&mut limited_update.content.title, 190);
            limited_update.content.title.push_str("...");
        }

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

        // Publish to the hot tier and the canonical list only once every
        // step succeeded, so an error leaves no half-applied entry. Both
        // share one allocation; the hot tier is copy-on-write (see
        // `hot_context_mut`), so other session versions are untouched.
        let shared = Arc::new(limited_update);
        self.hot_context_mut().push(Arc::clone(&shared));
        Arc::make_mut(&mut self.incremental_updates).push(shared);

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
    /// Cleans up the hot context cache, the warm-compressed cache, the
    /// canonical `incremental_updates` vector, and everything derived from
    /// the update that is traceable to it: `change_history` records
    /// (`related_update_id`), `conversation_flow` items (`related_updates`),
    /// the decision / question / concept / spec item it created (matched by
    /// title and timestamp), its `code_references` entry, and its id in the
    /// entity graph's mention lists. Returns true if the entry existed.
    ///
    /// Caller is responsible for persisting the session and (optionally) rebuilding
    /// the entity graph if relations from this update should be revoked.
    pub fn remove_update_by_id(&mut self, entry_id: &Uuid) -> bool {
        // Capture the stored update first so derived state can be traced.
        let removed: Option<Arc<ContextUpdate>> = self
            .incremental_updates
            .iter()
            .find(|u| u.id == *entry_id)
            .cloned()
            .or_else(|| self.hot_context.find(|u| u.id == *entry_id))
            .or_else(|| {
                self.warm_context
                    .iter()
                    .find(|c| c.update.id == *entry_id)
                    .map(|c| Arc::new(c.update.clone()))
            });
        let Some(removed) = removed else {
            return false;
        };

        // Copy-on-write everywhere: only touch (and so only clone) the
        // components that actually reference the update.
        if self.hot_context.contains_id(entry_id) {
            self.hot_context_mut().remove_by_id(entry_id);
        }
        if self.warm_context.iter().any(|c| c.update.id == *entry_id) {
            Arc::make_mut(&mut self.warm_context).retain(|c| c.update.id != *entry_id);
        }
        if self.incremental_updates.iter().any(|u| u.id == *entry_id) {
            Arc::make_mut(&mut self.incremental_updates).retain(|u| u.id != *entry_id);
        }
        if self
            .change_history
            .iter()
            .any(|c| c.related_update_id == Some(*entry_id))
        {
            Arc::make_mut(&mut self.change_history)
                .retain(|c| c.related_update_id != Some(*entry_id));
        }

        self.remove_derived_state(&removed);

        if let Some(code_ref) = &removed.related_code {
            self.remove_code_reference(code_ref);
        }

        if self
            .entity_graph
            .entity_mentions
            .values()
            .any(|ids| ids.contains(entry_id))
        {
            let graph = Arc::make_mut(&mut self.entity_graph);
            for ids in graph.entity_mentions.values_mut() {
                ids.retain(|id| id != entry_id);
            }
            graph.entity_mentions.retain(|_, ids| !ids.is_empty());
        }

        true
    }

    /// Undo what `update_current_state` recorded for `update`.
    fn remove_derived_state(&mut self, update: &ContextUpdate) {
        let id = update.id;
        let title = &update.content.title;
        let ts = update.timestamp;
        let state = &*self.current_state;

        let flow_hit = state
            .conversation_flow
            .iter()
            .any(|f| f.related_updates.contains(&id));
        // The typed item carries no update id; it is the one with this
        // update's title and timestamp (exactly what update_current_state wrote).
        let item_pos = match update.update_type {
            UpdateType::QuestionAnswered => state
                .open_questions
                .iter()
                .position(|q| q.question == *title && q.timestamp == ts),
            UpdateType::DecisionMade => state
                .key_decisions
                .iter()
                .position(|d| d.description == *title && d.timestamp == ts),
            UpdateType::ConceptDefined => state
                .key_concepts
                .iter()
                .position(|c| c.name == *title && c.timestamp == ts),
            UpdateType::RequirementAdded => state
                .technical_specifications
                .iter()
                .position(|r| r.title == *title && r.timestamp == ts),
            UpdateType::ProblemSolved | UpdateType::CodeChanged => None,
        };
        if !flow_hit && item_pos.is_none() {
            return;
        }

        let state = Arc::make_mut(&mut self.current_state);
        if flow_hit {
            for flow in &mut state.conversation_flow {
                flow.related_updates.retain(|u| *u != id);
            }
            state
                .conversation_flow
                .retain(|f| !f.related_updates.is_empty());
        }
        if let Some(pos) = item_pos {
            match update.update_type {
                UpdateType::QuestionAnswered => {
                    state.open_questions.remove(pos);
                }
                UpdateType::DecisionMade => {
                    state.key_decisions.remove(pos);
                }
                UpdateType::ConceptDefined => {
                    state.key_concepts.remove(pos);
                }
                UpdateType::RequirementAdded => {
                    state.technical_specifications.remove(pos);
                }
                UpdateType::ProblemSolved | UpdateType::CodeChanged => {}
            }
        }
    }

    /// Remove the `code_references` entry `add_code_reference` created for
    /// `code_ref` (first exact match), dropping the file key when empty.
    fn remove_code_reference(&mut self, code_ref: &crate::core::context_update::CodeReference) {
        let matches = |r: &CodeReference| {
            r.start_line == code_ref.start_line
                && r.end_line == code_ref.end_line
                && r.code_snippet == code_ref.code_snippet
                && r.change_description == code_ref.change_description
                && r.commit_hash == code_ref.commit_hash
                && r.branch == code_ref.branch
        };
        let Some(pos) = self
            .code_references
            .get(&code_ref.file_path)
            .and_then(|refs| refs.iter().position(matches))
        else {
            return;
        };
        let code_references = Arc::make_mut(&mut self.code_references);
        if let Some(refs) = code_references.get_mut(&code_ref.file_path) {
            refs.remove(pos);
            if refs.is_empty() {
                code_references.remove(&code_ref.file_path);
            }
        }
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
                .is_none_or(|cr| cr.file_path != file_path)
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
        let entities_before = self.entity_graph.entity_count();
        // Start from a fresh graph instead of `make_mut` + `clear`, which
        // would deep-clone a shared graph only to empty it.
        self.entity_graph = Arc::new(SimpleEntityGraph::new());

        // Cheap: clones the outer Arc, not the updates.
        let updates = Arc::clone(&self.incremental_updates);
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
        Arc::make_mut(&mut self.change_history).push(Arc::new(ChangeRecord {
            id: Uuid::new_v4(),
            timestamp: update.timestamp,
            change_type: format!("{:?}", update.update_type),
            description: update.content.description.clone(),
            related_update_id: Some(update.id),
        }));
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
        threshold > 0 && len > 0 && len.is_multiple_of(threshold)
    }

    fn create_periodic_summary(&mut self) -> anyhow::Result<()> {
        // Compact marker: the covered range is the last `threshold` updates.
        // No state snapshot - `current_state` is always available and the
        // snapshots were never read (see `StructuredSummary`).
        let threshold = self.metadata.user_preferences.auto_summary_threshold;
        let updates = &self.incremental_updates;
        let window = &updates[updates.len().saturating_sub(threshold)..];
        let referenced_updates = match (window.first(), window.last()) {
            (Some(first), Some(last)) if first.id != last.id => vec![first.id, last.id],
            (Some(only), _) => vec![only.id],
            _ => Vec::new(),
        };
        let summary = StructuredSummary {
            summary_id: Uuid::new_v4(),
            created_at: Utc::now(),
            context_snapshot: StructuredContext::new(),
            referenced_updates,
            summary_quality: 1.0, // Placeholder - would calculate actual quality
        };

        // Use Arc::make_mut for CoW semantics on cold_context
        let cold = Arc::make_mut(&mut self.cold_context);
        cold.push(summary);
        if cold.len() > MAX_COLD_SUMMARIES {
            cold.drain(..cold.len() - MAX_COLD_SUMMARIES);
        }
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
        for update in self.hot_context.get_recent_arcs(10) {
            summary.push(format!("- {}", update.content.description));
        }
        if summary.is_empty() {
            "No context available".to_string()
        } else {
            summary.join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::context_update::{
        CodeReference as UpdateCodeReference, EntityRelationship, RelationType, UpdateContent,
    };
    use chrono::TimeZone;
    use std::collections::VecDeque;

    /// Blob written by the pre-optimisation serializer (deep-clone
    /// `ActiveSessionData`), captured before the borrowed view existed.
    const FIXTURE_V1: &str = include_str!("../../tests/fixtures/active_session_v1.json");

    /// The previous on-disk struct and its derive-based serde, verbatim in
    /// shape. Used to prove the new serializer is byte-identical and that
    /// older readers can still load what we write.
    #[derive(Serialize, Deserialize)]
    struct LegacyActiveSessionData {
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

    impl LegacyActiveSessionData {
        /// Exactly what the old `Serialize for ActiveSession` built.
        fn from_session(s: &ActiveSession) -> Self {
            Self {
                id: s.metadata.id,
                name: s.metadata.name.clone(),
                description: s.metadata.description.clone(),
                created_at: s.metadata.created_at,
                last_updated: s.last_updated,
                user_preferences: s.metadata.user_preferences.clone(),
                hot_context: VecDeque::from(s.hot_context.snapshot()),
                warm_context: (*s.warm_context).clone(),
                cold_context: (*s.cold_context).clone(),
                current_state: (*s.current_state).clone(),
                incremental_updates: s.incremental_updates_owned(),
                code_references: (*s.code_references).clone(),
                change_history: s.change_history_owned(),
                entity_graph: (*s.entity_graph).clone(),
                vectorized_update_ids: s.vectorized_update_ids.iter().map(|id| *id).collect(),
            }
        }
    }

    fn prefs(max_hot: usize, threshold: usize) -> UserPreferences {
        UserPreferences {
            auto_save_enabled: true,
            context_retention_days: 30,
            max_hot_context_size: max_hot,
            auto_summary_threshold: threshold,
            important_keywords: vec![],
        }
    }

    fn session_with(prefs: UserPreferences) -> ActiveSession {
        ActiveSession::from_components(
            Uuid::new_v4(),
            Some("s".into()),
            None,
            Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            Utc::now(),
            prefs,
            vec![],
            vec![],
            vec![],
            StructuredContext::new(),
            vec![],
            HashMap::new(),
            vec![],
            SimpleEntityGraph::new(),
            vec![],
        )
    }

    fn update(i: u32, update_type: UpdateType, with_code: bool) -> ContextUpdate {
        ContextUpdate {
            id: Uuid::new_v4(),
            timestamp: Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, i).unwrap(),
            update_type,
            content: UpdateContent {
                title: format!("title {i}"),
                description: format!("description {i}"),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            related_code: with_code.then(|| UpdateCodeReference {
                file_path: "src/lib.rs".into(),
                start_line: i,
                end_line: i + 1,
                code_snippet: format!("fn f{i}() {{}}"),
                commit_hash: None,
                branch: None,
                change_description: format!("change {i}"),
            }),
            parent_update: None,
            user_marked_important: false,
            creates_entities: vec![format!("e{i}")],
            creates_relationships: vec![EntityRelationship {
                from_entity: format!("e{i}"),
                to_entity: "hub".into(),
                relation_type: RelationType::DependsOn,
                context: String::new(),
            }],
            references_entities: vec![],
            typed_entities: vec![],
        }
    }

    async fn populated_session() -> ActiveSession {
        let mut s = session_with(prefs(3, 2));
        let types = [
            UpdateType::QuestionAnswered,
            UpdateType::DecisionMade,
            UpdateType::ConceptDefined,
            UpdateType::RequirementAdded,
            UpdateType::CodeChanged,
        ];
        for (i, t) in types.iter().enumerate() {
            s.add_incremental_update(update(i as u32, t.clone(), i % 2 == 0))
                .await
                .unwrap();
        }
        let first = s.incremental_updates[0].id;
        s.vectorized_update_ids.insert(first);
        s
    }

    /// Drop the (compacted) cold tier and sort the vectorized-id set, whose
    /// order depends on the DashSet hasher.
    fn without_cold(mut v: serde_json::Value) -> serde_json::Value {
        let obj = v.as_object_mut().unwrap();
        obj.remove("cold_context");
        if let Some(serde_json::Value::Array(ids)) = obj.get_mut("vectorized_update_ids") {
            ids.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        }
        v
    }

    #[test]
    fn fixture_v1_blob_still_loads_and_round_trips() {
        let loaded: ActiveSession = serde_json::from_str(FIXTURE_V1).unwrap();
        assert_eq!(loaded.incremental_updates.len(), 7);
        assert_eq!(loaded.hot_context.len(), 4);
        assert_eq!(loaded.change_history.len(), 7);
        assert_eq!(loaded.vectorized_update_ids.len(), 2);
        assert_eq!(loaded.entity_graph.get_graph_stats().edge_count, 7);
        // created_at comes from the blob, not from the load time.
        assert_eq!(
            loaded.created_at(),
            Utc.with_ymd_and_hms(2024, 12, 31, 0, 0, 0).unwrap()
        );
        // Hot entries share their allocation with incremental_updates.
        let newest = loaded.hot_context.back_arc().unwrap();
        assert!(Arc::ptr_eq(
            &newest,
            loaded.incremental_updates.last().unwrap()
        ));

        // Old cold-tier snapshots are compacted on load.
        assert_eq!(loaded.cold_context.len(), 2);
        for summary in loaded.cold_context.iter() {
            assert_eq!(summary.context_snapshot, StructuredContext::new());
            assert!(summary.referenced_updates.len() <= 2);
        }

        // Everything except the compacted cold tier re-serialises unchanged.
        let original: serde_json::Value = serde_json::from_str(FIXTURE_V1).unwrap();
        let rewritten = serde_json::to_value(&loaded).unwrap();
        assert_eq!(without_cold(original), without_cold(rewritten));

        // And the old reader accepts the new blob.
        let json = serde_json::to_string(&loaded).unwrap();
        let legacy: LegacyActiveSessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(legacy.incremental_updates.len(), 7);
        assert_eq!(legacy.cold_context.len(), 2);
    }

    #[tokio::test]
    async fn borrowed_serializer_is_byte_identical_to_legacy() {
        let s = populated_session().await;
        let new_json = serde_json::to_string(&s).unwrap();
        let legacy_json =
            serde_json::to_string(&LegacyActiveSessionData::from_session(&s)).unwrap();
        assert_eq!(new_json, legacy_json);

        let back: ActiveSession = serde_json::from_str(&new_json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), new_json);
    }

    #[tokio::test]
    async fn view_without_incremental_updates_keeps_field() {
        let s = populated_session().await;
        let json =
            serde_json::to_string(&s.serializable_view().without_incremental_updates()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["incremental_updates"], serde_json::json!([]));
        assert_eq!(v["hot_context"].as_array().unwrap().len(), 3);
        let _: LegacyActiveSessionData = serde_json::from_str(&json).unwrap();
        let back: ActiveSession = serde_json::from_str(&json).unwrap();
        assert!(back.incremental_updates.is_empty());
        assert_eq!(back.hot_context.len(), 3);
    }

    #[tokio::test]
    async fn cloned_session_push_does_not_touch_original_hot_context() {
        let mut original = session_with(prefs(50, 100));
        original
            .add_incremental_update_fast(update(0, UpdateType::CodeChanged, false))
            .await
            .unwrap();

        // Simulates a CAS retry: two attempts on clones of the same version.
        for attempt in 0..2 {
            let mut attempt_session = original.clone();
            attempt_session
                .add_incremental_update_fast(update(1 + attempt, UpdateType::CodeChanged, false))
                .await
                .unwrap();
            assert_eq!(attempt_session.hot_context.len(), 2);
            assert_eq!(original.hot_context.len(), 1);
        }

        let mut removing = original.clone();
        let id = original.incremental_updates[0].id;
        assert!(removing.remove_update_by_id(&id));
        assert_eq!(removing.hot_context.len(), 0);
        assert_eq!(original.hot_context.len(), 1);
        assert_eq!(original.incremental_updates.len(), 1);
    }

    #[tokio::test]
    async fn remove_update_by_id_cleans_derived_state() {
        let mut s = session_with(prefs(50, 100));
        let decision = update(0, UpdateType::DecisionMade, true);
        let keep = update(1, UpdateType::DecisionMade, true);
        let (decision_id, keep_id) = (decision.id, keep.id);
        s.add_incremental_update(decision).await.unwrap();
        s.add_incremental_update(keep).await.unwrap();
        assert_eq!(s.current_state.key_decisions.len(), 2);
        assert_eq!(s.code_references["src/lib.rs"].len(), 2);

        assert!(s.remove_update_by_id(&decision_id));
        assert!(!s.remove_update_by_id(&decision_id));

        assert_eq!(s.incremental_updates.len(), 1);
        assert!(!s.hot_context.contains_id(&decision_id));
        assert!(
            s.change_history
                .iter()
                .all(|c| c.related_update_id == Some(keep_id))
        );
        assert_eq!(s.change_history.len(), 1);
        assert_eq!(s.current_state.key_decisions.len(), 1);
        assert_eq!(s.current_state.key_decisions[0].description, "title 1");
        assert!(
            s.current_state
                .conversation_flow
                .iter()
                .all(|f| !f.related_updates.contains(&decision_id))
        );
        assert_eq!(s.current_state.conversation_flow.len(), 1);
        assert_eq!(s.code_references["src/lib.rs"].len(), 1);
        assert_eq!(s.code_references["src/lib.rs"][0].start_line, 1);

        assert!(s.remove_update_by_id(&keep_id));
        assert!(s.code_references.is_empty());
        assert!(s.current_state.key_decisions.is_empty());
        assert!(s.current_state.conversation_flow.is_empty());
    }

    #[tokio::test]
    async fn pending_vectorization_counts_each_entry_once() {
        let mut s = session_with(prefs(50, 100));
        for i in 0..3 {
            s.add_incremental_update_fast(update(i, UpdateType::CodeChanged, false))
                .await
                .unwrap();
        }
        // Every entry is in both hot and incremental.
        assert_eq!(s.pending_vectorization_count(), 3);
        let first = s.incremental_updates[0].id;
        s.vectorized_update_ids.insert(first);
        assert_eq!(s.pending_vectorization_count(), 2);
        assert!(!s.is_vectorization_pending(first));
        assert!(s.is_vectorization_pending(s.incremental_updates[1].id));

        // A hot-only entry (incremental replaced, e.g. checkpoint restore).
        s.set_incremental_updates(vec![]);
        assert_eq!(s.pending_vectorization_count(), 2);
    }

    #[tokio::test]
    async fn cold_context_is_compact_and_capped() {
        let mut s = session_with(prefs(5, 1));
        for i in 0..(MAX_COLD_SUMMARIES as u32 + 20) {
            s.add_incremental_update_fast(update(i % 60, UpdateType::CodeChanged, false))
                .await
                .unwrap();
        }
        assert_eq!(s.cold_context.len(), MAX_COLD_SUMMARIES);
        for summary in s.cold_context.iter() {
            assert_eq!(summary.context_snapshot, StructuredContext::new());
            assert!(!summary.referenced_updates.is_empty());
            assert!(summary.referenced_updates.len() <= 2);
        }

        // Oversized legacy cold tiers are capped on load.
        let mut v = serde_json::to_value(&s).unwrap();
        let one = v["cold_context"][0].clone();
        v["cold_context"] = serde_json::Value::Array(vec![one; MAX_COLD_SUMMARIES + 7]);
        let loaded: ActiveSession = serde_json::from_value(v).unwrap();
        assert_eq!(loaded.cold_context.len(), MAX_COLD_SUMMARIES);
    }

    #[test]
    fn hot_context_helpers_do_not_require_cloning() {
        let hot = HotContext::new(2);
        let a = Arc::new(update(0, UpdateType::CodeChanged, false));
        hot.push(Arc::clone(&a));
        hot.push(update(1, UpdateType::CodeChanged, false));
        hot.push(update(2, UpdateType::CodeChanged, false));
        assert!(!hot.contains_id(&a.id));
        let mut seen = Vec::new();
        hot.for_each(|u| seen.push(u.content.title.clone()));
        assert_eq!(seen, vec!["title 1", "title 2"]);
        assert!(hot.any(|u| u.content.title == "title 2"));
        assert_eq!(hot.find(|_| true).unwrap().content.title, "title 1");
        assert_eq!(hot.get_recent_arcs(1)[0].content.title, "title 2");

        let fork = hot.clone();
        fork.push(update(3, UpdateType::CodeChanged, false));
        assert_eq!(hot.back().unwrap().content.title, "title 2");
        assert_eq!(fork.back().unwrap().content.title, "title 3");
    }
}
