// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use post_cortex_proto::board::Decision;
use post_cortex_proto::module::PersonGrant;
use post_cortex_proto::telegram::{Chat, Person};

pub fn config() -> TelegramConfig {
    TelegramConfig {
        bot_token: "T".into(),
        chats: vec![Chat {
            name: "ops".into(),
            chat_id: -100,
        }],
        people: vec![Person {
            telegram_id: 42,
            name: "julius".into(),
        }],
        persons: Some(PersonGrant {
            persons: vec!["julius".into()],
        }),
        approvals: "ops".into(),
        api_url: String::new(),
    }
}

#[test]
fn settings_name_chats_once_and_people_it_acts_for() {
    assert_eq!(check(&config()), Ok(()));
    let field = |change: fn(&mut TelegramConfig)| {
        let mut config = config();
        change(&mut config);
        match check(&config) {
            Err(Refused::Setting { field, .. }) => field,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(field(|c| c.bot_token.clear()), "bot_token");
    assert_eq!(
        field(|c| c.chats[0].name = "Ops Room".into()),
        "chats.0.name"
    );
    assert_eq!(field(|c| c.chats.push(c.chats[0].clone())), "chats.1.name");
    assert_eq!(field(|c| c.people[0].name = "eve".into()), "people.0.name");
    assert_eq!(
        field(|c| c.people.push(c.people[0].clone())),
        "people.1.telegram_id"
    );
    assert_eq!(field(|c| c.approvals = "elsewhere".into()), "approvals");
}

#[test]
fn chats_and_people_are_found_by_their_telegram_ids() {
    let config = config();
    assert_eq!(chat(&config, "ops"), Ok(-100));
    assert_eq!(
        chat(&config, "dev"),
        Err(Refused::UnknownChat("dev".into()))
    );
    assert!(serves(&config, -100) && !serves(&config, -5));
    assert_eq!(person(&config, 42), Some("julius"));
    assert_eq!(person(&config, 7), None);
    assert_eq!(said("  "), Err(Refused::Empty));
}

#[test]
fn a_buttons_data_names_its_choice_and_prompt() {
    assert_eq!(callback(Choice::Approve, 12), "a:12");
    assert_eq!(pressed("a:12"), Some((Choice::Approve, 12)));
    assert_eq!(pressed("d:3"), Some((Choice::Decline, 3)));
    for foreign in ["x:1", "a:", "a:one", "nothing"] {
        assert_eq!(pressed(foreign), None, "{foreign}");
    }
}

#[test]
fn a_waiting_decision_reads_with_what_why_and_its_buttons() {
    let decision = DecisionRecord {
        id: "D-1".into(),
        version: 1,
        pending: true,
        decision: Some(Decision {
            task: "t".into(),
            what: "Restart svc-music".into(),
            why: "it crash-loops".into(),
            decided_by: "eq-medic".into(),
            risk: Risk::Code.into(),
            held_because: Some("the team is supervised".into()),
            ..Decision::default()
        }),
        ..DecisionRecord::default()
    };
    assert!(waits(&decision));
    let (text, buttons) = prompt(&decision, "PodCrashLooping · svc-music", 5);
    assert!(text.starts_with("Waits for you: Restart svc-music"));
    assert!(text.contains("By eq-medic, risking code"));
    assert!(text.contains("Not approved by the policy: the team is supervised"));
    let data: Vec<&str> = buttons.iter().map(|b| b.callback_data.as_str()).collect();
    assert_eq!(data, ["a:5", "d:5"]);
    assert_eq!(
        settled("x", Choice::Decline, "julius"),
        "Declined by julius: x"
    );
}
