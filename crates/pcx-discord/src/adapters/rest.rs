// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Discord's REST API (ARCHITECTURE section 8, the outside world's JSON):
//! each call's body a mirror message written by the canonical proto3 JSON
//! mapping, each answer decoded into its mirror message (its nulls fields
//! left out). The token goes in a header, never in a URL, and no error
//! carries it. A `429` waits what it names and tries again once.

use std::time::Duration;

use async_trait::async_trait;
use pcx_module_api::{ModuleError, ModuleErrorKind, json};
use post_cortex_proto::discord::{
    ApiError, CreateBan, CreateMessage, DiscordMessage, EditMember, EditThread, GatewayBot, Guild,
    GuildChannel, Member, Role, User,
};
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json as value};

use crate::ports::Rest;

/// How long a call may take.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The longest a `429` is waited for; a longer one is the caller's to try
/// again.
const LONGEST_WAIT: Duration = Duration::from_secs(10);

/// What the bot calls itself, as Discord asks of a bot's `User-Agent`.
const USER_AGENT: &str = concat!(
    "DiscordBot (https://pcx.is, ",
    env!("CARGO_PKG_VERSION"),
    ")"
);

fn failed(kind: ModuleErrorKind, what: impl std::fmt::Display) -> ModuleError {
    ModuleError::new(kind, format!("Discord: {what}"))
}

/// What kind of failure `e` is, without its URL.
fn kind(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "no connection"
    } else {
        "failed"
    }
}

/// Discord's REST API of one bot.
pub struct HttpRest {
    http: reqwest::Client,
    base: String,
    authorization: String,
}

impl HttpRest {
    /// The bot of `token` at `api` (e.g. `https://discord.com/api/v10`).
    ///
    /// # Errors
    /// The HTTP client could not be made.
    pub fn new(api: &str, token: &str) -> Result<Self, ModuleError> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| failed(ModuleErrorKind::Unavailable, kind(&e)))?;
        Ok(Self {
            http,
            base: api.trim_end_matches('/').to_string(),
            authorization: format!("Bot {}", token.trim()),
        })
    }

    /// One exchange: the answer's status and body.
    async fn exchange(
        &self,
        method: &Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(StatusCode, Vec<u8>), ModuleError> {
        let mut request = self
            .http
            .request(method.clone(), format!("{}{path}", self.base))
            .header(reqwest::header::AUTHORIZATION, &self.authorization);
        if let Some(body) = body {
            request = request.json(body);
        } else if *method != Method::GET {
            // Discord asks a length of a call without a body.
            request = request.header(reqwest::header::CONTENT_LENGTH, 0);
        }
        let unavailable = |what: &str, e: &reqwest::Error| {
            failed(
                ModuleErrorKind::Unavailable,
                format!("{method} {path} {what} ({})", kind(e)),
            )
        };
        let response = request
            .send()
            .await
            .map_err(|e| unavailable("could not be sent", &e))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|e| unavailable("was not answered whole", &e))?;
        Ok((status, body.to_vec()))
    }

    /// Call `method` `path` with `body`: the answer's body.
    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Vec<u8>, ModuleError> {
        let (mut status, mut answer) = self.exchange(&method, path, body).await?;
        if status == StatusCode::TOO_MANY_REQUESTS {
            let wait = refusal(&answer).retry_after;
            let wait = Duration::try_from_secs_f64(wait).unwrap_or(LONGEST_WAIT);
            if wait > LONGEST_WAIT {
                return Err(failed(
                    ModuleErrorKind::ResourceExhausted,
                    format!("{method} {path} is rate limited for {} s", wait.as_secs()),
                ));
            }
            tokio::time::sleep(wait).await;
            (status, answer) = self.exchange(&method, path, body).await?;
        }
        if status.is_success() {
            return Ok(answer);
        }
        let why = refusal(&answer).message;
        let kind = match status {
            StatusCode::UNAUTHORIZED => ModuleErrorKind::FailedPrecondition,
            StatusCode::FORBIDDEN => ModuleErrorKind::Denied,
            StatusCode::NOT_FOUND => ModuleErrorKind::NotFound,
            StatusCode::TOO_MANY_REQUESTS => ModuleErrorKind::ResourceExhausted,
            status if status.is_server_error() => ModuleErrorKind::Unavailable,
            _ => ModuleErrorKind::FailedPrecondition,
        };
        // A token Discord does not know is a person's to fix, in the
        // module's settings: said so, since the module reports it.
        let what = if status == StatusCode::UNAUTHORIZED {
            "the bot's token was not taken (401): set the bot's token from the Developer Portal in the module's settings".to_string()
        } else {
            format!("{method} {path} was refused ({}): {why}", status.as_u16())
        };
        Err(failed(kind, what))
    }

    /// Call `method` `path` with `body`: its answer as `O`.
    async fn answered<O: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<O, ModuleError> {
        let answer = self.call(method.clone(), path, body).await?;
        json::decode(&answer).map_err(|e| {
            failed(
                ModuleErrorKind::Unavailable,
                format!("{method} {path} answered what is not its answer: {e}"),
            )
        })
    }
}

/// Why Discord refused, as far as its answer says.
fn refusal(body: &[u8]) -> ApiError {
    json::decode(body).unwrap_or_else(|_| ApiError {
        message: "it did not say why".to_string(),
        ..ApiError::default()
    })
}

/// `input`'s proto3 JSON.
fn written<I: Serialize>(input: &I) -> Result<Value, ModuleError> {
    serde_json::to_value(input).map_err(|e| failed(ModuleErrorKind::Internal, e))
}

#[async_trait]
impl Rest for HttpRest {
    async fn gateway(&self) -> Result<String, ModuleError> {
        let found: GatewayBot = self.answered(Method::GET, "/gateway/bot", None).await?;
        Ok(found.url)
    }

    async fn me(&self) -> Result<User, ModuleError> {
        self.answered(Method::GET, "/users/@me", None).await
    }

    async fn member(&self, guild: &str, user: &str) -> Result<Member, ModuleError> {
        let path = format!("/guilds/{guild}/members/{user}");
        self.answered(Method::GET, &path, None).await
    }

    async fn guild(&self, guild: &str) -> Result<Guild, ModuleError> {
        let path = format!("/guilds/{guild}");
        self.answered(Method::GET, &path, None).await
    }

    async fn roles(&self, guild: &str) -> Result<Vec<Role>, ModuleError> {
        let path = format!("/guilds/{guild}/roles");
        self.answered(Method::GET, &path, None).await
    }

    async fn channels(&self, guild: &str) -> Result<Vec<GuildChannel>, ModuleError> {
        let path = format!("/guilds/{guild}/channels");
        self.answered(Method::GET, &path, None).await
    }

    async fn channel(&self, id: &str) -> Result<GuildChannel, ModuleError> {
        self.answered(Method::GET, &format!("/channels/{id}"), None)
            .await
    }

    async fn send(&self, channel: &str, message: &CreateMessage) -> Result<String, ModuleError> {
        let mut body = written(message)?;
        // No text mentions anyone: the list of what may is empty, which the
        // proto3 JSON mapping would leave out, so it is written here.
        body["allowed_mentions"] = value!({ "parse": [] });
        let path = format!("/channels/{channel}/messages");
        let sent: DiscordMessage = self.answered(Method::POST, &path, Some(&body)).await?;
        Ok(sent.id)
    }

    async fn give_role(&self, guild: &str, member: &str, role: &str) -> Result<(), ModuleError> {
        let path = format!("/guilds/{guild}/members/{member}/roles/{role}");
        self.call(Method::PUT, &path, None).await.map(|_| ())
    }

    async fn retag(&self, thread: &str, tags: &[String]) -> Result<(), ModuleError> {
        let body = written(&EditThread {
            applied_tags: tags.to_vec(),
            ..EditThread::default()
        })?;
        let path = format!("/channels/{thread}");
        self.call(Method::PATCH, &path, Some(&body))
            .await
            .map(|_| ())
    }

    async fn time_out(
        &self,
        guild: &str,
        member: &str,
        until: Option<&str>,
    ) -> Result<(), ModuleError> {
        let body = match until {
            Some(until) => written(&EditMember {
                communication_disabled_until: until.to_string(),
            })?,
            // A lift is a null, which the proto3 JSON mapping would leave
            // out.
            None => value!({ "communication_disabled_until": null }),
        };
        let path = format!("/guilds/{guild}/members/{member}");
        self.call(Method::PATCH, &path, Some(&body))
            .await
            .map(|_| ())
    }

    async fn lock(&self, thread: &str) -> Result<(), ModuleError> {
        let body = written(&EditThread {
            locked: true,
            archived: true,
            ..EditThread::default()
        })?;
        let path = format!("/channels/{thread}");
        self.call(Method::PATCH, &path, Some(&body))
            .await
            .map(|_| ())
    }

    async fn ban(&self, guild: &str, member: &str, delete_seconds: u32) -> Result<(), ModuleError> {
        let body = written(&CreateBan {
            delete_message_seconds: delete_seconds,
        })?;
        let path = format!("/guilds/{guild}/bans/{member}");
        self.call(Method::PUT, &path, Some(&body)).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests;
