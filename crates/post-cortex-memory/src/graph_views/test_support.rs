// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Records and scopes for the view tests, built without a memory system.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use post_cortex_core::core::context_update::{
    CodeReference, ContextUpdate, EntityRelationship, EntityType, RelationType, TypedEntity,
    UpdateContent, UpdateMeta, UpdateType,
};
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::session::active_session::ActiveSession;
use uuid::Uuid;

use crate::graph_views::GraphScope;

/// Records are written `minutes` after a fixed start.
pub fn at(minutes: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + Duration::days(20_000) + Duration::minutes(minutes)
}

/// A record to build (see [`rec`]).
pub struct Rec {
    minutes: i64,
    kind: UpdateType,
    title: &'static str,
    description: &'static str,
    entities: Vec<(&'static str, EntityType)>,
    relations: Vec<(&'static str, &'static str, RelationType)>,
    meta: UpdateMeta,
    author: &'static str,
    file: Option<&'static str>,
    derived_from: Vec<String>,
}

/// A record of `kind` titled `title`, written at [`at`]`(minutes)` by
/// `claude-code`, described "because".
pub fn rec(minutes: i64, kind: UpdateType, title: &'static str) -> Rec {
    Rec {
        minutes,
        kind,
        title,
        description: "because",
        entities: Vec::new(),
        relations: Vec::new(),
        meta: UpdateMeta::default(),
        author: "claude-code",
        file: None,
        derived_from: Vec::new(),
    }
}

impl Rec {
    /// It names `entities`.
    pub fn about(mut self, entities: &[(&'static str, EntityType)]) -> Self {
        self.entities = entities.to_vec();
        self
    }
    /// It asserts `a` -`r`-> `b`.
    pub fn rel(mut self, a: &'static str, b: &'static str, r: RelationType) -> Self {
        self.relations.push((a, b, r));
        self
    }
    /// Written by `author`.
    pub fn by(mut self, author: &'static str) -> Self {
        self.author = author;
        self
    }
    /// It points at `path`.
    pub fn file(mut self, path: &'static str) -> Self {
        self.file = Some(path);
        self
    }
    /// Its description.
    pub fn described(mut self, description: &'static str) -> Self {
        self.description = description;
        self
    }
    /// It replaces `ids`.
    pub fn supersedes(mut self, ids: &[Uuid]) -> Self {
        self.meta.supersedes = ids.to_vec();
        self
    }
    /// Drawn from the turns `sources` (`message:<seq>`).
    pub fn drawn_from(mut self, sources: &[&str]) -> Self {
        self.derived_from = sources.iter().map(|s| s.to_string()).collect();
        self
    }
    /// The record.
    pub fn update(&self) -> ContextUpdate {
        ContextUpdate {
            id: Uuid::new_v4(),
            timestamp: at(self.minutes),
            update_type: self.kind,
            content: UpdateContent {
                title: self.title.to_string(),
                description: self.description.to_string(),
                details: Vec::new(),
                examples: Vec::new(),
                implications: Vec::new(),
            },
            related_code: self.file.map(|f| CodeReference {
                file_path: f.to_string(),
                start_line: 1,
                end_line: 2,
                code_snippet: String::new(),
                commit_hash: None,
                branch: None,
                change_description: String::new(),
            }),
            parent_update: None,
            user_marked_important: false,
            creates_entities: Vec::new(),
            creates_relationships: self
                .relations
                .iter()
                .map(|(a, b, r)| EntityRelationship {
                    from_entity: a.to_string(),
                    to_entity: b.to_string(),
                    relation_type: *r,
                    context: format!("{a} {} {b}", r.as_str()),
                })
                .collect(),
            references_entities: Vec::new(),
            typed_entities: self
                .entities
                .iter()
                .map(|(n, t)| TypedEntity {
                    name: n.to_string(),
                    entity_type: *t,
                })
                .collect(),
            author: Some(self.author.into()),
            derived_from: self.derived_from.clone(),
            meta: self.meta.clone(),
        }
    }
}

/// One session named "main" holding `records`, in order.
pub async fn scope(records: Vec<ContextUpdate>) -> GraphScope {
    let mut session = ActiveSession::new(Uuid::new_v4(), Some("main".into()), None, Utc::now());
    for update in records {
        session.apply_record(update);
    }
    GraphScope {
        graph: Arc::clone(session.entity_graph_shared()),
        sessions: vec![Arc::new(session)],
    }
}

/// One session per list of records, their graphs merged as a workspace
/// scope merges them.
pub async fn workspace(members: Vec<Vec<ContextUpdate>>) -> GraphScope {
    let mut sessions = Vec::new();
    let mut graph = SimpleEntityGraph::new();
    for records in members {
        let member = scope(records).await;
        graph.merge_from(&member.graph);
        sessions.extend(member.sessions);
    }
    GraphScope {
        sessions,
        graph: Arc::new(graph),
    }
}
