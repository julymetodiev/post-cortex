// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_proto::discord::ForumTag;

use super::*;
use crate::domain::tests::config;

const ME: &str = "9";
const GUILD: &str = "100";

fn role(id: &str, name: &str, permissions: u64, position: i32) -> Role {
    Role {
        id: id.to_string(),
        name: name.to_string(),
        permissions: permissions.to_string(),
        position,
    }
}

fn overwrite(id: &str, kind: i32, allow: u64, deny: u64) -> Overwrite {
    Overwrite {
        id: id.to_string(),
        r#type: kind,
        allow: allow.to_string(),
        deny: deny.to_string(),
    }
}

fn channel(id: &str, kind: i32, overwrites: Vec<Overwrite>) -> GuildChannel {
    GuildChannel {
        id: id.to_string(),
        r#type: kind,
        guild_id: GUILD.to_string(),
        permission_overwrites: overwrites,
        available_tags: if kind == FORUM {
            vec![
                ForumTag {
                    id: "71".to_string(),
                    name: "answered".to_string(),
                },
                ForumTag {
                    id: "72".to_string(),
                    name: "needs-team".to_string(),
                },
                ForumTag {
                    id: "73".to_string(),
                    name: "bug".to_string(),
                },
            ]
        } else {
            Vec::new()
        },
        ..GuildChannel::default()
    }
}

/// What the settings of the tests need of a bot.
const NEEDED: u64 = VIEW_CHANNEL
    | SEND_MESSAGES
    | SEND_MESSAGES_IN_THREADS
    | EMBED_LINKS
    | READ_MESSAGE_HISTORY
    | MANAGE_THREADS
    | MANAGE_ROLES;

/// The server: @everyone sees and writes, the bot's role `bot` rights at
/// place 5, `Member` at place 1.
fn roles(bot: u64) -> Vec<Role> {
    vec![
        role(GUILD, "@everyone", VIEW_CHANNEL | SEND_MESSAGES, 0),
        role("50", "pcx", bot, 5),
        role("51", "Member", 0, 1),
    ]
}

fn bot() -> Member {
    Member {
        roles: vec!["50".to_string()],
        ..Member::default()
    }
}

fn channels() -> Vec<GuildChannel> {
    vec![
        channel("200", FORUM, Vec::new()),
        channel("201", 0, Vec::new()),
        channel("202", 0, Vec::new()),
    ]
}

#[test]
fn a_bot_with_the_rights_its_settings_need_is_read_with_its_roles_and_tags() {
    let server = server(&config(), ME, &bot(), &roles(NEEDED), &channels()).unwrap();
    assert_eq!(server.me, ME);
    assert_eq!(server.roles, [("Member".to_string(), "51".to_string())]);
    let (forum, tags) = &server.tags[0];
    assert_eq!(forum, "200");
    assert_eq!(tags.len(), 3, "every tag of the forum, to name a post's");

    // An administrator may do everything, whatever a channel denies.
    let mut denied = channels();
    denied[0].permission_overwrites = vec![overwrite(GUILD, OF_ROLE, 0, VIEW_CHANNEL)];
    assert!(server_of(&roles(ADMINISTRATOR), &denied).is_ok());
}

fn server_of(roles: &[Role], channels: &[GuildChannel]) -> Result<Server, Vec<String>> {
    server(&config(), ME, &bot(), roles, channels)
}

#[test]
fn everything_the_bot_lacks_is_said_at_once() {
    // No rights of its own: @everyone's only.
    let lacks = server_of(&roles(0), &channels()).unwrap_err();
    assert_eq!(
        lacks,
        [
            "in channel help the bot lacks: Send Messages in Threads, Embed Links, Read Message History, Manage Threads",
            "in channel general the bot lacks: Read Message History",
            "in channel changelog the bot lacks: Send Messages in Threads, Embed Links",
            "the bot lacks: Manage Roles",
        ]
    );
}

#[test]
fn a_channels_overwrites_take_and_give_rights() {
    // #changelog: nobody writes, the bot's role does; then the bot itself
    // is denied the view.
    let mut changed = channels();
    changed[2].permission_overwrites = vec![
        overwrite(GUILD, OF_ROLE, 0, SEND_MESSAGES),
        overwrite("50", OF_ROLE, SEND_MESSAGES, 0),
    ];
    assert!(server_of(&roles(NEEDED & !SEND_MESSAGES), &changed).is_ok());
    changed[2]
        .permission_overwrites
        .push(overwrite(ME, 1, 0, VIEW_CHANNEL));
    assert_eq!(
        server_of(&roles(NEEDED), &changed).unwrap_err(),
        ["in channel changelog the bot lacks: View Channel"]
    );
    // An overwrite of a role the bot does not have is not its.
    let mut other = channels();
    other[1].permission_overwrites = vec![overwrite("51", OF_ROLE, 0, VIEW_CHANNEL)];
    assert!(server_of(&roles(NEEDED), &other).is_ok());
}

#[test]
fn a_channel_a_role_or_a_tag_the_server_lacks_is_said() {
    let mut fewer = channels();
    fewer.remove(1);
    assert_eq!(
        server_of(&roles(NEEDED), &fewer).unwrap_err(),
        ["channel general (201) is not in the server"]
    );

    // A role above the bot's own, a role that is not there, one named twice.
    let mut above = roles(NEEDED);
    above[2].position = 6;
    assert!(server_of(&above, &channels()).unwrap_err()[0].contains("not below the bot's own"));
    let mut none = roles(NEEDED);
    none.remove(2);
    assert_eq!(
        server_of(&none, &channels()).unwrap_err(),
        ["the server has no role Member"]
    );
    let mut twice = roles(NEEDED);
    twice.push(role("52", "Member", 0, 2));
    assert_eq!(
        server_of(&twice, &channels()).unwrap_err(),
        ["the server has several roles named Member"]
    );

    // A tag no forum the bot posts to has.
    let mut untagged = channels();
    untagged[0].available_tags.remove(0);
    assert_eq!(
        server_of(&roles(NEEDED), &untagged).unwrap_err(),
        ["no forum the bot posts to has a tag answered"]
    );
}
