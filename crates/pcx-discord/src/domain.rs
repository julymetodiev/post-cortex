// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The Discord module's domain (docs/modules/discord.md): what its settings
//! may be, where it may post, what a message may say and how a long one is
//! cut. Pure: no host, no network.

use std::fmt;

use post_cortex_proto::discord::{Channel, DiscordConfig, Embed, EmbedImage};

pub mod gateway;
pub mod heard;
pub mod moderation;
pub mod rights;

/// The module's id.
pub const MODULE_ID: &str = "discord";

/// The API's URL when the settings name none.
pub const API_URL: &str = "https://discord.com/api/v10";

/// Characters of one message, Discord's most.
pub const MESSAGE_CHARS: usize = 2000;

/// Characters of an embed's title and of its text, Discord's most.
const EMBED_CHARS: (usize, usize) = (256, 4096);

/// Pictures of one embed: what Discord shows side by side.
const EMBED_IMAGES: usize = 4;

/// Digits of a Discord id (a 64-bit number), at most.
const ID_DIGITS: usize = 20;

/// A setting or a request the module refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// A setting is wrong.
    Setting {
        /// The setting, as a path of field names.
        field: String,
        /// What is wrong.
        reason: String,
    },
    /// The settings name no such channel, or not one the bot posts to.
    Channel(String),
    /// The settings name no such role.
    Role(String),
    /// The settings name no such forum tag.
    Tag(String),
    /// A thread of a channel the bot does not post to.
    Thread(String),
    /// What the settings do not let the bot do to members and posts.
    Moderation(String),
    /// A member Discord lets no bot moderate here: the server's owner, the
    /// bot itself, or one whose role is not below the bot's.
    Rank(String),
    /// A request that names nothing, or what is no Discord id.
    Request(String),
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Setting { field, reason } => write!(f, "{field}: {reason}"),
            Self::Channel(name) => {
                write!(f, "the settings name no channel {name} the bot posts to")
            }
            Self::Role(name) => write!(f, "the settings name no role {name} the bot may give"),
            Self::Tag(name) => write!(f, "the settings name no forum tag {name} the bot may set"),
            Self::Thread(id) => write!(
                f,
                "{id} is no post or thread of a channel the settings let the bot post to"
            ),
            Self::Moderation(reason) | Self::Rank(reason) | Self::Request(reason) => {
                f.write_str(reason)
            }
        }
    }
}

impl std::error::Error for Refused {}

fn setting(field: String, reason: impl Into<String>) -> Refused {
    Refused::Setting {
        field,
        reason: reason.into(),
    }
}

/// Whether `id` may be a Discord id: 1 to 20 digits.
#[must_use]
pub fn is_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= ID_DIGITS && id.bytes().all(|b| b.is_ascii_digit())
}

/// `id`, a Discord id a request names as `what`.
///
/// # Errors
/// It is none.
pub fn id<'a>(what: &str, id: &'a str) -> Result<&'a str, Refused> {
    if is_id(id) {
        Ok(id)
    } else {
        Err(Refused::Request(format!("{what} is a Discord id: digits")))
    }
}

/// Whether `config` may run: a token, a server and a workspace; channels
/// named once, each a Discord id named once; roles and tags named once;
/// a moderation's longest timeout within Discord's.
///
/// # Errors
/// The first setting that is wrong.
pub fn check(config: &DiscordConfig) -> Result<(), Refused> {
    if config.bot_token.trim().is_empty() {
        return Err(setting("bot_token".to_string(), "is empty"));
    }
    if !is_id(&config.guild_id) {
        return Err(setting(
            "guild_id".to_string(),
            "is the server's Discord id: digits",
        ));
    }
    if config.workspace_id.trim().is_empty() {
        return Err(setting("workspace_id".to_string(), "is empty"));
    }
    let (mut names, mut ids): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
    for (i, channel) in config.channels.iter().enumerate() {
        let name = channel.name.as_str();
        if !pcx_module_api::name::is_slug(name) {
            return Err(setting(
                format!("channels.{i}.name"),
                "is 1 to 64 of a-z, 0-9 and '-'",
            ));
        }
        if names.contains(&name) {
            return Err(setting(
                format!("channels.{i}.name"),
                format!("names {name} twice"),
            ));
        }
        names.push(name);
        let id = channel.channel_id.as_str();
        if !is_id(id) {
            return Err(setting(
                format!("channels.{i}.channel_id"),
                "is the channel's Discord id: digits",
            ));
        }
        if ids.contains(&id) {
            return Err(setting(
                format!("channels.{i}.channel_id"),
                format!("names channel {id} twice"),
            ));
        }
        ids.push(id);
    }
    for (field, list) in [("roles", &config.roles), ("tags", &config.tags)] {
        for (i, name) in list.iter().enumerate() {
            if name.trim().is_empty() {
                return Err(setting(format!("{field}.{i}"), "is empty"));
            }
            if list[..i].contains(name) {
                return Err(setting(
                    format!("{field}.{i}"),
                    format!("names {name} twice"),
                ));
            }
        }
    }
    config.moderation.as_ref().map_or(Ok(()), moderation::check)
}

/// The channel named `name` the bot may post to.
///
/// # Errors
/// The settings name none, or one it does not post to.
pub fn destination<'a>(config: &'a DiscordConfig, name: &str) -> Result<&'a Channel, Refused> {
    config
        .channels
        .iter()
        .find(|c| c.name == name && c.post)
        .ok_or_else(|| Refused::Channel(name.to_string()))
}

/// The channel of Discord id `id`, when the settings name it.
#[must_use]
pub fn channel<'a>(config: &'a DiscordConfig, id: &str) -> Option<&'a Channel> {
    config.channels.iter().find(|c| c.channel_id == id)
}

/// What a message says: `text` without the space around it.
///
/// # Errors
/// It says nothing.
pub fn said(text: &str) -> Result<&str, Refused> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Refused::Request("a message says something".to_string()));
    }
    Ok(text)
}

/// `embed` as the cards Discord takes: itself with its first picture, and a
/// card of the same link for each further picture, which Discord shows as
/// one card with the pictures side by side.
///
/// # Errors
/// What is wrong with it: as [`card`], more pictures than Discord shows
/// together, one that is no https address, or several without a link.
pub fn cards(embed: Embed) -> Result<Vec<Embed>, Refused> {
    card(&embed)?;
    if embed.images.len() > EMBED_IMAGES {
        return Err(Refused::Request(format!(
            "an embed has at most {EMBED_IMAGES} images"
        )));
    }
    if embed
        .images
        .iter()
        .any(|image| !image.starts_with("https://"))
    {
        return Err(Refused::Request("an embed's image is https".to_string()));
    }
    if embed.images.len() > 1 && embed.url.is_empty() {
        return Err(Refused::Request(
            "an embed with several images has a url: Discord shows the cards of one link together"
                .to_string(),
        ));
    }
    let picture = |url: &String| EmbedImage { url: url.clone() };
    let mut first = Embed {
        image: embed.images.first().map(picture),
        ..embed
    };
    let more: Vec<Embed> = first
        .images
        .iter()
        .skip(1)
        .map(|image| Embed {
            url: first.url.clone(),
            image: Some(picture(image)),
            ..Embed::default()
        })
        .collect();
    first.images.clear();
    Ok(std::iter::once(first).chain(more).collect())
}

/// `embed`, when it may be sent: a title or a text, each within Discord's
/// most, and a link that is one.
///
/// # Errors
/// What is wrong with it.
fn card(embed: &Embed) -> Result<(), Refused> {
    let (title, text) = (
        embed.title.chars().count(),
        embed.description.chars().count(),
    );
    if title == 0 && text == 0 {
        return Err(Refused::Request(
            "an embed has a title or a description".to_string(),
        ));
    }
    if title > EMBED_CHARS.0 || text > EMBED_CHARS.1 {
        return Err(Refused::Request(format!(
            "an embed's title is at most {} characters and its description {}",
            EMBED_CHARS.0, EMBED_CHARS.1
        )));
    }
    if !embed.url.is_empty() && !embed.url.starts_with("https://") {
        return Err(Refused::Request("an embed's url is https".to_string()));
    }
    Ok(())
}

/// `text` as the messages it goes in: whole when it fits one, else cut at
/// the last line end (or space, or anywhere) that fits, each piece at most
/// [`MESSAGE_CHARS`] characters.
#[must_use]
pub fn pieces(text: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while rest.chars().count() > MESSAGE_CHARS {
        // The byte where the most characters of one message end.
        let most = rest
            .char_indices()
            .nth(MESSAGE_CHARS)
            .map_or(rest.len(), |(at, _)| at);
        let head = &rest[..most];
        let cut = head
            .rfind('\n')
            .or_else(|| head.rfind(' '))
            .filter(|at| *at > 0)
            .unwrap_or(most);
        let piece = rest[..cut].trim_end();
        if !piece.is_empty() {
            pieces.push(piece);
        }
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        pieces.push(rest);
    }
    pieces
}

#[cfg(test)]
mod tests;
