// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The views as the app shows them (`pcx.views.v1`): the graph, the
//! timeline, the record browser, records in full with their provenance, the
//! overview, the entity page, topics, radar, files, conversations and
//! search hits. Each is computed on
//! the call's [`crate::graph_views::Index`], built once, so a call that shows
//! several views (an entity with its impact and lineage, an overview with its
//! radar) indexes the scope once.

mod browse;
mod conversation;
mod entity;
mod found;
mod graph;
mod overview;
mod record;
mod types;

pub use browse::{InvalidCursor, page_after};
pub use conversation::{
    ConversationPage, ConversationSummary, Conversations, DrawnFrom, PageAt, TurnDetail,
    neighbours, turn,
};
pub use found::{Found, FoundItem, SearchHitDetail, Turns};
pub use types::{
    AuthorCount, EdgeDetail, EntityDetail, EntityRef, EventKind, FileDetail, FileTouch, Files,
    GraphDetail, ImpactDetail, LineageDetail, LineageStepDetail, NodeDetail, Overview, PageCursor,
    Provenance, RadarCounts, RadarDetail, RecordDetail, RecordFilter, RecordPage, RecordRelation,
    RiskDetail, SessionStat, Timeline, TimelineEvent, TopicDetail, TopicsDetail, ValidityFilter,
};

#[cfg(test)]
mod tests;
