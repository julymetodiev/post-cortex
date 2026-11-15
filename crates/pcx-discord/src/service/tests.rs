// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The service over Discord kept in memory: what it sends and where, what
//! it refuses, what it hears, and one connection of the Gateway from a
//! script of frames.

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use post_cortex_proto::discord::{Channel, ForumTag, Guild, Member, Role, User};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::domain::gateway::{Frame, Sent, Session};
use crate::domain::heard::Heard;
use crate::ports::{Gateway, Link};

const GUILD: &str = "100";

fn config() -> DiscordConfig {
    let channel = |name: &str, id: &str, post: bool, hear: bool| Channel {
        name: name.to_string(),
        channel_id: id.to_string(),
        post,
        hear,
    };
    DiscordConfig {
        bot_token: "token".to_string(),
        guild_id: GUILD.to_string(),
        workspace_id: "public".to_string(),
        channels: vec![
            channel("help", "200", true, true),
            channel("general", "201", false, true),
        ],
        roles: vec!["Member".to_string()],
        tags: vec!["answered".to_string()],
        moderation: None,
    }
}

fn thread(id: &str, parent: &str) -> GuildChannel {
    GuildChannel {
        id: id.to_string(),
        r#type: 11,
        guild_id: GUILD.to_string(),
        name: "How do I pair a runner?".to_string(),
        parent_id: parent.to_string(),
        ..GuildChannel::default()
    }
}

/// Discord in memory: what was sent, the roles given, the tags set.
#[derive(Default)]
struct Discord {
    /// Every right the settings need, or none of the bot's own.
    powerless: bool,
    threads: Mutex<Vec<GuildChannel>>,
    sent: Mutex<Vec<(String, CreateMessage)>>,
    given: Mutex<Vec<(String, String)>>,
    asked: Mutex<Vec<String>>,
    /// Each timeout given (member, until; none: lifted), lock and ban
    /// (member, seconds deleted).
    timeouts: Mutex<Vec<(String, Option<String>)>>,
    locked: Mutex<Vec<String>>,
    banned: Mutex<Vec<(String, u32)>>,
}

#[async_trait]
impl Rest for Discord {
    async fn gateway(&self) -> Result<String, ModuleError> {
        Ok("wss://gateway.example".to_string())
    }

    async fn me(&self) -> Result<User, ModuleError> {
        Ok(User {
            id: "9".to_string(),
            username: "pcx".to_string(),
            bot: true,
            ..User::default()
        })
    }

    /// The bot (9) has the role pcx, member 2 Moderators (above it), any
    /// other Member.
    async fn member(&self, guild: &str, user: &str) -> Result<Member, ModuleError> {
        if guild != GUILD {
            return Err(ModuleError::new(ModuleErrorKind::NotFound, "Unknown Guild"));
        }
        let role = match user {
            "9" => "50",
            "2" => "52",
            _ => "51",
        };
        Ok(Member {
            roles: vec![role.to_string()],
            ..Member::default()
        })
    }

    /// Owned by member 1.
    async fn guild(&self, guild: &str) -> Result<Guild, ModuleError> {
        Ok(Guild {
            id: guild.to_string(),
            owner_id: "1".to_string(),
        })
    }

    async fn roles(&self, _guild: &str) -> Result<Vec<Role>, ModuleError> {
        let role = |id: &str, name: &str, permissions: u64, position: i32| Role {
            id: id.to_string(),
            name: name.to_string(),
            permissions: permissions.to_string(),
            position,
        };
        // Administrator, or nothing.
        let own = if self.powerless { 0 } else { 1 << 3 };
        Ok(vec![
            role(GUILD, "@everyone", 0, 0),
            role("50", "pcx", own, 5),
            role("51", "Member", 0, 1),
            role("52", "Moderators", 0, 6),
        ])
    }

    async fn channels(&self, _guild: &str) -> Result<Vec<GuildChannel>, ModuleError> {
        let channel = |id: &str, kind: i32| GuildChannel {
            id: id.to_string(),
            r#type: kind,
            guild_id: GUILD.to_string(),
            available_tags: vec![ForumTag {
                id: "71".to_string(),
                name: "answered".to_string(),
            }],
            ..GuildChannel::default()
        };
        Ok(vec![channel("200", 15), channel("201", 0)])
    }

    async fn channel(&self, id: &str) -> Result<GuildChannel, ModuleError> {
        self.asked.lock().push(id.to_string());
        self.threads
            .lock()
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .ok_or_else(|| ModuleError::new(ModuleErrorKind::NotFound, "Unknown Channel"))
    }

    async fn send(&self, channel: &str, message: &CreateMessage) -> Result<String, ModuleError> {
        let mut sent = self.sent.lock();
        sent.push((channel.to_string(), message.clone()));
        Ok(format!("m{}", sent.len()))
    }

    async fn give_role(&self, _guild: &str, member: &str, role: &str) -> Result<(), ModuleError> {
        self.given
            .lock()
            .push((member.to_string(), role.to_string()));
        Ok(())
    }

    async fn retag(&self, thread: &str, tags: &[String]) -> Result<(), ModuleError> {
        let mut threads = self.threads.lock();
        let post = threads.iter_mut().find(|t| t.id == thread).unwrap();
        post.applied_tags = tags.to_vec();
        Ok(())
    }

    async fn time_out(
        &self,
        _guild: &str,
        member: &str,
        until: Option<&str>,
    ) -> Result<(), ModuleError> {
        self.timeouts
            .lock()
            .push((member.to_string(), until.map(str::to_string)));
        Ok(())
    }

    async fn lock(&self, thread: &str) -> Result<(), ModuleError> {
        self.locked.lock().push(thread.to_string());
        Ok(())
    }

    async fn ban(
        &self,
        _guild: &str,
        member: &str,
        delete_seconds: u32,
    ) -> Result<(), ModuleError> {
        self.banned
            .lock()
            .push((member.to_string(), delete_seconds));
        Ok(())
    }
}

/// The bus in memory.
#[derive(Default)]
struct Bus(Mutex<Vec<Heard>>);

#[async_trait]
impl Events for Bus {
    async fn publish(&self, heard: Heard) {
        self.0.lock().push(heard);
    }
}

/// A running module over `discord` and `bus`.
struct World {
    config: DiscordConfig,
    server: Arc<Server>,
    kept: Kept,
    discord: Discord,
    bus: Bus,
}

impl World {
    async fn new() -> Self {
        Self::with(config()).await
    }

    async fn with(config: DiscordConfig) -> Self {
        let discord = Discord::default();
        let server = Arc::new(server(&config, &discord).await.unwrap());
        Self {
            config,
            server,
            kept: Kept::default(),
            discord,
            bus: Bus::default(),
        }
    }

    fn service(&self) -> Service<'_> {
        Service::new(
            &self.config,
            Arc::clone(&self.server),
            &self.kept,
            &self.discord,
            &self.bus,
        )
    }
}

mod listen;
mod moderation;
mod start;

#[tokio::test]
async fn it_posts_only_to_the_channels_its_settings_let_it() {
    let world = World::new().await;
    let service = world.service();
    assert_eq!(service.send("help", " down ").await.unwrap(), "m1");
    let embed = Embed {
        title: "0.5.1".to_string(),
        description: "Fixes".to_string(),
        ..Embed::default()
    };
    assert_eq!(
        service.post("help", "", Some(embed.clone())).await.unwrap(),
        "m2"
    );
    {
        let sent = world.discord.sent.lock();
        assert_eq!(
            (sent[0].0.as_str(), sent[0].1.content.as_str()),
            ("200", "down")
        );
        assert_eq!(sent[1].1.embeds, [embed]);
    }
    // It hears `general` and does not post there; `ops` is not named.
    for channel in ["general", "ops"] {
        let refused = service.send(channel, "hi").await.unwrap_err();
        assert_eq!(refused.kind, ModuleErrorKind::Denied, "{channel}");
    }
    let empty = service.send("help", " ").await.unwrap_err();
    assert_eq!(empty.kind, ModuleErrorKind::InvalidArgument);
    assert_eq!(world.discord.sent.lock().len(), 2);
}

#[tokio::test]
async fn a_long_answer_goes_as_several_messages_the_first_a_reply() {
    let world = World::new().await;
    world.discord.threads.lock().push(thread("300", "200"));
    let line = "a".repeat(1500);
    let first = world
        .service()
        .reply("300", "", "300", &format!("{line}\n{line}"))
        .await
        .unwrap();
    assert_eq!(first, "m1");
    let sent = world.discord.sent.lock();
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().all(|(to, m)| to == "300" && m.content == line));
    assert_eq!(
        sent[0].1.message_reference.as_ref().unwrap().message_id,
        "300"
    );
    assert_eq!(sent[1].1.message_reference, None);
}

#[tokio::test]
async fn a_reply_goes_only_into_a_thread_of_a_channel_it_posts_to() {
    let world = World::new().await;
    world.discord.threads.lock().extend([
        thread("300", "200"),
        // A thread of `general`, which it only hears; one of another
        // server; a channel that is no thread.
        thread("301", "201"),
        GuildChannel {
            guild_id: "101".to_string(),
            ..thread("302", "200")
        },
        GuildChannel {
            r#type: 0,
            ..thread("303", "200")
        },
    ]);
    let service = world.service();
    service
        .reply("300", "", "", "pcx connect, then pcx status")
        .await
        .unwrap();
    for foreign in ["301", "302", "303", "999"] {
        let refused = service.reply(foreign, "", "", "hi").await.unwrap_err();
        assert_eq!(refused.kind, ModuleErrorKind::Denied, "{foreign}");
    }
    let no_id = service
        .reply("../guilds/100", "", "", "hi")
        .await
        .unwrap_err();
    assert_eq!(no_id.kind, ModuleErrorKind::InvalidArgument);
    // By a channel's name instead; a thread is read from Discord once.
    service.reply("", "help", "", "hi").await.unwrap();
    service.reply("300", "", "", "again").await.unwrap();
    assert_eq!(world.discord.sent.lock().len(), 3);
    let asked = world.discord.asked.lock();
    assert_eq!(asked.iter().filter(|id| *id == "300").count(), 1);
}

#[tokio::test]
async fn it_gives_only_a_role_and_sets_only_a_tag_its_settings_name() {
    let world = World::new().await;
    world.discord.threads.lock().push(GuildChannel {
        applied_tags: vec!["73".to_string()],
        ..thread("300", "200")
    });
    let service = world.service();
    service.give_role("7", "Member").await.unwrap();
    assert_eq!(
        *world.discord.given.lock(),
        [("7".to_string(), "51".to_string())]
    );
    let refused = service.give_role("7", "Owner").await.unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
    assert!(service.give_role("ana", "Member").await.is_err());

    // The tag is added to those the post has; set again, nothing changes.
    service.tag_post("300", "answered").await.unwrap();
    service.tag_post("300", "answered").await.unwrap();
    assert_eq!(world.discord.threads.lock()[0].applied_tags, ["73", "71"]);
    let unnamed = service.tag_post("300", "bug").await.unwrap_err();
    assert_eq!(unnamed.kind, ModuleErrorKind::Denied);
    assert_eq!(world.discord.given.lock().len(), 1);
}
