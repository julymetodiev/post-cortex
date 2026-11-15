// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::capabilities::records::good_request;
use crate::capabilities::test_support::rocksdb_memory;
use crate::capabilities::{Memory, ScopeError};
use post_cortex_core::core::context_update::UpdateType;

async fn with_decisions(n: usize) -> (Memory, Uuid, tempfile::TempDir) {
    let (memory, dir) = rocksdb_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    for i in 0..n {
        let mut req = good_request();
        req.session_id = sid;
        req.interaction_type = UpdateType::DecisionMade;
        req.content.title = format!("Decision {i}");
        memory.records.write(req).await.unwrap();
    }
    (memory, sid, dir)
}

#[test]
fn include_names_sections() {
    let parse = |names: &[&str]| {
        let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        SummarySection::parse_include(Some(&names))
    };
    assert!(SummarySection::parse_include(None).unwrap().is_none());
    assert!(parse(&["decisions", "ALL"]).unwrap().is_none());
    assert_eq!(
        parse(&["Insights", "bogus", "decisions"]).unwrap().unwrap(),
        [SummarySection::Decisions, SummarySection::Insights]
    );
    assert!(matches!(
        parse(&["bogus"]),
        Err(ViewsError::InvalidArgument(_))
    ));
}

/// One summary operation: each section keeps its own limit, insights only
/// when asked for (review W4 item 10: insights used the decisions limit).
#[tokio::test]
async fn sections_keep_their_own_limits() {
    let (memory, sid, _dir) = with_decisions(3).await;
    let summary = memory
        .views
        .summary(SummaryRequest {
            session_id: sid,
            sections: Some(vec![SummarySection::Decisions, SummarySection::Insights]),
            decisions_limit: Some(1),
            entities_limit: Some(1),
            insights_limit: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(summary.compaction, Compaction::None);
    assert_eq!(summary.view.key_decisions.len(), 1);
    assert_eq!(summary.view.entity_summaries.len(), 1);
    assert_eq!(summary.insights.as_ref().map(Vec::len), Some(1));

    let whole = memory
        .views
        .summary(SummaryRequest {
            session_id: sid,
            compact: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(whole.compaction, Compaction::Requested);
    assert!(whole.insights.is_none(), "not asked for");
    assert_eq!(whole.view.key_decisions.len(), 3);

    assert_eq!(memory.views.key_decisions(sid).await.unwrap().len(), 3);
    assert!(memory.views.key_insights(sid, Some(1)).await.unwrap().len() <= 1);
    assert!(matches!(
        memory
            .views
            .summary(SummaryRequest {
                session_id: Uuid::new_v4(),
                ..Default::default()
            })
            .await,
        Err(ViewsError::Scope(ScopeError::NotFound(_)))
    ));
}

#[tokio::test]
async fn views_render_over_a_session() {
    let (memory, sid, _dir) = with_decisions(1).await;
    let entity = ViewRequest::parse(
        "entity",
        &[("entity".to_string(), "Foo".to_string())].into(),
    )
    .unwrap();
    let rendered = memory
        .views
        .view(Target::Session(sid), entity)
        .await
        .unwrap();
    assert!(matches!(rendered, RenderedView::Entity(_)));
    let ghost = ViewRequest::parse(
        "entity",
        &[("entity".to_string(), "Ghost".to_string())].into(),
    )
    .unwrap();
    assert!(matches!(
        memory.views.view(Target::Session(sid), ghost).await,
        Err(ViewsError::View(ViewError::EntityNotFound(_)))
    ));
}
