// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `telegram` module (ARCHITECTURE section 8, operations, W29): the
//! first channel. It serves `pcx.channel.v1.Channel` (a message to one of
//! its chats), sends each decision that waits for a person to its
//! approvals chat with buttons that approve or decline it as the person
//! who pressed (the host admits that for the persons its settings name),
//! and takes a person's message as a question to the chief. The domain
//! holds the rules; the service applies them over the Bot API, the board
//! and the chief (its ports); the handler only maps.

#![forbid(unsafe_code)]

mod adapters;
mod bot;
mod domain;
mod handlers;
mod module;
mod ports;
mod prompts;
mod service;

pub use module::TelegramModule;
