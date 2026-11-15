// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Merging an import into existing data: cards by history and time, and
//! messages and their embeddings by position.

use crate::capabilities::transfer::merge::{CardMerge, merge_card, restored_embedding};
use crate::capabilities::transfer::tests::{of, preview_of};
use crate::capabilities::transfer::*;
use chrono::{DateTime, Utc};
use post_cortex_core::archive::*;
use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_ports::storage::{CardStore, WorkspaceStore};
use std::collections::HashMap;
use uuid::Uuid;

fn card(version: u64, text: &str, minutes: i64) -> Card {
    Card {
        scope: CardScope::User,
        version,
        text: text.to_string(),
        updated_at: DateTime::<Utc>::from_timestamp(1_700_000_000 + minutes * 60, 0).unwrap(),
        author: Some(format!("author {version}")),
    }
}

/// A card is written whole into an empty target, continued when one
/// history extends the other, left when the latest texts match. On
/// divergence the version numbers are not compared: the target's
/// latest version stays unless the import asks for the archive's newer
/// one (`cards_newer`, by `updated_at`) or overwrites.
#[test]
fn cards_merge_by_history_then_time_when_asked() {
    let keep = ImportOptions {
        skip_existing: true,
        ..Default::default()
    };
    let newer = ImportOptions {
        cards_newer: true,
        ..keep.clone()
    };
    let overwrite = ImportOptions {
        overwrite: true,
        ..Default::default()
    };
    let (a1, a2, a3) = (card(1, "a", 0), card(2, "b", 1), card(3, "c", 2));
    let both = [a1.clone(), a2.clone()];
    for options in [&keep, &newer, &overwrite] {
        assert_eq!(
            merge_card(&both, &[], options),
            (CardMerge::Add(both.to_vec()), None)
        );
        assert_eq!(merge_card(&both, &both, options), (CardMerge::Keep, None));
        assert_eq!(
            merge_card(
                &[a1.clone(), a2.clone(), a3.clone()],
                std::slice::from_ref(&a1),
                options
            ),
            (CardMerge::Add(vec![a2.clone(), a3.clone()]), None)
        );
        assert_eq!(
            merge_card(std::slice::from_ref(&a1), &both, options),
            (CardMerge::Keep, None)
        );
        assert_eq!(merge_card(&[], &both, options), (CardMerge::Keep, None));
    }

    // The observed case: the target's v1 was written a day after the
    // archive's v2. Version 2 does not make the archive's newer.
    let here = [card(1, "current", 24 * 60)];
    let archive = [card(1, "old", 0), card(2, "older head", 60)];
    for options in [&keep, &newer, &ImportOptions::default()] {
        assert_eq!(
            merge_card(&archive, &here, options),
            (CardMerge::Keep, Some(ConflictResolution::Keep))
        );
    }
    // The archive's head written later: kept unless asked for.
    let here = [card(1, "earlier", 0)];
    assert_eq!(
        merge_card(&archive, &here, &keep),
        (CardMerge::Keep, Some(ConflictResolution::Keep))
    );
    assert_eq!(
        merge_card(&archive, &here, &newer),
        (
            CardMerge::Add(vec![Card {
                version: 2,
                ..archive[1].clone()
            }]),
            Some(ConflictResolution::Update)
        )
    );
    // A higher target version written earlier still loses with
    // cards_newer: only time counts.
    let ours = [a1.clone(), card(2, "x", -5), card(3, "y", -4)];
    assert_eq!(
        merge_card(&both, &ours, &newer),
        (
            CardMerge::Add(vec![Card {
                version: 4,
                ..a2.clone()
            }]),
            Some(ConflictResolution::Update)
        )
    );
    // Same time: the target stays.
    assert_eq!(
        merge_card(&both, &[a1.clone(), card(2, "theirs", 1)], &newer),
        (CardMerge::Keep, Some(ConflictResolution::Keep))
    );
    // Overwrite replaces the history, whichever is newer.
    let ours = [a1.clone(), card(2, "theirs", 5)];
    assert_eq!(
        merge_card(&both, &ours, &overwrite),
        (
            CardMerge::Replace(both.to_vec()),
            Some(ConflictResolution::Overwrite)
        )
    );
    // After an update the latest texts match: a re-import is quiet.
    let updated = [
        a1.clone(),
        card(2, "theirs", 0),
        Card {
            version: 3,
            ..a3.clone()
        },
    ];
    assert_eq!(
        merge_card(&[a1, a2, a3], &updated, &newer),
        (CardMerge::Keep, None)
    );
}

/// A card conflict in a preview carries both latest versions, and its
/// description shows their dates and first lines.
#[tokio::test]
async fn card_conflicts_show_both_heads() {
    let ws = Uuid::new_v4();
    let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
    let scoped = |version, text: &str, when| Card {
        scope: CardScope::Workspace(ws),
        version,
        text: text.to_string(),
        updated_at: at(when),
        author: None,
    };
    let mut export = ExportData::new(
        "1.4.1",
        "",
        ExportType::Full,
        CompressionType::None,
        chrono::Utc::now(),
    );
    export.workspaces.push(ExportedWorkspace {
        id: ws,
        name: "post-cortex".to_string(),
        slug: None,
        description: String::new(),
        sessions: vec![],
        projects: vec![],
        archived_at: None,
        remote_rerank: false,
        created_at: 0,
        dismissals: vec![],
    });
    export.cards = vec![
        scoped(1, "first", "2026-10-19T10:00:00Z"),
        scoped(2, "\n# Production card\nmore", "2026-10-20T14:42:00Z"),
    ];
    let dir = tempfile::tempdir().unwrap();
    let target = post_cortex_storage::RealRocksDBStorage::new(dir.path())
        .await
        .unwrap();
    target
        .save_workspace_metadata(ws, "post-cortex", &ws.to_string(), "", &[])
        .await
        .unwrap();
    target
        .restore_card_versions(
            CardScope::Workspace(ws),
            vec![scoped(1, "# Staging card", "2026-10-21T08:01:00Z")],
            true,
        )
        .await
        .unwrap();
    let target = of(&target);
    let skip = ImportOptions {
        skip_existing: true,
        ..Default::default()
    };
    let preview = preview_of(&target, &export, &skip).await.unwrap();
    let conflict = preview.conflicts.iter().find(|c| c.kind == "card").unwrap();
    assert_eq!(conflict.resolution, ConflictResolution::Keep);
    let heads = conflict.card.as_ref().unwrap();
    assert_eq!(
        (heads.target.version, heads.target.first_line.as_str()),
        (1, "# Staging card")
    );
    assert_eq!(
        (heads.archive.version, heads.archive.first_line.as_str()),
        (2, "# Production card")
    );
    assert_eq!(
        conflict.describe(),
        format!(
            "card of workspace \"post-cortex\" ({ws}): keep; \
             here v1, 2026-10-21 08:01 UTC: \"# Staging card\"; \
             archive v2, 2026-10-20 14:42 UTC: \"# Production card\""
        )
    );
    // Asking for the newer one does not take an older archive head.
    let newer = ImportOptions {
        cards_newer: true,
        ..skip
    };
    let preview = preview_of(&target, &export, &newer).await.unwrap();
    let conflict = preview.conflicts.iter().find(|c| c.kind == "card").unwrap();
    assert_eq!(conflict.resolution, ConflictResolution::Keep);
}

/// Message vectors follow their message to its stored position (chunks
/// and metadata too) or are dropped with it; other vectors pass.
#[test]
fn message_embeddings_follow_their_message() {
    let sid = Uuid::new_v4().to_string();
    let vector = |id: &str, content_type: &str, meta: &[(&str, &str)]| ExportedEmbedding {
        content_id: id.to_string(),
        session_id: sid.clone(),
        vector: vec![1.0],
        text: String::new(),
        content_type: content_type.to_string(),
        timestamp: Utc::now().to_rfc3339(),
        metadata: meta
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    };
    let message_type = post_cortex_ports::MESSAGE_CONTENT_TYPE;
    let embeddings = vec![
        vector("message:0", message_type, &[("seq", "0")]),
        vector(
            "message:0#2",
            message_type,
            &[("seq", "0"), ("parent", "message:0")],
        ),
        vector("message:1", message_type, &[("seq", "1")]),
        vector("message:5", message_type, &[("seq", "5")]),
        vector("entity:x", "entity", &[]),
    ];
    let positions: HashMap<(String, u64), u64> = [((sid.clone(), 0), 7), ((sid.clone(), 5), 5)]
        .into_iter()
        .collect();
    let kept: Vec<ExportedEmbedding> = embeddings
        .into_iter()
        .filter_map(|e| restored_embedding(e, &positions))
        .collect();
    let ids: Vec<&str> = kept.iter().map(|e| e.content_id.as_str()).collect();
    assert_eq!(ids, ["message:7", "message:7#2", "message:5", "entity:x"]);
    assert_eq!(kept[1].metadata["seq"], "7");
    assert_eq!(kept[1].metadata["parent"], "message:7");
    assert!(!kept[3].metadata.contains_key("seq"));
}
