// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The channel's service over a bot, a board and a chief held in memory.

use async_trait::async_trait;
use parking_lot::Mutex;
use post_cortex_proto::board::{Decision, DecisionRecord, Risk};
use post_cortex_proto::telegram::{Button, CallbackQuery, Message, TelegramChat, User};

use super::*;

use crate::domain::tests::config;

/// What the fakes were asked.
#[derive(Default)]
struct World {
    sent: Mutex<Vec<(i64, String, Vec<Button>)>>,
    edited: Mutex<Vec<(i64, i64, String, usize)>>,
    answered: Mutex<Vec<String>>,
    updates: Mutex<Vec<Update>>,
    decision: Mutex<Option<Waiting>>,
    approved: Mutex<Vec<(String, String, u64)>>,
    declined: Mutex<Vec<(String, String, u64)>>,
    asked: Mutex<Vec<(String, String, String)>>,
}

#[async_trait]
impl Bot for World {
    async fn updates(&self, _offset: i64, _timeout: u32) -> Result<Vec<Update>, ModuleError> {
        Ok(std::mem::take(&mut *self.updates.lock()))
    }
    async fn send(&self, chat: i64, text: &str, buttons: &[Button]) -> Result<i64, ModuleError> {
        let mut sent = self.sent.lock();
        sent.push((chat, text.to_string(), buttons.to_vec()));
        Ok(i64::try_from(sent.len()).unwrap() + 100)
    }
    async fn edit(
        &self,
        chat: i64,
        message: i64,
        text: &str,
        buttons: &[Button],
    ) -> Result<(), ModuleError> {
        self.edited
            .lock()
            .push((chat, message, text.to_string(), buttons.len()));
        Ok(())
    }
    async fn answer(&self, _query: &str, text: &str) -> Result<(), ModuleError> {
        self.answered.lock().push(text.to_string());
        Ok(())
    }
}

#[async_trait]
impl Board for World {
    async fn decision(&self, _workspace: &str, _id: &str) -> Result<Option<Waiting>, ModuleError> {
        Ok(self.decision.lock().clone())
    }
    async fn approve(
        &self,
        _ws: &str,
        person: &str,
        id: &str,
        version: u64,
    ) -> Result<(), ModuleError> {
        self.approved
            .lock()
            .push((person.to_string(), id.to_string(), version));
        Ok(())
    }
    async fn decline(
        &self,
        _ws: &str,
        person: &str,
        id: &str,
        version: u64,
    ) -> Result<(), ModuleError> {
        self.declined
            .lock()
            .push((person.to_string(), id.to_string(), version));
        Ok(())
    }
}

#[async_trait]
impl Chief for World {
    async fn ask(
        &self,
        person: &str,
        question: &str,
        conversation: &str,
    ) -> Result<(String, String), ModuleError> {
        self.asked.lock().push((
            person.to_string(),
            question.to_string(),
            conversation.to_string(),
        ));
        Ok(("All green.".to_string(), "c-1".to_string()))
    }
}

fn waiting(pending: bool, version: u64) -> Waiting {
    Waiting {
        decision: DecisionRecord {
            id: "D-1".into(),
            version,
            pending,
            decision: Some(Decision {
                task: "t-1".into(),
                what: "Restart svc-music".into(),
                why: "it crash-loops".into(),
                decided_by: "eq-medic".into(),
                risk: Risk::Code.into(),
                approved_by: (!pending).then(|| "julius".to_string()),
                ..Decision::default()
            }),
            ..DecisionRecord::default()
        },
        task: "PodCrashLooping".into(),
    }
}

fn press(id: i64, data: &str, update: i64) -> Update {
    Update {
        update_id: update,
        callback_query: Some(CallbackQuery {
            id: "q".into(),
            from: Some(User {
                id,
                ..User::default()
            }),
            data: data.into(),
            ..CallbackQuery::default()
        }),
        ..Update::default()
    }
}

fn message(from: i64, chat: i64, text: &str, update: i64) -> Update {
    Update {
        update_id: update,
        message: Some(Message {
            from: Some(User {
                id: from,
                ..User::default()
            }),
            chat: Some(TelegramChat { id: chat }),
            text: text.into(),
            ..Message::default()
        }),
        ..Update::default()
    }
}

#[tokio::test]
async fn a_waiting_decision_is_sent_once_and_approved_by_a_person_it_acts_for() {
    let (config, kept, world) = (config(), Kept::default(), World::default());
    let service = Service::new(&config, &kept, &world, &world, &world);
    *world.decision.lock() = Some(waiting(true, 1));
    service.decision_written("ws", "D-1").await.unwrap();
    service.decision_written("ws", "D-1").await.unwrap();
    let sent = world.sent.lock().clone();
    assert_eq!(sent.len(), 1, "a version is sent once");
    assert_eq!(sent[0].0, -100, "to the approvals chat");
    let approve = sent[0].2[0].callback_data.clone();

    *world.updates.lock() = vec![press(7, &approve, 1), press(42, &approve, 2)];
    service.poll(0).await.unwrap();
    assert_eq!(
        world.approved.lock().clone(),
        [("julius".to_string(), "D-1".to_string(), 1)]
    );
    let answered = world.answered.lock().clone();
    assert!(
        answered[0].starts_with("You do not decide here"),
        "a stranger decides nothing"
    );
    assert_eq!(answered[1], "Approved by julius: Restart svc-music");
    assert_eq!(
        world.edited.lock()[0],
        (
            -100,
            101,
            "Approved by julius: Restart svc-music".to_string(),
            0
        )
    );
    assert_eq!(kept.offset(), 3, "the next read is after the last update");

    // The approval's own write settles nothing more.
    *world.decision.lock() = Some(waiting(false, 2));
    service.decision_written("ws", "D-1").await.unwrap();
    assert_eq!(world.edited.lock().len(), 1);
}

#[tokio::test]
async fn a_decline_is_the_persons_refusal_and_an_end_elsewhere_settles_the_message() {
    let (config, kept, world) = (config(), Kept::default(), World::default());
    let service = Service::new(&config, &kept, &world, &world, &world);
    *world.decision.lock() = Some(waiting(true, 1));
    service.decision_written("ws", "D-1").await.unwrap();
    let decline = world.sent.lock()[0].2[1].callback_data.clone();
    *world.updates.lock() = vec![press(42, &decline, 1)];
    service.poll(0).await.unwrap();
    assert!(world.approved.lock().is_empty());
    assert_eq!(
        world.declined.lock().clone(),
        [("julius".to_string(), "D-1".to_string(), 1)]
    );
    assert_eq!(
        world.edited.lock()[0],
        (
            -100,
            101,
            "Declined by julius: Restart svc-music".to_string(),
            0
        )
    );
    *world.updates.lock() = vec![press(42, &decline, 2)];
    service.poll(0).await.unwrap();
    assert!(world.answered.lock()[1].starts_with("This prompt is gone"));

    // Waiting again (revoked), then approved in the app.
    *world.decision.lock() = Some(waiting(true, 3));
    service.decision_written("ws", "D-1").await.unwrap();
    *world.decision.lock() = Some(waiting(false, 4));
    service.decision_written("ws", "D-1").await.unwrap();
    let edited = world.edited.lock().clone();
    assert_eq!(
        edited.last().unwrap().2,
        "Approved by julius: Restart svc-music"
    );

    // Asked again, then withdrawn by who asked it: its buttons go too.
    *world.decision.lock() = Some(waiting(true, 5));
    service.decision_written("ws", "D-1").await.unwrap();
    let mut withdrawn = waiting(true, 6);
    withdrawn.decision.pending = false;
    withdrawn.decision.decision.as_mut().unwrap().withdrawn = Some("its run ended".into());
    *world.decision.lock() = Some(withdrawn);
    service.decision_written("ws", "D-1").await.unwrap();
    assert_eq!(
        world.edited.lock().last().unwrap().clone(),
        (
            -100,
            103,
            "Withdrawn by eq-medic: Restart svc-music (its run ended)".to_string(),
            0
        )
    );
}

#[tokio::test]
async fn a_persons_message_in_its_chat_is_a_question_to_the_chief() {
    let (config, kept, world) = (config(), Kept::default(), World::default());
    let service = Service::new(&config, &kept, &world, &world, &world);
    *world.updates.lock() = vec![
        message(42, -100, "status?", 1),
        message(7, -100, "who are you", 2),
        message(42, -5, "elsewhere", 3),
    ];
    service.poll(0).await.unwrap();
    *world.updates.lock() = vec![message(42, -100, "and now?", 4)];
    service.poll(0).await.unwrap();
    let asked = world.asked.lock().clone();
    assert_eq!(asked.len(), 2, "only its people, only its chats");
    assert_eq!(
        asked[0],
        ("julius".to_string(), "status?".to_string(), String::new())
    );
    assert_eq!(asked[1].2, "c-1", "the chat goes on with its conversation");
    let replies: Vec<String> = world.sent.lock().iter().map(|s| s.1.clone()).collect();
    assert_eq!(replies, ["All green.", "All green."]);
}

#[tokio::test]
async fn a_message_goes_to_a_destination_by_name() {
    let (config, kept, world) = (config(), Kept::default(), World::default());
    let service = Service::new(&config, &kept, &world, &world, &world);
    assert_eq!(service.send("ops", "Disk 95% full").await.unwrap(), "101");
    assert_eq!(
        service.send("dev", "x").await.unwrap_err().kind,
        ModuleErrorKind::NotFound
    );
    assert_eq!(
        service.send("ops", " ").await.unwrap_err().kind,
        ModuleErrorKind::InvalidArgument
    );
}

/// Ops daemon 2026-11-11: the board wrote a workflow's decision twice as it
/// asked (versions 1 and 2), each was sent, and after a press on one the
/// other still showed its buttons and answered "This prompt is gone".
#[tokio::test]
async fn a_decision_written_again_while_it_waits_keeps_its_one_message() {
    let (config, kept, world) = (config(), Kept::default(), World::default());
    let service = Service::new(&config, &kept, &world, &world, &world);
    // Both writes heard together: the second finds the first's prompt.
    *world.decision.lock() = Some(waiting(true, 1));
    let (first, second) = tokio::join!(
        service.decision_written("ws", "D-1"),
        service.decision_written("ws", "D-1")
    );
    first.unwrap();
    second.unwrap();
    assert_eq!(world.sent.lock().len(), 1);

    // Its next version says the same: nothing is sent or edited, and the
    // one message approves that version.
    *world.decision.lock() = Some(waiting(true, 2));
    service.decision_written("ws", "D-1").await.unwrap();
    assert_eq!(world.sent.lock().len(), 1);
    assert!(world.edited.lock().is_empty());

    // A version that says more: the message is edited, its buttons kept.
    let mut held = waiting(true, 3);
    held.decision.decision.as_mut().unwrap().held_because = Some("no team approves it".into());
    *world.decision.lock() = Some(held);
    service.decision_written("ws", "D-1").await.unwrap();
    assert_eq!(world.sent.lock().len(), 1);
    let edited = world.edited.lock().clone();
    assert_eq!(edited.len(), 1);
    assert_eq!(
        (edited[0].1, edited[0].3),
        (101, 2),
        "the same message, both buttons"
    );
    assert!(
        edited[0].2.contains("no team approves it"),
        "{}",
        edited[0].2
    );

    let approve = world.sent.lock()[0].2[0].callback_data.clone();
    *world.updates.lock() = vec![press(42, &approve, 1)];
    service.poll(0).await.unwrap();
    assert_eq!(
        world.approved.lock().clone(),
        [("julius".to_string(), "D-1".to_string(), 3)]
    );
    assert_eq!(world.edited.lock().len(), 2, "settled once");

    // The approval's own write, heard after: nothing more to settle.
    *world.decision.lock() = Some(waiting(false, 4));
    service.decision_written("ws", "D-1").await.unwrap();
    assert_eq!(world.edited.lock().len(), 2);
}
