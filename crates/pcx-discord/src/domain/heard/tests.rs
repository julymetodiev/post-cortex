// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_proto::discord::{AutomodAction, Member, MessageReference};

use super::*;
use crate::domain::tests::config;

fn server() -> Server {
    Server {
        me: "9".to_string(),
        roles: Vec::new(),
        tags: vec![(
            "200".to_string(),
            vec![
                ("71".to_string(), "answered".to_string()),
                ("73".to_string(), "bug".to_string()),
            ],
        )],
    }
}

fn user(id: &str, name: &str) -> User {
    User {
        id: id.to_string(),
        username: name.to_string(),
        ..User::default()
    }
}

fn written(id: &str, channel: &str, text: &str) -> DiscordMessage {
    DiscordMessage {
        id: id.to_string(),
        channel_id: channel.to_string(),
        guild_id: "100".to_string(),
        author: Some(user("7", "ana")),
        content: text.to_string(),
        ..DiscordMessage::default()
    }
}

/// A post of the forum `help` (channel 200).
fn post(id: &str) -> GuildChannel {
    GuildChannel {
        id: id.to_string(),
        r#type: 11,
        guild_id: "100".to_string(),
        name: "How do I pair a runner?".to_string(),
        parent_id: "200".to_string(),
        applied_tags: vec!["73".to_string(), "99".to_string()],
        ..GuildChannel::default()
    }
}

#[test]
fn only_a_person_of_the_server_is_heard() {
    let (config, server) = (config(), server());
    let heard = |change: fn(&mut DiscordMessage)| {
        let mut message = written("5", "201", "hi");
        change(&mut message);
        of_a_person(&config, &server, &message)
    };
    assert!(heard(|_| {}));
    assert!(!heard(|m| m.guild_id = "101".into()));
    assert!(!heard(|m| m.guild_id.clear()), "a direct message");
    assert!(!heard(|m| m.author.as_mut().unwrap().bot = true));
    assert!(!heard(|m| m.author.as_mut().unwrap().id = "9".into()));
    assert!(!heard(|m| m.webhook_id = "33".into()));
    assert!(!heard(|m| m.author = None));
}

#[test]
fn a_message_of_a_channel_it_hears_is_an_event_named_by_the_settings() {
    let (config, server) = (config(), server());
    let mut said = written("5", "201", "is there a FreeBSD build?");
    said.member = Some(Member {
        nick: "Ana K".to_string(),
        ..Member::default()
    });
    said.message_reference = Some(MessageReference {
        message_id: "4".to_string(),
    });
    assert_eq!(
        message(&config, &server, &said, None),
        Some(Heard::Message(MessageCreated {
            channel: "general".to_string(),
            thread_id: String::new(),
            message_id: "5".to_string(),
            text: "is there a FreeBSD build?".to_string(),
            author_id: "7".to_string(),
            author_name: "Ana K".to_string(),
            reply_to: "4".to_string(),
        }))
    );
    // `changelog` is posted to and not heard; 999 is not named.
    assert_eq!(
        message(&config, &server, &written("5", "202", "x"), None),
        None
    );
    assert_eq!(
        message(&config, &server, &written("5", "999", "x"), None),
        None
    );
}

#[test]
fn a_forum_posts_first_message_is_the_post_and_the_next_are_messages_of_it() {
    let (config, server) = (config(), server());
    let thread = post("300");
    let first = written("300", "300", "The docs say pcx connect, then what?");
    assert_eq!(
        message(&config, &server, &first, Some(&thread)),
        Some(Heard::Post(PostCreated {
            channel: "help".to_string(),
            thread_id: "300".to_string(),
            title: "How do I pair a runner?".to_string(),
            text: "The docs say pcx connect, then what?".to_string(),
            author_id: "7".to_string(),
            author_name: "ana".to_string(),
            // A tag the forum no longer has is left out.
            tags: vec!["bug".to_string()],
        }))
    );
    let next = written("301", "300", "anyone?");
    let Some(Heard::Message(heard)) = message(&config, &server, &next, Some(&thread)) else {
        panic!("a message of a post was not heard");
    };
    assert_eq!(
        (heard.channel.as_str(), heard.thread_id.as_str()),
        ("help", "300")
    );

    // A thread of a channel it does not hear, or another thread.
    let mut elsewhere = post("300");
    elsewhere.parent_id = "202".to_string();
    assert_eq!(message(&config, &server, &next, Some(&elsewhere)), None);
    assert_eq!(message(&config, &server, &next, Some(&post("302"))), None);
    assert_eq!(message(&config, &server, &next, None), None);
}

#[test]
fn a_person_who_joins_is_an_event_with_when_their_account_was_made() {
    let config = config();
    let added = |guild: &str, bot: bool| MemberAdd {
        guild_id: guild.to_string(),
        user: Some(User {
            // Discord's own example id: made 2016-04-30T11:18:25.796Z.
            id: "175928847299117063".to_string(),
            username: "ana".to_string(),
            global_name: "Ana".to_string(),
            bot,
        }),
    };
    assert_eq!(
        joined(&config, &added("100", false)),
        Some(Heard::Joined(MemberJoined {
            member_id: "175928847299117063".to_string(),
            name: "Ana".to_string(),
            account_created: "2016-04-30T11:18:25Z".to_string(),
        }))
    );
    assert_eq!(joined(&config, &added("101", false)), None);
    assert_eq!(joined(&config, &added("100", true)), None);
}

#[test]
fn an_action_of_automod_is_an_event_that_says_what_it_did() {
    let config = config();
    let acted = |kind: i32, channel: &str| AutomodExecution {
        guild_id: "100".to_string(),
        action: Some(AutomodAction { r#type: kind }),
        rule_id: "88".to_string(),
        user_id: "7".to_string(),
        channel_id: channel.to_string(),
        content: "join discord.gg/free-nitro".to_string(),
        matched_content: "discord.gg/free-nitro".to_string(),
    };
    let Some(Heard::Automod(blocked)) = automod(&config, &acted(1, "201")) else {
        panic!("an action was not heard");
    };
    assert_eq!(
        (
            blocked.action.as_str(),
            blocked.channel.as_str(),
            blocked.rule_id.as_str()
        ),
        ("block", "general", "88")
    );
    assert_eq!(blocked.matched, "discord.gg/free-nitro");
    let Some(Heard::Automod(timed_out)) = automod(&config, &acted(3, "999")) else {
        panic!("an action was not heard");
    };
    assert_eq!(
        (timed_out.action.as_str(), timed_out.channel.as_str()),
        ("timeout", "")
    );
    let mut other = acted(1, "201");
    other.guild_id = "101".to_string();
    assert_eq!(automod(&config, &other), None);
}
