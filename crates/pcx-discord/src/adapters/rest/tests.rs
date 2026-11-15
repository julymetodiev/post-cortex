// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The REST adapter against a server that answers as Discord does.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use parking_lot::Mutex;

use super::*;

const TOKEN: &str = "MTIz.secret-token";

/// What the server saw and how often it was asked.
#[derive(Default)]
struct Seen {
    asked: AtomicUsize,
    headers: Mutex<Vec<(String, String)>>,
    bodies: Mutex<Vec<Value>>,
}

async fn me(State(seen): State<Arc<Seen>>, headers: HeaderMap) -> Json<Value> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    *seen.headers.lock() = vec![
        ("authorization".to_string(), header("authorization")),
        ("user-agent".to_string(), header("user-agent")),
    ];
    Json(value!({
        "id": "9", "username": "pcx", "global_name": null, "bot": true, "avatar": null,
    }))
}

/// Limited once, then sent.
async fn send(State(seen): State<Arc<Seen>>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    seen.bodies.lock().push(body);
    if seen.asked.fetch_add(1, Ordering::SeqCst) == 0 {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(value!({ "message": "You are being rate limited.", "retry_after": 0.01 })),
        );
    }
    (
        StatusCode::OK,
        Json(value!({ "id": "777", "channel_id": "5" })),
    )
}

async fn limited() -> (StatusCode, Json<Value>) {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(value!({ "message": "You are being rate limited.", "retry_after": 0.01 })),
    )
}

async fn slow() -> (StatusCode, Json<Value>) {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(value!({ "message": "You are being rate limited.", "retry_after": 600.0 })),
    )
}

async fn forbidden() -> (StatusCode, Json<Value>) {
    (
        StatusCode::FORBIDDEN,
        Json(value!({ "message": "Missing Permissions", "code": 50013 })),
    )
}

async fn unknown_token() -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(value!({ "message": "401: Unauthorized", "code": 0 })),
    )
}

async fn given(headers: HeaderMap) -> StatusCode {
    // Discord refuses a PUT without a length.
    if headers.contains_key("content-length") {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::LENGTH_REQUIRED
    }
}

async fn discord() -> (HttpRest, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let router = Router::new()
        .route("/users/@me", get(me))
        .route("/channels/5/messages", post(send))
        .route("/channels/6/messages", post(limited))
        .route("/channels/7/messages", post(slow))
        .route("/channels/8/messages", post(forbidden))
        .route("/channels/401", get(unknown_token))
        .route("/guilds/100/members/7/roles/51", put(given))
        .with_state(Arc::clone(&seen));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await });
    let rest = HttpRest::new(&format!("http://{address}/"), TOKEN).unwrap();
    (rest, seen)
}

fn hello() -> CreateMessage {
    CreateMessage {
        content: "hello @everyone".to_string(),
        ..CreateMessage::default()
    }
}

#[tokio::test]
async fn a_call_carries_the_bots_token_and_its_answers_nulls_are_left_out() {
    let (rest, seen) = discord().await;
    let me = rest.me().await.unwrap();
    assert_eq!(
        (me.id.as_str(), me.username.as_str(), me.bot),
        ("9", "pcx", true)
    );
    assert_eq!(me.global_name, "");
    let headers = seen.headers.lock().clone();
    assert_eq!(headers[0].1, format!("Bot {TOKEN}"));
    assert!(
        headers[1].1.starts_with("DiscordBot (https://pcx.is, "),
        "{headers:?}"
    );
}

#[tokio::test]
async fn a_message_mentions_no_one_and_a_rate_limit_is_waited_once() {
    let (rest, seen) = discord().await;
    assert_eq!(rest.send("5", &hello()).await.unwrap(), "777");
    assert_eq!(seen.asked.load(Ordering::SeqCst), 2, "limited, then sent");
    let body = seen.bodies.lock()[1].clone();
    assert_eq!(
        body,
        value!({ "content": "hello @everyone", "allowed_mentions": { "parse": [] } })
    );

    // Limited again after the wait, or for longer than it waits: said.
    for channel in ["6", "7"] {
        let refused = rest.send(channel, &hello()).await.unwrap_err();
        assert_eq!(
            refused.kind,
            ModuleErrorKind::ResourceExhausted,
            "{refused:?}"
        );
    }
}

#[tokio::test]
async fn a_refusal_says_why_and_never_the_token() {
    let (rest, _) = discord().await;
    let refused = rest.send("8", &hello()).await.unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
    assert!(
        refused.message.contains("Missing Permissions"),
        "{}",
        refused.message
    );
    let missing = rest.channel("404").await.unwrap_err();
    assert_eq!(missing.kind, ModuleErrorKind::NotFound);

    // A token Discord does not know is a person's to fix, and says where
    // (the module reports it as its problem: a new organisation's wrong
    // token read "GET /users/@me was refused (401)", 2026-11-14).
    let token = rest.channel("401").await.unwrap_err();
    assert_eq!(token.kind, ModuleErrorKind::FailedPrecondition);
    assert!(
        token.message.contains("the bot's token was not taken")
            && token.message.contains("module's settings"),
        "{}",
        token.message
    );

    // A server that is not there.
    let nowhere = HttpRest::new("http://127.0.0.1:1", TOKEN).unwrap();
    let unreachable = nowhere.me().await.unwrap_err();
    assert_eq!(unreachable.kind, ModuleErrorKind::Unavailable);
    for error in [refused, missing, token, unreachable] {
        assert!(!error.message.contains(TOKEN), "{}", error.message);
        assert!(!error.message.contains("secret"), "{}", error.message);
    }
}

#[tokio::test]
async fn a_role_is_given_with_a_call_that_has_no_body() {
    let (rest, _) = discord().await;
    rest.give_role("100", "7", "51").await.unwrap();
}

/// Against Discord itself, with the bot of `DISCORD_BOT_TOKEN` in the
/// server `DISCORD_GUILD_ID`: what the start reads is what Discord sends
/// (its nulls and the fields this version does not know), and the bot's
/// rights are computed from it for every channel of the server.
#[tokio::test]
#[ignore = "needs DISCORD_BOT_TOKEN, DISCORD_GUILD_ID and the network"]
async fn live_the_server_is_read_as_discord_sends_it() {
    use post_cortex_proto::discord::{Channel, DiscordConfig};

    let token = std::env::var("DISCORD_BOT_TOKEN").unwrap();
    let guild = std::env::var("DISCORD_GUILD_ID").unwrap();
    let rest = HttpRest::new(crate::domain::API_URL, &token).unwrap();
    let channels = rest.channels(&guild).await.unwrap();
    assert!(!channels.is_empty());
    let named: Vec<Channel> = channels
        .iter()
        // A category is no channel to post to or hear.
        .filter(|c| c.r#type != 4)
        .map(|c| Channel {
            name: c.name.clone(),
            channel_id: c.id.clone(),
            post: true,
            hear: true,
        })
        .collect();
    let one = rest.channel(&named[0].channel_id).await.unwrap();
    assert_eq!(one.guild_id, guild);
    let config = DiscordConfig {
        bot_token: token,
        guild_id: guild,
        workspace_id: "live".to_string(),
        channels: named,
        roles: vec!["Member".to_string()],
        ..DiscordConfig::default()
    };
    let server = crate::service::server(&config, &rest).await.unwrap();
    assert_eq!(server.roles.len(), 1, "the role Member, by its id");
    assert!(!server.tags.is_empty(), "the server has a forum");
}
