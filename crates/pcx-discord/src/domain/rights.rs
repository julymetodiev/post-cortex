// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the bot may do in its server (docs/modules/discord.md, connecting a
//! server): its rights computed as Discord computes them (the roles'
//! permissions, then each channel's overwrites), against what its settings
//! need, so a wrong invite shows when the module starts. Pure.

use post_cortex_proto::discord::{DiscordConfig, GuildChannel, Member, Overwrite, Role};

/// Ban a member.
const BAN_MEMBERS: u64 = 1 << 2;
/// Does everything.
const ADMINISTRATOR: u64 = 1 << 3;
/// See a channel.
const VIEW_CHANNEL: u64 = 1 << 10;
/// Write in a channel.
const SEND_MESSAGES: u64 = 1 << 11;
/// Send an embed.
const EMBED_LINKS: u64 = 1 << 14;
/// Read what was written before.
const READ_MESSAGE_HISTORY: u64 = 1 << 16;
/// Give a role below its own.
const MANAGE_ROLES: u64 = 1 << 28;
/// Change a thread: a forum post's tags.
const MANAGE_THREADS: u64 = 1 << 34;
/// Write in a thread: a forum post.
const SEND_MESSAGES_IN_THREADS: u64 = 1 << 38;
/// Give a member a timeout.
const MODERATE_MEMBERS: u64 = 1 << 40;

/// A forum channel.
pub const FORUM: i32 = 15;

/// An overwrite of a role.
const OF_ROLE: i32 = 0;

/// The rights by their names in Discord's settings.
const NAMES: [(u64, &str); 9] = [
    (VIEW_CHANNEL, "View Channel"),
    (SEND_MESSAGES, "Send Messages"),
    (SEND_MESSAGES_IN_THREADS, "Send Messages in Threads"),
    (EMBED_LINKS, "Embed Links"),
    (READ_MESSAGE_HISTORY, "Read Message History"),
    (MANAGE_THREADS, "Manage Threads"),
    (MANAGE_ROLES, "Manage Roles"),
    (MODERATE_MEMBERS, "Timeout Members"),
    (BAN_MEMBERS, "Ban Members"),
];

/// The link that adds bot `me` (its user id, its application's too) to a
/// server with the rights the module may need and no other.
#[must_use]
pub fn invite(me: &str) -> String {
    let rights = NAMES.iter().fold(0, |all, (bit, _)| all | bit);
    format!("https://discord.com/oauth2/authorize?client_id={me}&scope=bot&permissions={rights}")
}

/// A decimal bit set; one that is none grants nothing.
fn bits(set: &str) -> u64 {
    set.parse().unwrap_or_default()
}

/// The names of `rights`.
fn named(rights: u64) -> String {
    NAMES
        .iter()
        .filter(|(bit, _)| rights & bit != 0)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What `member` may do in server `guild` by its roles: @everyone's (the
/// role with the server's id) and its own.
fn in_server(guild: &str, roles: &[Role], member: &Member) -> u64 {
    roles
        .iter()
        .filter(|role| role.id == guild || member.roles.contains(&role.id))
        .fold(0, |all, role| all | bits(&role.permissions))
}

/// What a member of `base` rights (user `me`, of `mine` roles) may do in a
/// channel of `overwrites`: @everyone's overwrite, then its roles' (what
/// they deny, then what they allow), then its own.
fn in_channel(base: u64, guild: &str, me: &str, mine: &[String], overwrites: &[Overwrite]) -> u64 {
    if base & ADMINISTRATOR != 0 {
        return u64::MAX;
    }
    let apply = |rights: u64, o: &Overwrite| rights & !bits(&o.deny) | bits(&o.allow);
    let mut rights = base;
    if let Some(everyone) = overwrites
        .iter()
        .find(|o| o.r#type == OF_ROLE && o.id == guild)
    {
        rights = apply(rights, everyone);
    }
    let (deny, allow) = overwrites
        .iter()
        .filter(|o| o.r#type == OF_ROLE && o.id != guild && mine.contains(&o.id))
        .fold((0, 0), |(deny, allow), o| {
            (deny | bits(&o.deny), allow | bits(&o.allow))
        });
    rights = rights & !deny | allow;
    if let Some(own) = overwrites
        .iter()
        .find(|o| o.r#type != OF_ROLE && o.id == me)
    {
        rights = apply(rights, own);
    }
    rights
}

/// What the server holds that the settings name by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Server {
    /// The bot's user id.
    pub me: String,
    /// The roles it may give: each name of the settings with its id.
    pub roles: Vec<(String, String)>,
    /// The tags of each forum it posts to: the forum's id, then each tag's
    /// id and name.
    pub tags: Vec<(String, Vec<(String, String)>)>,
}

/// The server as `config` needs it, read from the bot `member` (user
/// `me`), the server's `roles` and its `channels`.
///
/// # Errors
/// Everything the settings need that the server lacks: a channel that is
/// not there, a right the bot does not have in one, a role that is not
/// there or not below the bot's own, a tag no forum has.
pub fn server(
    config: &DiscordConfig,
    me: &str,
    member: &Member,
    roles: &[Role],
    channels: &[GuildChannel],
) -> Result<Server, Vec<String>> {
    let guild = config.guild_id.as_str();
    let base = in_server(guild, roles, member);
    let admin = base & ADMINISTRATOR != 0;
    let mut lacks = Vec::new();
    let mut tags = Vec::new();
    for named_channel in &config.channels {
        let Some(found) = channels.iter().find(|c| c.id == named_channel.channel_id) else {
            lacks.push(format!(
                "channel {} ({}) is not in the server",
                named_channel.name, named_channel.channel_id
            ));
            continue;
        };
        let forum = found.r#type == FORUM;
        let mut needs = 0;
        if named_channel.hear {
            needs |= VIEW_CHANNEL | READ_MESSAGE_HISTORY;
        }
        if named_channel.post {
            needs |= VIEW_CHANNEL | SEND_MESSAGES | SEND_MESSAGES_IN_THREADS | EMBED_LINKS;
            // Tags, and the locks of a moderation.
            if (forum && !config.tags.is_empty()) || config.moderation.is_some() {
                needs |= MANAGE_THREADS;
            }
        }
        let has = in_channel(base, guild, me, &member.roles, &found.permission_overwrites);
        if needs & !has != 0 {
            lacks.push(format!(
                "in channel {} the bot lacks: {}",
                named_channel.name,
                named(needs & !has)
            ));
        }
        if forum && named_channel.post {
            tags.push((
                found.id.clone(),
                found
                    .available_tags
                    .iter()
                    .map(|tag| (tag.id.clone(), tag.name.clone()))
                    .collect::<Vec<_>>(),
            ));
        }
    }
    for tag in &config.tags {
        if !tags
            .iter()
            .any(|(_, of)| of.iter().any(|(_, name)| name == tag))
        {
            lacks.push(format!("no forum the bot posts to has a tag {tag}"));
        }
    }
    // A role is given by one above it: the bot's highest.
    let top = roles
        .iter()
        .filter(|role| member.roles.contains(&role.id))
        .map(|role| role.position)
        .max()
        .unwrap_or_default();
    let mut given = Vec::new();
    let mut needs = 0;
    if !config.roles.is_empty() {
        needs |= MANAGE_ROLES;
    }
    if let Some(moderation) = &config.moderation {
        needs |= MODERATE_MEMBERS;
        if moderation.ban {
            needs |= BAN_MEMBERS;
        }
    }
    if !admin && needs & !base != 0 {
        lacks.push(format!("the bot lacks: {}", named(needs & !base)));
    }
    for name in &config.roles {
        let mut of_name = roles.iter().filter(|role| &role.name == name);
        match (of_name.next(), of_name.next()) {
            (None, _) => lacks.push(format!("the server has no role {name}")),
            (Some(_), Some(_)) => lacks.push(format!("the server has several roles named {name}")),
            (Some(role), None) if role.position >= top => lacks.push(format!(
                "role {name} is not below the bot's own role: move the bot's role above it"
            )),
            (Some(role), None) => given.push((name.clone(), role.id.clone())),
        }
    }
    if lacks.is_empty() {
        Ok(Server {
            me: me.to_string(),
            roles: given,
            tags,
        })
    } else {
        Err(lacks)
    }
}

#[cfg(test)]
mod tests;
