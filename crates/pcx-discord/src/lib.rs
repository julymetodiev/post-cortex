// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `discord` module (docs/modules/discord.md): a channel to a Discord
//! server and a community run through workflows. It serves
//! `pcx.channel.v1.Channel` (a message to one of its channels) beside
//! Telegram, and `pcx.discord.v1.Discord` (posts, replies, roles, forum
//! tags, each only where its settings name it); it listens on Discord's
//! Gateway and publishes what the server's people write as events on the
//! bus, which workflows follow. It answers nothing by itself. The domain
//! holds the rules; the service applies them over Discord's REST API, its
//! Gateway and the bus (its ports); the handlers only map.

#![forbid(unsafe_code)]

mod adapters;
mod domain;
mod handlers;
mod module;
mod ports;
mod service;
mod templates;

pub use module::DiscordModule;
