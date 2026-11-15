// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The chief service over documents held in memory, a scripted agent and a
//! fixed day.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_documents::InMemoryDocuments;
use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnGrant, TurnStop};
use pcx_module_api::model::{StopReason, Usage};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::domain::Kind;

const TODAY: &str = "2026-10-25";

struct Today;

impl Clock for Today {
    fn today(&self) -> String {
        TODAY.to_string()
    }
}

#[derive(Default)]
struct Made {
    count: Mutex<u32>,
}

#[async_trait]
impl Sessions for Made {
    async fn create(&self, _: &str) -> Result<String, ModuleError> {
        let mut count = self.count.lock();
        *count += 1;
        Ok(format!("session-{count}"))
    }
}

/// Calls `task_add` (answered) and `comment` (no result: cancelled), uses
/// 10 input, 5 output, 1000 cache-read and 3 cache-write tokens; keeps the
/// requests and the scope's author.
#[derive(Default)]
struct Scripted {
    asked: Mutex<Vec<(TurnRequest, Option<String>)>>,
}

#[async_trait]
impl Turns for Scripted {
    async fn run(
        &self,
        scope: &Scope,
        request: &TurnRequest,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        self.asked
            .lock()
            .push((request.clone(), scope.author.clone()));
        for event in [
            TurnEvent::Text("on it".into()),
            TurnEvent::ToolCall {
                id: "1".into(),
                name: "task_add".into(),
                input: json!({ "title": "ship" }),
            },
            TurnEvent::ToolResult {
                id: "1".into(),
                content: "added".into(),
                is_error: false,
            },
            TurnEvent::ToolCall {
                id: "2".into(),
                name: "comment".into(),
                input: json!({}),
            },
        ] {
            if events.send(event).await.is_err() {
                return Err(ModuleError::new(ModuleErrorKind::Internal, "no reader"));
            }
        }
        Ok(TurnEnd {
            stop: TurnStop::Answered(StopReason::EndTurn),
            steps: 2,
            usage: Usage {
                input: 10,
                output: 5,
                cache_read: 1000,
                cache_write: 3,
            },
            answer: "done".into(),
        })
    }
}

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: Some("w1".into()),
        author: Some("chief".into()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(5),
        cancel: CancellationToken::new(),
    }
}

fn settings(budget: u64) -> Settings {
    Settings {
        workspace: "w1".into(),
        name: "chief".into(),
        instructions: "Run the place.".into(),
        model: Some("glm-5.3".into()),
        budget,
        grant: TurnGrant {
            person_tools: vec!["team_create".into()],
            withheld: Vec::new(),
        },
    }
}

const JULIUS: Asker<'static> = Asker {
    author: "julius",
    channel: "telegram",
};

async fn actions(docs: &InMemoryDocuments) -> Vec<Action> {
    Typed::new(docs, Kind::Action.document())
        .all(None)
        .await
        .unwrap()
        .into_iter()
        .map(|a: Item<Action>| a.data)
        .collect()
}

async fn days(docs: &InMemoryDocuments) -> Vec<Day> {
    Typed::new(docs, Kind::Day.document())
        .all(None)
        .await
        .unwrap()
        .into_iter()
        .map(|d: Item<Day>| d.data)
        .collect()
}

#[tokio::test]
async fn a_question_runs_a_turn_records_its_calls_and_counts_its_tokens() {
    let (docs, sessions, turns) = (
        InMemoryDocuments::new(Some("chief"), post_cortex_proto::CHIEF_DESCRIPTOR_SET).unwrap(),
        Made::default(),
        Scripted::default(),
    );
    let (settings, scope) = (settings(100), scope());
    let chief = Chief {
        docs: &docs,
        sessions: &sessions,
        days: &docs,
        turns: &turns,
        clock: &Today,
        settings: &settings,
        scope: &scope,
    };
    let asked = chief
        .ask(JULIUS, "Ship it", Conversation::New, None)
        .await
        .unwrap();
    assert_eq!(asked.end.answer, "done");
    assert_eq!(asked.end.stop, TurnStop::Answered(StopReason::EndTurn));
    let talk: Item<Talk> = chief.talk_docs().get(&asked.conversation).await.unwrap();
    assert_eq!(
        talk.data,
        Talk {
            asked_by: "julius".into(),
            channel: "telegram".into(),
            session_id: "session-1".into(),
            answer: "done".into(),
            ..Talk::default()
        }
    );
    assert_eq!(talk.document.text, "Ship it");
    {
        let asked = turns.asked.lock();
        let (request, author) = &asked[0];
        assert_eq!(request.session, "session-1");
        assert_eq!(request.prompt, "Ship it");
        assert_eq!(request.instructions, "Run the place.");
        assert_eq!(request.model.as_deref(), Some("glm-5.3"));
        assert_eq!(author.as_deref(), Some("chief"));
    }

    // The answered call and the one no result came for, both recorded.
    let mut recorded: Vec<(String, Option<bool>, String)> = actions(&docs)
        .await
        .into_iter()
        .map(|a| (a.tool, a.failed, a.asked_by))
        .collect();
    recorded.sort();
    assert_eq!(
        recorded,
        [
            ("comment".into(), None, "julius".into()),
            ("task_add".into(), Some(false), "julius".into()),
        ]
    );
    assert_eq!(
        days(&docs).await,
        [Day {
            date: TODAY.into(),
            tokens: 18
        }]
    );

    // The conversation goes on in its session; the day adds up.
    let again = chief
        .ask(
            JULIUS,
            "And now?",
            Conversation::Talk(&asked.conversation),
            None,
        )
        .await
        .unwrap();
    assert_eq!(again.conversation, asked.conversation);
    assert_eq!(turns.asked.lock()[1].0.session, "session-1");
    assert_eq!(days(&docs).await[0].tokens, 36);
}

#[tokio::test]
async fn only_who_started_a_conversation_goes_on_with_it_from_its_channel() {
    let (docs, sessions, turns) = (
        InMemoryDocuments::new(Some("chief"), post_cortex_proto::CHIEF_DESCRIPTOR_SET).unwrap(),
        Made::default(),
        Scripted::default(),
    );
    let (settings, scope) = (settings(1000), scope());
    let chief = Chief {
        docs: &docs,
        sessions: &sessions,
        days: &docs,
        turns: &turns,
        clock: &Today,
        settings: &settings,
        scope: &scope,
    };
    let asked = chief
        .ask(JULIUS, "Ship it", Conversation::New, None)
        .await
        .unwrap();
    for other in [
        Asker {
            author: "mallory",
            channel: "telegram",
        },
        Asker {
            author: "julius",
            channel: "",
        },
    ] {
        let refused = chief
            .ask(
                other,
                "Tell me",
                Conversation::Talk(&asked.conversation),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(refused.kind, ModuleErrorKind::Denied, "{refused:?}");
    }
    let missing = chief
        .ask(JULIUS, "Hi", Conversation::Talk("nope"), None)
        .await
        .unwrap_err();
    assert_eq!(missing.kind, ModuleErrorKind::NotFound);
    assert_eq!(turns.asked.lock().len(), 1);
}

#[tokio::test]
async fn a_spent_budget_refuses_the_next_turn_and_the_one_that_crosses_it_runs() {
    let (docs, sessions, turns) = (
        InMemoryDocuments::new(Some("chief"), post_cortex_proto::CHIEF_DESCRIPTOR_SET).unwrap(),
        Made::default(),
        Scripted::default(),
    );
    // Two documents of the day (two turns made it at once) count together.
    let kind = Typed::new(&docs, Kind::Day.document());
    for tokens in [10, 5] {
        let day = Day {
            date: TODAY.into(),
            tokens,
        };
        kind.create(&day, "").await.unwrap();
    }
    let (settings, scope) = (settings(20), scope());
    let chief = Chief {
        docs: &docs,
        sessions: &sessions,
        days: &docs,
        turns: &turns,
        clock: &Today,
        settings: &settings,
        scope: &scope,
    };
    // 15 of 20 used: the turn runs and crosses the budget.
    chief
        .ask(JULIUS, "Ship it", Conversation::New, None)
        .await
        .unwrap();
    assert_eq!(chief.used(TODAY).await.unwrap(), 33);
    let refused = chief
        .ask(JULIUS, "More", Conversation::New, None)
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::ResourceExhausted);
    assert_eq!(turns.asked.lock().len(), 1);
}

#[tokio::test]
async fn a_clients_session_is_its_conversation_and_its_events_are_watched() {
    let (docs, sessions, turns) = (
        InMemoryDocuments::new(Some("chief"), post_cortex_proto::CHIEF_DESCRIPTOR_SET).unwrap(),
        Made::default(),
        Scripted::default(),
    );
    let (settings, scope) = (settings(1000), scope());
    let chief = Chief {
        docs: &docs,
        sessions: &sessions,
        days: &docs,
        turns: &turns,
        clock: &Today,
        settings: &settings,
        scope: &scope,
    };
    let client = Asker {
        author: "julius",
        channel: "",
    };
    let (watcher, mut watched) = mpsc::channel(16);
    let first = chief
        .ask(
            client,
            "Hi",
            Conversation::Session("s-client"),
            Some(&watcher),
        )
        .await
        .unwrap();
    drop(watcher);
    let mut events = 0;
    while watched.recv().await.is_some() {
        events += 1;
    }
    assert_eq!(events, 4);
    assert_eq!(turns.asked.lock()[0].0.session, "s-client");
    assert_eq!(*sessions.count.lock(), 0, "the client's session is used");
    // Its next turn is the same conversation; no one else's.
    let next = chief
        .ask(client, "Again", Conversation::Session("s-client"), None)
        .await
        .unwrap();
    assert_eq!(next.conversation, first.conversation);
    let refused = chief
        .ask(
            Asker {
                author: "mallory",
                channel: "",
            },
            "Peek",
            Conversation::Session("s-client"),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);

    // What the view reads.
    let talks = chief.talks(0).await.unwrap();
    assert_eq!(talks.len(), 1);
    assert_eq!(talks[0].document.text, "Hi");
    let actions = chief.actions(Some(&first.conversation), 0).await.unwrap();
    assert_eq!(actions.len(), 4);
    assert!(chief.actions(Some("other"), 0).await.unwrap().is_empty());
    assert_eq!(
        chief.actions(None, 0).await.unwrap().len(),
        actions.len(),
        "every conversation's actions"
    );
    // A limit gives the newest.
    let newest = chief.actions(None, 2).await.unwrap();
    assert_eq!(newest, actions[..2]);
    let rights = chief.rights().await.unwrap();
    assert_eq!((rights.budget_tokens, rights.used_today), (1000, 36));
    assert_eq!(rights.person_tools, ["team_create"]);
}
