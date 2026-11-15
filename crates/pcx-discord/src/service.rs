// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The Discord module's service (docs/modules/discord.md): a message to a
//! channel, a reply in a post, a role given, a forum tag set, each only
//! where the settings name it; a timeout, a lock and a ban, only as their
//! moderation lets it; and what the Gateway says of the server,
//! published as events. Between the handlers (and the module's listener)
//! and the ports.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use pcx_module_api::{ModuleError, ModuleErrorKind, json};
use post_cortex_proto::discord::{
    AutomodExecution, CreateMessage, DiscordConfig, DiscordMessage, Embed, GuildChannel,
    GuildCreate, MemberAdd, MessageReference,
};
use serde_json::Value;

use crate::domain::heard::{self, is_thread};
use crate::domain::rights::{self, Server};
use crate::domain::{self, Refused};
use crate::ports::{Events, Rest};

mod linked;
mod listen;
mod moderate;

pub use linked::Linked;

fn refused(rule: Refused) -> ModuleError {
    let kind = match rule {
        Refused::Setting { .. } | Refused::Request(_) => ModuleErrorKind::InvalidArgument,
        Refused::Channel(_)
        | Refused::Role(_)
        | Refused::Tag(_)
        | Refused::Thread(_)
        | Refused::Moderation(_) => ModuleErrorKind::Denied,
        Refused::Rank(_) => ModuleErrorKind::FailedPrecondition,
    };
    let error = ModuleError::new(kind, rule.to_string());
    match rule {
        Refused::Setting { field, .. } => error.with_field(field),
        _ => error,
    }
}

/// `config` checked before the module starts.
///
/// # Errors
/// `InvalidArgument` naming the first wrong setting.
pub fn checked(config: DiscordConfig) -> Result<DiscordConfig, ModuleError> {
    domain::check(&config).map_err(refused)?;
    Ok(config)
}

/// The server as `config` needs it, read over `rest`: the bot, its rights
/// in each channel the settings name, the roles and the forum tags by
/// name.
///
/// # Errors
/// `FailedPrecondition` when the bot is not in the server, or listing
/// everything it lacks there, each with the link that adds it; the API
/// failed.
pub async fn server(config: &DiscordConfig, rest: &dyn Rest) -> Result<Server, ModuleError> {
    let guild = config.guild_id.as_str();
    let me = rest.me().await?;
    let invite = rights::invite(&me.id);
    let member = rest.member(guild, &me.id).await.map_err(|e| match e.kind {
        ModuleErrorKind::NotFound | ModuleErrorKind::Denied => ModuleError::new(
            ModuleErrorKind::FailedPrecondition,
            format!("the bot is not in server {guild}: add it with {invite}"),
        ),
        _ => e,
    })?;
    let roles = rest.roles(guild).await?;
    let channels = rest.channels(guild).await?;
    rights::server(config, &me.id, &member, &roles, &channels).map_err(|lacks| {
        ModuleError::new(
            ModuleErrorKind::FailedPrecondition,
            format!(
                "the bot cannot do in its server what its settings ask: {}. The link that adds it with the rights it may need: {invite}",
                lacks.join("; ")
            ),
        )
    })
}

/// What the module keeps while it runs: the threads of its server it has
/// seen (none: a channel that is no thread of it), by id.
#[derive(Default)]
pub struct Kept {
    threads: Mutex<HashMap<String, Option<GuildChannel>>>,
}

/// The module over its ports, as `config` sets it in `server`.
pub struct Service<'a> {
    config: &'a DiscordConfig,
    server: Arc<Server>,
    kept: &'a Kept,
    rest: &'a dyn Rest,
    events: &'a dyn Events,
}

impl<'a> Service<'a> {
    /// The service.
    pub fn new(
        config: &'a DiscordConfig,
        server: Arc<Server>,
        kept: &'a Kept,
        rest: &'a dyn Rest,
        events: &'a dyn Events,
    ) -> Self {
        Self {
            config,
            server,
            kept,
            rest,
            events,
        }
    }

    /// Send `text` to the channel named `destination`: the message's id.
    ///
    /// # Errors
    /// `Denied` for a channel the settings do not let it post to;
    /// `InvalidArgument` for an empty text; the API failed.
    pub async fn send(&self, destination: &str, text: &str) -> Result<String, ModuleError> {
        self.post(destination, text, None).await
    }

    /// Post `text`, with `embed` under it, in the channel named `channel`:
    /// the message's id.
    ///
    /// # Errors
    /// As [`Service::send`]; `InvalidArgument` for an embed that may not be
    /// sent.
    pub async fn post(
        &self,
        channel: &str,
        text: &str,
        embed: Option<Embed>,
    ) -> Result<String, ModuleError> {
        let to = domain::destination(self.config, channel).map_err(refused)?;
        self.say(&to.channel_id, text, embed, "").await
    }

    /// Reply with `text` in post or thread `thread_id`, or in the channel
    /// named `channel`, to message `message_id` when it names one: the
    /// reply's id.
    ///
    /// # Errors
    /// `Denied` for a channel, or a thread of one, the settings do not let
    /// it post to; `InvalidArgument` for an empty text or what is no id;
    /// the API failed.
    pub async fn reply(
        &self,
        thread_id: &str,
        channel: &str,
        message_id: &str,
        text: &str,
    ) -> Result<String, ModuleError> {
        if !message_id.is_empty() {
            domain::id("message_id", message_id).map_err(refused)?;
        }
        if thread_id.is_empty() {
            let to = domain::destination(self.config, channel).map_err(refused)?;
            return self.say(&to.channel_id, text, None, message_id).await;
        }
        let thread = self.ours(thread_id).await?;
        self.say(&thread.id, text, None, message_id).await
    }

    /// Give member `member_id` the role named `role`.
    ///
    /// # Errors
    /// `Denied` for a role the settings do not name; `InvalidArgument` for
    /// what is no id; the API failed.
    pub async fn give_role(&self, member_id: &str, role: &str) -> Result<(), ModuleError> {
        let member = domain::id("member_id", member_id).map_err(refused)?;
        let (_, id) = self
            .server
            .roles
            .iter()
            .find(|(name, _)| name == role)
            .ok_or_else(|| refused(Refused::Role(role.to_string())))?;
        self.rest.give_role(&self.config.guild_id, member, id).await
    }

    /// Set the forum tag named `tag` on post `thread_id`, with the tags it
    /// has.
    ///
    /// # Errors
    /// `Denied` for a tag the settings do not name or its forum does not
    /// have, or a post of a channel it does not post to; the API failed.
    pub async fn tag_post(&self, thread_id: &str, tag: &str) -> Result<(), ModuleError> {
        let unknown = || refused(Refused::Tag(tag.to_string()));
        if !self.config.tags.iter().any(|name| name == tag) {
            return Err(unknown());
        }
        let seen = self.ours(thread_id).await?;
        let (id, _) = self
            .server
            .tags
            .iter()
            .find(|(forum, _)| *forum == seen.parent_id)
            .and_then(|(_, tags)| tags.iter().find(|(_, name)| name == tag))
            .ok_or_else(unknown)?;
        // Its tags as they are now, not as they were when it was seen.
        let mut post = self.rest.channel(&seen.id).await?;
        if !post.applied_tags.contains(id) {
            post.applied_tags.push(id.clone());
            self.rest.retag(&post.id, &post.applied_tags).await?;
        }
        self.kept.threads.lock().insert(post.id.clone(), Some(post));
        Ok(())
    }

    /// Say `text` (with `embed` under its last message) in channel or
    /// thread `to`, replying to `message_id` when it names one: the first
    /// message's id.
    async fn say(
        &self,
        to: &str,
        text: &str,
        embed: Option<Embed>,
        message_id: &str,
    ) -> Result<String, ModuleError> {
        let (pieces, cards) = match embed {
            Some(card) => {
                let cards = domain::cards(card).map_err(refused)?;
                // A card may stand alone.
                let text = text.trim();
                if text.is_empty() {
                    (vec![""], cards)
                } else {
                    (domain::pieces(text), cards)
                }
            }
            None => (
                domain::pieces(domain::said(text).map_err(refused)?),
                Vec::new(),
            ),
        };
        let last = pieces.len().saturating_sub(1);
        let mut first = String::new();
        for (i, piece) in pieces.into_iter().enumerate() {
            let message = CreateMessage {
                content: piece.to_string(),
                embeds: if i == last { cards.clone() } else { Vec::new() },
                message_reference: (i == 0 && !message_id.is_empty()).then(|| MessageReference {
                    message_id: message_id.to_string(),
                }),
            };
            let id = self.rest.send(to, &message).await?;
            if i == 0 {
                first = id;
            }
        }
        Ok(first)
    }

    /// Thread `id`, when it is one of a channel the settings let the bot
    /// post to.
    async fn ours(&self, id: &str) -> Result<GuildChannel, ModuleError> {
        let id = domain::id("thread_id", id).map_err(refused)?;
        self.thread(id)
            .await?
            .filter(|thread| {
                domain::channel(self.config, &thread.parent_id).is_some_and(|parent| parent.post)
            })
            .ok_or_else(|| refused(Refused::Thread(id.to_string())))
    }

    /// Thread `id` of the server, as it was last seen, else as the API
    /// answers it now; none for what is no thread of the server.
    async fn thread(&self, id: &str) -> Result<Option<GuildChannel>, ModuleError> {
        if let Some(seen) = self.kept.threads.lock().get(id) {
            return Ok(seen.clone());
        }
        let found = match self.rest.channel(id).await {
            Ok(channel) => {
                Some(channel).filter(|c| is_thread(c) && c.guild_id == self.config.guild_id)
            }
            Err(e) if e.kind == ModuleErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        self.kept
            .threads
            .lock()
            .insert(id.to_string(), found.clone());
        Ok(found)
    }

    /// The Gateway said `event` of the server, carrying `d`: what of it the
    /// settings hear is published.
    ///
    /// # Errors
    /// `d` is not what the event carries; a thread could not be read.
    pub async fn heard(&self, event: &str, d: Value) -> Result<(), ModuleError> {
        fn carried<T: serde::de::DeserializeOwned>(
            event: &str,
            d: Value,
        ) -> Result<T, ModuleError> {
            json::decode_value(d).map_err(|e| {
                ModuleError::new(
                    ModuleErrorKind::Unavailable,
                    format!("Discord's {event} is not what it sends: {e}"),
                )
            })
        }
        let heard = match event {
            "MESSAGE_CREATE" => {
                let message: DiscordMessage = carried(event, d)?;
                if !heard::of_a_person(self.config, &self.server, &message) {
                    return Ok(());
                }
                let thread = match domain::channel(self.config, &message.channel_id) {
                    Some(_) => None,
                    None => self.thread(&message.channel_id).await?,
                };
                heard::message(self.config, &self.server, &message, thread.as_ref())
            }
            "THREAD_CREATE" | "THREAD_UPDATE" => {
                let thread: GuildChannel = carried(event, d)?;
                if thread.guild_id == self.config.guild_id {
                    self.kept
                        .threads
                        .lock()
                        .insert(thread.id.clone(), Some(thread));
                }
                None
            }
            "THREAD_DELETE" => {
                let thread: GuildChannel = carried(event, d)?;
                self.kept.threads.lock().remove(&thread.id);
                None
            }
            "GUILD_CREATE" => {
                let guild: GuildCreate = carried(event, d)?;
                if guild.id == self.config.guild_id {
                    let mut threads = self.kept.threads.lock();
                    for thread in guild.threads {
                        // A thread of the list names no server: it is the
                        // list's.
                        threads.insert(
                            thread.id.clone(),
                            Some(GuildChannel {
                                guild_id: guild.id.clone(),
                                ..thread
                            }),
                        );
                    }
                }
                None
            }
            "GUILD_MEMBER_ADD" => {
                let added: MemberAdd = carried(event, d)?;
                heard::joined(self.config, &added)
            }
            "AUTO_MODERATION_ACTION_EXECUTION" => {
                let acted: AutomodExecution = carried(event, d)?;
                heard::automod(self.config, &acted)
            }
            _ => None,
        };
        if let Some(heard) = heard {
            self.events.publish(heard).await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
