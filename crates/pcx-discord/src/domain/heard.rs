// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the module hears (docs/modules/discord.md, what it hears): which of
//! the Gateway's events of its server become events on the bus, and what
//! each says. A person's, of a channel the settings hear; never a bot's.
//! Pure.

use chrono::{DateTime, SecondsFormat};
use post_cortex_proto::discord::{
    AutomodActed, AutomodExecution, DiscordConfig, DiscordMessage, GuildChannel, MemberAdd,
    MemberJoined, MessageCreated, PostCreated, User,
};

use crate::domain::channel;
use crate::domain::rights::Server;

/// `discord.post_created`.
pub const POST_CREATED: &str = "discord.post_created";
/// `discord.message_created`.
pub const MESSAGE_CREATED: &str = "discord.message_created";
/// `discord.member_joined`.
pub const MEMBER_JOINED: &str = "discord.member_joined";
/// `discord.automod_acted`.
pub const AUTOMOD_ACTED: &str = "discord.automod_acted";

/// Milliseconds from 1970 to Discord's epoch (2015), which an id counts
/// from in its bits above the 22nd.
const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

/// An event to publish.
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    /// A new post of a forum.
    Post(PostCreated),
    /// A message.
    Message(MessageCreated),
    /// A person joined.
    Joined(MemberJoined),
    /// AutoMod acted.
    Automod(AutomodActed),
}

/// When the account of Discord id `id` was made (RFC 3339); none for what
/// is no id.
fn made(id: &str) -> Option<String> {
    let id: u64 = id.parse().ok()?;
    let ms = i64::try_from(id >> 22).ok()? + DISCORD_EPOCH_MS;
    Some(DateTime::from_timestamp_millis(ms)?.to_rfc3339_opts(SecondsFormat::Secs, true))
}

/// The name the server shows for `user`: the nickname, else the display
/// name, else the username.
fn shown(user: &User, nick: &str) -> String {
    [nick, user.global_name.as_str(), user.username.as_str()]
        .into_iter()
        .find(|name| !name.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// Whether `message` is one the module may hear: of its server, written by
/// a person (no bot, no webhook, not the bot itself).
#[must_use]
pub fn of_a_person(config: &DiscordConfig, server: &Server, message: &DiscordMessage) -> bool {
    message.guild_id == config.guild_id
        && message.webhook_id.is_empty()
        && message
            .author
            .as_ref()
            .is_some_and(|author| !author.bot && author.id != server.me)
}

/// The event of `message`, which [`of_a_person`] admitted, written in a
/// channel the settings hear or in `thread` (the thread it is in, when its
/// channel is none of the settings'); none when neither is heard.
#[must_use]
pub fn message(
    config: &DiscordConfig,
    server: &Server,
    message: &DiscordMessage,
    thread: Option<&GuildChannel>,
) -> Option<Heard> {
    let author = message.author.as_ref()?;
    let nick = message.member.as_ref().map_or("", |m| m.nick.as_str());
    let (author_id, author_name) = (author.id.clone(), shown(author, nick));
    let hears = |id: &str| channel(config, id).filter(|c| c.hear);
    if let Some(heard) = hears(&message.channel_id) {
        return Some(Heard::Message(MessageCreated {
            channel: heard.name.clone(),
            thread_id: String::new(),
            message_id: message.id.clone(),
            text: message.content.clone(),
            author_id,
            author_name,
            reply_to: replied(message),
        }));
    }
    let thread = thread.filter(|t| t.id == message.channel_id)?;
    let forum = hears(&thread.parent_id)?;
    // A forum post's first message has the post's id.
    if message.id == thread.id {
        let names = server
            .tags
            .iter()
            .find(|(id, _)| *id == thread.parent_id)
            .map(|(_, tags)| tags.as_slice())
            .unwrap_or_default();
        return Some(Heard::Post(PostCreated {
            channel: forum.name.clone(),
            thread_id: thread.id.clone(),
            title: thread.name.clone(),
            text: message.content.clone(),
            author_id,
            author_name,
            tags: thread
                .applied_tags
                .iter()
                .filter_map(|id| names.iter().find(|(tag, _)| tag == id))
                .map(|(_, name)| name.clone())
                .collect(),
        }));
    }
    Some(Heard::Message(MessageCreated {
        channel: forum.name.clone(),
        thread_id: thread.id.clone(),
        message_id: message.id.clone(),
        text: message.content.clone(),
        author_id,
        author_name,
        reply_to: replied(message),
    }))
}

fn replied(message: &DiscordMessage) -> String {
    message
        .message_reference
        .as_ref()
        .map(|r| r.message_id.clone())
        .unwrap_or_default()
}

/// The event of a person who joined the module's server; none for another
/// server's, or a bot.
#[must_use]
pub fn joined(config: &DiscordConfig, added: &MemberAdd) -> Option<Heard> {
    let user = added.user.as_ref()?;
    if added.guild_id != config.guild_id || user.bot {
        return None;
    }
    Some(Heard::Joined(MemberJoined {
        member_id: user.id.clone(),
        name: shown(user, ""),
        account_created: made(&user.id).unwrap_or_default(),
    }))
}

/// The event of an action of the server's AutoMod; none for another
/// server's.
#[must_use]
pub fn automod(config: &DiscordConfig, acted: &AutomodExecution) -> Option<Heard> {
    if acted.guild_id != config.guild_id {
        return None;
    }
    let action = match acted.action.as_ref().map(|a| a.r#type) {
        Some(1) => "block".to_string(),
        Some(2) => "alert".to_string(),
        Some(3) => "timeout".to_string(),
        // One this version does not know, as Discord numbers it.
        Some(other) => other.to_string(),
        None => String::new(),
    };
    Some(Heard::Automod(AutomodActed {
        rule_id: acted.rule_id.clone(),
        action,
        member_id: acted.user_id.clone(),
        channel: channel(config, &acted.channel_id)
            .map(|c| c.name.clone())
            .unwrap_or_default(),
        content: acted.content.clone(),
        matched: acted.matched_content.clone(),
    }))
}

/// Whether `channel` is a thread: a forum's post, or a thread of a text or
/// announcement channel.
#[must_use]
pub fn is_thread(channel: &GuildChannel) -> bool {
    (10..=12).contains(&channel.r#type)
}

#[cfg(test)]
mod tests;
