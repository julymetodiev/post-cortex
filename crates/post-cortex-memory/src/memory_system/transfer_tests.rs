// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Archives, cards and messages move between memory systems by export
//! and import, on every backend.

use super::MemoryRuntime;
use super::backends_tests::backends;
use super::lifecycle_tests::{turn, workspace};
use super::write_path_tests::text_record;
use post_cortex_core::workspace::SessionRole;

use post_cortex_core::archive::{ExportOptions, ExportScope, ImportOptions};
use uuid::Uuid;

/// Export from one system and import into a fresh one: the counts match,
/// the workspace (archived) and its records come back, and a preview of a
/// second import reports conflicts without writing.
#[tokio::test]
async fn archives_round_trip_between_systems() {
    let sources = backends().await;
    let targets = backends().await;
    for (source_backend, target_backend) in sources.iter().zip(&targets) {
        let source = source_backend.start().await;
        let mem = crate::capabilities::test_support::memory(&source);
        let ws = workspace(&mem, "ws").await;
        let sid = source
            .mem
            .sessions
            .create(Some("s".into()), None)
            .await
            .unwrap();
        mem.workspaces
            .add_session(ws, sid, SessionRole::Related)
            .await
            .unwrap();
        for title in ["one", "two", "three"] {
            source
                .mem
                .records
                .commit(sid, text_record(title), false)
                .await
                .unwrap();
        }
        let lone = source
            .mem
            .sessions
            .create(Some("lone".into()), None)
            .await
            .unwrap();
        mem.workspaces.set_archived(ws, true).await.unwrap();

        let all = source
            .mem
            .transfer
            .export_data(ExportScope::All, ExportOptions::default())
            .await
            .unwrap();
        assert_eq!(
            (
                all.metadata.session_count,
                all.metadata.workspace_count,
                all.metadata.update_count
            ),
            (2, 1, 3),
            "{}",
            source_backend.name
        );
        let scoped = source
            .mem
            .transfer
            .export_data(ExportScope::Workspace(ws), ExportOptions::default())
            .await
            .unwrap();
        assert_eq!(scoped.session_ids(), [sid]);
        assert!(
            source
                .mem
                .transfer
                .export_data(
                    ExportScope::Workspace(Uuid::new_v4()),
                    ExportOptions::default()
                )
                .await
                .is_err()
        );

        let target = target_backend.start().await;
        let preview = target
            .mem
            .transfer
            .preview_data(&all, &ImportOptions::default())
            .await
            .unwrap();
        assert_eq!(
            (preview.sessions, preview.workspaces, preview.updates),
            (2, 1, 3)
        );
        assert!(preview.conflicts.is_empty());
        let result = target
            .mem
            .transfer
            .import_data(all.clone(), ImportOptions::default())
            .await
            .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(
            (
                result.sessions_imported,
                result.workspaces_imported,
                result.updates_imported
            ),
            (2, 1, 3),
            "{}",
            target_backend.name
        );
        let imported = target.workspace_manager.get_workspace(&ws).unwrap();
        assert!(imported.is_archived());
        assert_eq!(imported.get_session_role(&sid), Some(SessionRole::Related));
        let session = target.get_session(sid).await.unwrap().load_full();
        assert_eq!(session.records().len(), 3, "{}", target_backend.name);
        assert!(target.get_session(lone).await.is_ok());

        // Exported again from the target, the counts are the source's.
        let back = target
            .mem
            .transfer
            .export_data(ExportScope::All, ExportOptions::default())
            .await
            .unwrap();
        assert_eq!(
            (
                back.metadata.session_count,
                back.metadata.workspace_count,
                back.metadata.update_count
            ),
            (2, 1, 3)
        );

        let again = target
            .mem
            .transfer
            .preview_data(
                &all,
                &ImportOptions {
                    skip_existing: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(again.conflicts.len(), 3, "{}", target_backend.name);
    }
}

async fn page(
    memory: &super::backends_tests::TestSystem,
    sid: Uuid,
) -> Vec<post_cortex_core::core::message::Message> {
    memory
        .mem
        .messages
        .page(
            sid,
            post_cortex_core::core::message::MessageCursor::From(0),
            10,
            Default::default(),
        )
        .await
        .unwrap()
}

async fn history(
    memory: &MemoryRuntime,
    scope: post_cortex_core::core::card::CardScope,
) -> Vec<post_cortex_core::core::card::Card> {
    memory.stores.cards.card_history(scope, 10).await.unwrap()
}

/// Cards (user and workspace, two versions) and the message stream go from
/// one system to a fresh one; importing again changes nothing; a system
/// that has the session and its own (later) card merges the messages
/// while the session is skipped, keeps its card, and its preview reports
/// the card with both latest versions.
#[tokio::test]
async fn cards_and_messages_round_trip_between_systems() {
    use crate::capabilities::ConflictResolution;
    use post_cortex_core::core::card::CardScope;

    let sources = backends().await;
    let targets = backends().await;
    let others = backends().await;
    for ((source_backend, target_backend), other_backend) in
        sources.iter().zip(&targets).zip(&others)
    {
        let name = target_backend.name;
        let source = source_backend.start().await;
        let mem = crate::capabilities::test_support::memory(&source);
        let ws = workspace(&mem, "ws").await;
        let sid = source
            .mem
            .sessions
            .create(Some("s".into()), None)
            .await
            .unwrap();
        mem.workspaces
            .add_session(ws, sid, SessionRole::Primary)
            .await
            .unwrap();
        source
            .mem
            .messages
            .store(sid, vec![turn("a"), turn("b"), turn("c")])
            .await
            .unwrap();
        let actor = &source.stores.cards;
        let ws_card = CardScope::Workspace(ws);
        actor
            .put_card(CardScope::User, "me".into(), Some("julius".into()), 0)
            .await
            .unwrap();
        actor
            .put_card(ws_card, "v1".into(), Some("claude-code".into()), 0)
            .await
            .unwrap();
        actor
            .put_card(ws_card, "v2".into(), Some("hermes".into()), 1)
            .await
            .unwrap();
        let all = source
            .mem
            .transfer
            .export_data(ExportScope::All, ExportOptions::default())
            .await
            .unwrap();
        assert_eq!(
            (all.metadata.message_count, all.metadata.card_count),
            (3, 3),
            "{name}"
        );

        let target = target_backend.start().await;
        let preview = target
            .mem
            .transfer
            .preview_data(&all, &ImportOptions::default())
            .await
            .unwrap();
        assert_eq!(
            (preview.messages, preview.messages_new, preview.cards),
            (3, 3, 2)
        );
        let result = target
            .mem
            .transfer
            .import_data(all.clone(), ImportOptions::default())
            .await
            .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!((result.messages_imported, result.cards_imported), (3, 2));
        assert_eq!(page(&target, sid).await, page(&source, sid).await, "{name}");
        assert_eq!(
            history(&target, ws_card).await,
            history(&source, ws_card).await
        );
        assert_eq!(
            history(&target, CardScope::User).await,
            history(&source, CardScope::User).await
        );

        let skip = ImportOptions {
            skip_existing: true,
            ..Default::default()
        };
        let again = target
            .mem
            .transfer
            .import_data(all.clone(), skip.clone())
            .await
            .unwrap();
        assert_eq!(
            (
                again.messages_imported,
                again.cards_imported,
                again.cards_skipped
            ),
            (0, 0, 2),
            "{name}"
        );
        assert_eq!(target.mem.messages.stats(sid).await.unwrap().count, 3);

        // A system with the session, one turn of it and its own card.
        let other = other_backend.start().await;
        other
            .mem
            .transfer
            .import_data(
                source
                    .mem
                    .transfer
                    .export_data(
                        ExportScope::Sessions(vec![sid]),
                        ExportOptions {
                            include_messages: false,
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap(),
                ImportOptions::default(),
            )
            .await
            .unwrap();
        other
            .mem
            .messages
            .store(sid, vec![turn("b")])
            .await
            .unwrap();
        other
            .stores
            .cards
            .put_card(ws_card, "mine".into(), None, 0)
            .await
            .unwrap();
        let preview = other.mem.transfer.preview_data(&all, &skip).await.unwrap();
        assert_eq!(preview.messages_new, 2, "{name}");
        assert!(preview.conflicts.iter().any(|c| {
            c.kind == "card"
                && c.id == ws
                && c.resolution == ConflictResolution::Keep
                && c.card
                    .as_ref()
                    .is_some_and(|h| h.target.first_line == "mine" && h.archive.first_line == "v2")
        }));
        let merged = other
            .mem
            .transfer
            .import_data(all.clone(), skip.clone())
            .await
            .unwrap();
        assert!(merged.errors.is_empty(), "{:?}", merged.errors);
        assert_eq!(
            (
                merged.sessions_skipped,
                merged.messages_imported,
                merged.messages_skipped
            ),
            (1, 2, 1),
            "{name}"
        );
        let turns: Vec<String> = page(&other, sid)
            .await
            .into_iter()
            .map(|m| m.turn_id)
            .collect();
        assert_eq!(turns, ["b", "a", "c"], "{name}");
        let texts: Vec<String> = history(&other, ws_card)
            .await
            .into_iter()
            .map(|c| c.text)
            .collect();
        // Its own card, written after the archive's, stays (diverged
        // histories are not decided by version numbers).
        assert_eq!(texts, ["mine"], "{name}");
    }
}
