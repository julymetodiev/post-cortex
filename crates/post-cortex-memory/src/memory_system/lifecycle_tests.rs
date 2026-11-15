// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Workspace updates and archiving, radar dismissals, message deletion and
//! archive export/import through the memory system, on every backend.

use chrono::Utc;
use uuid::Uuid;

use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::core::dismissal::RadarItemKind;
use post_cortex_core::core::message::{MessageRole, NewMessage};
use post_cortex_core::services::UpdateContextRequest;
use post_cortex_core::workspace::SessionRole;

use super::backends_tests::backends;
use crate::capabilities::UpdateWorkspace;
use crate::capabilities::{CreateWorkspace, Memory, Target, ViewsError, WorkspaceError};

pub(super) async fn workspace(memory: &Memory, name: &str) -> Uuid {
    memory
        .workspaces
        .create(CreateWorkspace {
            name: name.into(),
            description: "d".into(),
        })
        .await
        .unwrap()
        .id
}

/// Renames and archive state are what a restart finds; archived workspaces
/// leave the default list; bad input changes nothing.
#[tokio::test]
async fn workspace_updates_and_archive_survive_a_restart() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let ws = workspace(&memory, "ws").await;
        let other = workspace(&memory, "other").await;
        let sid = memory.sessions.create(None, None).await.unwrap();
        memory
            .workspaces
            .add_session(ws, sid, SessionRole::Primary)
            .await
            .unwrap();

        let updated = memory
            .workspaces
            .update(
                ws,
                UpdateWorkspace {
                    name: Some("  renamed ".into()),
                    description: Some("new".into()),
                    ..UpdateWorkspace::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.name, "renamed", "{}", backend.name);
        for bad in [Some(String::new()), Some("x".repeat(201))] {
            assert!(matches!(
                memory
                    .workspaces
                    .update(
                        ws,
                        UpdateWorkspace {
                            name: bad,
                            description: None,
                            ..UpdateWorkspace::default()
                        }
                    )
                    .await,
                Err(WorkspaceError::InvalidArgument(_))
            ));
        }
        assert!(matches!(
            memory
                .workspaces
                .update(
                    ws,
                    UpdateWorkspace {
                        name: None,
                        description: None,
                        ..UpdateWorkspace::default()
                    }
                )
                .await,
            Err(WorkspaceError::InvalidArgument(_))
        ));
        assert!(matches!(
            memory
                .workspaces
                .update(
                    ws,
                    UpdateWorkspace {
                        name: Some("other".into()),
                        description: None,
                        ..UpdateWorkspace::default()
                    }
                )
                .await,
            Err(WorkspaceError::Conflict(_))
        ));
        assert!(matches!(
            memory
                .workspaces
                .update(
                    Uuid::new_v4(),
                    UpdateWorkspace {
                        name: Some("x".into()),
                        description: None,
                        ..UpdateWorkspace::default()
                    }
                )
                .await,
            Err(WorkspaceError::NotFound(_))
        ));

        let (archived, changed) = memory.workspaces.set_archived(ws, true).await.unwrap();
        assert!(changed && archived.is_archived());
        let archived_at = archived.archived_at();
        let (_, changed) = memory.workspaces.set_archived(ws, true).await.unwrap();
        assert!(!changed, "archiving twice keeps the first time");
        let listed: Vec<Uuid> = memory.workspaces.list(false).iter().map(|w| w.id).collect();
        assert_eq!(listed, [other], "{}", backend.name);
        drop(memory);

        let restarted = backend.memory().await;
        let ws_after = restarted.workspaces.get(ws).unwrap();
        assert_eq!(
            (ws_after.name.as_str(), ws_after.description.as_str()),
            ("renamed", "new"),
            "{}",
            backend.name
        );
        assert_eq!(ws_after.archived_at(), archived_at, "{}", backend.name);
        assert_eq!(ws_after.get_session_role(&sid), Some(SessionRole::Primary));
        assert_eq!(
            restarted.workspaces.find_by_name("renamed").map(|w| w.id),
            Some(ws)
        );

        let (_, changed) = restarted.workspaces.set_archived(ws, false).await.unwrap();
        assert!(changed);
        drop(restarted);
        let again = backend.memory().await;
        assert!(
            !again.workspaces.get(ws).unwrap().is_archived(),
            "{}",
            backend.name
        );
        assert!(again.workspaces.delete(ws).await.unwrap());
        assert!(!again.workspaces.delete(ws).await.unwrap());
        drop(again);
        assert!(
            backend.memory().await.workspaces.get(ws).is_err(),
            "{}",
            backend.name
        );
    }
}

/// Dismissals persist per workspace and apply to the radar of the
/// workspace and of its member sessions.
#[tokio::test]
async fn radar_dismissals_persist_and_apply_to_member_sessions() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let ws = workspace(&memory, "ws").await;
        let sid = memory.sessions.create(None, None).await.unwrap();
        let outsider = memory.sessions.create(None, None).await.unwrap();
        memory
            .workspaces
            .add_session(ws, sid, SessionRole::Primary)
            .await
            .unwrap();

        let d = memory
            .views
            .dismiss_radar_item(
                ws,
                RadarItemKind::StaleEntity,
                " redis ",
                Some("ann".into()),
            )
            .await
            .unwrap();
        assert_eq!(
            (d.key.as_str(), d.author.as_deref()),
            ("redis", Some("ann"))
        );
        assert!(matches!(
            memory
                .views
                .dismiss_radar_item(ws, RadarItemKind::StaleEntity, "  ", None)
                .await,
            Err(ViewsError::InvalidArgument(_))
        ));
        assert!(matches!(
            memory
                .views
                .dismiss_radar_item(Uuid::new_v4(), RadarItemKind::StaleEntity, "x", None)
                .await,
            Err(ViewsError::WorkspaceNotFound(_))
        ));
        drop(memory);

        let memory = backend.memory().await;
        let listed = memory.views.radar_dismissals(ws).await.unwrap();
        assert_eq!(listed.len(), 1, "{}", backend.name);
        for (target, expected) in [
            (Target::Workspace(ws), 1),
            (Target::Session(sid), 1),
            (Target::Session(outsider), 0),
        ] {
            assert_eq!(
                memory.views.dismissals_for(target).await.unwrap().len(),
                expected,
                "{}",
                backend.name
            );
        }
        assert!(
            memory
                .views
                .restore_radar_item(ws, RadarItemKind::StaleEntity, "redis")
                .await
                .unwrap()
        );
        assert!(
            !memory
                .views
                .restore_radar_item(ws, RadarItemKind::StaleEntity, "redis")
                .await
                .unwrap()
        );
        assert!(memory.views.radar_dismissals(ws).await.unwrap().is_empty());
    }
}

pub(super) fn turn(turn_id: &str) -> NewMessage {
    NewMessage {
        turn_id: turn_id.into(),
        role: MessageRole::User,
        author: None,
        source_session: None,
        timestamp: Utc::now(),
        text: format!("turn {turn_id}"),
        tools: vec![],
        files: vec![],
        parent: None,
        blocks: Vec::new(),
    }
}

/// One turn, then the rest of the stream, are deleted; stats follow and a
/// record drawn from a deleted turn keeps its provenance id.
#[tokio::test]
async fn messages_are_deleted_and_records_keep_provenance() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let sid = memory.sessions.create(None, None).await.unwrap();
        memory
            .messages
            .append(post_cortex_core::services::AppendMessagesRequest {
                session_id: sid,
                messages: vec![turn("a"), turn("b"), turn("c")],
            })
            .await
            .unwrap();
        let entity = |name: &str| EntityData {
            aliases: Vec::new(),
            name: name.to_string(),
            entity_type: EntityType::Concept,
            first_mentioned: Utc::now(),
            last_mentioned: Utc::now(),
            mention_count: 1,
            importance_score: 1.0,
            description: None,
        };
        let written = memory
            .records
            .write(UpdateContextRequest {
                session_id: sid,
                interaction_type: UpdateType::DecisionMade,
                content: UpdateContent {
                    title: "Keep turns".into(),
                    description: "drawn from b".into(),
                    details: vec![],
                    examples: vec![],
                    implications: vec![],
                },
                entities: vec![entity("turns"), entity("stream")],
                relations: vec![EntityRelationship {
                    from_entity: "turns".into(),
                    to_entity: "stream".into(),
                    relation_type: RelationType::RelatedTo,
                    context: "test".into(),
                }],
                code_reference: None,
                author: None,
                derived_from: vec!["message:1".into()],
                meta: Default::default(),
            })
            .await
            .unwrap();

        assert!(memory.messages.delete(sid, 1).await.unwrap());
        assert!(!memory.messages.delete(sid, 1).await.unwrap());
        let stats = memory.messages.stats(sid).await.unwrap();
        assert_eq!(
            (stats.count, stats.last_seq),
            (2, Some(2)),
            "{}",
            backend.name
        );
        assert_eq!(memory.messages.delete_all(sid).await.unwrap(), 2);
        assert_eq!(memory.messages.stats(sid).await.unwrap().count, 0);
        assert!(memory.messages.delete(Uuid::new_v4(), 0).await.is_err());

        let session = memory.sessions.get(sid).await.unwrap();
        let kept = session
            .records()
            .iter()
            .find(|u| u.id == written.entry_id)
            .unwrap();
        assert_eq!(kept.derived_from, ["message:1"], "{}", backend.name);
    }
}

/// A created workspace is in storage when the call returns (a restart finds
/// it), and the one create path is what project resolution uses.
#[tokio::test]
async fn a_created_workspace_is_stored_before_it_is_returned() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let ws = memory
            .workspaces
            .create(CreateWorkspace {
                name: " made ".into(),
                description: "here".into(),
            })
            .await
            .unwrap();
        assert_eq!(ws.name, "made", "the name is trimmed");

        let restarted = backend.memory().await;
        let found = restarted
            .workspaces
            .get(ws.id)
            .unwrap_or_else(|_| panic!("{}: workspace not stored", backend.name));
        assert_eq!(found.name, "made", "{}", backend.name);
        assert_eq!(
            found.created_at, ws.created_at,
            "{}: a restart keeps the creation time",
            backend.name
        );
        for bad in ["", "   ", &"x".repeat(201)] {
            assert!(matches!(
                memory
                    .workspaces
                    .create(CreateWorkspace {
                        name: bad.to_string(),
                        description: String::new(),
                    })
                    .await,
                Err(WorkspaceError::InvalidArgument(_))
            ));
        }
        assert_eq!(memory.workspaces.count(), 1, "{}", backend.name);
    }
}
