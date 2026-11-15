// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A written record is stored as the session holds it.

use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::session::active_session::MAX_DESCRIPTION_BYTES;

use super::backends_tests::backends;

/// A description over the size limit is cut once, by the domain, and the
/// cut record is what the call returns, the session holds and storage
/// keeps (the stored row used to be the caller's uncut copy).
#[tokio::test]
async fn an_oversized_record_is_stored_as_the_session_holds_it() {
    for backend in backends().await {
        let memory = backend.start().await;
        let sid = memory.mem.sessions.create(None, None).await.unwrap();
        let mut update: ContextUpdate = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(),
            "timestamp": chrono::Utc::now(),
            "update_type": "DecisionMade",
            "content": {
                "title": "t".repeat(300),
                "description": "é".repeat(MAX_DESCRIPTION_BYTES),
                "details": [], "examples": [], "implications": []
            },
            "related_code": null,
            "parent_update": null,
            "user_marked_important": false,
            "creates_entities": [],
            "creates_relationships": [],
            "references_entities": []
        }))
        .unwrap();
        update.author = Some("test".into());

        let committed = memory
            .mem
            .records
            .commit(sid, update.clone(), true)
            .await
            .unwrap();
        assert!(committed.durable, "{}", backend.name);

        let held = memory.get_session(sid).await.unwrap().load_full();
        let returned = held.records()[0].content.clone();
        assert!(returned.description.ends_with("... (truncated)"));
        assert!(returned.title.len() < 300);

        let stored = memory
            .session_manager
            .load_stored(sid)
            .await
            .unwrap()
            .expect("stored session");
        let stored = &stored.session.records()[0].content;
        assert_eq!(stored.description, returned.description, "{}", backend.name);
        assert_eq!(stored.title, returned.title, "{}", backend.name);
    }
}
