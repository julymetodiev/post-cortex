// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::rocksdb_storage::rows::WorkspaceRow;
use serde::Serialize;

/// Mirror of the legacy layout for writing old rows in tests.
#[derive(Serialize)]
struct OldUpdate<'a> {
    id: Uuid,
    timestamp: DateTime<Utc>,
    update_type: UpdateType,
    content: &'a UpdateContent,
    related_code: Option<CodeReference>,
    parent_update: Option<Uuid>,
    user_marked_important: bool,
    creates_entities: Vec<String>,
    creates_relationships: Vec<EntityRelationship>,
    references_entities: Vec<String>,
    typed_entities: Vec<TypedEntity>,
}

fn content() -> UpdateContent {
    UpdateContent {
        title: "decided".to_string(),
        description: "use rocksdb".to_string(),
        details: vec!["d".to_string()],
        examples: Vec::new(),
        implications: Vec::new(),
    }
}

fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    bincode::serde::encode_to_vec(v, bincode::config::standard()).unwrap()
}

#[test]
fn old_update_rows_still_decode() {
    let content = content();
    let id = Uuid::new_v4();
    let old = encode(&OldUpdate {
        id,
        timestamp: Utc::now(),
        update_type: UpdateType::DecisionMade,
        content: &content,
        related_code: None,
        parent_update: None,
        user_marked_important: true,
        creates_entities: vec!["RocksDB".to_string()],
        creates_relationships: Vec::new(),
        references_entities: Vec::new(),
        typed_entities: Vec::new(),
    });
    let update = decode_update(&old).expect("legacy row");
    assert_eq!(update.id, id);
    assert_eq!(update.content.description, "use rocksdb");
    assert!(update.user_marked_important);
    assert_eq!(update.author, None);

    let authored = encode(&(
        OldUpdate {
            id,
            timestamp: Utc::now(),
            update_type: UpdateType::DecisionMade,
            content: &content,
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: Vec::new(),
            creates_relationships: Vec::new(),
            references_entities: Vec::new(),
            typed_entities: Vec::new(),
        },
        Some("hermes:eq-lead"),
    ));
    let update = decode_update(&authored).expect("authored row");
    assert_eq!(update.author.as_deref(), Some("hermes:eq-lead"));
    assert!(update.derived_from.is_empty());

    let derived = encode(&(
        OldUpdate {
            id,
            timestamp: Utc::now(),
            update_type: UpdateType::DecisionMade,
            content: &content,
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: Vec::new(),
            creates_relationships: Vec::new(),
            references_entities: Vec::new(),
            typed_entities: Vec::new(),
        },
        Some("claude-code"),
        vec!["message:3"],
    ));
    let update = decode_update(&derived).expect("derived row");
    assert_eq!(update.author.as_deref(), Some("claude-code"));
    assert_eq!(update.derived_from, ["message:3"]);
    assert_eq!(update.meta, Default::default());

    let mut new = update.clone();
    new.meta.supersedes = vec![Uuid::new_v4()];
    new.meta.confidence = Some(0.5);
    let decoded = decode_update(&encode(&new)).expect("current row");
    assert_eq!(decoded.derived_from, ["message:3"]);
    assert_eq!(decoded.meta, new.meta);
}

/// Mirror of the workspace row before `projects`.
#[derive(Serialize)]
struct OldWorkspace {
    id: Uuid,
    name: String,
    description: String,
    sessions: Vec<(Uuid, SessionRole)>,
    created_at: u64,
}

/// Unmarked workspace rows decode with no projects, marked ones with
/// theirs.
#[test]
fn workspace_rows_of_both_layouts_decode() {
    let (id, session) = (Uuid::new_v4(), Uuid::new_v4());
    let old = decode_workspace(&encode(&OldWorkspace {
        id,
        name: "ws".to_string(),
        description: "d".to_string(),
        sessions: vec![(session, SessionRole::Primary)],
        created_at: 7,
    }))
    .expect("legacy workspace row");
    assert_eq!((old.id, old.name.as_str(), old.created_at), (id, "ws", 7));
    assert!(old.projects.is_empty());

    let project = post_cortex_core::workspace::WorkspaceProject {
        key: "github.com/a/b".to_string(),
        session_id: session,
    };
    // A pcx-ws-v2 row: the legacy fields followed by the projects.
    let mut v2 = WORKSPACE_V2.to_vec();
    v2.extend(encode(&(
        OldWorkspace {
            id,
            name: "ws".to_string(),
            description: "d".to_string(),
            sessions: vec![],
            created_at: 7,
        },
        vec![project.clone()],
    )));
    let with_projects = decode_workspace(&v2).expect("v2 workspace row");
    let project_row = |p: &post_cortex_core::workspace::WorkspaceProject| {
        WorkspaceRow::from_projects(std::slice::from_ref(p))
    };
    assert_eq!(with_projects.projects, project_row(&project));
    assert_eq!(with_projects.archived_at, None);

    // A pcx-ws-v3 row: with `archived_at` after the projects.
    let mut v3 = WORKSPACE_V3.to_vec();
    v3.extend(encode(&(
        OldWorkspace {
            id,
            name: "ws".to_string(),
            description: "d".to_string(),
            sessions: vec![(session, SessionRole::Related)],
            created_at: 7,
        },
        vec![project.clone()],
        Some(9u64),
    )));
    let current = decode_workspace(&v3).expect("v3 workspace row");
    assert_eq!(current.projects, project_row(&project));
    assert_eq!(current.created_at, 7);
    assert_eq!(current.archived_at, Some(9));
}

/// Unmarked membership rows decode, the role kept.
#[test]
fn membership_rows_decode() {
    #[derive(Serialize)]
    struct OldMembership {
        workspace_id: Uuid,
        session_id: Uuid,
        role: SessionRole,
        added_at: u64,
    }
    let (w, s) = (Uuid::new_v4(), Uuid::new_v4());
    let row = decode_membership(&encode(&OldMembership {
        workspace_id: w,
        session_id: s,
        role: SessionRole::Shared,
        added_at: 5,
    }))
    .unwrap();
    assert_eq!((row.workspace_id, row.session_id, row.added_at), (w, s, 5));
    assert_eq!(row.role().unwrap(), SessionRole::Shared);
}

/// Mirror of the checkpoint layout around updates of type `U`.
#[derive(Serialize)]
struct OldCheckpoint<U> {
    id: Uuid,
    session_id: Uuid,
    created_at: DateTime<Utc>,
    structured_context: StructuredContext,
    recent_updates: Vec<U>,
    code_references: HashMap<String, Vec<CodeReference>>,
    change_history: Vec<ChangeRecord>,
    total_updates: usize,
    context_quality_score: f32,
    compression_ratio: f32,
}

fn checkpoint<U>(recent_updates: Vec<U>) -> OldCheckpoint<U> {
    OldCheckpoint {
        id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        created_at: Utc::now(),
        structured_context: StructuredContext::new(),
        recent_updates,
        code_references: HashMap::new(),
        change_history: Vec::new(),
        total_updates: 9,
        context_quality_score: 0.5,
        compression_ratio: 2.0,
    }
}

/// Each checkpoint layout is read by its marker, and the fields after
/// the updates come through intact.
#[test]
fn checkpoints_of_every_layout_decode() {
    let content = content();
    let old = || OldUpdate {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        update_type: UpdateType::DecisionMade,
        content: &content,
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: Vec::new(),
        creates_relationships: Vec::new(),
        references_entities: Vec::new(),
        typed_entities: Vec::new(),
    };

    let legacy = decode_checkpoint(&encode(&checkpoint(vec![old(), old()]))).unwrap();
    assert_eq!(legacy.recent_updates.len(), 2);
    assert_eq!(legacy.total_updates, 9);

    let mut v2 = CHECKPOINT_V2.to_vec();
    v2.extend(encode(&checkpoint(vec![(old(), Some("alice"))])));
    let authored = decode_checkpoint(&v2).unwrap();
    assert_eq!(authored.recent_updates[0].author.as_deref(), Some("alice"));
    assert_eq!(authored.compression_ratio, 2.0);

    let mut v3 = CHECKPOINT_V3.to_vec();
    v3.extend(encode(&checkpoint(vec![(
        old(),
        Some("bob"),
        vec!["message:1"],
    )])));
    let derived = decode_checkpoint(&v3).unwrap();
    assert_eq!(derived.recent_updates[0].derived_from, ["message:1"]);
    assert_eq!(derived.total_updates, 9);

    let mut update = derived.recent_updates[0].clone();
    update.meta.retired_at = Some(Utc::now());
    let mut v4 = CHECKPOINT_V4.to_vec();
    v4.extend(encode(&checkpoint(vec![update.clone()])));
    let current = decode_checkpoint(&v4).unwrap();
    assert_eq!(current.recent_updates[0].meta, update.meta);
    assert_eq!(current.compression_ratio, 2.0);
}
