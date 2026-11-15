// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use pcx_documents::InMemoryDocuments;

use super::*;

fn wake(reach: Option<Vec<String>>) -> Wake {
    Wake {
        member: "dev".into(),
        task: "t1".into(),
        answers: Some("c1".into()),
        why: Why::Mentioned,
        reach,
    }
}

async fn work(docs: &InMemoryDocuments) -> Item<Work> {
    let work = Work {
        agent: "dev".into(),
        task: "t1".into(),
        session_id: "s1".into(),
        ..Work::default()
    };
    Typed::new(docs, Kind::Work.document())
        .create(&work, "dev")
        .await
        .unwrap()
}

#[tokio::test]
async fn work_kept_as_going_comes_back_as_it_woke_and_goes_when_forgotten() {
    let docs = InMemoryDocuments::new(None, post_cortex_proto::TEAM_DESCRIPTOR_SET).unwrap();
    let made = work(&docs).await;
    assert!(kept(&docs).await.unwrap().is_empty());

    mark(&docs, made, &wake(Some(vec!["w1".into()])))
        .await
        .unwrap();
    let found = kept(&docs).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].data.session_id, "s1");
    let back = wake_of(&found[0].data).unwrap();
    assert_eq!(back.why, Why::Resumed);
    assert_eq!(back.answers.as_deref(), Some("c1"));
    assert_eq!(back.reach, Some(vec!["w1".to_string()]));

    forget(&docs, found.into_iter().next().unwrap())
        .await
        .unwrap();
    assert!(kept(&docs).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_wake_without_a_reach_or_a_comment_comes_back_without_them() {
    let docs = InMemoryDocuments::new(None, post_cortex_proto::TEAM_DESCRIPTOR_SET).unwrap();
    let made = work(&docs).await;
    let woke = Wake {
        answers: None,
        ..wake(None)
    };
    mark(&docs, made, &woke).await.unwrap();
    let back = wake_of(&kept(&docs).await.unwrap()[0].data).unwrap();
    assert_eq!(back.answers, None);
    assert_eq!(back.reach, None);
}

#[test]
fn a_column_closes_its_tasks_by_its_stage_and_the_last_one_without_stages() {
    let columns = ["todo".to_string(), "done".to_string()];
    let stages = || [Stage::Open, Stage::Done].into_iter();
    assert!(!column_closed(&columns, stages(), "todo"));
    assert!(column_closed(&columns, stages(), "done"));
    assert!(!column_closed(&columns, std::iter::empty(), "todo"));
    assert!(column_closed(&columns, std::iter::empty(), "done"));
    assert!(column_closed(&columns, stages(), "gone"));
}
