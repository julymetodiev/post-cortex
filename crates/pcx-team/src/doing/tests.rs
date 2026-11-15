// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The members' work over ports held in memory.

use async_trait::async_trait;
use pcx_documents::InMemoryDocuments;

use super::*;
use crate::domain::Agent;
use crate::ports::Heading;
use crate::wake::{Change, Thread};

/// A board with one task, `t1`.
struct OneTask;

#[async_trait]
impl Board for OneTask {
    async fn change(&self, _: &str, _: &str, _: u64) -> Result<Option<Change>, ModuleError> {
        Ok(None)
    }

    async fn heading(&self, task: &str) -> Result<Option<Heading>, ModuleError> {
        Ok((task == "t1").then(|| Heading {
            title: "Ship".into(),
            column: "doing".into(),
        }))
    }

    async fn thread(&self, _: &str) -> Result<Thread, ModuleError> {
        Ok(Thread::default())
    }

    fn answer_tool(&self) -> &str {
        "comment"
    }

    async fn answer(&self, _: &str, _: &str, _: Option<&str>) -> Result<(), ModuleError> {
        Ok(())
    }
}

fn member(name: &str) -> Agent {
    Agent {
        name: name.into(),
        role: "works".into(),
        authority: crate::domain::Authority::Discuss.into(),
        harness: true,
        ..Agent::default()
    }
}

#[tokio::test]
async fn the_members_work_says_its_task_and_whether_it_runs() {
    let docs = InMemoryDocuments::new(None, post_cortex_proto::TEAM_DESCRIPTOR_SET).unwrap();
    let teams = Service::new(&docs);
    for name in ["dev", "ops"] {
        teams.add_agent(member(name)).await.unwrap();
    }
    teams
        .create_team("core", vec!["dev".into()], "ws")
        .await
        .unwrap();
    teams
        .create_team("infra", vec!["ops".into()], "ws")
        .await
        .unwrap();
    let works = Typed::new(&docs, Kind::Work.document());
    for (agent, task) in [("dev", "t1"), ("ops", "gone")] {
        let work = Work {
            agent: agent.into(),
            task: task.into(),
            session_id: format!("s-{agent}"),
            turns: 2,
            ..Work::default()
        };
        works.create(&work, agent).await.unwrap();
    }
    let doing = Doing {
        docs: &docs,
        board: &OneTask,
    };
    let busy = [crate::progress::Busy {
        task: "t1".to_string(),
        member: "dev".to_string(),
        quiet_seconds: 3,
        received: 40,
    }];

    let every = doing.list(&WorkFilter::default(), &busy).await.unwrap();
    assert_eq!(every.len(), 2);
    let core = doing
        .list(
            &WorkFilter {
                team: Some("core"),
                ..WorkFilter::default()
            },
            &busy,
        )
        .await
        .unwrap();
    assert_eq!(core.len(), 1, "only the team's members");
    let dev = &core[0];
    assert_eq!(
        (
            dev.agent.as_str(),
            dev.title.as_str(),
            dev.column.as_str(),
            dev.session_id.as_str(),
            dev.turns,
            dev.busy
        ),
        ("dev", "Ship", "doing", "s-dev", 2, true)
    );
    assert!(dev.updated_at.is_some());
    let ops = doing
        .list(
            &WorkFilter {
                team: Some("infra"),
                ..WorkFilter::default()
            },
            &busy,
        )
        .await
        .unwrap();
    assert_eq!(
        (ops[0].title.as_str(), ops[0].busy),
        ("", false),
        "a gone task has no title"
    );

    let running = doing
        .list(
            &WorkFilter {
                busy_only: true,
                ..WorkFilter::default()
            },
            &busy,
        )
        .await
        .unwrap();
    assert!(!running.is_empty());
    assert!(running.iter().all(|w| w.busy), "{running:?}");
    let on_t1 = doing
        .list(
            &WorkFilter {
                task: Some("t1"),
                ..WorkFilter::default()
            },
            &busy,
        )
        .await
        .unwrap();
    assert!(!on_t1.is_empty());
    assert!(on_t1.iter().all(|w| w.task == "t1"), "{on_t1:?}");
}
