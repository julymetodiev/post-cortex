// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Radar dismissals: per workspace, replaced by kind and key, restored
//! once, and going with the workspace.

use chrono::Utc;
use uuid::Uuid;

use post_cortex_core::core::dismissal::{RadarDismissal, RadarItemKind};

use crate::storage::Stores;

/// Dismissals are per workspace, replaced by kind and key, restored once,
/// and go with the workspace.
pub async fn dismissals_are_per_workspace(stores: &Stores) {
    let (workspace, other) = (Uuid::new_v4(), Uuid::new_v4());
    for ws in [workspace, other] {
        stores
            .workspaces
            .save_workspace_metadata(ws, &ws.to_string(), &ws.to_string(), "", &[])
            .await
            .unwrap();
    }
    let dismissal = |kind, key: &str, author: Option<&str>| RadarDismissal {
        kind,
        key: key.to_string(),
        dismissed_at: Utc::now(),
        author: author.map(str::to_string),
    };
    let question = "Should we shard by tenant? (a: yes | b: no)";
    for d in [
        dismissal(RadarItemKind::OpenQuestion, question, Some("ann")),
        dismissal(RadarItemKind::StaleEntity, "redis", None),
        // Replaces the first one.
        dismissal(RadarItemKind::OpenQuestion, question, Some("bob")),
    ] {
        stores
            .dismissals
            .put_dismissal(workspace, &d)
            .await
            .unwrap();
    }
    stores
        .dismissals
        .put_dismissal(other, &dismissal(RadarItemKind::StaleEntity, "redis", None))
        .await
        .unwrap();

    let mut listed = stores.dismissals.list_dismissals(workspace).await.unwrap();
    listed.sort_by(|a, b| (a.kind, &a.key).cmp(&(b.kind, &b.key)));
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].kind, RadarItemKind::OpenQuestion);
    assert_eq!(listed[0].key, question);
    assert_eq!(listed[0].author.as_deref(), Some("bob"));

    assert!(
        stores
            .dismissals
            .delete_dismissal(workspace, RadarItemKind::StaleEntity, "redis")
            .await
            .unwrap()
    );
    assert!(
        !stores
            .dismissals
            .delete_dismissal(workspace, RadarItemKind::StaleEntity, "redis")
            .await
            .unwrap()
    );
    assert_eq!(
        stores
            .dismissals
            .list_dismissals(workspace)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        stores
            .dismissals
            .list_dismissals(other)
            .await
            .unwrap()
            .len(),
        1
    );

    stores.workspaces.delete_workspace(workspace).await.unwrap();
    assert!(
        stores
            .dismissals
            .list_dismissals(workspace)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores
            .dismissals
            .list_dismissals(other)
            .await
            .unwrap()
            .len(),
        1
    );
}
