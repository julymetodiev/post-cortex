// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the bot may do to members and posts (docs/modules/discord.md, what
//! it does): a timeout within the settings' longest, a lock, a ban only when
//! the settings let it. Pure.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use post_cortex_proto::discord::{DiscordConfig, Member, Moderation, Role};

use crate::domain::Refused;

/// Discord's longest timeout: 28 days.
pub const LONGEST_TIMEOUT_MINUTES: u32 = 28 * 24 * 60;

/// The most hours before a ban whose messages Discord deletes: 7 days.
pub const LONGEST_DELETE_HOURS: u32 = 7 * 24;

/// What the settings let the bot do to members and posts.
///
/// # Errors
/// `Refused::Moderation`: they let it moderate nothing.
pub fn allowed(config: &DiscordConfig) -> Result<&Moderation, Refused> {
    config
        .moderation
        .as_ref()
        .ok_or_else(|| Refused::Moderation("the settings let the bot moderate nothing".to_string()))
}

/// Whether `moderation` may run: a longest timeout of 1 to
/// [`LONGEST_TIMEOUT_MINUTES`].
///
/// # Errors
/// `Refused::Setting` naming it.
pub fn check(moderation: &Moderation) -> Result<(), Refused> {
    let longest = moderation.longest_timeout_minutes;
    if longest == 0 || longest > LONGEST_TIMEOUT_MINUTES {
        return Err(Refused::Setting {
            field: "moderation.longest_timeout_minutes".to_string(),
            reason: format!("is 1 to {LONGEST_TIMEOUT_MINUTES}"),
        });
    }
    Ok(())
}

/// When a timeout of `minutes` given at `now` ends, as Discord writes a
/// time; none: `minutes` is 0, the timeout is lifted.
///
/// # Errors
/// `Refused::Moderation` without moderation; `Refused::Request` for more
/// than the settings' longest.
pub fn until(
    config: &DiscordConfig,
    now: DateTime<Utc>,
    minutes: u32,
) -> Result<Option<String>, Refused> {
    let longest = allowed(config)?.longest_timeout_minutes;
    if minutes > longest {
        return Err(Refused::Request(format!(
            "a timeout is at most {longest} minutes"
        )));
    }
    Ok((minutes > 0).then(|| {
        (now + Duration::minutes(i64::from(minutes))).to_rfc3339_opts(SecondsFormat::Secs, true)
    }))
}

/// The seconds of messages a ban deletes, for `hours` before it.
///
/// # Errors
/// `Refused::Moderation` when the settings let the bot ban no one;
/// `Refused::Request` for more than [`LONGEST_DELETE_HOURS`].
pub fn ban(config: &DiscordConfig, hours: u32) -> Result<u32, Refused> {
    if !allowed(config)?.ban {
        return Err(Refused::Moderation(
            "the settings let the bot ban no one".to_string(),
        ));
    }
    if hours > LONGEST_DELETE_HOURS {
        return Err(Refused::Request(format!(
            "a ban deletes at most {LONGEST_DELETE_HOURS} hours of messages"
        )));
    }
    Ok(hours * 3600)
}

/// The highest of `member`'s `roles` (by `position`), as the server's
/// `roles` place them: its position and name; none, @everyone's.
fn top<'a>(member: &Member, roles: &'a [Role]) -> Option<&'a Role> {
    roles
        .iter()
        .filter(|role| member.roles.contains(&role.id))
        .max_by_key(|role| role.position)
}

/// Whether the bot `me` (its member `bot`) may moderate member `target`
/// (its member `member`) in the server owned by `owner` with `roles`: as
/// Discord decides it, checked before Discord is asked.
///
/// # Errors
/// `Refused::Rank`: `target` is the server's owner, the bot itself, or a
/// member whose highest role is not below the bot's.
pub fn reaches(
    me: &str,
    owner: &str,
    bot: &Member,
    target: &str,
    member: &Member,
    roles: &[Role],
) -> Result<(), Refused> {
    if target == owner {
        return Err(Refused::Rank(format!(
            "member {target} owns the server: Discord lets no bot moderate its owner"
        )));
    }
    if target == me {
        return Err(Refused::Rank(
            "the bot does not moderate itself".to_string(),
        ));
    }
    let position = |role: Option<&Role>| role.map_or(0, |r| r.position);
    let (theirs, mine) = (top(member, roles), top(bot, roles));
    if position(theirs) >= position(mine) {
        let role = theirs.map_or("@everyone", |r| r.name.as_str());
        return Err(Refused::Rank(format!(
            "member {target} has the role {role}, which is not below the bot's: Discord lets a bot moderate only members below its highest role"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
